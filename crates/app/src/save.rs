//! The two ways a board crosses the disk.
//!
//! Small, and deliberately not in `board_view.rs`: the view's job is what the
//! pointer is doing, and a file write is the one thing in this app that can
//! lose work. It is easier to be careful about in a module that does nothing else.

// The atomic write is the native path's alone; a browser's store has no second
// step to order against the first. See `through`, below.
#[cfg(not(target_family = "wasm"))]
use std::fs::File;
use std::io::Cursor;
#[cfg(not(target_family = "wasm"))]
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

use mbrd_core::naming::now_iso8601;
use mbrd_core::Document;

/// Write a `.mbrd`, atomically.
///
/// Via a temporary file in the same directory and then a rename, because the
/// obvious version — truncate the real file, write into it — turns a crash
/// halfway through into a board that is neither the old one nor the new one.
/// The rename is what makes the swap all-or-nothing, and it has to be the *same
/// directory* or it stops being a rename and becomes a copy.
///
/// **A rename alone is not the whole of it, and the missing half is the flush.**
/// Renaming is atomic against another *process* — that is what stops a reader
/// opening a board that is still being written — but it is not atomic against
/// the power going out. The directory entry is metadata and the archive is
/// data, and on every filesystem here they can reach the disk in that order, so
/// a machine that loses power in the wrong second comes back to a board that is
/// exactly the thing this function exists to prevent. [`through`] orders them.
///
/// It matters more here than the shape of the code suggests, because
/// `BoardView::arm_autosave` runs this on a timer with nobody watching — which
/// is the whole of why this app has no unsaved-work indicator.
pub fn write(path: &Path, doc: &Document) -> Result<()> {
    let bytes = mbrd_core::mbrd::to_bytes(doc, &now_iso8601()).context("packing the board")?;

    let temp = path.with_extension("mbrd.part");
    through(&temp, path, &bytes).inspect_err(|_| {
        // Best effort, and the same rule `spill::write_through` follows: a part
        // file left beside the board is megabytes nothing will ever come back
        // for, and this write is already being reported as having failed.
        let _ = crate::store::remove_file(&temp);
    })
}

/// The write, the flush, and the rename — see [`write`], which is where the
/// order is argued. Split out only so that a failure anywhere in it has one
/// place to be cleaned up after.
fn through(temp: &Path, path: &Path, bytes: &[u8]) -> Result<()> {
    // A browser's store has no second step to order against the first: a write
    // there either stores the whole value or throws, which is the property the
    // temporary file and the rename below exist to buy. So the web build takes
    // the write and stops — see `webfs.rs`.
    #[cfg(target_family = "wasm")]
    {
        let _ = temp;
        return crate::store::write(path, bytes)
            .with_context(|| format!("writing {}", path.display()));
    }

    #[cfg(target_family = "wasm")]
    #[allow(unreachable_code)]
    {
        unreachable!()
    }

    #[cfg(not(target_family = "wasm"))]
    let mut file = File::create(temp).with_context(|| format!("writing {}", temp.display()))?;
    #[cfg(not(target_family = "wasm"))]
    {
        file.write_all(bytes).with_context(|| format!("writing {}", temp.display()))?;
        file.sync_all().with_context(|| format!("flushing {}", temp.display()))?;
        drop(file);

        crate::store::rename(temp, path)
            .with_context(|| format!("replacing {}", path.display()))?;
    }

    // And the rename, which is a change to the *directory* and needs the same
    // treatment for the same reason. Best effort rather than checked: Windows
    // will not open a directory as a file at all, and the failure here is
    // benign in a way the one above is not — a board whose rename is not yet
    // durable is still the old board on disk, which is a board.
    #[cfg(not(target_family = "wasm"))]
    if let Some(dir) = path.parent() {
        let _ = File::open(dir).and_then(|dir| dir.sync_all());
    }
    Ok(())
}

/// Read a `.mbrd` off the disk, saying how far it has got.
///
/// Whole, into memory, before anything is parsed. A board is tens of megabytes
/// at the outside and the alternative — reading the archive in place — means
/// holding a file handle open for as long as the board is, which is what turns
/// "somebody moved the file" into a crash an hour later.
///
/// For the opening line — see [`mbrd_core::mbrd::read_watched`], which is where
/// the numbers come from and what they mean. Nothing is reported for the file
/// read itself, which is the one part of this with no entries to count; on a
/// board large enough for that to be visible it is a fraction of the inflating
/// and hashing that follows.
///
/// The one way into this app that reads a board off disk — `main.rs`'s
/// `argv` handling and the Finder's `on_open_urls` both fold into
/// `BoardView::open_board`, which calls this rather than a bare, unwatched
/// read, so there has only ever needed to be the one function here.
pub fn read_watched(path: &Path, watch: impl FnMut(u64, u64)) -> Result<Document> {
    let bytes = crate::store::read(path).context("reading the file")?;
    mbrd_core::mbrd::read_watched(Cursor::new(bytes), watch).context("reading the board")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(what: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mbrd-save-{what}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary directory");
        dir
    }

    fn a_board() -> Document {
        Document::default()
    }

    #[test]
    fn a_written_board_leaves_no_part_file_beside_it() {
        // The part file is an implementation detail of the swap and must not
        // outlive it — a `.part` sitting next to a board is the one thing in
        // this directory nothing else will ever come back for.
        let dir = scratch("clean");
        let path = dir.join("board.mbrd");

        write(&path, &a_board()).expect("writing");

        assert!(path.is_file(), "the board is where it was asked for");
        assert!(!path.with_extension("mbrd.part").exists(), "the part file was renamed, not left");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_write_that_fails_takes_its_part_file_with_it() {
        // The failure arm. Before this, a rename that failed left the whole
        // archive behind under `.part` and nothing swept it — see `write`,
        // which now cleans up after every arm of `through` rather than only
        // after the ones that never ran.
        let dir = scratch("failing");
        // A board *inside a file*: the parent of the part file is not a
        // directory, so `File::create` cannot make it.
        let wall = dir.join("wall");
        std::fs::write(&wall, b"not a directory").expect("writing");
        let path = wall.join("board.mbrd");

        assert!(write(&path, &a_board()).is_err(), "there is nowhere to put this");
        assert!(!path.with_extension("mbrd.part").exists(), "no part file survived the failure");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
