//! Whether this copy of the app can become the next version, and how.
//!
//! Most installs cannot do it the obvious way. A `.deb` puts the binary at
//! `/usr/bin/mbrd` and `dpkg` owns it from then on; a Flatpak runs from a
//! read-only mount; a `cargo build` in a checkout is somebody's working tree.
//! Writing over any of those is between rude and destructive, and the package
//! manager that finds a file it did not put there will say so at the worst
//! possible moment.
//!
//! So the question is asked before anything is downloaded, and the answer is a
//! [`Verdict`] rather than a `bool`, because it has three shapes rather than
//! two:
//!
//! - **replace it** — a portable binary, an AppImage, a `.app`, an installed
//!   `.exe`. The app owns the file and swaps it. See `install.rs`.
//! - **hand it over** — a `.deb` or `.rpm`, where the new version is the
//!   *package* and the tool that owns the file installs it. See `package.rs`.
//! - **say something** — everything else. **A refusal still has something to
//!   say**: "0.3.0 is out, update it through Flatpak" is more useful than
//!   silence and more honest than an install button that fails. This is also
//!   where the notify-only behaviour lives, which means it is a path with real
//!   users rather than a fallback nobody exercises.
//!
//! Everything here is pure — it is handed the facts rather than going to look
//! for them — which is what makes the table of cases testable on one machine.

use std::fmt;
use std::path::{Path, PathBuf};

use super::package::{self, Package};

/// What the app is running as, gathered once by [`Install::detect`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Install {
    /// The thing that would have to be replaced: the `.app` bundle on macOS,
    /// the AppImage file where there is one, the executable everywhere else.
    pub target: PathBuf,
    /// Whether a distribution's package manager put it there.
    pub packaged: bool,
    /// Which package format owns it, where that is knowable and where the
    /// package we publish would replace this exact file. See
    /// [`Package::owning`].
    pub package: Option<Package>,
    /// Whether there is a way to ask for the permission to install one.
    pub escalation: bool,
    /// Whether it is running inside a sandbox with its own update channel.
    pub sandboxed: Option<&'static str>,
    /// Whether the target can actually be written to.
    pub writable: bool,
    /// Whether this is a development build.
    pub development: bool,
}

/// What would be done, and to what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The file this install *is*, which is what gets replaced or what the
    /// package puts back — and either way what the restart reopens.
    pub target: PathBuf,
    pub how: How,
}

/// The two ways an install can become a newer one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    /// Move the new version over the old one ourselves.
    Replace,
    /// Give the package to the tool that owns the old one.
    Package(Package),
}

/// Whether an update can be installed, and if not, what to say instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Go ahead, this way.
    Go(Plan),
    /// Say a new version exists, and say this about getting it.
    Tell(String),
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Go(plan) => match plan.how {
                How::Replace => write!(f, "ready to install"),
                How::Package(package) => write!(f, "ready to install {}", package.label()),
            },
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

    if install.development {
        // Not a safety rule so much as a sanity one. A development build
        // replacing itself with a release would delete somebody's build
        // directory out from under a compiler — and a development build
        // asking for a password to install a package over itself is worse.
        return Verdict::Tell("this is a development build".into());
    }

    // Before the packaged refusal below, because this is the case where the
    // package manager can be *used* rather than merely deferred to.
    if let Some(package) = install.package {
        if install.escalation {
            return Verdict::Go(Plan {
                target: install.target.clone(),
                how: How::Package(package),
            });
        }
        // Nothing to ask for the permission with. The old sentence is still
        // the right one, and it is better said now than after a download.
        return Verdict::Tell("update it through your package manager".into());
    }

    if install.packaged {
        return Verdict::Tell("update it through your package manager".into());
    }

    if !install.writable {
        return Verdict::Tell(format!("{} is not writable by this user", install.target.display()));
    }

    Verdict::Go(Plan { target: install.target.clone(), how: How::Replace })
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

        let packaged = packaged(&target);
        // Only asked of an install a distribution owns, and only then. On
        // every other install — which is most of them, and all of Windows and
        // macOS — this costs nothing, because the question above has already
        // said no.
        let package = packaged.then(|| Package::owning(&target)).flatten();

        Some(Self {
            writable: writable(&target),
            packaged,
            escalation: package.is_some() && package::can_escalate(),
            package,
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
///
/// This says *that* something owns it, not *what* — the marker cannot say
/// which, because one build becomes both packages. See [`Package::owning`],
/// which answers the second question at runtime.
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
            package: None,
            escalation: false,
            sandboxed: None,
            writable: true,
            development: false,
        }
    }

    /// The shape of a `.deb` install on a machine with polkit.
    fn from_a_package(package: Package) -> Install {
        Install {
            target: PathBuf::from("/usr/bin/mbrd"),
            packaged: true,
            package: Some(package),
            escalation: true,
            writable: false,
            ..install()
        }
    }

    #[test]
    fn an_ordinary_install_may_replace_itself() {
        assert_eq!(
            verdict(&install()),
            Verdict::Go(Plan {
                target: PathBuf::from("/home/somebody/Apps/mbrd"),
                how: How::Replace
            })
        );
    }

    #[test]
    fn a_packaged_install_is_offered_its_own_package() {
        // The whole point of `package.rs`: `/usr/bin/mbrd` is not writable and
        // never will be, and the update for it is not a swap.
        for package in [Package::Deb, Package::Rpm] {
            assert_eq!(
                verdict(&from_a_package(package)),
                Verdict::Go(Plan {
                    target: PathBuf::from("/usr/bin/mbrd"),
                    how: How::Package(package)
                }),
                "a {package} install should be offered a {package}",
            );
        }
    }

    #[test]
    fn a_packaged_install_with_no_way_to_ask_is_told_instead() {
        // No polkit, so there is no way to install a package without a
        // terminal. Said before the download rather than after it.
        let no_polkit = Install { escalation: false, ..from_a_package(Package::Deb) };
        assert_eq!(
            verdict(&no_polkit),
            Verdict::Tell("update it through your package manager".into())
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
            Install { escalation: false, ..from_a_package(Package::Rpm) },
        ];
        for case in cases {
            match verdict(&case) {
                Verdict::Go(plan) => panic!("{case:?} should not have been installable: {plan:?}"),
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
    fn a_development_build_is_never_given_a_package_to_install() {
        // A checkout that somehow looks packaged — a debug build run out of
        // `/opt`, a container — must not reach the password prompt. The
        // development case is the one with the useful sentence anyway.
        let building = Install { development: true, ..from_a_package(Package::Deb) };
        assert_eq!(verdict(&building), Verdict::Tell("this is a development build".into()));
    }

    #[test]
    fn a_package_manager_is_named_before_a_permission_problem() {
        // `/usr/bin` is both, and the useful half is the package manager —
        // whether that means using it or being sent to it.
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
