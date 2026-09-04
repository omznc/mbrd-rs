//! The boards you had open, remembered between runs.
//!
//! A list of paths in a small JSON file in the **state** directory `dirs.rs`
//! names. *State* rather than *config*: this is something the app noticed, not
//! something anybody set, and the distinction is what stops a backup of
//! somebody's settings carrying their file history with it.
//!
//! Every operation here is best-effort and silent. A recent list that cannot be
//! read is an empty one, and a recent list that cannot be written is a switcher
//! that forgets — neither is worth interrupting somebody's work over, and
//! neither can lose anything, because the only copy of anything that matters is
//! the board on disk.

use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::dirs;

/// How many to keep. Long enough to hold a project's worth of boards, short
/// enough that the switcher is a list rather than a search.
const KEEP: usize = 24;

/// Where the list lives.
///
/// The *state* directory, not the config one — see the note above, and
/// `dirs.rs` for what that resolves to on each platform.
fn store() -> Option<PathBuf> {
    Some(dirs::state()?.join("recent.json"))
}

/// The boards worth offering, most recent first.
///
/// Paths that no longer exist are dropped on the way out rather than on the way
/// in — a board on a drive that is not plugged in right now has not been
/// deleted, and forgetting it the one time you opened the app without it would
/// be the wrong lesson to learn.
pub fn load() -> Vec<PathBuf> {
    let Some(path) = store() else { return Vec::new() };
    let Ok(text) = crate::store::read_to_string(&path) else { return Vec::new() };
    let Ok(value) = serde_json::from_str::<Value>(&text) else { return Vec::new() };
    value
        .get("boards")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(PathBuf::from)
                .filter(|p| crate::store::exists(p))
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
    write_all(&path, &boards);
}

/// Take a board off the list, for good.
///
/// The one case where [`load`]'s forgiveness is the wrong behaviour. That
/// filter exists because a path that is missing today may be a drive that is
/// plugged in tomorrow — but a board somebody has just deleted is not coming
/// back, and leaving the entry in the store means filtering it out on every
/// read from now until the list rolls past [`KEEP`].
///
/// Called *after* the file is gone, so the path is taken as given rather than
/// canonicalised: there is nothing left on disk to canonicalise against, and
/// everything in the store was written canonical already.
pub fn forget(board: &Path) {
    let Some(path) = store() else { return };
    let mut boards = read_all();
    let before = boards.len();
    boards.retain(|p| p != board);
    // Nothing to say, so nothing is written. A board opened from beside the
    // current one has never been in the store, and rewriting the file to
    // change none of it would be a write per deletion for no reason.
    if boards.len() == before {
        return;
    }
    write_all(&path, &boards);
}

/// A board's file moved: swap the path where it stands.
///
/// Not a `forget` and a `remember`, deliberately — that pair would carry the
/// board to the top of the list, and a move is filing rather than a visit.
/// The board's place in the list is its place in time, which renaming its
/// file does not change. Like [`forget`], the old path is taken as given:
/// everything in the store was written canonical already.
pub fn rename(old: &Path, new: &Path) {
    let Some(path) = store() else { return };
    let new = new.canonicalize().unwrap_or_else(|_| new.to_path_buf());
    let mut boards = read_all();
    let mut hit = false;
    for slot in boards.iter_mut().filter(|slot| *slot == old) {
        *slot = new.clone();
        hit = true;
    }
    // Nothing to say, so nothing is written — a board that was never in the
    // store does not enter it by being renamed.
    if !hit {
        return;
    }
    write_all(&path, &boards);
}

/// Put the list back on disk, best effort.
///
/// Best effort because there is nothing useful to do about a failure: the list
/// is a convenience, and a run where it could not be written is a run where the
/// switcher is one board shorter than it might have been.
fn write_all(path: &Path, boards: &[PathBuf]) {
    let out = serde_json::json!({ "boards": boards });
    if let Some(dir) = path.parent() {
        let _ = crate::store::create_dir_all(dir);
    }
    let _ = crate::store::write(path, out.to_string().as_bytes());
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
    let Ok(paths) = crate::store::read_dir_paths(dir) else { return Vec::new() };
    paths.into_iter().filter(|p| p.extension().is_some_and(|e| e == "mbrd")).collect()
}
