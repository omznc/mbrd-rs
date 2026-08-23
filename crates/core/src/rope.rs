//! Where a connection runs between two cards.
//!
//! A [`Connection`](crate::model::Connection) stores two ids and nothing about
//! the path, which is the right decision — where a line runs is a function of
//! where the two cards are *now*, so a stored path is a path that goes stale
//! every time somebody moves something. This module is the function.
//!
//! Kept in `core` and kept pure for the reason `geometry` is: a curve between
//! two rectangles is arithmetic, and written here it can be tested by asserting
//! points. Written inside the painter it could only be tested by looking at it.
//!
//! ## World units, and y points up
//!
//! Everything here is in world space, where a card at `y: 100` is *above* the
//! origin — so [`Side::Top`] is the high-`y` face and its outward normal is
//! `+y`. The flip to screen coordinates happens in `viewport.rs` and nowhere
//! else, which is what lets a rope be worked out once and drawn at any zoom.

use crate::geometry::{point, Point, Rect};

/// How many points a curve is sampled into for measuring and for hit-testing.
///
/// Enough that the polyline is within a fraction of a world unit of the real
/// curve at the sizes a card is, and few enough that testing a pointer against
/// every rope on screen is not something a frame notices.
const SAMPLES: usize = 24;

/// The shortest and longest a control arm may be, in world units.
///
/// The arm is what makes a rope a rope rather than a straight line: it leaves
/// the card perpendicular to the face it left from and only then bends toward
/// the other end. Too short and two adjacent cards get a kink; too long and two
/// distant cards get a balloon that wanders off the route entirely.
const MIN_ARM: f32 = 32.0;
const MAX_ARM: f32 = 260.0;

/// How much of the gap between two cards the arm takes up, between those bounds.
const ARM_SHARE: f32 = 0.45;

/// Which face of a card a rope leaves from.
///
/// Named for how the card looks on screen — `Top` is the face you would point
/// at above the card — which is the high-`y` edge, because y points up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

impl Side {
    /// Every one, in the order they are drawn.
    pub const ALL: [Side; 4] = [Side::Top, Side::Right, Side::Bottom, Side::Left];

    /// The middle of this face of a card.
    pub fn spot(self, card: Rect) -> Point {
        let mid = card.centre();
        match self {
            Side::Left => point(card.x0, mid.y),
            Side::Right => point(card.x1, mid.y),
            Side::Top => point(mid.x, card.y1),
            Side::Bottom => point(mid.x, card.y0),
        }
    }

    /// The unit vector pointing away from the card, out of this face.
    pub fn normal(self) -> Point {
        match self {
            Side::Left => point(-1.0, 0.0),
            Side::Right => point(1.0, 0.0),
            Side::Top => point(0.0, 1.0),
            Side::Bottom => point(0.0, -1.0),
        }
    }

    pub fn opposite(self) -> Side {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
            Side::Top => Side::Bottom,
            Side::Bottom => Side::Top,
        }
    }

    /// A name, for a menu or a test to say.
    pub fn as_str(self) -> &'static str {
        match self {
            Side::Left => "left",
            Side::Right => "right",
            Side::Top => "top",
            Side::Bottom => "bottom",
        }
    }
}

/// Which faces two cards should be joined by, when nobody has said.
///
/// The obvious rule — whichever axis the two centres are further apart on —
/// gets it wrong for the shape a card usually is. Cards are wider than they are
/// tall, so two of them stacked with a small vertical gap are still further
/// apart horizontally than vertically by the raw numbers, and the rope leaves
/// out of the side and doubles back. So the separation on each axis is measured
/// **against the room the two cards take up on that axis**, which is the
/// question actually being asked: which way round do these two cards sit?
///
/// Ties go to the horizontal, because that is the reading direction and the way
/// two related cards are usually put down.
pub fn facing(from: Rect, to: Rect) -> (Side, Side) {
    let (a, b) = (from.centre(), to.centre());
    let dx = b.x - a.x;
    let dy = b.y - a.y;
    // Half of each card, plus half of the other: the distance at which they
    // would just touch on that axis. Never zero, so the division is safe on a
    // card that has been collapsed to nothing.
    let across = ((from.width() + to.width()) / 2.0).max(1.0);
    let up = ((from.height() + to.height()) / 2.0).max(1.0);

    if (dx / across).abs() >= (dy / up).abs() {
        if dx >= 0.0 {
            (Side::Right, Side::Left)
        } else {
            (Side::Left, Side::Right)
        }
    } else if dy >= 0.0 {
        (Side::Top, Side::Bottom)
    } else {
        (Side::Bottom, Side::Top)
    }
}

/// A rope: a cubic Bézier from one card to another, in world units.
///
/// Four points, because that is all a cubic is, and because holding it as data
/// rather than as a drawing call is what lets the same curve be painted, hit
/// tested, and asked where to put a label without any of the three
/// re-deriving it slightly differently.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rope {
    /// Where it leaves.
    pub a: Point,
    /// The arm out of `a`, perpendicular to the face it left.
    pub ca: Point,
    /// The arm into `b`.
    pub cb: Point,
    /// Where it arrives.
    pub b: Point,
}

impl Rope {
    /// The rope between two cards, given the faces it joins.
    pub fn between(from: Rect, from_side: Side, to: Rect, to_side: Side) -> Self {
        Self::of(from_side.spot(from), from_side, to_side.spot(to), Some(to_side))
    }

    /// The rope between two cards, choosing the faces itself.
    pub fn auto(from: Rect, to: Rect) -> Self {
        let (a, b) = facing(from, to);
        Self::between(from, a, to, b)
    }

    /// A rope being dragged, whose far end is the pointer and not a card yet.
    ///
    /// The loose end has no face to be perpendicular to, so it has no arm: the
    /// curve arrives straight at the pointer. Anything else would make the end
    /// of the rope lag behind the cursor, which reads as the drag being stuck.
    pub fn loose(from: Rect, from_side: Side, to: Point) -> Self {
        Self::of(from_side.spot(from), from_side, to, None)
    }

    fn of(a: Point, a_side: Side, b: Point, b_side: Option<Side>) -> Self {
        let span = ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
        let arm = (span * ARM_SHARE).clamp(MIN_ARM, MAX_ARM);
        // Never longer than the gap itself, or two cards almost touching get a
        // rope that loops out past both of them to travel four units.
        let arm = arm.min(span.max(1.0));
        let out = |p: Point, side: Side| {
            let n = side.normal();
            point(p.x + n.x * arm, p.y + n.y * arm)
        };
        Self {
            a,
            ca: out(a, a_side),
            cb: match b_side {
                Some(side) => out(b, side),
                None => b,
            },
            b,
        }
    }

    /// The point a fraction of the way along.
    pub fn at(self, t: f32) -> Point {
        let u = 1.0 - t;
        let (w0, w1, w2, w3) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
        point(
            w0 * self.a.x + w1 * self.ca.x + w2 * self.cb.x + w3 * self.b.x,
            w0 * self.a.y + w1 * self.ca.y + w2 * self.cb.y + w3 * self.b.y,
        )
    }

    /// Which way the curve is heading there, as a unit vector.
    ///
    /// What an arrowhead points along. Degenerate at a rope of no length —
    /// a card connected to itself, or two cards exactly on top of each other —
    /// and answers `+x` rather than a `NaN` that would take the arrowhead with
    /// it.
    pub fn heading(self, t: f32) -> Point {
        let u = 1.0 - t;
        let (w0, w1, w2) = (3.0 * u * u, 6.0 * u * t, 3.0 * t * t);
        let dx = w0 * (self.ca.x - self.a.x)
            + w1 * (self.cb.x - self.ca.x)
            + w2 * (self.b.x - self.cb.x);
        let dy = w0 * (self.ca.y - self.a.y)
            + w1 * (self.cb.y - self.ca.y)
            + w2 * (self.b.y - self.cb.y);
        let len = (dx * dx + dy * dy).sqrt();
        if len < 1e-6 {
            point(1.0, 0.0)
        } else {
            point(dx / len, dy / len)
        }
    }

    /// The middle, which is where a label goes.
    pub fn middle(self) -> Point {
        self.at(0.5)
    }

    /// The curve as a run of points, for anything that walks it.
    pub fn samples(self) -> [Point; SAMPLES] {
        let mut out = [self.a; SAMPLES];
        for (i, slot) in out.iter_mut().enumerate() {
            *slot = self.at(i as f32 / (SAMPLES - 1) as f32);
        }
        out
    }

    /// A box the whole curve is inside.
    ///
    /// The control hull, not a measurement of the curve: a Bézier never leaves
    /// the convex hull of its four points, so this is a superset and it costs
    /// four comparisons instead of twenty-four. Culling wants a superset —
    /// missing one is a rope that vanishes when its cards are off screen even
    /// though the middle of it is not.
    pub fn hull(self) -> Rect {
        let xs = [self.a.x, self.ca.x, self.cb.x, self.b.x];
        let ys = [self.a.y, self.ca.y, self.cb.y, self.b.y];
        Rect::new(
            xs.iter().copied().fold(f32::INFINITY, f32::min),
            ys.iter().copied().fold(f32::INFINITY, f32::min),
            xs.iter().copied().fold(f32::NEG_INFINITY, f32::max),
            ys.iter().copied().fold(f32::NEG_INFINITY, f32::max),
        )
    }

    /// How far a point is from the curve, in world units.
    ///
    /// Measured against the sampled polyline rather than solved for, because
    /// the exact answer is the root of a quintic and this one is wrong by less
    /// than the width of the line it is testing.
    pub fn distance(self, p: Point) -> f32 {
        let samples = self.samples();
        let mut best = f32::INFINITY;
        for pair in samples.windows(2) {
            best = best.min(to_segment(p, pair[0], pair[1]));
        }
        best
    }

    /// Whether a point is near enough to count as being on the rope.
    ///
    /// `reach` is in world units and the caller works it out from the zoom, for
    /// the same reason a grip does: a rope is a thing you aim a pointer at, so
    /// what should stay constant is how close you have to get in *pixels*.
    pub fn near(self, p: Point, reach: f32) -> bool {
        // The hull first. A board may carry two thousand connections and a
        // press should cost the ropes it could plausibly be on, not all of
        // them; this rejects almost all of them for four comparisons.
        if !self.hull().inflate(reach).contains(p) {
            return false;
        }
        self.distance(p) <= reach
    }
}

// ---------------------------------------------------------------------------
// Joining two cards, and letting them go
// ---------------------------------------------------------------------------

/// Join two cards, and answer whether that changed anything.
///
/// Every rule about what a connection may be is here rather than at the call
/// site, because the call site is a mouse-up and a mouse-up is the worst place
/// to keep a rule: it is the one spelling of the gesture, so a rule written
/// there is a rule the next spelling — a menu entry, a paste, a script — will
/// not have.
///
/// - **A card is not joined to itself.** The format does not forbid it and a
///   reader must not choke on one, but there is no gesture that should make it:
///   a drag that ends on the card it started from is a drag somebody thought
///   better of.
/// - **The pair is unordered**, so joining `a` to `b` when `b` is already
///   joined to `a` is not a second rope. The order given is still the order
///   stored, because `dir` is read against it.
/// - **Both ends have to exist.** A rope to nothing is a rope the next load
///   prunes, which would make it look like the save lost it.
/// - **[`MAX_CONNECTIONS`] is a ceiling**, and reaching it fails rather than
///   dropping the oldest. Silently throwing away something somebody drew is
///   worse than declining to draw another.
///
/// [`MAX_CONNECTIONS`]: crate::model::MAX_CONNECTIONS
pub fn join(board: &mut crate::model::Board, a: &str, b: &str) -> bool {
    use crate::model::{Connection, MAX_CONNECTIONS};
    if a == b || a.is_empty() || b.is_empty() {
        return false;
    }
    if board.item(a).is_none() || board.item(b).is_none() {
        return false;
    }
    if between(board, a, b).is_some() {
        return false;
    }
    if board.connections.len() >= MAX_CONNECTIONS {
        return false;
    }
    board.connections.push(Connection { a: a.to_string(), b: b.to_string(), meta: <_>::default() });
    true
}

/// Take a connection off the board, and answer whether one was there.
///
/// The cards stay. That is worth saying because the gesture that reaches this
/// is a `Delete` press, and `Delete` everywhere else in the app means the bin.
pub fn part(board: &mut crate::model::Board, a: &str, b: &str) -> bool {
    let before = board.connections.len();
    board.connections.retain(|c| !same(c, a, b));
    board.connections.len() != before
}

/// The connection between two cards, whichever way round it was drawn.
pub fn between<'b>(
    board: &'b crate::model::Board,
    a: &str,
    b: &str,
) -> Option<&'b crate::model::Connection> {
    board.connections.iter().find(|c| same(c, a, b))
}

/// The same, to change. Only ever reached from inside the door — see
/// `state::BoardState`, which is what lends out the `&mut Board` this needs.
pub fn between_mut<'b>(
    board: &'b mut crate::model::Board,
    a: &str,
    b: &str,
) -> Option<&'b mut crate::model::Connection> {
    board.connections.iter_mut().find(|c| same(c, a, b))
}

/// Whether this connection joins these two cards, in either order.
fn same(c: &crate::model::Connection, a: &str, b: &str) -> bool {
    (c.a == a && c.b == b) || (c.a == b && c.b == a)
}

/// The distance from a point to a line segment.
fn to_segment(p: Point, a: Point, b: Point) -> f32 {
    let (vx, vy) = (b.x - a.x, b.y - a.y);
    let len2 = vx * vx + vy * vy;
    // A segment of no length is a point, and the projection below would be a
    // division by zero rather than the obvious answer.
    if len2 < 1e-9 {
        return ((p.x - a.x).powi(2) + (p.y - a.y).powi(2)).sqrt();
    }
    let t = (((p.x - a.x) * vx + (p.y - a.y) * vy) / len2).clamp(0.0, 1.0);
    let (nx, ny) = (a.x + vx * t, a.y + vy * t);
    ((p.x - nx).powi(2) + (p.y - ny).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(cx: f32, cy: f32) -> Rect {
        Rect::centred(cx, cy, 200.0, 120.0)
    }

    #[test]
    fn a_rope_starts_and_ends_on_the_faces_it_was_given() {
        let (left, right) = (card(0.0, 0.0), card(500.0, 0.0));
        let rope = Rope::between(left, Side::Right, right, Side::Left);
        assert_eq!(rope.a, point(100.0, 0.0), "the right face of the left card");
        assert_eq!(rope.b, point(400.0, 0.0), "the left face of the right card");
        assert_eq!(rope.at(0.0), rope.a);
        assert_eq!(rope.at(1.0), rope.b);
    }

    #[test]
    fn the_arms_leave_perpendicular_to_the_faces() {
        // This is what makes it a rope rather than a line: it comes out of the
        // card square before it bends, so the join reads as attached.
        let rope = Rope::between(card(0.0, 0.0), Side::Right, card(500.0, 0.0), Side::Left);
        assert_eq!(rope.ca.y, rope.a.y, "the arm out should not have risen");
        assert!(rope.ca.x > rope.a.x, "and it should point away from the card");
        assert_eq!(rope.cb.y, rope.b.y);
        assert!(rope.cb.x < rope.b.x, "the far arm points back the way it came");
    }

    #[test]
    fn cards_side_by_side_are_joined_across_and_stacked_ones_up() {
        assert_eq!(facing(card(0.0, 0.0), card(600.0, 0.0)), (Side::Right, Side::Left));
        assert_eq!(facing(card(600.0, 0.0), card(0.0, 0.0)), (Side::Left, Side::Right));
        // Above, in world terms, is the greater y — and the rope leaves the top
        // of the lower card and arrives at the bottom of the upper one.
        assert_eq!(facing(card(0.0, 0.0), card(0.0, 400.0)), (Side::Top, Side::Bottom));
        assert_eq!(facing(card(0.0, 400.0), card(0.0, 0.0)), (Side::Bottom, Side::Top));
    }

    #[test]
    fn two_cards_stacked_close_are_joined_top_to_bottom_not_side_to_side() {
        // The bug this is here for: a card is 200 wide and 120 tall, so two of
        // them one above the other with a 40-unit gap are 0 apart horizontally
        // and 160 apart vertically — but a rule that compared the *centres*
        // against each other would still see a wider card and route sideways.
        let below = card(0.0, 0.0);
        let above = card(30.0, 160.0);
        assert_eq!(facing(below, above), (Side::Top, Side::Bottom));
    }

    #[test]
    fn a_loose_rope_ends_exactly_under_the_pointer() {
        // Anything else and the end of the rope trails the cursor during the
        // drag, which reads as the gesture having come off.
        let pointer = point(333.0, -77.0);
        let rope = Rope::loose(card(0.0, 0.0), Side::Right, pointer);
        assert_eq!(rope.b, pointer);
        assert_eq!(rope.at(1.0), pointer);
    }

    #[test]
    fn the_middle_of_a_symmetric_rope_is_halfway_between_the_cards() {
        let rope = Rope::between(card(0.0, 0.0), Side::Right, card(500.0, 0.0), Side::Left);
        let mid = rope.middle();
        assert!((mid.x - 250.0).abs() < 0.001, "{mid:?}");
        assert!((mid.y - 0.0).abs() < 0.001, "{mid:?}");
    }

    #[test]
    fn a_point_on_the_curve_is_on_the_rope_and_one_away_from_it_is_not() {
        let rope = Rope::between(card(0.0, 0.0), Side::Right, card(400.0, 300.0), Side::Left);
        for step in 0..=10 {
            let on = rope.at(step as f32 / 10.0);
            assert!(rope.near(on, 2.0), "the curve should be on itself at {step}: {on:?}");
        }
        assert!(!rope.near(point(250.0, 900.0), 8.0));
        assert!(!rope.near(point(-900.0, 0.0), 8.0));
    }

    #[test]
    fn the_hull_contains_every_point_of_the_curve() {
        // Culling reads the hull, so a curve that left it would be a rope that
        // disappeared while part of it was still on screen.
        let rope = Rope::between(card(0.0, 0.0), Side::Top, card(-600.0, 500.0), Side::Bottom);
        let hull = rope.hull();
        for step in 0..=40 {
            let p = rope.at(step as f32 / 40.0);
            assert!(hull.contains(p), "{p:?} escaped {hull:?}");
        }
    }

    #[test]
    fn an_arm_is_never_longer_than_the_gap_it_is_crossing() {
        // Two cards all but touching. A fixed minimum arm would send the rope
        // a hundred units out and back to travel four.
        let rope = Rope::between(card(0.0, 0.0), Side::Right, card(204.0, 0.0), Side::Left);
        let hull = rope.hull();
        assert!(hull.width() <= 8.0, "the rope bulged out of the gap: {hull:?}");
    }

    #[test]
    fn a_rope_that_goes_nowhere_still_has_a_direction() {
        // A card connected to itself, which the format does not forbid.
        let same = card(0.0, 0.0);
        let rope = Rope::between(same, Side::Right, same, Side::Right);
        let heading = rope.heading(0.5);
        assert!(heading.x.is_finite() && heading.y.is_finite(), "{heading:?}");
    }

    // -----------------------------------------------------------------------
    // Joining
    // -----------------------------------------------------------------------

    fn board_of(ids: &[&str]) -> crate::model::Board {
        let mut board = crate::model::Board::default();
        for id in ids {
            board.items.push(crate::model::Item::new(*id, crate::model::ItemType::Note));
        }
        board
    }

    #[test]
    fn joining_two_cards_puts_one_rope_between_them() {
        let mut board = board_of(&["a", "b"]);
        assert!(join(&mut board, "a", "b"));
        assert_eq!(board.connections.len(), 1);
        assert_eq!((board.connections[0].a.as_str(), board.connections[0].b.as_str()), ("a", "b"));
    }

    #[test]
    fn joining_the_same_pair_twice_is_still_one_rope() {
        let mut board = board_of(&["a", "b"]);
        assert!(join(&mut board, "a", "b"));
        assert!(!join(&mut board, "a", "b"), "the same way round");
        assert!(!join(&mut board, "b", "a"), "and the other way round");
        assert_eq!(board.connections.len(), 1);
    }

    #[test]
    fn the_order_a_rope_was_drawn_in_is_the_order_it_is_kept_in() {
        // The pair is unordered for identity and ordered for `dir`: "fwd" means
        // it points at the second id, so reversing the two would reverse every
        // arrowhead on the board.
        let mut board = board_of(&["a", "b"]);
        join(&mut board, "b", "a");
        assert_eq!(board.connections[0].a, "b");
    }

    #[test]
    fn a_card_cannot_be_joined_to_itself() {
        let mut board = board_of(&["a"]);
        assert!(!join(&mut board, "a", "a"));
        assert!(board.connections.is_empty());
    }

    #[test]
    fn a_rope_to_a_card_that_is_not_there_is_refused() {
        // Otherwise the save writes one and the next load prunes it, which
        // looks from the outside exactly like the file having lost it.
        let mut board = board_of(&["a"]);
        assert!(!join(&mut board, "a", "ghost"));
        assert!(!join(&mut board, "ghost", "a"));
        assert!(board.connections.is_empty());
    }

    #[test]
    fn a_board_at_the_ceiling_declines_rather_than_dropping_the_oldest() {
        let mut board = board_of(&["a", "b"]);
        board.connections = (0..crate::model::MAX_CONNECTIONS)
            .map(|i| crate::model::Connection {
                a: format!("x{i}"),
                b: format!("y{i}"),
                meta: <_>::default(),
            })
            .collect();
        assert!(!join(&mut board, "a", "b"));
        assert_eq!(board.connections.len(), crate::model::MAX_CONNECTIONS);
        assert_eq!(board.connections[0].a, "x0", "the first one should still be there");
    }

    #[test]
    fn parting_takes_the_rope_and_leaves_the_cards() {
        let mut board = board_of(&["a", "b"]);
        join(&mut board, "a", "b");
        assert!(part(&mut board, "b", "a"), "either order finds it");
        assert!(board.connections.is_empty());
        assert_eq!(board.items.len(), 2, "the cards are not what was deleted");
        assert!(!part(&mut board, "a", "b"), "and again is nothing to do");
    }

    #[test]
    fn a_rope_is_found_whichever_way_round_it_is_asked_for() {
        let mut board = board_of(&["a", "b"]);
        join(&mut board, "a", "b");
        assert!(between(&board, "a", "b").is_some());
        assert!(between(&board, "b", "a").is_some());
        assert!(between(&board, "a", "c").is_none());

        between_mut(&mut board, "b", "a").unwrap().meta.color = crate::model::ConnColor::Leaf;
        assert_eq!(board.connections[0].meta.color, crate::model::ConnColor::Leaf);
    }

    #[test]
    fn the_heading_at_the_start_is_the_way_the_arm_points() {
        let rope = Rope::between(card(0.0, 0.0), Side::Top, card(0.0, 600.0), Side::Bottom);
        let heading = rope.heading(0.0);
        assert!(heading.y > 0.9, "it should be leaving upward: {heading:?}");
        let arriving = rope.heading(1.0);
        assert!(arriving.y > 0.9, "and still going up when it lands: {arriving:?}");
    }
}
