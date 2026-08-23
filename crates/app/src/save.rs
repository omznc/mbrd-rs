//! The two ways a board crosses the disk.
//!
//! Small, and deliberately not in `board_view.rs`: the view's job is what the
//! pointer is doing, and a file write is the one thing in this app that can
//! lose work. It is easier to be careful about in a module that does nothing else.

use std::io::Cursor;
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
pub fn write(path: &Path, doc: &Document) -> Result<()> {
    let bytes = mbrd_core::mbrd::to_bytes(doc, &now_iso8601()).context("packing the board")?;

    let temp = path.with_extension("mbrd.part");
    std::fs::write(&temp, &bytes).with_context(|| format!("writing {}", temp.display()))?;
    std::fs::rename(&temp, path).with_context(|| format!("replacing {}", path.display()))?;
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
    let bytes = std::fs::read(path).context("reading the file")?;
    mbrd_core::mbrd::read_watched(Cursor::new(bytes), watch).context("reading the board")
}
