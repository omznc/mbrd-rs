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
