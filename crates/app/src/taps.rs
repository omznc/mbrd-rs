//! Two taps of a modifier, with nothing in between.
//!
//! A bare Shift never arrives as a key press — the platform reports it as a
//! change in which modifiers are down, and GPUI passes that through as
//! `on_modifiers_changed`. So a double-tap cannot be a row in `command.rs`'s
//! key table the way every other shortcut is; it has to be watched for.
//!
//! What makes it safe to watch for is knowing when *not* to. Shift is held
//! constantly — for a capital, for a shift-drag, for a whole grid step on an
//! arrow key — and a palette that opened in the middle of any of those would be
//! worse than no palette. So a hold is spoiled the moment anything else
//! happens under it, and only a press-and-release with nothing in between
//! counts. The view calls [`Taps::spoil`] from every key press and every mouse
//! press, and [`Taps::forget`] whenever the keyboard belongs to something else.
//!
//! The reason it is worth the trouble: every chord worth having in this app is
//! already taken, and a double-tap spends no key at all.

use std::time::Duration;
// WASM EXPERIMENT: std's clock panics on wasm32-unknown-unknown; `web-time`
// is the same API over `performance.now()`.
#[cfg(not(target_family = "wasm"))]
use std::time::Instant;
#[cfg(target_family = "wasm")]
use web_time::Instant;

use gpui::Modifiers;

/// How long the second tap has to arrive.
///
/// Long enough not to punish a slow hand, short enough that two deliberate but
/// unrelated presses of Shift a beat apart are not read as one gesture. This is
/// the same figure the desktop conventions use for a double click, and for the
/// same reason: it is about how fast people repeat something on purpose.
const WINDOW: Duration = Duration::from_millis(400);

/// Which modifier was tapped twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tap {
    Shift,
    /// Ctrl on Linux and Windows, Command on macOS — whichever one this
    /// platform's shortcuts are built on, so the gesture reads the same way
    /// everywhere without naming a different key per platform.
    Secondary,
}

/// The watcher. One per view; costs nothing when nobody is tapping anything.
#[derive(Debug, Default)]
pub struct Taps {
    /// What the modifiers were last time, so that arming can be limited to a
    /// press that started from nothing at all.
    prev: Modifiers,
    /// The lone modifier currently down, where exactly one is and it began
    /// from nothing.
    held: Option<Tap>,
    /// Whether something happened while it was held. A spoiled hold is not a
    /// tap however it ends.
    spoiled: bool,
    /// The last completed tap, waiting for its second.
    last: Option<(Tap, Instant)>,
}

impl Taps {
    /// The modifiers changed. Answers a tap when this press completes one.
    pub fn changed(&mut self, mods: Modifiers, now: Instant) -> Option<Tap> {
        let prev = std::mem::replace(&mut self.prev, mods);

        if mods.modified() {
            // Arm only on the way up from nothing. Letting go of Shift in the
            // middle of `Ctrl Shift Z` leaves one modifier down, and reading
            // that as the start of a Ctrl tap would fire the palette on the
            // tail of an ordinary chord.
            self.held = if prev.modified() { None } else { lone(mods) };
            if !prev.modified() {
                self.spoiled = false;
            }
            return None;
        }

        // Everything is up.
        let held = self.held.take();
        let spoiled = std::mem::replace(&mut self.spoiled, false);
        let tap = held.filter(|_| !spoiled)?;

        match self.last.take() {
            // The second one. `last` is left empty rather than replaced, so a
            // third tap starts a fresh pair instead of firing again.
            Some((first, at)) if first == tap && now.duration_since(at) <= WINDOW => Some(tap),
            _ => {
                self.last = Some((tap, now));
                None
            }
        }
    }

    /// Something else happened. Whatever is being held is no longer a tap.
    ///
    /// Called from every key press and every mouse press. Cheap enough to call
    /// unconditionally, which is what keeps it from being forgotten at one of
    /// the several places a press arrives.
    pub fn spoil(&mut self) {
        if self.held.is_some() {
            self.spoiled = true;
        }
    }

    /// Forget everything, including a tap that was waiting for its second.
    ///
    /// For when the keyboard stops being the board's — a note being typed
    /// into, a palette already open. Somebody typing capitals taps Shift
    /// constantly, and half a gesture left over from before is one that
    /// completes on the first Shift they press afterwards.
    pub fn forget(&mut self) {
        *self = Self::default();
    }
}

/// The one modifier that is down, where exactly one is and it is one of the two
/// this watches.
fn lone(mods: Modifiers) -> Option<Tap> {
    let down = [mods.control, mods.alt, mods.shift, mods.platform, mods.function]
        .into_iter()
        .filter(|d| *d)
        .count();
    if down != 1 {
        return None;
    }
    if mods.shift {
        Some(Tap::Shift)
    } else if mods.secondary() {
        Some(Tap::Secondary)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shift() -> Modifiers {
        Modifiers { shift: true, ..Default::default() }
    }

    fn none() -> Modifiers {
        Modifiers::default()
    }

    /// Press and release, at a moment.
    fn tap(taps: &mut Taps, mods: Modifiers, at: Instant) -> Option<Tap> {
        taps.changed(mods, at);
        taps.changed(none(), at)
    }

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn two_taps_in_quick_succession_are_the_gesture() {
        let mut taps = Taps::default();
        let start = t0();
        assert_eq!(tap(&mut taps, shift(), start), None, "one tap is not the gesture");
        assert_eq!(tap(&mut taps, shift(), start + Duration::from_millis(150)), Some(Tap::Shift));
    }

    #[test]
    fn two_taps_too_far_apart_are_two_taps() {
        let mut taps = Taps::default();
        let start = t0();
        tap(&mut taps, shift(), start);
        assert_eq!(tap(&mut taps, shift(), start + Duration::from_millis(900)), None);
    }

    #[test]
    fn a_third_tap_does_not_fire_again_off_the_second() {
        // Otherwise holding a conversation in capitals would open the palette
        // on every other letter.
        let mut taps = Taps::default();
        let start = t0();
        tap(&mut taps, shift(), start);
        assert_eq!(tap(&mut taps, shift(), start + Duration::from_millis(100)), Some(Tap::Shift));
        assert_eq!(tap(&mut taps, shift(), start + Duration::from_millis(200)), None);
    }

    #[test]
    fn a_letter_typed_under_the_modifier_spoils_it() {
        // Shift+A twice quickly is somebody typing, not somebody gesturing.
        let mut taps = Taps::default();
        let start = t0();
        for i in 0..2 {
            taps.changed(shift(), start + Duration::from_millis(i * 100));
            taps.spoil();
            assert_eq!(taps.changed(none(), start + Duration::from_millis(i * 100 + 10)), None);
        }
    }

    #[test]
    fn a_press_of_the_mouse_under_the_modifier_spoils_it_too() {
        // A shift-drag is the other way a modifier is held for a while.
        let mut taps = Taps::default();
        let start = t0();
        taps.changed(shift(), start);
        taps.spoil();
        taps.changed(none(), start + Duration::from_millis(50));
        assert_eq!(tap(&mut taps, shift(), start + Duration::from_millis(100)), None);
    }

    #[test]
    fn the_two_modifiers_are_different_gestures() {
        let mut taps = Taps::default();
        let start = t0();
        let ctrl = Modifiers::secondary_key();
        tap(&mut taps, shift(), start);
        // A tap of the other one does not complete the first one's pair.
        assert_eq!(tap(&mut taps, ctrl, start + Duration::from_millis(100)), None);
        assert_eq!(tap(&mut taps, ctrl, start + Duration::from_millis(200)), Some(Tap::Secondary));
    }

    #[test]
    fn a_chord_unwinding_does_not_arm_the_modifier_left_behind() {
        // Letting go of Shift at the end of `Ctrl Shift Z` leaves Ctrl down.
        // Reading that as the start of a Ctrl tap would fire the palette on
        // the tail of an ordinary shortcut.
        let mut taps = Taps::default();
        let start = t0();
        let ctrl = Modifiers::secondary_key();
        let both = Modifiers { shift: true, ..Modifiers::secondary_key() };
        taps.changed(ctrl, start);
        taps.changed(both, start + Duration::from_millis(20));
        taps.changed(ctrl, start + Duration::from_millis(40));
        assert_eq!(taps.changed(none(), start + Duration::from_millis(60)), None);
        // And nothing was left half-armed for the next press to complete.
        assert_eq!(tap(&mut taps, ctrl, start + Duration::from_millis(80)), None);
    }

    #[test]
    fn two_modifiers_at_once_are_not_a_tap_of_either() {
        let mut taps = Taps::default();
        let start = t0();
        let both = Modifiers { shift: true, ..Modifiers::secondary_key() };
        assert_eq!(tap(&mut taps, both, start), None);
        assert_eq!(tap(&mut taps, both, start + Duration::from_millis(100)), None);
    }

    #[test]
    fn forgetting_drops_a_tap_that_was_waiting_for_its_second() {
        // What stops a Shift pressed before opening a note from completing a
        // gesture with the first capital typed inside it.
        let mut taps = Taps::default();
        let start = t0();
        tap(&mut taps, shift(), start);
        taps.forget();
        assert_eq!(tap(&mut taps, shift(), start + Duration::from_millis(100)), None);
    }

    #[test]
    fn a_modifier_this_does_not_watch_is_not_a_tap() {
        let mut taps = Taps::default();
        let alt = Modifiers { alt: true, ..Default::default() };
        let start = t0();
        assert_eq!(tap(&mut taps, alt, start), None);
        assert_eq!(tap(&mut taps, alt, start + Duration::from_millis(100)), None);
    }
}
