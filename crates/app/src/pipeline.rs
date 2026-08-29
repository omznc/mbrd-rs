//! Sound and pictures, decoded by the desktop's own media stack.
//!
//! The thing behind the play button. [`crate::playback`] has held where every
//! playhead is since the transport strip was drawn, deliberately with nothing
//! underneath it; this is the underneath. One `playbin3` per card that is
//! actually playing, audio straight out to the desktop, video pulled back as
//! frames the canvas can draw.
//!
//! ## Why GStreamer, and what it costs
//!
//! Decoding video is not a thing an application writes. It is a decade of codec
//! work, hardware paths that differ per machine, and a container zoo — and
//! every desktop this app runs on already has one. The cost is honest and worth
//! writing down: **this is the first real system dependency in the project.**
//! The binary is no longer the whole story on Linux; the packages name
//! GStreamer in `depends`, and a machine without it gets an app whose media
//! cards say so rather than one that fails to start. See [`Stack::start`],
//! which is the only place that can fail for that reason, and fails *softly*.
//!
//! ## Files, not buffers
//!
//! GStreamer wants a URI, and an asset lives in memory. A played file is laid
//! out on disk under the hash of its own contents first — see [`crate::spill`],
//! which is shared with the other two backends and holds the whole argument for
//! why a path beats an `appsrc`.
//!
//! ## Polled, not signalled
//!
//! **There is no GLib main loop here, and no callback that runs on a
//! GStreamer thread.** The board already runs a frame loop for as long as
//! anything is playing — see `playback::Media::tick` — so the bus is drained
//! and the newest frame pulled *from that loop*, on the thread that draws. That
//! removes the whole class of bug where a decoder thread reaches into a view it
//! does not own, and it costs one non-blocking pull per playing card per frame.
//!
//! ## BGRA, because that is what gpui draws
//!
//! `RenderImage` holds an `RgbaImage` whose bytes it reads as BGRA — see
//! `images::to_bgra`, which exists to swap them for the still-picture path.
//! Asking the converter for `BGRA` means the video path never swaps anything:
//! the bytes come off the decoder in the order the GPU wants them.
//!
//! ## Linux only, and the door is the same shape on the others
//!
//! GStreamer is a *link-time* dependency, so `main.rs` compiles this module on
//! Linux alone and picks a sibling everywhere else: `pipeline_mac.rs` against
//! AVFoundation, `pipeline_win.rs` against the Media Foundation Media Engine,
//! `pipeline_off.rs` on anything exotic. Satisfying GStreamer on those two
//! would have meant shipping the runtime inside the installer, and both of them
//! already have a decoder in the operating system — `RELEASING.md` has the
//! arithmetic.
//!
//! All four are the same [`Stack`] and the same [`Beat`], and nothing in
//! `board_view.rs` knows which it is holding. That is the constraint worth
//! keeping: a method added for one of them is a method the other three have to
//! answer, and each time so far the honest version was one all four could.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::RenderImage;
use gstreamer as gst;
use gstreamer::prelude::*;
use gstreamer_app as gst_app;
use gstreamer_video as gst_video;
use image::{Frame, RgbaImage};

/// The longest edge a decoded frame is scaled to on its way out.
///
/// The same ceiling still pictures keep — see `images::LONGEST_SIDE` — and for
/// the same reason: a card is drawn a few hundred units across, and a 4K frame
/// is thirty-three megabytes uploaded sixty times a second to be drawn at a
/// twentieth of its size. The scaling happens **inside the pipeline**, where
/// `videoscale` can hand it to whatever the machine has, rather than in our own
/// code a frame at a time.
const LONGEST_SIDE: i32 = 1024;

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
#[derive(Debug, Clone, Default)]
pub struct Beat {
    /// Where the playhead really is, according to the pipeline rather than to a
    /// wall clock. This is the whole reason the poll exists: a decoder that
    /// stalls for two hundred milliseconds has a playhead that stalls with it,
    /// and a scrubber running on a timer would slide away from the sound.
    pub at: Duration,
    /// How long the whole thing is, once the demuxer knows — which is usually
    /// a frame or two after it starts, not immediately.
    pub length: Option<Duration>,
    /// The end was reached this frame.
    pub ended: bool,
    /// A new picture arrived this frame, so the board has something to redraw.
    pub fresh: bool,
    /// Something went wrong, said in words a person can read. The card stops
    /// asking after this — see [`Reel::broken`].
    pub trouble: Option<String>,
}

/// One card's pipeline.
struct Reel {
    play: gst::Element,
    /// Where frames come back, on a card that has pictures. `None` for sound,
    /// which needs nothing back: `playbin3` reaches the speakers by itself.
    sink: Option<gst_app::AppSink>,
    /// The newest frame, kept so the painter has something to draw on every
    /// frame rather than only on the ones a new picture arrived in.
    picture: Option<Arc<RenderImage>>,
    length: Option<Duration>,
    /// Set once, and then reported once. A reel that has failed is left in
    /// place rather than dropped, so that nothing tries to open it again on the
    /// very next frame and fail in a loop.
    broken: Option<String>,
    /// Whether the failure has been handed to the view yet.
    told: bool,
    /// Since when this reel has been standing still, for [`Stack::trim`].
    /// `None` while it is playing.
    rested: Option<Instant>,
}

/// Every reel there is, and the one-time setup behind them.
pub struct Stack {
    /// `None` until somebody asks for the first time; `Some(false)` on a
    /// machine with no usable GStreamer, which is a state rather than a crash.
    started: Option<bool>,
    /// Where played files are laid out. See [`crate::spill`].
    spill: Option<crate::spill::Spill>,
    reels: HashMap<String, Reel>,
}

impl Default for Stack {
    fn default() -> Self {
        Self::new()
    }
}

impl Stack {
    pub fn new() -> Self {
        Self { started: None, spill: None, reels: HashMap::new() }
    }

    /// Bring GStreamer up, once, and answer whether there is a media stack at
    /// all.
    ///
    /// Lazy rather than at startup: a board of photographs never touches this,
    /// and initialising a media framework — which scans the plugin registry —
    /// is not something to make every launch pay for on the chance somebody
    /// drops a video in.
    ///
    /// A machine without the libraries answers `false` for the rest of the
    /// session and every card says so in words. That is the soft failure the
    /// module note promises: the app opens, the board draws, the photographs
    /// work, and the clip tells you why it does not.
    fn start(&mut self) -> bool {
        if let Some(known) = self.started {
            return known;
        }
        let up = gst::init().is_ok();
        self.started = Some(up);
        if up {
            self.spill = Some(crate::spill::Spill::open());
        }
        up
    }

    /// Make sure there is a pipeline for this card, and say how far off one is.
    ///
    /// Idempotent, and cheap on every frame after the first: a card that
    /// already has a reel returns immediately. `bytes` is only *copied* on the
    /// frame that starts the spill — see [`crate::spill`], which is also where
    /// the third answer comes from.
    pub fn open(&mut self, id: &str, hash: &str, ext: &str, bytes: &[u8], video: bool) -> Opening {
        if self.reels.contains_key(id) {
            return match self.reels[id].broken.is_none() {
                true => Opening::Ready,
                false => Opening::Refused,
            };
        }
        if !self.start() {
            self.fail(id, "no media stack on this machine".into());
            return Opening::Refused;
        }
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
        match build(&path, video) {
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
            // A dead reel still needs an element to hold the shape of the
            // struct. A `fakesink` is the cheapest thing GStreamer will build
            // and it is never set to playing.
            play: gst::ElementFactory::make("fakesink").build().expect("fakesink always exists"),
            sink: None,
            picture: None,
            length: None,
            broken: Some(why),
            told: false,
            // Never trimmed for age: a broken reel is what stops the card
            // trying the same file again on the very next frame, and is the
            // cheapest thing in the map.
            rested: None,
        });
    }

    /// Start, or carry on. Nothing happens to a reel that is already playing.
    ///
    /// Called every frame for every playing card rather than at the moment
    /// somebody presses the button — see `BoardView::pump_media`. GStreamer
    /// takes a state it is already in as a no-op, which is what makes "the
    /// pipeline follows the playhead" affordable as a rule.
    pub fn play(&mut self, id: &str) {
        let Some(reel) = self.reels.get_mut(id).filter(|reel| reel.broken.is_none()) else {
            return;
        };
        reel.rested = None;
        let _ = reel.play.set_state(gst::State::Playing);
    }

    pub fn pause(&mut self, id: &str) {
        let Some(reel) = self.reels.get_mut(id).filter(|reel| reel.broken.is_none()) else {
            return;
        };
        // Stamped once and not on every frame it stays paused, so that the
        // stamp says when it stopped rather than when it was last asked.
        reel.rested.get_or_insert_with(Instant::now);
        let _ = reel.play.set_state(gst::State::Paused);
    }

    /// Move the playhead. `at` is from the start.
    ///
    /// `FLUSH` because this is somebody dragging a scrubber and what they want
    /// is the picture to follow the hand; `KEY_UNIT` because landing on the
    /// nearest keyframe is both far faster and what every other player does.
    pub fn seek(&mut self, id: &str, at: Duration) {
        let Some(reel) = self.reels.get(id).filter(|reel| reel.broken.is_none()) else { return };
        let to = gst::ClockTime::try_from(at).unwrap_or(gst::ClockTime::ZERO);
        let _ = reel.play.seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, to);
    }

    /// How loud, `0.0..=1.0`, and whether it is silenced.
    ///
    /// Both are `playbin3`'s own properties rather than a volume element we
    /// place ourselves, which is the whole reason `playbin3` is here instead of
    /// a hand-built `uridecodebin3`.
    pub fn set_loudness(&mut self, id: &str, level: f32, muted: bool) {
        let Some(reel) = self.reels.get(id).filter(|reel| reel.broken.is_none()) else { return };
        reel.play.set_property("volume", level.clamp(0.0, 1.0) as f64);
        reel.play.set_property("mute", muted);
    }

    /// One frame of the clock for one card: drain the bus, take the newest
    /// picture, and read where the playhead really is.
    ///
    /// `looping` is taken rather than remembered because it is board state and
    /// can change under a playing card — see `media::set_looping`. A loop is a
    /// seek back to nought rather than a restart: the pipeline stays up, so
    /// there is no gap between the end and the beginning.
    pub fn poll(&mut self, id: &str, looping: bool) -> Option<Beat> {
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

        // The bus first, because what it has to say — an error, the end —
        // outranks whatever position the pipeline would otherwise report.
        if let Some(bus) = reel.play.bus() {
            while let Some(message) = bus.pop_filtered(&[
                gst::MessageType::Error,
                gst::MessageType::Eos,
                gst::MessageType::DurationChanged,
            ]) {
                match message.view() {
                    gst::MessageView::Error(problem) => {
                        // The element's own words, which name the codec or the
                        // file rather than saying "playback failed".
                        let why = problem.error().to_string();
                        reel.broken = Some(why.clone());
                        reel.told = true;
                        let _ = reel.play.set_state(gst::State::Null);
                        return Some(Beat { trouble: Some(why), ..beat });
                    }
                    gst::MessageView::Eos(_) => beat.ended = true,
                    // The length is not known when a file opens, and for some
                    // containers it changes once the demuxer has read further
                    // in. Cleared here so the read below asks again.
                    gst::MessageView::DurationChanged(_) => reel.length = None,
                    _ => {}
                }
            }
        }

        if let Some(sink) = &reel.sink {
            // Every frame that has arrived since the last poll, keeping only
            // the last: the sink is capped at a couple of buffers and drops
            // what it cannot hold, so this is a drain rather than a queue —
            // a board that fell behind shows the newest picture rather than
            // working through a backlog in slow motion.
            let mut newest = None;
            while let Some(sample) = sink.try_pull_sample(gst::ClockTime::ZERO) {
                newest = Some(sample);
            }
            if let Some(sample) = newest {
                if let Some(picture) = frame_of(&sample) {
                    reel.picture = Some(picture);
                    beat.fresh = true;
                }
            }
        }

        if reel.length.is_none() {
            reel.length = reel.play.query_duration::<gst::ClockTime>().map(|len| len.into());
        }
        beat.length = reel.length;
        beat.at =
            reel.play.query_position::<gst::ClockTime>().map(Duration::from).unwrap_or_default();

        if beat.ended {
            match looping {
                // Back to the beginning without tearing the pipeline down, so
                // a loop has no gap in it. `beat.at` is left where the end put
                // it: the caller decides what a finished clip reads as.
                true => {
                    let _ = reel.play.seek_simple(gst::SeekFlags::FLUSH, gst::ClockTime::ZERO);
                    beat.at = Duration::ZERO;
                    beat.ended = false;
                }
                // Held at the end rather than reset. A clip that snapped back
                // to its first frame the instant it finished would be one you
                // could never see the end of.
                false => {
                    let _ = reel.play.set_state(gst::State::Paused);
                }
            }
        }

        Some(beat)
    }

    /// The newest picture for a card, for the painter.
    pub fn picture(&self, id: &str) -> Option<Arc<RenderImage>> {
        self.reels.get(id)?.picture.clone()
    }

    /// Which cards have a pipeline standing, so the frame loop knows what to
    /// poll without walking the board.
    pub fn open_reels(&self) -> Vec<String> {
        self.reels.keys().cloned().collect()
    }

    /// Tear one down — the card was deleted, stopped, or pushed out by
    /// `playback::AT_ONCE`.
    ///
    /// Set to `Null` first and then dropped. A pipeline dropped while playing
    /// takes its threads with it whenever they notice; stopping it first is
    /// what makes the sound end when the card does.
    pub fn forget(&mut self, id: &str) {
        if let Some(reel) = self.reels.remove(id) {
            let _ = reel.play.set_state(gst::State::Null);
        }
    }

    /// Drop the reels that have been standing still longest, down to `keep` of
    /// them.
    ///
    /// A paused pipeline still holds a decoder and its buffers, and nothing
    /// else here ever lets one go: `playback::AT_ONCE` caps how many cards
    /// *play* at once and leaves the rest paused where they are, which is the
    /// right answer for the playhead and the wrong one for memory. Somebody
    /// working through thirty clips on one board would otherwise finish with
    /// thirty pipelines standing.
    ///
    /// Only resting reels are counted and only resting ones are dropped —
    /// a playing card is never torn out from under itself — and one that is
    /// dropped comes back on the next press with its playhead intact, because
    /// the playhead was never in here. That is the whole reason this is safe
    /// to do behind the board's back.
    pub fn trim(&mut self, keep: usize) {
        let mut resting: Vec<(String, Instant)> = self
            .reels
            .iter()
            .filter_map(|(id, reel)| reel.rested.map(|at| (id.clone(), at)))
            .collect();
        if resting.len() <= keep {
            return;
        }
        // Oldest first, so the ones dropped are the ones nobody has come back
        // to — the same rule `prune` follows on the spill directory.
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

/// Build a `playbin3` for one file.
///
/// The video half is a bin of three: convert to a format the canvas can take,
/// scale it down to something worth uploading, and hand it to an `appsink` the
/// frame loop pulls from. Handed to `playbin3` as its `video-sink`, so the
/// audio half is still entirely `playbin3`'s business — which is the point of
/// using it.
fn build(path: &Path, video: bool) -> Result<Reel, String> {
    let uri = gstreamer::glib::filename_to_uri(path, None)
        .map_err(|_| "that file is somewhere this cannot be named".to_string())?;

    let play = gst::ElementFactory::make("playbin3")
        .build()
        .map_err(|_| "this machine has no playbin3 — is gstreamer-plugins-base installed?")?;
    play.set_property("uri", uri.as_str());

    let sink = match video {
        false => None,
        true => {
            // A range rather than a fixed size: a clip already smaller than the
            // ceiling passes through untouched, and only a large one is scaled.
            // `videoscale` keeps the shape, so nothing here has to do the
            // arithmetic that would get a pixel aspect ratio wrong.
            let caps = format!(
                "video/x-raw,format=BGRA,width=[1,{LONGEST_SIDE}],height=[1,{LONGEST_SIDE}]"
            );
            let caps: gst::Caps =
                caps.parse().map_err(|_| "could not ask for a drawable frame".to_string())?;

            let appsink = gst_app::AppSink::builder()
                .caps(&caps)
                // Two, and dropping. The frame loop takes the newest and lets
                // the rest go — see `poll` — so a deep queue would only buy
                // frames nobody will ever draw.
                .max_buffers(2)
                .drop(true)
                // Kept on: this is what makes the picture arrive in time with
                // the sound rather than as fast as the decoder can manage.
                .sync(true)
                .build();

            let convert = gst::ElementFactory::make("videoconvert")
                .build()
                .map_err(|_| "no videoconvert on this machine")?;
            let scale = gst::ElementFactory::make("videoscale")
                .build()
                .map_err(|_| "no videoscale on this machine")?;

            let bin = gst::Bin::new();
            bin.add_many([&convert, &scale, appsink.upcast_ref()])
                .map_err(|_| "could not assemble the video sink")?;
            gst::Element::link_many([&convert, &scale, appsink.upcast_ref()])
                .map_err(|_| "could not link the video sink")?;
            // The bin has to look like one element to `playbin3`, which means
            // its first element's sink pad has to be the bin's own.
            let pad = convert.static_pad("sink").ok_or("videoconvert has no sink pad")?;
            let ghost = gst::GhostPad::with_target(&pad)
                .map_err(|_| "could not expose the video sink's pad")?;
            bin.add_pad(&ghost).map_err(|_| "could not expose the video sink's pad")?;

            play.set_property("video-sink", &bin);
            Some(appsink)
        }
    };

    // Paused rather than playing: this brings the file up to the point where
    // the demuxer knows what is in it and the first frame is decoded, without
    // starting the sound. The caller presses play.
    play.set_state(gst::State::Paused)
        .map_err(|_| "nothing on this machine can open that file".to_string())?;

    // Built paused, so `rested` starts stamped: a reel that is opened and then
    // never played — which happens when the press that opened it is undone in
    // the same breath — is trimmable like any other.
    Ok(Reel {
        play,
        sink,
        picture: None,
        length: None,
        broken: None,
        told: false,
        rested: Some(Instant::now()),
    })
}

/// One decoded frame, as a picture the canvas can draw.
///
/// The copy is row by row rather than one `to_vec`, because a decoder is
/// entitled to pad every row out to an alignment it likes and very often does:
/// a 1918-pixel-wide frame commonly arrives with a stride of 1920 times four,
/// and reading it as though it were tight produces a picture that shears
/// diagonally.
fn frame_of(sample: &gst::Sample) -> Option<Arc<RenderImage>> {
    use gst_video::prelude::*;

    let caps = sample.caps()?;
    let info = gst_video::VideoInfo::from_caps(caps).ok()?;
    let buffer = sample.buffer()?;
    let frame = gst_video::VideoFrameRef::from_buffer_ref_readable(buffer, &info).ok()?;

    let (width, height) = (frame.width(), frame.height());
    if width == 0 || height == 0 {
        return None;
    }
    let stride = *frame.plane_stride().first()? as usize;
    let data = frame.plane_data(0).ok()?;
    let row = width as usize * 4;
    if stride < row {
        return None;
    }

    let mut out = Vec::with_capacity(row * height as usize);
    for y in 0..height as usize {
        let from = y * stride;
        out.extend_from_slice(data.get(from..from + row)?);
    }

    // Already BGRA — see the module note. `RenderImage` reads an `RgbaImage`'s
    // bytes in that order, so this is a rename rather than a conversion.
    let image = RgbaImage::from_raw(width, height, out)?;
    Some(Arc::new(RenderImage::new(vec![Frame::new(image)])))
}
