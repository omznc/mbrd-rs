//! Sound and pictures on macOS, through AVFoundation.
//!
//! The same door as [`crate::pipeline`], opening onto a different room. One
//! `AVPlayer` per card that is playing, sound straight out to the system, and
//! video pulled back a frame at a time through an `AVPlayerItemVideoOutput`.
//! `board_view.rs` cannot tell which of the three backends it is holding, and
//! that is the point — see `pipeline_off.rs` for the seam.
//!
//! ## Why not the same GStreamer everywhere
//!
//! Because GStreamer is a *link-time* dependency, and satisfying it here would
//! mean `GStreamer.framework` inside `mbrd.app` — a couple of hundred megabytes
//! of decoders, an `install_name_tool` pass, and a real signing identity to
//! replace the ad-hoc signature the release uses today. AVFoundation is already
//! in the operating system, hardware-accelerated, and is what every other
//! application on the machine plays video with. The cost is this file.
//!
//! It is not a smaller feature, either: `AVPlayer` reads every container macOS
//! knows — H.264, HEVC, ProRes, AAC, ALAC, MP3 — which is everything a phone or
//! a camera produces.
//!
//! ## Polled, not observed
//!
//! AVFoundation's natural shape is KVO and `NSNotification`: watch
//! `AVPlayerItem.status`, subscribe to `AVPlayerItemDidPlayToEndTimeNotification`.
//! **Nothing here does that**, for the same reason the Linux backend drains its
//! bus by hand — the board already runs a frame loop for as long as anything is
//! playing, so the questions are asked from that loop, on the thread that draws,
//! and no observer ever reaches into a view it does not own.
//!
//! The end of a clip is the one thing that has no polled flag of its own, so it
//! is read off `timeControlStatus` — see [`Reel::wanted`].
//!
//! ## BGRA, because that is what gpui draws
//!
//! The video output is asked for `kCVPixelFormatType_32BGRA` rather than being
//! given whatever the decoder prefers, which on this platform is a biplanar YUV
//! that would need converting by hand. `RenderImage` holds an `RgbaImage` whose
//! bytes it reads as BGRA, so the copy out of the pixel buffer is a copy and
//! nothing else.
//!
//! ## Main thread
//!
//! `AVPlayer` and `AVPlayerItem` are `MainThreadOnly`, which objc2 enforces by
//! asking for a `MainThreadMarker`. That is not a constraint here — every call
//! in this file happens inside gpui's render or event handling, which is the
//! main thread — but it is why [`Stack::start`] can fail: a machine is fine, a
//! *thread* is what would be wrong, and that is worth saying rather than
//! panicking.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{RenderImage, Window};
use image::{Frame, RgbaImage};

use objc2::rc::Retained;
use objc2::{AnyThread, MainThreadMarker};
use objc2_av_foundation::{
    AVPlayer, AVPlayerActionAtItemEnd, AVPlayerItem, AVPlayerItemStatus, AVPlayerItemVideoOutput,
    AVPlayerStatus, AVPlayerTimeControlStatus,
};
use objc2_core_media::{CMTime, CMTimeFlags};
use objc2_core_video::{
    kCVPixelBufferHeightKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey,
    kCVPixelFormatType_32BGRA, CVPixelBuffer, CVPixelBufferGetBaseAddress,
    CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight, CVPixelBufferGetWidth,
    CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress,
};
use objc2_foundation::{NSDictionary, NSNumber, NSString, NSURL};

/// The longest edge a frame reaches the board at.
///
/// A 4K frame is thirty-three megabytes to be drawn at a twentieth of its size.
/// This is asked for **twice**, and both halves matter.
///
/// The decoder is asked first, through the video output's pixel buffer
/// attributes — see [`resize_output`]. That is the half that was missing: the
/// attributes used to name only the pixel format, so every frame of a 4K clip
/// was decoded at full size and then reduced by hand on the thread that draws,
/// which is the one thread that cannot afford thirty-three megabytes sixty
/// times a second.
///
/// It cannot be asked at the moment the output is built, because nothing is
/// known about the file then — an `AVPlayerItem` that has not loaded reports
/// `CGSizeZero`, and a size guessed before the aspect ratio is known is a
/// picture stretched. So the output starts unsized and is swapped for a sized
/// one the first frame the item can answer.
///
/// [`shrink_to`] is the second half and stays regardless: it is what makes the
/// first frames — the ones that arrive before the swap — and any frame from a
/// decoder that declined the hint still land at a sane size. It reads the
/// buffer's own width and stride rather than assuming either, so the two halves
/// cannot disagree about what arrived.
const LONGEST_SIDE: usize = 1024;

/// What asking for a card's decoder found.
///
/// Three answers rather than a `bool`, because the two halves of "no" mean
/// opposite things to the frame loop. A card whose file is still being laid
/// out on disk is one to ask about again on the next frame; a card this
/// machine cannot play is one to leave alone forever. See `crate::spill`,
/// which is where the middle answer comes from, and `BoardView::pump_media`,
/// which is what does the asking again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opening {
    /// There is a decoder for this card now.
    Ready,
    /// The file is still being unpacked. Ask again.
    Waiting,
    /// There will not be one. The card has been told why, and `poll` will
    /// hand that sentence over once.
    Refused,
}

/// What one frame of the clock says about a card that is playing.
///
/// The Linux backend's twin. Kept in step by the fact that `board_view.rs`
/// reads every field of it — see `BoardView::pump_media`.
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

/// One card's player.
struct Reel {
    player: Retained<AVPlayer>,
    item: Retained<AVPlayerItem>,
    /// Where frames come back, on a card that has pictures. `None` for sound,
    /// which needs nothing back: `AVPlayer` reaches the speakers by itself.
    output: Option<Retained<AVPlayerItemVideoOutput>>,
    /// The newest frame, kept so the painter has something to draw on every
    /// frame rather than only on the ones a new picture arrived in.
    picture: Option<Arc<RenderImage>>,
    length: Option<Duration>,
    /// Set once, then reported once. A failed reel is left in place rather than
    /// dropped, so nothing opens it again on the very next frame.
    broken: Option<String>,
    told: bool,
    /// Since when this reel has been standing still, for [`Stack::trim`].
    rested: Option<Instant>,
    /// Whether the video output has been asked for frames at the size they
    /// will be drawn.
    ///
    /// `true` from the start on a card with no pictures, which has no output
    /// to size, and on a clip already small enough to be worth leaving alone.
    /// See [`LONGEST_SIDE`], which is about why this cannot be settled when
    /// the reel is built.
    sized: bool,
    /// Whether we have asked it to play.
    ///
    /// This is what tells "it finished" from "somebody paused it".
    /// `actionAtItemEnd` is set to `Pause`, so a clip that runs out stops
    /// itself — and a player that stopped itself while we still wanted it
    /// playing is a player that reached the end. Buffering does not look like
    /// this: a stalled `AVPlayer` reports `WaitingToPlayAtSpecifiedRate`, which
    /// is a third state and not `Paused`.
    wanted: bool,
}

/// Every player there is, and the one-time setup behind them.
pub struct Stack {
    /// `None` until somebody asks for the first time. `Some(false)` where the
    /// first ask did not come from the main thread, which is a state rather
    /// than a crash.
    started: Option<bool>,
    spill: Option<crate::spill::Spill>,
    reels: HashMap<String, Reel>,
    /// Cards that never became a reel at all, and the reason. Kept apart from
    /// `reels` rather than folded in as a `Reel` with every field empty: there
    /// is no `AVPlayer` that means "nothing", and inventing one to hold the
    /// shape of the struct would be a player that could be told to play.
    broken: HashMap<String, String>,
    /// Pictures nothing will draw again, waiting for a window to hand their
    /// atlas tiles back. See [`Stack::sweep`].
    dropped: Vec<Arc<RenderImage>>,
}

impl Default for Stack {
    fn default() -> Self {
        Self::new()
    }
}

impl Stack {
    pub fn new() -> Self {
        Self {
            started: None,
            spill: None,
            reels: HashMap::new(),
            broken: HashMap::new(),
            dropped: Vec::new(),
        }
    }

    /// Open the spill directory, once, and answer whether this can play at all.
    ///
    /// Lazy rather than at startup: a board of photographs never touches this,
    /// and making the cache directory is not something to make every launch pay
    /// for on the chance somebody drops a video in.
    fn start(&mut self) -> bool {
        if let Some(known) = self.started {
            return known;
        }
        let up = MainThreadMarker::new().is_some();
        self.started = Some(up);
        if up {
            self.spill = Some(crate::spill::Spill::open());
        }
        up
    }

    /// Make sure there is a player for this card, and say how far off one is.
    ///
    /// Idempotent, and cheap on every frame after the first. `bytes` is only
    /// *copied* on the frame that starts the spill — see [`crate::spill`],
    /// which is also where the third answer comes from.
    pub fn open(&mut self, id: &str, hash: &str, ext: &str, bytes: &[u8], video: bool) -> Opening {
        if self.reels.contains_key(id) {
            return match self.reels[id].broken.is_none() {
                true => Opening::Ready,
                false => Opening::Refused,
            };
        }
        if !self.start() {
            self.fail(id, "media can only be opened from the main thread".into());
            return Opening::Refused;
        }
        let Some(mtm) = MainThreadMarker::new() else {
            self.fail(id, "media can only be opened from the main thread".into());
            return Opening::Refused;
        };
        let laid = match self.spill.as_ref() {
            Some(spill) => spill.lay_out(hash, ext, bytes),
            None => crate::spill::Laid::Nowhere("nowhere to unpack this file".into()),
        };
        let path = match laid {
            crate::spill::Laid::Ready(path) => path,
            // Still being written, on a thread. Not a failure and not a reel
            // yet — and deliberately *not* recorded against the card, because
            // `fail` is what stops a card ever being tried again.
            crate::spill::Laid::Working => return Opening::Waiting,
            crate::spill::Laid::Nowhere(why) => {
                self.fail(id, why);
                return Opening::Refused;
            }
        };
        match build(&path, video, mtm) {
            Ok(reel) => {
                self.reels.insert(id.to_string(), reel);
                Opening::Ready
            }
            Err(why) => {
                self.fail(id, why);
                Opening::Refused
            }
        }
    }

    /// Remember a failure against the card, so the next frame does not try the
    /// whole thing again.
    ///
    /// Unlike the Linux backend there is no empty object to stand in the
    /// struct's shape, so a broken reel is kept in its own map. The two behave
    /// the same from outside: reported once by `poll`, and gone after.
    fn fail(&mut self, id: &str, why: String) {
        self.broken.insert(id.to_string(), why);
    }

    /// Start, or carry on.
    ///
    /// Called every frame for every playing card rather than at the press —
    /// see `BoardView::pump_media`. `play` on a player that is already playing
    /// is a no-op, which is what makes "the decoder follows the playhead"
    /// affordable as a rule.
    pub fn play(&mut self, id: &str) {
        let Some(reel) = self.reels.get_mut(id).filter(|reel| reel.broken.is_none()) else {
            return;
        };
        reel.rested = None;
        reel.wanted = true;
        unsafe { reel.player.play() };
    }

    pub fn pause(&mut self, id: &str) {
        let Some(reel) = self.reels.get_mut(id).filter(|reel| reel.broken.is_none()) else {
            return;
        };
        // Stamped once and not on every frame it stays paused, so the stamp
        // says when it stopped rather than when it was last asked.
        reel.rested.get_or_insert_with(Instant::now);
        reel.wanted = false;
        unsafe { reel.player.pause() };
    }

    /// Move the playhead. `at` is from the start.
    pub fn seek(&mut self, id: &str, at: Duration) {
        let Some(reel) = self.reels.get(id).filter(|reel| reel.broken.is_none()) else { return };
        unsafe { reel.player.seekToTime(from_duration(at)) };
    }

    /// How loud, `0.0..=1.0`, and whether it is silenced. Both are the
    /// player's own properties.
    pub fn set_loudness(&mut self, id: &str, level: f32, muted: bool) {
        let Some(reel) = self.reels.get(id).filter(|reel| reel.broken.is_none()) else { return };
        unsafe {
            reel.player.setVolume(level.clamp(0.0, 1.0));
            reel.player.setMuted(muted);
        }
    }

    /// One frame of the clock for one card: ask what went wrong, take the
    /// newest picture, and read where the playhead really is.
    ///
    /// `looping` is taken rather than remembered because it is board state and
    /// can change under a playing card. A loop is a seek back to nought rather
    /// than a new player, so there is no gap between the end and the beginning.
    pub fn poll(&mut self, id: &str, looping: bool) -> Option<Beat> {
        // A card that failed to open at all, which on this backend never got
        // as far as being a reel. Said once, then forgotten.
        if let Some(why) = self.broken.remove(id) {
            return Some(Beat { trouble: Some(why), ..Beat::default() });
        }

        // The picture this frame replaces, if it replaces one. Held here rather
        // than pushed straight onto `dropped`, because the reel is borrowed out
        // of the same struct for the whole of the body below. See
        // [`Stack::sweep`] for what it is owed.
        let mut retire = None;
        let reel = self.reels.get_mut(id)?;
        if let Some(why) = &reel.broken {
            // Once. A card that says "no decoder for this" every frame would
            // hold the status bar for as long as it is on screen.
            if reel.told {
                return None;
            }
            reel.told = true;
            return Some(Beat { trouble: Some(why.clone()), ..Beat::default() });
        }

        let mut beat = Beat::default();

        // The failures first, because what they have to say outranks whatever
        // position the player would otherwise report. The item's error is the
        // useful one — it names the codec or the file — and the player's is
        // the fallback for a failure that happened before the item loaded.
        let trouble = unsafe {
            match (reel.item.status(), reel.player.status()) {
                (AVPlayerItemStatus::Failed, _) => Some(said(reel.item.error())),
                (_, AVPlayerStatus::Failed) => Some(said(reel.player.error())),
                _ => None,
            }
        };
        if let Some(why) = trouble {
            reel.broken = Some(why.clone());
            reel.told = true;
            unsafe { reel.player.pause() };
            return Some(Beat { trouble: Some(why), ..beat });
        }

        // Not known when a file opens, and for some containers not known until
        // the demuxer has read further in — so it is asked for until it
        // answers, and then never again.
        if reel.length.is_none() {
            reel.length = unsafe { to_duration(reel.item.duration()) };
        }
        beat.length = reel.length;
        // One reading of the player's clock, shared by the playhead and by the
        // frame — two readings would be a picture and a scrubber a millisecond
        // apart, which is the kind of thing that shows up as a shimmer nobody
        // can account for.
        let now = unsafe { reel.player.currentTime() };
        beat.at = to_duration(now).unwrap_or_default();

        // Before the frame is taken, so that the swap happens on a frame that
        // was going to ask the output a question anyway. Once — see
        // [`LONGEST_SIDE`] on why this cannot be settled when the reel is
        // built, and `resize_output` on why it cannot stretch anything.
        if !reel.sized {
            reel.sized = resize_output(reel);
        }

        if let Some(output) = &reel.output {
            // The playhead's own time rather than a host clock: this runs once
            // per drawn frame, and what the board wants is the newest picture
            // at or before now.
            if unsafe { output.hasNewPixelBufferForItemTime(now) } {
                let copied = unsafe {
                    output.copyPixelBufferForItemTime_itemTimeForDisplay(now, std::ptr::null_mut())
                };
                // A null buffer is AVFoundation saying "nothing should be
                // displayed at this time", which is an answer rather than a
                // failure — the card keeps the frame it has.
                if let Some(picture) = copied.and_then(|buffer| frame_of(&buffer)) {
                    retire = reel.picture.replace(picture);
                    beat.fresh = true;
                }
            }
        }

        // The end has no flag of its own — see `Reel::wanted`.
        let stopped =
            unsafe { reel.player.timeControlStatus() } == AVPlayerTimeControlStatus::Paused;
        if reel.wanted && stopped && beat.at > Duration::ZERO {
            match looping {
                // Back to the beginning without tearing the player down, so a
                // loop has no gap in it.
                true => {
                    unsafe {
                        reel.player.seekToTime(from_duration(Duration::ZERO));
                        reel.player.play();
                    }
                    beat.at = Duration::ZERO;
                }
                // Held at the end rather than reset. A clip that snapped back
                // to its first frame the instant it finished would be one you
                // could never see the end of.
                false => {
                    reel.wanted = false;
                    beat.ended = true;
                }
            }
        }

        self.dropped.extend(retire);
        Some(beat)
    }

    /// The newest picture for a card, for the painter.
    pub fn picture(&self, id: &str) -> Option<Arc<RenderImage>> {
        self.reels.get(id)?.picture.clone()
    }

    /// Which cards have something standing, so the frame loop knows what to
    /// poll without walking the board. Includes the ones that failed to open,
    /// which is how their one message gets out.
    pub fn open_reels(&self) -> Vec<String> {
        self.reels.keys().chain(self.broken.keys()).cloned().collect()
    }

    /// Tear one down — the card was deleted, stopped, or trimmed.
    pub fn forget(&mut self, id: &str) {
        self.broken.remove(id);
        if let Some(reel) = self.reels.remove(id) {
            // Stopped before dropping. A player released while playing takes
            // its audio unit with it whenever the runtime notices; pausing
            // first is what makes the sound end when the card does.
            unsafe { reel.player.pause() };
            self.dropped.extend(reel.picture);
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
        for image in self.dropped.drain(..) {
            // Best effort, for the reason `images.rs` gives: a tile that was
            // never uploaded has nothing to drop, and a window on its way out
            // will not take instructions.
            let _ = window.drop_image(image);
        }
    }

    /// Everything, for a board being closed.
    pub fn forget_all(&mut self) {
        for id in self.open_reels() {
            self.forget(&id);
        }
    }

    /// Drop the reels that have been standing still longest, down to `keep`.
    /// See `pipeline::Stack::trim`, which holds the argument.
    pub fn trim(&mut self, keep: usize) {
        let mut resting: Vec<(String, Instant)> = self
            .reels
            .iter()
            .filter_map(|(id, reel)| reel.rested.map(|at| (id.clone(), at)))
            .collect();
        if resting.len() <= keep {
            return;
        }
        resting.sort_by_key(|(_, at)| *at);
        let over = resting.len() - keep;
        for (id, _) in resting.into_iter().take(over) {
            self.forget(&id);
        }
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        self.forget_all();
    }
}

/// Build a player for one file.
fn build(path: &Path, video: bool, mtm: MainThreadMarker) -> Result<Reel, String> {
    let text = path.to_str().ok_or("that file is somewhere this cannot name")?;
    let url = NSURL::fileURLWithPath(&NSString::from_str(text));
    let item = unsafe { AVPlayerItem::playerItemWithURL(&url, mtm) };
    let player = unsafe { AVPlayer::playerWithPlayerItem(Some(&item), mtm) };
    // Stop at the end rather than advancing to nothing. Looping is done here
    // rather than left to the player, because whether a card loops is board
    // state that can change while it plays — see `poll`.
    unsafe { player.setActionAtItemEnd(AVPlayerActionAtItemEnd::Pause) };

    let output = video.then(|| {
        // Unsized, because nothing is known about the file yet — see
        // [`LONGEST_SIDE`]. `poll` swaps it for a sized one the first frame
        // the item can say how big it is.
        let output = video_output(None);
        unsafe { item.addOutput(&output) };
        output
    });

    Ok(Reel {
        player,
        item,
        output,
        picture: None,
        length: None,
        broken: None,
        told: false,
        // Built paused, so `rested` starts stamped: a player opened and then
        // never played is trimmable like any other.
        rested: Some(Instant::now()),
        // Nothing to size on a card that is only sound.
        sized: !video,
        wanted: false,
    })
}

/// A Core Video attribute key as the Foundation string it already is.
///
/// The keys are `CFString`s and the dictionary wants `NSString`s. The two are
/// the same object — Core Foundation and Foundation strings are toll-free
/// bridged, which is a guarantee of the platform rather than a coincidence —
/// so this is a cast and not a conversion. Taken as a pointer so this file does
/// not have to name `CFString`, which would be a whole dependency for one
/// signature.
fn bridged(key: *const std::ffi::c_void) -> &'static NSString {
    // SAFETY: toll-free bridging, and every caller passes one of Core Video's
    // own `&'static CFString` constants — so the pointer is non-null, aligned
    // and lives for the program.
    unsafe { &*(key.cast::<NSString>()) }
}

/// One video output, asking for BGRA and — once anybody knows what to ask for —
/// for a particular size.
///
/// BGRA because that is the order `RenderImage` reads an `RgbaImage`'s bytes
/// in, so the copy out of the pixel buffer is a copy and nothing else. The
/// alternative is what the decoder prefers on this platform, which is a
/// biplanar YUV that would need converting by hand.
fn video_output(fit: Option<(u32, u32)>) -> Retained<AVPlayerItemVideoOutput> {
    let format = NSNumber::numberWithUnsignedInt(kCVPixelFormatType_32BGRA);
    let mut keys: Vec<&NSString> = vec![bridged(
        unsafe { kCVPixelBufferPixelFormatTypeKey } as *const _ as *const std::ffi::c_void
    )];
    let mut values: Vec<&objc2::runtime::AnyObject> = vec![format.as_ref()];

    // Bound outside the `if`, so they outlive the borrows in `values`.
    let width;
    let height;
    if let Some((w, h)) = fit {
        width = NSNumber::numberWithUnsignedInt(w);
        height = NSNumber::numberWithUnsignedInt(h);
        keys.push(
            bridged(unsafe { kCVPixelBufferWidthKey } as *const _ as *const std::ffi::c_void),
        );
        keys.push(bridged(
            unsafe { kCVPixelBufferHeightKey } as *const _ as *const std::ffi::c_void
        ));
        values.push(width.as_ref());
        values.push(height.as_ref());
    }

    let attributes = NSDictionary::from_slices(&keys, &values);
    unsafe {
        AVPlayerItemVideoOutput::initWithPixelBufferAttributes(
            AVPlayerItemVideoOutput::alloc(),
            Some(&attributes),
        )
    }
}

/// Ask the decoder for frames the size they will actually be drawn, once the
/// item knows how big they are.
///
/// Returns whether the question has been settled — either because the output
/// was swapped, or because there was nothing worth swapping it for. A `false`
/// means the item still has not loaded and it is worth asking again next
/// frame.
///
/// **The size comes from the item rather than from us**, which is what makes
/// this safe to do at all: `LONGEST_SIDE` is applied to the longer edge and
/// the other edge follows the source's own aspect ratio, so a decoder that
/// honours the request scales the picture and a decoder that scales to exactly
/// the rectangle it was handed produces the same picture either way. There is
/// no arrangement of these two numbers that stretches anything.
fn resize_output(reel: &mut Reel) -> bool {
    let Some(output) = reel.output.as_ref() else { return true };
    let size = unsafe { reel.item.presentationSize() };
    let (w, h) = (size.width, size.height);
    // `CGSizeZero` until the underlying resource has loaded, and for some
    // containers for a few frames after playback starts. Not an answer.
    if !(w.is_finite() && h.is_finite()) || w < 1.0 || h < 1.0 {
        return false;
    }
    let Some(fit) = fit_within(w, h) else {
        // Already smaller than anything we would ask for. Settled, and the
        // output it was built with is the right one.
        return true;
    };

    let sized = video_output(Some(fit));
    unsafe {
        reel.item.removeOutput(output);
        reel.item.addOutput(&sized);
    }
    reel.output = Some(sized);
    true
}

/// The rectangle to ask the decoder for, or `None` for a clip already small
/// enough that asking would only cost a swap.
///
/// Even on both axes, because a chroma-subsampled source scaled to an odd width
/// is a scaler being asked for something it is going to round anyway.
fn fit_within(w: f64, h: f64) -> Option<(u32, u32)> {
    let longest = w.max(h);
    if longest <= LONGEST_SIDE as f64 {
        return None;
    }
    let scale = LONGEST_SIDE as f64 / longest;
    let even = |side: f64| ((side * scale).round() as u32).max(2) & !1;
    Some((even(w), even(h)))
}

/// One decoded frame, as a picture the canvas can draw.
///
/// The copy is row by row rather than one `to_vec`, because a decoder is
/// entitled to pad every row out to an alignment it likes and very often does:
/// a 1918-pixel-wide frame commonly arrives with a stride of 1920 times four,
/// and reading it as though it were tight produces a picture that shears
/// diagonally.
fn frame_of(buffer: &CVPixelBuffer) -> Option<Arc<RenderImage>> {
    // Read-only, which is what lets the decoder keep the buffer in whatever
    // memory it prefers instead of copying it somewhere we could write.
    let locked = unsafe { CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly) };
    if locked != 0 {
        return None;
    }
    let picture = read_locked(buffer);
    unsafe { CVPixelBufferUnlockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly) };
    picture
}

/// The inside of [`frame_of`], split out so the unlock above cannot be skipped
/// by an early return — there are five of them in here.
fn read_locked(buffer: &CVPixelBuffer) -> Option<Arc<RenderImage>> {
    let width = CVPixelBufferGetWidth(buffer);
    let height = CVPixelBufferGetHeight(buffer);
    let stride = CVPixelBufferGetBytesPerRow(buffer);
    let base = CVPixelBufferGetBaseAddress(buffer);
    if width == 0 || height == 0 || base.is_null() {
        return None;
    }
    let row = width.checked_mul(4)?;
    if stride < row {
        return None;
    }

    // SAFETY: the buffer is locked for reading for the whole of this function,
    // and Core Video guarantees `stride * height` readable bytes from the base
    // address of a packed format like 32BGRA.
    let data =
        unsafe { std::slice::from_raw_parts(base as *const u8, stride.checked_mul(height)?) };

    let (step, out_w, out_h) = shrink_to(width, height);
    let mut out = Vec::with_capacity(out_w.checked_mul(out_h)?.checked_mul(4)?);
    for y in 0..out_h {
        let from = y * step * stride;
        let line = data.get(from..from + row)?;
        match step {
            1 => out.extend_from_slice(line),
            // Nearest neighbour, and deliberately: this only runs on a frame
            // already far larger than the card it is drawn on, the reduction is
            // a whole number, and a proper filter here would be a resample per
            // frame per card on the thread that draws.
            _ => {
                for x in 0..out_w {
                    let at = x * step * 4;
                    out.extend_from_slice(line.get(at..at + 4)?);
                }
            }
        }
    }

    // Already BGRA — see the module note. `RenderImage` reads an `RgbaImage`'s
    // bytes in that order, so this is a rename rather than a conversion.
    let image = RgbaImage::from_raw(out_w as u32, out_h as u32, out)?;
    Some(Arc::new(RenderImage::new(vec![Frame::new(image)])))
}

/// How many pixels to step by to bring a frame under [`LONGEST_SIDE`], and the
/// size that leaves.
///
/// A whole number, so the copy stays a copy: taking every second or every third
/// pixel is a subscript, where an arbitrary scale factor is arithmetic per
/// pixel. The result is never bigger than the ceiling and never smaller than
/// half of it, which is close enough for a card on a board.
fn shrink_to(width: usize, height: usize) -> (usize, usize, usize) {
    let mut step = 1;
    while width / step > LONGEST_SIDE || height / step > LONGEST_SIDE {
        step += 1;
    }
    (step, (width / step).max(1), (height / step).max(1))
}

/// A `CMTime` as a `Duration`, or `None` where it is not a real length.
///
/// Not every `CMTime` is a number. A live stream's duration is *indefinite*, a
/// file still loading reports invalid, and both arrive here as ordinary structs
/// with a flag set — so the flags are checked before the arithmetic, and a
/// negative time is refused because a `Duration` cannot hold one.
fn to_duration(time: CMTime) -> Option<Duration> {
    let (value, timescale, flags) = (time.value, time.timescale, time.flags);
    if !flags.contains(CMTimeFlags::Valid) || flags.intersects(CMTimeFlags::ImpliedValueFlagsMask) {
        return None;
    }
    if timescale <= 0 || value < 0 {
        return None;
    }
    Some(Duration::from_secs_f64(value as f64 / timescale as f64))
}

/// A `Duration` as a `CMTime`.
///
/// Nanoseconds, because that is what a `Duration` is: a timescale of a
/// thousand million fits in the `i32` the field allows, and it means no seek
/// ever lands somewhere the caller did not ask for through rounding.
fn from_duration(at: Duration) -> CMTime {
    CMTime {
        value: at.as_nanos().min(i64::MAX as u128) as i64,
        timescale: 1_000_000_000,
        flags: CMTimeFlags::Valid,
        epoch: 0,
    }
}

/// What an `NSError` says, in words a person can read.
fn said(error: Option<Retained<objc2_foundation::NSError>>) -> String {
    match error {
        Some(error) => error.localizedDescription().to_string(),
        None => "this file could not be played".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one piece of arithmetic in this file, and the one thing in it that
    /// can be tested without a Mac in the room.
    #[test]
    fn a_frame_inside_the_ceiling_is_copied_whole_and_a_huge_one_is_stepped_over() {
        assert_eq!(shrink_to(1920, 1080), (2, 960, 540));
        assert_eq!(shrink_to(1024, 768), (1, 1024, 768));
        assert_eq!(shrink_to(3840, 2160), (4, 960, 540));
        // Portrait counts too — it is the longest edge, not the width.
        assert_eq!(shrink_to(720, 2400), (3, 240, 800));
    }

    /// A one-pixel-tall frame must not step its way down to nothing.
    #[test]
    fn nothing_ever_shrinks_to_a_frame_with_no_pixels_in_it() {
        let (_, w, h) = shrink_to(4000, 1);
        assert!(w >= 1 && h >= 1);
    }

    #[test]
    fn a_time_that_is_not_a_number_is_no_length_rather_than_a_wrong_one() {
        let good = CMTime { value: 90, timescale: 30, flags: CMTimeFlags::Valid, epoch: 0 };
        assert_eq!(to_duration(good), Some(Duration::from_secs(3)));

        let unset = CMTime { value: 90, timescale: 30, flags: CMTimeFlags::empty(), epoch: 0 };
        assert_eq!(to_duration(unset), None, "an invalid time is not zero");

        let live = CMTime {
            value: 0,
            timescale: 1,
            flags: CMTimeFlags::Valid | CMTimeFlags::Indefinite,
            epoch: 0,
        };
        assert_eq!(to_duration(live), None, "a stream has no length to scrub across");

        let broken = CMTime { value: 1, timescale: 0, flags: CMTimeFlags::Valid, epoch: 0 };
        assert_eq!(to_duration(broken), None, "and nothing divides by zero");
    }

    #[test]
    fn a_seek_survives_the_trip_out_and_back() {
        let at = Duration::from_millis(1234);
        assert_eq!(to_duration(from_duration(at)), Some(at));
    }
}
