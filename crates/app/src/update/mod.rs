//! Finding out that a new version exists, and becoming it.
//!
//! ```text
//!   look()  ── https ──▶  latest.json + .minisig
//!     │                        │
//!     │                   verify against the key built into this binary
//!     ▼                        │
//!   Found ◀────────────────────┘
//!     │
//!     ├── Tell   — say so; this install cannot become the new version itself
//!     └── Ready  — stage(), then apply(), then restart
//! ```
//!
//! ## Two ways of becoming the new version
//!
//! Most installs are a file this app owns, and `install.rs` swaps it. A `.deb`
//! or `.rpm` install is not: `dpkg` owns `/usr/bin/mbrd` and records a hash for
//! it, so the update for those is the *package*, downloaded through the same
//! signed manifest and handed to the tool that owns the file — `package.rs`.
//! `eligible.rs` decides which, before anything is downloaded, because the two
//! are different artifacts under different keys.
//!
//! ## What is trusted, and by what
//!
//! Nothing here is signed by Apple or Microsoft — see `RELEASING.md` — so the
//! ed25519 key compiled into this binary is the *entire* boundary between a
//! release download and arbitrary code execution. That shapes the module:
//!
//! - The key is absent by default. A build without `MBRD_UPDATE_KEY` set
//!   cannot install anything, which is the right behaviour for anybody
//!   building this themselves: they are not publishing a signed manifest, so
//!   an app of theirs that tried to install one would be an app that trusts a
//!   stranger's.
//! - The signature is checked over the manifest bytes before those bytes are
//!   parsed, let alone acted on. See `manifest.rs`.
//! - The artifact's URL and hash come from inside the signed manifest, and the
//!   URL is still checked against the hosts we publish from, because a
//!   signature says "we wrote this" and not "this is sensible".
//!
//! ## When it asks
//!
//! Once a day at most, never on the first run, and never at all if it has been
//! turned off. The first-run rule is not a technicality: an app whose opening
//! act is a network request has told you something about itself, and it has
//! nothing to report anyway.

pub mod eligible;
pub mod install;
pub mod manifest;
pub mod net;
pub mod package;
pub mod version;

use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};

use crate::dirs;
use eligible::{How, Install, Plan, Verdict};
use manifest::{Artifact, Manifest};
use version::Version;

/// The exact triple this binary was built for. See `build.rs`.
pub const TARGET: &str = env!("MBRD_TARGET");

/// The ed25519 public key this build trusts, if it was given one.
///
/// `None` in every build that did not set `MBRD_UPDATE_KEY` — which is every
/// build except the release workflow's, and deliberately so.
pub const KEY: Option<&str> = option_env!("MBRD_UPDATE_KEY");

/// Where to ask.
///
/// GitHub's `releases/latest/download/…` always resolves to the newest
/// published release, so this is one fixed URL that never has to be told what
/// the newest version is — which is convenient, and also means the client
/// never has to be trusted to work that out.
const MANIFEST_URL: &str = "https://github.com/omznc/mbrd-rs/releases/latest/download/latest.json";

/// How long between checks.
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);

/// What a look found.
#[derive(Debug)]
pub enum Found {
    /// This is the newest version, or there is nothing to say.
    Nothing,
    /// A new version exists and this install can become it. The [`Plan`] says
    /// how — a swap, or a package handed to `dpkg`.
    Ready { version: Version, artifact: Artifact, plan: Plan },
    /// A new version exists and this install cannot. Carries what to say
    /// instead — see `eligible.rs`, which is where the sentence comes from.
    Tell { version: Version, why: String },
}

/// The manifest key an install of this shape looks itself up under.
///
/// One triple, up to three artifacts: the tarball or bundle or `.exe` under
/// the bare triple, and the two Linux packages under it with a suffix. The
/// suffix rather than a separate map because the manifest's whole index is
/// "which download is mine", and a `.deb` install's answer is as much a
/// property of the install as its architecture is.
///
/// Written here rather than in `manifest.rs` so that the shape of an install
/// and the shape of the manifest meet in exactly one function; the drift test
/// in `manifest.rs` calls this one rather than spelling the names out again.
pub fn key(target: &str, how: How) -> String {
    match how {
        How::Replace => target.to_string(),
        How::Package(package) => format!("{target}.{}", package.suffix()),
    }
}

/// Whether this build is capable of updating at all.
///
/// Distinct from whether it is *allowed* to right now, which is
/// `eligible::verdict`. This one is about the binary: no key means no
/// verification means nothing will ever be installed, so there is no reason to
/// make the request.
pub fn possible() -> bool {
    KEY.is_some()
}

/// Whether to look, given what somebody has asked for and when we last did.
///
/// `by_hand` overrides the clock but not the switch: somebody who has turned
/// updates off and then pressed the key has contradicted themselves, and the
/// setting is the more considered of the two.
pub fn due(wanted: bool, by_hand: bool) -> bool {
    if !wanted || !possible() {
        return false;
    }
    if by_hand {
        return true;
    }
    match last_checked() {
        // Never checked. That is the first run, and the first run does not
        // ask — it only writes down that it was here, so that tomorrow's
        // launch is the first one that does.
        None => {
            remember_check();
            false
        }
        Some(last) => elapsed_since(last) >= EVERY,
    }
}

/// Ask, and work out what it means for this install. Blocking; call it on the
/// background executor.
pub fn look() -> Result<Found> {
    let trusted = KEY.context("this build has no update key and cannot verify a release")?;

    let json = net::fetch_small(MANIFEST_URL)?;
    let signature = net::fetch_small(&format!("{MANIFEST_URL}.minisig"))?;
    let manifest = Manifest::verify(json.as_bytes(), &signature, trusted)?;

    remember_check();

    if !manifest.is_newer_than(Version::current()) {
        return Ok(Found::Nothing);
    }
    let version = manifest.version;

    let Some(install) = Install::detect() else {
        return Ok(Found::Tell { version, why: "this install cannot be located".into() });
    };

    // The verdict first, because *which* artifact is on offer depends on it:
    // a `.deb` install is offered the `.deb` and not the tarball, and the two
    // sit under different keys for the same triple. See [`key`].
    let plan = match eligible::verdict(&install) {
        Verdict::Go(plan) => plan,
        Verdict::Tell(why) => return Ok(Found::Tell { version, why }),
    };

    match manifest.artifact_for(&key(TARGET, plan.how)) {
        Some(artifact) => Ok(Found::Ready { version, artifact: artifact.clone(), plan }),

        // A release that skipped this platform is not an error and not worth
        // mentioning — there is genuinely nothing on offer. The one case that
        // *is* worth a sentence is a packaged install and a release with no
        // package in it: this install could have been updated and the release
        // is why it was not, which is not something to be silent about.
        None => Ok(match plan.how {
            How::Replace => Found::Nothing,
            How::Package(package) => Found::Tell {
                version,
                why: format!(
                    "this release has no .{package} — update it through your package manager"
                ),
            },
        }),
    }
}

/// Download and check it, ready to be installed. Blocking; call it on the
/// background executor.
pub fn stage(
    artifact: &Artifact,
    version: Version,
    plan: &Plan,
    progress: impl FnMut(u64),
) -> Result<install::Staged> {
    install::stage(artifact, version, plan, progress)
}

/// Clear up after the last update, if there was one.
///
/// Cheap and silent, and called at launch: the thing it removes is the
/// previous version, which was still running at the moment it was displaced.
pub fn sweep() {
    if let Some(install) = Install::detect() {
        install::sweep(&install.target);
    }
}

// ---------------------------------------------------------------------------
// When we last asked
// ---------------------------------------------------------------------------

/// Where the stamp lives. *State* — something the app noticed — so the state
/// directory, next to the recent boards. See `dirs.rs`.
fn stamp() -> Option<PathBuf> {
    Some(dirs::state()?.join("update.json"))
}

fn last_checked() -> Option<SystemTime> {
    let text = std::fs::read_to_string(stamp()?).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let seconds = value.get("checked")?.as_u64()?;
    Some(UNIX_EPOCH + Duration::from_secs(seconds))
}

/// How long ago that was, treating a stamp in the future as "just now".
///
/// A clock that has been moved backwards — a laptop that resumed with a bad
/// time, a machine that just synced — would otherwise make every launch look
/// overdue and check on every one of them. Saturating means the worst that
/// happens is one skipped day.
fn elapsed_since(last: SystemTime) -> Duration {
    SystemTime::now().duration_since(last).unwrap_or(Duration::ZERO)
}

fn remember_check() {
    let Some(path) = stamp() else { return };
    let Ok(now) = SystemTime::now().duration_since(UNIX_EPOCH) else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Best-effort, like everything else that writes to the state directory: a
    // stamp that cannot be written means checking again tomorrow's launch
    // instead of the day after, which is not worth an error.
    let _ = std::fs::write(&path, serde_json::json!({ "checked": now.as_secs() }).to_string());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_build_without_a_key_never_looks() {
        // The safety property behind `possible`: a build that cannot verify a
        // manifest must not fetch one and act on it, and the check for that
        // is the same check that keeps somebody's own build quiet.
        if KEY.is_none() {
            assert!(!possible());
            assert!(!due(true, true), "it should not look even when asked by hand");
            assert!(look().is_err(), "and it should refuse outright if it somehow got here");
        } else {
            assert!(possible());
        }
    }

    #[test]
    fn turning_it_off_beats_asking_by_hand() {
        // Somebody who has set `"update": false` and then pressed the key has
        // contradicted themselves; the setting is the more considered half.
        assert!(!due(false, true));
        assert!(!due(false, false));
    }

    #[test]
    fn the_target_triple_is_a_real_one() {
        // `build.rs` fills this in, and a build that somehow lost it would
        // silently match nothing in every manifest forever.
        assert!(TARGET.contains('-'), "{TARGET} is not a target triple");
        assert!(!TARGET.is_empty());
    }

    #[test]
    fn a_package_install_looks_itself_up_under_its_own_key() {
        // Three artifacts share this triple and only one of them is the right
        // download. Handing a `.deb` install the tarball would put a binary
        // nowhere useful; handing a tarball install the `.deb` would ask for a
        // password it has no business asking for.
        use package::Package;
        assert_eq!(key(TARGET, How::Replace), TARGET);
        assert_eq!(key(TARGET, How::Package(Package::Deb)), format!("{TARGET}.deb"));
        assert_eq!(key(TARGET, How::Package(Package::Rpm)), format!("{TARGET}.rpm"));
    }

    #[test]
    fn a_clock_that_went_backwards_does_not_cause_a_check_every_launch() {
        let future = SystemTime::now() + Duration::from_secs(60 * 60 * 24 * 365);
        assert_eq!(elapsed_since(future), Duration::ZERO);
    }
}
