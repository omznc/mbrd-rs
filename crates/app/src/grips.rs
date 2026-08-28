//! The handles around a selected card, and what dragging one does.
//!
//! Kept apart from the gesture pipeline for the same reason `geometry` is kept
//! apart from `viewport`: which handle the pointer is over, and what a drag of
//! it should do to a rectangle, are both arithmetic. Written here they can be
//! tested by asserting rectangles; written inline in the pipeline they could
//! only be tested by driving a window.
//!
//! ## Grips are a screen-space idea
//!
//! A handle is eight pixels across whether the board is at 10% or 500%, because
//! it is something you aim a pointer at rather than something on the board. So
//! the *test* happens in screen pixels and the *result* is in world units, and
//! this module is the only place the two meet outside `viewport.rs`.

use mbrd_core::geometry::{cells, clamp_size, place, snap, Point, Rect};
use mbrd_core::viewport::Viewport;

/// How far from an edge or corner still counts as being on the handle.
///
/// Generous, because a card's edge is one pixel and a pointer is not that
/// accurate. The corners win ties — see [`Grip::at`] — so a larger reach here
/// costs nothing except making the corners slightly easier to hit, which is the
/// direction it should err in.
pub const REACH: f32 = 9.0;

/// The smallest a card can be on screen and still be worth putting grips on.
///
/// Below this the handles would overlap each other and the whole card would be
/// one big grip, so pressing it could never mean "move me".
pub const TOO_SMALL: f32 = 34.0;

/// How wide a corner handle is drawn.
///
/// Only the corners are *drawn*. All eight still exist, because an edge is
/// worth dragging — but an edge does not need a dot to advertise itself when
/// the whole edge is the target, and eight dots around every selected card is
/// a lot of furniture on a board whose point is the pictures.
pub const DOT: f32 = 8.0;

/// Which handle, named by the corner or edge it sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grip {
    TopLeft,
    Top,
    TopRight,
    Right,
    BottomRight,
    Bottom,
    BottomLeft,
    Left,
}

impl Grip {
    /// The four that are drawn, in the order they are drawn.
    pub const CORNERS: [Grip; 4] =
        [Grip::TopLeft, Grip::TopRight, Grip::BottomRight, Grip::BottomLeft];

    /// What the pointer should look like over this handle.
    ///
    /// Only the four corners are *drawn* — see [`DOT`] — so for the edges this
    /// is the whole of the affordance, and it is the thing that makes an
    /// undrawn edge handle discoverable at all: the pointer changes as it
    /// crosses onto the edge, which is how somebody finds out an edge can be
    /// dragged without being told.
    ///
    /// The diagonals are named for the axis they lie along rather than for the
    /// corner they sit on, which is why two corners share each of them.
    pub fn cursor(self) -> gpui::CursorStyle {
        use gpui::CursorStyle;
        match self {
            Grip::TopLeft | Grip::BottomRight => CursorStyle::ResizeUpLeftDownRight,
            Grip::TopRight | Grip::BottomLeft => CursorStyle::ResizeUpRightDownLeft,
            Grip::Top | Grip::Bottom => CursorStyle::ResizeUpDown,
            Grip::Left | Grip::Right => CursorStyle::ResizeLeftRight,
        }
    }

    /// Whether this handle moves the left edge, and the top, and so on.
    ///
    /// The whole of what distinguishes the eight of them, which is why the
    /// resize below is one function rather than eight.
    fn edges(self) -> (bool, bool, bool, bool) {
        // left, right, top, bottom — "top" meaning the high-y side, since y
        // points up in world space.
        match self {
            Grip::TopLeft => (true, false, true, false),
            Grip::Top => (false, false, true, false),
            Grip::TopRight => (false, true, true, false),
            Grip::Right => (false, true, false, false),
            Grip::BottomRight => (false, true, false, true),
            Grip::Bottom => (false, false, false, true),
            Grip::BottomLeft => (true, false, false, true),
            Grip::Left => (true, false, false, false),
        }
    }

    /// Whether dragging it can change the width, and the height.
    pub fn moves(self) -> (bool, bool) {
        let (l, r, t, b) = self.edges();
        (l || r, t || b)
    }

    /// Where the handle sits on screen, as a point.
    pub fn spot(self, card: Rect, vp: &Viewport) -> Point {
        let (l, r, t, b) = self.edges();
        let x = if l {
            card.x0
        } else if r {
            card.x1
        } else {
            (card.x0 + card.x1) / 2.0
        };
        let y = if t {
            card.y1
        } else if b {
            card.y0
        } else {
            (card.y0 + card.y1) / 2.0
        };
        vp.to_screen(Point { x, y })
    }

    /// Which handle the pointer is on, if any.
    ///
    /// **Corners before edges.** They overlap, and a corner is the harder thing
    /// to aim at and the more useful thing to hit.
    ///
    /// An edge counts along its **whole run**, not just at its midpoint. That
    /// is what lets the drawing drop to four corner dots without losing
    /// anything: the edges are still there to grab, they are simply where you
    /// would already have aimed — at the edge — rather than at a dot sitting on
    /// it. Aiming at the midpoint of a long edge was the fiddliest thing on the
    /// board and it was fiddly for no reason.
    pub fn at(pointer: Point, card: Rect, vp: &Viewport) -> Option<Grip> {
        let top_left = vp.to_screen(Point { x: card.x0, y: card.y1 });
        let bottom_right = vp.to_screen(Point { x: card.x1, y: card.y0 });
        let (left, right) = (top_left.x, bottom_right.x);
        let (top, bottom) = (top_left.y, bottom_right.y);
        if right - left < TOO_SMALL || bottom - top < TOO_SMALL {
            return None;
        }
        if let Some(grip) = Grip::CORNERS.into_iter().find(|g| near(pointer, g.spot(card, vp))) {
            return Some(grip);
        }
        // Beside the card as well as on it, so that a pointer a few pixels
        // outside the edge is still on the edge. The band eats into the card by
        // the same reach, which is what `TOO_SMALL` keeps honest: below that
        // size there would be no middle left to press for a move.
        let along_x = pointer.x >= left - REACH && pointer.x <= right + REACH;
        let along_y = pointer.y >= top - REACH && pointer.y <= bottom + REACH;
        let close = |a: f32, b: f32| (a - b).abs() <= REACH;
        if along_y && close(pointer.x, left) {
            return Some(Grip::Left);
        }
        if along_y && close(pointer.x, right) {
            return Some(Grip::Right);
        }
        // Screen y grows down, so the small-y edge is the one named `Top`.
        if along_x && close(pointer.y, top) {
            return Some(Grip::Top);
        }
        if along_x && close(pointer.y, bottom) {
            return Some(Grip::Bottom);
        }
        None
    }
}

fn near(a: Point, b: Point) -> bool {
    (a.x - b.x).abs() <= REACH && (a.y - b.y).abs() <= REACH
}

/// What a card becomes when a handle is dragged to `pointer`.
///
/// The **opposite** edge stays put, which is the whole rule: dragging the right
/// edge moves the right edge, and the card's centre moves with it because a
/// centre is derived from two edges rather than being one of them. Getting this
/// backwards produces a card that slides away as you resize it, which is the
/// classic bug here.
///
/// `keep` is a shape to hold, as width over height, and it is a number rather
/// than a flag because the shape worth holding is not always the one the card
/// started with: a photograph should keep **the picture's** proportions, and a
/// card that has been stretched away from them should come back to them rather
/// than preserve the stretch. `None` resizes freely.
///
/// `to_grid` snaps the edges, not the centre, because it is the edges somebody
/// is lining up — and **all four of them**, not only the two being dragged.
/// The anchored edge used to be left alone on the reasoning that a resize
/// should not move the side nobody touched. But the size is the gap between
/// the two, so an anchor sitting mid-square guarantees a size that is not a
/// whole number of cells and a card that can never come square however
/// carefully its other edge is placed. On a board that is already snapped the
/// anchor is on a dot and rounding it does nothing; on one that is not, this
/// is what makes the first resize tidy the card instead of preserving the mess.
pub fn resized(
    grip: Grip,
    start: Rect,
    pointer: Point,
    keep: Option<f32>,
    to_grid: Option<f32>,
) -> Rect {
    let (l, r, t, b) = grip.edges();
    let mut out = start;

    let to_dot = |v: f32| match to_grid {
        Some(step) => snap(v, step),
        None => v,
    };
    // The dragged edges follow the pointer; the other two are put on the
    // nearest dot where they stand. Both go through the same rounding, so a
    // card that was already square comes out of this untouched.
    out.x0 = to_dot(if l { pointer.x } else { out.x0 });
    out.x1 = to_dot(if r { pointer.x } else { out.x1 });
    out.y1 = to_dot(if t { pointer.y } else { out.y1 });
    out.y0 = to_dot(if b { pointer.y } else { out.y0 });

    // Dragging an edge past its opposite flips the card rather than inverting
    // it, which is what a rectangle given by edges would otherwise do — and an
    // inverted rectangle is one that draws as nothing and hit-tests as nothing.
    if out.x1 < out.x0 {
        std::mem::swap(&mut out.x0, &mut out.x1);
    }
    if out.y1 < out.y0 {
        std::mem::swap(&mut out.y0, &mut out.y1);
    }

    // Whole cells on a snapped board, the plain clamp otherwise. Since both
    // edges of a dragged axis are now on dots, the gap between them is already
    // a whole number of cells and this changes nothing there — it is the
    // aspect-derived sizes below, and the `MIN_SIZE` floor, that need it. A
    // card may not come out of a resize smaller than one square of the
    // pattern: `clamp_size` would stop it at `MIN_SIZE`, which on the default
    // lattice is smaller than a cell and a multiple of nothing.
    let size = |v: f32| match to_grid {
        Some(step) => cells(v, step),
        None => clamp_size(v),
    };
    let mut w = size(out.width());
    let mut h = size(out.height());

    if let Some(shape) = keep.filter(|s| s.is_finite() && *s > 0.0) {
        let (moves_w, moves_h) = grip.moves();

        // A corner follows whichever axis was dragged further, so the card
        // keeps up with the pointer instead of lagging on one side. An edge has
        // only one axis, and the other simply follows it.
        //
        // The derived axis goes through `size` like the dragged one, which is
        // all the grid needs from it now: a whole-cell size measured from an
        // anchor that is on a dot puts the far edge on a dot too, so there is
        // no separate edge-rounding pass to keep in step with this one. It
        // costs the ratio a little — a shape can only be held as exactly as
        // two whole numbers of cells can express it — and that is the trade a
        // snapped board is asking for.
        if moves_w && moves_h {
            if w / shape > h {
                h = size(w / shape);
            } else {
                w = size(h * shape);
            }
        } else if moves_w {
            h = size(w / shape);
        } else {
            w = size(h * shape);
        }
    }

    // Grow away from whichever edge is staying put. Where neither is — the
    // other axis of an edge grip, which a held shape has just resized without
    // anybody dragging it — grow about the centre, which is the only answer
    // that does not pick a side arbitrarily.
    let (x0, x1) = span(out.x0, out.x1, w, l, r);
    let (y0, y1) = span(out.y0, out.y1, h, b, t);
    let out = Rect { x0, y0, x1, y1 };

    // That centre-grown axis is the one case where two dots and a whole-cell
    // size still do not land on a dot: a card three cells tall has its middle
    // half a cell off the pattern, and growing a two-cell size about that
    // middle puts both new edges mid-square. So the axis nobody dragged is
    // placed rather than centred — it keeps the middle it had to within half a
    // cell, and it keeps the lattice exactly.
    match to_grid {
        None => out,
        Some(step) => {
            let centre = out.centre();
            let x = if l || r { centre.x } else { place(centre.x, w, step) };
            let y = if t || b { centre.y } else { place(centre.y, h, step) };
            Rect::centred(x, y, w, h)
        }
    }
}

/// One axis of the above: keep the anchored edge, put the other `size` away.
fn span(lo: f32, hi: f32, size: f32, moving_lo: bool, moving_hi: bool) -> (f32, f32) {
    match (moving_lo, moving_hi) {
        // The low edge moved, so the high one is the anchor.
        (true, false) => (hi - size, hi),
        (false, true) => (lo, lo + size),
        _ => {
            let middle = (lo + hi) / 2.0;
            (middle - size / 2.0, middle + size / 2.0)
        }
    }
}

#[cfg(test)]
mod tests {

    /// Every one. Corners and edges both, since all eight can be dragged —
    /// only the four corners are *drawn*, which is why this is here rather
    /// than beside `CORNERS`.
    const ALL: [Grip; 8] = [
        Grip::TopLeft,
        Grip::Top,
        Grip::TopRight,
        Grip::Right,
        Grip::BottomRight,
        Grip::Bottom,
        Grip::BottomLeft,
        Grip::Left,
    ];

    #[test]
    fn every_handle_points_the_way_it_actually_resizes() {
        use gpui::CursorStyle;
        // For the four edges the pointer is the *whole* affordance — they are
        // draggable and deliberately not drawn — so a wrong one here is a
        // handle nobody finds. Derived from `edges` rather than listed again,
        // or this would only be the same match written twice.
        for grip in ALL {
            let (l, _, t, _) = grip.edges();
            let wanted = match grip.moves() {
                // A corner. The diagonal it lies along is decided by whether
                // the two edges it moves are on the same side of the card.
                (true, true) if l == t => CursorStyle::ResizeUpLeftDownRight,
                (true, true) => CursorStyle::ResizeUpRightDownLeft,
                (true, false) => CursorStyle::ResizeLeftRight,
                (false, true) => CursorStyle::ResizeUpDown,
                (false, false) => unreachable!("a handle that resizes nothing"),
            };
            assert_eq!(grip.cursor(), wanted, "{grip:?} points the wrong way");
        }
    }

    #[test]
    fn no_two_handles_move_the_same_pair_of_edges() {
        // What makes eight handles eight rather than one function called
        // eight times: each has to move a different combination, or two of
        // them do the same thing and one of them is a hole in the card's edge
        // that answers to nothing useful.
        for (i, a) in ALL.iter().enumerate() {
            assert!(a.edges() != (false, false, false, false), "{a:?} moves no edge at all");
            for b in &ALL[i + 1..] {
                assert_ne!(a.edges(), b.edges(), "{a:?} and {b:?} do the same thing");
            }
        }
    }
    use super::*;
    use mbrd_core::model::{MAX_SIZE, MIN_SIZE};
    use mbrd_core::viewport::ViewSize;

    fn card() -> Rect {
        Rect::centred(0.0, 0.0, 200.0, 100.0)
    }

    fn camera() -> Viewport {
        Viewport { size: ViewSize { width: 1000.0, height: 800.0 }, ..Viewport::default() }
    }

    fn point(x: f32, y: f32) -> Point {
        Point { x, y }
    }

    #[test]
    fn dragging_an_edge_leaves_the_opposite_one_where_it_was() {
        let out = resized(Grip::Right, card(), point(300.0, 0.0), None, None);
        assert_eq!(out.x0, -100.0, "the left edge moved");
        assert_eq!(out.x1, 300.0);
        assert_eq!(out.height(), 100.0, "the other axis should not have changed");

        let out = resized(Grip::Left, card(), point(-300.0, 0.0), None, None);
        assert_eq!(out.x1, 100.0, "the right edge moved");
        assert_eq!(out.x0, -300.0);
    }

    #[test]
    fn dragging_a_corner_moves_two_edges_and_pins_the_other_two() {
        let out = resized(Grip::TopRight, card(), point(400.0, 300.0), None, None);
        assert_eq!((out.x0, out.y0), (-100.0, -50.0), "the far corner should be pinned");
        assert_eq!((out.x1, out.y1), (400.0, 300.0));
    }

    #[test]
    fn a_held_shape_is_the_shape_that_comes_out() {
        let before = card();
        let shape = before.width() / before.height();
        let out = resized(Grip::TopRight, before, point(600.0, 60.0), Some(shape), None);
        assert!((out.width() / out.height() - shape).abs() < 0.001, "{out:?}");
    }

    #[test]
    fn a_shape_the_card_is_not_already_is_the_one_it_becomes() {
        // A photograph in a card somebody stretched. Holding the *picture's*
        // shape has to pull the card back to it, which is the whole reason
        // this is a ratio and not a flag.
        let out = resized(Grip::BottomRight, card(), point(400.0, -400.0), Some(1.0), None);
        assert!((out.width() - out.height()).abs() < 0.001, "{out:?}");
    }

    #[test]
    fn a_proportional_corner_drag_still_pins_the_far_corner() {
        // The card must not swell away from the pointer in every direction —
        // it is the corner opposite the one being dragged that stays put.
        let out = resized(Grip::TopRight, card(), point(600.0, 60.0), Some(2.0), None);
        assert_eq!((out.x0, out.y0), (-100.0, -50.0), "{out:?}");
    }

    #[test]
    fn a_proportional_edge_drag_grows_the_other_axis_about_the_centre() {
        // Otherwise dragging the right edge would also march the card
        // downward, which looks like a bug even though it is not one.
        let out = resized(Grip::Right, card(), point(300.0, 0.0), Some(2.0), None);
        assert!((out.centre().y - 0.0).abs() < 0.001, "{out:?}");
        assert!((out.width() / out.height() - 2.0).abs() < 0.001);
    }

    #[test]
    fn dragging_an_edge_past_its_opposite_flips_rather_than_inverting() {
        let out = resized(Grip::Right, card(), point(-500.0, 0.0), None, None);
        assert!(out.x1 > out.x0, "an inverted card draws as nothing: {out:?}");
        assert!(out.width() >= MIN_SIZE);
    }

    #[test]
    fn a_card_cannot_be_dragged_smaller_or_larger_than_a_board_allows() {
        let out = resized(Grip::Right, card(), point(-99.0, 0.0), None, None);
        assert_eq!(out.width(), MIN_SIZE);
        // And the pinned edge is still pinned, so it shrank towards it.
        assert_eq!(out.x0, -100.0);

        let out = resized(Grip::Right, card(), point(1e9, 0.0), None, None);
        assert_eq!(out.width(), MAX_SIZE);
    }

    /// Every edge of a rectangle, counted in grid cells from the origin.
    ///
    /// A whole number is on a dot and anything else is not, which is the only
    /// question a snapped resize has to answer.
    fn in_cells(r: Rect, step: f32) -> [f32; 4] {
        [r.x0 / step, r.x1 / step, r.y0 / step, r.y1 / step]
    }

    fn all_whole(r: Rect, step: f32) -> bool {
        in_cells(r, step).iter().all(|v| (v - v.round()).abs() < 0.001)
    }

    #[test]
    fn snapping_lines_up_the_edge_being_dragged() {
        // 287 is a hair nearer 256 than 320, and snapping rounds.
        let out = resized(Grip::Right, card(), point(287.0, 0.0), None, Some(64.0));
        assert_eq!(out.x1, 256.0);
        // And the anchored edge comes onto the lattice with it. The card in
        // this test starts at -100, which is mid-square: leaving it there — as
        // this used to — would hand back a card 356 wide, five and a half
        // cells, which is exactly the size a snapped board is not allowed to
        // produce.
        assert_eq!(out.x0, -128.0, "the anchored edge is squared up too");
        assert_eq!(out.width(), 384.0, "six whole cells");
        assert!(all_whole(out, 64.0), "{out:?}");
    }

    #[test]
    fn keeping_aspect_while_snapping_a_dragged_corner_lands_the_corrected_edge_on_the_grid_too() {
        // width (driven directly by the pointer) beats height, so the aspect
        // lock overwrites height — and that overwritten edge must land on the
        // grid too, not at whatever the raw ratio produced.
        let out = resized(Grip::TopRight, card(), point(287.0, 30.0), Some(1.6), Some(64.0));
        assert_eq!((out.x0, out.x1), (-128.0, 256.0), "the driven axis, both edges squared");
        assert_eq!(out.y0, -64.0, "the anchored edge, squared where it stood");
        assert_eq!(out.y1, 192.0, "the aspect-derived edge must also sit on the grid");
        assert!(all_whole(out, 64.0), "{out:?}");
    }

    #[test]
    fn keeping_aspect_while_snapping_a_dragged_corner_handles_the_other_overwritten_axis() {
        // Same case, opposite branch: height drives, so width gets overwritten.
        let out = resized(Grip::TopLeft, card(), point(-120.0, 250.0), Some(1.6), Some(64.0));
        assert_eq!(out.x1, 128.0, "the anchored edge, squared where it stood");
        assert_eq!((out.y0, out.y1), (-64.0, 256.0), "the driven axis, both edges squared");
        assert_eq!(out.x0, -384.0, "the aspect-derived edge must also sit on the grid");
        assert!(all_whole(out, 64.0), "{out:?}");
    }

    #[test]
    fn keeping_aspect_while_snapping_a_single_edge_snaps_the_derived_size_not_a_position() {
        // Right has no edge on the vertical axis — span grows it about the
        // centre — so there is nothing to line up on the grid except height.
        let out = resized(Grip::Right, card(), point(300.0, 0.0), Some(2.0), Some(64.0));
        assert_eq!((out.x0, out.x1), (-128.0, 320.0), "the driven axis, both edges squared");
        assert_eq!(out.height(), 256.0, "the derived size is a whole number of cells");
        assert!(
            (out.centre().y - 0.0).abs() < 0.001,
            "still centred, per the existing centring rule"
        );
        assert!(all_whole(out, 64.0), "{out:?}");
    }

    #[test]
    fn a_snapped_resize_lands_every_edge_on_the_grid_from_every_handle() {
        // The centre-grown axis is the one this is really about: a card an odd
        // number of cells tall has its middle half a cell off the pattern, so
        // growing the other axis about that middle used to put both of its
        // edges mid-square — a resize that snapped and still came out halfway.
        let step = 64.0;
        // Deliberately mid-square to begin with, and an odd number of cells
        // once it has been squared up, so neither axis starts out convenient.
        let start = Rect::centred(30.0, -18.0, 170.0, 210.0);
        for grip in ALL {
            for at in [point(287.0, 30.0), point(-311.0, -122.0), point(41.0, 260.0)] {
                for keep in [None, Some(1.6), Some(0.5)] {
                    let out = resized(grip, start, at, keep, Some(step));
                    assert!(all_whole(out, step), "{grip:?} at {at:?} keeping {keep:?}: {out:?}");
                    assert!(out.width() >= step, "{grip:?}: {} is under a cell", out.width());
                    assert!(out.height() >= step, "{grip:?}: {} is under a cell", out.height());
                }
            }
        }
    }

    #[test]
    fn a_snapped_card_cannot_be_dragged_smaller_than_one_square_of_the_pattern() {
        // Free, the floor is `MIN_SIZE` — which is 48, smaller than a 64 cell
        // and a multiple of nothing. Snapped, the floor has to be the cell, or
        // the smallest card on the board is one that sits across the grid.
        let out = resized(Grip::Right, card(), point(-99.0, 0.0), None, Some(64.0));
        assert_eq!(out.width(), 64.0);
        assert!(all_whole(out, 64.0), "{out:?}");
        // The pinned edge is still the pinned edge, so it shrank towards it.
        assert_eq!(out.x1, -64.0, "grown from the anchor, which was squared to -128");
    }

    #[test]
    fn a_corner_is_easier_to_hit_than_the_edges_meeting_at_it() {
        let vp = camera();
        let corner = Grip::TopRight.spot(card(), &vp);
        assert_eq!(Grip::at(corner, card(), &vp), Some(Grip::TopRight));
        // A pixel along the top edge from the corner is still the corner.
        let nearly = point(corner.x - 2.0, corner.y);
        assert_eq!(Grip::at(nearly, card(), &vp), Some(Grip::TopRight));
    }

    #[test]
    fn the_middle_of_an_edge_is_that_edge() {
        let vp = camera();
        assert_eq!(Grip::at(Grip::Top.spot(card(), &vp), card(), &vp), Some(Grip::Top));
        assert_eq!(Grip::at(Grip::Left.spot(card(), &vp), card(), &vp), Some(Grip::Left));
    }

    #[test]
    fn anywhere_along_an_edge_is_that_edge_and_not_only_its_middle() {
        // The four dots are the corners; the edges are grabbed by aiming at
        // the edge, which is where somebody aims anyway.
        let vp = camera();
        let middle = Grip::Right.spot(card(), &vp);
        let corner = Grip::TopRight.spot(card(), &vp);
        let part_way = point(middle.x, (middle.y + corner.y) / 2.0);
        assert_eq!(Grip::at(part_way, card(), &vp), Some(Grip::Right));
        // And a few pixels outside the card, which is still aiming at the edge.
        assert_eq!(Grip::at(point(part_way.x + 4.0, part_way.y), card(), &vp), Some(Grip::Right));
    }

    #[test]
    fn the_face_of_a_card_is_still_somewhere_to_press_for_a_move() {
        // The edge band eats into the card. If it ate the whole card there
        // would be no way to pick one up, which is what `TOO_SMALL` is for.
        let vp = camera();
        let box_ = Rect::centred(0.0, 0.0, 200.0, 100.0);
        for spot in [point(0.0, 0.0), point(20.0, 10.0), point(-40.0, -20.0)] {
            assert_eq!(Grip::at(vp.to_screen(spot), box_, &vp), None, "{spot:?}");
        }
    }

    #[test]
    fn the_middle_of_a_card_is_not_a_grip() {
        let vp = camera();
        assert_eq!(Grip::at(vp.to_screen(point(0.0, 0.0)), card(), &vp), None);
    }

    #[test]
    fn a_card_too_small_on_screen_has_no_grips_at_all() {
        // Otherwise every press on a distant card would be a resize and there
        // would be no way to pick one up and move it.
        let vp = Viewport { zoom: 0.1, ..camera() };
        let corner = Grip::TopRight.spot(card(), &vp);
        assert_eq!(Grip::at(corner, card(), &vp), None);
    }

    #[test]
    fn grips_stay_the_same_size_however_far_in_the_camera_is() {
        // The reach is in pixels, so a card at 500% has grips no larger than
        // one at 100% — which is the point of testing in screen space.
        let close = Viewport { zoom: 5.0, ..camera() };
        let corner = Grip::TopRight.spot(card(), &close);
        assert_eq!(
            Grip::at(point(corner.x - REACH + 1.0, corner.y), card(), &close),
            Some(Grip::TopRight)
        );
        // And a few pixels further along is the *edge* rather than still the
        // corner: the card is a thousand pixels wide at this zoom and the
        // corner did not grow with it.
        assert_eq!(
            Grip::at(point(corner.x - REACH - 4.0, corner.y), card(), &close),
            Some(Grip::Top)
        );
        // Nor did the edge band. A pointer well in from the edge is on the
        // card's face, which is what makes it something to pick up and move.
        assert_eq!(Grip::at(point(corner.x - 100.0, corner.y + REACH + 4.0), card(), &close), None);
    }
}
