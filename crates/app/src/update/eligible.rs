//! Whether this copy of the app is allowed to replace itself.
//!
//! Most installs are not. A `.deb` puts the binary at `/usr/bin/mbrd` and
//! `dpkg` owns it from then on; a Flatpak runs from a read-only mount; a
//! `cargo build` in a checkout is somebody's working tree. Writing over any of
//! those is between rude and destructive, and the package manager that finds a
//! file it did not put there will say so at the worst possible moment.
//!
//! So the question is asked before anything is downloaded, and the answer is a
//! [`Verdict`] rather than a `bool`, because **a refusal still has something to
//! say**. "0.3.0 is out, run `dnf upgrade mbrd`" is more useful than silence
//! and more honest than an install button that fails. This is also where the
//! notify-only behaviour lives, which means it is a path with real users rather
//! than a fallback nobody exercises.
//!
//! Everything here is pure — it is handed the facts rather than going to look
//! for them — which is what makes the table of cases testable on one machine.

use std::fmt;
use std::path::{Path, PathBuf};

/// What the app is running as, gathered once by [`Install::detect`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Install {
    /// The thing that would have to be replaced: the `.app` bundle on macOS,
    /// the AppImage file where there is one, the executable everywhere else.
    pub target: PathBuf,
    /// Whether a distribution's package manager put it there.
    pub packaged: bool,
    /// Whether it is running inside a sandbox with its own update channel.
    pub sandboxed: Option<&'static str>,
    /// Whether the target can actually be written to.
    pub writable: bool,
    /// Whether this is a development build.
    pub development: bool,
}

/// Whether an update can be installed, and if not, what to say instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Go ahead. Carries what would be replaced.
    Install(PathBuf),
    /// Say a new version exists, and say this about getting it.
    Tell(String),
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Install(_) => write!(f, "ready to install"),
            Self::Tell(why) => write!(f, "{why}"),
        }
    }
}

/// The rules, in the order they are worth checking.
///
/// Ordered by how specific the advice is rather than by how likely the case
/// is: somebody inside a Flatpak *and* on a read-only mount should be told
/// about Flatpak, because that is the sentence with a next step in it.
pub fn verdict(install: &Install) -> Verdict {
    if let Some(sandbox) = install.sandboxed {
        return Verdict::Tell(format!("update it through {sandbox}"));
    }

    if install.packaged {
        return Verdict::Tell("update it through your package manager".into());
    }

    if install.development {
        // Not a safety rule so much as a sanity one. A development build
        // replacing itself with a release would delete somebody's build
        // directory out from under a compiler.
        return Verdict::Tell("this is a development build".into());
    }

    if !install.writable {
        return Verdict::Tell(format!("{} is not writable by this user", install.target.display()));
    }

    Verdict::Install(install.target.clone())
}

impl Install {
    /// Work out what this copy is, by looking.
    pub fn detect() -> Option<Self> {
        let exe = std::env::current_exe().ok()?;

        // An AppImage is mounted read-only at a path that is not the file
        // anybody has; `$APPIMAGE` is the file, and it is the thing to replace.
        // Checked before the bundle question because an AppImage has neither.
        let appimage = std::env::var_os("APPIMAGE").map(PathBuf::from);

        let target = match appimage {
            Some(path) => path,
            None => bundle_of(&exe).unwrap_or(exe),
        };

        Some(Self {
            writable: writable(&target),
            packaged: packaged(&target),
            sandboxed: sandboxed(),
            development: cfg!(debug_assertions),
            target,
        })
    }
}

/// The `.app` a macOS executable is inside, where it is inside one.
///
/// `…/mbrd.app/Contents/MacOS/mbrd` → `…/mbrd.app`. Replacing only the
/// executable would break the ad-hoc signature on the bundle around it, which
/// on Apple Silicon is the difference between an app that launches and one
/// macOS kills — so the bundle is the unit that gets swapped.
///
/// Not `#[cfg(target_os = "macos")]`: the path shape is what is being
/// recognised, and keeping it compiled everywhere is what lets the test below
/// run on the machine anybody is actually sitting at.
fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let macos = exe.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app")
        .then(|| bundle.to_path_buf())
}

/// Whether a distribution owns this file.
///
/// Two signals, because neither is enough alone. The build-time marker is set
/// by the `.deb` and `.rpm` builds and is exact but only present if those
/// builds remembered; the prefix check catches anything installed into a
/// system location by other means, including a `make install` and a
/// distribution that packaged this without asking.
fn packaged(target: &Path) -> bool {
    if option_env!("MBRD_PACKAGED").is_some() {
        return true;
    }
    ["/usr/bin", "/usr/local/bin", "/usr/lib", "/usr/share", "/opt", "/bin", "/snap"]
        .iter()
        .any(|prefix| target.starts_with(prefix))
}

/// The sandbox this is running in, if it is running in one.
fn sandboxed() -> Option<&'static str> {
    if std::env::var_os("FLATPAK_ID").is_some() || Path::new("/.flatpak-info").exists() {
        return Some("Flatpak");
    }
    if std::env::var_os("SNAP").is_some() {
        return Some("Snap");
    }
    None
}

/// Whether we could actually replace this.
///
/// The **directory** is what has to be writable, not the file: every swap in
/// `install.rs` is a `rename` within the parent, and a read-only file in a
/// writable directory can still be replaced while a writable file in a
/// read-only directory cannot. Asking the question of the file is the usual
/// way to get this backwards.
fn writable(target: &Path) -> bool {
    let Some(parent) = target.parent() else { return false };
    match parent.metadata() {
        Ok(meta) => !meta.permissions().readonly(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn install() -> Install {
        Install {
            target: PathBuf::from("/home/somebody/Apps/mbrd"),
            packaged: false,
            sandboxed: None,
            writable: true,
            development: false,
        }
    }

    #[test]
    fn an_ordinary_install_may_replace_itself() {
        assert_eq!(
            verdict(&install()),
            Verdict::Install(PathBuf::from("/home/somebody/Apps/mbrd"))
        );
    }

    #[test]
    fn every_refusal_says_something_useful() {
        // The property that matters more than which branch is taken: a
        // refusal is never silence, because somebody still wants the new
        // version and still has to be told how to get it.
        let cases = [
            Install { packaged: true, ..install() },
            Install { sandboxed: Some("Flatpak"), ..install() },
            Install { writable: false, ..install() },
            Install { development: true, ..install() },
        ];
        for case in cases {
            match verdict(&case) {
                Verdict::Install(_) => panic!("{case:?} should not have been installable"),
                Verdict::Tell(why) => {
                    assert!(!why.trim().is_empty(), "{case:?} refused without saying why");
                }
            }
        }
    }

    #[test]
    fn a_sandbox_is_named_before_anything_else() {
        // Somebody inside a Flatpak is also on a read-only mount and also
        // packaged. All three are true and only one of them has a next step.
        let confused =
            Install { sandboxed: Some("Flatpak"), packaged: true, writable: false, ..install() };
        assert_eq!(verdict(&confused), Verdict::Tell("update it through Flatpak".into()));
    }

    #[test]
    fn a_package_manager_is_named_before_a_permission_problem() {
        // `/usr/bin` is both, and "run dnf upgrade" is the useful half.
        let system = Install { packaged: true, writable: false, ..install() };
        assert_eq!(
            verdict(&system),
            Verdict::Tell("update it through your package manager".into())
        );
    }

    #[test]
    fn a_system_path_counts_as_packaged_without_a_marker() {
        for path in ["/usr/bin/mbrd", "/usr/local/bin/mbrd", "/opt/mbrd/mbrd", "/snap/mbrd/x/mbrd"]
        {
            assert!(packaged(Path::new(path)), "{path} should have counted as packaged");
        }
    }

    #[test]
    fn a_path_somebody_owns_does_not() {
        // The `/usr` check is a prefix match on path *components*, so a
        // directory that merely starts with the same letters is not caught.
        for path in ["/home/somebody/Apps/mbrd", "/home/somebody/usr/bin/mbrd", "/usrland/mbrd"] {
            assert!(!packaged(Path::new(path)), "{path} should not have counted as packaged");
        }
    }

    #[test]
    fn a_mac_executable_resolves_to_the_bundle_around_it() {
        assert_eq!(
            bundle_of(Path::new("/Applications/mbrd.app/Contents/MacOS/mbrd")),
            Some(PathBuf::from("/Applications/mbrd.app")),
            "the bundle is the unit that gets swapped, not the executable inside it"
        );
    }

    #[test]
    fn an_executable_that_is_not_in_a_bundle_resolves_to_nothing() {
        for path in [
            "/usr/bin/mbrd",
            "/home/somebody/mbrd",
            // The right depth, the wrong names — which is what a directory
            // called `Contents` somewhere unrelated would look like.
            "/somewhere/mbrd.zip/Contents/MacOS/mbrd",
            "/somewhere/mbrd.app/Resources/MacOS/mbrd",
            "/somewhere/mbrd.app/Contents/Helpers/mbrd",
        ] {
            assert_eq!(bundle_of(Path::new(path)), None, "{path} is not a bundle");
        }
    }
}
