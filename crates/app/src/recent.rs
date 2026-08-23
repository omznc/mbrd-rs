//! The boards you had open, remembered between runs.
//!
//! A list of paths in a small JSON file under the XDG state directory. *State*
//! rather than *config*: this is something the app noticed, not something
//! anybody set, and the distinction is what stops a backup of somebody's
//! settings carrying their file history with it.
//!
//! Every operation here is best-effort and silent. A recent list that cannot be
//! read is an empty one, and a recent list that cannot be written is a switcher
//! that forgets — neither is worth interrupting somebody's work over, and
//! neither can lose anything, because the only copy of anything that matters is
//! the board on disk.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// How many to keep. Long enough to hold a project's worth of boards, short
/// enough that the switcher is a list rather than a search.
const KEEP: usize = 24;

/// Where the list lives.
fn store() -> Option<PathBuf> {
    let dir = match std::env::var_os("XDG_STATE_HOME") {
        Some(state) if !state.is_empty() => PathBuf::from(state),
        // The spec's own fallback. Not `~/.config`: see the note above.
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".local/state"),
    };
    Some(dir.join("mbrd/recent.json"))
}

/// The boards worth offering, most recent first.
///
/// Paths that no longer exist are dropped on the way out rather than on the way
/// in — a board on a drive that is not plugged in right now has not been
/// deleted, and forgetting it the one time you opened the app without it would
/// be the wrong lesson to learn.
pub fn load() -> Vec<PathBuf> {
    let Some(path) = store() else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(&path) else { return Vec::new() };
    let Ok(value) = serde_json::from_str::<Value>(&text) else { return Vec::new() };
    value
        .get("boards")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .filter(|p| p.exists())
                .take(KEEP)
                .collect()
        })
        .unwrap_or_default()
}

/// Put a board at the top of the list.
pub fn remember(board: &Path) {
    let Some(path) = store() else { return };
    // Absolute, because the switcher may well be used from a different working
    // directory than the one the board was opened from.
    let board = board.canonicalize().unwrap_or_else(|_| board.to_path_buf());

    let mut boards = read_all();
    boards.retain(|p| p != &board);
    boards.insert(0, board);
    boards.truncate(KEEP);

    let out = serde_json::json!({ "boards": boards });
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&path, out.to_string());
}

/// The stored list without the does-it-exist filter, so that writing back does
/// not quietly delete entries [`load`] merely declined to offer.
fn read_all() -> Vec<PathBuf> {
    let Some(path) = store() else { return Vec::new() };
    let Ok(text) = std::fs::read_to_string(&path) else { return Vec::new() };
    serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| {
            v.get("boards")
                .and_then(Value::as_array)
                .map(|l| l.iter().filter_map(Value::as_str).map(PathBuf::from).collect())
        })
        .unwrap_or_default()
}

/// Every `.mbrd` sitting in a directory, sorted by name.
///
/// The switcher offers these alongside the remembered ones, so that a board you
/// have never opened is still one press away when you are working in the folder
/// it lives in. Not recursive: a walk of somebody's home directory is not
/// something a keystroke should start.
pub fn beside(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "mbrd"))
        .collect();
    out.sort();
    out
}
