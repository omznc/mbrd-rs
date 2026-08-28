//! The strip of controls along the bottom of a card that plays, and what
//! pressing each part of it means.
//!
//! Kept apart from the gesture pipeline for the reason `grips.rs` and
//! `anchor.rs` are, and it is the same reason: where each control sits and
//! which one the pointer is over are both arithmetic, and written here they can
//! be tested by asserting rectangles instead of by driving a window.
//!
//! ## The strip is a screen-space idea, and it sits *inside* the card
//!
//! Like a grip, a button is a fixed number of pixels across at every zoom,
//! because it is something you aim a pointer at. Unlike a grip it is *within*
//! the card's outline rather than on it, which is what keeps the two from
//! meaning the same press: the grips live in a band [`grips::REACH`] either
//! side of the edge, and [`INSET`] holds the strip clear of it.
//!
//! ## Dropping controls is the layout, not a fallback
//!
//! A card three hundred pixels wide fits everything; one at a hundred and fifty
//! fits a play button and somewhere to scrub. Rather than hiding the strip
//! below some width — which would make the controls appear and vanish as
//! somebody zooms — the parts come off in a fixed order and the two that matter
//! stay to the end. See [`Strip::fit`].
//!
//! [`grips::REACH`]: crate::grips::REACH

use mbrd_core::geometry::Point;

/// How far inside the card's edge the strip sits, in pixels.
///
/// Past [`crate::grips::REACH`] **plus [`PAD`]**, so the band a grip answers to
/// and the band a press on the strip answers to never overlap. It is not enough
/// to clear the grip band with the strip's own drawn geometry — [`at`] tests a
/// box grown by `PAD` on every side, so the *hit-test* band reaches `PAD` pixels
/// closer to the card's edge than the strip is drawn. Padding the target
/// without widening this margin to match would have reopened the exact bug the
/// margin exists to prevent, just a few pixels further in: a selected video
/// card wears both a grip and a strip, and a press in a band that answers to
/// two gestures means whichever was asked first, which is a coin toss dressed
/// up as an order.
pub const INSET: f32 = 14.0;

/// How much larger than its drawn size a control's hit-test box is, in pixels.
///
/// A 22–24px button and an 18px-tall slider are comfortably visible and
/// uncomfortably small to aim at — a few pixels either side of the true edge is
/// still "meant it". Grown on every side rather than only outward, because the
/// gap *between* two controls (the mute button and the volume slider above it,
/// notably — see `board_view.rs`'s hover keep-alive) is exactly where a press
/// aimed at one of them and caught the seam instead.
pub const PAD: f32 = 4.0;

/// How tall the strip is, in pixels.
pub const BAR: f32 = 26.0;

/// How wide a button is, and the gap between two of them.
///
/// A shade over [`crate::icons::ICON_LG`], the size the picture inside it
/// draws at — see `board_view.rs`'s `TRANSPORT_ICON` — so a hover has room to
/// be a wash around the picture rather than a hairline against its edge.
pub const BUTTON: f32 = 24.0;
pub const GAP: f32 = 6.0;

/// How wide the elapsed/length label is. Enough for `1:04 / 3:22`.
pub const TIME: f32 = 62.0;

/// The volume slider that appears over the mute button.
pub const VOLUME_W: f32 = 78.0;
pub const VOLUME_H: f32 = 18.0;

/// The smallest a card can be on screen and still carry a strip at all.
///
/// Below this what is left is a play button that is most of the card, which
/// reads as a card with a mistake on it rather than as a control.
pub const MIN_WIDTH: f32 = BUTTON + GAP + 44.0 + INSET * 2.0;
pub const MIN_HEIGHT: f32 = BAR + INSET * 2.0;

/// The strip's *hit-test* band — its drawn geometry grown by [`PAD`] — must not
/// reach into the band the grips answer to.
///
/// Checked here rather than in a test, because these are constants and there is
/// no point finding out at run time — the same guard `anchor.rs` puts on its
/// own clearance, for the same bug. `INSET` alone used to be enough because the
/// hit test was the drawn box; once [`at`] started testing an inflated one, the
/// true margin became `INSET - PAD`, which is what this now asserts.
const _: () = assert!(INSET - PAD > crate::grips::REACH);

/// Which face a card wears.
///
/// **This changes what is painted, not where anything is.** Both faces put the
/// same controls in the same places — a person who has learnt where the play
/// button is on a video should find it in the same corner of a voice memo — and
/// the difference is what fills the scrubber: a waveform for a recording with
/// nothing to look at, a thin progress line over the picture for everything
/// else. Making the geometry differ as well was a layout with no argument
/// behind it, and it moved the controls under somebody's pointer for no reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Face {
    /// Controls laid over the bottom of something you are looking at: a video,
    /// an animation, a record sleeve.
    Overlay,
    /// The card *is* the player. A voice memo has nothing to look at, so the
    /// scrubber draws the shape of the sound instead.
    Memo,
}

/// A rectangle in screen pixels, with `y0` at the **top**.
///
/// Deliberately not [`mbrd_core::geometry::Rect`], whose `y0` is documented as
/// the bottom edge because world y points up. Reusing it here would put two
/// opposite meanings on one field name, and the bug that follows is a control
/// drawn in one place and pressed in another.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Box2 {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Box2 {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self { x0, y0, x1, y1 }
    }

    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x0 && p.x <= self.x1 && p.y >= self.y0 && p.y <= self.y1
    }

    /// This box, grown by `amount` on every side. What [`at`] presses against
    /// instead of the drawn box itself — see [`PAD`].
    pub fn inflate(&self, amount: f32) -> Self {
        Self {
            x0: self.x0 - amount,
            y0: self.y0 - amount,
            x1: self.x1 + amount,
            y1: self.y1 + amount,
        }
    }

    /// How far along this box a point lies, as `0.0..=1.0`.
    ///
    /// Clamped, so that a pointer dragged past either end of a scrubber holds
    /// at the end rather than running off it — which is what every scrubber
    /// does and what somebody dragging quickly relies on.
    pub fn fraction(&self, x: f32) -> f32 {
        match self.width() > 0.0 {
            true => ((x - self.x0) / self.width()).clamp(0.0, 1.0),
            false => 0.0,
        }
    }

    /// Where a fraction along this box falls, in pixels.
    pub fn along(&self, fraction: f32) -> f32 {
        self.x0 + self.width() * fraction.clamp(0.0, 1.0)
    }

    /// The same rectangle the painter is about to draw into.
    ///
    /// Taking gpui's own rectangle rather than re-deriving one from the item
    /// and the camera is what keeps the strip on the card: the card's screen
    /// box is worked out once, in the cull, and both the paint and the press
    /// are measured from that one answer.
    pub fn of(bounds: gpui::Bounds<gpui::Pixels>) -> Self {
        let (x0, y0) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));
        Self {
            x0,
            y0,
            x1: x0 + f32::from(bounds.size.width),
            y1: y0 + f32::from(bounds.size.height),
        }
    }
}

/// How many bars a waveform is drawn as, along a scrubber this wide.
///
/// One bar per three pixels: a bar and a gap at a width a person can see, which
/// at a card's usual size is somewhere between forty and a hundred and sixty.
/// The ceiling is not about drawing cost — it is that past it the bars are
/// thinner than the gaps and the whole thing reads as a texture rather than as
/// a recording.
///
/// Here rather than in the painter because two places need the same answer: the
/// painter, which draws that many, and `board_view::controls_for`, which
/// resamples the stored peaks down to that many so the painter never carries
/// numbers it will not draw.
pub fn bars(track: Box2) -> usize {
    ((track.width() / 3.0).floor() as usize).clamp(1, 160)
}

/// Where each control sits, once the card's width has had its say.
///
/// The two that are never `None` are the two the strip exists for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Strip {
    /// The whole strip, for drawing a backing behind it.
    pub bar: Box2,
    pub play: Box2,
    /// The scrubber, which for a memo is also where the waveform is drawn.
    pub scrub: Box2,
    pub time: Option<Box2>,
    pub mute: Option<Box2>,
    pub looping: Option<Box2>,
}

impl Strip {
    /// Lay the controls out inside a card that is already in screen pixels.
    ///
    /// `None` for a card too small to carry them, which is the same rule the
    /// painter applies: something you cannot see must not be something you can
    /// press.
    pub fn fit(card: Box2, sound: bool) -> Option<Strip> {
        if card.width() < MIN_WIDTH || card.height() < MIN_HEIGHT {
            return None;
        }

        // Along the bottom, held clear of the edge on all three sides. The one
        // place it can be that leaves the card itself — the picture, the
        // sleeve, the name — the thing you are looking at.
        let bar =
            Box2::new(card.x0 + INSET, card.y1 - INSET - BAR, card.x1 - INSET, card.y1 - INSET);

        let middle_of = |b: &Box2, w: f32| {
            let centre = (b.y0 + b.y1) / 2.0;
            (centre - w / 2.0, centre + w / 2.0)
        };
        let (top, bottom) = middle_of(&bar, BUTTON.min(bar.height()));

        let mut left = bar.x0;
        let play = Box2::new(left, top, left + BUTTON, bottom);
        left = play.x1 + GAP;

        // What comes off, and the order it comes off in. Least useful first:
        // a loop flag is a setting you set once, a mute is reachable from the
        // menu, and the length is the one of the three you read at a glance.
        let mut right = bar.x1;
        let mut looping = None;
        let mut mute = None;
        let mut time = None;

        let room = |right: &mut f32, width: f32| {
            // The scrubber's own floor. Taking the last of the space for a
            // button would leave a track too short to aim at, which is worse
            // than not offering the button.
            if *right - (left + width + GAP) < SCRUB_MIN {
                return None;
            }
            let taken = Box2::new(*right - width, top, *right, bottom);
            *right -= width + GAP;
            Some(taken)
        };

        if let Some(b) = room(&mut right, BUTTON) {
            looping = Some(b);
        }
        // No mute on something that makes no noise. A control that cannot do
        // anything is worse than a missing one: it is a promise the card then
        // fails to keep, and on a silent clip it is also a lie about the clip.
        if sound {
            if let Some(b) = room(&mut right, BUTTON) {
                mute = Some(b);
            }
        }
        if let Some(b) = room(&mut right, TIME) {
            time = Some(b);
        }

        // The scrubber is a full-height target rather than a hairline. Aiming
        // at a two-pixel track is the single most-complained-about thing in
        // every player that has ever drawn one.
        let scrub = Box2::new(left, bar.y0, right, bar.y1);

        Some(Strip { bar, play, scrub, time, mute, looping })
    }

    /// Where the volume slider sits when it is showing.
    ///
    /// Above the mute button it belongs to, and pulled back inside the card if
    /// that would hang it off the right-hand edge. `None` when there is no mute
    /// button to hang it on, which is also when there was no room for it.
    pub fn volume(&self, card: Box2) -> Option<Box2> {
        let mute = self.mute?;
        let x1 = (mute.x1).min(card.x1 - INSET / 2.0);
        let x0 = (x1 - VOLUME_W).max(card.x0 + INSET / 2.0);
        let y1 = mute.y0 - GAP;
        Some(Box2::new(x0, y1 - VOLUME_H, x1, y1))
    }
}

/// The shortest a scrubber may be and still be worth aiming at.
const SCRUB_MIN: f32 = 44.0;

/// What a press on the strip would do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Hit {
    PlayPause,
    /// How far along the recording, `0.0..=1.0`.
    Scrub(f32),
    Mute,
    Looping,
    /// How loud, `0.0..=1.0`.
    Volume(f32),
}

/// Which control the pointer is over, if any.
///
/// **Takes the strip that was drawn rather than the card it belongs to.** The
/// alternative — laying the controls out a second time here from the item and
/// the camera — is two copies of the same arithmetic that agree until one of
/// them is changed, and the bug that follows is a button drawn in one place and
/// pressed in another. `board_view` keeps the last frame's strips for exactly
/// this, the same way it keeps the lines it drew so a rope can be pressed where
/// it actually runs.
///
/// `volume` is the slider *if it is showing*, and it is asked first because it
/// hangs over the strip: while it is open, a press where the loop button would
/// be is a press on the slider.
///
/// Every box is tested **grown by [`PAD`]**, in the same order the boxes
/// themselves are laid out in: a button is a small target and deserves a few
/// pixels of grace, and testing the inflated boxes in this order rather than
/// the drawn ones is what resolves the overlaps that grace creates — whichever
/// control is asked about first keeps the pixels it already had, so the slider
/// still wins over the loop button beneath it and the play button still wins
/// over a scrubber that now reaches a little further left.
pub fn at(pointer: Point, strip: &Strip, volume: Option<Box2>) -> Option<Hit> {
    if let Some(slider) = volume {
        if slider.inflate(PAD).contains(pointer) {
            return Some(Hit::Volume(slider.fraction(pointer.x)));
        }
    }

    if strip.play.inflate(PAD).contains(pointer) {
        return Some(Hit::PlayPause);
    }
    if strip.mute.is_some_and(|b| b.inflate(PAD).contains(pointer)) {
        return Some(Hit::Mute);
    }
    if strip.looping.is_some_and(|b| b.inflate(PAD).contains(pointer)) {
        return Some(Hit::Looping);
    }
    // The label is not a control, but it is inside the strip, and letting a
    // press fall through it onto the card would mean the card moved when
    // somebody meant to read the time.
    if strip.time.is_some_and(|b| b.inflate(PAD).contains(pointer)) {
        return Some(Hit::PlayPause);
    }
    if strip.scrub.inflate(PAD).contains(pointer) {
        return Some(Hit::Scrub(strip.scrub.fraction(pointer.x)));
    }
    None
}

/// Whether the pointer still counts as reaching for the volume slider.
///
/// The slider sits [`GAP`] above the mute button that opens it, and a hover
/// check that only asked `at` — which tests each control's own box — dropped
/// the slider the instant the pointer crossed that gap: reaching for the
/// slider was the one motion guaranteed to close it. `anchor.rs`'s
/// `reaching` doc comment describes exactly this bug for the anchor marks,
/// and the fix is the same shape: answer against the union of everything a
/// person could be on the way *to*, not against the controls one at a time.
///
/// The union of the two boxes already covers the gap continuously — `mute`
/// and `slider` sit one directly above the other, so the rectangle spanning
/// both has no seam of empty space inside it for a hover to fall through —
/// and it is grown by [`PAD`] on top of that for the same reason every other
/// hit test here is.
pub fn reaching(pointer: Point, mute: Box2, slider: Box2) -> bool {
    let x0 = mute.x0.min(slider.x0) - PAD;
    let y0 = mute.y0.min(slider.y0) - PAD;
    let x1 = mute.x1.max(slider.x1) + PAD;
    let y1 = mute.y1.max(slider.y1) + PAD;
    pointer.x >= x0 && pointer.x <= x1 && pointer.y >= y0 && pointer.y <= y1
}

/// A length, as a player writes it. `3:07`, or `1:02:44` past an hour.
pub fn clock(seconds: f32) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "0:00".into();
    }
    let total = seconds as u64;
    let (h, m, s) = (total / 3600, (total % 3600) / 60, total % 60);
    match h {
        0 => format!("{m}:{s:02}"),
        _ => format!("{h}:{m:02}:{s:02}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mbrd_core::geometry::point;

    fn body(w: f32, h: f32) -> Box2 {
        Box2::new(0.0, 0.0, w, h)
    }

    #[test]
    fn a_roomy_card_gets_everything() {
        let strip = Strip::fit(body(400.0, 300.0), true).expect("room for a strip");
        assert!(strip.time.is_some());
        assert!(strip.mute.is_some());
        assert!(strip.looping.is_some());
        assert!(strip.scrub.width() >= SCRUB_MIN);
    }

    #[test]
    fn the_parts_come_off_in_order_as_the_card_narrows() {
        // The property, rather than four hand-picked widths: at every width,
        // anything still present must be present at every larger width too.
        let mut seen_time = false;
        let mut seen_mute = false;
        let mut seen_loop = false;
        for w in (130..500).step_by(5) {
            let Some(strip) = Strip::fit(body(w as f32, 300.0), true) else { continue };
            if strip.time.is_some() {
                seen_time = true;
            }
            if strip.mute.is_some() {
                seen_mute = true;
            }
            if strip.looping.is_some() {
                seen_loop = true;
            }
            // Once something has appeared it may not go away again as the card
            // gets wider.
            assert!(!seen_time || strip.time.is_some(), "the time vanished at {w}");
            assert!(!seen_mute || strip.mute.is_some(), "the mute vanished at {w}");
            assert!(!seen_loop || strip.looping.is_some(), "the loop vanished at {w}");
            // And the two that matter are always there.
            assert!(strip.scrub.width() >= SCRUB_MIN, "the scrubber shrank away at {w}");
            assert!(strip.play.width() > 0.0);
        }
        assert!(seen_time && seen_mute && seen_loop, "nothing was ever laid out");
    }

    #[test]
    fn the_loop_is_the_first_thing_to_go() {
        // Wide enough for a scrubber and one extra control, and no more.
        let narrow = Strip::fit(body(MIN_WIDTH + BUTTON, 300.0), true).expect("a strip");
        assert!(narrow.looping.is_none(), "the least useful control survived");
    }

    #[test]
    fn a_silent_card_is_offered_no_mute_and_no_slider() {
        let card = body(400.0, 300.0);
        let strip = Strip::fit(card, false).expect("a strip");
        assert!(strip.mute.is_none(), "a mute button on something with no sound");
        assert!(strip.volume(card).is_none(), "a slider with nothing to slide");
        // And the space it would have taken goes to the scrubber rather than
        // being left as a hole in the strip.
        let loud = Strip::fit(card, true).expect("a strip");
        assert!(strip.scrub.width() > loud.scrub.width(), "the gap was left empty");
    }

    #[test]
    fn a_card_too_small_offers_nothing_rather_than_something_unaimable() {
        assert!(Strip::fit(body(MIN_WIDTH - 1.0, 300.0), true).is_none());
        assert!(Strip::fit(body(400.0, MIN_HEIGHT - 1.0), true).is_none());
    }

    #[test]
    fn nothing_in_the_strip_reaches_the_band_the_grips_answer_to() {
        // The bug the clearance exists to prevent, asserted at a real size
        // rather than only in the const assert above.
        let card = body(400.0, 300.0);
        let strip = Strip::fit(card, true).expect("a strip");
        let reach = crate::grips::REACH;
        for part in [strip.play, strip.scrub, strip.bar] {
            assert!(part.x0 >= card.x0 + reach, "{part:?} reaches the left grip band");
            assert!(part.x1 <= card.x1 - reach, "{part:?} reaches the right grip band");
            assert!(part.y1 <= card.y1 - reach, "{part:?} reaches the bottom grip band");
        }
    }

    #[test]
    fn the_controls_are_in_the_same_place_on_every_kind_of_card() {
        // The property the `Face` note argues for: a voice memo and a video of
        // the same size put their play buttons in the same spot, so learning
        // one teaches the other.
        let card = body(320.0, 96.0);
        let strip = Strip::fit(card, true).expect("a strip");
        assert!(strip.bar.y1 <= card.y1 - INSET);
        assert!(strip.bar.y0 >= card.y1 - INSET - BAR - 0.001);
    }

    fn middle_of(b: Box2) -> Point {
        point((b.x0 + b.x1) / 2.0, (b.y0 + b.y1) / 2.0)
    }

    #[test]
    fn a_press_lands_on_the_control_it_looks_like_it_is_on() {
        let strip = Strip::fit(body(400.0, 300.0), true).expect("a strip");

        assert_eq!(at(middle_of(strip.play), &strip, None), Some(Hit::PlayPause));
        assert_eq!(at(middle_of(strip.mute.unwrap()), &strip, None), Some(Hit::Mute));
        assert_eq!(at(middle_of(strip.looping.unwrap()), &strip, None), Some(Hit::Looping));

        // Halfway along the scrubber is halfway through the recording.
        let Some(Hit::Scrub(f)) = at(middle_of(strip.scrub), &strip, None) else {
            panic!("the middle of the scrubber was not a scrub");
        };
        assert!((f - 0.5).abs() < 0.01, "{f}");
    }

    #[test]
    fn a_press_on_the_card_above_the_strip_is_not_a_press_on_the_strip() {
        // Otherwise a video card could never be dragged.
        let card = body(400.0, 300.0);
        let strip = Strip::fit(card, true).expect("a strip");
        let above = point((card.x0 + card.x1) / 2.0, card.y0 + 20.0);
        assert_eq!(at(above, &strip, None), None);
    }

    #[test]
    fn the_slider_wins_over_what_is_underneath_it_while_it_is_showing() {
        let card = body(400.0, 300.0);
        let strip = Strip::fit(card, true).expect("a strip");
        let slider = strip.volume(card).expect("a slider");
        let on = middle_of(slider);

        let Some(Hit::Volume(v)) = at(on, &strip, Some(slider)) else {
            panic!("the slider did not answer while it was open");
        };
        assert!((v - 0.5).abs() < 0.01, "{v}");
        // And closed, the same point is not a volume.
        assert!(!matches!(at(on, &strip, None), Some(Hit::Volume(_))));
    }

    #[test]
    fn the_slider_stays_inside_the_card_it_belongs_to() {
        let card = body(MIN_WIDTH + 60.0, 300.0);
        let strip = Strip::fit(card, true).expect("a strip");
        if let Some(slider) = strip.volume(card) {
            assert!(slider.x0 >= card.x0, "{slider:?} hangs off the left of {card:?}");
            assert!(slider.x1 <= card.x1, "{slider:?} hangs off the right of {card:?}");
        }
    }

    #[test]
    fn a_press_just_outside_a_button_still_lands_on_it() {
        // The whole point of `PAD`: a button is a small target, and a press a
        // couple of pixels short of its true edge is still "meant it".
        let strip = Strip::fit(body(400.0, 300.0), true).expect("a strip");
        let just_outside = point(strip.play.x0 - PAD + 1.0, strip.play.y0 - PAD + 1.0);
        assert_eq!(at(just_outside, &strip, None), Some(Hit::PlayPause));
        // And past the padding — above the whole strip, where nothing else's
        // own padding could reach either — it is nothing at all.
        let too_far = point(strip.play.x0, strip.bar.y0 - PAD - 4.0);
        assert_eq!(at(too_far, &strip, None), None);
    }

    #[test]
    fn the_slider_still_wins_the_padded_overlap_with_what_is_beneath_it() {
        // Padding grows every box by the same amount, so the order `at` tests
        // them in is what has to resolve the overlap it creates — the slider
        // is asked about first, so it keeps the pixels it already had.
        let card = body(400.0, 300.0);
        let strip = Strip::fit(card, true).expect("a strip");
        let slider = strip.volume(card).expect("a slider");
        let near_edge = point(slider.x0 + 1.0, slider.y0 + 1.0);
        assert!(matches!(at(near_edge, &strip, Some(slider)), Some(Hit::Volume(_))));
    }

    #[test]
    fn reaching_for_the_slider_across_the_gap_does_not_lose_it() {
        // The bug: the slider sits `GAP` above the mute button, and a hover
        // check that only tested each control's own box dropped the slider
        // the instant the pointer entered that gap on the way to it.
        let card = body(400.0, 300.0);
        let strip = Strip::fit(card, true).expect("a strip");
        let mute = strip.mute.expect("a mute button");
        let slider = strip.volume(card).expect("a slider");
        assert!(slider.y1 < mute.y0, "set up wrong: the slider is not above the button");
        let midpoint = point((mute.x0 + mute.x1) / 2.0, (slider.y1 + mute.y0) / 2.0);
        assert!(reaching(midpoint, mute, slider), "the gap between them lost the slider");
    }

    #[test]
    fn reaching_is_still_bounded_somewhere() {
        let card = body(400.0, 300.0);
        let strip = Strip::fit(card, true).expect("a strip");
        let mute = strip.mute.expect("a mute button");
        let slider = strip.volume(card).expect("a slider");
        let far = point(mute.x0 - 500.0, mute.y0);
        assert!(!reaching(far, mute, slider), "reaching never gives up");
    }

    #[test]
    fn a_fraction_holds_at_the_ends_rather_than_running_past_them() {
        let track = Box2::new(100.0, 0.0, 200.0, 10.0);
        assert_eq!(track.fraction(50.0), 0.0);
        assert_eq!(track.fraction(250.0), 1.0);
        assert!((track.fraction(150.0) - 0.5).abs() < 0.001);
        // A track with no width does not divide by it.
        assert_eq!(Box2::new(5.0, 0.0, 5.0, 1.0).fraction(5.0), 0.0);
    }

    #[test]
    fn a_length_reads_the_way_a_player_writes_it() {
        assert_eq!(clock(0.0), "0:00");
        assert_eq!(clock(7.0), "0:07");
        assert_eq!(clock(187.0), "3:07");
        assert_eq!(clock(3764.0), "1:02:44");
        // And nothing a decoder can hand back turns into a panic or a mess.
        assert_eq!(clock(-1.0), "0:00");
        assert_eq!(clock(f32::NAN), "0:00");
    }

    #[test]
    fn a_button_is_the_same_size_at_every_zoom() {
        // A four-hundred-unit card is four hundred pixels at 1:1 and eight
        // hundred at 2:1, and the button is twenty-two either way — which is
        // the whole of what makes it something you can aim at.
        for width in [MIN_WIDTH, 400.0, 800.0, 3000.0] {
            let card = body(width, 300.0);
            let strip = Strip::fit(card, true).expect("a strip");
            assert!((strip.play.width() - BUTTON).abs() < 0.01, "at {width}");
            assert!(strip.bar.x0 >= card.x0 && strip.bar.x1 <= card.x1, "at {width}");
        }
    }

    #[test]
    fn the_strip_is_measured_from_the_rectangle_the_painter_uses() {
        let bounds = gpui::Bounds::new(
            gpui::point(gpui::px(12.0), gpui::px(30.0)),
            gpui::size(gpui::px(400.0), gpui::px(300.0)),
        );
        let card = Box2::of(bounds);
        assert_eq!((card.x0, card.y0, card.x1, card.y1), (12.0, 30.0, 412.0, 330.0));
    }
}
