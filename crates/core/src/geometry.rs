//! Points, rectangles, and the handful of questions the canvas asks of them.
//!
//! Everything here is in **world** units with **y pointing up**. Nothing in
//! this module knows what a pixel is; `viewport.rs` is the only place the flip
//! to screen space happens, and keeping that boundary sharp is what lets these
//! be tested without a window.

use crate::model::{Item, MAX_SIZE, MIN_SIZE};

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

pub fn point(x: f32, y: f32) -> Point {
    Point { x, y }
}

/// An axis-aligned box, given by its edges rather than by an origin and a size.
///
/// `y0` is the **bottom** edge and `y1` the top, because y points up. A `Rect`
/// with `y1 < y0` is empty rather than merely inverted, and `union` is what
/// builds one correctly from a set of items.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub fn new(x0: f32, y0: f32, x1: f32, y1: f32) -> Self {
        Self { x0, y0, x1, y1 }
    }

    /// The box an item occupies, **rotation included**.
    ///
    /// Remember that an item's `x`/`y` is its **centre**, which is why this is
    /// a half-width away in each direction rather than a width away in one.
    ///
    /// Rotation widens the box rather than tilting it: a card turned forty-five
    /// degrees genuinely reaches further than its own width, and every caller
    /// here wants the reach. Culling with the untilted box clips a rotated card
    /// into existence early; framing with it crops a corner off the fit;
    /// sweeping with it misses a card the rectangle plainly crossed. The tilted
    /// box is a superset, so the only cost is being slightly generous, and each
    /// of those three is a place where generous is the right way to be wrong.
    ///
    /// It is a superset for hit-testing too, which is why [`hit`] exists and
    /// this does not replace it: this narrows the field, `hit` decides.
    pub fn of_item(item: &Item) -> Self {
        if item.rot == 0.0 {
            return Self::centred(item.x, item.y, item.w, item.h);
        }
        let t = item.rot.to_radians();
        let (sin, cos) = (t.sin().abs(), t.cos().abs());
        let (hw, hh) = (item.w / 2.0, item.h / 2.0);
        Self::centred(item.x, item.y, 2.0 * (hw * cos + hh * sin), 2.0 * (hw * sin + hh * cos))
    }

    pub fn centred(cx: f32, cy: f32, w: f32, h: f32) -> Self {
        let (hw, hh) = (w / 2.0, h / 2.0);
        Self { x0: cx - hw, y0: cy - hh, x1: cx + hw, y1: cy + hh }
    }

    pub fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> f32 {
        self.y1 - self.y0
    }

    pub fn centre(&self) -> Point {
        point((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0)
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.x0 && p.x <= self.x1 && p.y >= self.y0 && p.y <= self.y1
    }

    pub fn intersects(&self, other: &Rect) -> bool {
        self.x0 <= other.x1 && other.x0 <= self.x1 && self.y0 <= other.y1 && other.y0 <= self.y1
    }

    /// Grow the box by `by` on every side. A negative `by` shrinks it, and may
    /// invert it — callers that care should test `intersects` rather than
    /// assuming the result is still a sensible box.
    pub fn inflate(&self, by: f32) -> Self {
        Self { x0: self.x0 - by, y0: self.y0 - by, x1: self.x1 + by, y1: self.y1 + by }
    }
}

/// The box that holds every one of these items, or `None` for none at all.
///
/// `None` rather than a zero-sized box at the origin, because the two mean
/// different things to a caller: "fit this" on an empty board should fall back
/// to the origin at 100%, not zoom infinitely into a point.
pub fn union<'a>(items: impl IntoIterator<Item = &'a Item>) -> Option<Rect> {
    let mut out: Option<Rect> = None;
    for item in items {
        let r = Rect::of_item(item);
        out = Some(match out {
            None => r,
            Some(acc) => Rect {
                x0: acc.x0.min(r.x0),
                y0: acc.y0.min(r.y0),
                x1: acc.x1.max(r.x1),
                y1: acc.y1.max(r.y1),
            },
        });
    }
    out
}

/// Whether a point falls on an item, taking its rotation into account.
///
/// Rotating the *point* back into the item's own frame rather than rotating the
/// item's four corners forward: same answer, a quarter of the arithmetic, and
/// it stays correct for a rectangle of any aspect.
pub fn hit(item: &Item, p: Point) -> bool {
    let (dx, dy) = (p.x - item.x, p.y - item.y);
    let (lx, ly) = if item.rot == 0.0 {
        (dx, dy)
    } else {
        // Anticlockwise-positive degrees, so undoing it is a clockwise turn.
        let t = -item.rot.to_radians();
        (dx * t.cos() - dy * t.sin(), dx * t.sin() + dy * t.cos())
    };
    lx.abs() <= item.w / 2.0 && ly.abs() <= item.h / 2.0
}

/// Hold a size to what the board can draw.
///
/// Both bounds matter and for different reasons: below `MIN_SIZE` a card has no
/// room for its own grips, and above `MAX_SIZE` it stops being an item on a
/// board and becomes a backdrop nothing can be dragged off.
pub fn clamp_size(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(MIN_SIZE, MAX_SIZE)
    } else {
        MIN_SIZE
    }
}

/// Snap a coordinate to the nearest multiple of `step`.
pub fn snap(v: f32, step: f32) -> f32 {
    if step <= 0.0 {
        v
    } else {
        (v / step).round() * step
    }
}

/// The fewest whole cells a card may be, and the most.
///
/// One cell is the floor a person can see — a card smaller than a square of
/// the pattern is the thing snapping is supposed to make impossible — but it
/// is not the only floor: `MIN_SIZE` is what a card needs to have room for its
/// own grips, and on a fine lattice one cell is under it. So the floor is
/// whichever is larger, rounded *up* to whole cells so that it is itself a
/// legal size. The ceiling rounds the other way for the same reason.
///
/// In cells rather than world units, because every caller here is about to
/// multiply by the step anyway and the clamp has to happen in the counted
/// quantity — clamping the product would put a card back off the lattice,
/// which is the bug this whole module exists to stop.
fn bounds(step: f32) -> (f32, f32) {
    let low = (MIN_SIZE / step).ceil().max(1.0);
    // Never below the floor. A step coarse enough that one cell is already
    // over `MAX_SIZE` has no legal size at all, and a card of one cell is a
    // better answer than a card of none.
    let high = (MAX_SIZE / step).floor().max(low);
    (low, high)
}

/// A size rounded to whole grid cells, and never smaller than one.
///
/// The counterpart to [`clamp_size`] for a board that is snapped: same job —
/// hold a size to what the board can draw — but the legal sizes are the whole
/// multiples of the step rather than the whole range between the two bounds.
/// A step of nothing is not a lattice, so it falls back to the free clamp.
pub fn cells(v: f32, step: f32) -> f32 {
    counted(v, step, f32::round)
}

/// [`cells`], rounding up.
///
/// For the one caller that may not round down: a note fitted to its own words
/// has a height the text decided, and taking a cell off it would cut a line
/// off the bottom. Growing is only ever air.
pub fn cells_up(v: f32, step: f32) -> f32 {
    counted(v, step, f32::ceil)
}

fn counted(v: f32, step: f32, round: fn(f32) -> f32) -> f32 {
    if !step.is_finite() || step <= 0.0 || !v.is_finite() {
        return clamp_size(v);
    }
    let (low, high) = bounds(step);
    round(v / step).clamp(low, high) * step
}

/// Where a card of this size sits when its **low edge** is on the lattice.
///
/// Takes a centre and hands one back, because a centre is what an item stores
/// — but the rounding happens on the edge, and that is the whole of the fix
/// this exists for. A lattice that takes centres puts every card of an odd
/// number of cells exactly half a cell off the pattern: three cells centred on
/// a dot reaches a cell and a half each way, so both its edges land in the
/// middle of a square. Rounding the low edge instead lands both edges on dots
/// for *every* size, and the centre falls where it falls — on a dot for an
/// even card, between two for an odd one, which is where the middle of an odd
/// card actually is.
pub fn place(centre: f32, size: f32, step: f32) -> f32 {
    if !step.is_finite() || step <= 0.0 || !centre.is_finite() || !size.is_finite() {
        return centre;
    }
    snap(centre - size / 2.0, step) + size / 2.0
}

/// A whole card put on the lattice: whole cells, and every edge on a dot.
///
/// The one function that says what "snapped" means for a rectangle, so that
/// the several places which have to answer that question — turning the setting
/// on, dropping a drag, letting go of a handle, putting a new card down —
/// cannot answer it four slightly different ways. Takes and returns a centre
/// and a size, in the order an [`Item`] holds them.
///
/// Sizes first, because [`place`] needs the size the card is *going* to be:
/// rounding the edge for the old size and then changing the size underneath it
/// moves the edge back off the dot it was just put on.
pub fn conform(x: f32, y: f32, w: f32, h: f32, step: f32) -> (f32, f32, f32, f32) {
    let (w, h) = (cells(w, step), cells(h, step));
    (place(x, w, step), place(y, h, step), w, h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ItemType;

    fn card(x: f32, y: f32, w: f32, h: f32, rot: f32) -> Item {
        let mut item = Item::new("a", ItemType::Generic);
        item.x = x;
        item.y = y;
        item.w = w;
        item.h = h;
        item.rot = rot;
        item
    }

    #[test]
    fn a_turned_card_reaches_further_than_its_own_width() {
        let flat = Rect::of_item(&card(0.0, 0.0, 200.0, 100.0, 0.0));
        assert_eq!((flat.width(), flat.height()), (200.0, 100.0));

        // On its side, the two dimensions have simply swapped.
        let side = Rect::of_item(&card(0.0, 0.0, 200.0, 100.0, 90.0));
        assert!((side.width() - 100.0).abs() < 0.01);
        assert!((side.height() - 200.0).abs() < 0.01);

        // At forty-five degrees it is wider than either, which is the case the
        // untilted box gets wrong.
        let tilted = Rect::of_item(&card(0.0, 0.0, 200.0, 100.0, 45.0));
        assert!(tilted.width() > 200.0);
        assert!(tilted.height() > 200.0);
    }

    #[test]
    fn the_box_around_a_turned_card_still_holds_every_corner_of_it() {
        let item = card(30.0, -12.0, 260.0, 90.0, 33.0);
        let box_ = Rect::of_item(&item);
        let t: f32 = item.rot.to_radians();
        let (hw, hh) = (item.w / 2.0, item.h / 2.0);
        for (sx, sy) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0f32)] {
            let (lx, ly) = (sx * hw, sy * hh);
            let corner =
                point(item.x + lx * t.cos() - ly * t.sin(), item.y + lx * t.sin() + ly * t.cos());
            // A hair of slack: the box is built from sines and the corner
            // from cosines, and the two agree only to the last bit.
            assert!(box_.inflate(0.001).contains(corner), "{corner:?} escaped {box_:?}");
        }
    }

    #[test]
    fn a_size_on_a_lattice_is_a_whole_number_of_cells() {
        assert_eq!(cells(137.0, 64.0), 128.0);
        assert_eq!(cells(160.0, 64.0), 192.0, "half a cell rounds up");
        assert_eq!(cells(192.0, 64.0), 192.0, "an odd count is as legal as an even one");
    }

    #[test]
    fn a_card_cannot_be_smaller_than_one_square_of_the_pattern() {
        // The point of the whole exercise: below a cell there is a size the
        // grid cannot express and no dot to line it up against.
        assert_eq!(cells(1.0, 64.0), 64.0);
        assert_eq!(cells(0.0, 64.0), 64.0);
        assert_eq!(cells(-500.0, 64.0), 64.0);
    }

    #[test]
    fn the_floor_is_whichever_is_larger_of_a_cell_and_the_smallest_card() {
        // A fine lattice: one cell is under `MIN_SIZE`, so the floor is the
        // fewest whole cells that clear it — and it has to be whole cells, or
        // the floor is itself a size that sits off the grid.
        assert_eq!(cells(1.0, 16.0), 48.0);
        assert_eq!(cells(1.0, 32.0), 64.0, "48 is not a multiple of 32");
        for step in [1.0, 7.0, 16.0, 32.0, 48.0, 64.0, 96.0, 128.0, 4096.0] {
            let smallest = cells(0.0, step);
            assert!(smallest >= MIN_SIZE, "{smallest} at step {step} is under MIN_SIZE");
            assert!(smallest >= step, "{smallest} at step {step} is under one cell");
            assert!((smallest / step).fract().abs() < 0.001, "{smallest} is not whole cells");
        }
    }

    #[test]
    fn a_size_is_still_held_to_what_the_board_can_draw() {
        assert_eq!(cells(1e9, 64.0), (MAX_SIZE / 64.0).floor() * 64.0);
        assert!(cells(1e9, 64.0) <= MAX_SIZE);
        // A step with no lattice in it falls back to the free clamp rather
        // than dividing by nothing.
        assert_eq!(cells(137.0, 0.0), 137.0);
        assert_eq!(cells(f32::NAN, 64.0), MIN_SIZE);
        assert_eq!(cells(137.0, f32::NAN), 137.0);
    }

    #[test]
    fn a_fitted_size_only_ever_grows() {
        // A note's height is decided by its words. Taking a cell off it to
        // reach the nearest multiple would cut a line off the bottom.
        assert_eq!(cells_up(129.0, 64.0), 192.0);
        assert_eq!(cells_up(192.0, 64.0), 192.0, "already whole, so it stays");
    }

    #[test]
    fn placing_a_card_rounds_its_edge_and_lets_the_centre_fall_where_it_will() {
        // Even cells: the centre lands on a dot, as it always did.
        assert_eq!(place(37.0, 128.0, 64.0), 64.0);
        // Odd cells: the centre lands *between* two dots, which is the fix.
        // Three cells at 64 is 192, so a centre of 96 puts its edges on 0
        // and 192 — both dots — and rounding the centre to 64 instead would
        // have put them on -32 and 160, both mid-square.
        assert_eq!(place(90.0, 192.0, 64.0), 96.0);
    }

    #[test]
    fn a_conformed_card_has_every_edge_on_a_dot() {
        let step = 64.0;
        // Swept, because the failure was odd/even: half the sizes passed the
        // old rule and half did not, so a single case proves nothing.
        for n in 1..40 {
            let (w, h) = (n as f32 * 17.0, 400.0 - n as f32 * 9.0);
            let (x, y, w, h) = conform(n as f32 * 13.0 - 100.0, 71.0 - n as f32 * 7.0, w, h, step);
            for edge in [x - w / 2.0, x + w / 2.0, y - h / 2.0, y + h / 2.0] {
                let cells_out = edge / step;
                assert!(
                    (cells_out - cells_out.round()).abs() < 0.001,
                    "an edge {cells_out} cells from the origin is halfway onto the grid"
                );
            }
            assert!((w / step).fract().abs() < 0.001);
            assert!((h / step).fract().abs() < 0.001);
        }
    }

    #[test]
    fn conforming_a_card_that_is_already_square_moves_it_nowhere() {
        // What lets `snap::engage` tell "already on the lattice" from "taken
        // onto it", which is what decides whether a memo is written.
        // Three cells wide with its low edge on the origin, so its centre is
        // at 96 — between two dots, and correct.
        let before = (96.0, 0.0, 192.0, 256.0);
        let after = conform(before.0, before.1, before.2, before.3, 64.0);
        assert_eq!(after, before);
    }

    #[test]
    fn turning_a_card_does_not_move_where_it_can_be_pressed() {
        // The tilted box is generous; `hit` is not. A point just outside the
        // turned rectangle but inside its box must not count as a press.
        let item = card(0.0, 0.0, 200.0, 40.0, 45.0);
        let corner = point(84.0, 0.0);
        assert!(Rect::of_item(&item).contains(corner));
        assert!(!hit(&item, corner));
        assert!(hit(&item, point(60.0, 60.0)));
    }
}
