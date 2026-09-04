//! Sound and pictures on Windows, through the Media Foundation Media Engine.
//!
//! The same door as [`crate::pipeline`], opening onto a third room. One
//! `IMFMediaEngine` per card that is playing, sound straight out to the default
//! endpoint, and video pulled back a frame at a time into a WIC bitmap the
//! canvas can read. `board_view.rs` cannot tell which of the three backends it
//! is holding, and that is the point — see `pipeline_off.rs` for the seam.
//!
//! ## Why the Media Engine and not the Source Reader
//!
//! `IMFSourceReader` is the obvious Media Foundation entry point and it is the
//! wrong one here. It is a *pull* API with no clock: taking it means writing an
//! audio ring buffer, a WASAPI endpoint, a resampler for whatever rate the
//! device wants, and then an A/V sync loop to keep the picture with the sound.
//! That is a media player, and this is a moodboard.
//!
//! The Media Engine is the engine behind `<video>` in Edge, exposed as COM. It
//! owns the clock, reaches the speakers by itself, and answers `Play`, `Pause`,
//! `SetCurrentTime`, `GetDuration`, `SetVolume`, `SetMuted`, `SetLoop` and
//! `IsEnded` — which is, almost exactly, [`Stack`]'s whole surface. The mapping
//! being that close is not luck: both are describing the same object, one that
//! HTML settled the shape of a long time ago.
//!
//! ## Everything Media Foundation happens on a thread of its own
//!
//! **This is the biggest difference between this backend and the other two, and
//! it is not a preference.** Media Foundation does not support the
//! single-threaded apartment: it does not marshal STA objects onto its work
//! queues and does not maintain STA invariants, and its own components cannot
//! run in one. Microsoft says so in as many words.
//!
//! gpui's main thread *is* an STA — it calls `OleInitialize` before any of this
//! app's code runs, because that is what a window needs for drag and drop. So
//! the thread that draws is the one thread in the process that must not own a
//! Media Engine. This file used to own them all there: `MFStartup`,
//! `CreateInstance`, every `Play` and every `TransferVideoFrame`, on the STA.
//! That is a documented-unsupported configuration, and the shape of its failure
//! is a hang — `IMFMediaEngine::Shutdown` blocks until the engine's worker
//! threads stop, those threads want to call back into the apartment, and the
//! apartment is inside `Shutdown` and no longer pumping messages.
//!
//! So there is a worker thread here that calls `CoInitializeEx` with
//! `COINIT_MULTITHREADED`, and every COM object in this file is created on it,
//! used on it and released on it. Nothing that is not plain data crosses back.
//!
//! ## Which makes the seam a snapshot rather than a call
//!
//! The other two backends answer `poll` by asking the decoder, on the spot, on
//! the thread that draws. This one cannot, so it does not: [`Shared`] is a map
//! the board writes what it *wants* into and the worker writes what it *found*
//! into, and every method on [`Stack`] is a short lock over that map and
//! nothing else. The board never waits for a decoder and the decoder never
//! waits for a frame.
//!
//! That inverts one thing worth being explicit about. `play` and `pause` here
//! do not start or stop anything; they record that the board would like the
//! card started or stopped, and the worker makes it so within a tick. Since
//! `BoardView::pump_media` already calls them on every frame for every playing
//! card — idempotently, by design — the difference does not reach the board.
//!
//! ## Frame-server mode, and why there is no Direct3D in this file
//!
//! Given no playback window and no DXGI device manager, the Media Engine runs
//! in *frame-server* mode: it decodes, and hands each frame to whoever asks.
//! `TransferVideoFrame` will blit into "a DXGI surface or WIC bitmap" — and the
//! WIC half is the one that needs no device, no swap chain and no `Map` of a
//! staging texture. An `IWICBitmap` in `32bppBGRA` is a rectangle of bytes in
//! ordinary memory, which is exactly what `RenderImage` wants.
//!
//! There is a real cost to admit: this is the software path, so a large clip is
//! decoded on the CPU and copied once. It is bounded by [`LONGEST_SIDE`] — the
//! engine letterboxes and scales into whatever rectangle it is given, so the
//! scaling happens *inside* the engine rather than in our own loop — and it now
//! happens on the worker rather than on the thread that draws.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{RenderImage, Window};
use image::{Frame, RgbaImage};

use windows::core::{implement, BSTR};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppBGRA, IWICBitmap, IWICImagingFactory,
    WICBitmapCacheOnDemand, WICBitmapLockRead, WICRect,
};
use windows::Win32::Media::MediaFoundation::{
    CLSID_MFMediaEngineClassFactory, IMFMediaEngine, IMFMediaEngineClassFactory,
    IMFMediaEngineNotify, IMFMediaEngineNotify_Impl, MFCreateAttributes, MFShutdown, MFStartup,
    MFSTARTUP_FULL, MF_MEDIA_ENGINE_CALLBACK, MF_MEDIA_ENGINE_ERR_ABORTED,
    MF_MEDIA_ENGINE_ERR_DECODE, MF_MEDIA_ENGINE_ERR_ENCRYPTED, MF_MEDIA_ENGINE_ERR_NETWORK,
    MF_MEDIA_ENGINE_ERR_SRC_NOT_SUPPORTED, MF_MEDIA_ENGINE_EVENT_ERROR,
    MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, MF_VERSION,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};

/// The longest edge a frame is transferred at.
///
/// A 4K frame is thirty-three megabytes to be drawn at a twentieth of its size.
/// The engine scales into the rectangle it is given, so asking for a smaller
/// rectangle is asking the decoder's own scaler to do the work — which is the
/// same bargain `videoscale` gives the Linux backend.
const LONGEST_SIDE: u32 = 1024;

/// How long the worker waits between ticks while something is playing.
///
/// A display's frame, near enough. The board consumes at most one picture per
/// drawn frame, so a worker running faster than this would decode frames for
/// nobody; one running much slower would be a video that stutters against a
/// board that does not.
const TICK: Duration = Duration::from_millis(16);

/// And while nothing is.
///
/// A board of photographs still has this thread on it — it is started by the
/// first card that plays and lives as long as the window — so the resting cost
/// has to be near enough nothing. Ten wakeups a second is what it costs to
/// notice a press within a frame or two of it happening.
const REST: Duration = Duration::from_millis(100);

/// What one frame of the clock says about a card that is playing.
///
/// The other two backends' twin. Kept in step by the fact that
/// `board_view.rs` reads every field of it — see `BoardView::pump_media`.
#[derive(Debug, Clone, Default)]
pub struct Beat {
    /// Where the playhead really is, according to the decoder rather than to a
    /// wall clock.
    pub at: Duration,
    /// How long the whole thing is, once the demuxer knows.
    pub length: Option<Duration>,
    /// The end was reached this frame.
    pub ended: bool,
    /// A new picture arrived this frame.
    pub fresh: bool,
    /// Something went wrong, said in words a person can read.
    pub trouble: Option<String>,
}

/// What asking for a card's decoder found.
///
/// Three answers rather than a `bool`, because the two halves of "no" mean
/// opposite things to the frame loop. A card whose file is still being laid
/// out on disk is one to ask about again on the next frame; a card this
/// machine cannot play is one to leave alone forever. See `crate::spill`,
/// which is where the middle answer comes from, and `BoardView::pump_media`,
/// which is what does the asking again.
///
/// On this backend `Waiting` covers one more case than it does on the other
/// two: the file may be laid out and the *engine* not built yet, because
/// building it is the worker's job and the worker runs on its own clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opening {
    /// There is a decoder for this card now.
    Ready,
    /// The file is still being unpacked, or the worker has not built the
    /// engine yet. Ask again.
    Waiting,
    /// There will not be one. The card has been told why, and `poll` will
    /// hand that sentence over once.
    Refused,
}

// ---------------------------------------------------------------------------
// What crosses between the board and the worker
// ---------------------------------------------------------------------------

/// What the board would like a card to be doing.
///
/// Written by the board, read by the worker, and *only* ever a wish: nothing
/// in here is a call, so nothing in here can block the thread that draws. The
/// worker brings the engine into line with it on its next tick.
#[derive(Debug, Clone, Copy)]
struct Want {
    playing: bool,
    looping: bool,
    volume: f32,
    muted: bool,
    /// Where the board dragged the playhead to. Taken by the worker rather
    /// than read, so a seek happens once and is not re-applied every tick —
    /// which would be a scrubber that could not be let go of.
    seek: Option<Duration>,
}

impl Default for Want {
    fn default() -> Self {
        // Full volume rather than silence, so a card whose loudness nobody has
        // set is audible. `BoardView::pump_media` overwrites this on the first
        // frame anyway; the default is what one tick before that sounds like.
        Self { playing: false, looping: false, volume: 1.0, muted: false, seek: None }
    }
}

/// What the worker found out about a card.
///
/// Written by the worker, drained by the board. `ended` and `fresh` are
/// *accumulated* rather than sampled: the worker may tick twice between two of
/// the board's frames, and an end that happened on the first of them must not
/// be lost because the second one did not also end.
#[derive(Debug, Default, Clone)]
struct News {
    at: Duration,
    length: Option<Duration>,
    ended: bool,
    fresh: bool,
    /// Set once by the worker. `told` is what stops it being said every frame
    /// — the same contract the other two backends' `Reel::told` holds.
    trouble: Option<String>,
    told: bool,
    picture: Option<Arc<RenderImage>>,
    /// Whether the worker has an engine for this card yet.
    live: bool,
}

/// One card, as both sides see it.
#[derive(Debug, Clone)]
struct Card {
    /// The file, already laid out on disk by [`crate::spill`]. Read once, by
    /// the worker, when it builds the engine.
    source: PathBuf,
    video: bool,
    want: Want,
    news: News,
    /// Since when this card has been standing still, for [`Stack::trim`].
    /// Board-side: it is the board that knows what it asked for.
    rested: Option<Instant>,
}

/// The whole of what the two threads share.
#[derive(Debug, Default)]
struct Shared {
    /// `None` until the worker has tried to bring Media Foundation up;
    /// `Some(false)` on a machine where it would not — Windows N without the
    /// Media Feature Pack is the real case, and it is somebody's actual
    /// computer.
    up: Option<bool>,
    /// Every card the board has asked for. **This map is the contract**: an id
    /// in here is a card the worker should have an engine for, and an id that
    /// leaves it is a card the worker should tear down. The worker keeps its
    /// engines in a map of its own and reconciles against this one, which is
    /// what makes "the board deleted a card" and "the board trimmed a card"
    /// one case instead of two.
    cards: HashMap<String, Card>,
    /// Pictures nothing will draw again, waiting for a window to hand their
    /// atlas tiles back. Filled by the worker as it replaces frames and by
    /// every path that drops a card; emptied by [`Stack::sweep`], which is the
    /// only one of the two sides that has a window.
    dropped: Vec<Arc<RenderImage>>,
}

// ---------------------------------------------------------------------------
// The board's side
// ---------------------------------------------------------------------------

/// Every engine there is, seen from the thread that draws.
///
/// Holds no COM object of any kind, deliberately — see the module note. What it
/// holds is a handle on the worker and a lock over what the two of them say to
/// each other.
pub struct Stack {
    shared: Arc<Mutex<Shared>>,
    /// Cleared on the way out, which is how the worker learns to stop.
    running: Arc<AtomicBool>,
    /// `false` until the first card asks to play. Starting a media stack scans
    /// codecs and stands a thread up, and a board of photographs should not
    /// pay for either.
    started: bool,
    spill: Option<crate::spill::Spill>,
}

impl Default for Stack {
    fn default() -> Self {
        Self::new()
    }
}

impl Stack {
    pub fn new() -> Self {
        Self {
            shared: Arc::default(),
            running: Arc::new(AtomicBool::new(true)),
            started: false,
            spill: None,
        }
    }

    /// Stand the worker up, once.
    ///
    /// Answers whether there is a thread to talk to, which is not the same
    /// question as whether Media Foundation came up on it — that one is the
    /// worker's to answer and arrives later, in `Shared::up`. A machine with no
    /// media stack therefore reports `Waiting` for a tick or two before it
    /// reports the refusal, which is right: "we do not know yet" is the honest
    /// answer until the thread has been round once.
    fn start(&mut self) -> bool {
        if self.started {
            return true;
        }
        self.started = true;
        self.spill = Some(crate::spill::Spill::open());

        let shared = self.shared.clone();
        let running = self.running.clone();
        std::thread::Builder::new()
            .name("mbrd-media".into())
            .spawn(move || work(&shared, &running))
            .is_ok()
    }

    /// Make sure there is an engine for this card, and say how far off one is.
    ///
    /// Idempotent, and cheap on every frame after the first: this is a lock, a
    /// map lookup and — on the frame a clip is first pressed — an `is_file`.
    /// `bytes` is only *copied* on the frame that starts the spill.
    pub fn open(&mut self, id: &str, hash: &str, ext: &str, bytes: &[u8], video: bool) -> Opening {
        if !self.start() {
            self.refuse(id, "the media thread would not start");
            return Opening::Refused;
        }

        // Looked at before the spill is asked, so a card that already has an
        // engine costs nothing at all.
        {
            let Ok(shared) = self.shared.lock() else {
                return Opening::Refused;
            };
            if shared.up == Some(false) {
                drop(shared);
                self.refuse(id, "no media stack on this machine");
                return Opening::Refused;
            }
            if let Some(card) = shared.cards.get(id) {
                return match (&card.news.trouble, card.news.live) {
                    (Some(_), _) => Opening::Refused,
                    (None, true) => Opening::Ready,
                    (None, false) => Opening::Waiting,
                };
            }
        }

        let laid = match self.spill.as_ref() {
            Some(spill) => spill.lay_out(hash, ext, bytes),
            None => crate::spill::Laid::Nowhere("nowhere to unpack this file".into()),
        };
        let source = match laid {
            crate::spill::Laid::Ready(path) => path,
            // Still being written, on a thread. Not a failure and not a card
            // yet — and deliberately not recorded, because a refusal is what
            // stops a card ever being tried again.
            crate::spill::Laid::Working => return Opening::Waiting,
            crate::spill::Laid::Nowhere(why) => {
                self.refuse(id, &why);
                return Opening::Refused;
            }
        };

        let Ok(mut shared) = self.shared.lock() else {
            return Opening::Refused;
        };
        shared.cards.insert(
            id.to_string(),
            Card {
                source,
                video,
                want: Want::default(),
                news: News::default(),
                // Built paused, so `rested` starts stamped: a card opened and
                // then never played is trimmable like any other.
                rested: Some(Instant::now()),
            },
        );
        // The engine is the worker's to build, and it has not been round since
        // this was inserted.
        Opening::Waiting
    }

    /// Record a failure against a card that never became one.
    ///
    /// A card rather than a map of its own, unlike the other two backends: here
    /// there is no COM object to stand in the struct's shape either way, so a
    /// refusal is just a card the worker will never find anything to do with —
    /// it has no `source` worth reading and its `trouble` is already set, which
    /// is the one thing `tick` checks before building anything.
    fn refuse(&mut self, id: &str, why: &str) {
        let Ok(mut shared) = self.shared.lock() else { return };
        let card = shared.cards.entry(id.to_string()).or_insert_with(|| Card {
            source: PathBuf::new(),
            video: false,
            want: Want::default(),
            news: News::default(),
            rested: Some(Instant::now()),
        });
        if card.news.trouble.is_none() {
            card.news.trouble = Some(why.to_string());
        }
    }

    /// Change what the board wants of one card.
    ///
    /// Every setter goes through here, because every one of them is the same
    /// three lines and because a setter that took the lock differently from its
    /// neighbours would be the one that held it too long.
    fn wish(&mut self, id: &str, change: impl FnOnce(&mut Card)) {
        let Ok(mut shared) = self.shared.lock() else { return };
        if let Some(card) = shared.cards.get_mut(id) {
            if card.news.trouble.is_none() {
                change(card);
            }
        }
    }

    /// Start, or carry on.
    ///
    /// Called every frame for every playing card rather than at the press — see
    /// `BoardView::pump_media`. Recording that a card should be playing when it
    /// already should be is a write of a `true` over a `true`, which is what
    /// makes "the decoder follows the playhead" affordable as a rule.
    pub fn play(&mut self, id: &str) {
        self.wish(id, |card| {
            card.rested = None;
            card.want.playing = true;
        });
    }

    pub fn pause(&mut self, id: &str) {
        self.wish(id, |card| {
            // Stamped once and not on every frame it stays paused, so the stamp
            // says when it stopped rather than when it was last asked.
            card.rested.get_or_insert_with(Instant::now);
            card.want.playing = false;
        });
    }

    /// Move the playhead. `at` is from the start.
    pub fn seek(&mut self, id: &str, at: Duration) {
        self.wish(id, |card| card.want.seek = Some(at));
    }

    /// How loud, `0.0..=1.0`, and whether it is silenced.
    pub fn set_loudness(&mut self, id: &str, level: f32, muted: bool) {
        self.wish(id, |card| {
            card.want.volume = level.clamp(0.0, 1.0);
            card.want.muted = muted;
        });
    }

    /// One frame of the clock for one card: what the worker has found since the
    /// last time this was asked.
    ///
    /// `looping` is taken rather than remembered because it is board state and
    /// can change under a playing card. It is recorded on the way past, which
    /// is the one write this method does.
    pub fn poll(&mut self, id: &str, looping: bool) -> Option<Beat> {
        let mut shared = self.shared.lock().ok()?;
        let card = shared.cards.get_mut(id)?;
        card.want.looping = looping;

        if let Some(why) = &card.news.trouble {
            // Once. A card that says "no decoder for this" every frame would
            // hold the status bar for as long as it is on screen.
            if card.news.told {
                return None;
            }
            card.news.told = true;
            return Some(Beat { trouble: Some(why.clone()), ..Beat::default() });
        }

        Some(Beat {
            at: card.news.at,
            length: card.news.length,
            // Taken, not read: these are things that *happened*, and a happening
            // reported twice is a loop that restarts twice.
            ended: std::mem::take(&mut card.news.ended),
            fresh: std::mem::take(&mut card.news.fresh),
            trouble: None,
        })
    }

    /// The newest picture for a card, for the painter.
    pub fn picture(&self, id: &str) -> Option<Arc<RenderImage>> {
        self.shared.lock().ok()?.cards.get(id)?.news.picture.clone()
    }

    /// Which cards have something standing, so the frame loop knows what to
    /// poll without walking the board. Includes the ones that failed to open,
    /// which is how their one message gets out.
    pub fn open_reels(&self) -> Vec<String> {
        match self.shared.lock() {
            Ok(shared) => shared.cards.keys().cloned().collect(),
            Err(_) => Vec::new(),
        }
    }

    /// Tear one down — the card was deleted, stopped, or trimmed.
    ///
    /// Taking it out of the map is the whole of it. The engine belongs to the
    /// worker, which shuts down anything it holds that the map no longer names
    /// — on its own thread, where a blocking `Shutdown` costs nobody a frame.
    /// That is the point of the reconciliation: this call cannot hang.
    pub fn forget(&mut self, id: &str) {
        if let Ok(mut shared) = self.shared.lock() {
            let gone = shared.cards.remove(id).and_then(|card| card.news.picture);
            shared.dropped.extend(gone);
        }
    }

    /// Release the atlas tiles of every picture retired since the last call.
    ///
    /// **A video is a new picture thirty times a second, and every one of them
    /// takes a tile in the sprite atlas that nothing else ever gives back.**
    /// The atlas is a cache keyed by image id with no eviction in it —
    /// `Window::drop_image` is the only door out — so a clip left playing would
    /// otherwise grow the texture it draws from until the GPU refused another.
    ///
    /// Call once a frame from somewhere with a window, beside
    /// [`Images::sweep`](crate::images::Images::sweep) and
    /// [`Live::sweep`](crate::live::Live::sweep), which exist for the same
    /// reason and are swept in the same breath.
    pub fn sweep(&mut self, window: &mut Window) {
        // Taken out from under the lock and dropped outside it: the worker
        // wants this mutex sixty times a second, and a frame handed back to the
        // GPU is not something to hold it across.
        let Ok(mut shared) = self.shared.lock() else { return };
        let dropped = std::mem::take(&mut shared.dropped);
        drop(shared);
        for image in dropped {
            // Best effort, for the reason `images.rs` gives: a tile that was
            // never uploaded has nothing to drop, and a window on its way out
            // will not take instructions.
            let _ = window.drop_image(image);
        }
    }

    /// Everything, for a board being closed.
    pub fn forget_all(&mut self) {
        if let Ok(mut shared) = self.shared.lock() {
            let gone: Vec<_> =
                shared.cards.drain().filter_map(|(_, card)| card.news.picture).collect();
            shared.dropped.extend(gone);
        }
    }

    /// Drop the cards that have been standing still longest, down to `keep`.
    /// See `pipeline::Stack::trim`, which holds the argument.
    pub fn trim(&mut self, keep: usize) {
        let Ok(mut shared) = self.shared.lock() else { return };
        let mut resting: Vec<(String, Instant)> = shared
            .cards
            .iter()
            .filter_map(|(id, card)| card.rested.map(|at| (id.clone(), at)))
            .collect();
        if resting.len() <= keep {
            return;
        }
        resting.sort_by_key(|(_, at)| *at);
        let over = resting.len() - keep;
        let gone: Vec<_> = resting
            .into_iter()
            .take(over)
            .filter_map(|(id, _)| shared.cards.remove(&id))
            .filter_map(|card| card.news.picture)
            .collect();
        shared.dropped.extend(gone);
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        // Emptied first, so that the last thing the worker does before it
        // notices it is finished is shut every engine down — see `work`, which
        // reconciles before it checks whether to carry on.
        self.forget_all();
        self.running.store(false, Ordering::Relaxed);
        // **Not joined**, and that is the whole reason this thread exists.
        // Joining would put the thread that draws back inside the blocking
        // `Shutdown` and `MFShutdown` calls this file was rewritten to get it
        // out of — at the one moment, a window closing, when the operating
        // system is already watching for an application that has stopped
        // answering. The worker holds nothing the process needs on the way
        // out: its engines are its own and its last act is to release them.
    }
}

// ---------------------------------------------------------------------------
// The worker's side
// ---------------------------------------------------------------------------

/// One card's engine. Never leaves the worker thread.
struct Reel {
    engine: IMFMediaEngine,
    signals: Arc<Signals>,
    /// Where frames are transferred to, made once the engine knows how big the
    /// video is. `None` for sound, and `None` for a video whose first frame has
    /// not arrived yet — the native size is not known until it has.
    bitmap: Option<(IWICBitmap, u32, u32)>,
    /// Whether this card has pictures at all.
    video: bool,
    /// The presentation time of the frame already copied.
    ///
    /// `OnVideoStreamTick` answers `S_FALSE` when there is nothing new, and the
    /// Rust binding folds `S_FALSE` into `Ok` — it is a success code — so the
    /// honest test for "a new frame" is that the time moved. That is also the
    /// more robust one: a repeated timestamp is the same picture whatever the
    /// return code said.
    shown: Option<i64>,
    length: Option<Duration>,
    /// Set once, then never asked again — the card carries the sentence back to
    /// the board, and this is what stops the worker touching a dead engine.
    broken: bool,
}

/// What the engine's callback thread is allowed to say.
///
/// One atomic and nothing else. The callback runs on a Media Foundation work
/// queue and neither the board nor the worker is on it, so what crosses is a
/// flag rather than a decision.
///
/// Only the failure is here. *Ended* is not, even though there is an event for
/// it, because `IsEnded` answers the same question from the worker's own tick
/// and is the only one of the two that stays right across a seek and a loop —
/// an event flag would have to be un-set by hand in three places, and the third
/// one is always the one that gets forgotten.
#[derive(Default)]
struct Signals {
    failed: AtomicBool,
}

/// The callback the Media Engine will not be created without.
#[implement(IMFMediaEngineNotify)]
struct Notify(Arc<Signals>);

impl IMFMediaEngineNotify_Impl for Notify_Impl {
    fn EventNotify(&self, event: u32, _param1: usize, _param2: u32) -> windows::core::Result<()> {
        // Compared as numbers rather than matched as patterns: these constants
        // are newtypes over `i32`, and a `match` arm against one is a structural
        // pattern the compiler will not build from a `const` of a tuple struct.
        if event as i32 == MF_MEDIA_ENGINE_EVENT_ERROR.0 {
            self.0.failed.store(true, Ordering::Relaxed);
        }
        Ok(())
    }
}

/// Everything a tick found out about one card, on its way back to the board.
#[derive(Default)]
struct Found {
    at: Duration,
    length: Option<Duration>,
    ended: bool,
    picture: Option<Arc<RenderImage>>,
    trouble: Option<String>,
    live: bool,
}

/// The worker.
///
/// Owns the apartment, the two factories and every engine. Runs until the
/// `Stack` that started it goes away.
fn work(shared: &Arc<Mutex<Shared>>, running: &Arc<AtomicBool>) {
    // The whole reason this thread exists — see the module note. Paired with a
    // `CoUninitialize` at the very end, and that pairing is real here, unlike
    // the main thread's: this apartment is one this code entered.
    let entered = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();

    let up = entered && unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.is_ok();
    let factory: Option<IMFMediaEngineClassFactory> = up
        .then(|| unsafe {
            CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)
        })
        .and_then(Result::ok);
    let imaging: Option<IWICImagingFactory> = up
        .then(|| unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) })
        .and_then(Result::ok);

    let (Some(factory), Some(imaging)) = (factory, imaging) else {
        if let Ok(mut lock) = shared.lock() {
            lock.up = Some(false);
        }
        // Nothing to run, but the apartment and possibly Media Foundation are
        // up and have to come back down in the right order.
        if up {
            let _ = unsafe { MFShutdown() };
        }
        if entered {
            unsafe { CoUninitialize() };
        }
        return;
    };
    if let Ok(mut lock) = shared.lock() {
        lock.up = Some(true);
    }

    let mut reels: HashMap<String, Reel> = HashMap::new();
    loop {
        let busy = tick(&mut reels, shared, &factory, &imaging);
        // Checked *after* a tick rather than before, so the last thing this
        // thread does is the reconciliation that shuts every engine down — the
        // board empties the map and clears the flag in that order, and this
        // reads them in the same one.
        if !running.load(Ordering::Relaxed) {
            break;
        }
        std::thread::sleep(if busy { TICK } else { REST });
    }

    // Every engine down before Media Foundation is, and Media Foundation down
    // before the apartment is. Shutting either out from under a live object is
    // how a process comes to crash on the way out.
    for (_, reel) in reels.drain() {
        let _ = unsafe { reel.engine.Shutdown() };
    }
    let _ = unsafe { MFShutdown() };
    unsafe { CoUninitialize() };
}

/// One pass over every card, and whether anything is playing.
///
/// The lock is taken twice and held across neither the frame copy nor any COM
/// call, which is the rule this whole design exists to keep: the thread that
/// draws asks this map questions on every frame, and a lock held across a
/// `TransferVideoFrame` would be that thread waiting on a decoder again.
fn tick(
    reels: &mut HashMap<String, Reel>,
    shared: &Arc<Mutex<Shared>>,
    factory: &IMFMediaEngineClassFactory,
    imaging: &IWICImagingFactory,
) -> bool {
    // ---- 1. What the board wants, as of now.
    let mut asked: Vec<(String, PathBuf, bool, Want)> = Vec::new();
    {
        let Ok(mut lock) = shared.lock() else { return false };
        for (id, card) in lock.cards.iter_mut() {
            if card.news.trouble.is_some() {
                continue;
            }
            // Taken here so a seek is applied once. See `Want::seek`.
            let want = Want { seek: card.want.seek.take(), ..card.want };
            asked.push((id.clone(), card.source.clone(), card.video, want));
        }
    }

    // ---- 2. Anything the board has stopped naming is the worker's to release.
    let named: HashSet<&str> = asked.iter().map(|(id, ..)| id.as_str()).collect();
    let gone: Vec<String> =
        reels.keys().filter(|id| !named.contains(id.as_str())).cloned().collect();
    for id in gone {
        if let Some(reel) = reels.remove(&id) {
            // `Shutdown` first and then dropped. An engine released while
            // playing takes its audio endpoint with it whenever COM notices;
            // shutting it down first is what makes the sound end when the card
            // does. It blocks, and on this thread that is nobody's problem.
            let _ = unsafe { reel.engine.Shutdown() };
        }
    }

    // ---- 3. Everything that is still here, brought up to date.
    let mut news: Vec<(String, Found)> = Vec::new();
    let mut busy = false;
    for (id, source, video, want) in asked {
        if !reels.contains_key(&id) {
            match build(factory, &source, video) {
                Ok(reel) => {
                    reels.insert(id.clone(), reel);
                }
                Err(why) => {
                    news.push((id, Found { trouble: Some(why), ..Found::default() }));
                    continue;
                }
            }
        }
        let Some(reel) = reels.get_mut(&id) else { continue };
        busy |= want.playing;
        news.push((id, drive(reel, want, imaging)));
    }

    // ---- 4. And what was found, handed back.
    let Ok(mut lock) = shared.lock() else { return busy };
    // The pictures this pass replaced, gathered while the cards are borrowed
    // and handed over once they are not. See [`Stack::sweep`].
    let mut retired: Vec<Arc<RenderImage>> = Vec::new();
    for (id, found) in news {
        let Some(card) = lock.cards.get_mut(&id) else { continue };
        card.news.live = found.live;
        card.news.at = found.at;
        if found.length.is_some() {
            card.news.length = found.length;
        }
        // Or-ed rather than assigned: the board may not have looked since the
        // last tick, and an end that happened then is still an end.
        card.news.ended |= found.ended;
        if let Some(picture) = found.picture {
            retired.extend(card.news.picture.replace(picture));
            card.news.fresh = true;
        }
        if card.news.trouble.is_none() {
            card.news.trouble = found.trouble;
        }
    }
    lock.dropped.append(&mut retired);
    busy
}

/// One engine, brought into line with what the board wants, and asked where it
/// has got to.
fn drive(reel: &mut Reel, want: Want, imaging: &IWICImagingFactory) -> Found {
    if reel.broken {
        return Found::default();
    }
    // The failure first, because what it has to say outranks whatever position
    // the engine would otherwise report.
    if reel.signals.failed.swap(false, Ordering::Relaxed) {
        let why = said(&reel.engine);
        reel.broken = true;
        let _ = unsafe { reel.engine.Shutdown() };
        return Found { trouble: Some(why), ..Found::default() };
    }

    let mut found = Found { live: true, ..Found::default() };

    unsafe {
        let _ = reel.engine.SetLoop(want.looping);
        let _ = reel.engine.SetVolume(f64::from(want.volume));
        let _ = reel.engine.SetMuted(want.muted);
        if let Some(at) = want.seek {
            let _ = reel.engine.SetCurrentTime(at.as_secs_f64());
        }
        // `Play` on an engine that is already playing is a no-op, and so is
        // `Pause` on one that is already paused — which is what lets this be
        // said on every tick rather than only on the ticks it changed.
        match want.playing {
            true => {
                let _ = reel.engine.Play();
            }
            false => {
                let _ = reel.engine.Pause();
            }
        }
    }

    // Not known until the engine has read the file's metadata, and `NaN` until
    // then — which is why this is asked for until it answers rather than once.
    // A live stream answers infinity and is left with no length at all, because
    // a scrubber across a stream is a scrubber across nothing.
    if reel.length.is_none() {
        let seconds = unsafe { reel.engine.GetDuration() };
        if seconds.is_finite() && seconds > 0.0 {
            reel.length = Some(Duration::from_secs_f64(seconds));
        }
    }
    found.length = reel.length;

    let at = unsafe { reel.engine.GetCurrentTime() };
    found.at = match at.is_finite() && at > 0.0 {
        true => Duration::from_secs_f64(at),
        false => Duration::ZERO,
    };

    if reel.video {
        found.picture = take_frame(reel, imaging);
    }

    // Asked, not remembered — see `Signals`. The engine has already been told
    // whether to loop, so a looping clip never reads as ended and the gapless
    // restart happens inside it rather than here.
    if unsafe { reel.engine.IsEnded() }.as_bool() {
        found.ended = true;
        // Held at the end rather than reset. A clip that snapped back to its
        // first frame the instant it finished would be one you could never see
        // the end of.
        let _ = unsafe { reel.engine.Pause() };
    }

    found
}

/// Build a Media Engine for one file.
fn build(factory: &IMFMediaEngineClassFactory, path: &Path, video: bool) -> Result<Reel, String> {
    let text = path.to_str().ok_or("that file is somewhere this cannot name")?;

    let signals = Arc::new(Signals::default());
    let notify: IMFMediaEngineNotify = Notify(signals.clone()).into();

    let mut attributes = None;
    unsafe { MFCreateAttributes(&mut attributes, 2) }
        .map_err(|_| "could not ask for a media engine".to_string())?;
    let attributes = attributes.ok_or("could not ask for a media engine")?;

    unsafe {
        // Required. `CreateInstance` refuses without a callback, which is the
        // whole reason `Notify` exists.
        attributes
            .SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &notify)
            .map_err(|_| "could not attach the media callback".to_string())?;
        // What frames come back as. BGRA, because that is the order
        // `RenderImage` reads an `RgbaImage`'s bytes in — see the module note
        // on the other two backends, which ask their own way for the same
        // thing.
        attributes
            .SetUINT32(&MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM.0 as u32)
            .map_err(|_| "could not ask for a drawable frame".to_string())?;
    }

    // No window and no DXGI manager, which is what puts the engine in
    // frame-server mode — see the module note.
    let engine = unsafe { factory.CreateInstance(0, &attributes) }
        .map_err(|_| "this machine has no media engine — is the Media Feature Pack installed?")?;

    unsafe {
        // Loading is what `SetSource` starts; playing is the board's decision
        // and arrives later, through `Want::playing`.
        let _ = engine.SetAutoPlay(false);
        engine
            .SetSource(&BSTR::from(text))
            .map_err(|_| "nothing on this machine can open that file".to_string())?;
    }

    Ok(Reel { engine, signals, bitmap: None, video, shown: None, length: None, broken: false })
}

/// Take the current frame, if it is one we have not already taken.
fn take_frame(reel: &mut Reel, imaging: &IWICImagingFactory) -> Option<Arc<RenderImage>> {
    // `S_FALSE` means "nothing new" and folds into `Ok` here, so the timestamp
    // is what decides — see `Reel::shown`.
    let pts = (unsafe { reel.engine.OnVideoStreamTick() }).ok()?;
    if pts < 0 || Some(pts) == reel.shown {
        return None;
    }

    // The size is not known until the engine has decoded far enough to know it,
    // which is why the bitmap is made here rather than in `build`.
    if reel.bitmap.is_none() {
        let (mut wide, mut high) = (0u32, 0u32);
        unsafe {
            reel.engine.GetNativeVideoSize(Some(&mut wide as *mut u32), Some(&mut high as *mut u32))
        }
        .ok()?;
        let (wide, high) = fit_inside(wide, high)?;
        let bitmap = unsafe {
            imaging.CreateBitmap(wide, high, &GUID_WICPixelFormat32bppBGRA, WICBitmapCacheOnDemand)
        }
        .ok()?;
        reel.bitmap = Some((bitmap, wide, high));
    }
    let (bitmap, wide, high) = reel.bitmap.as_ref()?;
    let (wide, high) = (*wide, *high);

    // The rectangle is the whole bitmap, and the bitmap keeps the video's own
    // shape — so the engine's letterboxing has nothing to letterbox and the
    // border colour never shows. That is why it is passed as null.
    let into = RECT { left: 0, top: 0, right: wide as i32, bottom: high as i32 };
    unsafe { reel.engine.TransferVideoFrame(bitmap, None, &into, None) }.ok()?;

    let picture = read_bitmap(bitmap, wide, high);
    if picture.is_some() {
        reel.shown = Some(pts);
    }
    picture
}

/// A locked WIC bitmap, as a picture the canvas can draw.
///
/// The copy is row by row rather than one `to_vec`, because WIC is entitled to
/// pad every row out to an alignment it likes: a 1918-pixel-wide frame commonly
/// arrives with a stride of 1920 times four, and reading it as though it were
/// tight produces a picture that shears diagonally.
fn read_bitmap(bitmap: &IWICBitmap, wide: u32, high: u32) -> Option<Arc<RenderImage>> {
    let whole = WICRect { X: 0, Y: 0, Width: wide as i32, Height: high as i32 };
    // Read-only, so WIC is free to keep the pixels wherever it likes rather
    // than staging a copy somewhere we could write.
    let lock = unsafe { bitmap.Lock(&whole, WICBitmapLockRead.0 as u32) }.ok()?;

    let stride = unsafe { lock.GetStride() }.ok()? as usize;
    let mut size = 0u32;
    let mut base: *mut u8 = std::ptr::null_mut();
    unsafe { lock.GetDataPointer(&mut size, &mut base) }.ok()?;
    if base.is_null() {
        return None;
    }

    let row = (wide as usize).checked_mul(4)?;
    if stride < row {
        return None;
    }
    // SAFETY: the lock is alive for the whole of this function — it is dropped
    // at the end of the scope, which is what unlocks the bitmap — and WIC
    // reports the size of the region it made valid.
    let data = unsafe { std::slice::from_raw_parts(base as *const u8, size as usize) };

    let mut out = Vec::with_capacity(row.checked_mul(high as usize)?);
    for y in 0..high as usize {
        out.extend_from_slice(data.get(y * stride..y * stride + row)?);
    }

    // Already BGRA — see the module note. `RenderImage` reads an `RgbaImage`'s
    // bytes in that order, so this is a rename rather than a conversion.
    let image = RgbaImage::from_raw(wide, high, out)?;
    Some(Arc::new(RenderImage::new(vec![Frame::new(image)])))
}

/// The size to transfer a frame at: the video's own shape, brought inside
/// [`LONGEST_SIDE`].
///
/// `None` for a video with no size yet, which is what the engine reports before
/// it has read far enough in — and is a "not yet" rather than a failure, so the
/// caller tries again on the next tick.
fn fit_inside(wide: u32, high: u32) -> Option<(u32, u32)> {
    if wide == 0 || high == 0 {
        return None;
    }
    let longest = wide.max(high);
    if longest <= LONGEST_SIDE {
        return Some((wide, high));
    }
    // Rounded through f64 rather than integer arithmetic, so a 1920x1080 frame
    // comes back 1024x576 rather than 1024x575 — a shape half a pixel out is a
    // shape the engine letterboxes into, and the border would show as a line.
    let scale = LONGEST_SIDE as f64 / longest as f64;
    let wide = ((wide as f64 * scale).round() as u32).max(1);
    let high = ((high as f64 * scale).round() as u32).max(1);
    Some((wide, high))
}

/// What an engine's error means, in words a person can read.
fn said(engine: &IMFMediaEngine) -> String {
    let Ok(error) = (unsafe { engine.GetError() }) else {
        return "this file could not be played".to_string();
    };
    let code = unsafe { error.GetErrorCode() } as i32;
    let why = if code == MF_MEDIA_ENGINE_ERR_SRC_NOT_SUPPORTED.0 {
        "this machine has no decoder for that file"
    } else if code == MF_MEDIA_ENGINE_ERR_DECODE.0 {
        "that file is damaged, or is not what its name says"
    } else if code == MF_MEDIA_ENGINE_ERR_ENCRYPTED.0 {
        "that file is protected and cannot be played here"
    } else if code == MF_MEDIA_ENGINE_ERR_NETWORK.0 {
        "that file could not be read"
    } else if code == MF_MEDIA_ENGINE_ERR_ABORTED.0 {
        "that file stopped part way through"
    } else {
        "this file could not be played"
    };
    why.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one piece of arithmetic in this file, and one of the few things in
    /// it that can be tested without a Windows machine in the room.
    #[test]
    fn a_frame_is_brought_inside_the_ceiling_with_its_shape_intact() {
        assert_eq!(fit_inside(1920, 1080), Some((1024, 576)));
        assert_eq!(fit_inside(1024, 768), Some((1024, 768)), "already inside, left alone");
        assert_eq!(fit_inside(640, 480), Some((640, 480)), "never enlarged");
        // Portrait counts too — it is the longest edge, not the width.
        assert_eq!(fit_inside(1080, 1920), Some((576, 1024)));
    }

    /// A video whose size the engine has not worked out yet is a "not yet",
    /// and a frame with no pixels in it would be a panic in `RgbaImage`.
    #[test]
    fn a_size_that_is_not_known_yet_is_no_answer_rather_than_a_zero() {
        assert_eq!(fit_inside(0, 0), None);
        assert_eq!(fit_inside(1920, 0), None);
        let (wide, high) = fit_inside(8000, 1).unwrap();
        assert!(wide >= 1 && high >= 1);
    }

    /// The board's half of the seam is plain data, so what it does with a
    /// worker that has not been round yet can be checked anywhere.
    ///
    /// The important line is the last one. `ended` and `fresh` are things that
    /// *happened*, and a `poll` that read them instead of taking them would
    /// report the same end on every frame — which on a looping card is a clip
    /// that restarts sixty times a second.
    #[test]
    fn a_happening_is_reported_once_and_a_position_every_time() {
        let mut stack = Stack::new();
        stack.plant(
            "card",
            News {
                at: Duration::from_secs(3),
                ended: true,
                fresh: true,
                live: true,
                ..News::default()
            },
        );

        let first = stack.poll("card", false).expect("a card the board knows about");
        assert_eq!(first.at, Duration::from_secs(3));
        assert!(first.ended && first.fresh);

        let second = stack.poll("card", false).expect("still there");
        assert_eq!(second.at, Duration::from_secs(3), "the position is where it is, every time");
        assert!(!second.ended && !second.fresh, "a happening is not reported twice");
    }

    /// Trouble is said once and then the card goes quiet, so a file with no
    /// decoder does not hold the status bar for as long as it is on screen.
    #[test]
    fn a_refusal_is_said_once() {
        let mut stack = Stack::new();
        stack.refuse("card", "no decoder for that");

        let first = stack.poll("card", false).expect("a card with something to say");
        assert_eq!(first.trouble.as_deref(), Some("no decoder for that"));
        assert!(stack.poll("card", false).is_none(), "and nothing after it");
    }

    /// Forgetting is a removal and not a call, which is what makes it unable to
    /// block the thread that draws — the engine is the worker's to release.
    #[test]
    fn forgetting_a_card_takes_it_out_of_the_map_and_does_not_wait_for_anything() {
        let mut stack = Stack::new();
        stack.refuse("card", "whatever");
        assert_eq!(stack.open_reels(), vec!["card".to_string()]);
        stack.forget("card");
        assert!(stack.open_reels().is_empty());
    }

    /// The trim keeps the ones that stopped most recently.
    #[test]
    fn trimming_drops_the_ones_that_have_been_still_longest() {
        let mut stack = Stack::new();
        for (id, ago) in [("old", 300u64), ("middle", 200), ("new", 100)] {
            stack.plant(id, News { live: true, ..News::default() });
            stack.wish(id, |card| {
                card.rested = Instant::now().checked_sub(Duration::from_secs(ago));
            });
        }
        stack.trim(1);
        assert_eq!(stack.open_reels(), vec!["new".to_string()]);
    }

    impl Stack {
        /// A card in the map as though the worker had put it there, for the
        /// tests above — the worker itself needs a Windows machine, and the
        /// board's half of the contract does not.
        fn plant(&mut self, id: &str, news: News) {
            let Ok(mut shared) = self.shared.lock() else { return };
            shared.cards.insert(
                id.to_string(),
                Card {
                    source: PathBuf::from("nowhere"),
                    video: true,
                    want: Want::default(),
                    news,
                    rested: None,
                },
            );
        }
    }
}
