//! Sound and pictures in a browser, out of the page's own media elements.
//!
//! The fourth real backend behind `mod pipeline`, and the only one whose
//! decoder is not a library this crate links against. A browser already has
//! every codec the machine has, wired to the speakers and to a compositor, and
//! it hands them out through one element: `<video>`. So a card that plays is a
//! `<video>` or an `<audio>` that nothing ever adds to the page — created,
//! pointed at the card's own bytes, and read from once a frame.
//!
//! ## The frames come back the long way round
//!
//! This is the one thing here that is not free, and it is worth writing down.
//!
//! The three desktop backends pull decoded frames straight out of the decoder:
//! GStreamer's `appsink`, an `AVPlayerItemVideoOutput`, a Media Foundation
//! frame server. A browser will not do that. There is no API that says "give me
//! the picture you just decoded" — the picture belongs to the element, and the
//! only supported way to a copy of it is to draw the element into a canvas and
//! read the canvas back:
//!
//! ```text
//! <video> ──drawImage──▶ <canvas> ──getImageData──▶ bytes ──▶ RenderImage
//! ```
//!
//! That is a copy per frame, so three things keep it affordable:
//!
//! 1. **Only when the picture changed.** The element's own `currentTime` says
//!    whether it has moved since the last read — see [`Reel::drawn`]. A paused
//!    card, or a board redrawing for some other reason, costs nothing.
//! 2. **`willReadFrequently`.** The canvas is asked for with that flag set,
//!    which is the browser being told to keep this surface where the CPU can
//!    reach it. Without it every `getImageData` is a read back off the GPU and
//!    a stall in the middle of our own frame.
//! 3. **[`LONGEST_SIDE`].** The canvas is the card's size rather than the
//!    file's, so a 4K clip is scaled down by the browser — in the `drawImage`
//!    it was going to do anyway — instead of being copied at full size and
//!    thrown away.
//!
//! ## Blobs, not files
//!
//! The desktop backends lay a played file out on disk and hand over a path,
//! because that is what a decoder wants — see [`crate::spill`]. There is no
//! disk here and no path. The bytes become a `Blob` and the blob becomes an
//! object URL, which is the browser's own name for "a file that is already in
//! memory". Revoked in [`Stack::forget`], which is the only place a reel is
//! ever torn down, so nothing is left holding the bytes after the card is.
//!
//! ## What a browser will not play
//!
//! Every desktop this app runs on plays Matroska, and no browser does. The
//! element is asked before anything else happens — `canPlayType`, which is the
//! browser answering for itself rather than us keeping a list — and a file it
//! will not take is refused at [`Stack::open`] with the extension in the
//! sentence. The card then says so once, exactly as it would for a codec
//! missing from a desktop.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use gpui::{RenderImage, Window};
use image::{Frame, RgbaImage};
use wasm_bindgen::{Clamped, JsCast, JsValue};
use web_sys::{
    Blob, BlobPropertyBag, CanvasRenderingContext2d, HtmlCanvasElement, HtmlMediaElement,
    HtmlVideoElement, Url,
};
use web_time::Instant;

/// The longest edge a frame is read back at.
///
/// The same ceiling the desktop keeps — see `pipeline::LONGEST_SIDE` — and here
/// it does twice the work, because the canvas this sizes is also the thing
/// being copied. A 4K frame read back whole would be thirty-three megabytes
/// through `getImageData` for a card drawn three hundred units across.
const LONGEST_SIDE: u32 = 1024;

/// `HAVE_CURRENT_DATA`: there is a picture at the playhead to draw.
///
/// Named rather than written as `2` at the one place it is compared, because
/// the number is a constant of the platform and nothing in this file explains
/// it.
const HAVE_CURRENT_DATA: u16 = 2;

/// What asking for a card's decoder found. The desktop's twin — see
/// [`crate::pipeline`], where the three answers are argued.
///
/// `Waiting` never comes back from this backend. It means "the file is still
/// being laid out on disk", and there is no disk: the bytes are already in
/// memory and a blob is made out of them in the same call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opening {
    Ready,
    Waiting,
    Refused,
}

/// What one frame of the clock says about a card that is playing. The
/// desktop's twin, field for field — see [`crate::pipeline::Beat`].
#[derive(Debug, Clone, Default)]
pub struct Beat {
    pub at: Duration,
    pub length: Option<Duration>,
    pub ended: bool,
    pub fresh: bool,
    pub trouble: Option<String>,
}

/// Where a card's frames are read back.
///
/// Built on the first frame rather than at [`Stack::open`], because the size to
/// build it at is the video's own and that is not known until the browser has
/// read the file's header.
struct Painter {
    canvas: HtmlCanvasElement,
    ink: CanvasRenderingContext2d,
    width: u32,
    height: u32,
}

/// One card's element, and everything read off it.
struct Reel {
    /// `None` on a reel that failed, which is a card's refusal remembered
    /// rather than a card with a player behind it. The desktop's twin holds a
    /// `fakesink` in the same slot and for the same reason.
    play: Option<HtmlMediaElement>,
    /// The object URL, so it can be revoked. Revoking it is what lets the
    /// browser drop the bytes.
    url: Option<String>,
    /// `None` on a card that is only sound.
    painter: Option<Painter>,
    /// The playhead the newest picture was read at, so a frame is copied once
    /// rather than once a redraw.
    drawn: Option<f64>,
    picture: Option<Arc<RenderImage>>,
    length: Option<Duration>,
    broken: Option<String>,
    told: bool,
    rested: Option<Instant>,
}

/// Every reel there is.
#[derive(Default)]
pub struct Stack {
    reels: HashMap<String, Reel>,
    /// Pictures nothing will draw again, waiting for a window to hand their
    /// atlas tiles back. See [`Stack::sweep`].
    dropped: Vec<Arc<RenderImage>>,
}

impl Stack {
    pub fn new() -> Self {
        Self::default()
    }

    /// Make sure there is a player for this card, and say how far off one is.
    ///
    /// Idempotent, and cheap on every frame after the first. `bytes` is copied
    /// exactly once, on the frame the blob is made: the copy is unavoidable —
    /// the bytes are in this module's memory and the decoder is on the other
    /// side of the browser — and it is why a reel is built once and kept.
    pub fn open(&mut self, id: &str, _hash: &str, ext: &str, bytes: &[u8], video: bool) -> Opening {
        if let Some(reel) = self.reels.get(id) {
            return match reel.broken.is_none() {
                true => Opening::Ready,
                false => Opening::Refused,
            };
        }
        match build(ext, bytes, video) {
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
    fn fail(&mut self, id: &str, why: String) {
        self.reels.entry(id.to_string()).or_insert_with(|| Reel {
            play: None,
            url: None,
            painter: None,
            drawn: None,
            picture: None,
            length: None,
            broken: Some(why),
            told: false,
            // Never trimmed for age, for the reason the desktop gives: a broken
            // reel is what stops the card trying the same file again on the
            // very next frame.
            rested: None,
        });
    }

    /// The element behind a card that has one and is not broken.
    fn live(&self, id: &str) -> Option<&HtmlMediaElement> {
        self.reels.get(id).filter(|reel| reel.broken.is_none())?.play.as_ref()
    }

    /// Start, or carry on.
    ///
    /// Called every frame for every playing card rather than at the press —
    /// see `BoardView::pump_media` — so the guard matters here in a way it does
    /// not on the desktop: `play()` on an element that is already playing is a
    /// second promise and a second chance to be refused, sixty times a second.
    pub fn play(&mut self, id: &str) {
        let Some(reel) = self.reels.get_mut(id).filter(|reel| reel.broken.is_none()) else {
            return;
        };
        reel.rested = None;
        let Some(element) = reel.play.as_ref() else { return };
        if !element.paused() {
            return;
        }
        // The promise is dropped rather than awaited. A browser refuses to
        // start sound for exactly one reason we can do nothing about from here
        // — the page has not been touched yet — and every press that reaches
        // this line *is* the page being touched. Anything else it could go
        // wrong with arrives at `poll` as the element's own error, which is
        // where a card is told.
        let _ = element.play();
    }

    pub fn pause(&mut self, id: &str) {
        let Some(reel) = self.reels.get_mut(id).filter(|reel| reel.broken.is_none()) else {
            return;
        };
        if reel.rested.is_none() {
            reel.rested = Some(Instant::now());
        }
        if let Some(element) = reel.play.as_ref() {
            element.pause().ok();
        }
    }

    pub fn seek(&mut self, id: &str, at: Duration) {
        let Some(element) = self.live(id) else { return };
        element.set_current_time(at.as_secs_f64());
    }

    pub fn set_loudness(&mut self, id: &str, level: f32, muted: bool) {
        let Some(element) = self.live(id) else { return };
        element.set_volume(level.clamp(0.0, 1.0) as f64);
        element.set_muted(muted);
    }

    /// One frame of the clock for one card: read where the playhead is, take a
    /// picture if there is a new one, and notice an element that has stopped.
    ///
    /// `looping` is handed to the element rather than acted on here, which is
    /// the one place this backend is *simpler* than the desktop: a browser
    /// loops a media element itself, with no gap and no seek, so the board's
    /// loop flag is one property set on the way past.
    pub fn poll(&mut self, id: &str, looping: bool) -> Option<Beat> {
        let mut retire = None;
        let beat = {
            let reel = self.reels.get_mut(id)?;
            if let Some(why) = &reel.broken {
                // Once. A card that said this every frame would hold the status
                // bar for as long as it is on screen.
                if reel.told {
                    return None;
                }
                reel.told = true;
                return Some(Beat { trouble: Some(why.clone()), ..Beat::default() });
            }

            let Some(element) = reel.play.clone() else { return None };

            // The element's own complaint outranks everything below it: a file
            // that stopped decoding half way through has a `currentTime` that
            // is merely where it gave up.
            if let Some(problem) = element.error() {
                let why = trouble(&problem);
                reel.broken = Some(why.clone());
                reel.told = true;
                return Some(Beat { trouble: Some(why), ..Beat::default() });
            }

            element.set_loop(looping);

            let mut beat = Beat {
                at: Duration::from_secs_f64(element.current_time().max(0.0)),
                ended: element.ended(),
                ..Beat::default()
            };

            // NaN until the browser has read the header, and infinite on a
            // stream that has no end. Neither is a length anybody can draw a
            // scrubber against.
            let length = element.duration();
            if length.is_finite() && length > 0.0 {
                reel.length = Some(Duration::from_secs_f64(length));
            }
            beat.length = reel.length;

            if reel.painter.is_some() || element.ready_state() >= HAVE_CURRENT_DATA {
                if let Some(picture) = reel.repaint(&element) {
                    retire = reel.picture.replace(picture);
                    beat.fresh = true;
                }
            }

            beat
        };
        // The picture this frame replaced is not going to be drawn again, and
        // its atlas tile is two megabytes. See [`Stack::sweep`].
        self.dropped.extend(retire);
        Some(beat)
    }

    /// The newest picture for a card, for the painter.
    pub fn picture(&self, id: &str) -> Option<Arc<RenderImage>> {
        self.reels.get(id)?.picture.clone()
    }

    /// Which cards have a player standing, so the frame loop knows what to poll
    /// without walking the board.
    pub fn open_reels(&self) -> Vec<String> {
        self.reels.keys().cloned().collect()
    }

    /// Release the atlas tiles of every picture retired since the last call.
    ///
    /// **A video is a new picture thirty times a second, and every one of them
    /// takes a tile in the sprite atlas that nothing else ever gives back.**
    /// The atlas is a cache keyed by image id with no eviction in it — see
    /// `Window::drop_image`, which is the only door out — so a clip left
    /// playing would otherwise grow the texture it draws from until the GPU
    /// refused another.
    ///
    /// Call once a frame from somewhere with a window, beside
    /// [`Images::sweep`](crate::images::Images::sweep) and
    /// [`Live::sweep`](crate::live::Live::sweep), which exist for the same
    /// reason and are swept in the same line of `render`.
    pub fn sweep(&mut self, window: &mut Window) {
        for image in self.dropped.drain(..) {
            // Best effort, for the reason `images.rs` gives: a tile that was
            // never uploaded has nothing to drop, and a window on its way out
            // will not take instructions.
            let _ = window.drop_image(image);
        }
    }

    /// Tear one down — the card was deleted, stopped, or pushed out by
    /// `playback::AT_ONCE`.
    ///
    /// The element is stopped and emptied before it is dropped. A `<video>`
    /// with a source still on it is a decoder the browser is entitled to keep
    /// running for as long as something holds the object, and clearing `src`
    /// is what tells it the file is finished with. The URL is revoked in the
    /// same breath, which is what lets go of the bytes themselves.
    pub fn forget(&mut self, id: &str) {
        let Some(reel) = self.reels.remove(id) else { return };
        if let Some(element) = reel.play {
            element.pause().ok();
            element.remove_attribute("src").ok();
            element.load();
        }
        if let Some(url) = reel.url {
            Url::revoke_object_url(&url).ok();
        }
        self.dropped.extend(reel.picture);
    }

    /// Drop the reels that have been standing still longest, down to `keep` of
    /// them.
    ///
    /// The desktop's reasoning holds here twice over: a paused element is a
    /// decoder, a canvas and a copy of the file, and a browser tab has less
    /// room for thirty of those than a desktop does.
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

    /// Everything, for a board being closed.
    pub fn forget_all(&mut self) {
        for id in self.open_reels() {
            self.forget(&id);
        }
    }
}

impl Drop for Stack {
    fn drop(&mut self) {
        self.forget_all();
    }
}

impl Reel {
    /// Read the element's picture back, if it has moved since the last read.
    ///
    /// `None` when there is nothing new, which is most calls: the board redraws
    /// for its own reasons — a pointer moving, a spring settling — far more
    /// often than a thirty-a-second video hands over a frame.
    fn repaint(&mut self, element: &HtmlMediaElement) -> Option<Arc<RenderImage>> {
        let video: &HtmlVideoElement = element.dyn_ref()?;
        let at = element.current_time();
        if self.drawn == Some(at) {
            return None;
        }

        if self.painter.is_none() {
            self.painter = Some(Painter::of(video)?);
        }
        let picture = self.painter.as_ref()?.grab(video)?;
        self.drawn = Some(at);
        Some(picture)
    }
}

impl Painter {
    /// A canvas the shape of the video, held to [`LONGEST_SIDE`].
    ///
    /// `None` until the browser has read the header, which is where the shape
    /// comes from: an element that has not got that far reports nought by
    /// nought, and a canvas of that size is one every later frame would be
    /// drawn into.
    fn of(video: &HtmlVideoElement) -> Option<Self> {
        let (w, h) = (video.video_width(), video.video_height());
        if w == 0 || h == 0 {
            return None;
        }
        let longest = w.max(h);
        let (width, height) = match longest > LONGEST_SIDE {
            false => (w, h),
            true => {
                let scale = LONGEST_SIDE as f64 / longest as f64;
                (((w as f64 * scale) as u32).max(1), ((h as f64 * scale) as u32).max(1))
            }
        };

        let canvas: HtmlCanvasElement =
            web_sys::window()?.document()?.create_element("canvas").ok()?.dyn_into().ok()?;
        canvas.set_width(width);
        canvas.set_height(height);

        // `willReadFrequently`, which is the whole reason this asks for the
        // context the long way. It tells the browser this surface is going to
        // be read back rather than composited, so it keeps it in ordinary
        // memory; without it every `getImageData` waits on the GPU in the
        // middle of our own frame. See the module note.
        let options = js_sys::Object::new();
        js_sys::Reflect::set(
            &options,
            &JsValue::from_str("willReadFrequently"),
            &JsValue::from_bool(true),
        )
        .ok()?;
        let ink: CanvasRenderingContext2d =
            canvas.get_context_with_context_options("2d", &options).ok()??.dyn_into().ok()?;

        Some(Self { canvas, ink, width, height })
    }

    /// One frame, copied out of the element and into something gpui can draw.
    fn grab(&self, video: &HtmlVideoElement) -> Option<Arc<RenderImage>> {
        let (w, h) = (self.width as f64, self.height as f64);
        self.ink.draw_image_with_html_video_element_and_dw_and_dh(video, 0.0, 0.0, w, h).ok()?;
        let data = self.ink.get_image_data(0.0, 0.0, w, h).ok()?;
        let Clamped(bytes) = data.data();

        let mut image = RgbaImage::from_raw(self.width, self.height, bytes)?;
        // The canvas answers in RGBA and `RenderImage` reads its bytes as
        // BGRA, so the two channels are swapped in place — the same pass
        // `images::to_bgra` makes over a still picture, and the one thing the
        // desktop's video path gets for free, because it can ask its decoder
        // for the order it wants.
        crate::images::to_bgra(&mut image);
        Some(Arc::new(RenderImage::new(vec![Frame::new(image)])))
    }
}

impl Drop for Painter {
    fn drop(&mut self) {
        // Emptied rather than merely dropped. The canvas is not in the page, so
        // nothing else refers to it, but its backing store is as large as a
        // frame and this is the one line that says plainly when it goes.
        self.canvas.set_width(0);
        self.canvas.set_height(0);
    }
}

/// Build the element behind one card, or say why there will not be one.
fn build(ext: &str, bytes: &[u8], video: bool) -> Result<Reel, String> {
    let kind = mime(ext);
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or_else(|| "there is no page".to_string())?;
    let tag = match video {
        true => "video",
        false => "audio",
    };
    let element: HtmlMediaElement = document
        .create_element(tag)
        .ok()
        .and_then(|made| made.dyn_into().ok())
        .ok_or_else(|| "this browser has no media element".to_string())?;

    // The browser answering for itself, rather than this file keeping a list of
    // what browsers play. It is asked before the bytes are copied, so a card
    // nothing here can open costs nothing to refuse.
    if element.can_play_type(kind).is_empty() {
        return Err(format!("this browser plays no {ext} — the desktop app does"));
    }

    let array = js_sys::Uint8Array::new_with_length(bytes.len() as u32);
    array.copy_from(bytes);
    let parts = js_sys::Array::of1(&array.buffer());
    let options = BlobPropertyBag::new();
    options.set_type(kind);
    let blob = Blob::new_with_u8_array_sequence_and_options(&parts, &options)
        .map_err(|_| "this file could not be held in memory".to_string())?;
    let url = Url::create_object_url_with_blob(&blob)
        .map_err(|_| "this file could not be opened".to_string())?;

    element.set_src(&url);
    // Ask for the whole thing rather than only its header. The bytes are
    // already in this tab's memory — there is no network behind that URL — so
    // the only thing "auto" costs is the decoder starting sooner.
    element.set_preload("auto");
    // Without this, a phone plays the video full screen over the page instead
    // of in the board, and the board never gets a frame back at all.
    element.set_attribute("playsinline", "").ok();

    Ok(Reel {
        play: Some(element),
        url: Some(url),
        painter: None,
        drawn: None,
        picture: None,
        length: None,
        broken: None,
        told: false,
        rested: Some(Instant::now()),
    })
}

/// What a media element's own failure means, in words a person reads.
///
/// The element's `message` where there is one — browsers put the codec's own
/// complaint there and it names the thing that was wrong — and the code's
/// meaning where there is not, which is the usual case in Safari.
fn trouble(problem: &web_sys::MediaError) -> String {
    let said = problem.message();
    if !said.trim().is_empty() {
        return said;
    }
    match problem.code() {
        1 => "this clip was stopped before it finished".into(),
        2 => "this clip could not be read".into(),
        3 => "this clip could not be decoded".into(),
        _ => "this browser cannot play this clip".into(),
    }
}

/// What to tell the browser a card's bytes are.
///
/// The media type matters twice: `canPlayType` is asked with it, and the blob
/// carries it. Everything not on this list is handed over as the empty string,
/// which `canPlayType` answers "" to — so an unknown extension is refused with
/// the same sentence a known unplayable one gets.
///
/// **QuickTime is deliberately called MP4.** Nearly every `.mov` a camera or a
/// phone makes is H.264 and AAC in a container the same demuxer reads, and the
/// browsers that answer "" to `video/quicktime` play those files perfectly
/// when they are not told the container's real name. A `.mov` that genuinely
/// holds something else fails at `poll` with the decoder's own words, which is
/// the same place every other unplayable file is answered.
fn mime(ext: &str) -> &'static str {
    match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "mp4" | "m4v" | "mov" => "video/mp4",
        "webm" => "video/webm",
        "ogv" => "video/ogg",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "wmv" => "video/x-ms-wmv",
        "flv" => "video/x-flv",
        "mpg" | "mpeg" => "video/mpeg",
        "3gp" => "video/3gpp",
        "mts" => "video/mp2t",

        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/ogg; codecs=opus",
        "m4a" | "alac" => "audio/mp4",
        "aac" => "audio/aac",
        "aiff" | "aif" => "audio/aiff",
        "wma" => "audio/x-ms-wma",
        "ape" => "audio/x-ape",

        _ => "",
    }
}
