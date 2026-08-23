//! How a number gets from where it is to where it is going.
//!
//! Kept in `core` and kept pure for the reason `geometry` and `rope` are: a
//! spring is arithmetic, and written here it can be tested by stepping it and
//! asserting where it ended up, with no window, no event loop and no clock of
//! its own. Everything in this module takes the elapsed time as an argument.
//!
//! ## Two numbers, not three
//!
//! The physics of a damped oscillator is mass, stiffness and damping, and
//! nobody has ever tuned an interface by choosing a mass. So the description
//! here is the one Apple's designers use instead:
//!
//! - **[`Spring::damping`]** is the damping *ratio*, and it decides overshoot.
//!   `1.0` is critically damped — the fastest approach that never goes past.
//!   Below `1.0` it overshoots and comes back.
//! - **[`Spring::response`]** is roughly how long the approach takes, in
//!   seconds. It is deliberately **not** a duration: a spring has no duration,
//!   because it can be retargeted half-way and its settling time falls out of
//!   the arithmetic rather than being prescribed.
//!
//! The rule of thumb the constants below follow: overshoot has to be *earned*
//! by the gesture. A camera sent home by a key press did not come from
//! anybody's hand and should arrive without bouncing; a board thrown sideways
//! did, and should.
//!
//! ## Why the closed form rather than an integrator
//!
//! The obvious implementation steps the differential equation once per frame.
//! It is also the one that explodes when a frame takes 200ms, which happens
//! whenever a window is dragged between monitors or a big picture finishes
//! decoding. [`Sprung::step`] evaluates the exact solution at `t = dt`, so a
//! long frame is merely a long step rather than a divergence, and a spring
//! never has to be told to substep.

use std::f32::consts::TAU;

/// The feel of a spring, in the two numbers worth thinking in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spring {
    /// The damping ratio. `1.0` never overshoots; below it, it does.
    ///
    /// Held at or under `1.0` by [`Spring::new`]. An overdamped spring — one
    /// that crawls in without ever oscillating — is slower than critical for
    /// no gain in calm, and there is no interface here that wants one.
    pub damping: f32,
    /// About how long the approach takes, in seconds. Not a duration.
    pub response: f32,
}

impl Spring {
    /// A spring, with both numbers held to what they can actually mean.
    pub fn new(damping: f32, response: f32) -> Self {
        Self { damping: damping.clamp(0.05, 1.0), response: response.max(0.01) }
    }

    /// Moving the camera somewhere it was asked to be.
    ///
    /// Critically damped, because a jump to the origin or a fit to the board is
    /// a *destination* rather than a throw: nothing about pressing `0` implies
    /// momentum, and a camera that sailed past the origin and drifted back
    /// would be inventing a gesture nobody made.
    pub const CAMERA: Spring = Spring { damping: 1.0, response: 0.4 };

    /// Carrying on after a flick.
    ///
    /// The one place overshoot belongs, because here the hand really was
    /// moving: the board is following through on a throw, and a throw that
    /// stopped dead on the millimetre would feel like it hit a wall.
    pub const FLICK: Spring = Spring { damping: 0.8, response: 0.4 };

    /// Answering the wheel.
    ///
    /// Faster than [`CAMERA`](Spring::CAMERA) and still critically damped. A
    /// zoom notch is a small, repeated, aimed input — the smoothing is there to
    /// stop a mouse wheel arriving in steps, not to take the scenic route, and
    /// anything slower than this reads as lag rather than as motion.
    pub const ZOOM: Spring = Spring { damping: 1.0, response: 0.19 };

    /// The undamped angular frequency the two numbers imply.
    fn omega(&self) -> f32 {
        TAU / self.response
    }
}

/// One number on its way somewhere, and how fast it is going.
///
/// The velocity is a field rather than something rediscovered from the last two
/// values, and that is the whole reason a gesture can be interrupted cleanly:
/// retargeting mid-flight keeps it, so a board thrown left and then sent home
/// bends out of the throw instead of stopping and restarting. Recomputing
/// velocity from positions loses exactly that.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sprung {
    value: f32,
    velocity: f32,
    target: f32,
}

impl Sprung {
    /// At rest, here.
    pub fn at(value: f32) -> Self {
        Self { value, velocity: 0.0, target: value }
    }

    /// Where it is now. **This is the number to draw**, always — never the
    /// target. Drawing the target is what makes an interrupted animation jump.
    pub fn value(&self) -> f32 {
        self.value
    }

    /// How fast it is going, in units per second.
    pub fn velocity(&self) -> f32 {
        self.velocity
    }

    /// Where it is going. What a *save* should write down, since a camera
    /// caught mid-flight is not anywhere anybody chose to be.
    pub fn target(&self) -> f32 {
        self.target
    }

    /// Put it here, now, with no motion and nothing left to do.
    ///
    /// What a drag wants on every frame: while a hand is on it the value is the
    /// hand's, and there is nothing for a spring to decide.
    pub fn park(&mut self, value: f32) {
        self.value = value;
        self.target = value;
        self.velocity = 0.0;
    }

    /// Send it somewhere else, keeping whatever motion it already had.
    pub fn retarget(&mut self, target: f32) {
        self.target = target;
    }

    /// Send it somewhere else at a given speed — the handoff at the end of a
    /// gesture, where the animation has to leave at exactly the speed the
    /// pointer arrived at or there is a visible seam between the two.
    pub fn fling(&mut self, target: f32, velocity: f32) {
        self.target = target;
        self.velocity = velocity;
    }

    /// Whether this still has somewhere to be.
    pub fn moving(&self) -> bool {
        self.value != self.target || self.velocity != 0.0
    }

    /// Advance by `dt` seconds. Answers whether it is still moving.
    ///
    /// `rest` is how close counts as arrived, in the same units as the value —
    /// a quarter of a screen pixel for a camera, a thousandth for a log scale.
    /// It is an argument rather than a field because the same spring drives
    /// world units and log units in the same frame, and "close enough" is a
    /// different number for each.
    pub fn step(&mut self, spring: Spring, dt: f32, rest: f32) -> bool {
        if dt <= 0.0 {
            return self.moving();
        }
        let (x0, v0) = (self.value - self.target, self.velocity);
        // Already there and not going anywhere. Checked before the arithmetic
        // rather than after, so a settled spring costs nothing per frame on a
        // board where the camera has not been touched in an hour.
        if x0 == 0.0 && v0 == 0.0 {
            return false;
        }

        let w = spring.omega();
        let z = spring.damping;
        let decay = (-z * w * dt).exp();

        let (x, v) = if z >= 1.0 {
            // Critically damped: `(x0 + (v0 + w x0) t) e^{-wt}`, and its
            // derivative. One repeated root, so no oscillating term at all.
            let c = v0 + w * x0;
            (decay * (x0 + c * dt), decay * (v0 - w * dt * c))
        } else {
            // Underdamped: the same decay wrapped round a rotation, which is
            // the overshoot — the value crosses the target and comes back.
            let wd = w * (1.0 - z * z).sqrt();
            let (sin, cos) = (wd * dt).sin_cos();
            let c = (v0 + z * w * x0) / wd;
            let x = decay * (x0 * cos + c * sin);
            let v = decay * ((c * wd - z * w * x0) * cos - (x0 * wd + z * w * c) * sin);
            (x, v)
        };

        // Near enough, and slow enough that it would not travel far if left
        // alone. Both halves are needed: a spring at full speed *through* the
        // target is exactly at zero distance for one instant, and stopping it
        // there would eat the overshoot that was the point of `damping < 1`.
        if x.abs() <= rest && (v * spring.response).abs() <= rest {
            self.value = self.target;
            self.velocity = 0.0;
            return false;
        }

        self.value = self.target + x;
        self.velocity = v;
        true
    }
}

/// How fast a flicked thing stops, per the scroll views everybody has already
/// learned the feel of. Nearer `1.0` slides further.
pub const DECELERATION: f32 = 0.998;

/// How far something thrown at `velocity` will travel before it stops.
///
/// This is the function that turns a small gesture into a large result: a flick
/// across an inch of desk should carry the board a screen and a half, and the
/// only way to know how far "a flick" means is to ask how fast the hand was
/// going when it let go.
///
/// It is the exponential-decay result rather than the `v² / 2a` from a physics
/// textbook, and that is not an approximation of the textbook — it is a
/// different model, of a velocity multiplied by a constant factor per
/// millisecond, which is what scroll views actually do and therefore what the
/// hand in question has spent years calibrated against.
pub fn project(velocity: f32, deceleration: f32) -> f32 {
    // Guarded rather than trusted: `deceleration` at exactly 1.0 is a thing
    // that never stops, and the divide would hand back an infinity that then
    // spreads into the camera and takes the whole board with it.
    let d = deceleration.clamp(0.0, 0.999);
    (velocity / 1000.0) * d / (1.0 - d)
}

/// How far past a boundary something dragged `overshoot` past it should
/// actually be drawn.
///
/// A hard stop reads as a seized input — the hand is still moving and the
/// screen is not, and the first guess is that something broke. Resistance that
/// grows with the distance reads as what it is: there is more of the gesture
/// available, and nothing left at the end of it.
///
/// The result never reaches `limit`, however hard it is pushed, which is what
/// makes `limit` the honest thing to name it: it is the asymptote.
pub fn rubberband(overshoot: f32, limit: f32, tension: f32) -> f32 {
    if limit <= 0.0 {
        return 0.0;
    }
    (overshoot * limit * tension) / (limit + tension * overshoot.abs())
}

/// The tension every rubber band here is pulled at.
///
/// UIKit's number. It is not derived from anything — it is the one that has
/// been under everybody's thumb since 2007, and picking a different one would
/// be picking a fight with a decade of muscle memory for no gain.
pub const TENSION: f32 = 0.55;

#[cfg(test)]
mod tests {
    use super::*;

    /// Step a spring for `seconds` at 120fps, and say where it got to.
    fn run(s: &mut Sprung, spring: Spring, seconds: f32, rest: f32) {
        let dt = 1.0 / 120.0;
        let mut t = 0.0;
        while t < seconds {
            s.step(spring, dt, rest);
            t += dt;
        }
    }

    #[test]
    fn a_critically_damped_spring_never_goes_past_its_target() {
        let mut s = Sprung::at(0.0);
        s.retarget(100.0);
        let mut highest: f32 = 0.0;
        for _ in 0..600 {
            s.step(Spring::CAMERA, 1.0 / 120.0, 0.01);
            highest = highest.max(s.value());
        }
        assert!(highest <= 100.0 + 0.01, "it overshot to {highest}");
    }

    #[test]
    fn an_underdamped_spring_does_go_past() {
        // The whole difference between the two constants, asserted rather than
        // trusted: if this stops overshooting, a flick has stopped feeling
        // like a throw and nothing else in the app would notice.
        let mut s = Sprung::at(0.0);
        s.retarget(100.0);
        let mut highest: f32 = 0.0;
        for _ in 0..600 {
            s.step(Spring::FLICK, 1.0 / 120.0, 0.01);
            highest = highest.max(s.value());
        }
        assert!(highest > 100.0, "it never overshot; highest was {highest}");
    }

    #[test]
    fn a_spring_arrives_and_then_stops_costing_anything() {
        let mut s = Sprung::at(0.0);
        s.retarget(100.0);
        run(&mut s, Spring::CAMERA, 3.0, 0.01);
        assert_eq!(s.value(), 100.0);
        assert_eq!(s.velocity(), 0.0);
        // The point of the exact equality above: a settled spring answers
        // `false` forever after, which is what lets the window stop asking for
        // frames instead of burning one a sixtieth of a second on a still board.
        assert!(!s.step(Spring::CAMERA, 1.0 / 120.0, 0.01));
    }

    #[test]
    fn a_very_long_frame_does_not_blow_the_spring_up() {
        // The failure this module's closed form exists to prevent. A window
        // dragged between monitors, or a twenty-megapixel decode landing, can
        // hand the next frame a third of a second; an integrator stepped once
        // by that much diverges and the camera leaves the solar system.
        let mut s = Sprung::at(0.0);
        s.retarget(100.0);
        s.step(Spring::CAMERA, 0.35, 0.01);
        assert!(s.value().is_finite());
        assert!(s.value() >= 0.0 && s.value() <= 101.0, "it went to {}", s.value());
    }

    #[test]
    fn one_long_step_lands_near_where_many_short_ones_do() {
        // Not identical — the closed form is exact per step and the sampling is
        // not — but close, which is what says the solution is the right one
        // rather than merely a stable one.
        let (mut coarse, mut fine) = (Sprung::at(0.0), Sprung::at(0.0));
        coarse.retarget(100.0);
        fine.retarget(100.0);
        coarse.step(Spring::CAMERA, 0.1, 0.0001);
        run(&mut fine, Spring::CAMERA, 0.1, 0.0001);
        assert!(
            (coarse.value() - fine.value()).abs() < 1.0,
            "{} against {}",
            coarse.value(),
            fine.value()
        );
    }

    #[test]
    fn retargeting_mid_flight_keeps_the_motion_it_already_had() {
        let mut s = Sprung::at(0.0);
        s.retarget(100.0);
        run(&mut s, Spring::CAMERA, 0.1, 0.01);
        let carried = s.velocity();
        assert!(carried > 0.0, "it should be moving by now");
        s.retarget(-100.0);
        // Still travelling the old way for an instant, which is the absence of
        // the brick wall: reversing a gesture bends the motion round rather
        // than replacing it with a new one that starts from nothing.
        assert_eq!(s.velocity(), carried);
    }

    #[test]
    fn parking_a_spring_leaves_it_with_nothing_to_do() {
        let mut s = Sprung::at(0.0);
        s.fling(500.0, 900.0);
        s.park(42.0);
        assert_eq!(s.value(), 42.0);
        assert_eq!(s.target(), 42.0);
        assert!(!s.moving());
    }

    #[test]
    fn a_fling_leaves_at_the_speed_it_was_given() {
        // The seam this prevents: an animation that starts from a standstill
        // after a drag that was moving is a visible hitch at the exact moment
        // the hand comes off, which is the moment somebody is looking.
        let mut s = Sprung::at(0.0);
        s.fling(100.0, 250.0);
        assert_eq!(s.velocity(), 250.0);
    }

    #[test]
    fn a_faster_flick_goes_further() {
        let slow = project(300.0, DECELERATION);
        let fast = project(1200.0, DECELERATION);
        assert!(fast > slow * 3.9 && fast < slow * 4.1, "{slow} then {fast}");
    }

    #[test]
    fn a_flick_backwards_projects_backwards() {
        assert!(project(-800.0, DECELERATION) < 0.0);
    }

    #[test]
    fn projection_survives_a_deceleration_of_one() {
        // Nobody should pass this, and the day somebody does the answer has to
        // be a large number rather than an infinity that silently becomes a
        // `NaN` camera two frames later.
        assert!(project(500.0, 1.0).is_finite());
    }

    #[test]
    fn a_rubber_band_gives_less_the_harder_it_is_pulled() {
        let (a, b) = (rubberband(10.0, 100.0, TENSION), rubberband(20.0, 100.0, TENSION));
        assert!(b > a, "further should still be further");
        assert!(b < a * 2.0, "but not twice as far for twice the pull");
    }

    #[test]
    fn a_rubber_band_never_reaches_its_limit() {
        for pull in [50.0, 500.0, 5_000.0, 500_000.0] {
            let given = rubberband(pull, 100.0, TENSION);
            assert!(given < 100.0, "a pull of {pull} gave {given}, which is past the limit");
        }
    }

    #[test]
    fn a_rubber_band_pulled_the_other_way_gives_the_other_way() {
        assert_eq!(rubberband(-30.0, 100.0, TENSION), -rubberband(30.0, 100.0, TENSION));
    }
}
