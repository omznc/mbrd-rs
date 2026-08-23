//! The four faint marks that appear beside a card, and what dragging one does.
//!
//! Kept apart from the gesture pipeline for the reason `grips.rs` is, and it is
//! the same reason: which mark the pointer is over is arithmetic, and written
//! here it can be tested by asserting points rather than by driving a window.
//!
//! ## An anchor is a screen-space idea, and it sits *outside* the card
//!
//! Like a grip, a mark is a fixed number of pixels across whether the board is
//! at 10% or 500% — you aim a pointer at it, so it is measured in pointer
//! units. Unlike a grip it is offset *away* from the card rather than sitting
//! on its edge, and that is the whole design: a grip is part of the card's
//! outline and an anchor is a place the card is offering you to start a rope
//! from. Putting them in the same place would make one press mean two things.
//!
//! [`GAP`] is what keeps them apart. It is larger than [`grips::REACH`], so the
//! band a grip answers to and the band an anchor answers to do not overlap —
//! and where they very nearly do, the gesture pipeline asks about grips first.
//!
//! [`grips::REACH`]: crate::grips::REACH

use mbrd_core::geometry::{Point, Rect};
use mbrd_core::rope::Side;
use mbrd_core::viewport::Viewport;

/// How far outside the card's edge the middle of a mark sits, in pixels.
///
/// Comfortably past [`crate::grips::REACH`], so that a selected card — which
/// has both — never leaves a press ambiguous. A grip's edge band reaches a
/// reach *outside* the card as well as inside it, so the clearance has to be
/// measured from there rather than from the card's own edge.
pub const GAP: f32 = 18.0;

/// How far from a mark still counts as being on it.
///
/// Smaller than a grip's reach on purpose. A grip is the more common gesture
/// and the harder thing to re-aim at once you have missed; an anchor being
/// slightly fussier is the right way round.
pub const REACH: f32 = 8.0;

/// How wide the mark is drawn.
pub const DOT: f32 = 7.0;

/// The smallest a card can be on screen and still be offered anchors.
///
/// Below this the four marks would be further apart than the card is wide, and
/// a card wearing a halo of dots larger than itself reads as noise rather than
/// as an offer.
pub const TOO_SMALL: f32 = 30.0;

/// The bug this exists to prevent: a selected card wears both a grip and a
/// mark on every edge, and a press in a band that answers to each of them means
/// whichever the pipeline happens to ask about first. Keeping the bands apart
/// makes that order a tie-break rather than the whole answer.
///
/// Checked here rather than in a test, because these are constants and there is
/// no point finding out at run time: changing either number to something that
/// overlaps stops the build.
const _: () = assert!(GAP - REACH >= crate::grips::REACH);

/// Where a mark sits on screen, as a point.
pub fn spot(side: Side, card: Rect, vp: &Viewport) -> Point {
    let on_card = vp.to_screen(side.spot(card));
    let (dx, dy) = out(side);
    Point { x: on_card.x + dx * GAP, y: on_card.y + dy * GAP }
}

/// Which mark the pointer is on, if any.
///
/// `None` for a card too small to wear them, which is the same rule the painter
/// applies — something you cannot see must not be something you can press.
pub fn at(pointer: Point, card: Rect, vp: &Viewport) -> Option<Side> {
    if too_small(card, vp) {
        return None;
    }
    Side::ALL.into_iter().find(|&side| {
        let mark = spot(side, card, vp);
        (pointer.x - mark.x).abs() <= REACH && (pointer.y - mark.y).abs() <= REACH
    })
}

/// Whether the pointer is still inside the band a card's marks live in.
///
/// The card's own rectangle grown by as far as a mark can be pressed, in
/// screen units. Not the four dots on their own: the ten pixels between the
/// card's edge and the near side of a mark are on the way *to* the mark, and a
/// hover that dropped while crossing them would take the marks away exactly as
/// you reached for one. Grown rather than cut to shape for the same reason —
/// the corners are dead space, but they are dead space you can only get to by
/// leaving, and rounding them off buys nothing but a flicker.
///
/// `false` for a card too small to wear marks, since there is then nothing out
/// there to reach for.
pub fn reaching(pointer: Point, card: Rect, vp: &Viewport) -> bool {
    if too_small(card, vp) {
        return false;
    }
    // The screen rectangle: world y points up, so the card's high-y edge is
    // the small screen y.
    let top_left = vp.to_screen(Point { x: card.x0, y: card.y1 });
    let bottom_right = vp.to_screen(Point { x: card.x1, y: card.y0 });
    let band = GAP + REACH;
    pointer.x >= top_left.x - band
        && pointer.x <= bottom_right.x + band
        && pointer.y >= top_left.y - band
        && pointer.y <= bottom_right.y + band
}

/// Whether this card is too small on screen to wear marks.
pub fn too_small(card: Rect, vp: &Viewport) -> bool {
    card.width() * vp.zoom < TOO_SMALL || card.height() * vp.zoom < TOO_SMALL
}

/// Which way is *away from the card* on screen, for a given face.
///
/// The one place the flip lives. World y points up and screen y points down, so
/// the face called `Top` — the high-y one — is the mark drawn *above* the card,
/// at the smaller screen y. Getting this wrong does not crash; it puts the top
/// anchor underneath the card, which is the kind of thing you stare at for a
/// while.
fn out(side: Side) -> (f32, f32) {
    match side {
        Side::Left => (-1.0, 0.0),
        Side::Right => (1.0, 0.0),
        Side::Top => (0.0, -1.0),
        Side::Bottom => (0.0, 1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mbrd_core::viewport::ViewSize;

    fn card() -> Rect {
        Rect::centred(0.0, 0.0, 200.0, 120.0)
    }

    fn camera() -> Viewport {
        Viewport { size: ViewSize { width: 1000.0, height: 800.0 }, ..Viewport::default() }
    }

    #[test]
    fn every_mark_is_outside_the_card_it_belongs_to() {
        let vp = camera();
        let top_left = vp.to_screen(Point { x: card().x0, y: card().y1 });
        let bottom_right = vp.to_screen(Point { x: card().x1, y: card().y0 });
        for side in Side::ALL {
            let mark = spot(side, card(), &vp);
            let outside = mark.x < top_left.x
                || mark.x > bottom_right.x
                || mark.y < top_left.y
                || mark.y > bottom_right.y;
            assert!(outside, "{} landed on the card at {mark:?}", side.as_str());
        }
    }

    #[test]
    fn the_top_mark_is_drawn_above_the_card() {
        // World y points up and screen y points down, and this is the assertion
        // that catches the flip being missed: the mark for the high-y face has
        // to be at the *small* screen y.
        let vp = camera();
        let middle = vp.to_screen(Point { x: 0.0, y: 0.0 });
        assert!(spot(Side::Top, card(), &vp).y < middle.y);
        assert!(spot(Side::Bottom, card(), &vp).y > middle.y);
        assert!(spot(Side::Left, card(), &vp).x < middle.x);
        assert!(spot(Side::Right, card(), &vp).x > middle.x);
    }

    #[test]
    fn pointing_at_a_mark_finds_it() {
        let vp = camera();
        for side in Side::ALL {
            assert_eq!(at(spot(side, card(), &vp), card(), &vp), Some(side));
        }
    }

    #[test]
    fn the_middle_of_a_card_is_not_an_anchor() {
        let vp = camera();
        assert_eq!(at(vp.to_screen(Point { x: 0.0, y: 0.0 }), card(), &vp), None);
    }

    #[test]
    fn the_marks_are_inside_the_band_the_card_keeps_them_for() {
        // The whole point: a pointer that has left the card but is on one of
        // its marks is still reaching for that card.
        let vp = camera();
        for side in Side::ALL {
            assert!(reaching(spot(side, card(), &vp), card(), &vp), "{}", side.as_str());
        }
    }

    #[test]
    fn the_gap_between_the_card_and_a_mark_is_inside_the_band_too() {
        // Halfway out to the right-hand mark, which is nothing at all — not on
        // the card, not on the dot — and is the pixel the marks used to vanish
        // at.
        let vp = camera();
        let edge = vp.to_screen(Point { x: card().x1, y: 0.0 });
        assert!(reaching(Point { x: edge.x + GAP / 2.0, y: edge.y }, card(), &vp));
    }

    #[test]
    fn past_the_marks_is_outside_the_band() {
        let vp = camera();
        let mark = spot(Side::Right, card(), &vp);
        assert!(!reaching(Point { x: mark.x + REACH + 1.0, y: mark.y }, card(), &vp));
    }

    #[test]
    fn a_card_too_small_to_wear_marks_keeps_no_band_either() {
        let far = Viewport { zoom: 0.1, ..camera() };
        assert!(!reaching(far.to_screen(Point { x: 0.0, y: 0.0 }), card(), &far));
    }

    #[test]
    fn a_card_too_small_on_screen_has_no_anchors_at_all() {
        let far = Viewport { zoom: 0.1, ..camera() };
        assert_eq!(at(spot(Side::Right, card(), &far), card(), &far), None);
    }

    #[test]
    fn marks_stay_the_same_size_however_far_in_the_camera_is() {
        // The gap is in pixels, so a card at 500% wears its marks exactly as
        // far out as one at 100% — which is the point of testing in screen
        // space rather than in world units.
        let close = Viewport { zoom: 5.0, ..camera() };
        let near = camera();
        let edge_close = close.to_screen(Point { x: card().x1, y: 0.0 });
        let edge_near = near.to_screen(Point { x: card().x1, y: 0.0 });
        assert!(
            ((spot(Side::Right, card(), &close).x - edge_close.x)
                - (spot(Side::Right, card(), &near).x - edge_near.x))
                .abs()
                < 0.001
        );
    }
}
