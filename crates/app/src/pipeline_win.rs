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
//! scaling happens *inside* the engine rather than in our own loop.
//!
//! ## Polled, not called back
//!
//! The Media Engine requires a callback — `MFCreateMediaEngine` refuses without
//! one — so there is a [`Notify`] here, and it does as little as it is allowed
//! to: it sets one atomic. Everything else is asked from the board's frame
//! loop, on the thread that draws, for the same reason the other two backends
//! do it that way. Nothing on a Media Foundation worker thread ever touches a
//! view it does not own.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::RenderImage;
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
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};

/// The longest edge a frame is transferred at.
///
/// A 4K frame is thirty-three megabytes to be drawn at a twentieth of its size.
/// The engine scales into the rectangle it is given, so asking for a smaller
/// rectangle is asking the decoder's own scaler to do the work — which is the
/// same bargain `videoscale` gives the Linux backend.
const LONGEST_SIDE: u32 = 1024;

/// What one frame of the clock says about a card that is playing.
///
/// The other two backends' twin. Kept in step by the fact that `board_view.rs`
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

/// What the engine's callback thread is allowed to say.
///
/// One atomic and nothing else. The callback runs on a Media Foundation worker
/// thread and the board is not on it, so what crosses is a flag rather than a
/// decision — see the module note.
///
/// Only the failure is here. *Ended* is not, even though there is an event for
/// it, because `IsEnded` answers the same question from the frame loop and is
/// the only one of the two that stays right across a seek and a loop — an event
/// flag would have to be un-set by hand in three places, and the third one is
/// always the one that gets forgotten.
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

/// One card's engine.
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
    /// The newest frame, kept so the painter has something to draw on every
    /// frame rather than only on the ones a new picture arrived in.
    picture: Option<Arc<RenderImage>>,
    length: Option<Duration>,
    /// Set once, then reported once.
    broken: Option<String>,
    told: bool,
    /// Since when this reel has been standing still, for [`Stack::trim`].
    rested: Option<Instant>,
}

/// Every engine there is, and the one-time setup behind them.
pub struct Stack {
    /// `None` until somebody asks for the first time; `Some(false)` on a
    /// machine whose Media Foundation would not start, which is a state rather
    /// than a crash.
    started: Option<bool>,
    spill: Option<crate::spill::Spill>,
    factory: Option<IMFMediaEngineClassFactory>,
    imaging: Option<IWICImagingFactory>,
    reels: HashMap<String, Reel>,
    /// Cards that never became a reel at all, and the reason. Reported once by
    /// `poll` and then dropped, which is the same contract `Reel::told` holds.
    broken: HashMap<String, String>,
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
            factory: None,
            imaging: None,
            reels: HashMap::new(),
            broken: HashMap::new(),
        }
    }

    /// Bring Media Foundation up, once, and answer whether there is anything to
    /// play with.
    ///
    /// Lazy rather than at startup: a board of photographs never touches this,
    /// and `MFStartup` loads a good deal of machinery that a board of
    /// photographs would then be paying for.
    ///
    /// A machine where this fails answers `false` for the rest of the session
    /// and every card says so in words — the soft failure the other two
    /// backends promise, promised here too. Windows N without the Media Feature
    /// Pack is the real case, and it is somebody's actual computer.
    fn start(&mut self) -> bool {
        if let Some(known) = self.started {
            return known;
        }
        self.started = Some(false);

        // Ignored on purpose, and never paired with `CoUninitialize`. gpui has
        // already initialised COM on this thread for its window and its file
        // dialogs, so this returns `S_FALSE` — or `RPC_E_CHANGED_MODE` if it
        // chose the other apartment, which is equally fine: the thread is
        // initialised either way, and *undoing* an initialisation this code did
        // not perform is the one thing that would actually break something.
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };

        if unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.is_err() {
            return false;
        }
        let factory: IMFMediaEngineClassFactory = match unsafe {
            CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)
        } {
            Ok(factory) => factory,
            Err(_) => return false,
        };
        let imaging: IWICImagingFactory =
            match unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
            {
                Ok(imaging) => imaging,
                Err(_) => return false,
            };

        self.factory = Some(factory);
        self.imaging = Some(imaging);
        self.spill = Some(crate::spill::Spill::open());
        self.started = Some(true);
        true
    }

    /// Make sure there is an engine for this card, and answer whether there is
    /// one now.
    ///
    /// Idempotent, and cheap on every frame after the first.
    pub fn open(&mut self, id: &str, hash: &str, ext: &str, bytes: &[u8], video: bool) -> bool {
        if self.reels.contains_key(id) {
            return self.reels[id].broken.is_none();
        }
        if !self.start() {
            self.fail(id, "no media stack on this machine".into());
            return false;
        }
        let Some(path) = self.spill.as_ref().and_then(|spill| spill.lay_out(hash, ext, bytes))
        else {
            self.fail(id, "nowhere to unpack this file".into());
            return false;
        };
        let Some(factory) = self.factory.clone() else {
            self.fail(id, "no media stack on this machine".into());
            return false;
        };
        match build(&factory, &path, video) {
            Ok(reel) => {
                self.reels.insert(id.to_string(), reel);
                true
            }
            Err(why) => {
                self.fail(id, why);
                false
            }
        }
    }

    fn fail(&mut self, id: &str, why: String) {
        self.broken.insert(id.to_string(), why);
    }

    /// Start, or carry on.
    ///
    /// Called every frame for every playing card rather than at the press — see
    /// `BoardView::pump_media`. `Play` on an engine that is already playing is a
    /// no-op, which is what makes "the decoder follows the playhead" affordable
    /// as a rule.
    pub fn play(&mut self, id: &str) {
        let Some(reel) = self.reels.get_mut(id).filter(|reel| reel.broken.is_none()) else {
            return;
        };
        reel.rested = None;
        let _ = unsafe { reel.engine.Play() };
    }

    pub fn pause(&mut self, id: &str) {
        let Some(reel) = self.reels.get_mut(id).filter(|reel| reel.broken.is_none()) else {
            return;
        };
        // Stamped once and not on every frame it stays paused, so the stamp
        // says when it stopped rather than when it was last asked.
        reel.rested.get_or_insert_with(Instant::now);
        let _ = unsafe { reel.engine.Pause() };
    }

    /// Move the playhead. `at` is from the start.
    pub fn seek(&mut self, id: &str, at: Duration) {
        let Some(reel) = self.reels.get(id).filter(|reel| reel.broken.is_none()) else { return };
        let _ = unsafe { reel.engine.SetCurrentTime(at.as_secs_f64()) };
    }

    /// How loud, `0.0..=1.0`, and whether it is silenced. Both are the
    /// engine's own properties.
    pub fn set_loudness(&mut self, id: &str, level: f32, muted: bool) {
        let Some(reel) = self.reels.get(id).filter(|reel| reel.broken.is_none()) else { return };
        unsafe {
            let _ = reel.engine.SetVolume(level.clamp(0.0, 1.0) as f64);
            let _ = reel.engine.SetMuted(muted);
        }
    }

    /// One frame of the clock for one card: ask what went wrong, take the
    /// newest picture, and read where the playhead really is.
    ///
    /// `looping` is taken rather than remembered because it is board state and
    /// can change under a playing card. Here it is handed straight to the
    /// engine, which loops without a gap — the one place this backend gets to
    /// do less work than the other two rather than more.
    pub fn poll(&mut self, id: &str, looping: bool) -> Option<Beat> {
        // Split, so a reel can be written to while the imaging factory beside
        // it is read.
        let Self { imaging, reels, broken, .. } = self;
        if let Some(why) = broken.remove(id) {
            return Some(Beat { trouble: Some(why), ..Beat::default() });
        }

        let reel = reels.get_mut(id)?;
        if let Some(why) = &reel.broken {
            // Once. A card that says "no decoder for this" every frame would
            // hold the status bar for as long as it is on screen.
            if reel.told {
                return None;
            }
            reel.told = true;
            return Some(Beat { trouble: Some(why.clone()), ..Beat::default() });
        }

        // The failure first, because what it has to say outranks whatever
        // position the engine would otherwise report.
        if reel.signals.failed.swap(false, Ordering::Relaxed) {
            let why = said(&reel.engine);
            reel.broken = Some(why.clone());
            reel.told = true;
            let _ = unsafe { reel.engine.Shutdown() };
            return Some(Beat { trouble: Some(why), ..Beat::default() });
        }

        let mut beat = Beat::default();
        let _ = unsafe { reel.engine.SetLoop(looping) };

        // Not known until the engine has read the file's metadata, and `NaN`
        // until then — which is why this is asked for until it answers rather
        // than once. A live stream answers infinity and is left with no length
        // at all, because a scrubber across a stream is a scrubber across
        // nothing.
        if reel.length.is_none() {
            let seconds = unsafe { reel.engine.GetDuration() };
            if seconds.is_finite() && seconds > 0.0 {
                reel.length = Some(Duration::from_secs_f64(seconds));
            }
        }
        beat.length = reel.length;

        let at = unsafe { reel.engine.GetCurrentTime() };
        beat.at = match at.is_finite() && at > 0.0 {
            true => Duration::from_secs_f64(at),
            false => Duration::ZERO,
        };

        if let (true, Some(imaging)) = (reel.video, imaging.as_ref()) {
            beat.fresh = take_frame(reel, imaging);
        }

        // Asked, not remembered — see `Signals`. The engine has already been
        // told whether to loop, so a looping clip never reads as ended and the
        // gapless restart happens inside it rather than here.
        if unsafe { reel.engine.IsEnded() }.as_bool() {
            beat.ended = true;
            // Held at the end rather than reset. A clip that snapped back to
            // its first frame the instant it finished would be one you could
            // never see the end of.
            let _ = unsafe { reel.engine.Pause() };
        }

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
    ///
    /// `Shutdown` first and then dropped. An engine released while playing
    /// takes its audio endpoint with it whenever COM notices; shutting it down
    /// first is what makes the sound end when the card does.
    pub fn forget(&mut self, id: &str) {
        self.broken.remove(id);
        if let Some(reel) = self.reels.remove(id) {
            let _ = unsafe { reel.engine.Shutdown() };
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
        // Only where it was started, and after every engine is down: shutting
        // Media Foundation down under a live object is how a process comes to
        // crash on the way out.
        if self.started == Some(true) {
            let _ = unsafe { MFShutdown() };
        }
    }
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
        // and arrives later, through `Stack::play`.
        let _ = engine.SetAutoPlay(false);
        engine
            .SetSource(&BSTR::from(text))
            .map_err(|_| "nothing on this machine can open that file".to_string())?;
    }

    Ok(Reel {
        engine,
        signals,
        bitmap: None,
        video,
        shown: None,
        picture: None,
        length: None,
        broken: None,
        told: false,
        // Built paused, so `rested` starts stamped: an engine opened and then
        // never played is trimmable like any other.
        rested: Some(Instant::now()),
    })
}

/// Take the current frame, if it is one we have not already taken, and answer
/// whether the card has something new to draw.
fn take_frame(reel: &mut Reel, imaging: &IWICImagingFactory) -> bool {
    // `S_FALSE` means "nothing new" and folds into `Ok` here, so the timestamp
    // is what decides — see `Reel::shown`.
    let Ok(pts) = (unsafe { reel.engine.OnVideoStreamTick() }) else { return false };
    if pts < 0 || Some(pts) == reel.shown {
        return false;
    }

    // The size is not known until the engine has decoded far enough to know it,
    // which is why the bitmap is made here rather than in `build`.
    if reel.bitmap.is_none() {
        let (mut wide, mut high) = (0u32, 0u32);
        let asked = unsafe {
            reel.engine.GetNativeVideoSize(Some(&mut wide as *mut u32), Some(&mut high as *mut u32))
        };
        if asked.is_err() {
            return false;
        }
        let Some((wide, high)) = fit_inside(wide, high) else { return false };
        let Ok(bitmap) = (unsafe {
            imaging.CreateBitmap(wide, high, &GUID_WICPixelFormat32bppBGRA, WICBitmapCacheOnDemand)
        }) else {
            return false;
        };
        reel.bitmap = Some((bitmap, wide, high));
    }
    let Some((bitmap, wide, high)) = &reel.bitmap else { return false };
    let (wide, high) = (*wide, *high);

    // The rectangle is the whole bitmap, and the bitmap keeps the video's own
    // shape — so the engine's letterboxing has nothing to letterbox and the
    // border colour never shows. That is why it is passed as null.
    let into = RECT { left: 0, top: 0, right: wide as i32, bottom: high as i32 };
    if unsafe { reel.engine.TransferVideoFrame(bitmap, None, &into, None) }.is_err() {
        return false;
    }

    let picture = read_bitmap(bitmap, wide, high);
    if picture.is_some() {
        reel.picture = picture;
        reel.shown = Some(pts);
        return true;
    }
    false
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
/// caller tries again on the next frame.
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

    /// The one piece of arithmetic in this file, and the one thing in it that
    /// can be tested without a Windows machine in the room.
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
}
