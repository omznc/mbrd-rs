//! Where the camera is going, and how it gets there.
//!
//! [`Viewport`] is where the camera *is* — one pan, one zoom, and the four
//! conversions everything else in the app measures against. It stays exactly
//! that. This module is the layer above it: the springs that move it, the
//! momentum a flick leaves behind, and the resistance at the ends of the zoom
//! range. Every frame it writes a pan and a zoom into the viewport, and
//! everything downstream — hit-testing, culling, the grid — goes on reading
//! the viewport and never knows the difference.
//!
//! ## One invariant, and it is the whole design
//!
//! **A camera that is not moving agrees with the viewport.** [`Camera::step`]
//! parks itself on the live viewport whenever it has nothing to do, which is
//! what lets a drag keep writing `viewport.pan` directly the way it always
//! has: the next frame absorbs it, and any spring that starts afterwards
//! starts from the truth rather than from wherever the camera was last
//! interested. Without that rule every direct write would need a matching
//! `camera.park`, and the one somebody forgot would be a camera that snapped
//! back to a stale position the next time anything was animated.
//!
//! The corollary is that starting a gesture has to stop the camera — see
//! [`Camera::seize`], which is called on mouse-down. That is not an
//! optimisation, it is the interruption rule: a board still sliding from a
//! flick must come to hand the instant it is grabbed, at the position it is
//! actually drawn at, rather than finishing its slide first.
//!
//! ## Zoom is sprung in log space
//!
//! Zoom is multiplicative — the step from 10% to 20% is the same *gesture* as
//! the step from 250% to 500%, and one wheel notch means a factor rather than
//! an amount. A spring run on the raw scale spends almost all of its travel at
//! the far end and reads as accelerating into your face. So the spring runs on
//! `ln(zoom)`, where a notch is a constant distance, and the exponential comes
//! back out at the end.

use std::collections::VecDeque;
use std::time::Instant;

use mbrd_core::geometry::{point, Point as WorldPoint};
use mbrd_core::motion::{self, Spring, Sprung};
use mbrd_core::viewport::{Viewport, MAX_ZOOM, MIN_ZOOM};

/// How near the target counts as arrived, for `ln(zoom)`.
///
/// A twentieth of a percent of scale. Below anything a pixel can show.
const SCALE_REST: f32 = 0.0005;

/// How near counts as arrived for a pan, in *screen* pixels.
///
/// Converted to world units against the live zoom inside [`Camera::step`],
/// because a quarter of a pixel is a quarter of a world unit at 100% and
/// two and a half of them at 10% — and the thing that has to be invisible is
/// the pixel, not the world unit.
const PAN_REST: f32 = 0.25;

/// The furthest past a zoom limit the board will ever be pushed, in log units.
///
/// `0.3` is a factor of about 1.35, so leaning on the wheel at 500% strains to
/// something like 675% and comes back. Enough to feel like give; not so much
/// that the board appears to have accepted the zoom.
const SLACK: f32 = 0.3;

/// How quickly the strain comes out once the wheel stops, as a time constant.
///
/// Short. This is not a journey, it is the board relaxing — anything slower
/// reads as the zoom limit having been a suggestion.
const SLACK_EASE: f32 = 0.12;

/// The longest a frame is allowed to count as, in seconds.
///
/// A window dragged between monitors, a modal loop, or a machine that went to
/// sleep can hand the next frame an arbitrarily long gap. The springs survive
/// it — that is what the closed form in [`mbrd_core::motion`] is for — but a
/// camera that teleports a second's worth of travel in one frame is not
/// animation, so a very long frame is treated as a slow one instead.
///
/// This is only for that pathology — a window dragged off to sleep, a modal
/// loop, a monitor drag — and not for anything a working board does in the
/// course of an afternoon. It used to sit at `1/15`, which also caught a
/// perfectly routine 200ms hitch (a big picture landing mid-decode) and
/// rewound the camera's clock to make it look like the flick in flight had
/// only travelled a fifteenth of a second, roughly 130ms behind the wall
/// clock it was actually released on. The closed-form spring does not need
/// that protection — a 350ms step is exercised in `motion`'s own tests and
/// stays finite — so the clamp only has to be short enough to stop a frame
/// from reading as a teleport, not short enough to catch a hitch a decode
/// leaves behind.
const LONGEST_FRAME: f32 = 0.25;

/// The camera's motion: three springs, a strain, and what it is aiming at.
pub struct Camera {
    /// The world point under the middle of the view, as two independent
    /// springs.
    ///
    /// Two rather than one spring on the distance, which matters whenever the
    /// two axes are moving at different speeds: a single spring on a 2D
    /// distance couples them, and a flick that is mostly sideways arrives
    /// diagonally.
    x: Sprung,
    y: Sprung,
    /// `ln(zoom)`. See the module note on why it is not the zoom itself.
    scale: Sprung,
    /// Which spring the pan is using. A throw and an errand want different
    /// feels, and the difference is decided by whoever set the target.
    pan_spring: Spring,
    /// How far past a zoom limit the wheel has pushed, in log units, before
    /// the rubber band has its say. Decays to nothing on its own.
    slack: f32,
    /// A world point and the screen pixel it is pinned to, while a zoom runs.
    ///
    /// This is what makes zoom-to-cursor survive being animated. Holding the
    /// point still only at the *ends* of the motion lets the board slide out
    /// from under the pointer in between, which is the tell that a zoom was
    /// animated by something that did not think about where you were aiming.
    /// While this is set the pan is derived from it every frame rather than
    /// sprung, and any deliberate pan clears it.
    ///
    /// Only ever taken while the pan is at rest — see [`Camera::zoom_by`] —
    /// because deriving the pan from the pivot means *not* stepping its
    /// spring, and a pan already in flight to a `travel_to` or a flick has
    /// somewhere to be that a zoom did not ask it to give up.
    pivot: Option<(WorldPoint, ScreenPoint)>,
    /// When the last frame was, so that a step knows how long it is for.
    last: Option<Instant>,
}

/// A point in canvas pixels, which is what a pointer position is.
type ScreenPoint = (f32, f32);

impl Camera {
    pub fn new(vp: &Viewport) -> Self {
        Self {
            x: Sprung::at(vp.pan.x),
            y: Sprung::at(vp.pan.y),
            scale: Sprung::at(vp.zoom.max(MIN_ZOOM).ln()),
            pan_spring: Spring::CAMERA,
            slack: 0.0,
            pivot: None,
            last: None,
        }
    }

    /// Whether anything is still in flight.
    pub fn moving(&self) -> bool {
        self.x.moving() || self.y.moving() || self.scale.moving() || self.slack != 0.0
    }

    /// Where the camera will end up, once it has stopped.
    ///
    /// **What a save writes down.** A camera caught mid-flight is not anywhere
    /// anybody chose to be, and a board saved during a flick would reopen
    /// halfway through somebody's gesture. It is also the only value promised
    /// to be inside the zoom range, since the live one may be out on the
    /// rubber band.
    pub fn resting(&self) -> (WorldPoint, f32) {
        (point(self.x.target(), self.y.target()), self.scale.target().exp())
    }

    /// Forget everything and sit exactly on this viewport.
    ///
    /// For arriving rather than travelling: opening a board is not a move
    /// across a space somebody is looking at, so there is nothing to show them
    /// about how the two views relate.
    pub fn park(&mut self, vp: &Viewport) {
        self.x.park(vp.pan.x);
        self.y.park(vp.pan.y);
        self.scale.park(vp.zoom.clamp(MIN_ZOOM, MAX_ZOOM).ln());
        self.slack = 0.0;
        self.pivot = None;
    }

    /// Stop where you are, right now.
    ///
    /// What a press does. The board is grabbed at the pixel it is drawn at,
    /// mid-flight or not — which is the difference between an interface you
    /// can interrupt and one that has to be waited out.
    pub fn seize(&mut self, vp: &Viewport) {
        if self.moving() {
            self.park(vp);
        }
    }

    /// Go to another viewport's pan and zoom, springing there.
    ///
    /// Takes a whole [`Viewport`] rather than a pan and a zoom so that the
    /// arithmetic for *which* viewport stays in `core` — `fit` and `home` are
    /// already written there and are what the file format means by a view.
    pub fn travel_to(&mut self, want: &Viewport) {
        self.pivot = None;
        self.pan_spring = Spring::CAMERA;
        self.x.retarget(want.pan.x);
        self.y.retarget(want.pan.y);
        self.scale.retarget(want.zoom.clamp(MIN_ZOOM, MAX_ZOOM).ln());
    }

    /// Carry on from a released drag at the speed it was going.
    ///
    /// `from` is where the camera is now and the velocity is in world units per
    /// second — the pan's own units, so that the projection below is a distance
    /// in the same space as the thing being projected.
    ///
    /// Uses [`Spring::THROW`], not [`Spring::CAMERA`], and the two are not
    /// interchangeable here the way they might look: `THROW` is specifically
    /// the spring whose response is matched to `motion::DECELERATION`, so that
    /// the curve `project` used to pick the landing point is the same curve
    /// the spring actually draws on the way there. A mismatched spring hits
    /// this exact landing point too, just not smoothly — see `Spring::THROW`'s
    /// doc for what that costs.
    pub fn fling(&mut self, from: WorldPoint, vx: f32, vy: f32) {
        self.pivot = None;
        self.pan_spring = Spring::THROW;
        self.x.fling(from.x + motion::project(vx, motion::DECELERATION), vx);
        self.y.fling(from.y + motion::project(vy, motion::DECELERATION), vy);
    }

    /// Slide the camera by a screen-space delta, smoothly.
    ///
    /// For the wheel, which arrives in notches. The target moves by the whole
    /// delta and the spring covers the gap, so a mouse with one detent pans as
    /// continuously as a trackpad does.
    pub fn nudge(&mut self, dx: f32, dy: f32, vp: &Viewport) {
        self.pivot = None;
        self.pan_spring = Spring::ZOOM;
        self.x.retarget(self.x.target() - dx / vp.zoom);
        self.y.retarget(self.y.target() + dy / vp.zoom);
    }

    /// Zoom by a factor about a screen point, smoothly, with give at the ends.
    ///
    /// The world point currently under `at` is read *now* and pinned, so the
    /// thing being aimed at is the thing that holds still for the whole of the
    /// animation and not merely at the end of it.
    ///
    /// That pin is only taken when the pan is at rest. `step`'s pivot branch
    /// derives the pan from the pivot every frame instead of stepping its
    /// spring, which is exactly right when the pan had nowhere else to be —
    /// but a pan mid-flight to a `travel_to` destination or still carrying a
    /// flick *does* have somewhere else to be, and deriving it from a wheel
    /// notch would silently overwrite that flight and park its spring at
    /// zero velocity: the board would slam to a halt sideways, the destination
    /// discarded without anything having asked for that. So a pan already
    /// moving is left alone and the zoom simply scales about the middle of the
    /// view for as long as the flight lasts — losing zoom-to-cursor for that
    /// stretch is a far smaller thing than losing the destination outright.
    pub fn zoom_by(&mut self, factor: f32, at: ScreenPoint, vp: &Viewport) {
        if factor <= 0.0 || !factor.is_finite() {
            return;
        }
        if !self.x.moving() && !self.y.moving() {
            self.pivot = Some((vp.to_world(point(at.0, at.1)), at));
        }

        let wanted = self.scale.target() + factor.ln();
        let bound = wanted.clamp(MIN_ZOOM.ln(), MAX_ZOOM.ln());
        // What did not fit becomes strain rather than being dropped. Dropping
        // it is the hard stop: the wheel keeps turning and the board stops
        // answering, which reads as a seized input rather than as an end.
        //
        // Capped well past the rubber band's own asymptote, because the band
        // is what limits the *result* — this only has to stop an unattended
        // scroll wheel accumulating a number with no upper bound.
        self.slack = (self.slack + (wanted - bound)).clamp(-SLACK * 4.0, SLACK * 4.0);
        self.scale.retarget(bound);
    }

    /// How long it has been since the last frame, in seconds.
    ///
    /// Clamped at both ends: a zero would make a step a no-op and a very long
    /// one would make it a teleport. Kept separate from [`step`](Self::step)
    /// because everything else that animates in a frame — the marks fading in
    /// beside a card, a picture arriving — is on the same clock and there must
    /// be exactly one reading of it per frame.
    pub fn tick(&mut self, now: Instant) -> f32 {
        let dt = match self.last {
            Some(then) => now.duration_since(then).as_secs_f32().min(LONGEST_FRAME),
            // The first frame has no previous one to measure from, and guessing
            // a sixtieth of a second would be a guess. Nothing is moving yet.
            None => 0.0,
        };
        self.last = Some(now);
        dt
    }

    /// Restart the clock at `now`, without measuring anything.
    ///
    /// For the first frame after the board has been *still* — which is a
    /// different thing from a long frame. [`tick`](Self::tick)'s clamp
    /// exists for a stall in the middle of a motion, where the springs are
    /// owed the time and a slow frame is better than a dropped one. An idle
    /// board owes its springs nothing: no frames were requested because
    /// nothing was moving, and billing the whole quiet stretch — even
    /// clamped — to the first thing that *starts* moving lands a palette's
    /// entire arrival inside one invisible frame. The caller knows which of
    /// the two it is looking at, because the caller is the one that stopped
    /// asking for frames; see `BoardView::advance`.
    pub fn wake(&mut self, now: Instant) {
        self.last = Some(now);
    }

    /// Advance by `dt` and write the result into the viewport.
    ///
    /// Answers whether another frame is wanted. That return value is the whole
    /// of the app's animation scheduling: while it is true the window asks for
    /// another frame, and the moment it is false the board goes back to
    /// redrawing only when something happens.
    pub fn step(&mut self, vp: &mut Viewport, dt: f32) -> bool {
        // Nothing to do, so the viewport is the authority and the camera
        // catches up to it. This is what absorbs a drag's direct writes — see
        // the module note; it is the invariant the rest of this relies on.
        if !self.moving() {
            self.x.park(vp.pan.x);
            self.y.park(vp.pan.y);
            self.scale.park(vp.zoom.clamp(MIN_ZOOM, MAX_ZOOM).ln());
            return false;
        }

        let mut live = self.scale.step(Spring::ZOOM, dt, SCALE_REST);

        if self.slack != 0.0 {
            self.slack *= (-dt / SLACK_EASE).exp();
            if self.slack.abs() < 1e-4 {
                self.slack = 0.0;
            } else {
                live = true;
            }
        }

        // The strain rides on top of the spring rather than being part of it,
        // so the *target* is always a legal zoom however hard the wheel is
        // leaned on. That is what keeps `resting` safe to save.
        let strain = motion::rubberband(self.slack, SLACK, motion::TENSION);
        vp.zoom = (self.scale.value() + strain).exp();

        match self.pivot {
            Some((world, (sx, sy))) => {
                // Put the pinned world point back under the pixel it was
                // pinned to. The same arithmetic as `Viewport::zoom_by`, run
                // every frame instead of once.
                let (cx, cy) = (vp.size.width / 2.0, vp.size.height / 2.0);
                vp.pan.x = world.x - (sx - cx) / vp.zoom;
                vp.pan.y = world.y + (sy - cy) / vp.zoom;
                // Kept level with the derived pan, so that a flick started
                // during a zoom leaves from where the board actually is.
                self.x.park(vp.pan.x);
                self.y.park(vp.pan.y);
                if !live {
                    self.pivot = None;
                }
            }
            None => {
                let rest = PAN_REST / vp.zoom.max(MIN_ZOOM);
                live |= self.x.step(self.pan_spring, dt, rest);
                live |= self.y.step(self.pan_spring, dt, rest);
                vp.pan.x = self.x.value();
                vp.pan.y = self.y.value();
            }
        }

        live
    }
}

/// How far the pointer has been moving lately, and how fast.
///
/// A release velocity taken from the last two events is noise: pointers report
/// at whatever rate the hardware feels like, a pair of them can be a
/// millisecond apart, and one stray sample divided by one millisecond is a
/// number that throws the board into the next county. So a short history is
/// kept and the velocity is measured across it.
///
/// [`Trail::throw`] is the one the release actually asks, and it is the
/// stricter of the two: a measured speed is not on its own a throw.
#[derive(Debug, Default)]
pub struct Trail {
    samples: VecDeque<(WorldPoint, Instant)>,
}

/// How many samples to keep. Enough to average out one bad one; few enough
/// that the answer is about the end of the gesture rather than its middle.
const TRAIL: usize = 5;

/// How far back a sample may be and still count towards the velocity.
const TRAIL_WINDOW: f32 = 0.1;

/// How stale the newest sample may be at the release and still mean anything.
///
/// The rule this enforces: **stopping before letting go means letting go.**
/// Somebody who drags, pauses with the button still down and then releases has
/// said exactly where they want the board, and flinging it from the velocity
/// they had a second ago would be answering a question they had already
/// withdrawn.
const TRAIL_STALE: f32 = 0.06;

/// The shortest span worth dividing by.
const TRAIL_SHORTEST: f32 = 0.008;

/// Below this speed at release, the hand was not throwing anything, in
/// *screen* pixels a second.
///
/// The rule this enforces: **inertia is for flicks, not for every drag.**
/// Placing the board by hand and letting go while the hand happens to still
/// be drifting is not a throw, and answering it with one is why the board
/// seemed to slide sometimes and not others — the difference was real but far
/// too small to feel deliberate, so it read as the board deciding for itself.
/// At or below this the board stays exactly where it was put; above it,
/// something carries on — see [`TRAIL_THROW`] for how much.
///
/// In screen pixels rather than world units because it is a statement about
/// the hand, and the hand does not know the zoom: the same flick across the
/// desk has to be the same flick at 10% and at 500%.
const TRAIL_FLOOR: f32 = 120.0;

/// At or above this release speed the board carries on at the hand's full
/// speed, in *screen* pixels a second.
///
/// This used to be the *only* threshold: below it, nothing; at it, the board
/// suddenly carried on at the full release velocity. A release at 399 stopped
/// dead and one at 401 sailed off, and there is no gesture that precise — the
/// hand cannot aim for a pixel-per-second, so which side of the line a release
/// landed on read as the board flipping a coin.
///
/// [`Trail::throw`] now ramps the gain smoothly from `0` at [`TRAIL_FLOOR`] to
/// `1` here, so the speed the hand actually let go at comes out as a
/// proportionate throw instead of an all-or-nothing guess. The cliff was the
/// bug; the two speeds themselves are unchanged from what a flick has always
/// meant here.
const TRAIL_THROW: f32 = 400.0;

impl Trail {
    /// Note where the camera is, now.
    pub fn push(&mut self, at: WorldPoint, now: Instant) {
        self.samples.push_back((at, now));
        while self.samples.len() > TRAIL {
            self.samples.pop_front();
        }
    }

    /// How fast it was going when the hand came off, in world units a second.
    ///
    /// `None` when there is nothing worth carrying: too few samples, too short
    /// a span to divide by, or a hand that had already stopped.
    pub fn velocity(&self, now: Instant) -> Option<(f32, f32)> {
        let (last, at) = *self.samples.back()?;
        if now.duration_since(at).as_secs_f32() > TRAIL_STALE {
            return None;
        }
        // The oldest sample still inside the window, which is what makes this
        // an average over the end of the gesture rather than over all of it.
        let (first, from) = *self
            .samples
            .iter()
            .find(|(_, t)| at.duration_since(*t).as_secs_f32() <= TRAIL_WINDOW)?;
        let span = at.duration_since(from).as_secs_f32();
        if span < TRAIL_SHORTEST {
            return None;
        }
        Some(((last.x - first.x) / span, (last.y - first.y) / span))
    }

    /// The velocity to carry on at, or `None` if the release did not clear
    /// [`TRAIL_FLOOR`].
    ///
    /// Everything [`Trail::velocity`] rejects, and one thing more: a hand at
    /// or below [`TRAIL_FLOOR`] at the release was placing the board rather
    /// than throwing it. Between the floor and [`TRAIL_THROW`] the release is
    /// scaled down by a smoothstep rather than passed through whole or
    /// dropped outright — see [`TRAIL_THROW`] for why a hard line was worth
    /// replacing. `zoom` converts both thresholds into the trail's own units.
    pub fn throw(&self, now: Instant, zoom: f32) -> Option<(f32, f32)> {
        let (vx, vy) = self.velocity(now)?;
        // The whole speed, not either axis: a diagonal flick is one gesture,
        // and asking per-axis would take a fast throw at 45 degrees for two
        // slow ones and drop it.
        let z = zoom.max(f32::EPSILON);
        let floor = TRAIL_FLOOR / z;
        let full = TRAIL_THROW / z;
        let speed = vx.hypot(vy);
        if speed <= floor {
            return None;
        }
        // Smoothstep rather than a straight ramp: a linear gain starts moving
        // the instant the hand clears the floor, which just relocates the
        // cliff to that boundary as a kink in the carried velocity instead of
        // a jump in it. The cubic's derivative is zero at both ends, so the
        // gain comes on gradually and saturates gradually too.
        let t = ((speed - floor) / (full - floor)).clamp(0.0, 1.0);
        let gain = t * t * (3.0 - 2.0 * t);
        Some((vx * gain, vy * gain))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mbrd_core::viewport::{ViewSize, BASE_ZOOM};
    use std::time::Duration;

    fn vp() -> Viewport {
        Viewport {
            pan: point(0.0, 0.0),
            zoom: BASE_ZOOM,
            size: ViewSize { width: 800.0, height: 600.0 },
        }
    }

    /// Run a camera to a standstill, and say how many frames it took.
    fn settle(cam: &mut Camera, vp: &mut Viewport) -> usize {
        for frame in 1..=600 {
            if !cam.step(vp, 1.0 / 120.0) {
                return frame;
            }
        }
        panic!("it never settled");
    }

    #[test]
    fn a_resting_camera_costs_nothing_and_follows_the_viewport() {
        // The invariant the whole module rests on: a drag writes the viewport
        // directly, and the next frame absorbs it rather than fighting it.
        let (mut v, mut cam) = (vp(), Camera::new(&vp()));
        v.pan = point(500.0, -250.0);
        v.zoom = 2.0;
        assert!(!cam.step(&mut v, 1.0 / 120.0), "a still camera asked for a frame");
        assert_eq!(v.pan, point(500.0, -250.0), "it moved the board");
        assert_eq!(v.zoom, 2.0);
        // And a spring started afterwards starts from there, not from the
        // origin the camera was made at.
        assert_eq!(cam.resting(), (point(500.0, -250.0), 2.0));
    }

    #[test]
    fn travelling_arrives_where_it_was_sent() {
        let (mut v, mut cam) = (vp(), Camera::new(&vp()));
        let mut want = v;
        want.pan = point(1200.0, 800.0);
        want.zoom = 2.5;
        cam.travel_to(&want);
        settle(&mut cam, &mut v);
        assert!((v.pan.x - 1200.0).abs() < 0.5, "landed at {:?}", v.pan);
        assert!((v.pan.y - 800.0).abs() < 0.5, "landed at {:?}", v.pan);
        assert!((v.zoom - 2.5).abs() < 0.01, "landed at {}", v.zoom);
    }

    #[test]
    fn travelling_takes_time_but_not_forever() {
        let (mut v, mut cam) = (vp(), Camera::new(&vp()));
        let mut want = v;
        want.pan = point(4000.0, 0.0);
        cam.travel_to(&want);
        let frames = settle(&mut cam, &mut v);
        // At 120fps: long enough to read as a move, short enough not to be a
        // wait. The response is 0.4s, and a critically damped spring needs a
        // couple of those to be within half a pixel of a four-thousand-unit
        // journey.
        assert!(frames > 20, "it teleported, in {frames} frames");
        assert!(frames < 240, "it took {frames} frames, which is over two seconds");
    }

    #[test]
    fn zooming_mid_travel_does_not_kill_the_pan() {
        // The brick wall this module's invariant promises not to be:
        // `zoom_by` used to take the pivot unconditionally, and the pivot
        // branch of `step` derives the pan from the pivot and parks the pan
        // springs every frame — so a wheel notch during a `travel_to` used to
        // silently zero the pan's velocity and drop the destination, and the
        // board would slam to a halt sideways mid-flight.
        let (mut v, mut cam) = (vp(), Camera::new(&vp()));
        let mut want = v;
        want.pan = point(4000.0, 0.0);
        cam.travel_to(&want);
        for _ in 0..10 {
            cam.step(&mut v, 1.0 / 120.0);
        }
        let target_before = cam.resting().0;

        // Velocity just before the zoom, from two consecutive positions.
        let p0 = v.pan;
        cam.step(&mut v, 1.0 / 120.0);
        let p1 = v.pan;
        let velocity_before = (p1.x - p0.x) * 120.0;
        assert!(velocity_before > 0.0, "it should already be travelling");

        cam.zoom_by(1.1, (400.0, 300.0), &v);
        cam.step(&mut v, 1.0 / 120.0);
        let p2 = v.pan;
        let velocity_after = (p2.x - p1.x) * 120.0;

        assert!(
            velocity_after > velocity_before * 0.9,
            "the zoom killed the pan: {velocity_before} became {velocity_after}"
        );
        assert_eq!(cam.resting().0, target_before, "zooming mid-flight changed the destination");
    }

    #[test]
    fn a_zoom_holds_the_point_under_the_cursor_for_the_whole_animation() {
        // The one that matters. Checking only the ends would pass on an
        // implementation that lets the board slide out from under the pointer
        // in between, which is exactly the artefact worth preventing.
        let (mut v, mut cam) = (vp(), Camera::new(&vp()));
        let at = (650.0, 120.0);
        let before = v.to_world(point(at.0, at.1));
        cam.zoom_by(1.6, at, &v);
        for _ in 0..200 {
            if !cam.step(&mut v, 1.0 / 120.0) {
                break;
            }
            let now = v.to_world(point(at.0, at.1));
            assert!(
                (now.x - before.x).abs() < 0.5 && (now.y - before.y).abs() < 0.5,
                "the board slid under the cursor: {before:?} became {now:?}",
            );
        }
    }

    #[test]
    fn zooming_past_the_top_strains_and_comes_back() {
        let (mut v, mut cam) = (vp(), Camera::new(&vp()));
        v.zoom = MAX_ZOOM;
        cam.park(&v);
        for _ in 0..20 {
            cam.zoom_by(1.2, (400.0, 300.0), &v);
            cam.step(&mut v, 1.0 / 120.0);
        }
        assert!(v.zoom > MAX_ZOOM, "it hard-stopped at {}", v.zoom);
        assert!(v.zoom < MAX_ZOOM * 1.4, "it strained all the way to {}", v.zoom);
        // And lets go the moment the wheel does.
        settle(&mut cam, &mut v);
        assert!((v.zoom - MAX_ZOOM).abs() < 0.001, "it stayed strained at {}", v.zoom);
    }

    #[test]
    fn a_strained_camera_still_saves_a_legal_zoom() {
        // The reason the rubber band rides on top of the spring rather than
        // inside it: `resting` is what a save writes, and a board must never
        // be written with a zoom the format does not allow.
        let (mut v, mut cam) = (vp(), Camera::new(&vp()));
        v.zoom = MAX_ZOOM;
        cam.park(&v);
        for _ in 0..20 {
            cam.zoom_by(1.2, (400.0, 300.0), &v);
            cam.step(&mut v, 1.0 / 120.0);
        }
        let (_, resting) = cam.resting();
        assert!(resting <= MAX_ZOOM, "it would have saved {resting}");
        assert!(resting >= MIN_ZOOM);
    }

    #[test]
    fn zooming_out_past_the_bottom_strains_too() {
        let (mut v, mut cam) = (vp(), Camera::new(&vp()));
        v.zoom = MIN_ZOOM;
        cam.park(&v);
        for _ in 0..20 {
            cam.zoom_by(0.8, (400.0, 300.0), &v);
            cam.step(&mut v, 1.0 / 120.0);
        }
        assert!(v.zoom < MIN_ZOOM, "it hard-stopped at {}", v.zoom);
        assert!(v.zoom > MIN_ZOOM * 0.7, "it strained all the way to {}", v.zoom);
    }

    #[test]
    fn a_flick_carries_on_the_way_it_was_going() {
        // 400.0 was the threshold before `motion::DECELERATION` was retuned
        // from 0.998 to 0.996 for a snappier canvas glide (see `Spring::THROW`
        // for why the two are a matched pair); `motion::project(1200.0, 0.996)`
        // now lands at roughly 300, so the bar is lower but still well short
        // of a no-op.
        let (mut v, mut cam) = (vp(), Camera::new(&vp()));
        cam.fling(point(0.0, 0.0), 1200.0, 0.0);
        settle(&mut cam, &mut v);
        assert!(v.pan.x > 250.0, "a hard flick only carried it to {}", v.pan.x);
        assert_eq!(v.pan.y, 0.0, "a sideways flick moved it vertically");
    }

    #[test]
    fn a_harder_flick_carries_further() {
        let mut gentle = (vp(), Camera::new(&vp()));
        let mut hard = (vp(), Camera::new(&vp()));
        gentle.1.fling(point(0.0, 0.0), 400.0, 0.0);
        hard.1.fling(point(0.0, 0.0), 1600.0, 0.0);
        settle(&mut gentle.1, &mut gentle.0);
        settle(&mut hard.1, &mut hard.0);
        assert!(hard.0.pan.x > gentle.0.pan.x * 3.0);
    }

    #[test]
    fn a_flick_never_speeds_up_after_the_hand_lets_go() {
        // The bug `Spring::THROW` exists to kill: with a pan spring stiffer
        // than the decay `motion::project` assumes, the position error at the
        // handoff dominates the seeded velocity and the board *accelerates*
        // for the first several frames after release — a lurch to several
        // times the release speed — before dying away. The two tests above
        // only look at where the flick lands, and land it does either way, so
        // neither would ever catch this: this one watches the whole flight.
        let (mut v, mut cam) = (vp(), Camera::new(&vp()));
        let release = 1200.0;
        cam.fling(point(0.0, 0.0), release, 0.0);

        let dt = 0.008;
        let mut last = v.pan;
        let mut peak: f32 = 0.0;
        for _ in 0..2000 {
            let moving = cam.step(&mut v, dt);
            let speed = ((v.pan.x - last.x) / dt).abs();
            peak = peak.max(speed);
            last = v.pan;
            if !moving {
                break;
            }
        }
        assert!(
            peak <= release * 1.05,
            "the flick peaked at {peak}, well past the release speed of {release}"
        );
    }

    #[test]
    fn seizing_a_moving_camera_stops_it_where_it_is_drawn() {
        // The interruption rule. What must *not* happen is the board carrying
        // on to its projected resting place after it has been grabbed.
        let (mut v, mut cam) = (vp(), Camera::new(&vp()));
        cam.fling(point(0.0, 0.0), 2000.0, 0.0);
        for _ in 0..10 {
            cam.step(&mut v, 1.0 / 120.0);
        }
        let caught = v.pan;
        cam.seize(&v);
        assert!(!cam.moving());
        assert!(!cam.step(&mut v, 1.0 / 120.0));
        assert_eq!(v.pan, caught, "it kept going after being grabbed");
    }

    #[test]
    fn one_enormous_frame_puts_the_camera_where_it_was_going() {
        // This is the whole of reduced motion — see `BoardView::advance`, which
        // implements it by handing this a frame long enough that everything has
        // already happened. What must be true is that the camera *arrives*
        // rather than being cancelled: somebody who cannot look at a moving
        // board still has to end up where they pressed `F` to go.
        let (mut v, mut cam) = (vp(), Camera::new(&vp()));
        let mut want = v;
        want.pan = point(3000.0, -1500.0);
        want.zoom = 2.5;
        cam.travel_to(&want);
        assert!(!cam.step(&mut v, 10.0), "it was still going after a ten-second frame");
        assert!((v.pan.x - 3000.0).abs() < 0.01, "landed at {:?}", v.pan);
        assert!((v.pan.y + 1500.0).abs() < 0.01, "landed at {:?}", v.pan);
        assert!((v.zoom - 2.5).abs() < 0.001, "landed at {}", v.zoom);
    }

    #[test]
    fn one_enormous_frame_takes_the_strain_out_too() {
        let (mut v, mut cam) = (vp(), Camera::new(&vp()));
        v.zoom = MAX_ZOOM;
        cam.park(&v);
        for _ in 0..20 {
            cam.zoom_by(1.2, (400.0, 300.0), &v);
            cam.step(&mut v, 1.0 / 120.0);
        }
        assert!(!cam.step(&mut v, 10.0));
        assert!((v.zoom - MAX_ZOOM).abs() < 0.001, "it stayed strained at {}", v.zoom);
    }

    #[test]
    fn the_first_frame_has_no_length() {
        let mut cam = Camera::new(&vp());
        assert_eq!(cam.tick(Instant::now()), 0.0);
    }

    #[test]
    fn a_very_long_gap_is_a_slow_frame_rather_than_a_jump() {
        let mut cam = Camera::new(&vp());
        let now = Instant::now();
        cam.tick(now);
        assert_eq!(cam.tick(now + Duration::from_secs(30)), LONGEST_FRAME);
    }

    #[test]
    fn waking_forgives_the_idle_time_instead_of_billing_it() {
        // A board that sat still for half an hour and then had its palette
        // opened: the first animation frame must be a fresh start, not a
        // quarter-second leap that lands every spring before it can be seen.
        let mut cam = Camera::new(&vp());
        let now = Instant::now();
        cam.tick(now);
        let later = now + Duration::from_secs(1800);
        cam.wake(later);
        assert_eq!(cam.tick(later), 0.0);
    }

    #[test]
    fn a_trail_measures_the_speed_it_was_dragged_at() {
        let now = Instant::now();
        let mut trail = Trail::default();
        // 100 world units in 50ms, which is 2000 a second.
        for i in 0..=5 {
            let t = now - Duration::from_millis(50 - i * 10);
            trail.push(point(i as f32 * 20.0, 0.0), t);
        }
        let (vx, vy) = trail.velocity(now).expect("that was a drag");
        assert!((vx - 2000.0).abs() < 50.0, "it measured {vx}");
        assert_eq!(vy, 0.0);
    }

    #[test]
    fn a_hand_that_stopped_before_letting_go_does_not_fling() {
        let now = Instant::now();
        let mut trail = Trail::default();
        for i in 0..=5 {
            trail.push(point(i as f32 * 20.0, 0.0), now - Duration::from_millis(500 - i * 10));
        }
        // Dragged fast, then held still for half a second. The board stays.
        assert!(trail.velocity(now).is_none());
    }

    /// A drag at `speed` world units a second, sampled the way a pointer does.
    fn dragged_at(speed: f32, now: Instant) -> Trail {
        let mut trail = Trail::default();
        for i in 0..=5 {
            let ago = 50 - i * 10;
            let t = now - Duration::from_millis(ago as u64);
            trail.push(point(-speed * ago as f32 / 1000.0, 0.0), t);
        }
        trail
    }

    #[test]
    fn a_slow_drag_is_a_placement_and_does_not_throw() {
        // The complaint this exists for: letting go of a deliberate drag while
        // the hand is still drifting used to slide the board a little, which
        // read as inertia arriving at random. Below TRAIL_FLOOR, not merely
        // below the old single TRAIL_THROW cliff.
        let now = Instant::now();
        let trail = dragged_at(TRAIL_FLOOR * 0.5, now);
        assert!(trail.velocity(now).is_some(), "the drag was not even measured");
        assert!(trail.throw(now, BASE_ZOOM).is_none(), "a slow drag threw the board");
    }

    #[test]
    fn a_quick_drag_still_throws() {
        let now = Instant::now();
        let (vx, vy) = dragged_at(1600.0, now).throw(now, BASE_ZOOM).expect("that was a flick");
        assert!((vx - 1600.0).abs() < 50.0, "it threw at {vx}");
        assert_eq!(vy, 0.0);
    }

    #[test]
    fn the_throw_floor_is_a_speed_of_the_hand_not_of_the_board() {
        // Zoomed out, the same hand covers far more world units a second. The
        // floor has to move with it, or a gentle drag at 10% would fling and
        // the identical gesture at 100% would not.
        let now = Instant::now();
        let gentle = (TRAIL_FLOOR * 0.5) / 0.1;
        assert!(
            dragged_at(gentle, now).throw(now, 0.1).is_none(),
            "the same slow hand threw the board once it was zoomed out"
        );
        let flick = (TRAIL_THROW * 4.0) / 0.1;
        assert!(dragged_at(flick, now).throw(now, 0.1).is_some(), "a flick at 10% did nothing");
    }

    #[test]
    fn a_release_between_the_floor_and_the_full_throw_is_scaled_not_all_or_nothing() {
        // The bug the smoothstep replaces: a straight cliff at TRAIL_THROW
        // meant every release in between either carried at the full release
        // speed or not at all. A release halfway between the two thresholds
        // should carry, but slower than the hand actually let go at.
        let now = Instant::now();
        let midpoint = (TRAIL_FLOOR + TRAIL_THROW) / 2.0;
        let (vx, _) = dragged_at(midpoint, now).throw(now, BASE_ZOOM).expect("above the floor");
        assert!(vx > 0.0, "it did not carry on at all");
        assert!(vx < midpoint, "a half-cleared throw carried at the full release speed");
    }

    #[test]
    fn a_diagonal_flick_is_one_gesture() {
        // Neither axis clears the floor alone; together they are plainly a
        // throw, and a per-axis test would have dropped it.
        let now = Instant::now();
        let mut trail = Trail::default();
        let speed = TRAIL_THROW * 0.8;
        for i in 0..=5 {
            let ago = 50 - i * 10;
            let d = -speed * ago as f32 / 1000.0;
            trail.push(point(d, d), now - Duration::from_millis(ago as u64));
        }
        assert!(trail.throw(now, BASE_ZOOM).is_some(), "a diagonal flick was ignored");
    }

    #[test]
    fn a_hand_that_stopped_before_letting_go_never_throws() {
        let now = Instant::now();
        let mut trail = Trail::default();
        for i in 0..=5 {
            trail.push(point(i as f32 * 200.0, 0.0), now - Duration::from_millis(500 - i * 10));
        }
        assert!(trail.throw(now, BASE_ZOOM).is_none());
    }

    #[test]
    fn one_sample_is_not_a_velocity() {
        let now = Instant::now();
        let mut trail = Trail::default();
        trail.push(point(0.0, 0.0), now);
        assert!(trail.velocity(now).is_none());
    }

    #[test]
    fn a_trail_forgets_the_beginning_of_a_long_drag() {
        // A drag that crossed the whole board slowly and then flicked at the
        // end should fling at the flick's speed, not at the average.
        let now = Instant::now();
        let mut trail = Trail::default();
        trail.push(point(-10_000.0, 0.0), now - Duration::from_secs(5));
        for i in 0..=4 {
            trail.push(point(i as f32 * 20.0, 0.0), now - Duration::from_millis(40 - i * 10));
        }
        let (vx, _) = trail.velocity(now).expect("that ended in a flick");
        assert!(vx > 0.0 && vx < 5000.0, "the slow beginning leaked in: {vx}");
    }
}
