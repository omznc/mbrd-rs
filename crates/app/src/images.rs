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
//! 2. **Keep two sizes, and never let go of the small one.** The atlas is
//!    finite and a photograph is usually drawn at a tenth of its size, so every
//!    picture is held twice: a [`THUMB_SIDE`] thumbnail and a [`LONGEST_SIDE`]
//!    sharp copy. Which one a card draws from depends on how large that card is
//!    *on screen this frame* — which is the whole of what makes zooming out
//!    cheap, because a thumbnail is a fortieth of the bytes and a fortieth of
//!    the texture fetches.
//!
//!    The asymmetry is the point. **Only the sharp copy is ever evicted.** A
//!    board with more photographs on screen than the budget holds used to lose
//!    them one at a time and decode them again on the next frame, which looked
//!    like pictures blinking and cards going empty at random — the cache
//!    thrashing, in the one place a user can see it. Now the worst that
//!    happens is that a card falls back to its thumbnail: softer, and only if
//!    you were zoomed in on it. Nothing ever goes back to being blank.
//! 3. **Let go, but not of what is on screen.** This is the one the roadmap
//!    flags, and it is not automatic:
//!    dropping the last `Arc<RenderImage>` frees the pixels but leaves the
//!    sprite atlas holding a tile, so a long session over a big board grows
//!    without bound in the one place a memory profile does not look.
//!    [`Images::sweep`] is what actually tells the window, and it is the reason
//!    eviction happens where there is a `&mut Window` rather than wherever the
//!    cache happened to notice it was full.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
// WASM EXPERIMENT: std's clock panics on wasm32-unknown-unknown; `web-time`
// is the same API over `performance.now()`.
#[cfg(not(target_family = "wasm"))]
use std::time::Instant;
#[cfg(target_family = "wasm")]
use web_time::Instant;

use gpui::{RenderImage, Window};
use image::{Frame, RgbaImage};

/// How much decoded pixel data to hold before the oldest starts falling out.
///
/// Two hundred and fifty megabytes is somewhere near a hundred photographs at
/// the cap below — a landscape one held at its longest side is under three —
/// which is more than a screenful at any zoom and less than a laptop minds.
const BUDGET: usize = 250 * 1024 * 1024;

/// The longest side a **thumbnail** is kept at, and the size at which a card
/// stops being drawn from one.
///
/// This is the copy that is never thrown away, so its cost is what a board of
/// pictures costs at rest: at four bytes a pixel a landscape thumbnail is under
/// 175 KB, so a thousand of them is under 175 MB and a screenful is nothing.
///
/// **256 is not a guess — it is bounded by the screen.** A card only needs the
/// sharp copy when it is drawn larger than this, and a window has a fixed
/// number of pixels: on a 4-megapixel display at most 62 cards can be 256
/// device pixels on a side at once. Sixty-two sharp copies is comfortably
/// inside [`BUDGET`], which is why zooming *in* cannot thrash the cache either.
/// Raising this would drop that ceiling quadratically and raise the resting
/// cost quadratically; lowering it does the reverse.
pub(crate) const THUMB_SIDE: u32 = 256;

/// The longest side the **sharp** copy of an image is kept at, in pixels.
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
/// softens.
///
/// This is the copy that is evicted under pressure, and the only one. See
/// [`THUMB_SIDE`], which is the floor it falls back to.
pub(crate) const LONGEST_SIDE: u32 = 1024;

/// The longest side an **animated** picture's frames are kept at.
///
/// Tighter than [`LONGEST_SIDE`] because the cost is multiplied by the frame
/// count: one still at a thousand pixels is four megabytes, and eighty frames
/// of one is three hundred and twenty. An animation is also the one kind of
/// picture nobody inspects closely — it is moving — so the sharpness this gives
/// up is sharpness nothing was reading.
const ANIMATED_SIDE: u32 = 640;

/// How much decoded pixel data one animation may hold.
///
/// A quarter of the whole cache's [`BUDGET`], so that a single long GIF cannot
/// evict every photograph on the board to make room for itself. Past it the
/// animation is **thinned rather than cut** — see [`thin`] — because half the
/// frames of a whole loop still reads as the thing it is, and the first third
/// of one reads as a fault.
const ANIMATION_BUDGET: usize = 64 * 1024 * 1024;

/// How recently a picture has to have been drawn to be safe from eviction.
///
/// **This is the anti-thrash rule, and it is the other half of the fix that
/// two tiers is the first half of.** Eviction walks least-recently-drawn first,
/// and on a board with more pictures on screen than the budget holds, *every*
/// candidate was drawn this frame — so the oldest is merely the one that
/// happened to be painted first, and throwing it out means throwing out
/// something that is on screen right now. It comes back next frame, evicts the
/// next one, and the board blinks its way around the z-order forever.
///
/// So: a picture drawn within the last few frames is not a candidate at all,
/// and the cache goes **over** its budget rather than take one. Being briefly
/// over is a number in a memory profile; the alternative is visible.
const FRESH: Duration = Duration::from_millis(250);

/// A hard stop, independent of the budget above.
///
/// The budget alone is not enough: a file can declare an enormous number of
/// tiny frames, which costs little to hold and a great deal to decode. This
/// bounds the decode itself.
const ANIMATION_FRAMES_MAX: usize = 4_000;

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
    ///
    /// Which of the two copies this is depends on the size asked for; see
    /// [`Images::look`]. A card never has to know which one it got.
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
    Ready(Ready),
    Failed,
}

/// A decoded picture, in the one or two sizes it is held at.
struct Ready {
    /// Always here, for as long as the picture is. See [`THUMB_SIDE`].
    thumb: Arc<RenderImage>,
    thumb_cost: usize,
    /// The sharp copy — the only thing eviction takes.
    sharp: Sharp,
    /// How far the arrival fade has come, `0.0..=1.0`. See [`ARRIVING`].
    ///
    /// Advanced by [`Images::tick`] from the frame's own `dt` rather than read
    /// off the wall clock at paint time — the bug that used to leave here: a
    /// picture reads `elapsed()` against when it landed, and reduced motion's
    /// one enormous `dt` never touches a clock, so a photograph on a board
    /// with motion turned off still took two hundred milliseconds to become
    /// fully opaque. Everything else on the frame clock lands instantly under
    /// reduced motion; this was the one straggler still waiting on real time.
    arrived: f32,
    /// When a card last drew it, for eviction. See [`FRESH`].
    seen: Instant,
    /// This hash's place in the recency order — bigger is more recent.
    ///
    /// Stamped from [`Images::stamp`], which only ever grows, so comparing
    /// two of these is the whole of "which was wanted more recently"
    /// without a scan or a list kept in order alongside it. See
    /// [`Images::touch`].
    last_used: u64,
    /// When a card last drew the *sharp* copy specifically — not merely
    /// looked at the picture, but was handed [`Sharp::Here`] and painted
    /// from it. `None` for a picture whose sharp copy has never once been
    /// on screen, whether because nothing has asked for it at full size yet
    /// or because the ask is still [`Sharp::Coming`].
    ///
    /// This is what pass one of eviction checks against [`FRESH`] — see
    /// [`Images::evict_down_to_budget`] for why a sharp copy a card is
    /// actively drawing from is not the one that pass takes back.
    sharp_seen: Option<Instant>,
}

/// Where the sharp copy of a picture has got to.
enum Sharp {
    Here {
        image: Arc<RenderImage>,
        cost: usize,
    },
    /// Evicted. A card drawn larger than [`THUMB_SIDE`] asks for it back.
    Gone,
    /// Asked for, and on its way.
    Coming,
    /// There never was one, and there never will be: the picture is already
    /// smaller than a thumbnail, or it is an animation, which is held once at
    /// [`ANIMATED_SIDE`] and budgeted separately.
    Never,
}

/// What a decode produced: the thumbnail, and the sharp copy if the picture is
/// large enough for the two to differ.
pub struct Decoded {
    pub(crate) thumb: Arc<RenderImage>,
    pub(crate) sharp: Option<Arc<RenderImage>>,
}

/// Decoded pictures, keyed by content hash, oldest thrown out first.
pub struct Images {
    slots: HashMap<String, Slot>,
    /// A monotonic counter, handed out by [`Images::touch`] as
    /// [`Ready::last_used`] — never read on its own, only compared.
    ///
    /// This is the recency order in its entirety. There used to be a
    /// `Vec<String>` here, reordered on every `touch` by an
    /// `iter().position()` scan and a `Vec::remove` — an O(*n*) string
    /// compare and an O(*n*) memmove, paid once per visible picture per
    /// frame, for a card that is usually already at the back of the queue.
    /// A counter turns that into one comparison-free write.
    stamp: u64,
    held: usize,
    /// How much may be held before the oldest starts falling out.
    ///
    /// A field rather than the constant directly so that the eviction rules can
    /// be tested against four small pictures instead of four enormous ones.
    budget: usize,
    /// How many pictures have not yet finished arriving.
    ///
    /// So that [`Images::arriving`] and [`Images::tick`] can both answer in
    /// $O(1)$ on the common frame — which is nearly every frame, since the
    /// window is 220ms — rather than walking every slot to find out that
    /// nothing on the board is currently fading in.
    arriving: usize,
    /// Hashes whose sharp copy a card wanted and did not find. Drained by the
    /// painter, which is the only thing that can start a decode.
    resharpen: Vec<String>,
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
            stamp: 0,
            held: 0,
            budget: BUDGET,
            arriving: 0,
            resharpen: Vec::new(),
            dropped: Vec::new(),
        }
    }
}

impl Images {
    /// What is known about this hash, mark it as wanted, and hand back the copy
    /// that suits the size it is about to be drawn at.
    ///
    /// `wanted` is the card's longest side **in device pixels** — not in the
    /// board's world units and not in logical pixels, because the question this
    /// answers is how many texels the GPU is about to read, and that is a
    /// question about the display. A card at or under [`THUMB_SIDE`] is drawn
    /// from the thumbnail; anything larger asks for the sharp copy and takes
    /// the thumbnail if it is not there.
    pub fn look(&mut self, hash: &str, wanted: f32) -> Load {
        let now = Instant::now();
        let Some(slot) = self.slots.get_mut(hash) else { return Load::Cold };
        let ready = match slot {
            Slot::Ready(ready) => ready,
            Slot::Waiting => return Load::Waiting,
            Slot::Failed => return Load::Failed,
        };

        ready.seen = now;
        let arrived = ready.arrived;

        let image = if wanted > THUMB_SIDE as f32 {
            match &ready.sharp {
                Sharp::Here { image, .. } => {
                    // The one place this is stamped: a card was just handed
                    // the sharp copy and is about to paint from it. See
                    // `Ready::sharp_seen` and pass one of eviction.
                    ready.sharp_seen = Some(now);
                    image.clone()
                }
                // Wanted and not here. The thumbnail stands in — softly, and
                // without a blank frame — while the decode is asked for.
                Sharp::Gone => {
                    ready.sharp = Sharp::Coming;
                    self.resharpen.push(hash.to_string());
                    let thumb = self.thumb_of(hash);
                    self.touch(hash);
                    return Load::Ready(thumb, arrived);
                }
                Sharp::Coming | Sharp::Never => ready.thumb.clone(),
            }
        } else {
            ready.thumb.clone()
        };

        self.touch(hash);
        Load::Ready(image, arrived)
    }

    /// The thumbnail of a hash known to be `Ready`. For the one branch above
    /// that has already given up its borrow of the slot.
    fn thumb_of(&self, hash: &str) -> Arc<RenderImage> {
        match self.slots.get(hash) {
            Some(Slot::Ready(ready)) => ready.thumb.clone(),
            _ => unreachable!("the caller has just looked at it"),
        }
    }

    /// The hashes whose sharp copy somebody wants back, taken away.
    ///
    /// Drained rather than read, because asking twice for the same one would
    /// start two decodes — the slot is already marked [`Sharp::Coming`], which
    /// is what stops the *next frame* asking again.
    pub fn resharpen(&mut self) -> Vec<String> {
        std::mem::take(&mut self.resharpen)
    }

    /// Claim a hash before starting a decode, so the next frame does not start
    /// a second one. Answers `false` if somebody got there first.
    ///
    /// True for a hash nobody has touched, and for one whose sharp copy was
    /// evicted and is wanted back — see [`Sharp::Coming`], which `look` has
    /// already set in that case. Never for one that is on its way, has failed,
    /// or is fully here.
    pub fn begin(&mut self, hash: &str) -> bool {
        match self.slots.get(hash) {
            None => {
                self.slots.insert(hash.to_string(), Slot::Waiting);
                true
            }
            Some(Slot::Ready(ready)) => matches!(ready.sharp, Sharp::Coming),
            _ => false,
        }
    }

    /// Hand back what the decode produced. `None` means it was not a picture.
    pub fn settle(&mut self, hash: &str, decoded: Option<Decoded>) {
        let Some(decoded) = decoded else {
            self.slots.insert(hash.to_string(), Slot::Failed);
            return;
        };

        // Whether this decode is delivering a *resharpen* — a sharp copy a
        // card already asked for and is waiting on — rather than a picture's
        // first decode. It matters for the grace `sharp_seen` gets below: a
        // resharpened copy has a card waiting on it that just has not drawn
        // a frame since it landed, where a first decode has not been asked
        // for at full size by anyone yet.
        let was_resharpen = matches!(self.slots.get(hash), Some(Slot::Ready(r)) if matches!(r.sharp, Sharp::Coming));

        // A re-decode replaces what was there, and if the picture it is
        // replacing had not finished arriving yet, that count has to come
        // down before the new one — starting again at `0.0` — puts it back
        // up. Missing this would leave `arriving` one too high forever, and
        // the frame clock asking for frames nobody needs.
        if matches!(self.slots.get(hash), Some(Slot::Ready(r)) if r.arrived < 1.0) {
            self.arriving = self.arriving.saturating_sub(1);
        }

        // A re-decode replaces what was there, so its bytes stop being counted
        // before the new ones start. Missing this is a `held` that only ever
        // grows, which reads as a cache that has stopped evicting.
        self.let_go_of(hash);

        let at = Instant::now();
        let thumb_cost = cost_of(&decoded.thumb);
        let sharp = match decoded.sharp {
            Some(image) => Sharp::Here { cost: cost_of(&image), image },
            None => Sharp::Never,
        };
        self.held += thumb_cost + sharp_cost(&sharp);
        // Settling counts as being wanted, same as a `look` — see `touch`.
        self.stamp += 1;

        self.slots.insert(
            hash.to_string(),
            Slot::Ready(Ready {
                thumb: decoded.thumb,
                thumb_cost,
                sharp,
                arrived: 0.0,
                seen: at,
                last_used: self.stamp,
                // A resharpen gets one `FRESH` window of protection from
                // pass one of eviction it has not earned by being drawn yet
                // — because without it, a sharp copy delivered while the
                // sharp working set is over budget is stripped straight back
                // off by the `evict_down_to_budget` call below, before the
                // card that asked for it ever gets a frame to paint it. The
                // next `look` sees `Sharp::Gone` again, asks again, and
                // decodes again: the exact treadmill this field exists to
                // stop. A first decode gets no such grace — nothing has
                // wanted it sharp yet, so there is nothing to protect, and
                // `the_sharp_copy_is_what_goes_first_and_the_card_never_goes_blank`
                // is the test that would catch giving it one by mistake.
                sharp_seen: was_resharpen.then_some(at),
            }),
        );
        // Newly settled, so it starts at `0.0` and has somewhere to go — see
        // `tick`.
        self.arriving += 1;
        self.evict_down_to_budget();
    }

    /// Stop counting whatever this hash was already holding, and queue its
    /// tiles for the window. Leaves the slot itself alone.
    fn let_go_of(&mut self, hash: &str) {
        let Some(Slot::Ready(ready)) = self.slots.get_mut(hash) else { return };
        self.held = self.held.saturating_sub(ready.thumb_cost);
        self.dropped.push(ready.thumb.clone());
        if let Sharp::Here { image, cost } = std::mem::replace(&mut ready.sharp, Sharp::Gone) {
            self.held = self.held.saturating_sub(cost);
            self.dropped.push(image);
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

    /// Whether any picture is still on its way in.
    pub fn arriving(&self) -> bool {
        self.arriving > 0
    }

    /// Bring every arrival fade `dt` seconds nearer done.
    ///
    /// Called from `BoardView::advance` with the frame's own `dt`, which is
    /// what lets reduced motion land every picture at full strength in one
    /// pass rather than the two hundred and twenty milliseconds `ARRIVING`
    /// otherwise takes — see `Ready::arrived` for the bug this replaced.
    /// Guarded on `self.arriving` so an idle board, which is nearly always,
    /// costs this function one comparison rather than a walk of every slot.
    pub fn tick(&mut self, dt: f32) {
        if self.arriving == 0 || dt <= 0.0 {
            return;
        }
        let step = dt / ARRIVING.as_secs_f32();
        for slot in self.slots.values_mut() {
            let Slot::Ready(ready) = slot else { continue };
            if ready.arrived >= 1.0 {
                continue;
            }
            ready.arrived = (ready.arrived + step).min(1.0);
            if ready.arrived >= 1.0 {
                self.arriving -= 1;
            }
        }
    }

    /// How many pictures are decoded and ready.
    ///
    /// For the eviction test below, and nothing else — the bottom bar used to
    /// report this and no longer does. See `BoardView::status_bar`, which says
    /// why a count that is true all day is a count nobody reads.
    #[cfg(test)]
    pub fn ready_count(&self) -> usize {
        self.slots.values().filter(|slot| matches!(slot, Slot::Ready(_))).count()
    }

    /// Mark a hash as the most recently wanted.
    ///
    /// $O(1)$: a hashmap lookup and an integer write. This used to be a
    /// linear scan of every hash held — `iter().position()` — followed by a
    /// `Vec::remove` to move it to the back of an order list: an O(*n*)
    /// string compare and an O(*n*) memmove, paid once per visible picture
    /// per frame, for a card that in the common case is already at the back
    /// of the queue. `stamp` only ever grows, so "more recently wanted than"
    /// is just "compares greater than", and that is all eviction's pass two
    /// needs [`Ready::last_used`] for.
    fn touch(&mut self, hash: &str) {
        self.stamp += 1;
        if let Some(Slot::Ready(ready)) = self.slots.get_mut(hash) {
            ready.last_used = self.stamp;
        }
    }

    /// Come back under budget, in two passes, giving up the least first.
    ///
    /// Both passes work by scanning the slot map directly rather than
    /// consulting a maintained order list — there is no such list any more.
    /// `settle` lands once per landed decode and `evict_down_to_budget` runs
    /// once per `settle`, against everything held, which for a board a
    /// screen can hold is at most a few hundred entries. That is the cheap
    /// side of the trade against keeping a `Vec<String>` in recency order:
    /// see [`Images::touch`], which is the thing that used to cost more than
    /// this does.
    ///
    /// **Pass one takes only sharp copies**, least-recently-drawn-sharp
    /// first, and skips one that is fresh: specifically, one a card was
    /// handed and painted from — [`Sharp::Here`] — within the last
    /// [`FRESH`]. Without that guard, a card drawn sharp *this very frame*
    /// is exactly as much a candidate as one nobody has looked at in a
    /// minute, because pass one otherwise knows nothing about which sharp
    /// copies are in use right now. Take that card's sharp copy anyway and
    /// the next `look` finds `Sharp::Gone`, asks for it straight back, and
    /// `begin` accepts because nothing else has claimed it — a decode/evict
    /// treadmill that never lands, the moment the sharp working set outgrows
    /// the budget. A card that loses its sharp copy to a genuinely idle
    /// eviction instead falls back to its thumbnail, which is a picture that
    /// got softer rather than a card that went blank — and a thumbnail is a
    /// fortieth of the bytes, so this pass alone is enough on any board a
    /// screen can hold.
    ///
    /// **Pass two takes whole pictures**, and refuses to take one that was on
    /// screen within the last [`FRESH`]. That refusal is what stops the
    /// blinking: without it, a board holding more than the budget evicts
    /// something visible on every frame, decodes it again on the next, and
    /// walks that hole around the z-order forever. Where every candidate is
    /// fresh, this simply stops and the cache runs over budget.
    fn evict_down_to_budget(&mut self) {
        if self.held <= self.budget {
            return;
        }
        let now = Instant::now();

        // Pass one. A temporary, sorted candidate list of the mutable
        // references themselves — not of the hashes — so nothing here
        // allocates a string to do it.
        let mut candidates: Vec<(u64, &mut Ready)> = self
            .slots
            .values_mut()
            .filter_map(|slot| match slot {
                Slot::Ready(ready) if matches!(ready.sharp, Sharp::Here { .. }) => Some(ready),
                _ => None,
            })
            .map(|ready| (ready.last_used, ready))
            .collect();
        candidates.sort_unstable_by_key(|(stamp, _)| *stamp);

        for (_, ready) in candidates {
            if self.held <= self.budget {
                return;
            }
            let fresh = ready.sharp_seen.is_some_and(|seen| now.duration_since(seen) < FRESH);
            if fresh {
                continue;
            }
            if let Sharp::Here { image, cost } = std::mem::replace(&mut ready.sharp, Sharp::Gone) {
                self.held = self.held.saturating_sub(cost);
                self.dropped.push(image);
            }
        }

        // Pass two. Removing a whole entry needs an owned key — `remove`
        // cannot borrow one from the scan that found it and mutate the map
        // in the same breath — so this clones exactly the one hash it is
        // about to take, and only when it is actually taking it, rather than
        // the whole cache's worth up front.
        //
        // Never down to nothing: one picture larger than the whole budget
        // should be held anyway rather than decoded again every frame to be
        // thrown away again every frame.
        loop {
            if self.held <= self.budget {
                return;
            }
            let mut count = 0usize;
            let mut oldest: Option<(u64, &str)> = None;
            for (hash, slot) in &self.slots {
                let Slot::Ready(ready) = slot else { continue };
                count += 1;
                if oldest.is_none_or(|(stamp, _)| ready.last_used < stamp) {
                    oldest = Some((ready.last_used, hash.as_str()));
                }
            }
            if count <= 1 {
                return;
            }
            let Some((_, hash)) = oldest else { return };
            let hash = hash.to_string();

            let fresh = match self.slots.get(&hash) {
                Some(Slot::Ready(ready)) => now.duration_since(ready.seen) < FRESH,
                _ => false,
            };
            if fresh {
                // The oldest thing here is still on screen. Everything else
                // is newer, so there is nothing left to take.
                return;
            }
            if let Some(Slot::Ready(ready)) = self.slots.remove(&hash) {
                self.held = self.held.saturating_sub(ready.thumb_cost + sharp_cost(&ready.sharp));
                self.dropped.push(ready.thumb);
                if let Sharp::Here { image, .. } = ready.sharp {
                    self.dropped.push(image);
                }
            }
        }
    }
}

/// What the sharp copy costs, or nothing where there is not one.
fn sharp_cost(sharp: &Sharp) -> usize {
    match sharp {
        Sharp::Here { cost, .. } => *cost,
        _ => 0,
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
/// Returns `None` for anything that is not a picture this build can read, which
/// includes every video and audio file — those cards draw their `meta.cover`
/// instead, and the cover is a picture — and every mesh. A mesh's raster
/// depends on which way its camera is turned, which is a fact about the
/// *card*, not about its bytes, so meshes are decoded in `mesh_cache` instead
/// of here: this function stays a pure function of bytes alone, which every
/// other picture in this cache still is.
///
/// **A picture may be more than one frame.** A GIF, an APNG and an animated
/// WebP come back with every frame and the delay between each, because
/// `RenderImage` has carried both since before this build used it and
/// `paint_image` takes the index. That is the whole of what animation costs
/// here: no decoder, no dependency, and a card that was showing a still frame
/// of a GIF now shows the GIF.
pub fn decode(bytes: &[u8]) -> Option<Decoded> {
    // Asked first and by the bytes, the same rule every other classification in
    // this tree follows: `image::ImageReader` cannot guess a text format at
    // all, so this is not a shortcut past its sniffing, it is the only sniffing
    // there is going to be.
    if is_svg(bytes) {
        return svg(bytes);
    }
    let reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format().ok()?;
    // The catalogue is not trusted for this — the extension in the archive is
    // a hint for rebuilding a media type, not a promise about the contents.
    let format = reader.format();

    // Asked first, and cheaply refused: `moving` returns `None` the moment the
    // format is not one of the three that can animate, which is every
    // photograph on a normal board.
    //
    // An animation is held at one size rather than two. Its frames are already
    // capped tighter than a still is — see `ANIMATED_SIDE` — and holding a
    // second copy of eighty frames to sharpen something that is moving is the
    // one place two tiers would cost more than they save.
    if let Some(frames) = moving(bytes, format) {
        if frames.len() > 1 {
            return Some(Decoded { thumb: Arc::new(RenderImage::new(frames)), sharp: None });
        }
    }

    // Decoded once and resampled twice, which is why this is not two decodes:
    // the expensive half is turning JPEG into pixels, and both sizes come off
    // the same pixels.
    let decoded = reader.decode().ok()?;
    let thumb = one(&decoded, THUMB_SIDE);
    // No second copy where there would be nothing in it. A picture already
    // smaller than a thumbnail is its own sharp copy, and holding it twice
    // would double the cost of exactly the pictures that were cheap.
    let sharp =
        (decoded.width().max(decoded.height()) > THUMB_SIDE).then(|| one(&decoded, LONGEST_SIDE));
    Some(Decoded { thumb, sharp })
}

/// One copy of a picture at one size, in the layout the atlas wants.
fn one(decoded: &image::DynamicImage, longest_side: u32) -> Arc<RenderImage> {
    let mut rgba = shrink(decoded, longest_side);
    to_bgra(&mut rgba);
    Arc::new(RenderImage::new(vec![Frame::new(rgba)]))
}

/// Every frame of an animation, or `None` for a picture that does not move.
///
/// `None` and a one-frame answer mean different things and both are normal:
/// `None` is "this format cannot animate", one frame is "this file could have
/// and did not". The caller treats them the same, but a future one measuring
/// GIFs would want to tell them apart.
fn moving(bytes: &[u8], format: Option<image::ImageFormat>) -> Option<Vec<Frame>> {
    use image::codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder};
    use image::{AnimationDecoder, ImageFormat};

    let at = || std::io::Cursor::new(bytes);
    let frames = match format? {
        ImageFormat::Gif => GifDecoder::new(at()).ok()?.into_frames(),
        ImageFormat::WebP => {
            let decoder = WebPDecoder::new(at()).ok()?;
            decoder.has_animation().then(|| decoder.into_frames())?
        }
        ImageFormat::Png => {
            let decoder = PngDecoder::new(at()).ok()?;
            decoder.is_apng().ok()?.then(|| decoder.apng().ok())??.into_frames()
        }
        _ => return None,
    };

    let mut held: Vec<(RgbaImage, Duration)> = Vec::new();
    let mut cost = 0usize;
    // How many source frames each held frame stands for. Doubles whenever the
    // budget is reached; see `thin`.
    let mut stride = 1usize;
    let mut since_kept = 0usize;

    for (seen, frame) in frames.enumerate() {
        if seen >= ANIMATION_FRAMES_MAX {
            break;
        }
        // A frame that will not decode ends the animation rather than
        // discarding it: what came before it is a real, if short, loop, and a
        // truncated GIF is a thing that exists in the wild.
        let Ok(frame) = frame else { break };
        let delay = Duration::from(frame.delay());

        if since_kept + 1 < stride {
            since_kept += 1;
            // The time this frame would have been on screen for is not lost —
            // it is added to the frame that is standing in for it, so a
            // thinned animation still runs at the length it was authored at.
            if let Some((_, last)) = held.last_mut() {
                *last += delay;
            }
            continue;
        }
        since_kept = 0;

        let mut rgba = shrink(&image::DynamicImage::ImageRgba8(frame.into_buffer()), ANIMATED_SIDE);
        to_bgra(&mut rgba);
        cost += bytes_of(&rgba);
        held.push((rgba, delay));

        if cost > ANIMATION_BUDGET {
            cost = thin(&mut held);
            stride *= 2;
        }
    }

    (!held.is_empty()).then(|| {
        held.into_iter()
            .map(|(rgba, delay)| {
                Frame::from_parts(rgba, 0, 0, image::Delay::from_saturating_duration(delay))
            })
            .collect()
    })
}

// ---------------------------------------------------------------------------
// Vector
// ---------------------------------------------------------------------------

/// Whether these bytes are (probably) SVG.
///
/// `image::ImageReader` has no way to guess a text format at all — there is no
/// magic byte for XML — so this is the whole of the sniffing that decides
/// whether a card's asset is handed to `resvg` or to the raster path below.
/// Only the common case is covered: a document opening straight into `<?xml`
/// or `<svg`, past an optional UTF-8 BOM. A file that leads with a comment
/// before either still opens as `Source` today, which is the same "close but
/// not universal" trade the rest of this crate's sniffing already makes.
fn is_svg(bytes: &[u8]) -> bool {
    let mut b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    while let Some((&c, rest)) = b.split_first() {
        if !c.is_ascii_whitespace() {
            break;
        }
        b = rest;
    }
    b.starts_with(b"<?xml") || b.starts_with(b"<svg")
}

/// A process-wide font database, built once.
///
/// Loading every system font is not cheap, and unlike a decoded picture this
/// has nothing to do with any one asset's content hash — the same fonts serve
/// every SVG this session ever rasterises, so it is built at most once rather
/// than once per unique file. Most board SVGs are icons and logos with no
/// `<text>` at all, but the ones that do carry text want real fonts rather
/// than silently missing words.
fn fontdb() -> (Arc<resvg::usvg::fontdb::Database>, Option<String>) {
    static DB: std::sync::OnceLock<(Arc<resvg::usvg::fontdb::Database>, Option<String>)> =
        std::sync::OnceLock::new();
    DB.get_or_init(|| {
        let mut db = resvg::usvg::fontdb::Database::new();
        db.load_system_fonts();
        let default_family = set_generic_families(&mut db);
        (Arc::new(db), default_family)
    })
    .clone()
}

/// Point `fontdb`'s generic families at whatever is actually installed, and
/// hand back a serif (or, failing that, sans-serif) family name to use as the
/// document default.
///
/// `fontdb`'s own generic defaults are Windows font names — serif is "Times
/// New Roman", sans-serif is "Arial" — and `load_system_fonts` only loads
/// what is actually on the machine; unlike `fontconfig`, it does not alias one
/// to the other. Left alone, a Linux box that has never heard of "Times New
/// Roman" resolves nothing for a `<text>` element that does not name its own
/// font, and the words are silently absent rather than merely in the wrong
/// face. This is a heuristic — matching on a loaded face's own family name —
/// rather than a real font-matching engine, but it is what turns "no font
/// found" into "some reasonable font found" on every platform this build
/// ships for, `fontconfig` or none.
fn set_generic_families(db: &mut resvg::usvg::fontdb::Database) -> Option<String> {
    let mut serif = None;
    let mut sans = None;
    let mut mono = None;
    for face in db.faces() {
        let Some((name, _)) = face.families.first() else { continue };
        let lower = name.to_ascii_lowercase();
        if face.monospaced && mono.is_none() {
            mono = Some(name.clone());
        } else if lower.contains("sans") && sans.is_none() {
            sans = Some(name.clone());
        } else if lower.contains("serif") && !lower.contains("sans") && serif.is_none() {
            serif = Some(name.clone());
        }
        if serif.is_some() && sans.is_some() && mono.is_some() {
            break;
        }
    }
    // Cursive and fantasy have no naming convention reliable enough to search
    // for, so they are left at `fontdb`'s own guess.
    if let Some(name) = &serif {
        db.set_serif_family(name.clone());
    }
    if let Some(name) = &sans {
        db.set_sans_serif_family(name.clone());
    }
    if let Some(name) = mono {
        db.set_monospace_family(name);
    }
    serif.or(sans)
}

/// Rasterise an SVG at both tiers.
///
/// Unlike a raster picture, a vector has no native resolution to be short of —
/// `resvg` re-renders the whole document from its paths at whatever size is
/// asked for, so both tiers are rendered directly from the one parsed tree
/// rather than one being resampled from the other the way [`shrink`] resamples
/// a decoded raster. There is no "already smaller than a thumbnail" case to
/// skip a second copy for, either: rendering a vector twice costs the same
/// either way its target size compares to the document's own.
fn svg(bytes: &[u8]) -> Option<Decoded> {
    let mut opt = resvg::usvg::Options::default();
    let (fontdb, default_family) = fontdb();
    opt.fontdb = fontdb;
    if let Some(family) = default_family {
        opt.font_family = family;
    }
    let tree = resvg::usvg::Tree::from_data(bytes, &opt).ok()?;

    let size = tree.size();
    let longest = size.width().max(size.height());
    if longest.is_nan() || longest <= 0.0 {
        return None;
    }
    let thumb = one_svg(&tree, longest, THUMB_SIDE)?;
    let sharp = one_svg(&tree, longest, LONGEST_SIDE)?;
    Some(Decoded { thumb, sharp: Some(sharp) })
}

/// One rendering of a parsed tree, scaled so its longest side lands on
/// `target`.
fn one_svg(tree: &resvg::usvg::Tree, longest: f32, target: u32) -> Option<Arc<RenderImage>> {
    let scale = target as f32 / longest;
    let size = tree.size();
    let w = ((size.width() * scale).round() as u32).max(1);
    let h = ((size.height() * scale).round() as u32).max(1);

    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
    resvg::render(
        tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    let mut rgba = straighten(pixmap.take(), w, h);
    to_bgra(&mut rgba);
    Some(Arc::new(RenderImage::new(vec![Frame::new(rgba)])))
}

/// `tiny_skia` hands back premultiplied RGBA; every raster decoder in this
/// crate hands back straight alpha, which is the convention [`to_bgra`] and
/// the atlas beyond it are written against. Converting once here is cheaper
/// than teaching the rest of the pipeline a second convention for one source.
fn straighten(mut data: Vec<u8>, w: u32, h: u32) -> RgbaImage {
    for px in data.as_chunks_mut::<4>().0 {
        let a = u32::from(px[3]);
        if a != 0 && a != 255 {
            px[0] = ((u32::from(px[0]) * 255) / a) as u8;
            px[1] = ((u32::from(px[1]) * 255) / a) as u8;
            px[2] = ((u32::from(px[2]) * 255) / a) as u8;
        }
    }
    RgbaImage::from_raw(w, h, data).expect("a pixmap's own dimensions fit its own buffer")
}

/// Halve an animation in place, keeping every other frame and giving each of
/// them the time the frame it replaced would have had.
///
/// This is what "thinned rather than cut" means. A twelve-second loop that will
/// not fit becomes a twelve-second loop at half the frame rate, which on a
/// moodboard is very nearly the same thing to look at — where the first four
/// seconds of it would read as something broken.
///
/// Returns what the kept frames now cost.
fn thin(held: &mut Vec<(RgbaImage, Duration)>) -> usize {
    let mut kept: Vec<(RgbaImage, Duration)> = Vec::with_capacity(held.len().div_ceil(2));
    for (i, (rgba, delay)) in held.drain(..).enumerate() {
        match i % 2 == 0 {
            true => kept.push((rgba, delay)),
            // The dropped frame's time goes to the one before it, so the loop
            // keeps its length.
            false => {
                if let Some((_, last)) = kept.last_mut() {
                    *last += delay;
                }
            }
        }
    }
    let cost = kept.iter().map(|(rgba, _)| bytes_of(rgba)).sum();
    *held = kept;
    cost
}

/// `RenderImage` wants BGRA and `image` produces RGBA, and this is the whole of
/// the difference: swap the two ends of each pixel in place.
///
/// Done here rather than at paint time so it is paid once, off the main thread,
/// instead of on every upload.
pub(crate) fn to_bgra(rgba: &mut RgbaImage) {
    for pixel in rgba.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
}

fn bytes_of(rgba: &RgbaImage) -> usize {
    rgba.width() as usize * rgba.height() as usize * 4
}

/// Hold a picture to `longest_side`, keeping its shape.
///
/// By reference rather than by value, because it is called twice on the same
/// decode — once for each tier — and `image`'s resize does not consume its
/// argument either.
fn shrink(decoded: &image::DynamicImage, longest_side: u32) -> RgbaImage {
    let (w, h) = (decoded.width(), decoded.height());
    let longest = w.max(h);
    if longest <= longest_side || longest == 0 {
        return decoded.to_rgba8();
    }
    let scale = longest_side as f32 / longest as f32;
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

    /// The longest side of a decoded picture, in pixels.
    fn longest(image: &Arc<RenderImage>) -> i32 {
        let size = image.size(0);
        size.width.0.max(size.height.0)
    }

    /// A tiny SVG: a red square on a transparent field, wide rather than
    /// square, so the two axes are told apart in a test.
    fn svg_bytes(w: u32, h: u32) -> Vec<u8> {
        format!(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}">
                <rect width="{w}" height="{h}" fill="#ff0000"/>
            </svg>"##
        )
        .into_bytes()
    }

    #[test]
    fn svg_bytes_are_recognised_with_or_without_a_prolog() {
        assert!(is_svg(b"<svg width=\"1\" height=\"1\"/>"));
        assert!(is_svg(b"<?xml version=\"1.0\"?><svg/>"));
        assert!(is_svg(b"  \n\t<svg/>"), "leading whitespace should not matter");
        assert!(is_svg(b"\xEF\xBB\xBF<svg/>"), "nor a UTF-8 BOM");
        assert!(!is_svg(&png(4, 4)), "a real PNG must not be mistaken for one");
        assert!(!is_svg(b"just some words"));
    }

    #[test]
    fn an_svg_rasterises_to_both_tiers_at_the_right_proportions() {
        // Twice as wide as it is tall, so width rather than height is the
        // longest side both tiers are scaled to.
        let decoded = decode(&svg_bytes(200, 100)).expect("that is an svg");
        let thumb = decoded.thumb.size(0);
        assert_eq!(thumb.width.0, THUMB_SIDE as i32);
        assert_eq!(thumb.height.0, (THUMB_SIDE / 2) as i32);

        let sharp = decoded.sharp.expect("a vector always gets a second tier").size(0);
        assert_eq!(sharp.width.0, LONGEST_SIDE as i32);
        assert_eq!(sharp.height.0, (LONGEST_SIDE / 2) as i32);
    }

    #[test]
    fn an_svgs_fill_colour_survives_un_premultiplying_and_the_channel_swap() {
        let decoded = decode(&svg_bytes(64, 64)).expect("that is an svg");
        let size = decoded.thumb.size(0);
        let bytes = decoded.thumb.as_bytes(0).expect("one frame");
        let (x, y) = (size.width.0 as usize / 2, size.height.0 as usize / 2);
        let at = (y * size.width.0 as usize + x) * 4;
        // BGRA: opaque red is (0, 0, 255, 255) once the channels are swapped.
        assert_eq!(&bytes[at..at + 4], [0, 0, 255, 255], "got {:?}", &bytes[at..at + 4]);
    }

    #[test]
    fn something_that_starts_like_svg_and_is_not_fails_quietly() {
        assert!(decode(b"<svg this is not actually valid xml").is_none());
    }

    #[test]
    fn text_with_no_font_family_still_draws_something() {
        // `usvg`'s own default is the literal string "Times New Roman", which
        // resolves to nothing at all on a Linux box that has never heard of
        // it — the fix in `set_generic_families` is what makes this test's
        // plain, font-family-less `<text>` paint any foreground pixels at all
        // rather than leaving the field solid background.
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="40">
            <rect width="100" height="40" fill="#000000"/>
            <text x="5" y="28" font-size="24" fill="#ffffff">Hi</text>
        </svg>"##;
        let decoded = decode(svg).expect("svg decodes");
        let size = decoded.thumb.size(0);
        let bytes = decoded.thumb.as_bytes(0).expect("one frame");
        let lit = bytes
            .as_chunks::<4>()
            .0
            .iter()
            .take((size.width.0 * size.height.0) as usize)
            .any(|px| px[0] > 40 || px[1] > 40 || px[2] > 40);
        assert!(lit, "no pixel brighter than the black background — the text never painted");
    }

    // STL/OBJ/GLB rasterisation used to be tested here; `decode` no longer
    // touches meshes at all (see `mesh_cache`, where those three tests moved
    // along with the fixtures they used).

    #[test]
    fn a_small_picture_is_held_once_and_at_the_size_it_was() {
        // Under the thumbnail cap, so the thumbnail *is* the picture and a
        // second copy would be the same pixels at twice the cost.
        let image = decode(&png(64, 32)).expect("that is a png");
        let size = image.thumb.size(0);
        assert_eq!((size.width.0, size.height.0), (64, 32));
        assert!(image.sharp.is_none(), "a picture smaller than a thumbnail was held twice");
    }

    #[test]
    fn a_large_picture_is_held_at_both_sizes() {
        // The whole of what makes zooming out cheap: one decode, two copies,
        // and the card picks between them by how large it is on screen.
        let image = decode(&png(300, 150)).expect("that is a png");
        assert_eq!(longest(&image.thumb), THUMB_SIDE as i32);
        let sharp = image.sharp.expect("a picture larger than a thumbnail needs a sharp copy");
        assert_eq!(longest(&sharp), 300, "the sharp copy was resampled for no reason");
    }

    #[test]
    fn the_colours_come_back_the_other_way_round() {
        // The atlas wants BGRA. The pixel written was r=10, g=20, b=30, so the
        // bytes held should start with 30 and end with 10.
        let image = decode(&png(2, 2)).expect("that is a png");
        let bytes = image.thumb.as_bytes(0).expect("frame zero");
        assert_eq!(&bytes[..4], &[30, 20, 10, 255]);
    }

    #[test]
    fn a_picture_far_too_large_is_brought_down_to_the_cap() {
        // `shrink` rather than `decode`, because encoding a four-megapixel PNG
        // to test the resize in it is twenty seconds of testing the encoder.
        let huge =
            image::DynamicImage::ImageRgba8(RgbaImage::new(LONGEST_SIDE * 2, LONGEST_SIDE / 2));
        let out = shrink(&huge, LONGEST_SIDE);
        assert_eq!(out.width(), LONGEST_SIDE);
        // And its shape survived the trip.
        assert_eq!(out.height(), LONGEST_SIDE / 4);
    }

    #[test]
    fn a_picture_already_small_enough_is_left_alone() {
        let small = image::DynamicImage::ImageRgba8(RgbaImage::new(300, 200));
        let out = shrink(&small, LONGEST_SIDE);
        assert_eq!((out.width(), out.height()), (300, 200));
    }

    /// A real animated GIF, encoded here so the test needs no fixture on disk.
    ///
    /// A hundred milliseconds a frame because GIF stores its timing in
    /// hundredths of a second — anything finer would be testing the rounding
    /// rather than the decode.
    fn gif(frames: u32, w: u32, h: u32) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        {
            let mut encoder = image::codecs::gif::GifEncoder::new(&mut out);
            encoder.set_repeat(image::codecs::gif::Repeat::Infinite).unwrap();
            for i in 0..frames {
                let shade = (i * 40 % 200) as u8;
                let buffer = RgbaImage::from_pixel(w, h, image::Rgba([shade, 40, 90, 255]));
                encoder
                    .encode_frame(Frame::from_parts(
                        buffer,
                        0,
                        0,
                        image::Delay::from_saturating_duration(Duration::from_millis(100)),
                    ))
                    .unwrap();
            }
        }
        out.into_inner()
    }

    #[test]
    fn an_animation_comes_back_with_every_frame_and_the_time_between_them() {
        let image = decode(&gif(6, 32, 24)).expect("that is a gif");
        assert_eq!(image.thumb.frame_count(), 6);
        for i in 0..image.thumb.frame_count() {
            let delay = Duration::from(image.thumb.delay(i));
            assert!((delay.as_millis() as i64 - 100).abs() <= 10, "frame {i} claimed {delay:?}");
        }
    }

    #[test]
    fn an_animation_is_held_once_however_large_it_is() {
        // Eighty frames of a second copy is the one place two tiers would cost
        // more than they save. See `decode`.
        let image = decode(&gif(4, 400, 400)).expect("that is a gif");
        assert!(image.sharp.is_none(), "an animation was held at two sizes");
    }

    #[test]
    fn a_still_picture_is_still_exactly_one_frame() {
        // The regression that matters in the other direction: every photograph
        // on a normal board goes down the animation path first now, and must
        // come out of it unchanged.
        let image = decode(&png(64, 32)).expect("that is a png");
        assert_eq!(image.thumb.frame_count(), 1);
        let size = image.thumb.size(0);
        assert_eq!((size.width.0, size.height.0), (64, 32));
    }

    #[test]
    fn a_gif_of_one_frame_is_a_picture_rather_than_an_animation() {
        let image = decode(&gif(1, 16, 16)).expect("that is a gif");
        assert_eq!(image.thumb.frame_count(), 1);
    }

    #[test]
    fn thinning_halves_the_frames_and_keeps_the_length() {
        let mut held: Vec<(RgbaImage, Duration)> =
            (0..8).map(|_| (RgbaImage::new(4, 4), Duration::from_millis(100))).collect();
        let before: Duration = held.iter().map(|(_, d)| *d).sum();

        let cost = thin(&mut held);

        assert_eq!(held.len(), 4, "it should have halved");
        let after: Duration = held.iter().map(|(_, d)| *d).sum();
        assert_eq!(before, after, "the loop changed length");
        assert_eq!(cost, 4 * 4 * 4 * 4, "the reported cost is of what is left");
    }

    #[test]
    fn thinning_an_odd_number_of_frames_still_keeps_the_length() {
        let mut held: Vec<(RgbaImage, Duration)> =
            (0..5).map(|_| (RgbaImage::new(2, 2), Duration::from_millis(60))).collect();
        let before: Duration = held.iter().map(|(_, d)| *d).sum();
        thin(&mut held);
        assert_eq!(held.len(), 3);
        assert_eq!(held.iter().map(|(_, d)| *d).sum::<Duration>(), before);
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

    /// A picture held at one size, like a small still or an animation.
    fn thumb_only(side: u32) -> Decoded {
        Decoded { thumb: pixels(side), sharp: None }
    }

    /// A picture held at both sizes, distinguishable by which one comes back.
    fn both(thumb: u32, sharp: u32) -> Decoded {
        Decoded { thumb: pixels(thumb), sharp: Some(pixels(sharp)) }
    }

    /// A cache with room for about four of `pixels(32)`.
    fn small_cache() -> Images {
        Images { budget: 32 * 32 * 4 * 4, ..Default::default() }
    }

    /// Push everything held far enough into the past that eviction is allowed
    /// to consider it at all. See `FRESH`.
    ///
    /// Every eviction test needs this, because a picture that has just settled
    /// counts as on screen — which is the point of the rule and would otherwise
    /// make all of them assert that nothing happens.
    fn age(images: &mut Images) {
        let then = Instant::now() - FRESH * 2;
        for slot in images.slots.values_mut() {
            if let Slot::Ready(ready) = slot {
                ready.seen = then;
            }
        }
    }

    #[test]
    fn asking_twice_only_starts_one_decode() {
        let mut images = Images::default();
        assert!(matches!(images.look("a", 0.0), Load::Cold));
        assert!(images.begin("a"));
        assert!(!images.begin("a"), "a second decode was started");
        assert!(matches!(images.look("a", 0.0), Load::Waiting));
    }

    #[test]
    fn a_picture_arrives_rather_than_appearing() {
        let mut images = Images::default();
        images.begin("a");
        images.settle("a", Some(thumb_only(32)));
        let Load::Ready(_, arrived) = images.look("a", 0.0) else {
            panic!("it should be ready");
        };
        // On the frame it lands it is barely there, which is the whole
        // difference between a decode arriving and a decode being swapped in.
        assert!(arrived < 0.5, "it turned up fully formed, at {arrived}");
        assert!(images.arriving(), "the frame clock was not told to keep going");
    }

    #[test]
    fn a_picture_that_has_been_ticked_a_while_is_not_still_arriving() {
        let mut images = Images::default();
        images.begin("a");
        images.settle("a", Some(thumb_only(32)));
        // Ticked twice the arrival window rather than waited for: a test that
        // slept for a fifth of a second to watch a fade would be a fifth of a
        // second every run, forever, to assert arithmetic.
        images.tick(ARRIVING.as_secs_f32() * 2.0);
        let Load::Ready(_, arrived) = images.look("a", 0.0) else {
            panic!("it should be ready");
        };
        assert_eq!(arrived, 1.0);
        assert!(!images.arriving(), "it never stopped asking for frames");
    }

    #[test]
    fn reduced_motion_lands_an_arrival_in_a_single_tick() {
        // The bug this replaced: the fade used to read `Instant::elapsed`, so
        // reduced motion's one enormous `dt` — which never touches a clock —
        // could not land it instantly the way every other animation on the
        // board does. Ticking once with a `dt` far larger than `ARRIVING`
        // is exactly what `BoardView::advance` does under reduced motion, and
        // it has to be enough on its own, in one call.
        let mut images = Images::default();
        images.begin("a");
        images.settle("a", Some(thumb_only(32)));
        images.tick(10.0);
        let Load::Ready(_, arrived) = images.look("a", 0.0) else {
            panic!("it should be ready");
        };
        assert_eq!(arrived, 1.0);
        assert!(!images.arriving());
    }

    #[test]
    fn a_file_that_is_not_a_picture_is_not_tried_again() {
        let mut images = Images::default();
        images.begin("bad");
        images.settle("bad", None);
        assert!(matches!(images.look("bad", 0.0), Load::Failed));
        assert!(!images.begin("bad"), "it went back for another go");
    }

    #[test]
    fn a_small_card_is_drawn_from_the_thumbnail_and_a_large_one_is_not() {
        // The two copies exist to be told apart by the size on screen, and
        // nothing else in the app can check which one it got.
        let mut images = Images::default();
        images.begin("a");
        images.settle("a", Some(both(32, 512)));

        let Load::Ready(small, _) = images.look("a", 10.0) else { panic!("ready") };
        assert_eq!(longest(&small), 32, "a card the size of a stamp took the sharp copy");

        let Load::Ready(large, _) = images.look("a", THUMB_SIDE as f32 * 2.0) else {
            panic!("ready")
        };
        assert_eq!(longest(&large), 512, "a card filling the screen took the thumbnail");
    }

    #[test]
    fn the_sharp_copy_is_what_goes_first_and_the_card_never_goes_blank() {
        // The bug this whole two-tier arrangement exists for: a board with more
        // pictures on screen than the budget holds used to lose whole pictures
        // and decode them again next frame, which looked like cards blinking.
        let mut images = small_cache();
        images.begin("a");
        images.settle("a", Some(both(32, 128)));

        assert!(images.held <= images.budget, "the sharp copy should have gone");
        let Load::Ready(image, _) = images.look("a", 1024.0) else {
            panic!("a card that lost its sharp copy went blank");
        };
        assert_eq!(longest(&image), 32, "it should have fallen back to the thumbnail");
    }

    #[test]
    fn a_card_that_wants_its_sharp_copy_back_asks_once() {
        let mut images = small_cache();
        images.begin("a");
        images.settle("a", Some(both(32, 128)));

        // Two frames in a row of the same zoomed-in card. One decode.
        images.look("a", 1024.0);
        images.look("a", 1024.0);
        assert_eq!(images.resharpen(), vec!["a".to_string()]);
        assert!(images.resharpen().is_empty(), "the same request came back twice");
        assert!(images.begin("a"), "the decode it asked for was refused");
    }

    #[test]
    fn a_resharpened_copy_survives_the_settle_that_delivers_it() {
        // The treadmill pass one used to be able to fall into: a card wants
        // a sharp copy back, the decode lands, and the very `settle` that
        // delivers it also runs eviction — which, without the fix, cannot
        // tell that sharp copy apart from one nobody has drawn in a minute,
        // strips it straight back off, and sends the next `look` right back
        // to asking for it. This never terminates on a board where the
        // sharp working set outgrows the budget: every settle both delivers
        // and immediately evicts.
        let mut images = small_cache();
        images.begin("a");
        images.settle("a", Some(both(32, 128)));
        images.begin("b");
        images.settle("b", Some(both(32, 128)));
        // Both sharp copies already went in pass one above — small_cache
        // holds about four thumbnails, nowhere near two thumbnails and two
        // sharp copies.
        age(&mut images);

        // "a"'s card is drawn large again. Gone -> Coming, thumbnail stands
        // in, one resharpen request.
        images.look("a", 1024.0);
        assert_eq!(images.resharpen(), vec!["a".to_string()]);
        assert!(images.begin("a"), "the decode it asked for was refused");

        // The decode lands. This settle's own eviction pass must not be the
        // thing that takes the copy it just delivered.
        images.settle("a", Some(both(32, 128)));
        assert!(images.held > images.budget, "set up wrong: nothing was under pressure");

        let Load::Ready(image, _) = images.look("a", 1024.0) else {
            panic!("a card that just resharpened went blank");
        };
        assert_eq!(longest(&image), 128, "the resharpened copy was evicted before it was drawn");
    }

    #[test]
    fn a_second_decode_of_the_same_picture_is_not_counted_twice() {
        // What `settle` calls `let_go_of` for. Missing it is a `held` that only
        // grows, which reads as a cache that has stopped evicting.
        let mut images = Images::default();
        images.begin("a");
        images.settle("a", Some(both(32, 128)));
        let once = images.held;
        images.settle("a", Some(both(32, 128)));
        assert_eq!(images.held, once);
        assert_eq!(images.ready_count(), 1, "it was listed twice");
    }

    #[test]
    fn the_oldest_picture_falls_out_when_there_is_no_room() {
        let mut images = small_cache();
        for name in ["a", "b", "c", "d"] {
            images.begin(name);
            images.settle(name, Some(thumb_only(32)));
        }
        age(&mut images);
        images.begin("e");
        images.settle("e", Some(thumb_only(32)));

        assert!(images.held <= images.budget, "held {} over {}", images.held, images.budget);
        assert_eq!(images.ready_count(), 4);
        assert!(matches!(images.look("a", 0.0), Load::Cold), "the oldest should be gone");
        assert!(matches!(images.look("e", 0.0), Load::Ready(..)), "the newest should be here");
        assert_eq!(images.dropped.len(), 1, "its tile is queued for the window");
    }

    #[test]
    fn nothing_that_was_on_screen_this_frame_is_evicted() {
        // The anti-thrash rule, and the reason the cache is allowed over its
        // budget. Without it every one of these is a candidate — they are all
        // on screen — and the front of the queue gets taken every frame.
        let mut images = small_cache();
        for name in ["a", "b", "c", "d", "e", "f"] {
            images.begin(name);
            images.settle(name, Some(thumb_only(32)));
        }
        assert_eq!(images.ready_count(), 6, "something on screen was thrown away");
        assert!(images.held > images.budget, "it should have gone over rather than blink");
    }

    #[test]
    fn looking_at_a_picture_moves_it_off_the_chopping_block() {
        let mut images = small_cache();
        for name in ["a", "b", "c", "d"] {
            images.begin(name);
            images.settle(name, Some(thumb_only(32)));
        }
        // "a" is the oldest — until it is looked at.
        assert!(matches!(images.look("a", 0.0), Load::Ready(..)));
        // Aged *after* the look, so the only thing keeping "a" is its place in
        // the queue rather than the freshness rule the test above covers.
        age(&mut images);
        images.begin("e");
        images.settle("e", Some(thumb_only(32)));
        assert!(matches!(images.look("a", 0.0), Load::Ready(..)), "the looked-at one went");
        assert!(matches!(images.look("b", 0.0), Load::Cold), "the untouched one stayed");
    }

    #[test]
    fn one_picture_larger_than_the_whole_budget_is_kept_anyway() {
        // Otherwise a board holding one enormous photograph decodes it every
        // frame, to throw it away every frame, forever.
        let mut images = small_cache();
        images.begin("huge");
        images.settle("huge", Some(thumb_only(256)));
        age(&mut images);
        images.begin("also");
        images.settle("also", Some(thumb_only(256)));
        assert!(images.held > images.budget);
        assert!(matches!(images.look("huge", 0.0), Load::Cold), "the oldest still goes");
        assert!(matches!(images.look("also", 0.0), Load::Ready(..)), "the last one stays");
    }
}
