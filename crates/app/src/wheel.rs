//! What one turn of a wheel, or one push of two fingers, is asking for.
//!
//! Kept apart from `board_view.rs` for the reason `grips.rs` and `anchor.rs`
//! are: the answer is arithmetic over four numbers and a flag, and written here
//! it can be asserted directly instead of by driving a window.
//!
//! ## Three devices arrive through one event
//!
//! A wheel reports **lines** — one notch, one line, and nothing between them. A
//! trackpad reports **pixels**, a stream of small ones for as long as fingers
//! are moving. And a *pinch* also reports pixels, with `control` set, which is
//! not a key anybody pressed: browsers have set that flag on a pinch since
//! Chrome 35 and IE10 before it, so that a page could tell the two apart at
//! all, and every canvas application on the web reads it the same way. gpui
//! carries it through from the browser, and on the desktop `Ctrl` and a wheel
//! is the same intent typed out.
//!
//! ## Why pixels pan
//!
//! They used to zoom, all of them, and that made the app unusable on a
//! trackpad in a way nobody had measured: two fingers zoomed, `Shift` and two
//! fingers panned sideways, and **nothing panned up or down**. The only ways
//! left were the Pan tool, a drag on empty paper, and the middle button, which
//! a trackpad does not have.
//!
//! Pixels therefore pan and lines still zoom, which is the split every other
//! infinite canvas draws and, on the desktop, is also exactly the split between
//! the two devices. In a browser it is not — Chrome reports a mouse wheel as
//! pixels too — but a mouse wheel that scrolls the page and `Ctrl` that zooms
//! it is what a browser does everywhere else, so the same rule lands on the
//! right answer twice for different reasons. Anybody who wants the old feel has
//! [`Command::ToggleTrackpadPan`] and gets it back for both.
//!
//! [`Command::ToggleTrackpadPan`]: crate::command::Command::ToggleTrackpadPan
//!
//! ## Why a pinch has a rate of its own
//!
//! A pinch reports one to twelve pixels per event. The old code divided pixels
//! by forty and raised `1.12` to that, which is 1.0085 for a three-pixel event
//! — a whole pinch, thirty events of it, moved the camera by about a third.
//! `PINCH_RATE` is the figure the rest of the world uses for the same gesture,
//! and it is roughly three and a half times faster.

/// How much a pinch zooms for each pixel the fingers report.
///
/// `exp(0.01 · pixels)`, which is the rate Figma, tldraw and the sibling
/// JavaScript app all converge on from different directions.
pub const PINCH_RATE: f32 = 0.01;

/// The delta at which pixels are a wheel notch rather than fingers.
///
/// A pinch never reports this much at once and a notch always reports more —
/// Chrome sends 100 or 120 for one. Without the split, `Ctrl` and one turn of
/// a mouse wheel in a browser would be `exp(1.0)`, which throws the camera
/// across most of the zoom range in a single detent.
pub const NOTCH: f32 = 50.0;

/// How far a notch of the wheel zooms, as a factor for each line.
///
/// The app's own long-standing figure. It is here rather than in
/// `board_view.rs` so that [`per_pixel_notch`] can be derived from it and the
/// two can never drift.
pub const PER_LINE: f32 = 0.12;

/// The pinch rate that makes a hundred-pixel notch worth exactly one line.
///
/// Derived rather than written down: a wheel that means one thing on a desktop
/// and another in a browser is the same wheel, and the person turning it should
/// not be able to tell which build they are in.
pub fn per_pixel_notch() -> f32 {
    (1.0 + PER_LINE).ln() / 100.0
}

/// What arrived, in the two shapes gpui reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Delta {
    /// A trackpad, or a browser's idea of a wheel. Exact pixels.
    Pixels { dx: f32, dy: f32 },
    /// A wheel with detents. Whole lines, or something close to them.
    Lines { dx: f32, dy: f32 },
}

/// The two modifiers that change what a scroll means.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Held {
    /// Set by hand on a desktop, and set by the browser on a pinch.
    pub control: bool,
    /// Sideways, by the convention every scrolling surface shares.
    pub shift: bool,
}

/// What the camera should be asked to do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Reading {
    /// Move by this many screen pixels, in the direction the *board* should
    /// go: positive is right and down, which is where the fingers went. The
    /// camera moves the other way, and `Camera::nudge` already flips one of
    /// the two axes itself, so the caller flips the other.
    Pan { dx: f32, dy: f32 },
    /// Multiply the zoom by this, about the pointer.
    Zoom { factor: f32 },
}

/// Read one event.
///
/// `pans` is the preference: false puts pixels back to zooming, which is what
/// this app did before trackpads could pan at all.
pub fn read(delta: Delta, held: Held, pans: bool) -> Reading {
    let (dx, dy) = match delta {
        Delta::Pixels { dx, dy } | Delta::Lines { dx, dy } => (dx, dy),
    };

    // Sideways first, and before the zoom, because `Shift` is the one answer
    // that is the same on every device: a mouse with one wheel has nothing but
    // its vertical delta to give, so that is what drives it, and a trackpad's
    // own horizontal component is added on top.
    if held.shift {
        let by = match delta {
            Delta::Pixels { .. } => dy + dx,
            Delta::Lines { .. } => (dy + dx) * 40.0,
        };
        return Reading::Pan { dx: by, dy: 0.0 };
    }

    match delta {
        // A pinch, or `Ctrl` and a wheel — see the module note on why those are
        // the same event.
        Delta::Pixels { .. } if held.control => {
            let rate = if dy.abs() >= NOTCH { per_pixel_notch() } else { PINCH_RATE };
            Reading::Zoom { factor: (dy * rate).exp() }
        }
        // Fingers. Panning is the whole of this change; `pans` is the way back.
        Delta::Pixels { .. } if pans => Reading::Pan { dx, dy },
        Delta::Pixels { .. } => Reading::Zoom { factor: (1.0 + PER_LINE).powf(dy / 40.0) },
        // A wheel, with or without `Ctrl`. Unchanged, and deliberately so: this
        // is the gesture the desktop app has always zoomed with.
        Delta::Lines { .. } => Reading::Zoom { factor: (1.0 + PER_LINE).powf(dy) },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pinch(dy: f32) -> Reading {
        read(Delta::Pixels { dx: 0.0, dy }, Held { control: true, shift: false }, true)
    }

    fn zoomed(reading: Reading) -> f32 {
        match reading {
            Reading::Zoom { factor } => factor,
            other => panic!("expected a zoom, got {other:?}"),
        }
    }

    #[test]
    fn a_pinch_moves_the_camera_at_a_rate_a_hand_can_feel() {
        // The complaint this exists to answer, in numbers. A macOS pinch
        // reports about three pixels an event, and the old arithmetic —
        // `1.12 ^ (3 / 40)` — turned that into 0.85% of zoom. Thirty events of
        // it, which is a whole pinch, moved the camera by a third.
        // Compared as *amounts of zoom* rather than as factors: 1.0085 and
        // 1.0304 are three per cent apart as numbers and three and a half
        // times apart as gestures, and it is the second one a hand feels.
        let was = (1.0f32 + PER_LINE).powf(3.0 / 40.0) - 1.0;
        let now = zoomed(pinch(3.0));
        assert!(now - 1.0 > was * 3.0, "was {was}, now {}", now - 1.0);
        assert!(now.powi(30) > 2.0, "a whole pinch still did not double it: {now}");
    }

    #[test]
    fn a_pinch_the_other_way_undoes_a_pinch() {
        // Symmetry, or a pinch out and a pinch back leave the board somewhere
        // it never was. The exponential is what buys this and it is worth an
        // assertion, because the arithmetic it replaced had it too.
        let out = zoomed(pinch(6.0));
        let back = zoomed(pinch(-6.0));
        assert!((out * back - 1.0).abs() < 1e-5, "{out} then {back}");
    }

    #[test]
    fn a_wheel_notch_held_with_control_is_not_read_as_a_pinch() {
        // A browser reports a mouse wheel in pixels too, so `Ctrl` and one
        // detent arrives here looking exactly like a very large pinch. At the
        // pinch rate that is `exp(1.0)` — most of the zoom range in one turn.
        let notch = zoomed(pinch(100.0));
        let line = (1.0f32 + PER_LINE).powf(1.0);
        assert!((notch - line).abs() < 1e-4, "a notch should be a line: {notch} vs {line}");
        assert!(notch < 1.5, "one detent moved the camera {notch} times");
    }

    #[test]
    fn two_fingers_pan_rather_than_zoom() {
        // The gesture that had no way to happen at all: before this, a plain
        // two-finger scroll zoomed, `Shift` panned sideways, and nothing on a
        // trackpad panned up or down.
        let reading = read(Delta::Pixels { dx: -4.0, dy: 12.0 }, Held::default(), true);
        assert_eq!(reading, Reading::Pan { dx: -4.0, dy: 12.0 });
    }

    #[test]
    fn the_preference_puts_the_old_behaviour_back() {
        let held = Held::default();
        let now = read(Delta::Pixels { dx: 0.0, dy: 12.0 }, held, false);
        let was = (1.0f32 + PER_LINE).powf(12.0 / 40.0);
        assert!((zoomed(now) - was).abs() < 1e-6, "{now:?} was not the old arithmetic");
    }

    #[test]
    fn a_wheel_still_zooms_whatever_the_preference_says() {
        // The desktop habit this change must not disturb. A notch is a line and
        // a line is `PER_LINE`, exactly as it has always been.
        for pans in [true, false] {
            let reading = read(Delta::Lines { dx: 0.0, dy: 1.0 }, Held::default(), pans);
            assert!((zoomed(reading) - (1.0 + PER_LINE)).abs() < 1e-6, "pans = {pans}");
        }
    }

    #[test]
    fn shift_is_sideways_on_both_devices() {
        // And it is the *vertical* delta that drives it, because a mouse with
        // one wheel has nothing else to give.
        let held = Held { control: false, shift: true };
        assert_eq!(
            read(Delta::Pixels { dx: 0.0, dy: 9.0 }, held, true),
            Reading::Pan { dx: 9.0, dy: 0.0 },
        );
        assert_eq!(
            read(Delta::Lines { dx: 0.0, dy: 1.0 }, held, true),
            Reading::Pan { dx: 40.0, dy: 0.0 },
        );
    }

    #[test]
    fn nothing_ever_reads_as_a_zoom_of_nothing() {
        // A factor of zero or a negative one is a camera that vanishes. The
        // exponential cannot produce either, and `powf` cannot from a positive
        // base, but the two are worth pinning: this is the one number in the
        // module that reaches the viewport directly.
        for dy in [-400.0, -100.0, -12.0, -0.5, 0.0, 0.5, 12.0, 100.0, 400.0] {
            for control in [true, false] {
                for pans in [true, false] {
                    for delta in [Delta::Pixels { dx: 0.0, dy }, Delta::Lines { dx: 0.0, dy }] {
                        if let Reading::Zoom { factor } =
                            read(delta, Held { control, shift: false }, pans)
                        {
                            assert!(factor > 0.0 && factor.is_finite(), "{delta:?} gave {factor}");
                        }
                    }
                }
            }
        }
    }
}
