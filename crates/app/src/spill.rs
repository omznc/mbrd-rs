//! Laying a played file out on disk, for a decoder that wants a path.
//!
//! Shared by all three media backends — `pipeline.rs` on Linux, `pipeline_mac.rs`
//! and `pipeline_win.rs` — because all three want exactly the same thing and
//! none of them wants it enough to be interesting. GStreamer takes a URI,
//! `AVPlayerItem` takes an `NSURL`, and the Media Engine takes a string; all of
//! them are happiest with a file that is sitting still on a disk.
//!
//! ## Why a file rather than the bytes we already have
//!
//! An asset lives in memory, unzipped out of the `.mbrd`. Feeding it to a
//! decoder as bytes means implementing a *source*: an `appsrc` with a
//! `seek-data` handler on Linux, an `AVAssetResourceLoaderDelegate` on macOS, an
//! `IMFByteStream` on Windows. Three of them, each a good deal more code than
//! this file, and each handing back a seek implementation we would have to write
//! and then be wrong about.
//!
//! A path costs one write, once, and buys seeking for free on all three.
//!
//! ## The hash is the name
//!
//! The same identity the archive itself uses. That is what makes the same clip
//! on four cards one file, and what makes a board reopened tomorrow find its
//! video already unpacked. It also means "the file is already there" is a
//! complete answer and can never be a stale one — the name *is* the contents.
//!
//! It also means a 300 MB video is not held twice: the pipeline reads it off
//! the disk it was going to be written to anyway.
//!
//! ## And it is a cache, so it is allowed to be deleted
//!
//! Everything under here can be rebuilt from a `.mbrd` that still exists, so
//! [`prune`] is blunt on purpose — oldest first, until it is under budget, and
//! deleting the wrong one costs a re-spill and nothing else. The budget is
//! about being a good guest on somebody's disk rather than about correctness.

use std::path::{Path, PathBuf};

/// How much of somebody's disk this may hold before the oldest files start
/// going. Half a gigabyte is a handful of long videos or a great many short
/// ones, which is the shape of a moodboard.
const BUDGET: u64 = 512 * 1024 * 1024;

/// Where played files are laid out, and whether there is anywhere at all.
pub struct Spill {
    /// `None` where there is nowhere to write. On a locked-down machine that is
    /// possible, and it is not fatal to anything else in the app — the board
    /// opens, the pictures draw, and the play button says why it cannot.
    dir: Option<PathBuf>,
}

impl Spill {
    /// Make the directory, and hold it to its budget.
    ///
    /// The prune runs once, here, rather than on every file laid out: it walks
    /// the whole directory, and doing that per press on a board of clips would
    /// be a stat call per file per press for a rule that only has to be right
    /// eventually.
    pub fn open() -> Self {
        let Some(dir) = crate::dirs::cache().map(|dir| dir.join("media")) else {
            return Self { dir: None };
        };
        if std::fs::create_dir_all(&dir).is_err() {
            return Self { dir: None };
        }
        prune(&dir, BUDGET);
        Self { dir: Some(dir) }
    }

    /// The file on disk for this asset, written if it is not there already.
    ///
    /// `bytes` is only read the first time a given hash is laid out, so passing
    /// the whole asset in on every press costs a borrow rather than a copy.
    pub fn lay_out(&self, hash: &str, ext: &str, bytes: &[u8]) -> Option<PathBuf> {
        let dir = self.dir.as_ref()?;
        // The extension is kept because some demuxers still take a hint from
        // it, and filtered because it arrived from a filename somebody else
        // chose and is about to become a path.
        let ext: String = ext.chars().filter(|c| c.is_ascii_alphanumeric()).take(12).collect();
        let name = match ext.is_empty() {
            true => hash.to_string(),
            false => format!("{hash}.{ext}"),
        };
        let path = dir.join(name);
        if path.is_file() {
            return Some(path);
        }
        // Through a part file and a rename, so a decoder can never open a copy
        // that is still being written — the same rule `save::write` follows for
        // the board itself.
        let part = path.with_extension("part");
        std::fs::write(&part, bytes).ok()?;
        std::fs::rename(&part, &path).ok()?;
        Some(path)
    }
}

/// Hold a directory to a budget, oldest first.
///
/// By modification time rather than by access time, because access times are
/// off on most filesystems now and a rule that read one would be a rule that
/// always deleted the same file.
fn prune(dir: &Path, budget: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(PathBuf, u64, std::time::SystemTime)> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let data = entry.metadata().ok()?;
            data.is_file().then_some((
                entry.path(),
                data.len(),
                data.modified().unwrap_or(std::time::UNIX_EPOCH),
            ))
        })
        .collect();

    let mut total: u64 = files.iter().map(|(_, size, _)| *size).sum();
    if total <= budget {
        return;
    }
    files.sort_by_key(|(_, _, when)| *when);
    for (path, size, _) in files {
        if total <= budget {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(size);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// The budget is the whole of what keeps a cache directory from being a
    /// disk leak, and it is one arithmetic mistake away from deleting
    /// everything or nothing.
    #[test]
    fn pruning_takes_the_oldest_and_stops_the_moment_it_is_under() {
        let dir = std::env::temp_dir().join(format!("mbrd-prune-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);

        // Written oldest first and stamped explicitly, since three files
        // written in the same millisecond have no order of their own.
        for (name, size, age) in [("old", 100, 300), ("middle", 100, 200), ("new", 100, 100)] {
            let path = dir.join(name);
            std::fs::write(&path, vec![0u8; size]).unwrap();
            let when = std::time::SystemTime::now() - Duration::from_secs(age);
            let _ = filetime(&path, when);
        }

        prune(&dir, 150);
        assert!(!dir.join("old").exists(), "the oldest went first");
        assert!(!dir.join("middle").exists(), "and the next, to get under");
        assert!(dir.join("new").exists(), "and it stopped there");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_directory_under_its_budget_is_left_entirely_alone() {
        let dir = std::env::temp_dir().join(format!("mbrd-prune-keep-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        std::fs::write(dir.join("small"), vec![0u8; 10]).unwrap();
        prune(&dir, 1000);
        assert!(dir.join("small").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The one part of a backend that is testable on any machine, which is why
    /// it is here rather than three times over: two of the three decoders this
    /// serves cannot be compiled on the platform the tests run on, let alone
    /// exercised.
    #[test]
    fn a_name_is_the_hash_and_an_extension_that_could_not_be_anything_else() {
        let dir = std::env::temp_dir().join(format!("mbrd-spill-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let spill = Spill { dir: Some(dir.clone()) };

        let path = spill.lay_out("abc123", "mp4", b"pretend film").unwrap();
        assert_eq!(path.file_name().unwrap(), "abc123.mp4");
        assert_eq!(std::fs::read(&path).unwrap(), b"pretend film");

        // Content-addressed, so the second call is the same file and does not
        // rewrite it — which is what makes one clip on four cards one file.
        let again = spill.lay_out("abc123", "mp4", b"different bytes entirely").unwrap();
        assert_eq!(again, path);
        assert_eq!(std::fs::read(&path).unwrap(), b"pretend film", "not rewritten");

        // An extension is a hint to a demuxer, not a path component somebody
        // else gets to choose.
        let nasty = spill.lay_out("def456", "../../etc/passwd", b"x").unwrap();
        assert_eq!(nasty.parent().unwrap(), dir);
        assert_eq!(nasty.file_name().unwrap(), "def456.etcpasswd");

        let bare = spill.lay_out("ghi789", "", b"x").unwrap();
        assert_eq!(bare.file_name().unwrap(), "ghi789");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Nowhere to write is a state, not a failure: every caller reads `None`
    /// as "this card cannot play" and says so.
    #[test]
    fn nowhere_to_write_lays_nothing_out_and_does_not_panic() {
        let spill = Spill { dir: None };
        assert!(spill.lay_out("abc", "mp4", b"x").is_none());
    }

    /// Nothing in `std` sets a modification time, and pulling in `filetime`
    /// for one test would be a dependency for a test. Every platform this
    /// runs on has the call.
    fn filetime(path: &Path, when: std::time::SystemTime) -> std::io::Result<()> {
        let file = std::fs::OpenOptions::new().write(true).open(path)?;
        file.set_modified(when)
    }
}
