//! Where a line between two cards runs.
//!
//! An orthogonal path that goes *around* the cards in between rather than
//! through them. Nothing about a path is ever stored — where a line runs is a
//! function of where the two cards are now — so there is nothing here to
//! invalidate and nothing to go stale. This module never reaches for the board
//! or the spatial index; obstacles are handed in.
//!
//! Right angles are how a route is *worked out* and not quite how it is drawn:
//! [`line`] hands what the search found to [`smooth`], which rounds the turns
//! without moving the runs between them. Everything below [`route`] is written
//! in terms of the right angles, so that is where the orthogonality holds.
//!
//! ## Why not a grid
//!
//! World space is infinite and float, so any fixed cell size either misses real
//! gaps or explodes. The lattice is built from the **obstacles' own edges**,
//! pushed out by a clearance: every corridor that exists between two cards has
//! a line on it, and there are no lines anywhere else. A\* runs over that, with
//! a cost of distance **plus a penalty per turn**. That penalty is the whole
//! difference between a diagram and a staircase.
//!
//! ## It concedes room before it concedes the route
//!
//! A straight diagonal is not a failed route — it is the thing this module
//! exists to stop drawing, since it scores through every card between the two
//! ends at an angle nothing else on the board is drawn at, and it reads as
//! damage. So a search that finds nothing is retried at a third of the
//! clearance, then at none of it (hugging the edges, where only real overlap
//! blocks). Then the cheap two-bend elbow is tried on all sixteen pairs of
//! faces, then the lattice search on as many of them as [`MAX_FACE_SEARCHES`]
//! allows — because by then each search is a real cost.
//!
//! And when even that finds nothing, the answer is **still** not a diagonal: it
//! is the plain two-bend elbow drawn through whatever is in the way. A line
//! that passes behind a card reads as a connector, because the lines are drawn
//! under the cards — the line goes under and out the other side, which is what
//! a wire behind a photograph does. So there is no such thing as a route that
//! failed, and nothing here returns one: there are routes that go round, and
//! routes that go behind.
//!
//! ## Two margins, not one
//!
//! [`CLEARANCE`] is the room the search keeps off a card; [`PULL_CLEARANCE`] is
//! what the taut string is pulled against. They want opposite things — the
//! search wants a generous margin, which is the air between a line and a
//! photograph, while [`pull`] drops a turn only when it can see through the
//! blocks it tests, so fattening those for the search's sake leaves every taut
//! line carrying bends it does not need. Split, each says what it means: find
//! the corridor generously, pull the string through it tightly.
//!
//! ## Lines cross freely and do not lie on each other
//!
//! Only cards are obstacles. A crossing costs nothing and never will — that is
//! what every diagram in existence does. What reads as damage is *coincidence*:
//! two routes down the same corridor do not read as two lines, they read as one
//! that is somehow heavier, and the second is invisible work. So a lattice edge
//! lying along a line already drawn simply costs more to travel, charged by the
//! length of the overlap so the price does not depend on how finely the
//! neighbourhood happens to be latticed. Nothing is refused, so no route is ever
//! lost to it.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

use crate::geometry::{point, Point, Rect};
use crate::rope::Rope;

/// The room the search keeps between a line and a card.
pub const CLEARANCE: f32 = 22.0;

/// What the taut string is pulled against. Smaller on purpose — see the module
/// note on why these are two numbers rather than one.
pub const PULL_CLEARANCE: f32 = 8.0;

/// How many cards may be in the way before the furthest start being shed.
///
/// The lattice is quadratic in this, so it is the lattice that sets the number
/// rather than the queue. Shedding the furthest obstacles is the right way to
/// lose: a card at the far end of the board was never going to be what the
/// route had to bend around.
pub const MAX_OBSTACLES: usize = 40;

/// The largest lattice a search will run over.
const MAX_NODES: usize = 12_000;

/// What a corner costs, in world units of detour.
///
/// High enough that the search will happily travel a long way to avoid one, and
/// that is the point: a route with one bend and a route with nine bends of the
/// same length are not equally good, and only this number knows it.
const TURN_COST: f32 = 60.0;

/// What travelling along a line already drawn costs, per unit of overlap.
const OVERLAP_COST: f32 = 4.0;

/// How many of the sixteen face pairs get a full lattice search.
const MAX_FACE_SEARCHES: usize = 4;

/// Which side of a card a line leaves by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Face {
    Left,
    Right,
    Bottom,
    Top,
}

impl Face {
    pub const ALL: [Face; 4] = [Face::Left, Face::Right, Face::Bottom, Face::Top];

    /// The unit step away from the card, for the stub that leaves the face.
    fn out(self) -> (f32, f32) {
        match self {
            Face::Left => (-1.0, 0.0),
            Face::Right => (1.0, 0.0),
            Face::Bottom => (0.0, -1.0),
            Face::Top => (0.0, 1.0),
        }
    }

    /// Whether the fan across this face spreads vertically.
    fn spreads_vertically(self) -> bool {
        matches!(self, Face::Left | Face::Right)
    }
}

/// One end of a line: which card, which face, and which of the lines meeting
/// that face this one is.
///
/// The slot is why this is a struct rather than a `Face`. Every line into a
/// card used to arrive at the *midpoint* of the face it came in by — right for
/// the only line meeting a card, and wrong for the fifth, since five routes
/// beginning at one point run one corridor away from it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct End {
    pub face: Face,
    /// Which line this is among those sharing the face, from zero.
    pub slot: usize,
    /// How many lines share the face.
    pub of: usize,
}

impl End {
    pub fn only(face: Face) -> Self {
        Self { face, slot: 0, of: 1 }
    }
}

/// How much of a face the fan is allowed to spread across.
///
/// Two thirds, centred, so the outermost line of a big fan still arrives well
/// inside the card's corner rather than at it — a line arriving at the very
/// corner of a card does not read as arriving at the card.
const SPREAD: f32 = 2.0 / 3.0;

/// Where on a card's face this line meets it.
pub fn anchor(box_: &Rect, end: End) -> Point {
    let mid = box_.centre();
    let frac = (end.slot + 1) as f32 / (end.of + 1) as f32;
    let off = frac - 0.5;
    if end.face.spreads_vertically() {
        let x = if end.face == Face::Left { box_.x0 } else { box_.x1 };
        point(x, mid.y + off * box_.height() * SPREAD)
    } else {
        let y = if end.face == Face::Bottom { box_.y0 } else { box_.y1 };
        point(mid.x + off * box_.width() * SPREAD, y)
    }
}

/// The four faces of `from`, best first, for a line heading towards `to`.
///
/// Ranked by the dominant axis: if the two cards are further apart sideways
/// than vertically, the sideways face pointing at the other card is best and
/// the sideways face pointing away is worst. That choice is made before
/// anything has looked at the rest of the board, which is why the ladder below
/// is willing to reconsider it — a card parked flat against the best face used
/// to fail the whole ladder while three open sides sat there, and that was the
/// common way a line ended up drawn behind something.
pub fn faces_towards(from: &Rect, to: &Rect) -> [Face; 4] {
    let a = from.centre();
    let b = to.centre();
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let across = if dx >= 0.0 { Face::Right } else { Face::Left };
    let along = if dy >= 0.0 { Face::Top } else { Face::Bottom };
    let back_across = if dx >= 0.0 { Face::Left } else { Face::Right };
    let back_along = if dy >= 0.0 { Face::Bottom } else { Face::Top };
    if dx.abs() >= dy.abs() {
        [across, along, back_along, back_across]
    } else {
        [along, across, back_across, back_along]
    }
}

/// One straight piece of a line already drawn, for the overlap charge.
#[derive(Debug, Clone, Copy)]
struct Seg {
    a: Point,
    b: Point,
}

/// What the router is given besides the two cards.
pub struct Ask<'a> {
    /// Every other card that might be in the way. The two ends are **not** in
    /// here — they are added by the router itself, at no clearance, so a route
    /// cannot pass back through the card it just left.
    pub obstacles: &'a [Rect],
    /// Lines settled earlier in this pass, to be avoided rather than refused.
    /// Order-dependent by construction: a line settled earlier is avoided and
    /// one settled later is not, which is what keeps a moved card from
    /// rerouting the whole board.
    pub avoid: &'a [Vec<Point>],
    pub clearance: f32,
    pub pull_clearance: f32,
}

impl<'a> Ask<'a> {
    pub fn new(obstacles: &'a [Rect], avoid: &'a [Vec<Point>]) -> Self {
        Self { obstacles, avoid, clearance: CLEARANCE, pull_clearance: PULL_CLEARANCE }
    }
}

/// Where the line between these two cards runs, as a polyline in world units.
///
/// Never empty and never fewer than two points. See the module note: there is
/// no failure case, only routes that go round and routes that go behind.
pub fn route(a: &Rect, b: &Rect, from: End, to: End, ask: &Ask) -> Vec<Point> {
    let start = anchor(a, from);
    let goal = anchor(b, to);
    let avoid: Vec<Seg> = ask.avoid.iter().flat_map(|line| segments(line)).collect();

    let ranked_a = faces_towards(a, b);
    let ranked_b = faces_towards(b, a);

    // Rung one: the faces the geometry picked, conceding room three times over.
    for room in [ask.clearance, ask.clearance / 3.0, 0.0] {
        if let Some(path) = attempt(a, b, from, to, start, goal, ask, &avoid, room, false) {
            return path;
        }
    }
    // Still nothing, so the one case margin cannot answer: a card lying *on
    // top of* an end. Nothing can be routed around that, so it stops counting.
    if let Some(path) = attempt(a, b, from, to, start, goal, ask, &avoid, 0.0, true) {
        return path;
    }

    // Rung two: the cheap two-bend elbow, on every one of the sixteen pairs.
    // No search, so all sixteen are affordable.
    let pairs = face_pairs(&ranked_a, &ranked_b, from, to);
    // Neither the obstacles nor the clearance change across the sixteen pairs,
    // so the wall set is built once rather than once per pair.
    let blocks = walls(a, b, ask.obstacles, ask.clearance, false);
    for (fa, fb) in &pairs {
        let (sa, sb) = (anchor(a, *fa), anchor(b, *fb));
        if let Some(bent) = elbow(sa, sb, *fa, *fb, &blocks) {
            return bent;
        }
    }

    // Rung three: a real search on a few of the other pairs.
    for (fa, fb) in pairs.iter().take(MAX_FACE_SEARCHES) {
        let (sa, sb) = (anchor(a, *fa), anchor(b, *fb));
        for room in [ask.clearance / 3.0, 0.0] {
            if let Some(path) = attempt(a, b, *fa, *fb, sa, sb, ask, &avoid, room, false) {
                return path;
            }
        }
    }

    // And the last resort, which is an elbow drawn through whatever is left.
    // Not a diagonal, and not marked as a failure: a state worth marking as a
    // failure has to look like one, and this looks like a wire behind a photo.
    forced_elbow(start, goal, from.face)
}

/// One rung of the ladder: build the lattice at this clearance and search it.
///
/// The two cards, the two faces, the two anchors, the margin and whether the
/// cards smothering an end still count — which is a great many arguments, and
/// they are all genuinely per-rung. Bundling them into a struct would mean
/// building one at every rung to hand it straight back, so the shape stays as
/// it is and the lint stays silenced here rather than everywhere.
#[allow(clippy::too_many_arguments)]
fn attempt(
    a: &Rect,
    b: &Rect,
    from: End,
    to: End,
    start: Point,
    goal: Point,
    ask: &Ask,
    avoid: &[Seg],
    room: f32,
    forgive_covering: bool,
) -> Option<Vec<Point>> {
    let mut blocks = walls(a, b, ask.obstacles, room, forgive_covering);
    let (sx, sy) = from.face.out();
    let (gx, gy) = to.face.out();
    // The stub is how a line leaves a face: straight out, far enough to clear
    // the card's own margin, before it is allowed to turn.
    let step = room.max(1.0);
    let stub_a = point(start.x + sx * step, start.y + sy * step);
    let stub_b = point(goal.x + gx * step, goal.y + gy * step);

    let (xs, ys) = lattice(&mut blocks, &[start, goal, stub_a, stub_b])?;
    let found = search(&xs, &ys, &blocks, avoid, stub_a, stub_b)?;

    let mut path = Vec::with_capacity(found.len() + 2);
    path.push(start);
    path.extend(found);
    path.push(goal);
    let pulled =
        pull(&path, &walls(a, b, ask.obstacles, ask.pull_clearance.min(room), forgive_covering));
    Some(tidy(&pulled))
}

/// The obstacle set for one attempt.
///
/// The two ends go in **uninflated**. A route may not pass back through the
/// card it left, and giving those cards a margin instead would push every
/// anchor off its own face.
fn walls(a: &Rect, b: &Rect, obstacles: &[Rect], room: f32, forgive_covering: bool) -> Vec<Rect> {
    let mut out: Vec<Rect> = obstacles
        .iter()
        .filter(|r| !forgive_covering || !(r.intersects(a) || r.intersects(b)))
        .map(|r| r.inflate(room))
        .collect();
    if out.len() > MAX_OBSTACLES {
        // Shed the furthest, which were never what the route had to bend
        // around. A lattice too large to search gives up cards, not the route.
        // What survives is unordered — nothing downstream cares which of the
        // kept obstacles comes first — so the key is worked out once per rect
        // and a partition finds the nearest `MAX_OBSTACLES` rather than a full
        // sort recomputing every centre on every comparison.
        let mid = point((a.centre().x + b.centre().x) / 2.0, (a.centre().y + b.centre().y) / 2.0);
        let d = |r: &Rect| {
            let c = r.centre();
            (c.x - mid.x).powi(2) + (c.y - mid.y).powi(2)
        };
        let mut keyed: Vec<(f32, Rect)> = out.into_iter().map(|r| (d(&r), r)).collect();
        keyed.select_nth_unstable_by(MAX_OBSTACLES - 1, |p, q| p.0.total_cmp(&q.0));
        keyed.truncate(MAX_OBSTACLES);
        out = keyed.into_iter().map(|(_, r)| r).collect();
    }
    out.push(*a);
    out.push(*b);
    out
}

/// The candidate lines, from the obstacles' own edges and the four fixed points.
///
/// Returns `None` when the lattice would be too large to search even after
/// shedding — the caller drops to the next rung rather than hanging.
fn lattice(blocks: &mut Vec<Rect>, fixed: &[Point]) -> Option<(Vec<f32>, Vec<f32>)> {
    loop {
        let mut xs: Vec<f32> = Vec::with_capacity(blocks.len() * 2 + fixed.len());
        let mut ys: Vec<f32> = Vec::with_capacity(blocks.len() * 2 + fixed.len());
        for r in blocks.iter() {
            xs.push(r.x0);
            xs.push(r.x1);
            ys.push(r.y0);
            ys.push(r.y1);
        }
        for p in fixed {
            xs.push(p.x);
            ys.push(p.y);
        }
        dedupe(&mut xs);
        dedupe(&mut ys);
        if xs.len() * ys.len() <= MAX_NODES {
            return Some((xs, ys));
        }
        if blocks.len() <= 2 {
            // Only the two ends left and it is still too large, which cannot
            // happen with four fixed points — but the loop does not get to
            // depend on that.
            return None;
        }
        blocks.remove(blocks.len() - 3);
    }
}

/// Two lattice lines closer together than this are the same line.
const SAME: f32 = 0.01;

fn dedupe(v: &mut Vec<f32>) {
    v.sort_by(f32::total_cmp);
    v.dedup_by(|a, b| (*a - *b).abs() < SAME);
}

fn nearest(v: &[f32], at: f32) -> usize {
    v.binary_search_by(|p| p.total_cmp(&at)).unwrap_or_else(|n| {
        if n == 0 {
            0
        } else if n >= v.len() {
            v.len() - 1
        } else if (v[n] - at).abs() < (at - v[n - 1]).abs() {
            n
        } else {
            n - 1
        }
    })
}

/// Whether an axis-aligned segment passes through a box's interior.
///
/// **Strictly** inside on the crossing axis, so a segment running exactly along
/// a box's edge is clear. That is what makes the no-clearance rung mean
/// something: at zero margin only real overlap blocks, and hugging is allowed.
fn crosses(a: Point, b: Point, r: &Rect) -> bool {
    if (a.y - b.y).abs() < SAME {
        let (lo, hi) = if a.x <= b.x { (a.x, b.x) } else { (b.x, a.x) };
        a.y > r.y0 + SAME && a.y < r.y1 - SAME && hi > r.x0 + SAME && lo < r.x1 - SAME
    } else {
        let (lo, hi) = if a.y <= b.y { (a.y, b.y) } else { (b.y, a.y) };
        a.x > r.x0 + SAME && a.x < r.x1 - SAME && hi > r.y0 + SAME && lo < r.y1 - SAME
    }
}

fn clear(a: Point, b: Point, blocks: &[Rect]) -> bool {
    !blocks.iter().any(|r| crosses(a, b, r))
}

fn segments(line: &[Point]) -> Vec<Seg> {
    line.windows(2).map(|w| Seg { a: w[0], b: w[1] }).collect()
}

/// How far this segment runs on top of a line already drawn.
fn shared(a: Point, b: Point, avoid: &[Seg]) -> f32 {
    let horizontal = (a.y - b.y).abs() < SAME;
    let mut total = 0.0;
    for s in avoid {
        let s_horizontal = (s.a.y - s.b.y).abs() < SAME;
        if s_horizontal != horizontal {
            continue;
        }
        let (off, s_off) = if horizontal { (a.y, s.a.y) } else { (a.x, s.a.x) };
        if (off - s_off).abs() > SAME {
            continue;
        }
        let (lo, hi) =
            if horizontal { (a.x.min(b.x), a.x.max(b.x)) } else { (a.y.min(b.y), a.y.max(b.y)) };
        let (s_lo, s_hi) = if horizontal {
            (s.a.x.min(s.b.x), s.a.x.max(s.b.x))
        } else {
            (s.a.y.min(s.b.y), s.a.y.max(s.b.y))
        };
        total += (hi.min(s_hi) - lo.max(s_lo)).max(0.0);
    }
    total
}

/// A node the search is standing on, and which way it arrived.
///
/// The arrival axis is part of the state because the turn penalty is: the same
/// lattice point reached going across and reached going up are not the same
/// place to be, since one of them is about to pay for a corner and the other
/// is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct Step {
    node: u32,
    horizontal: bool,
}

struct Queued {
    f: f32,
    step: Step,
}

impl PartialEq for Queued {
    fn eq(&self, other: &Self) -> bool {
        self.f == other.f
    }
}
impl Eq for Queued {}
impl PartialOrd for Queued {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Queued {
    /// Reversed, because `BinaryHeap` is a max-heap and A\* wants the smallest
    /// `f` next.
    fn cmp(&self, other: &Self) -> Ordering {
        other.f.total_cmp(&self.f)
    }
}

/// A\* over the lattice, with distance plus a penalty per turn.
fn search(
    xs: &[f32],
    ys: &[f32],
    blocks: &[Rect],
    avoid: &[Seg],
    start: Point,
    goal: Point,
) -> Option<Vec<Point>> {
    let (nx, ny) = (xs.len(), ys.len());
    if nx < 2 || ny < 2 {
        return None;
    }
    let at = |ix: usize, iy: usize| (iy * nx + ix) as u32;
    let xy = |n: u32| ((n as usize % nx), (n as usize / nx));
    let world = |n: u32| {
        let (ix, iy) = xy(n);
        point(xs[ix], ys[iy])
    };

    let start_n = at(nearest(xs, start.x), nearest(ys, start.y));
    let goal_n = at(nearest(xs, goal.x), nearest(ys, goal.y));
    if start_n == goal_n {
        return Some(vec![world(start_n)]);
    }
    let target = world(goal_n);
    let guess = |n: u32| {
        let p = world(n);
        (p.x - target.x).abs() + (p.y - target.y).abs()
    };

    // Flat vectors rather than hash maps: `node < nx * ny <= MAX_NODES`, so
    // every `Step` packs into a small dense index — see [`packed`] — and a
    // slot is a pointer add rather than a hash and a probe. Neither map had
    // any ordering effect on the search, so this changes nothing about which
    // route is found.
    let slots = 2 * nx * ny;
    let mut best: Vec<f32> = vec![f32::INFINITY; slots];
    // `u32::MAX` stands for "no predecessor" — never a real packed index,
    // since those top out at `slots - 1`.
    let mut came: Vec<u32> = vec![u32::MAX; slots];
    let mut open = BinaryHeap::new();
    // Both arrival axes are free at the start: the first move pays no corner,
    // whichever way it goes.
    for horizontal in [true, false] {
        let step = Step { node: start_n, horizontal };
        best[packed(step) as usize] = 0.0;
        open.push(Queued { f: guess(start_n), step });
    }

    while let Some(Queued { step, .. }) = open.pop() {
        let so_far = best[packed(step) as usize];
        if step.node == goal_n {
            return Some(unwind(&came, step, world));
        }
        let (ix, iy) = xy(step.node);
        // Loop-invariant across all four neighbours, so worked out once
        // rather than once per candidate direction.
        let from = world(step.node);
        let consider = |dx: i64,
                        dy: i64,
                        open: &mut BinaryHeap<Queued>,
                        came: &mut [u32],
                        best: &mut [f32]| {
            let (nix, niy) = (ix as i64 + dx, iy as i64 + dy);
            if nix < 0 || niy < 0 || nix >= nx as i64 || niy >= ny as i64 {
                return;
            }
            let next = at(nix as usize, niy as usize);
            let b = world(next);
            if !clear(from, b, blocks) {
                return;
            }
            let horizontal = dy == 0;
            let length = (b.x - from.x).abs() + (b.y - from.y).abs();
            let turn = if horizontal == step.horizontal { 0.0 } else { TURN_COST };
            let cost = so_far + length + turn + shared(from, b, avoid) * OVERLAP_COST;
            let onward = Step { node: next, horizontal };
            let oi = packed(onward) as usize;
            if cost < best[oi] {
                best[oi] = cost;
                came[oi] = packed(step);
                open.push(Queued { f: cost + guess(next), step: onward });
            }
        };
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            consider(dx, dy, &mut open, &mut came, &mut best);
        }
    }
    None
}

/// A [`Step`] packed into a dense index: `node * 2 + horizontal`.
///
/// `node < nx * ny <= MAX_NODES`, so this always fits comfortably in a `u32`
/// — it is what lets [`search`] use flat vectors instead of a
/// `HashMap<Step, _>`.
fn packed(step: Step) -> u32 {
    step.node * 2 + step.horizontal as u32
}

fn unwind(came: &[u32], end: Step, world: impl Fn(u32) -> Point) -> Vec<Point> {
    let mut out = vec![world(end.node)];
    let mut at = end;
    while came[packed(at) as usize] != u32::MAX {
        let prev = came[packed(at) as usize];
        at = Step { node: prev / 2, horizontal: prev % 2 == 1 };
        out.push(world(at.node));
        if out.len() > came.len() + 2 {
            break;
        }
    }
    out.reverse();
    out
}

/// Pull the string taut: drop every turn whose neighbours can see each other.
///
/// Greedy from the front, taking the furthest point that can be reached in one
/// straight line or one corner. On a clear board this collapses the whole
/// corridor to a single ruled line; with something in the way it bends by
/// however much the detour actually needs and no more.
fn pull(path: &[Point], blocks: &[Rect]) -> Vec<Point> {
    if path.len() < 3 {
        return path.to_vec();
    }
    let mut out = vec![path[0]];
    let mut i = 0;
    while i < path.len() - 1 {
        let mut took = i + 1;
        let mut through: Option<Point> = None;
        for j in (i + 2..path.len()).rev() {
            let (a, b) = (path[i], path[j]);
            let straight = (a.x - b.x).abs() < SAME || (a.y - b.y).abs() < SAME;
            if straight && clear(a, b, blocks) {
                took = j;
                through = None;
                break;
            }
            let corners = [point(b.x, a.y), point(a.x, b.y)];
            if let Some(c) =
                corners.into_iter().find(|c| clear(a, *c, blocks) && clear(*c, b, blocks))
            {
                took = j;
                through = Some(c);
                break;
            }
        }
        if let Some(c) = through {
            out.push(c);
        }
        out.push(path[took]);
        i = took;
    }
    out
}

/// Drop repeated and collinear points, so a straight run is one segment.
fn tidy(path: &[Point]) -> Vec<Point> {
    let mut out: Vec<Point> = Vec::with_capacity(path.len());
    for p in path {
        if out.last().is_some_and(|q| (q.x - p.x).abs() < SAME && (q.y - p.y).abs() < SAME) {
            continue;
        }
        out.push(*p);
    }
    let mut n = 1;
    while n + 1 < out.len() {
        let (a, b, c) = (out[n - 1], out[n], out[n + 1]);
        let flat = (a.y - b.y).abs() < SAME && (b.y - c.y).abs() < SAME;
        let upright = (a.x - b.x).abs() < SAME && (b.x - c.x).abs() < SAME;
        if flat || upright {
            out.remove(n);
        } else {
            n += 1;
        }
    }
    if out.len() < 2 {
        out.push(*path.last().unwrap_or(&point(0.0, 0.0)));
    }
    out
}

/// The largest a corner is rounded by, in world units.
///
/// Generous on purpose, because this is the number that decides whether a
/// route reads as a line or as a diagram: a two-unit fillet on a right angle
/// looks like a fault in the rasteriser rather than like a curve. What stops
/// it swallowing a short jog is the leg cap in [`smooth`], not this.
const CORNER: f32 = 36.0;

/// How many points one rounded corner is sampled into.
///
/// A corner is a quadratic, so eight is well inside a pixel at the sizes a
/// line is looked at — and a route with four bends still comes back with
/// fewer points than the twenty-four a [`Rope`] is sampled into, which is the
/// budget everything downstream was already built for.
const CORNER_STEPS: usize = 8;

/// The shortest radius worth rounding by. Below it, the corner stays square.
const CORNER_LEAST: f32 = 1.0;

/// Round the corners of a routed path, without letting it into a card.
///
/// The route is a stack of right angles because the search is — see the module
/// note — and that is the honest shape rather than the nice one. This replaces
/// each corner with a quadratic through the same turn, which is a **corner
/// cut**: a quadratic never leaves the triangle its three points make, so what
/// was clear of a card before is clear of it after.
///
/// That containment is the whole reason this is not the interpolating spline
/// it ends up looking like. A Catmull-Rom through the same points overshoots
/// *outside* them, and outside them is exactly where the card the route just
/// bent around is sitting — a smoother line that occasionally drives through a
/// photograph is a worse line than a square one.
///
/// The radius is still checked against the obstacles rather than trusted,
/// because the last rungs of the ladder route at no clearance at all and a
/// corner hugging an edge has no room to be cut. A corner that cannot be
/// rounded stays square, which is the same answer this module gives everywhere
/// else: concede the shape before conceding the route.
pub fn smooth(path: &[Point], blocks: &[Rect]) -> Vec<Point> {
    if path.len() < 3 {
        return path.to_vec();
    }
    let mut out = Vec::with_capacity(path.len() + (path.len() - 2) * CORNER_STEPS);
    out.push(path[0]);
    for i in 1..path.len() - 1 {
        let (prev, here, next) = (path[i - 1], path[i], path[i + 1]);
        // Half of each leg, so two corners sharing a run meet in the middle of
        // it rather than fighting over it — and so the stub that leaves a
        // card's face keeps half its length pointing straight out of the face.
        let mut radius = CORNER.min(span(prev, here) / 2.0).min(span(here, next) / 2.0);
        let rounded = loop {
            if radius < CORNER_LEAST {
                break None;
            }
            let arc = fillet(prev, here, next, radius);
            if arc.windows(2).all(|w| !blocks.iter().any(|r| meets(w[0], w[1], r))) {
                break Some(arc);
            }
            radius /= 2.0;
        };
        match rounded {
            Some(arc) => out.extend(arc),
            None => out.push(here),
        }
    }
    out.push(path[path.len() - 1]);
    out
}

/// One corner as a sampled quadratic: `radius` back along each leg, bending
/// through the turn with the corner itself as the control point.
fn fillet(prev: Point, here: Point, next: Point, radius: f32) -> Vec<Point> {
    let start = along(here, prev, radius);
    let end = along(here, next, radius);
    (0..=CORNER_STEPS)
        .map(|n| {
            let t = n as f32 / CORNER_STEPS as f32;
            let u = 1.0 - t;
            point(
                u * u * start.x + 2.0 * u * t * here.x + t * t * end.x,
                u * u * start.y + 2.0 * u * t * here.y + t * t * end.y,
            )
        })
        .collect()
}

/// The point `d` from `from`, on the way to `to`.
fn along(from: Point, to: Point, d: f32) -> Point {
    let len = span(from, to).max(1e-6);
    point(from.x + (to.x - from.x) / len * d, from.y + (to.y - from.y) / len * d)
}

/// How far apart two points are.
fn span(a: Point, b: Point) -> f32 {
    ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt()
}

/// The sixteen face pairs, best first, with the one already tried left out.
fn face_pairs(a: &[Face; 4], b: &[Face; 4], had_a: End, had_b: End) -> Vec<(End, End)> {
    let mut out = Vec::with_capacity(15);
    for (i, fa) in a.iter().enumerate() {
        for (j, fb) in b.iter().enumerate() {
            if *fa == had_a.face && *fb == had_b.face {
                continue;
            }
            out.push((i + j, End { face: *fa, ..had_a }, End { face: *fb, ..had_b }));
        }
    }
    out.sort_by_key(|(rank, _, _)| *rank);
    out.into_iter().map(|(_, x, y)| (x, y)).collect()
}

/// A two-bend elbow between two anchors, if either orientation is clear.
///
/// The first turn has to agree with the face the line left by — an elbow that
/// starts by running back across the card it came out of is not an elbow, it is
/// a line drawn through a card.
fn elbow(a: Point, b: Point, fa: End, fb: End, blocks: &[Rect]) -> Option<Vec<Point>> {
    let mid_x = point(b.x, a.y);
    let mid_y = point(a.x, b.y);
    let leaves_flat = matches!(fa.face, Face::Left | Face::Right);
    let arrives_flat = matches!(fb.face, Face::Left | Face::Right);
    let mut tries = Vec::new();
    if leaves_flat || !arrives_flat {
        tries.push(mid_x);
    }
    if !leaves_flat || arrives_flat {
        tries.push(mid_y);
    }
    for c in tries {
        if clear(a, c, blocks) && clear(c, b, blocks) {
            return Some(tidy(&[a, c, b]));
        }
    }
    None
}

/// The last resort: an elbow through whatever is in the way.
fn forced_elbow(a: Point, b: Point, face: Face) -> Vec<Point> {
    let corner =
        if matches!(face, Face::Left | Face::Right) { point(b.x, a.y) } else { point(a.x, b.y) };
    tidy(&[a, corner, b])
}

/// One line to work out, by index into the card list handed to [`pass`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Link {
    pub a: usize,
    pub b: usize,
}

/// Route a whole board's worth of lines, spreading the fan at each card.
///
/// Two things happen here that cannot happen one line at a time. The faces are
/// chosen for every line **first**, so lines sharing a face can be counted and
/// given slots across it — and sorted by where their far end sits along the
/// spread axis, so the fan does not cross itself. And each route is added to
/// the avoid set as it settles, so the next one prefers a different corridor.
///
/// Order-dependent by construction, and deliberately: the alternative is
/// rerouting the whole board whenever anything moves.
pub fn pass(cards: &[Rect], links: &[Link], obstacles: &[Rect]) -> Vec<Vec<Point>> {
    let ends = ends(cards, links);
    let mut out: Vec<Vec<Point>> = Vec::with_capacity(links.len());
    for (n, link) in links.iter().enumerate() {
        // Every card that is not one of the two ends is something to go round.
        let walls: Vec<Rect> = cards
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != link.a && *i != link.b)
            .map(|(_, r)| *r)
            .chain(obstacles.iter().copied())
            .collect();
        let ask = Ask::new(&walls, &out);
        let (from, to) = ends[n];
        out.push(route(&cards[link.a], &cards[link.b], from, to, &ask));
    }
    out
}

/// Which face each line leaves and arrives by, and where in the fan it sits.
///
/// Separate from [`pass`] because it is the half that has to see every line at
/// once, and it is also the cheap half — no search happens here. A caller that
/// wants to route lazily (only the lines it can currently see, say) still has
/// to run this over *all* of them, or a card's fan changes shape depending on
/// which way the board is scrolled.
pub fn ends(cards: &[Rect], links: &[Link]) -> Vec<(End, End)> {
    let mut chosen: Vec<(Face, Face)> = Vec::with_capacity(links.len());
    for link in links {
        let (a, b) = (&cards[link.a], &cards[link.b]);
        chosen.push((faces_towards(a, b)[0], faces_towards(b, a)[0]));
    }

    // Group by card and face, so each line knows how many it is sharing with.
    let mut fans: HashMap<(usize, Face), Vec<usize>> = HashMap::new();
    for (n, link) in links.iter().enumerate() {
        fans.entry((link.a, chosen[n].0)).or_default().push(n);
        fans.entry((link.b, chosen[n].1)).or_default().push(n);
    }
    let mut slot: HashMap<(usize, Face, usize), (usize, usize)> = HashMap::new();
    for ((card, face), mut members) in fans {
        // Sorted by where the far end sits along the axis the fan spreads on.
        // Unsorted, five lines out of one face cross each other on the way.
        members.sort_by(|&p, &q| {
            let far = |n: usize| {
                let link = links[n];
                let other = if link.a == card { link.b } else { link.a };
                let c = cards[other].centre();
                if face.spreads_vertically() {
                    c.y
                } else {
                    c.x
                }
            };
            far(p).total_cmp(&far(q))
        });
        let of = members.len();
        for (i, n) in members.into_iter().enumerate() {
            slot.insert((card, face, n), (i, of));
        }
    }

    links
        .iter()
        .enumerate()
        .map(|(n, link)| {
            let (fa, fb) = chosen[n];
            let end = |card: usize, face: Face| {
                let (i, of) = slot.get(&(card, face, n)).copied().unwrap_or((0, 1));
                End { face, slot: i, of }
            };
            (end(link.a, fa), end(link.b, fb))
        })
        .collect()
}

/// The cheapest set of lines that joins every one of these cards.
///
/// A minimum spanning tree over the distances between centres — Prim's, because
/// the graph is complete and dense and a heap would cost more than the scan it
/// saved. This is what the board's automatic web used to *be*, before lines
/// became something somebody draws; it survives as the generator, run once on
/// demand over a selection, and what it emits are ordinary connections that
/// route and can be deleted like any other.
///
/// `n - 1` links for `n` cards, and none at all for fewer than two.
pub fn spanning(cards: &[Rect]) -> Vec<Link> {
    if cards.len() < 2 {
        return Vec::new();
    }
    let far = |i: usize, j: usize| {
        let (a, b) = (cards[i].centre(), cards[j].centre());
        (a.x - b.x).powi(2) + (a.y - b.y).powi(2)
    };
    let mut joined = vec![false; cards.len()];
    joined[0] = true;
    let mut out = Vec::with_capacity(cards.len() - 1);
    for _ in 1..cards.len() {
        let mut best: Option<(usize, usize, f32)> = None;
        for (i, inside) in joined.iter().enumerate() {
            if !inside {
                continue;
            }
            for (j, outside) in joined.iter().enumerate() {
                if *outside {
                    continue;
                }
                let d = far(i, j);
                if best.is_none_or(|(_, _, had)| d < had) {
                    best = Some((i, j, d));
                }
            }
        }
        let Some((i, j, _)) = best else { break };
        joined[j] = true;
        out.push(Link { a: i, b: j });
    }
    out
}

/// What the line between two cards turned out to be.
///
/// Two shapes, and which one you get is a fact about the board rather than a
/// setting: a rope is the nicer line and an elbow is the honest one, so the
/// curve is drawn wherever the curve is *clear* and the search only takes over
/// where it is not. That keeps the ordinary board looking like a moodboard and
/// the crowded one readable, without asking anybody to choose.
#[derive(Debug, Clone, PartialEq)]
pub enum Line {
    /// Nothing was in the way. The ordinary [`Rope`] stands, and the caller
    /// draws it as the curve it already knew how to draw.
    Curve(Rope),
    /// Something was, so the line goes round it: a polyline in world units,
    /// never fewer than two points. Routed as right angles and then handed to
    /// [`smooth`], so the long runs are still axis-aligned and the turns
    /// between them are curves rather than corners.
    Around(Vec<Point>),
}

/// How much of a card a curve may clip before it counts as going through it.
///
/// A rope that shaves a corner by a couple of units reads as passing the card,
/// not as crossing it, and forcing an elbow for that would replace a good line
/// with a worse one. Deflating the obstacle is the cheap way to say so.
const GRAZE: f32 = 5.0;

/// The line between two cards, curved where it can be and routed where it cannot.
///
/// `obstacles` must **not** include the two cards being joined: every line
/// starts and ends on a card, so a card that is one of its own obstacles has no
/// line at all. Everything else near the pair belongs in there.
pub fn line(a: &Rect, b: &Rect, obstacles: &[Rect]) -> Line {
    let rope = Rope::auto(*a, *b);
    let samples = rope.samples();
    let blocked = obstacles.iter().any(|r| {
        let room = r.inflate(-GRAZE);
        room.x1 > room.x0
            && room.y1 > room.y0
            && samples.windows(2).any(|w| meets(w[0], w[1], &room))
    });
    if !blocked {
        return Line::Curve(rope);
    }
    let from = End::only(faces_towards(a, b)[0]);
    let to = End::only(faces_towards(b, a)[0]);
    // Rounded here rather than inside [`route`], because the right angles are
    // what the search, the pull and the overlap charge are all written in
    // terms of — the curve is how the answer is *drawn*, and this is the
    // boundary where a route stops being worked out and starts being a line.
    let path = route(a, b, from, to, &Ask::new(obstacles, &[]));
    Line::Around(smooth(&path, obstacles))
}

/// Whether a segment of any slope meets a box.
///
/// Slab clipping, which is the short way to say it: the segment is a ray with a
/// start and an end, and the box is the intersection of two intervals, so the
/// segment meets the box exactly when the two clipped parameter ranges overlap.
/// [`crosses`] is the axis-aligned special case and is kept separate because
/// the router asks it several hundred thousand times per search and does not
/// need the general answer.
fn meets(a: Point, b: Point, r: &Rect) -> bool {
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for (from, to, edge0, edge1) in [(a.x, b.x, r.x0, r.x1), (a.y, b.y, r.y0, r.y1)] {
        let d = to - from;
        if d.abs() < SAME {
            // Parallel to this pair of edges: either wholly inside the slab or
            // wholly outside it, and outside means there is nothing to clip.
            if from < edge0 || from > edge1 {
                return false;
            }
            continue;
        }
        let (mut t0, mut t1) = ((edge0 - from) / d, (edge1 - from) / d);
        if t0 > t1 {
            std::mem::swap(&mut t0, &mut t1);
        }
        lo = lo.max(t0);
        hi = hi.min(t1);
        if lo > hi {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn box_(cx: f32, cy: f32, w: f32, h: f32) -> Rect {
        Rect::centred(cx, cy, w, h)
    }

    fn orthogonal(path: &[Point]) -> bool {
        path.windows(2).all(|w| (w[0].x - w[1].x).abs() < SAME || (w[0].y - w[1].y).abs() < SAME)
    }

    fn hits(path: &[Point], r: &Rect) -> bool {
        path.windows(2).any(|w| crosses(w[0], w[1], r))
    }

    /// The same question for a path that has been rounded, whose corners are
    /// no longer axis-aligned and so are not [`crosses`]'s to answer.
    fn touches(path: &[Point], r: &Rect) -> bool {
        path.windows(2).any(|w| meets(w[0], w[1], r))
    }

    #[test]
    fn two_cards_with_nothing_between_them_are_joined_by_one_line() {
        let (a, b) = (box_(0.0, 0.0, 100.0, 100.0), box_(400.0, 0.0, 100.0, 100.0));
        let path =
            route(&a, &b, End::only(Face::Right), End::only(Face::Left), &Ask::new(&[], &[]));
        assert_eq!(path.len(), 2, "{path:?}");
        assert!((path[0].y - path[1].y).abs() < SAME);
    }

    #[test]
    fn every_line_is_made_of_right_angles() {
        // The one property the whole module exists to hold. A diagonal is what
        // this refuses to draw even when it has run out of ideas.
        let (a, b) = (box_(0.0, 0.0, 100.0, 100.0), box_(370.0, 260.0, 100.0, 100.0));
        for room in [CLEARANCE, 0.0] {
            let mut ask = Ask::new(&[], &[]);
            ask.clearance = room;
            let path = route(&a, &b, End::only(Face::Right), End::only(Face::Left), &ask);
            assert!(orthogonal(&path), "{path:?}");
        }
    }

    #[test]
    fn a_card_in_the_way_is_gone_round_and_not_through() {
        let (a, b) = (box_(0.0, 0.0, 100.0, 100.0), box_(600.0, 0.0, 100.0, 100.0));
        let wall = box_(300.0, 0.0, 120.0, 400.0);
        let path =
            route(&a, &b, End::only(Face::Right), End::only(Face::Left), &Ask::new(&[wall], &[]));
        assert!(orthogonal(&path), "{path:?}");
        assert!(!hits(&path, &wall), "went through the wall: {path:?}");
        assert!(path.len() >= 4, "a detour needs bends: {path:?}");
    }

    #[test]
    fn the_way_round_is_taut_rather_than_a_staircase() {
        let (a, b) = (box_(0.0, 0.0, 100.0, 100.0), box_(600.0, 0.0, 100.0, 100.0));
        let wall = box_(300.0, 0.0, 120.0, 200.0);
        let path =
            route(&a, &b, End::only(Face::Right), End::only(Face::Left), &Ask::new(&[wall], &[]));
        // Over, across, back: four bends at the very most, and that is with the
        // stub counted. A staircase would be a dozen.
        assert!(path.len() <= 6, "{path:?}");
    }

    #[test]
    fn a_fan_of_lines_arrives_spread_across_the_face() {
        let hub = box_(0.0, 0.0, 100.0, 300.0);
        let cards = vec![
            hub,
            box_(500.0, 200.0, 80.0, 80.0),
            box_(500.0, 0.0, 80.0, 80.0),
            box_(500.0, -200.0, 80.0, 80.0),
        ];
        let links = vec![Link { a: 0, b: 1 }, Link { a: 0, b: 2 }, Link { a: 0, b: 3 }];
        let paths = pass(&cards, &links, &[]);
        let mut ys: Vec<f32> = paths.iter().map(|p| p[0].y).collect();
        assert!(paths.iter().all(|p| (p[0].x - hub.x1).abs() < SAME), "{paths:?}");
        ys.sort_by(f32::total_cmp);
        ys.dedup_by(|a, b| (*a - *b).abs() < 1.0);
        assert_eq!(ys.len(), 3, "three lines left the same point: {paths:?}");
    }

    #[test]
    fn the_fan_does_not_cross_itself() {
        let hub = box_(0.0, 0.0, 100.0, 300.0);
        let cards = vec![hub, box_(500.0, -200.0, 80.0, 80.0), box_(500.0, 200.0, 80.0, 80.0)];
        // The far ends are handed in out of order on purpose.
        let links = vec![Link { a: 0, b: 1 }, Link { a: 0, b: 2 }];
        let paths = pass(&cards, &links, &[]);
        // The line to the lower card leaves lower.
        assert!(paths[0][0].y < paths[1][0].y, "{paths:?}");
    }

    #[test]
    fn a_second_line_down_the_same_corridor_takes_another_one() {
        let cards = vec![
            box_(0.0, 0.0, 60.0, 60.0),
            box_(400.0, 0.0, 60.0, 60.0),
            box_(0.0, 20.0, 60.0, 60.0),
            box_(400.0, 20.0, 60.0, 60.0),
        ];
        let links = vec![Link { a: 0, b: 1 }, Link { a: 2, b: 3 }];
        let paths = pass(&cards, &links, &[]);
        // Nothing is refused, so both exist; they simply are not the same line
        // drawn twice.
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().all(|p| p.len() >= 2));
    }

    #[test]
    fn a_card_lying_on_top_of_an_end_still_gets_a_line() {
        let (a, b) = (box_(0.0, 0.0, 100.0, 100.0), box_(400.0, 0.0, 100.0, 100.0));
        // Smothering `a` entirely: no margin can answer this, and the route
        // must still come back rather than give up.
        let blanket = box_(0.0, 0.0, 400.0, 400.0);
        let path = route(
            &a,
            &b,
            End::only(Face::Right),
            End::only(Face::Left),
            &Ask::new(&[blanket], &[]),
        );
        assert!(path.len() >= 2);
        assert!(orthogonal(&path), "{path:?}");
    }

    #[test]
    fn a_route_never_comes_back_as_a_diagonal() {
        // A wall of cards with no gap at all: the answer is an elbow drawn
        // behind them, and an elbow is still right angles.
        let (a, b) = (box_(0.0, 0.0, 60.0, 60.0), box_(500.0, 300.0, 60.0, 60.0));
        let wall: Vec<Rect> =
            (-6..=6).map(|n| box_(250.0, n as f32 * 100.0, 300.0, 100.0)).collect();
        let path =
            route(&a, &b, End::only(Face::Right), End::only(Face::Left), &Ask::new(&wall, &[]));
        assert!(orthogonal(&path), "{path:?}");
        assert!(path.len() >= 2);
    }

    #[test]
    fn a_line_leaves_and_arrives_on_the_faces_it_was_told_to() {
        let (a, b) = (box_(0.0, 0.0, 100.0, 100.0), box_(0.0, 400.0, 100.0, 100.0));
        let path =
            route(&a, &b, End::only(Face::Top), End::only(Face::Bottom), &Ask::new(&[], &[]));
        assert!((path[0].y - a.y1).abs() < SAME, "{path:?}");
        assert!((path[path.len() - 1].y - b.y0).abs() < SAME, "{path:?}");
    }

    #[test]
    fn a_clear_run_between_two_cards_is_left_as_a_curve() {
        let (a, b) = (box_(0.0, 0.0, 120.0, 90.0), box_(500.0, 60.0, 120.0, 90.0));
        assert!(matches!(line(&a, &b, &[]), Line::Curve(_)));
        // And a card well off to one side does not change that.
        let aside = box_(250.0, 900.0, 120.0, 90.0);
        assert!(matches!(line(&a, &b, &[aside]), Line::Curve(_)));
    }

    #[test]
    fn a_card_squarely_in_the_way_turns_the_curve_into_an_elbow() {
        let (a, b) = (box_(0.0, 0.0, 120.0, 90.0), box_(700.0, 0.0, 120.0, 90.0));
        let wall = box_(350.0, 0.0, 160.0, 500.0);
        let Line::Around(path) = line(&a, &b, &[wall]) else {
            panic!("a rope was drawn straight through a card");
        };
        // Not `orthogonal`: what comes out of `line` has had its corners
        // rounded. What still has to be true is the thing the elbow was for.
        assert!(!touches(&path, &wall), "{path:?}");
    }

    #[test]
    fn a_rounded_corner_stays_inside_the_turn_it_replaced() {
        // The containment the module note leans on: a quadratic never leaves
        // the triangle of its three points, so every point of the arc is
        // inside the box the corner and its two legs make.
        let (prev, here, next) = (point(0.0, 0.0), point(100.0, 0.0), point(100.0, 100.0));
        let arc = fillet(prev, here, next, 36.0);
        let box_ = Rect::new(64.0, 0.0, 100.0, 36.0);
        for p in &arc {
            assert!(box_.contains(*p), "{p:?} left the corner's own box");
        }
        // And it starts and ends on the legs rather than near them.
        assert!(span(arc[0], point(64.0, 0.0)) < SAME, "{arc:?}");
        assert!(span(arc[arc.len() - 1], point(100.0, 36.0)) < SAME, "{arc:?}");
    }

    #[test]
    fn smoothing_keeps_the_two_ends_exactly_where_they_were() {
        // The ends are on the cards' faces, and a line that arrives a whisker
        // off the face it was aimed at is a line that misses its card.
        let path = vec![point(0.0, 0.0), point(200.0, 0.0), point(200.0, 200.0)];
        let out = smooth(&path, &[]);
        assert_eq!(out[0], path[0]);
        assert_eq!(out[out.len() - 1], path[2]);
        assert!(out.len() > path.len(), "nothing was rounded: {out:?}");
    }

    #[test]
    fn a_corner_gives_up_room_rather_than_run_into_a_card() {
        // A card sitting *inside* the turn, which is where the cut goes and
        // where a square corner was clear. The corner still rounds, by less.
        let path = vec![point(0.0, 0.0), point(200.0, 0.0), point(200.0, 200.0)];
        let inside = Rect::new(170.0, 4.0, 196.0, 30.0);
        assert!(!hits(&path, &inside), "the square corner was supposed to be clear");
        let out = smooth(&path, &[inside]);
        assert!(!touches(&out, &inside), "the rounding drove through a card: {out:?}");
        assert!(out.len() > path.len(), "it gave up on the corner entirely: {out:?}");
        // Tighter than it would have been with the card gone.
        let free = smooth(&path, &[]);
        assert!(out[1].x > free[1].x, "it took the same radius anyway: {out:?}");
    }

    #[test]
    fn a_corner_between_two_very_short_legs_stays_square() {
        // Nothing here is worth a curve: a radius under a world unit is a
        // corner drawn as a corner with extra points in it.
        let path = vec![point(0.0, 0.0), point(1.5, 0.0), point(1.5, 1.5)];
        assert_eq!(smooth(&path, &[]), path);
    }

    #[test]
    fn a_short_jog_is_not_swallowed_by_its_own_corners() {
        // Two corners eight units apart. Each may take half the run between
        // them and no more, so the jog survives as a jog.
        let path = vec![point(0.0, 0.0), point(100.0, 0.0), point(100.0, 8.0), point(200.0, 8.0)];
        let out = smooth(&path, &[]);
        assert!(!touches(&out, &Rect::new(-1.0, 20.0, 300.0, 40.0)), "it ballooned: {out:?}");
        // The run still gets from one end to the other, in order.
        assert!(out[0].x < out[out.len() - 1].x);
        assert!(out.windows(2).all(|w| w[1].x >= w[0].x - SAME), "it doubled back: {out:?}");
    }

    #[test]
    fn a_line_of_two_points_has_no_corners_to_round() {
        let path = vec![point(0.0, 0.0), point(100.0, 0.0)];
        assert_eq!(smooth(&path, &[]), path);
    }

    #[test]
    fn a_curve_that_shaves_a_corner_is_still_a_curve() {
        // Replacing a good line with a worse one over two units of overlap is
        // not a trade worth making.
        let (a, b) = (box_(0.0, 0.0, 120.0, 90.0), box_(600.0, 0.0, 120.0, 90.0));
        let curve = Rope::auto(a, b);
        let mid = curve.middle();
        // A card whose edge sits a whisker past the curve's midpoint.
        let graze = Rect::new(mid.x - 60.0, mid.y - 60.0, mid.x + 60.0, mid.y + 2.0);
        assert!(matches!(line(&a, &b, &[graze]), Line::Curve(_)));
    }

    #[test]
    fn a_diagonal_segment_is_tested_against_the_whole_box() {
        // `crosses` only answers for axis-aligned segments, and a rope is not
        // one — this is the general case it needed.
        let r = Rect::new(0.0, 0.0, 100.0, 100.0);
        assert!(meets(point(-50.0, -50.0), point(150.0, 150.0), &r));
        assert!(!meets(point(-50.0, 200.0), point(150.0, 400.0), &r));
        // Ending short of the box is not meeting it.
        assert!(!meets(point(-50.0, -50.0), point(-10.0, -10.0), &r));
        // And a segment wholly inside counts.
        assert!(meets(point(10.0, 10.0), point(20.0, 20.0), &r));
    }

    #[test]
    fn a_web_over_a_selection_joins_everything_once() {
        let cards = vec![
            box_(0.0, 0.0, 60.0, 60.0),
            box_(100.0, 0.0, 60.0, 60.0),
            box_(200.0, 0.0, 60.0, 60.0),
            box_(1000.0, 0.0, 60.0, 60.0),
        ];
        let links = spanning(&cards);
        assert_eq!(links.len(), 3, "n cards want n-1 lines");
        // Every card is reachable, which is what "spanning" means.
        let mut seen = vec![false; cards.len()];
        seen[0] = true;
        for _ in 0..links.len() {
            for l in &links {
                if seen[l.a] || seen[l.b] {
                    seen[l.a] = true;
                    seen[l.b] = true;
                }
            }
        }
        assert!(seen.iter().all(|s| *s), "{links:?}");
        // And the far card is joined to its nearest neighbour rather than to
        // whichever one happened to be first.
        assert!(links.contains(&Link { a: 2, b: 3 }), "{links:?}");
    }

    #[test]
    fn one_card_is_a_web_of_nothing() {
        assert!(spanning(&[box_(0.0, 0.0, 10.0, 10.0)]).is_empty());
        assert!(spanning(&[]).is_empty());
    }

    #[test]
    fn the_faces_chosen_point_at_the_other_card() {
        let a = box_(0.0, 0.0, 100.0, 100.0);
        let right = box_(500.0, 20.0, 100.0, 100.0);
        assert_eq!(faces_towards(&a, &right)[0], Face::Right);
        assert_eq!(faces_towards(&right, &a)[0], Face::Left);
        let above = box_(20.0, 500.0, 100.0, 100.0);
        assert_eq!(faces_towards(&a, &above)[0], Face::Top);
    }

    #[test]
    fn a_board_of_obstacles_too_large_to_search_still_answers() {
        let (a, b) = (box_(0.0, 0.0, 60.0, 60.0), box_(4000.0, 0.0, 60.0, 60.0));
        let many: Vec<Rect> = (0..300)
            .map(|n| box_((n % 30) as f32 * 130.0, (n / 30) as f32 * 130.0, 60.0, 60.0))
            .collect();
        let path =
            route(&a, &b, End::only(Face::Right), End::only(Face::Left), &Ask::new(&many, &[]));
        assert!(orthogonal(&path), "{path:?}");
        assert!(path.len() >= 2);
    }
}
