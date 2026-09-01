//! Handing a new version to `dpkg` or `rpm` instead of writing over it.
//!
//! This is the one install shape the app cannot replace by itself and must not
//! try. A `.deb` puts the binary at `/usr/bin/mbrd` and `dpkg` records a hash
//! for it; a `rename` over that file leaves the package database describing
//! something that is no longer there, and the next `apt upgrade` either
//! reverts the app or complains about it. So the update for those installs is
//! not a swap — it is *the package*, downloaded, verified against the same
//! signed manifest as everything else, and then given to the tool that owns
//! the file so the database and the disk go on agreeing.
//!
//! ## The three questions
//!
//! 1. **Which package is this?** [`Package::owning`], and it answers by
//!    looking at the filesystem rather than by running a package manager —
//!    see the note there. It is asked at launch on a packaged install, so it
//!    may not cost a subprocess.
//! 2. **Can we ask for the permission?** `pkexec` or nothing. Installing a
//!    package is root's business, and an app that is not running as root has
//!    exactly one polite way to ask on a desktop. Without it this whole path
//!    is off and the app goes back to saying "update it through your package
//!    manager", which is the honest answer on a machine with no polkit.
//! 3. **Which tool?** Whatever is installed, preferring the one that resolves
//!    dependencies — see [`command`]. A new release that needs a library the
//!    old one did not is the case `dpkg -i` gets wrong and `apt-get install`
//!    gets right.
//!
//! Everything above except the running of it is pure and testable: [`command`]
//! is handed a way to look a program up rather than looking it up itself.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context as _, Result};

/// Where our own packages put the binary.
///
/// Both `[package.metadata.deb]` and `[package.metadata.generate-rpm]` in
/// `crates/app/Cargo.toml` install to exactly this path, and the package path
/// is only offered for a target that *is* this path. That is what keeps the
/// offer honest: a package we build replaces this file, so if the running
/// binary is anywhere else — a tarball somebody unpacked into `/opt`, a build
/// somebody copied into `/usr/local/bin` — installing our package would put a
/// new version somewhere the running one is not, and the restart would come
/// back up on the old one having reported success.
const OWNED_PATH: &str = "/usr/bin/mbrd";

/// How the permission to install is asked for.
///
/// polkit, or nothing. `sudo` is not a candidate: it wants a terminal this app
/// does not have, and an app that pops up its own password box is an app
/// teaching people to type their password into whatever asks.
const ESCALATE: &str = "pkexec";

/// A Linux package format, which is to say a package manager to defer to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Package {
    Deb,
    Rpm,
}

impl Package {
    /// The extension, which is also the suffix on this platform's manifest key.
    ///
    /// The manifest lists `x86_64-unknown-linux-gnu` for the tarball and
    /// `x86_64-unknown-linux-gnu.deb` beside it — same triple, different
    /// shapes of the same release. See `manifest::key`.
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Deb => "deb",
            Self::Rpm => "rpm",
        }
    }

    /// What to call it in a sentence somebody reads.
    pub fn label(self) -> &'static str {
        match self {
            Self::Deb => "the .deb",
            Self::Rpm => "the .rpm",
        }
    }

    /// Which package format owns `target`, when that can be known cheaply.
    ///
    /// **Not by asking a package manager.** `dpkg -S` walks every file list on
    /// the machine and `rpm -qf` opens the rpm database; both are tens of
    /// milliseconds at best, and this question is asked during launch. The
    /// files below are the same answer without the subprocess:
    ///
    /// | | what it means |
    /// | --- | --- |
    /// | `/var/lib/dpkg/info/mbrd.list` | dpkg has an `mbrd` installed, and this is its file list |
    /// | `/var/lib/rpm`, `/usr/lib/sysimage/rpm` | an rpm database, so an rpm distribution |
    /// | `/var/lib/dpkg/status` | a dpkg distribution, without our package named in it |
    ///
    /// In that order, because the first is *this package* and the other two
    /// are only the family of the distribution. The order of the last two
    /// matters on the rare machine carrying both: rpm wins, because `rpm` on
    /// a Debian system is a tool somebody installed and `dpkg` on a Fedora
    /// system is the same, but only the first case has an exact signal above
    /// it to catch it first.
    ///
    /// A wrong answer here is not dangerous — the tool the wrong package is
    /// handed to will refuse it, visibly — but it is a wasted download, which
    /// is why the exact signal is checked first.
    pub fn owning(target: &Path) -> Option<Self> {
        if !cfg!(target_os = "linux") || target != Path::new(OWNED_PATH) {
            return None;
        }
        if Path::new("/var/lib/dpkg/info/mbrd.list").exists() {
            return Some(Self::Deb);
        }
        if Path::new("/var/lib/rpm").exists() || Path::new("/usr/lib/sysimage/rpm").exists() {
            return Some(Self::Rpm);
        }
        Path::new("/var/lib/dpkg/status").exists().then_some(Self::Deb)
    }
}

impl std::fmt::Display for Package {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.suffix())
    }
}

/// Whether this machine has a way to ask for the permission to install.
///
/// Asked before anything is offered rather than after something is
/// downloaded, so that a machine with no polkit gets the same sentence it got
/// before this module existed instead of thirty megabytes and a dead end.
pub fn can_escalate() -> bool {
    cfg!(target_os = "linux") && on_path(ESCALATE).is_some()
}

/// The command that installs `file`, or why there isn't one.
///
/// `resolve` is how a program is looked up, injected so that the table below
/// can be tested on a machine that has none of these tools — which is every
/// machine that is not the one distribution the case is about.
///
/// The preference within each format is the same both times: **the tool that
/// resolves dependencies first**, the low-level one second. `dpkg -i` and
/// `rpm -U` will happily install a package whose dependencies are not present
/// and leave the app unable to start; `apt-get` and `dnf` fetch what is
/// missing. The low-level tools are still worth keeping as a fallback,
/// because a machine without `apt-get` on `PATH` is unusual rather than
/// impossible, and the alternative to a half-resolved install is no install.
fn command(
    package: Package,
    file: &Path,
    resolve: impl Fn(&str) -> Option<PathBuf>,
) -> Result<Vec<OsString>> {
    let escalate = resolve(ESCALATE)
        .context("pkexec is not installed, so there is no way to ask to install a package")?;

    // Each candidate is the program and the arguments that make it
    // non-interactive, because there is no terminal to answer a question on
    // and a tool that stops to ask one would hang with nothing on screen.
    let candidates: &[(&str, &[&str])] = match package {
        Package::Deb => &[
            // A path rather than a package name, which is what makes this an
            // install of *this file*. apt requires the path to contain a
            // separator for that reading, and ours is absolute.
            ("apt-get", &["install", "-y"]),
            ("dpkg", &["--install"]),
        ],
        Package::Rpm => &[
            // `install` rather than `upgrade`: dnf reads a local file's
            // version and upgrades anyway, and `upgrade` on a package that is
            // somehow absent would do nothing at all.
            ("dnf", &["install", "-y"]),
            // openSUSE. Ours is unsigned — see RELEASING.md on why nothing here
            // carries a vendor signature — and zypper stops to ask about that
            // where dnf does not.
            ("zypper", &["--non-interactive", "install", "--allow-unsigned-rpm"]),
            ("rpm", &["--upgrade"]),
        ],
    };

    let (tool, arguments) = candidates
        .iter()
        .find_map(|(name, arguments)| Some((resolve(name)?, *arguments)))
        .with_context(|| {
            let names: Vec<&str> = candidates.iter().map(|(name, _)| *name).collect();
            format!("none of {} is installed, so {} cannot be installed", names.join(", "), package)
        })?;

    let mut argv = vec![escalate.into_os_string(), tool.into_os_string()];
    argv.extend(arguments.iter().map(OsString::from));
    argv.push(file.as_os_str().to_os_string());
    Ok(argv)
}

/// Install `file` through the system's package manager, asking for the
/// permission on the way.
///
/// Blocking, and blocking for as long as somebody takes to answer a password
/// prompt — so it belongs on the background executor with everything else in
/// this module's neighbourhood, and `board_view.rs` puts it there.
///
/// The child's output is captured rather than inherited, for two reasons. It
/// is the only thing worth putting in the error message when a package manager
/// refuses — a GUI app has no terminal for anybody to read the real complaint
/// in — and it closes the child's stdin, which is what stops `pkexec` falling
/// back to asking for a password on a terminal nobody is looking at and
/// waiting there for ever.
pub fn install(package: Package, file: &Path) -> Result<()> {
    let argv = command(package, file, on_path)?;

    let output = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .output()
        .with_context(|| format!("could not run {}", argv[0].to_string_lossy()))?;

    if output.status.success() {
        return Ok(());
    }

    // pkexec's own two failures, which are the common ones and are not
    // failures of the install at all — somebody closed the box, or this
    // account is not allowed to. Neither deserves a package manager's error
    // text, because there isn't one.
    match output.status.code() {
        Some(126) => bail!("the permission prompt was dismissed"),
        Some(127) => bail!("this account is not allowed to install packages here"),
        _ => {
            let complaint = last_line(&output.stderr).or_else(|| last_line(&output.stdout));
            let tool = argv[1].to_string_lossy().to_string();
            match complaint {
                Some(line) => bail!("{tool} refused: {line}"),
                None => bail!("{tool} failed with {}", output.status),
            }
        }
    }
}

/// The last thing a tool said before it gave up.
///
/// The last line rather than the first: `apt-get` and `dnf` both narrate for a
/// while and then say what was wrong, and the first line of that is usually
/// "Reading package lists...".
fn last_line(bytes: &[u8]) -> Option<String> {
    String::from_utf8_lossy(bytes)
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(|line| line.chars().take(200).collect())
}

/// Where a program is, if it is anywhere on `PATH`.
///
/// Thirty lines fewer than a `which` crate, and this is the only place in the
/// workspace that needs one.
fn on_path(program: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|dir| dir.join(program))
        .find(|path| is_program(path))
}

/// Whether that path is something this process could actually run.
///
/// The executable bit is checked rather than assumed: a `PATH` carrying a
/// directory with a *file* called `dnf` in it that nobody can run is a
/// misleading yes, and the cost of asking is one `stat` we have already done.
fn is_program(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        path.metadata().is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `resolve` that only knows about the programs it is given.
    fn only(names: &'static [&'static str]) -> impl Fn(&str) -> Option<PathBuf> {
        move |name| names.contains(&name).then(|| PathBuf::from("/usr/bin").join(name))
    }

    fn argv(package: Package, has: &'static [&'static str]) -> Vec<String> {
        command(package, Path::new("/tmp/mbrd.pkg"), only(has))
            .expect("these tools are enough")
            .iter()
            .map(|part| part.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn the_dependency_resolving_tool_is_preferred() {
        // The case this ordering exists for: a release that needs a library
        // the last one did not. `dpkg -i` installs it broken and `rpm -U`
        // refuses outright, and both are worse than apt or dnf fetching it.
        assert_eq!(
            argv(Package::Deb, &["pkexec", "apt-get", "dpkg"]),
            ["/usr/bin/pkexec", "/usr/bin/apt-get", "install", "-y", "/tmp/mbrd.pkg"]
        );
        assert_eq!(
            argv(Package::Rpm, &["pkexec", "dnf", "rpm"]),
            ["/usr/bin/pkexec", "/usr/bin/dnf", "install", "-y", "/tmp/mbrd.pkg"]
        );
    }

    #[test]
    fn the_low_level_tool_is_the_fallback() {
        assert_eq!(
            argv(Package::Deb, &["pkexec", "dpkg"]),
            ["/usr/bin/pkexec", "/usr/bin/dpkg", "--install", "/tmp/mbrd.pkg"]
        );
        assert_eq!(
            argv(Package::Rpm, &["pkexec", "rpm"]),
            ["/usr/bin/pkexec", "/usr/bin/rpm", "--upgrade", "/tmp/mbrd.pkg"]
        );
        // openSUSE, where the unsigned package needs saying so about.
        assert!(argv(Package::Rpm, &["pkexec", "zypper"]).contains(&"--allow-unsigned-rpm".into()));
    }

    #[test]
    fn every_command_ends_in_the_file_and_starts_in_the_escalation() {
        // The two ends that must not drift: something has to ask for the
        // permission, and the last argument has to be the file rather than a
        // package name — which is what makes this an install of *this*
        // download instead of whatever a repository happens to hold.
        for (package, has) in [
            (Package::Deb, &["pkexec", "apt-get"][..]),
            (Package::Deb, &["pkexec", "dpkg"][..]),
            (Package::Rpm, &["pkexec", "dnf"][..]),
            (Package::Rpm, &["pkexec", "zypper"][..]),
            (Package::Rpm, &["pkexec", "rpm"][..]),
        ] {
            let argv = argv(package, has);
            assert_eq!(argv.first().unwrap(), "/usr/bin/pkexec");
            assert_eq!(argv.last().unwrap(), "/tmp/mbrd.pkg");
        }
    }

    #[test]
    fn a_machine_with_no_way_to_ask_gets_no_command() {
        // Not a fallback to running the tool unprivileged, which would fail
        // at the end of a download instead of before one.
        let err = command(Package::Deb, Path::new("/tmp/mbrd.deb"), only(&["apt-get"]))
            .expect_err("without pkexec there is nothing to run");
        assert!(format!("{err}").contains("pkexec"), "{err}");
    }

    #[test]
    fn a_machine_with_no_package_manager_says_which_ones_it_looked_for() {
        let err = command(Package::Rpm, Path::new("/tmp/mbrd.rpm"), only(&["pkexec"]))
            .expect_err("pkexec alone installs nothing");
        let text = format!("{err}");
        for name in ["dnf", "zypper", "rpm"] {
            assert!(text.contains(name), "{text} does not mention {name}");
        }
    }

    #[test]
    fn only_the_path_our_packages_own_is_a_package_install() {
        // The rule that keeps the offer honest. Everything else on the list
        // is a place somebody put a binary themselves, and installing a
        // package would update a *different* file and then restart into the
        // old one.
        for path in [
            "/usr/local/bin/mbrd",
            "/opt/mbrd/mbrd",
            "/home/somebody/.local/bin/mbrd",
            "/usr/bin/mbrd-dev",
        ] {
            assert_eq!(Package::owning(Path::new(path)), None, "{path}");
        }
    }

    #[test]
    fn the_suffix_is_the_extension_and_the_manifest_key_both() {
        // `manifest::key` builds `x86_64-unknown-linux-gnu.deb` out of this,
        // and the release workflow writes that name by hand. A change here is
        // a change there.
        assert_eq!(Package::Deb.suffix(), "deb");
        assert_eq!(Package::Rpm.suffix(), "rpm");
    }

    #[test]
    fn a_tools_complaint_is_summarised_rather_than_dumped() {
        // What ends up in the status bar when dnf refuses. The narration
        // before it is not the reason.
        let noise =
            b"Last metadata expiration check: 0:12:00 ago.\nError: nothing provides libfoo\n";
        assert_eq!(last_line(noise).as_deref(), Some("Error: nothing provides libfoo"));
        assert_eq!(last_line(b"   \n\n").as_deref(), None);
        assert_eq!(last_line(b"").as_deref(), None);
        assert_eq!(last_line(&[b'x'; 500]).unwrap().chars().count(), 200);
    }
}
