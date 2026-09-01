//! Putting the new version where the old one was.
//!
//! Everything here happens after [`Manifest::verify`] has already said the
//! manifest is ours, so what is left is making sure the *bytes* are the ones
//! the manifest described and then moving them into place without leaving the
//! app in a state where it is neither version.
//!
//! [`Manifest::verify`]: super::manifest::Manifest::verify
//!
//! ## A swap stages beside the target, never in the cache
//!
//! The final step of every swap is a `rename`, which is atomic and cannot
//! cross filesystems. Staging in the cache directory and renaming into
//! `/opt` or `~/Applications` fails on any machine where those are separate
//! mounts, which on Linux is most of them and on macOS is any external disk.
//! So the download lands in a temporary directory *in the target's own
//! parent*.
//!
//! A package is the exception, and for the same reason turned around: nothing
//! is renamed, and the target's parent is `/usr/bin`, which this process
//! cannot write to and should not want to. That download goes to the cache
//! directory, which is what a cache directory is for — see [`stage`].
//!
//! ## The three swaps, and the thing that is not a swap
//!
//! | | what is replaced | how |
//! | --- | --- | --- |
//! | Linux | the executable, or the AppImage | one `rename` over it |
//! | macOS | the whole `.app` | move the old aside, move the new in |
//! | Windows | the `.exe` | rename the running file aside, move the new in |
//! | `.deb`, `.rpm` | nothing, by us | `pkexec apt-get install …` — see `package.rs` |
//!
//! Linux gets the good one. A `rename` over a running executable on Unix
//! replaces the directory entry and leaves the running process on the old
//! inode, so there is no instant at which the file is missing and nothing to
//! clean up afterwards.
//!
//! The other two cannot: `rename` will not replace a non-empty directory, and
//! Windows will not let a running image be overwritten at all. Both therefore
//! move the old one aside first, which leaves a `.old` to sweep — see
//! [`sweep`], which runs at the next launch rather than at the end of this one,
//! because at the end of this one the thing to delete is still running.

use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context as _, Result};
use sha2::{Digest, Sha256};

use super::eligible::{How, Plan};
use super::manifest::Artifact;
use super::package;
use super::{net, version::Version};

/// A verified update, waiting to be put where it belongs.
#[derive(Debug)]
pub struct Staged {
    pub version: Version,
    /// What will be moved into place, or handed to a package manager.
    new: PathBuf,
    /// What it will replace — the file this install *is*, which is also what
    /// the restart reopens whether we moved it or `dpkg` did.
    target: PathBuf,
    /// Which of those two.
    how: How,
    /// The temporary directory holding `new`, removed when this is dropped
    /// without being applied.
    scratch: PathBuf,
}

impl Drop for Staged {
    fn drop(&mut self) {
        // A staged update that was never applied is somebody having declined,
        // or the app closing. Either way the megabytes should not be left in
        // whatever directory the app lives in.
        if self.scratch.exists() {
            let _ = fs::remove_dir_all(&self.scratch);
        }
    }
}

/// The suffix a displaced old version wears until the next launch.
const DISPLACED: &str = ".old";

/// What the staging directory beside the app is called.
///
/// Dotted, because it sits in the directory the app lives in — next to
/// somebody's `Applications` folder or their `~/bin` — for as long as a
/// download takes.
const SCRATCH: &str = ".mbrd-update";

/// Download it, check it, and get it ready.
///
/// Nothing outside the scratch directory is touched — the app on disk is
/// exactly as it was when this returns, whether it succeeded or not, and for a
/// package nothing has been asked of the system either.
pub fn stage(
    artifact: &Artifact,
    version: Version,
    plan: &Plan,
    progress: impl FnMut(u64),
) -> Result<Staged> {
    // A fixed name rather than a random one, so that a run that is killed
    // between creating this and removing it leaves one directory to find
    // rather than one per attempt.
    let scratch = scratch_for(plan)?;
    if scratch.exists() {
        fs::remove_dir_all(&scratch).context("could not clear the last update attempt")?;
    }
    if let Some(parent) = scratch.parent() {
        // Only ever needed for the cache directory, which may not exist yet on
        // a machine that has never staged anything. The target's own parent
        // exists by definition — the app is in it.
        let _ = fs::create_dir_all(parent);
    }
    fs::create_dir(&scratch)
        .with_context(|| format!("could not create {} to stage the update", scratch.display()))?;

    // From here on any failure has to take the scratch directory with it, so
    // the work is done in a closure and the cleanup is unconditional.
    let staged = (|| -> Result<PathBuf> {
        let payload = scratch.join(payload_name(plan, version));
        let mut file =
            BufWriter::new(File::create(&payload).context("could not create the download")?);
        net::download(&artifact.url, artifact.size, &mut file, progress)?;
        file.flush().context("could not finish writing the download")?;
        drop(file);

        verify_hash(&payload, &artifact.sha256)?;
        unpack(&payload, &scratch, plan.how)
    })();

    match staged {
        Ok(new) => Ok(Staged { version, new, target: plan.target.clone(), how: plan.how, scratch }),
        Err(err) => {
            let _ = fs::remove_dir_all(&scratch);
            Err(err)
        }
    }
}

/// Where the download lands.
///
/// Beside the target for a swap, because the last step is a `rename` and a
/// `rename` cannot cross a filesystem. In the cache for a package, because
/// there is no rename — the file is read by a package manager and then thrown
/// away — and because the target's parent is `/usr/bin`, which this process
/// cannot write to and has no business trying.
fn scratch_for(plan: &Plan) -> Result<PathBuf> {
    match plan.how {
        How::Replace => {
            let parent = plan.target.parent().context("the running app has no parent directory")?;
            Ok(parent.join(SCRATCH))
        }
        How::Package(_) => {
            let cache = crate::dirs::cache().context("there is nowhere to put the download")?;
            Ok(cache.join("update"))
        }
    }
}

/// What the download is called on disk.
///
/// It matters for a package and only for a package: `apt-get` and `dnf` both
/// decide what they have been handed by looking at the extension, and both
/// refuse a file called `payload`. The version is in the name because it is
/// the name that ends up in a package manager's log.
fn payload_name(plan: &Plan, version: Version) -> String {
    match plan.how {
        How::Replace => "payload".into(),
        How::Package(package) => format!("mbrd-{version}.{}", package.suffix()),
    }
}

/// Confirm the bytes are the ones the manifest vouched for.
///
/// Streamed rather than read into memory: the payload is tens of megabytes and
/// there is no reason for all of it to be resident at once.
fn verify_hash(payload: &Path, expected: &str) -> Result<()> {
    let mut file = File::open(payload).context("could not reopen the download")?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).context("could not read the download")?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    let actual = hex(&hasher.finalize());
    ensure!(
        actual == expected,
        "the download does not match the manifest — expected {expected}, got {actual}"
    );
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
        use std::fmt::Write as _;
        let _ = write!(out, "{byte:02x}");
        out
    })
}

/// Turn the downloaded payload into the thing that will be moved into place,
/// or handed over.
///
/// The archive shape is decided by the plan rather than by sniffing, because
/// the plan comes from the signed manifest and the running install, and the
/// bytes are not yet anything we have agreed to interpret.
fn unpack(payload: &Path, scratch: &Path, how: How) -> Result<PathBuf> {
    // The Windows artifact is a bare executable and a package is a package —
    // there is nothing to unpack in either, and wrapping one file in an
    // archive to unwrap it again would be a step that exists only to be
    // symmetrical. Unpacking a `.deb` would be worse than pointless: taking it
    // apart ourselves is exactly the job we are handing to `dpkg`.
    if !payload_is_archive(how) {
        return Ok(payload.to_path_buf());
    }

    let file = File::open(payload).context("could not reopen the download")?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));

    let out = scratch.join("unpacked");
    fs::create_dir(&out).context("could not create the unpack directory")?;

    // Checked rather than trusted. `tar` refuses `..` on its own, but an
    // absolute path or a symlink pointing out of the tree is the classic way
    // an archive writes somewhere it was not invited, and the check is four
    // lines.
    for entry in archive.entries().context("the download is not a readable archive")? {
        let mut entry = entry.context("the archive has an unreadable entry")?;
        let path = entry.path().context("the archive has an unreadable path")?.into_owned();
        ensure!(!path.is_absolute(), "the archive contains an absolute path: {}", path.display());
        ensure!(
            !path.components().any(|c| matches!(c, std::path::Component::ParentDir)),
            "the archive tries to escape its directory: {}",
            path.display()
        );
        entry.unpack_in(&out).context("could not unpack the archive")?;
    }

    // Exactly one thing at the top: the `.app` bundle, or the binary. More
    // than one means the artifact is not shaped the way the release workflow
    // builds it, and picking one of them would be a guess.
    let mut top: Vec<PathBuf> = fs::read_dir(&out)
        .context("could not read the unpacked archive")?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .collect();
    top.sort();

    match top.len() {
        1 => Ok(top.remove(0)),
        0 => bail!("the archive is empty"),
        n => bail!("the archive holds {n} things at the top level, expected one"),
    }
}

/// Whether this payload is an archive rather than the file itself.
fn payload_is_archive(how: How) -> bool {
    // Windows ships the `.exe` bare and Linux packages ship as themselves;
    // macOS and the Linux tarball ship a `.tar.gz`, because a `.app` is a
    // directory and a Unix binary has a mode bit to preserve.
    matches!(how, How::Replace) && !cfg!(windows)
}

impl Staged {
    /// Whether applying this will ask somebody for a password.
    ///
    /// Worth saying before it happens rather than after: a prompt that arrives
    /// unannounced looks like something else on the machine wanting a password,
    /// which is the exact instinct nobody should be training away.
    pub fn asks_for_permission(&self) -> bool {
        matches!(self.how, How::Package(_))
    }

    /// Put it where it belongs.
    ///
    /// After this returns the app on disk is the new version and the running
    /// process is the old one, which is why the only sensible thing to do next
    /// is restart.
    ///
    /// Blocking for as long as somebody takes to answer a password prompt when
    /// there is one — so on the package path this belongs on the background
    /// executor, and `board_view.rs` puts it there.
    pub fn apply(mut self) -> Result<()> {
        let target = self.target.clone();
        let new = self.new.clone();

        if self.asks_for_permission() {
            self.hand_over()?;
            let _ = fs::remove_dir_all(&self.scratch);
            self.scratch = PathBuf::new();
            return Ok(());
        }

        if replace_in_one_step(&target) {
            // Unix, a plain file. `rename` over it is atomic, leaves the
            // running process on the old inode, and leaves nothing behind.
            make_executable(&new)?;
            fs::rename(&new, &target)
                .with_context(|| format!("could not move the update over {}", target.display()))?;
        } else {
            // A directory, or Windows. The old one has to move first.
            let displaced = displaced_name(&target)?;
            if displaced.exists() {
                remove(&displaced)
                    .with_context(|| format!("could not clear {}", displaced.display()))?;
            }
            fs::rename(&target, &displaced)
                .with_context(|| format!("could not move {} aside", target.display()))?;

            if let Err(err) = fs::rename(&new, &target) {
                // Put it back. Failing here would leave no app at all, which
                // is the one outcome worth unwinding for.
                let _ = fs::rename(&displaced, &target);
                return Err(err).with_context(|| {
                    format!("could not move the update into {}", target.display())
                });
            }
            make_executable(&target)?;
        }

        // The scratch directory has served its purpose, and `new` has moved
        // out of it. Cleared here rather than by `Drop`, which would otherwise
        // run after a successful apply and find nothing.
        let _ = fs::remove_dir_all(&self.scratch);
        self.scratch = PathBuf::new();
        Ok(())
    }

    /// Hand it to the package manager, keeping the download.
    ///
    /// The package half of [`apply`], with the ownership the other way round,
    /// and that is the whole reason it exists separately: the likeliest
    /// failure on this path is somebody closing the password box, and a
    /// download thrown away for *that* would have to be fetched all over
    /// again to press the button a second time. `board_view.rs` holds the
    /// `Staged` across the call and puts it back on failure.
    ///
    /// Not our file to move, on this path. `dpkg` or `rpm` replaces it and
    /// updates its own record of what is installed, so the database and the
    /// disk go on agreeing — which is the whole reason this path exists rather
    /// than a `rename` that would work fine once and be wrong for ever after.
    pub fn hand_over(&self) -> Result<()> {
        match self.how {
            How::Package(package) => package::install(package, &self.new),
            // Only reachable from a caller that ignored
            // `asks_for_permission`, which is worth saying out loud rather
            // than quietly doing the other thing.
            How::Replace => bail!("this update is a swap, not a package"),
        }
    }

    /// What it will replace, for a message.
    pub fn target(&self) -> &Path {
        &self.target
    }
}

/// Whether the swap can be one atomic `rename`.
///
/// Only when the target is a plain file on a Unix that lets a running
/// executable's directory entry be replaced. A `.app` is a directory —
/// `rename` will not replace a non-empty one — and Windows will not let the
/// running image be overwritten at all.
fn replace_in_one_step(target: &Path) -> bool {
    !cfg!(windows) && target.is_file()
}

/// `mbrd.app` → `mbrd.app.old`.
///
/// Built from the whole file name rather than with `with_extension`, which
/// would turn `mbrd.app` into `mbrd.old` and `mbrd.exe` into `mbrd.old` —
/// losing which of them it was, and colliding if both ever sat side by side.
fn displaced_name(target: &Path) -> Result<PathBuf> {
    let parent = target.parent().context("the target has no parent directory")?;
    let name = target.file_name().context("the target has no name")?;
    let mut name = name.to_os_string();
    name.push(DISPLACED);
    Ok(parent.join(name))
}

fn remove(path: &Path) -> std::io::Result<()> {
    if path.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    }
}

/// Make sure the thing about to become the app can be run.
///
/// A `tar` built by the release workflow already carries the mode, but a
/// payload that lost it — repacked, or fetched through something that
/// normalises permissions — would install an application nobody can start, and
/// the failure would come at the next launch with no clue attached.
#[cfg(unix)]
fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;

    // A bundle's executable is inside it; the bundle itself is a directory and
    // is already traversable.
    if path.is_dir() {
        return Ok(());
    }
    let mut permissions =
        path.metadata().context("could not read the update's mode")?.permissions();
    permissions.set_mode(permissions.mode() | 0o755);
    fs::set_permissions(path, permissions).context("could not make the update executable")
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<()> {
    Ok(())
}

/// Delete what the last update left behind.
///
/// Called at launch, because the thing to delete is the previous version and
/// at the moment it was displaced it was still running. Silent and
/// best-effort: a leftover that cannot be removed is wasted disk and nothing
/// else, and it will be tried again next time.
pub fn sweep(target: &Path) {
    if let Ok(displaced) = displaced_name(target) {
        if displaced.exists() {
            let _ = remove(&displaced);
        }
    }
    if let Some(parent) = target.parent() {
        let scratch = parent.join(SCRATCH);
        if scratch.exists() {
            let _ = fs::remove_dir_all(&scratch);
        }
    }
    // And the other one. A package that was downloaded and then not installed
    // — the prompt dismissed, the app closed — leaves tens of megabytes in the
    // cache directory that `Drop` would have taken if the process had lived
    // long enough to run it.
    if let Some(cache) = crate::dirs::cache() {
        let staged = cache.join("update");
        if staged.exists() {
            let _ = fs::remove_dir_all(&staged);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory this test owns, named after the test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mbrd-install-test-{name}"));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temporary directory");
        dir
    }

    #[test]
    fn a_displaced_name_keeps_the_whole_original_name() {
        // `with_extension` would turn both of these into `mbrd.old`, which
        // loses which one it was and collides if both ever sit side by side.
        assert_eq!(
            displaced_name(Path::new("/Applications/mbrd.app")).unwrap(),
            PathBuf::from("/Applications/mbrd.app.old")
        );
        // Forward slashes even for the Windows case: a backslash is not a
        // separator on Unix, so `C:\Apps\mbrd.exe` would be one long file
        // name here and the test would be measuring the wrong thing.
        assert_eq!(
            displaced_name(Path::new("/Apps/mbrd.exe")).unwrap(),
            PathBuf::from("/Apps/mbrd.exe.old")
        );
        assert_eq!(
            displaced_name(Path::new("/home/somebody/bin/mbrd")).unwrap(),
            PathBuf::from("/home/somebody/bin/mbrd.old")
        );
    }

    /// The two plans, for the tests that care which one they are given.
    fn replacing(target: &Path) -> Plan {
        Plan { target: target.to_path_buf(), how: How::Replace }
    }

    fn packaged(package: super::package::Package) -> Plan {
        Plan { target: PathBuf::from("/usr/bin/mbrd"), how: How::Package(package) }
    }

    #[test]
    fn a_package_is_staged_in_the_cache_and_a_swap_beside_the_app() {
        // The distinction the whole module is arranged around. Staging a
        // package beside the app would mean writing to `/usr/bin`, which is
        // the thing this path exists to avoid; staging a swap in the cache
        // would mean a `rename` across filesystems, which fails on most
        // machines and on none of the ones it is likely to be tested on.
        let app = PathBuf::from("/home/somebody/Apps/mbrd");
        assert_eq!(
            scratch_for(&replacing(&app)).unwrap(),
            PathBuf::from("/home/somebody/Apps/.mbrd-update")
        );

        let staged = scratch_for(&packaged(super::package::Package::Deb)).unwrap();
        assert!(!staged.starts_with("/usr"), "{} is not ours to write to", staged.display());
        assert!(staged.ends_with("mbrd/update"), "{}", staged.display());
    }

    #[test]
    fn a_package_keeps_the_extension_its_installer_reads() {
        // `apt-get install /tmp/payload` refuses the file for having no `.deb`
        // on the end of it, and so does dnf. This is the only thing about the
        // name that matters, and it is invisible until an install fails.
        let version = Version::parse("0.3.0").unwrap();
        for package in [super::package::Package::Deb, super::package::Package::Rpm] {
            let name = payload_name(&packaged(package), version);
            assert_eq!(name, format!("mbrd-0.3.0.{package}"));
        }
        assert_eq!(payload_name(&replacing(Path::new("/x/mbrd")), version), "payload");
    }

    #[test]
    fn only_a_package_warns_that_it_will_ask_for_a_password() {
        // What `board_view.rs` says before it starts, so that a polkit box
        // arrives explained rather than out of nowhere.
        let staged = |how| Staged {
            version: Version::parse("0.3.0").unwrap(),
            new: PathBuf::from("/nowhere/new"),
            target: PathBuf::from("/nowhere/mbrd"),
            how,
            // Does not exist, so `Drop` has nothing to remove.
            scratch: PathBuf::from("/nowhere/.mbrd-update"),
        };
        assert!(staged(How::Package(super::package::Package::Deb)).asks_for_permission());
        assert!(!staged(How::Replace).asks_for_permission());
    }

    #[test]
    fn a_package_is_handed_over_whole() {
        // Not unpacked, not inspected. A `.deb` is an archive and taking it
        // apart is exactly the job being handed to `dpkg`; a payload that
        // arrived here already unpacked would be a payload no installer could
        // read.
        let dir = scratch("package-unpack");
        let payload = dir.join("mbrd-0.3.0.deb");
        fs::write(&payload, b"not really a package").unwrap();

        let out = unpack(&payload, &dir, How::Package(super::package::Package::Deb)).unwrap();
        assert_eq!(out, payload);
        assert!(!payload_is_archive(How::Package(super::package::Package::Rpm)));

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_hash_that_does_not_match_is_refused() {
        let dir = scratch("hash");
        let payload = dir.join("payload");
        fs::write(&payload, b"hello").unwrap();

        // sha256("hello")
        let right = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        verify_hash(&payload, right).expect("the right hash should pass");

        let wrong = "0000000000000000000000000000000000000000000000000000000000000000";
        assert!(verify_hash(&payload, wrong).is_err(), "the wrong hash should not");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sweeping_removes_what_an_update_left_and_nothing_else() {
        let dir = scratch("sweep");
        let target = dir.join("mbrd");
        fs::write(&target, b"the app").unwrap();
        fs::write(dir.join("mbrd.old"), b"the previous app").unwrap();
        fs::create_dir(dir.join(".mbrd-update")).unwrap();
        fs::write(dir.join(".mbrd-update/payload"), b"half a download").unwrap();
        // A neighbour, to prove the sweep is aimed rather than broad.
        fs::write(dir.join("notes.mbrd"), b"somebody's board").unwrap();

        sweep(&target);

        assert!(target.exists(), "it removed the app itself");
        assert!(!dir.join("mbrd.old").exists(), "it left the previous version");
        assert!(!dir.join(".mbrd-update").exists(), "it left a half-finished download");
        assert!(dir.join("notes.mbrd").exists(), "it removed something that was not its business");

        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sweeping_a_clean_directory_does_nothing_and_says_nothing() {
        let dir = scratch("clean");
        let target = dir.join("mbrd");
        fs::write(&target, b"the app").unwrap();

        sweep(&target); // The ordinary launch: there is nothing to sweep.

        assert!(target.exists());
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_file_target_is_replaced_in_one_step() {
        let dir = scratch("swap-file");
        let target = dir.join("mbrd");
        fs::write(&target, b"old version").unwrap();

        let staged = Staged {
            version: Version::parse("0.3.0").unwrap(),
            new: {
                let scratch = dir.join(".mbrd-update");
                fs::create_dir(&scratch).unwrap();
                let new = scratch.join("payload");
                fs::write(&new, b"new version").unwrap();
                new
            },
            target: target.clone(),
            how: How::Replace,
            scratch: dir.join(".mbrd-update"),
        };

        staged.apply().expect("the swap should work");

        assert_eq!(fs::read(&target).unwrap(), b"new version");
        // The good case leaves nothing at all behind — no `.old`, no scratch.
        assert!(!dir.join("mbrd.old").exists(), "a file swap should not displace anything");
        assert!(!dir.join(".mbrd-update").exists(), "the scratch directory should be gone");

        use std::os::unix::fs::PermissionsExt as _;
        let mode = target.metadata().unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "the new app is not executable");

        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_directory_target_displaces_the_old_one() {
        // The macOS shape: the target is a `.app`, which `rename` will not
        // replace, so the old one has to move aside and be swept later.
        let dir = scratch("swap-bundle");
        let target = dir.join("mbrd.app");
        fs::create_dir_all(target.join("Contents/MacOS")).unwrap();
        fs::write(target.join("Contents/MacOS/mbrd"), b"old version").unwrap();

        let scratch_dir = dir.join(".mbrd-update");
        let new = scratch_dir.join("unpacked/mbrd.app");
        fs::create_dir_all(new.join("Contents/MacOS")).unwrap();
        fs::write(new.join("Contents/MacOS/mbrd"), b"new version").unwrap();

        let staged = Staged {
            version: Version::parse("0.3.0").unwrap(),
            new,
            target: target.clone(),
            how: How::Replace,
            scratch: scratch_dir.clone(),
        };
        staged.apply().expect("the swap should work");

        assert_eq!(fs::read(target.join("Contents/MacOS/mbrd")).unwrap(), b"new version");
        let displaced = dir.join("mbrd.app.old");
        assert!(displaced.exists(), "the old bundle should still be there to sweep");
        assert_eq!(fs::read(displaced.join("Contents/MacOS/mbrd")).unwrap(), b"old version");

        // And the next launch takes it away.
        sweep(&target);
        assert!(!displaced.exists());
        assert!(!scratch_dir.exists());

        fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_swap_puts_the_old_one_back() {
        // The one outcome worth unwinding for: if the second rename fails,
        // the first has already moved the app out of the way and there would
        // otherwise be no app at all.
        let dir = scratch("swap-unwind");
        let target = dir.join("mbrd.app");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("marker"), b"old version").unwrap();

        let scratch_dir = dir.join(".mbrd-update");
        fs::create_dir(&scratch_dir).unwrap();
        let staged = Staged {
            version: Version::parse("0.3.0").unwrap(),
            // Does not exist, so the second rename cannot succeed.
            new: scratch_dir.join("missing"),
            target: target.clone(),
            how: How::Replace,
            scratch: scratch_dir.clone(),
        };

        assert!(staged.apply().is_err(), "it should have failed");
        assert!(target.exists(), "the app was left missing");
        assert_eq!(fs::read(target.join("marker")).unwrap(), b"old version");
        assert!(!dir.join("mbrd.app.old").exists(), "the displaced copy was left behind");

        fs::remove_dir_all(&dir).ok();
    }
}
