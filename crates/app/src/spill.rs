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
//! ## The write is not on the thread that draws
//!
//! **This is the whole reason there is a worker in here.** It used to be one
//! `std::fs::write` called straight out of [`lay_out`], and every caller of
//! `lay_out` is inside `BoardView::pump_media`, which is inside `advance`,
//! which is inside `Render::render`. Pressing play on a 300 MB clip therefore
//! wrote 300 MB to disk on the thread that draws — the app stopped painting,
//! stopped answering the pointer, and stopped pumping the platform's event
//! queue for as long as the disk took. On macOS that is the spinning wheel and
//! on Windows it is the compositor greying the window out as "not responding".
//! On a slow or network-mounted disk it is seconds.
//!
//! So a file is now *asked for* rather than made. [`lay_out`] answers
//! [`Laid::Ready`] once the file is there, [`Laid::Working`] while a thread is
//! putting it there, and [`Laid::Nowhere`] where there is nowhere to write at
//! all. The frame loop is already asking every frame for as long as a card
//! wants to play — see `BoardView::pump_media` — so "ask again next frame" is
//! not a mechanism anybody had to build, and a caller that reads `Working` as
//! "not yet" gets the whole fix for free.
//!
//! The one cost that stayed on the calling thread is the copy of the bytes,
//! because an asset is a `Vec<u8>` owned by the document and a thread cannot
//! borrow it. A copy is memory bandwidth where the write was disk latency —
//! tens of milliseconds against seconds — and it is paid once per hash per
//! session, behind an `is_file` check that costs one `stat` on every play
//! after the first.
//!
//! ## The hash is the name
//!
//! The same identity the archive itself uses. That is what makes the same clip
//! on four cards one file, and what makes a board reopened tomorrow find its
//! video already unpacked. It also means "the file is already there" is a
//! complete answer and can never be a stale one — the name *is* the contents.
//!
//! ## And it is a cache, so it is allowed to be deleted
//!
//! Everything under here can be rebuilt from a `.mbrd` that still exists, so
//! [`prune`] is blunt on purpose — oldest first, until it is under budget, and
//! deleting the wrong one costs a re-spill and nothing else. The budget is
//! about being a good guest on somebody's disk rather than about correctness.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

/// How much of somebody's disk this may hold before the oldest files start
/// going. Half a gigabyte is a handful of long videos or a great many short
/// ones, which is the shape of a moodboard.
const BUDGET: u64 = 512 * 1024 * 1024;

/// What asking for a file got.
///
/// Three answers rather than an `Option`, because the two halves of `None`
/// mean opposite things to a caller: one of them is worth asking about again
/// on the next frame and the other will never be true, and a backend that
/// could not tell them apart would either give up on a file that was still
/// arriving or spin forever on a disk that has no room.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Laid {
    /// It is on the disk, at this path, now.
    Ready(PathBuf),
    /// A thread is writing it. Ask again.
    Working,
    /// There is nowhere to write it, or writing it failed. Carries what to
    /// say, which is a sentence rather than an `io::Error` because it is on
    /// its way to the status bar.
    Nowhere(String),
}

/// One file to write, handed to the worker.
struct Job {
    part: PathBuf,
    path: PathBuf,
    bytes: Vec<u8>,
}

/// What the worker and the callers share.
///
/// A single lock over both maps rather than one each: every read touches both
/// — "is this working, and if not did it fail" is one question — and two locks
/// taken in sequence would be two chances to answer it out of a pair of
/// different moments.
#[derive(Default)]
struct Board {
    /// Files a thread is writing right now.
    working: HashSet<PathBuf>,
    /// Files that could not be written, and why. A card is told once and then
    /// stops asking, so this is never cleared: retrying a disk that just
    /// refused, once per frame, is a way to make a full disk feel like a hung
    /// application.
    refused: HashMap<PathBuf, String>,
}

/// Where played files are laid out, and whether there is anywhere at all.
pub struct Spill {
    /// `None` where there is nowhere to write. On a locked-down machine that is
    /// possible, and it is not fatal to anything else in the app — the board
    /// opens, the pictures draw, and the play button says why it cannot.
    dir: Option<PathBuf>,
    board: Arc<Mutex<Board>>,
    /// `None` where there is no directory, and so nothing a worker could do.
    to_worker: Option<Sender<Job>>,
}

impl Spill {
    /// Make the directory, and start the thread that writes into it.
    ///
    /// The `create_dir_all` stays on the calling thread because it is one
    /// syscall and its answer is what decides whether this object can do
    /// anything at all. The prune does not: it walks the whole directory and
    /// stats every file in it, which is the same kind of unbounded disk work
    /// as the writes, so it is the worker's first job rather than this
    /// function's last act.
    pub fn open() -> Self {
        let Some(dir) = crate::dirs::cache().map(|dir| dir.join("media")) else {
            return Self { dir: None, board: Arc::default(), to_worker: None };
        };
        if std::fs::create_dir_all(&dir).is_err() {
            return Self { dir: None, board: Arc::default(), to_worker: None };
        }

        let board: Arc<Mutex<Board>> = Arc::default();
        let (to_worker, jobs) = std::sync::mpsc::channel::<Job>();

        // A thread of our own rather than gpui's background executor, for two
        // reasons. The executor's pool is what decodes pictures and rasterises
        // meshes, and a 300 MB write parked on one of its threads is one fewer
        // thread doing the work somebody is actually looking at. And `Spill`
        // is shared by three backends that know nothing about gpui — see the
        // module note — so reaching for an executor here would be this file
        // growing a dependency on the app it is meant to be beneath.
        //
        // Detached deliberately: the thread ends when the channel closes,
        // which happens when the `Spill` is dropped, which happens when the
        // board goes away. A join would be a wait on a write nobody is going
        // to read.
        let worker = Worker { dir: dir.clone(), board: board.clone() };
        std::thread::Builder::new().name("mbrd-spill".into()).spawn(move || worker.run(jobs)).ok();

        Self { dir: Some(dir), board, to_worker: Some(to_worker) }
    }

    /// The file on disk for this asset, or how far off it is.
    ///
    /// `bytes` is only *copied* on the call that starts the write, so a card
    /// asking on every frame of a clip that is already unpacked costs one
    /// `is_file` and nothing else.
    pub fn lay_out(&self, hash: &str, ext: &str, bytes: &[u8]) -> Laid {
        let Some(dir) = self.dir.as_ref() else {
            return Laid::Nowhere("nowhere to unpack this file".into());
        };
        let path = dir.join(name_for(hash, ext));

        // The cheap answer first, and off the disk rather than off our own
        // bookkeeping: a file laid out by an earlier run of the app is just as
        // ready as one this run wrote, and the name is the contents so there
        // is no such thing as a stale one.
        if path.is_file() {
            return Laid::Ready(path);
        }

        let Ok(mut board) = self.board.lock() else {
            return Laid::Nowhere("the unpacking thread has gone".into());
        };
        if let Some(why) = board.refused.get(&path) {
            return Laid::Nowhere(why.clone());
        }
        if board.working.contains(&path) {
            return Laid::Working;
        }

        // Claimed *before* the job is sent, and while the lock is still held,
        // so that two cards showing the same clip on the same frame cannot
        // both start a write of it.
        board.working.insert(path.clone());
        drop(board);

        let job =
            Job { part: path.with_extension("part"), path: path.clone(), bytes: bytes.to_vec() };
        match self.to_worker.as_ref().map(|to| to.send(job)) {
            Some(Ok(())) => Laid::Working,
            // The worker has gone, which can only be a thread that failed to
            // spawn or one that panicked. Recorded as a refusal so the card
            // says so once rather than asking a dead channel every frame.
            _ => {
                self.refuse(&path, "the unpacking thread has gone");
                Laid::Nowhere("the unpacking thread has gone".into())
            }
        }
    }

    fn refuse(&self, path: &Path, why: &str) {
        if let Ok(mut board) = self.board.lock() {
            board.working.remove(path);
            board.refused.insert(path.to_path_buf(), why.to_string());
        }
    }
}

/// The thread that writes.
struct Worker {
    dir: PathBuf,
    board: Arc<Mutex<Board>>,
}

impl Worker {
    fn run(self, jobs: Receiver<Job>) {
        // Once, before the first write rather than after it: a directory
        // already over budget should come down before another few hundred
        // megabytes go into it, not after.
        prune(&self.dir, BUDGET);

        while let Ok(job) = jobs.recv() {
            let outcome = write_through(&job);
            let Ok(mut board) = self.board.lock() else { return };
            board.working.remove(&job.path);
            if let Err(why) = outcome {
                board.refused.insert(job.path.clone(), why);
            }
        }
    }
}

/// One file, through a part file and a rename.
///
/// The rename is what stops a decoder opening a copy that is still being
/// written — the same rule `save::write` follows for the board itself, and it
/// matters more here than it did when this was synchronous: the caller is now
/// asking `is_file` on another thread while this runs, and a bare write would
/// answer `true` to it halfway through.
fn write_through(job: &Job) -> Result<(), String> {
    if let Err(err) = std::fs::write(&job.part, &job.bytes) {
        // Best effort. A part file left behind is a few hundred megabytes the
        // prune will take on some later launch, which is better than failing
        // the write twice.
        let _ = std::fs::remove_file(&job.part);
        return Err(format!("could not unpack this file — {err}"));
    }
    std::fs::rename(&job.part, &job.path)
        .map_err(|err| format!("could not unpack this file — {err}"))
}

/// What a laid-out file is called.
///
/// The extension is kept because some demuxers still take a hint from it, and
/// filtered because it arrived from a filename somebody else chose and is
/// about to become a path.
fn name_for(hash: &str, ext: &str) -> String {
    let ext: String = ext.chars().filter(|c| c.is_ascii_alphanumeric()).take(12).collect();
    match ext.is_empty() {
        true => hash.to_string(),
        false => format!("{hash}.{ext}"),
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
    use std::time::{Duration, Instant};

    /// The budget is the whole of what keeps a cache directory from being a
    /// disk leak, and it is one arithmetic mistake away from deleting
    /// everything or nothing.
    #[test]
    fn pruning_takes_the_oldest_and_stops_the_moment_it_is_under() {
        let dir = scratch("prune");

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
        let dir = scratch("prune-keep");
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
        assert_eq!(name_for("abc123", "mp4"), "abc123.mp4");
        // An extension is a hint to a demuxer, not a path component somebody
        // else gets to choose.
        assert_eq!(name_for("def456", "../../etc/passwd"), "def456.etcpasswd");
        assert_eq!(name_for("ghi789", ""), "ghi789");
    }

    /// The whole of the fix this file exists for: the first ask does not
    /// block, and the file turns up afterwards.
    ///
    /// Written as "the first answer is never `Ready`" rather than as a timing
    /// assertion, because a test that measured the call would be a test that
    /// failed on a fast disk for the right reasons and on a loaded CI machine
    /// for the wrong ones.
    #[test]
    fn the_first_ask_does_not_write_and_a_later_one_finds_the_file() {
        let dir = scratch("spill-async");
        let spill = spilling(&dir);

        let first = spill.lay_out("abc123", "mp4", b"pretend film");
        assert_eq!(first, Laid::Working, "the calling thread must not have written it");

        let path = settle(&spill, "abc123", "mp4");
        assert_eq!(path.file_name().unwrap(), "abc123.mp4");
        assert_eq!(std::fs::read(&path).unwrap(), b"pretend film");
        assert!(!path.with_extension("part").exists(), "the part file was renamed, not left");

        // Content-addressed, so the second ask is the same file, is answered
        // straight away, and does not rewrite it — which is what makes one
        // clip on four cards one file.
        let again = spill.lay_out("abc123", "mp4", b"different bytes entirely");
        assert_eq!(again, Laid::Ready(path.clone()));
        assert_eq!(std::fs::read(&path).unwrap(), b"pretend film", "not rewritten");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two cards showing the same clip on the same frame are one write.
    ///
    /// The claim goes into `working` under the same lock that read it, so the
    /// second ask cannot slip between the first one's look and its send.
    #[test]
    fn asking_twice_before_it_lands_starts_one_write_and_not_two() {
        let dir = scratch("spill-once");
        let spill = spilling(&dir);

        assert_eq!(spill.lay_out("dup", "mp4", b"once"), Laid::Working);
        assert_eq!(spill.lay_out("dup", "mp4", b"once"), Laid::Working);

        let path = settle(&spill, "dup", "mp4");
        assert_eq!(std::fs::read(&path).unwrap(), b"once");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Nowhere to write is a state, not a failure: every caller reads it as
    /// "this card cannot play" and says so, once.
    #[test]
    fn nowhere_to_write_lays_nothing_out_and_does_not_panic() {
        let spill = Spill { dir: None, board: Arc::default(), to_worker: None };
        assert!(matches!(spill.lay_out("abc", "mp4", b"x"), Laid::Nowhere(_)));
    }

    /// A `Spill` on a directory of our choosing, with the same worker the real
    /// one gets. `Spill::open` picks the user's cache directory, which is not
    /// somewhere a test may write.
    fn spilling(dir: &Path) -> Spill {
        let board: Arc<Mutex<Board>> = Arc::default();
        let (to_worker, jobs) = std::sync::mpsc::channel::<Job>();
        let worker = Worker { dir: dir.to_path_buf(), board: board.clone() };
        std::thread::spawn(move || worker.run(jobs));
        Spill { dir: Some(dir.to_path_buf()), board, to_worker: Some(to_worker) }
    }

    /// Ask until the worker has landed the file, the way the frame loop does.
    ///
    /// Bounded, so a worker that never finishes fails the test instead of
    /// hanging the suite.
    fn settle(spill: &Spill, hash: &str, ext: &str) -> PathBuf {
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            match spill.lay_out(hash, ext, b"") {
                Laid::Ready(path) => return path,
                Laid::Working => std::thread::sleep(Duration::from_millis(5)),
                Laid::Nowhere(why) => panic!("the worker refused it: {why}"),
            }
        }
        panic!("the worker never laid {hash} out");
    }

    fn scratch(what: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mbrd-{what}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Nothing in `std` sets a modification time, and pulling in `filetime`
    /// for one test would be a dependency for a test. Every platform this
    /// runs on has the call.
    fn filetime(path: &Path, when: std::time::SystemTime) -> std::io::Result<()> {
        let file = std::fs::OpenOptions::new().write(true).open(path)?;
        file.set_modified(when)
    }
}
