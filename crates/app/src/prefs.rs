//! What somebody set, as opposed to what the app noticed.
//!
//! A small JSON file under the XDG **config** directory, which is the whole
//! distinction between this module and `recent.rs`: that one is *state* — a
//! list the app kept without being asked — and this is a choice a person made.
//! A backup of somebody's settings should carry this and not that.
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
}

impl Default for Prefs {
    fn default() -> Self {
        Self { motion: true }
    }
}

/// Where it lives.
fn store() -> Option<PathBuf> {
    let dir = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(config) if !config.is_empty() => PathBuf::from(config),
        // The spec's own fallback.
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".config"),
    };
    Some(dir.join("mbrd/settings.json"))
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
            }
        }
    }

    if let Some(set) = std::env::var_os("MBRD_MOTION") {
        prefs.motion = !matches!(set.to_string_lossy().as_ref(), "0" | "off" | "false" | "no");
    }

    prefs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn motion_is_on_until_somebody_says_otherwise() {
        assert!(Prefs::default().motion);
    }

    #[test]
    fn the_file_is_config_rather_than_state() {
        // The one thing about this module worth a test: `recent.rs` deliberately
        // writes under the *state* directory, and putting the two in the same
        // place would be the mistake that note exists to prevent.
        let path = store().expect("there is a home directory in a test environment");
        let shown = path.display().to_string();
        assert!(shown.contains(".config") || shown.contains("XDG"), "it landed at {shown}");
        assert!(!shown.contains(".local/state"), "it landed with the state at {shown}");
    }
}
