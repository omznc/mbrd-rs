//! Where this app is allowed to put things, on each of the three platforms.
//!
//! `prefs.rs` and `recent.rs` each used to work this out for themselves, from
//! `XDG_CONFIG_HOME` and `XDG_STATE_HOME` with a `$HOME` fallback. That is
//! correct on Linux and wrong twice over elsewhere: macOS has never used the
//! XDG variables, and Windows sets neither them nor `HOME` — so on Windows the
//! fallback returned `None`, every read was the defaults and every write went
//! nowhere. Silently, which is the right behaviour for a config file that is
//! not there yet and the wrong behaviour for a whole operating system.
//!
//! So the three questions are asked here, once:
//!
//! | | Linux | macOS | Windows |
//! | --- | --- | --- | --- |
//! | [`config`] | `$XDG_CONFIG_HOME/mbrd` | `~/Library/Application Support/mbrd` | `%APPDATA%\mbrd` |
//! | [`state`] | `$XDG_STATE_HOME/mbrd` | `~/Library/Application Support/mbrd` | `%LOCALAPPDATA%\mbrd` |
//! | [`cache`] | `$XDG_CACHE_HOME/mbrd` | `~/Library/Caches/mbrd` | `%LOCALAPPDATA%\mbrd\cache` |
//! | [`boards`] | `~/mbrd` | `~/mbrd` | `%USERPROFILE%\mbrd` |
//!
//! [`boards`] is the odd one and deliberately so: the other three are places
//! the *platform* set aside for applications, and a board is a document. It
//! goes somewhere a person can find it, open it from a file manager and hand to
//! somebody else, which on all three platforms means a plainly named folder in
//! the home directory.
//!
//! ## Why config and state are two functions and sometimes one directory
//!
//! The distinction is the one `prefs.rs` and `recent.rs` are built on: *config*
//! is a choice somebody made and *state* is something the app noticed, and a
//! backup of the first should not carry the second. Linux and Windows can both
//! express that. macOS cannot — one `Application Support` directory per
//! application, and no convention for splitting it — so there the two functions
//! return the same path.
//!
//! Keeping them as two functions anyway is the point. The callers go on saying
//! which kind of thing they are storing, the promise is kept where the platform
//! can keep it, and where it cannot it is honestly unenforceable rather than
//! quietly abandoned everywhere.
//!
//! ## Why not the `dirs` crate
//!
//! This is forty lines of `env::var_os`. The workspace has eight direct
//! dependencies and every one of them is a decision; this is not worth being
//! the ninth.
//!
//! Nothing here creates anything. A caller about to write is the one that knows
//! whether the directory is worth making, and every caller so far is
//! best-effort enough that a failed `create_dir_all` is not an error.

use std::path::PathBuf;

/// The directory name every one of these ends in.
const APP: &str = "mbrd";

/// A path under the home directory, where there is one.
#[cfg(not(windows))]
fn home(join: &str) -> Option<PathBuf> {
    Some(home_dir()?.join(join))
}

/// The home directory itself.
///
/// `HOME` everywhere but Windows, which has never set it — see the module note,
/// which is about this exact mistake being made twice already. `USERPROFILE` is
/// what Windows calls the same thing.
fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    let key = "USERPROFILE";
    #[cfg(not(windows))]
    let key = "HOME";
    from_env(key)
}

/// An environment variable holding a path, if it holds one at all.
///
/// Empty counts as unset, which is what the XDG specification says and what an
/// exported-but-never-assigned variable in a shell profile looks like from
/// here. Treating it as a path would put the file at the filesystem root.
fn from_env(key: &str) -> Option<PathBuf> {
    match std::env::var_os(key) {
        Some(value) if !value.is_empty() => Some(PathBuf::from(value)),
        _ => None,
    }
}

/// What somebody chose. See `prefs.rs`.
pub fn config() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    let dir = from_env("XDG_CONFIG_HOME").or_else(|| home(".config"))?.join(APP);

    #[cfg(target_os = "macos")]
    let dir = home("Library/Application Support")?.join(APP);

    #[cfg(windows)]
    let dir = from_env("APPDATA")?.join(APP);

    // Every other Unix — the BSDs — follows the XDG layout, and landing there
    // is better than returning nothing.
    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    let dir = from_env("XDG_CONFIG_HOME").or_else(|| home(".config"))?.join(APP);

    Some(dir)
}

/// Where somebody's own themes go. See `themes.rs`.
///
/// Under [`config`] rather than beside it, because a theme is a choice in
/// exactly the sense that module's note means: somebody wrote it or somebody
/// downloaded it, and a backup of their settings should carry it. The plural
/// directory rather than a key in `settings.json` because a theme is a
/// *document* — it is written in an editor, sent to other people, and dropped
/// in by hand, none of which is true of a boolean.
pub fn themes() -> Option<PathBuf> {
    Some(config()?.join("themes"))
}

/// What the app noticed. See `recent.rs`.
pub fn state() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    let dir = from_env("XDG_STATE_HOME").or_else(|| home(".local/state"))?.join(APP);

    // The same directory [`config`] returns, deliberately. See the note above.
    #[cfg(target_os = "macos")]
    let dir = home("Library/Application Support")?.join(APP);

    #[cfg(windows)]
    let dir = from_env("LOCALAPPDATA")?.join(APP);

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    let dir = from_env("XDG_STATE_HOME").or_else(|| home(".local/state"))?.join(APP);

    Some(dir)
}

/// Where a board this app makes for you is put.
///
/// `~/mbrd`, on every platform. **Not** under [`state`] or [`config`]: those
/// are for things the app keeps about itself, and a board is somebody's work —
/// hiding it in an application data directory would mean the only way back to
/// it is this app's own switcher, which is a poor thing to be true of a file
/// somebody spent an afternoon on.
///
/// Only boards *this app creates* land here — the new-board button, and the
/// first save of a board that has never had a file. A board opened from
/// anywhere else stays exactly where it was.
///
/// Nothing here creates the directory; see the module note. The caller about to
/// write is the one that knows whether it is worth making.
///
/// **This is the answer for somebody who has not chosen one.** It is the only
/// one of the five that a person can override — the other four are places the
/// platform set aside and not anybody's business — so most callers want
/// `Prefs::boards`, which falls through to here. See `prefs.rs`, whose
/// `boards_dir` note is about why the unchosen case stays a `None` there
/// rather than being written down as this path.
pub fn boards() -> Option<PathBuf> {
    Some(home_dir()?.join(APP))
}

/// What can be thrown away without losing anything.
///
/// Where `pipeline.rs` lays a played file out for the media stack to open, and
/// the first caller this had — it was written empty, on the argument that the
/// next thing wanting somewhere to put a file would otherwise work it out for
/// itself. Everything under here is rebuildable from a `.mbrd` that still
/// exists, which is what the word cache is claiming.
///
/// The second caller is `update/install.rs`, which stages a `.deb` or `.rpm`
/// here on its way to the package manager. Every other update is staged beside
/// the app it replaces, because that swap ends in a `rename` and a `rename`
/// cannot cross a filesystem — a package is not renamed anywhere, so it belongs
/// in the discardable place instead. That caller is on every platform, which is
/// why the `allow(dead_code)` this used to carry for the ones that swap in
/// `pipeline_off.rs` is gone: there is nowhere left for it to be dead.
pub fn cache() -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    let dir = from_env("XDG_CACHE_HOME").or_else(|| home(".cache"))?.join(APP);

    #[cfg(target_os = "macos")]
    let dir = home("Library/Caches")?.join(APP);

    // Windows has no separate cache location. `LOCALAPPDATA` is already the
    // machine-local, not-worth-roaming half of the split, so a subdirectory of
    // ours is as close as the platform gets to saying "discardable".
    #[cfg(windows)]
    let dir = from_env("LOCALAPPDATA")?.join(APP).join("cache");

    #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
    let dir = from_env("XDG_CACHE_HOME").or_else(|| home(".cache"))?.join(APP);

    Some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_lands_under_a_directory_of_ours() {
        // The one thing a caller relies on and cannot check for itself. Writing
        // `recent.json` straight into `~/.config` would be this module's worst
        // possible bug, and it would look like it had worked.
        for dir in [config(), state(), cache(), boards()] {
            let dir = dir.expect("a test environment has a home directory");
            assert!(
                dir.components().any(|c| c.as_os_str() == APP),
                "{} is not under an mbrd directory",
                dir.display()
            );
        }
    }

    #[test]
    fn config_and_state_differ_where_the_platform_can_say_so() {
        // The distinction `prefs.rs` and `recent.rs` are built on. macOS is
        // excluded because it genuinely cannot express it — see the note above
        // — and asserting it there would be asserting a lie.
        if cfg!(target_os = "macos") {
            return;
        }
        let config = config().expect("a test environment has a home directory");
        let state = state().expect("a test environment has a home directory");
        assert_ne!(config, state, "a backup of the settings would carry the file history");
    }

    #[test]
    fn an_empty_variable_is_an_unset_one() {
        let key = "MBRD_TEST_EMPTY_VARIABLE";
        // SAFETY: the key is unique to this test, so no other thread in the
        // process is reading or writing it while this runs.
        unsafe { std::env::set_var(key, "") };
        assert_eq!(from_env(key), None);
        unsafe { std::env::set_var(key, "/somewhere") };
        assert_eq!(from_env(key), Some(PathBuf::from("/somewhere")));
        unsafe { std::env::remove_var(key) };
    }
}
