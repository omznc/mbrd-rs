//! Snapping a dragged card to the cards already on the board, and saying why.
//!
//! The grid in [`snap`](crate::snap) is a lattice the whole board sits on: a
//! setting, applied to everything, whether or not anything was near it. This is
//! the other kind of snapping, and it is the one that makes a canvas feel like
//! it is helping — a card dragged near another card's edge takes that edge, and
//! a line is drawn through both so it is obvious *what* it took.
//!
//! The two are deliberately exclusive. A card cannot be on the lattice and
//! flush with its neighbour at the same time, and an app that tried to give it
//! both would give it whichever ran last. So the caller picks: grid snapping on
//! means the grid decides, and this is not consulted. See `board_view`'s move
//! handler, which is the only caller.
//!
//! ## What it looks for, in the order it prefers
//!
//! 1. **An edge or a centre lining up.** Three coordinates on the moving box —
//!    near edge, centre, far edge — against the same three on every candidate,
//!    which is nine offers per candidate per axis. The nearest wins, and every
//!    other pair that lands on the same coordinate joins the same line, which
//!    is what makes one drag draw one rule through four cards rather than four
//!    rules through two.
//! 2. **A gap repeating.** Only where nothing aligned on that axis, because a
//!    card that is both flush with something and evenly spaced from something
//!    else has to be one or the other and flush is the stronger claim. See
//!    [`spacing`].
//!
//! The two axes are worked out independently, which is what lets a card take a
//! neighbour's left edge while spacing itself evenly downward.
//!
//! ## Why this is here and not in the UI crate
//!
//! Every rule above is a statement about rectangles, and rectangles can be
//! tested in a millisecond without a window. The UI crate's half of this is the
//! part that genuinely needs one: turning [`Line`] into pixels.

use crate::geometry::Rect;

/// How near counts as near, when nothing says otherwise.
///
/// In **world** units, so a caller with a camera should divide its own screen
/// tolerance by the zoom rather than passing this — a snap that got harder to
/// reach the further you zoomed out would be a snap that stopped working on
/// exactly the boards big enough to need it.
pub const REACH: f32 = 6.0;

/// How near two floats have to be to be the same coordinate.
///
/// Not a tolerance for the snap — that is `reach`, and it is the caller's. This
/// is the one for deciding whether two *offers* landed in the same place, and
/// so whether they are one guide line or two.
const SAME: f32 = 0.01;

/// The most candidates any one axis will weigh.
///
/// A bound rather than a budget. The work is quadratic in the candidates for
/// [`spacing`] — every existing gap against every neighbour — and a drag on a
/// board where two hundred cards all overlap the pointer's row would spend the
/// frame on offers nobody could tell apart anyway. The nearest few are the ones
/// a person is aiming at.
const MOST: usize = 24;

/// How much further a coordinate already engaged reaches than a fresh one —
/// see [`find_held`].
///
/// Half again, not double. Enough that ordinary hand tremor at the boundary
/// cannot toggle a guide on and off several times a second, and not so much
/// that leaving a snap on purpose feels like it is stuck to it — the reach a
/// person judges by eye is still roughly `reach`, this just keeps a decision
/// already made from flickering on the noise around its own edge.
const HYSTERESIS: f32 = 1.5;

/// A rule drawn through everything that lined up.
///
/// World units, y pointing up, like everything else at this level. `Vertical`
/// is a line *of* constant x — the one you see when left edges line up — and
/// not a line that runs along x, which is the reading that gets this backwards
/// exactly once.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Line {
    /// At `x`, drawn from `y0` up to `y1`.
    Vertical { x: f32, y0: f32, y1: f32 },
    /// At `y`, drawn from `x0` across to `x1`.
    Horizontal { y: f32, x0: f32, x1: f32 },
}

impl Line {
    /// The coordinate the line is *at* — the x of a vertical, the y of a
    /// horizontal. What two lines have to share to be the same line.
    fn at(&self) -> f32 {
        match self {
            Self::Vertical { x, .. } => *x,
            Self::Horizontal { y, .. } => *y,
        }
    }
}

/// One gap that was matched, to be drawn as a measured bar between two cards.
///
/// `from` and `to` are along the axis and `across` is the line the bar is drawn
/// on, which is the middle of the overlap between the two cards it measures —
/// so a bar between two cards of different heights sits where both of them are.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Span {
    /// Whether the gap is measured left-to-right. `false` is a vertical gap.
    pub horizontal: bool,
    pub from: f32,
    pub to: f32,
    pub across: f32,
}

impl Span {
    pub fn size(&self) -> f32 {
        self.to - self.from
    }
}

/// Where the drag should actually put the card, and what to draw about it.
///
/// `dx` and `dy` are a **correction** to apply on top of the free position the
/// caller asked about, not an absolute place. Zero on an axis means nothing was
/// near enough on that axis, which is the common answer and costs the caller
/// nothing to apply.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snap {
    pub dx: f32,
    pub dy: f32,
    pub lines: Vec<Line>,
    pub spans: Vec<Span>,
}

impl Snap {
    /// Whether anything at all was found. A caller with nothing to draw and no
    /// correction to apply can stop here.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.spans.is_empty()
    }
}

/// Find what `moving` should line up with, among `others`.
///
/// `moving` is the box **at the free position** — where the pointer has put it
/// with nothing helping — and `others` is everything it could line up against,
/// which is the caller's job to filter: the moving set itself must not be in
/// there, or a card would line up with where it already is and never move.
///
/// A `reach` of zero or less turns the whole thing off and answers an empty
/// [`Snap`], which is the cheap way for a caller to keep one code path.
///
/// Plain wrapper over [`find_held`] with nothing held from a previous frame —
/// see that function for a caller mid-drag, which is the one that has one.
pub fn find(moving: Rect, others: &[Rect], reach: f32) -> Snap {
    find_held(moving, others, reach, (None, None))
}

/// The same search as [`find`], but a coordinate an axis was already engaged
/// on last frame reaches a little further than one discovered fresh.
///
/// Without this, a hand holding a card flush against a neighbour's edge lets
/// go the instant a pixel of ordinary tremor carries it past `reach`, and
/// reaches back the next frame the tremor reverses — a guide blinking on and
/// off around its own boundary. Hysteresis is the usual fix where a boundary
/// is judged from a noisy signal: leaving costs more than arriving did, so a
/// hand has to actually mean to let go rather than merely graze the edge.
///
/// `held` is `(x, y)`: the coordinate each axis last landed on, straight out
/// of that frame's own [`Snap::lines`] — the caller's to keep, since this
/// module holds nothing between calls. `None` on an axis that found nothing
/// last frame gets the ordinary `reach`, so this can only widen a match that
/// was already there; it is never a way to reach further for something new.
pub fn find_held(
    moving: Rect,
    others: &[Rect],
    reach: f32,
    held: (Option<f32>, Option<f32>),
) -> Snap {
    let mut out = Snap::default();
    if reach <= 0.0 || !reach.is_finite() || others.is_empty() {
        return out;
    }

    // The nearest candidates rather than all of them, and nearest by the gap
    // to the moving box rather than by centre distance: a long fence beside
    // the card is a thing to line up with, and its centre may be a screen away.
    // Below the cap there is nothing to cull, so the common case skips the
    // allocation entirely. Above it, the key is computed once per rect and a
    // partition finds the nearest `MOST` — their order past the cut is never
    // looked at, so there is nothing a full sort would buy.
    let near: Vec<&Rect> = if others.len() <= MOST {
        others.iter().collect()
    } else {
        let mut keyed: Vec<(f32, &Rect)> = others.iter().map(|r| (apart(moving, r), r)).collect();
        keyed.select_nth_unstable_by(MOST - 1, |a, b| {
            a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal)
        });
        keyed.truncate(MOST);
        keyed.into_iter().map(|(_, r)| r).collect()
    };

    let (dx, lines_x) = edges(moving, &near, reach, true, held.0);
    let (dy, lines_y) = edges(moving, &near, reach, false, held.1);
    out.dx = dx;
    out.dy = dy;
    out.lines.extend(lines_x);
    out.lines.extend(lines_y);

    // Spacing only where nothing aligned on that axis. A card flush with one
    // neighbour and evenly spaced from another has to be one or the other, and
    // flush is the stronger claim — it is the thing the eye actually checks.
    if out.dx == 0.0 {
        if let Some((delta, spans)) = spacing(moving, &near, reach, true) {
            out.dx = delta;
            out.spans.extend(spans);
        }
    }
    if out.dy == 0.0 {
        if let Some((delta, spans)) = spacing(moving, &near, reach, false) {
            out.dy = delta;
            out.spans.extend(spans);
        }
    }
    out
}

/// How far apart two boxes are, as the larger of the two axis gaps.
///
/// Zero where they overlap, which is deliberate: an overlapping card is as near
/// as it is possible to be, and every one of them should survive the cull.
fn apart(a: Rect, b: &Rect) -> f32 {
    let x = (b.x0 - a.x1).max(a.x0 - b.x1).max(0.0);
    let y = (b.y0 - a.y1).max(a.y0 - b.y1).max(0.0);
    x.max(y)
}

/// The three coordinates a box offers on one axis: near edge, centre, far edge.
fn offers(r: Rect, horizontal: bool) -> [f32; 3] {
    if horizontal {
        [r.x0, (r.x0 + r.x1) / 2.0, r.x1]
    } else {
        [r.y0, (r.y0 + r.y1) / 2.0, r.y1]
    }
}

/// The nearest edge or centre alignment on one axis, and every line it draws.
///
/// Two passes on purpose. The first finds *how far* the best offer is, and the
/// second collects every pair that lands on that same coordinate — because a
/// card dropped into a column of four should draw one rule through all four,
/// and a single-pass best-of would draw it through whichever it happened to
/// test first.
///
/// `held` is the coordinate this axis was engaged on last frame, if any — see
/// [`find_held`]. A candidate that lands on it is judged against a wider
/// reach than one that does not, which is the whole of the hysteresis: it
/// changes which offers are *eligible*, not which eligible one wins, so the
/// nearest-offer rule right below is untouched.
fn edges(
    moving: Rect,
    others: &[&Rect],
    reach: f32,
    horizontal: bool,
    held: Option<f32>,
) -> (f32, Vec<Line>) {
    let mine = offers(moving, horizontal);

    let mut best = f32::INFINITY;
    for other in others {
        for theirs in offers(**other, horizontal) {
            // Wider only for the coordinate already held, and only by
            // `HYSTERESIS` — see its own doc for why that number and not a
            // larger one.
            let allowed = if held.is_some_and(|h| (theirs - h).abs() < SAME) {
                reach * HYSTERESIS
            } else {
                reach
            };
            for m in mine {
                let delta = theirs - m;
                if delta.abs() <= allowed && delta.abs() < best.abs() {
                    best = delta;
                }
            }
        }
    }
    if !best.is_finite() {
        return (0.0, Vec::new());
    }

    // Where the moving box will actually be once the correction is applied.
    // The lines are drawn against *that*, not against where the pointer left
    // it, or a rule would appear a hair off the edge it is claiming.
    let landed = shifted(moving, best, horizontal);
    // Loop-invariant: `landed` does not change per candidate, so its offers
    // are worked out once rather than once per pair tested against them.
    let landed_offers = offers(landed, horizontal);
    let mut lines: Vec<Line> = Vec::new();
    for other in others {
        for theirs in offers(**other, horizontal) {
            let hit = landed_offers.iter().any(|m| (theirs - m).abs() < SAME);
            if !hit {
                continue;
            }
            let line = if horizontal {
                Line::Vertical {
                    x: theirs,
                    y0: landed.y0.min(other.y0),
                    y1: landed.y1.max(other.y1),
                }
            } else {
                Line::Horizontal {
                    y: theirs,
                    x0: landed.x0.min(other.x0),
                    x1: landed.x1.max(other.x1),
                }
            };
            join(&mut lines, line);
        }
    }
    (best, lines)
}

/// Add a line, or stretch the one already there to cover it.
///
/// Two cards lining up on the same coordinate is one rule through both, not two
/// rules on top of each other — which would be invisible on screen and twice
/// the draw calls to be invisible with.
fn join(lines: &mut Vec<Line>, line: Line) {
    for held in lines.iter_mut() {
        if std::mem::discriminant(held) != std::mem::discriminant(&line)
            || (held.at() - line.at()).abs() >= SAME
        {
            continue;
        }
        match (held, line) {
            (Line::Vertical { y0, y1, .. }, Line::Vertical { y0: a, y1: b, .. }) => {
                *y0 = y0.min(a);
                *y1 = y1.max(b);
            }
            (Line::Horizontal { x0, x1, .. }, Line::Horizontal { x0: a, x1: b, .. }) => {
                *x0 = x0.min(a);
                *x1 = x1.max(b);
            }
            _ => continue,
        }
        return;
    }
    lines.push(line);
}

/// The box, moved by `delta` along one axis.
fn shifted(r: Rect, delta: f32, horizontal: bool) -> Rect {
    if horizontal {
        Rect { x0: r.x0 + delta, x1: r.x1 + delta, ..r }
    } else {
        Rect { y0: r.y0 + delta, y1: r.y1 + delta, ..r }
    }
}

/// One box's extent along an axis, as `(near, far)`.
fn extent(r: Rect, horizontal: bool) -> (f32, f32) {
    if horizontal {
        (r.x0, r.x1)
    } else {
        (r.y0, r.y1)
    }
}

/// Whether two boxes overlap on the axis they are *not* being spaced along.
///
/// The filter that makes spacing mean anything: cards in the same row are
/// spaced along x, and a card three screens below is not part of that row
/// however evenly it happens to sit.
fn abreast(a: Rect, b: Rect, horizontal: bool) -> bool {
    let (a0, a1) = extent(a, !horizontal);
    let (b0, b1) = extent(b, !horizontal);
    a0 <= b1 && b0 <= a1
}

/// Where two boxes overlap across the axis, at its middle. Where a bar goes.
fn between(a: Rect, b: Rect, horizontal: bool) -> f32 {
    let (a0, a1) = extent(a, !horizontal);
    let (b0, b1) = extent(b, !horizontal);
    (a0.max(b0) + a1.min(b1)) / 2.0
}

/// Repeat a gap that already exists, or sit evenly between two neighbours.
///
/// Answers `None` where nothing was near enough, which is what lets the caller
/// leave the axis alone rather than correcting it by zero and drawing an empty
/// bar.
///
/// Two shapes of offer, and they are genuinely different things to want:
///
/// - **Even between two.** The moving box has a neighbour on each side and
///   wants the same gap to both. This is the one people do on purpose.
/// - **Repeat a gap.** Some pair in the row is already `g` apart, so `g` from
///   either end of any of them is a place worth landing on. This is the one
///   that builds a row without anybody having measured anything.
fn spacing(
    moving: Rect,
    others: &[&Rect],
    reach: f32,
    horizontal: bool,
) -> Option<(f32, Vec<Span>)> {
    // The row, in order. Anything not abreast of the moving box is not in it.
    let mut row: Vec<Rect> =
        others.iter().map(|r| **r).filter(|r| abreast(moving, *r, horizontal)).collect();
    if row.is_empty() {
        return None;
    }
    row.sort_by(|a, b| {
        extent(*a, horizontal)
            .0
            .partial_cmp(&extent(*b, horizontal).0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let (m0, m1) = extent(moving, horizontal);
    let mut best: Option<(f32, f32)> = None;
    let mut offer = |delta: f32, gap: f32| {
        if !delta.is_finite() || delta.abs() > reach || gap <= 0.0 {
            return;
        }
        if best.is_none_or(|(had, _)| delta.abs() < had.abs()) {
            best = Some((delta, gap));
        }
    };

    // Even between the two that bracket it.
    let left = row.iter().rfind(|r| extent(**r, horizontal).1 <= m0).copied();
    let right = row.iter().find(|r| extent(**r, horizontal).0 >= m1).copied();
    if let (Some(l), Some(r)) = (left, right) {
        let (_, l1) = extent(l, horizontal);
        let (r0, _) = extent(r, horizontal);
        let room = r0 - l1 - (m1 - m0);
        if room > 0.0 {
            // Half the leftover on each side. The correction is what moves the
            // near edge onto that.
            offer(l1 + room / 2.0 - m0, room / 2.0);
        }
    }

    // Every gap the row already has, offered off either end of every card in
    // it. Bounded by `MOST` twice over, which is what the cull upstream is for.
    let gaps: Vec<f32> = row
        .windows(2)
        .map(|w| extent(w[1], horizontal).0 - extent(w[0], horizontal).1)
        .filter(|g| *g > 0.0)
        .collect();
    for gap in &gaps {
        for other in &row {
            let (o0, o1) = extent(*other, horizontal);
            // To its far side, and to its near side.
            offer(o1 + gap - m0, *gap);
            offer(o0 - gap - m1, *gap);
        }
    }

    let (delta, gap) = best?;
    let landed = shifted(moving, delta, horizontal);

    // Every gap in the row that came out this size, the moving box's own
    // included. Drawing only the one it matched would say "this is `g`" where
    // what it means is "these are all `g`".
    //
    // `row` is already sorted by this same key — see above — so `landed` is
    // inserted at its place rather than appended with the whole thing sorted
    // again. `row` is not read after this, so the insert reuses it directly.
    let mut spans: Vec<Span> = Vec::new();
    let mut all = row;
    let at = all.partition_point(|r| extent(*r, horizontal).0 <= extent(landed, horizontal).0);
    all.insert(at, landed);
    for pair in all.windows(2) {
        let (_, a1) = extent(pair[0], horizontal);
        let (b0, _) = extent(pair[1], horizontal);
        if (b0 - a1 - gap).abs() >= SAME {
            continue;
        }
        spans.push(Span {
            horizontal,
            from: a1,
            to: b0,
            across: between(pair[0], pair[1], horizontal),
        });
    }
    if spans.is_empty() {
        return None;
    }
    Some((delta, spans))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(cx: f32, cy: f32, w: f32, h: f32) -> Rect {
        Rect::centred(cx, cy, w, h)
    }

    #[test]
    fn a_card_near_a_left_edge_takes_it() {
        // Moving box's left edge is at 3, the other's at 0. Three units off,
        // inside a reach of six.
        let moving = at(103.0, 500.0, 200.0, 100.0);
        let other = at(100.0, 0.0, 200.0, 100.0);
        let snap = find(moving, &[other], 6.0);
        assert_eq!(snap.dx, -3.0);
        assert_eq!(snap.dy, 0.0);
        assert!(matches!(snap.lines[0], Line::Vertical { x, .. } if x == 0.0));
    }

    #[test]
    fn nothing_near_enough_is_no_correction_and_nothing_drawn() {
        let moving = at(400.0, 400.0, 100.0, 100.0);
        let other = at(0.0, 0.0, 100.0, 100.0);
        let snap = find(moving, &[other], 6.0);
        assert_eq!((snap.dx, snap.dy), (0.0, 0.0));
        assert!(snap.is_empty());
    }

    #[test]
    fn the_nearest_of_several_offers_is_the_one_taken() {
        // Centres are two apart and left edges are five apart. The centre wins.
        let moving = at(2.0, 300.0, 100.0, 100.0);
        let other = at(0.0, 0.0, 100.0, 100.0);
        let snap = find(moving, &[other], 6.0);
        assert_eq!(snap.dx, -2.0);
    }

    #[test]
    fn one_rule_is_drawn_through_everything_that_lined_up() {
        // Three cards already in a column, and a fourth dropped near it. The
        // rule has to reach all four, or it says the drag lined up with one of
        // them and picked the others at random.
        let column: Vec<Rect> =
            [0.0, -200.0, -400.0].iter().map(|y| at(0.0, *y, 100.0, 100.0)).collect();
        let moving = at(3.0, 200.0, 100.0, 100.0);
        let snap = find(moving, &column, 6.0);
        assert_eq!(snap.dx, -3.0);
        let centre =
            snap.lines.iter().find(|l| matches!(l, Line::Vertical { x, .. } if *x == 0.0)).unwrap();
        let Line::Vertical { y0, y1, .. } = centre else { panic!("wrong way round") };
        assert_eq!((*y0, *y1), (-450.0, 250.0));
    }

    #[test]
    fn the_two_axes_are_answered_separately() {
        // Lines up on x with one card and on y with a completely different one.
        let up = at(0.0, 900.0, 100.0, 100.0);
        let across = at(900.0, 0.0, 100.0, 100.0);
        let moving = at(2.0, -3.0, 100.0, 100.0);
        let snap = find(moving, &[up, across], 6.0);
        assert_eq!((snap.dx, snap.dy), (-2.0, 3.0));
    }

    #[test]
    fn a_card_dropped_between_two_sits_evenly_between_them() {
        // Left card ends at 100, right card starts at 500. A 100-wide card
        // between them is even at x0 = 250, so a card at 253 is three off.
        let left = at(50.0, 0.0, 100.0, 100.0);
        let right = at(550.0, 0.0, 100.0, 100.0);
        let moving = at(303.0, 0.0, 100.0, 100.0);
        let snap = find(moving, &[left, right], 6.0);
        assert_eq!(snap.dx, -3.0);
        assert_eq!(snap.spans.len(), 2);
        assert!(snap.spans.iter().all(|s| (s.size() - 150.0).abs() < 0.01));
    }

    #[test]
    fn a_gap_that_already_exists_is_a_gap_worth_repeating() {
        // Two cards 40 apart. A third dropped 43 past the second takes 40.
        let a = at(0.0, 0.0, 100.0, 100.0);
        let b = at(140.0, 0.0, 100.0, 100.0);
        let moving = at(283.0, 0.0, 100.0, 100.0);
        let snap = find(moving, &[a, b], 6.0);
        assert_eq!(snap.dx, -3.0);
        // Both gaps drawn, not just the new one: what it means is "these are
        // all forty", not "this one is forty".
        assert_eq!(snap.spans.len(), 2);
    }

    #[test]
    fn a_card_in_a_different_row_is_not_part_of_this_rows_spacing() {
        // Same arithmetic as above, but the neighbours are nowhere near the
        // moving card on the other axis, so there is no row to be spaced in.
        let a = at(0.0, 5_000.0, 100.0, 100.0);
        let b = at(140.0, 5_000.0, 100.0, 100.0);
        let moving = at(283.0, 0.0, 100.0, 100.0);
        let snap = find(moving, &[a, b], 6.0);
        assert!(snap.spans.is_empty());
    }

    #[test]
    fn lining_up_beats_spacing_out_on_the_same_axis() {
        // The card can either sit flush with a left edge or evenly between two
        // others. Flush is the stronger claim, so no bar is drawn on that axis.
        let left = at(50.0, 0.0, 100.0, 100.0);
        let right = at(550.0, 0.0, 100.0, 100.0);
        // A fourth card whose left edge is a hair from the moving one's.
        let flush = at(302.0, -400.0, 100.0, 100.0);
        let moving = at(303.0, 0.0, 100.0, 100.0);
        let snap = find(moving, &[left, right, flush], 6.0);
        assert_eq!(snap.dx, -1.0);
        assert!(snap.spans.iter().all(|s| !s.horizontal));
    }

    #[test]
    fn a_reach_of_nothing_turns_the_whole_thing_off() {
        let moving = at(103.0, 0.0, 200.0, 100.0);
        let other = at(100.0, 0.0, 200.0, 100.0);
        assert!(find(moving, &[other], 0.0).is_empty());
        assert!(find(moving, &[other], f32::NAN).is_empty());
    }

    #[test]
    fn an_empty_board_has_nothing_to_line_up_with() {
        assert!(find(at(0.0, 0.0, 10.0, 10.0), &[], 6.0).is_empty());
    }

    #[test]
    fn a_crowd_of_candidates_is_culled_to_the_ones_nearby() {
        // Far more than `MOST`, all of them far away, plus one near neighbour.
        // The near one has to survive the cull or the snap it offers is lost.
        let mut crowd: Vec<Rect> =
            (0..200).map(|n| at(10_000.0 + n as f32 * 300.0, 0.0, 100.0, 100.0)).collect();
        crowd.push(at(0.0, 0.0, 100.0, 100.0));
        let snap = find(at(3.0, 300.0, 100.0, 100.0), &crowd, 6.0);
        assert_eq!(snap.dx, -3.0);
    }

    #[test]
    fn holding_a_coordinate_reaches_further_than_finding_it_fresh() {
        // Left edges are eight apart — past the ordinary reach of six, but
        // within `HYSTERESIS`'s wider one for a coordinate already held.
        // Offset well clear on y so only the x axis is ever in play.
        let moving = at(58.0, 0.0, 100.0, 100.0);
        let other = at(50.0, 500.0, 100.0, 100.0);
        assert!(find(moving, &[other], 6.0).is_empty(), "fresh, eight is too far to take");
        let held = find_held(moving, &[other], 6.0, (Some(0.0), None));
        assert_eq!(held.dx, -8.0, "held, the same eight still lands it");
    }

    #[test]
    fn hysteresis_never_reaches_past_its_own_multiple() {
        // Twelve is past even the widened reach of nine, so holding the
        // coordinate buys nothing here — hysteresis keeps a decision from
        // flickering, it does not make the reach unbounded.
        let moving = at(62.0, 0.0, 100.0, 100.0);
        let other = at(50.0, 500.0, 100.0, 100.0);
        assert!(find_held(moving, &[other], 6.0, (Some(0.0), None)).is_empty());
    }

    #[test]
    fn holding_the_wrong_coordinate_gets_no_extra_reach() {
        // The same eight-unit gap as the first test above, but held on a
        // coordinate nothing here actually offers. The wider reach must not
        // leak onto a candidate that was never the one held.
        let moving = at(58.0, 0.0, 100.0, 100.0);
        let other = at(50.0, 500.0, 100.0, 100.0);
        assert!(find_held(moving, &[other], 6.0, (Some(999.0), None)).is_empty());
    }
}
