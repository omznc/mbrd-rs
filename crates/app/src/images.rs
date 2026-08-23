//! Turning the bytes a board carries into something the GPU can draw.
//!
//! A `.mbrd` holds its pictures as whole encoded files — a JPEG is a JPEG — and
//! the compositor wants premultiplied BGRA in an atlas. Between the two sits a
//! decode, which for a twenty-four megapixel photograph is a hundred
//! milliseconds and change. Doing that on the thread that draws is six dropped
//! frames per picture, so it happens on the background executor and the card
//! draws as a plain quad until it lands.
//!
//! Three things this has to get right, in the order they bite:
//!
//! 1. **Decode once.** Keyed by content hash, which is the same key the format
//!    uses, so the same photograph on twelve cards is one decode and one
//!    texture. That is also why a failed decode is *remembered* — a file that
//!    is not really a PNG must not be retried on every frame forever.
//! 2. **Scale down.** The atlas is finite and a photograph is usually drawn at
//!    a tenth of its size. Full resolution costs ninety megabytes for something
//!    that will occupy three hundred pixels, so an image is capped on its
//!    longest side on the way in. The cap is generous enough to survive zooming
//!    in a long way, and the day it is not is the day this grows a second,
//!    sharper tier.
//! 3. **Let go.** This is the one the roadmap flags, and it is not automatic:
//!    dropping the last `Arc<RenderImage>` frees the pixels but leaves the
//!    sprite atlas holding a tile, so a long session over a big board grows
//!    without bound in the one place a memory profile does not look.
//!    [`Images::sweep`] is what actually tells the window, and it is the reason
//!    eviction happens where there is a `&mut Window` rather than wherever the
//!    cache happened to notice it was full.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{RenderImage, Window};
use image::{Frame, RgbaImage};

/// How much decoded pixel data to hold before the oldest starts falling out.
///
/// Two hundred and fifty megabytes is somewhere near a hundred photographs at
/// the cap below — a landscape one held at its longest side is under three —
/// which is more than a screenful at any zoom and less than a laptop minds.
const BUDGET: usize = 250 * 1024 * 1024;

/// The longest side an image is kept at, in pixels.
///
/// A card is at most [`MAX_SIZE`](mbrd_core::model::MAX_SIZE) world units and
/// the camera goes to five times, so this is short of sharp at the extremes. It
/// is chosen against the common case instead: a photograph on a normal card
/// filling a normal screen, where anything beyond this is detail nobody is
/// looking at, held at four bytes a pixel.
///
/// **This is a drawing cost as much as a memory one, and that is the half that
/// is easy to miss.** gpui's sprite atlas builds its textures with one mip
/// level — there is no mipmap chain to fall back to — so a card three hundred
/// pixels across drawn from a two-thousand-pixel texture takes one texel in
/// fifty, and every one of those fetches misses the texture cache. A screenful
/// of photographs is then a screenful of that, on every frame, and it is why a
/// board of pictures drags worse than a board of notes rather than merely
/// costing more to load. Halving this quarters both the bytes held and the
/// spread each fetch reaches across, and it also brings a picture back under
/// the atlas's own 1024-pixel texture size, so several share one texture and
/// the sprites for them batch into one draw call instead of one apiece.
///
/// A thousand is still two-to-one over a photograph filling a normal card, so
/// zooming in stays sharp for a good way past where anybody stops. Past that it
/// softens, and the day that is not good enough is the day this grows the
/// second, sharper tier the header talks about — decoded on demand for the one
/// card that is actually large on screen, rather than for every picture on the
/// board against the chance that one of them might be.
const LONGEST_SIDE: u32 = 1024;

/// How long a picture takes to arrive once it has decoded, in seconds.
///
/// A decode lands on whatever frame it happens to finish on, which for a large
/// photograph is a third of a second after the card appeared. Swapping a flat
/// coloured quad for a photograph between two frames is the most abrupt thing
/// that happens on this board, and it happens on its own — nobody did
/// anything, so there is no gesture for the change to be the consequence of.
///
/// Short: this is a picture becoming visible, not an entrance.
const ARRIVING: Duration = Duration::from_millis(220);

/// What the cache can say about one hash.
pub enum Load {
    /// Decoded and ready to paint, and how far it has arrived — `0.0` on the
    /// frame it landed, `1.0` once it is fully there. See [`ARRIVING`].
    Ready(Arc<RenderImage>, f32),
    /// Somebody is already decoding it. Draw the placeholder and wait.
    Waiting,
    /// Nobody has asked yet. The caller should start a decode and say so with
    /// [`Images::begin`].
    Cold,
    /// Tried, and these bytes are not a picture. Never tried again.
    Failed,
}

enum Slot {
    Waiting,
    Ready { image: Arc<RenderImage>, cost: usize, at: Instant },
    Failed,
}

/// Decoded pictures, keyed by content hash, oldest thrown out first.
pub struct Images {
    slots: HashMap<String, Slot>,
    /// Least-recently wanted first. Holds only hashes that are `Ready`.
    order: Vec<String>,
    held: usize,
    /// How much may be held before the oldest starts falling out.
    ///
    /// A field rather than the constant directly so that the eviction rules can
    /// be tested against four small pictures instead of four enormous ones.
    budget: usize,
    /// When the most recent picture landed, so that the frame clock can ask
    /// whether anything is still arriving without walking every slot.
    ///
    /// The newest is enough for all of them: if the last one to arrive has
    /// finished arriving, so has everything that came before it.
    newest: Option<Instant>,
    /// Pictures evicted since the last sweep, still holding an atlas tile.
    ///
    /// Eviction can happen at any point in a frame; releasing the tile needs
    /// the window, which exists at exactly one. So the two are separated, and
    /// this is the queue between them.
    dropped: Vec<Arc<RenderImage>>,
}

impl Default for Images {
    fn default() -> Self {
        Self {
            slots: HashMap::new(),
            order: Vec::new(),
            held: 0,
            budget: BUDGET,
            newest: None,
            dropped: Vec::new(),
        }
    }
}

impl Images {
    /// What is known about this hash, and mark it as wanted.
    pub fn look(&mut self, hash: &str) -> Load {
        match self.slots.get(hash) {
            Some(Slot::Ready { image, at, .. }) => {
                let image = image.clone();
                let arrived = at.elapsed().div_duration_f32(ARRIVING).min(1.0);
                self.touch(hash);
                Load::Ready(image, arrived)
            }
            Some(Slot::Waiting) => Load::Waiting,
            Some(Slot::Failed) => Load::Failed,
            None => Load::Cold,
        }
    }

    /// Claim a hash before starting a decode, so the next frame does not start
    /// a second one. Answers `false` if somebody got there first.
    pub fn begin(&mut self, hash: &str) -> bool {
        if self.slots.contains_key(hash) {
            return false;
        }
        self.slots.insert(hash.to_string(), Slot::Waiting);
        true
    }

    /// Hand back what the decode produced. `None` means it was not a picture.
    pub fn settle(&mut self, hash: &str, decoded: Option<Arc<RenderImage>>) {
        match decoded {
            Some(image) => {
                let cost = cost_of(&image);
                let at = Instant::now();
                self.held += cost;
                self.slots.insert(hash.to_string(), Slot::Ready { image, cost, at });
                self.order.push(hash.to_string());
                self.newest = Some(at);
                self.evict_down_to_budget();
            }
            None => {
                self.slots.insert(hash.to_string(), Slot::Failed);
            }
        }
    }

    /// Release the atlas tiles of everything evicted since the last call.
    ///
    /// Call once a frame, from somewhere with a window. Skipping it does not
    /// corrupt anything — it leaks, quietly, which is why it is one line at the
    /// top of `render` rather than something to remember at each call site.
    pub fn sweep(&mut self, window: &mut Window) {
        for image in self.dropped.drain(..) {
            // Best effort: a tile that was never uploaded has nothing to drop,
            // and a window on its way out will not take instructions. Neither
            // is worth failing a frame over.
            let _ = window.drop_image(image);
        }
    }

    /// How much decoded pixel data is being held, in bytes. For the status bar.
    pub fn bytes_held(&self) -> usize {
        self.held
    }

    /// Whether any picture is still on its way in.
    pub fn arriving(&self) -> bool {
        self.newest.is_some_and(|at| at.elapsed() < ARRIVING)
    }

    /// How many pictures are decoded and ready.
    pub fn ready_count(&self) -> usize {
        self.order.len()
    }

    fn touch(&mut self, hash: &str) {
        if let Some(at) = self.order.iter().position(|h| h == hash) {
            let held = self.order.remove(at);
            self.order.push(held);
        }
    }

    fn evict_down_to_budget(&mut self) {
        // Never down to nothing: one picture larger than the whole budget
        // should be held anyway rather than decoded again every frame to be
        // thrown away again every frame.
        while self.held > self.budget && self.order.len() > 1 {
            let oldest = self.order.remove(0);
            if let Some(Slot::Ready { image, cost, .. }) = self.slots.remove(&oldest) {
                self.held = self.held.saturating_sub(cost);
                self.dropped.push(image);
            }
        }
    }
}

/// What one decoded picture costs to hold, in bytes.
fn cost_of(image: &Arc<RenderImage>) -> usize {
    (0..image.frame_count())
        .map(|i| {
            let size = image.size(i);
            (size.width.0.max(0) as usize) * (size.height.0.max(0) as usize) * 4
        })
        .sum()
}

/// Decode one asset's bytes into something paintable, or `None`.
///
/// Runs on the background executor and touches nothing but its argument, which
/// is the whole reason it is a free function rather than a method: there is no
/// `&mut self` here to be tempted across a thread boundary.
///
/// Returns `None` for anything that is not a still picture this build can read,
/// which includes every video and audio file — those cards draw their
/// `meta.cover` instead, and the cover is a picture.
pub fn decode(bytes: &[u8]) -> Option<Arc<RenderImage>> {
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    // The catalogue is not trusted for this — the extension in the archive is
    // a hint for rebuilding a media type, not a promise about the contents.
    let decoded = reader.decode().ok()?;

    let mut rgba = shrink(decoded);
    // `RenderImage` wants BGRA and `image` produces RGBA, and this is the whole
    // of the difference: swap the two ends of each pixel in place. Doing it
    // here rather than at paint time means it is paid once, off the main
    // thread, instead of on every upload.
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }

    Some(Arc::new(RenderImage::new(vec![Frame::new(rgba)])))
}

/// Hold a picture to [`LONGEST_SIDE`], keeping its shape.
fn shrink(decoded: image::DynamicImage) -> RgbaImage {
    let (w, h) = (decoded.width(), decoded.height());
    let longest = w.max(h);
    if longest <= LONGEST_SIDE || longest == 0 {
        return decoded.into_rgba8();
    }
    let scale = LONGEST_SIDE as f32 / longest as f32;
    let (tw, th) = (((w as f32 * scale) as u32).max(1), ((h as f32 * scale) as u32).max(1));
    // Triangle rather than Lanczos: a moodboard thumbnail is going to be
    // resampled again by the GPU on its way to the screen, and the sharper
    // filter's extra pass is not visible through that.
    decoded.resize_exact(tw, th, image::imageops::FilterType::Triangle).into_rgba8()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny real PNG, encoded here so the test needs no fixture on disk.
    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn a_picture_decodes_and_comes_back_the_size_it_was() {
        let image = decode(&png(64, 32)).expect("that is a png");
        let size = image.size(0);
        assert_eq!((size.width.0, size.height.0), (64, 32));
    }

    #[test]
    fn the_colours_come_back_the_other_way_round() {
        // The atlas wants BGRA. The pixel written was r=10, g=20, b=30, so the
        // bytes held should start with 30 and end with 10.
        let image = decode(&png(2, 2)).expect("that is a png");
        let bytes = image.as_bytes(0).expect("frame zero");
        assert_eq!(&bytes[..4], &[30, 20, 10, 255]);
    }

    #[test]
    fn a_picture_far_too_large_is_brought_down_to_the_cap() {
        // `shrink` rather than `decode`, because encoding a four-megapixel PNG
        // to test the resize in it is twenty seconds of testing the encoder.
        let huge =
            image::DynamicImage::ImageRgba8(RgbaImage::new(LONGEST_SIDE * 2, LONGEST_SIDE / 2));
        let out = shrink(huge);
        assert_eq!(out.width(), LONGEST_SIDE);
        // And its shape survived the trip.
        assert_eq!(out.height(), LONGEST_SIDE / 4);
    }

    #[test]
    fn a_picture_already_small_enough_is_left_alone() {
        let small = image::DynamicImage::ImageRgba8(RgbaImage::new(300, 200));
        let out = shrink(small);
        assert_eq!((out.width(), out.height()), (300, 200));
    }

    #[test]
    fn something_that_is_not_a_picture_is_refused_rather_than_guessed_at() {
        assert!(decode(b"this is not a png, whatever it is called").is_none());
        assert!(decode(&[]).is_none());
    }

    /// A decoded picture of a given size, without going near a decoder.
    fn pixels(side: u32) -> Arc<RenderImage> {
        Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::new(side, side))]))
    }

    /// A cache with room for about four of `pixels(32)`.
    fn small_cache() -> Images {
        Images { budget: 32 * 32 * 4 * 4, ..Default::default() }
    }

    #[test]
    fn asking_twice_only_starts_one_decode() {
        let mut images = Images::default();
        assert!(matches!(images.look("a"), Load::Cold));
        assert!(images.begin("a"));
        assert!(!images.begin("a"), "a second decode was started");
        assert!(matches!(images.look("a"), Load::Waiting));
    }

    #[test]
    fn a_picture_arrives_rather_than_appearing() {
        let mut images = Images::default();
        images.begin("a");
        images.settle("a", Some(pixels(32)));
        let Load::Ready(_, arrived) = images.look("a") else {
            panic!("it should be ready");
        };
        // On the frame it lands it is barely there, which is the whole
        // difference between a decode arriving and a decode being swapped in.
        assert!(arrived < 0.5, "it turned up fully formed, at {arrived}");
        assert!(images.arriving(), "the frame clock was not told to keep going");
    }

    #[test]
    fn a_picture_that_has_been_there_a_while_is_not_still_arriving() {
        let mut images = Images::default();
        images.begin("a");
        images.settle("a", Some(pixels(32)));
        // Reached into rather than waited for: a test that slept for a fifth
        // of a second to watch a fade would be a fifth of a second every run,
        // forever, to assert arithmetic.
        images.newest = Some(Instant::now() - ARRIVING * 2);
        if let Some(Slot::Ready { at, .. }) = images.slots.get_mut("a") {
            *at = Instant::now() - ARRIVING * 2;
        }
        let Load::Ready(_, arrived) = images.look("a") else {
            panic!("it should be ready");
        };
        assert_eq!(arrived, 1.0);
        assert!(!images.arriving(), "it never stopped asking for frames");
    }

    #[test]
    fn a_file_that_is_not_a_picture_is_not_tried_again() {
        let mut images = Images::default();
        images.begin("bad");
        images.settle("bad", None);
        assert!(matches!(images.look("bad"), Load::Failed));
        assert!(!images.begin("bad"), "it went back for another go");
    }

    #[test]
    fn the_oldest_picture_falls_out_when_there_is_no_room() {
        let mut images = small_cache();
        for name in ["a", "b", "c", "d", "e"] {
            images.begin(name);
            images.settle(name, Some(pixels(32)));
        }
        assert!(images.held <= images.budget, "held {} over {}", images.held, images.budget);
        assert_eq!(images.ready_count(), 4);
        assert!(matches!(images.look("a"), Load::Cold), "the oldest should be gone");
        assert!(matches!(images.look("e"), Load::Ready(..)), "the newest should be here");
        assert_eq!(images.dropped.len(), 1, "its tile is queued for the window");
    }

    #[test]
    fn looking_at_a_picture_moves_it_off_the_chopping_block() {
        let mut images = small_cache();
        for name in ["a", "b", "c", "d"] {
            images.begin(name);
            images.settle(name, Some(pixels(32)));
        }
        // "a" is the oldest — until it is looked at.
        assert!(matches!(images.look("a"), Load::Ready(..)));
        images.begin("e");
        images.settle("e", Some(pixels(32)));
        assert!(matches!(images.look("a"), Load::Ready(..)), "the looked-at one went");
        assert!(matches!(images.look("b"), Load::Cold), "the untouched one stayed");
    }

    #[test]
    fn one_picture_larger_than_the_whole_budget_is_kept_anyway() {
        // Otherwise a board holding one enormous photograph decodes it every
        // frame, to throw it away every frame, forever.
        let mut images = small_cache();
        images.begin("huge");
        images.settle("huge", Some(pixels(256)));
        assert!(images.held > images.budget);
        assert!(matches!(images.look("huge"), Load::Ready(..)));
    }
}
