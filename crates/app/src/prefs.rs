//! What somebody set, as opposed to what the app noticed.
//!
//! A small JSON file in the **config** directory `dirs.rs` names, which is the
//! whole distinction between this module and `recent.rs`: that one is *state* —
//! a list the app kept without being asked — and this is a choice a person
//! made. A backup of somebody's settings should carry this and not that.
//!
//! It is also deliberately not part of the board. A `.mbrd` travels: it is sent
//! to other people, and `settings.desktop` inside one is about the *board* —
//! whether it has a grid, how big its cells are. How much a particular person's
//! particular eyes want the camera to move is not a fact about a moodboard, and
//! writing it into one would mean handing it to everybody the file is sent to.
//!
//! Reading is best-effort and silent, like `recent.rs`: a config that cannot be
//! read is the defaults, because the alternative is an app that will not start
//! over a stray comma.

use std::path::PathBuf;

use serde_json::Value;

use crate::dirs;

/// What somebody has chosen.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Prefs {
    /// Whether the interface is allowed to move.
    ///
    /// Off means the camera arrives rather than travelling, the marks beside a
    /// card are simply there or not, and a picture appears when it has decoded.
    /// **It does not mean less feedback** — every one of those things still
    /// happens and still says the same thing, it just happens at once. That is
    /// the distinction the reduced-motion setting on every platform draws, and
    /// getting it wrong by also removing the feedback is the usual way of
    /// making the accessible path the worse one.
    pub motion: bool,

    /// Whether to find out that a new version exists.
    ///
    /// On by default, and off is a real answer rather than a delay: it stops
    /// the request being made at all, not just the message being shown. An app
    /// that keeps asking after being told not to has not been told not to.
    ///
    /// This is only about *looking*. Whether anything can then be installed is
    /// a property of how the app was installed rather than of what anybody
    /// chose — see `update/eligible.rs`.
    pub update: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Self { motion: true, update: true }
    }
}

impl Prefs {
    /// Whether the environment is overriding a setting, and which.
    ///
    /// A toggle that writes a choice the next launch will ignore is a toggle
    /// that lies, so whatever offers one has to be able to say "this is being
    /// forced elsewhere". Answers the variable's name, which is the only useful
    /// thing to tell somebody: it is what they have to go and unset.
    pub fn forced(motion: bool) -> Option<&'static str> {
        match motion {
            true if std::env::var_os("MBRD_MOTION").is_some() => Some("MBRD_MOTION"),
            false if std::env::var_os("MBRD_NO_UPDATE").is_some() => Some("MBRD_NO_UPDATE"),
            _ => None,
        }
    }
}

/// Where it lives.
fn store() -> Option<PathBuf> {
    Some(dirs::config()?.join("settings.json"))
}

/// What somebody has chosen, or the defaults.
///
/// The environment variable wins, and exists because a setting somebody needs
/// in order to look at the screen without feeling ill should not require them
/// to look at the screen first. `MBRD_MOTION=0` in a launcher, a shell profile
/// or a desktop entry is a way in that needs nothing from this app.
pub fn load() -> Prefs {
    let mut prefs = Prefs::default();

    if let Some(path) = store() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                if let Some(motion) = value.get("motion").and_then(Value::as_bool) {
                    prefs.motion = motion;
                }
                if let Some(update) = value.get("update").and_then(Value::as_bool) {
                    prefs.update = update;
                }
            }
        }
    }

    if let Some(set) = std::env::var_os("MBRD_MOTION") {
        prefs.motion = !off(&set);
    }

    // `MBRD_NO_UPDATE` rather than `MBRD_UPDATE`, because the thing anybody
    // wants from an environment variable here is to switch it *off* — in a
    // launcher, a desktop entry, or a build somebody is redistributing. Being
    // set at all is enough; its value is not read.
    if std::env::var_os("MBRD_NO_UPDATE").is_some() {
        prefs.update = false;
    }

    prefs
}

/// Write what somebody chose.
///
/// Best-effort and silent, like [`load`] and like `recent.rs`: a settings file
/// that cannot be written is a setting that does not persist, which is worth
/// less than an app that refuses to toggle it.
///
/// **Unknown keys are carried through.** The file is read back, the two keys
/// this build knows are replaced, and everything else is left exactly as it
/// arrived — the same bargain `mbrd-core` makes with the board format, and for
/// the same reason: a settings file written by a newer build should survive
/// being opened by an older one rather than being quietly trimmed to whatever
/// this binary happens to understand.
///
/// Note what this cannot do. `MBRD_MOTION` and `MBRD_NO_UPDATE` win at load,
/// so toggling a setting that an environment variable is forcing will write the
/// choice and then appear not to have taken effect on the next run. That is the
/// right precedence — a variable in a launcher or a desktop entry is a
/// deliberate override by whoever set it up — but it does mean the two can
/// disagree, which is why [`Prefs::forced`] exists to say so.
pub fn save(prefs: Prefs) {
    let Some(path) = store() else { return };

    // Read first, so that keys this build does not know about survive.
    let mut out = match std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
    {
        Some(Value::Object(map)) => map,
        _ => serde_json::Map::new(),
    };
    out.insert("motion".into(), Value::Bool(prefs.motion));
    out.insert("update".into(), Value::Bool(prefs.update));

    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, Value::Object(out).to_string());
}

/// Whether an environment variable is saying no.
fn off(value: &std::ffi::OsStr) -> bool {
    matches!(value.to_string_lossy().as_ref(), "0" | "off" | "false" | "no")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_is_on_until_somebody_says_otherwise() {
        assert!(Prefs::default().motion);
    }

    #[test]
    fn looking_for_updates_is_on_until_somebody_says_otherwise() {
        assert!(Prefs::default().update);
    }

    #[test]
    fn the_ways_of_saying_no_are_the_ones_people_write() {
        for no in ["0", "off", "false", "no"] {
            assert!(off(std::ffi::OsStr::new(no)), "{no} should have counted as off");
        }
        for yes in ["1", "on", "true", "yes", ""] {
            assert!(!off(std::ffi::OsStr::new(yes)), "{yes} should not have counted as off");
        }
    }

    #[test]
    fn saving_keeps_keys_this_build_does_not_know_about() {
        // The same bargain the board format makes: a settings file written by
        // a newer build must survive being opened by an older one rather than
        // being trimmed to whatever this binary understands.
        //
        // Exercised against the merge directly rather than the filesystem,
        // because `save` writes to the real config directory and a test that
        // did that would clobber whatever the person running it had chosen.
        let existing = serde_json::json!({ "motion": true, "theme": "midnight" });
        let mut out = match existing {
            Value::Object(map) => map,
            _ => unreachable!(),
        };
        out.insert("motion".into(), Value::Bool(false));
        out.insert("update".into(), Value::Bool(false));

        assert_eq!(out.get("theme").and_then(Value::as_str), Some("midnight"));
        assert_eq!(out.get("motion").and_then(Value::as_bool), Some(false));
        assert_eq!(out.get("update").and_then(Value::as_bool), Some(false));
    }

    #[test]
    fn an_environment_variable_that_is_not_set_forces_nothing() {
        // `forced` is what stops a toggle lying about having taken effect. In
        // a test environment neither variable is set, so both answer None —
        // and the point of the test is that it asks about *both*, so a third
        // preference added without a `forced` arm shows up here.
        assert_eq!(Prefs::forced(true), None);
        assert_eq!(Prefs::forced(false), None);
    }

    #[test]
    fn the_file_is_config_rather_than_state() {
        // The one thing about this module worth a test: `recent.rs` deliberately
        // writes under the *state* directory, and putting the two in the same
        // place would be the mistake that note exists to prevent.
        //
        // Asked of `dirs` rather than of the spelling of the path, because the
        // spelling is per-platform and on macOS the two are legitimately the
        // same directory — see `dirs.rs`. What has to hold everywhere is that
        // this module asks the config question and `recent.rs` asks the state
        // one, so that the split exists wherever the platform offers one.
        let path = store().expect("there is a home directory in a test environment");
        let config = dirs::config().expect("there is a home directory in a test environment");
        assert!(path.starts_with(&config), "it landed at {}", path.display());
        assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("settings.json"));
    }
}
