//! The lines between cards: which ones to work out, and when.
//!
//! [`mbrd_core::rope`] knows the shape of one curve and [`mbrd_core::route`]
//! knows how to get round a card that is in the way. Neither knows there is
//! more than one line on the board. This is the half that does, and it exists
//! for one rule:
//!
//! **Nothing is routed while anything is moving.** A line is kept exactly as
//! long as both of its ends are where they were, so a card being dragged trails
//! a plain curve for the length of the gesture and the pass over the lines runs
//! once the hand comes off. The obvious implementation — ask the router on
//! every frame — is also the one that makes dragging a connected card feel like
//! dragging a filing cabinet.
//!
//! The curve is the *ordinary* answer and the elbow is the exception, which is
//! the other half of what makes this affordable: a board whose lines run
//! through open space never touches the search at all. Only a line with a card
//! across it pays for one.
//!
//! Nothing about a path is in the file. Where a line runs is a function of
//! where the cards are now, so this cache can be thrown away at any moment and
//! rebuilt — and is, whenever a board is closed.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use mbrd_core::geometry::{Point as WorldPoint, Rect};
use mbrd_core::model::{Board, ConnMeta, Connection};
use mbrd_core::rope::Rope;
use mbrd_core::route::{self, Line};

/// How many lines get the obstacle test and possibly a search, per settle.
///
/// Bounded because a board may carry two thousand connections: without a
/// ceiling, opening a heavily connected board zoomed all the way out would
/// spend a second deciding shapes before the first frame. The lines that lose
/// are the ones furthest from the middle of the window, which are the ones
/// nobody is looking at, and they get the plain curve — which is what they
/// would have got anyway unless something was across them.
const SETTLE_BUDGET: usize = 256;

/// How far outside the window a line is still worth working out.
///
/// A card's width, so that a bend just off-screen is drawn bending rather than
/// appearing to stop at the edge.
const MARGIN: f32 = 400.0;

/// One line, ready to draw.
///
/// `Clone` for [`Wires::plan`]'s own sake: the steady-state cache hands back
/// a clone of the last answer rather than the `Vec<Wire>` itself, since the
/// caller owns whatever it does with the one it is given.
#[derive(Clone)]
pub struct Wire {
    pub a: String,
    pub b: String,
    pub line: Line,
    pub meta: ConnMeta,
    /// Whether one of its two cards is selected, or the line itself is. A line
    /// to something selected is drawn brighter, which on a busy board is the
    /// only way to see what a card is joined to.
    pub lit: bool,
    /// The shape being replaced, and how far the new one has come in.
    ///
    /// `Some((was, 0.4))` means "draw `was` at six tenths and `line` at four".
    /// `None` is the ordinary case and costs nothing.
    pub leaving: Option<(Line, f32)>,
}

impl Wire {
    /// Whether a point is on this line, within `reach` world units.
    pub fn near(&self, p: WorldPoint, reach: f32) -> bool {
        match &self.line {
            Line::Curve(rope) => rope.near(p, reach),
            Line::Around(path) => path.windows(2).any(|w| near_segment(w[0], w[1], p) <= reach),
        }
    }

    /// Where this line's label sits.
    ///
    /// The middle unless somebody has slid it — see
    /// [`ConnMeta::label_at`](mbrd_core::model::ConnMeta::label_at). Measured
    /// along the line's own length, so a label stays the same distance into
    /// the line as the line bends around whatever turns up between its cards.
    pub fn label_spot(&self) -> WorldPoint {
        self.at(self.meta.label_at).0
    }

    /// How far along the line a point is, from `0.0` to `1.0`.
    ///
    /// The inverse of [`Self::at`], and a real one rather than a near one: a
    /// drag reads this from the pointer and writes it straight back into
    /// `label_at`, so any disagreement between the two is a label that jumps
    /// out from under the hand holding it.
    pub fn how_far_along(&self, p: WorldPoint) -> f32 {
        match &self.line {
            // A rope's own parameter, which is what `at` takes. Its samples are
            // evenly spaced in it, so one sample is one step whatever length of
            // curve it happens to cover.
            Line::Curve(rope) => nearest(rope.samples().as_slice(), p, false),
            // A route is walked by length — see `walk` — so this is measured by
            // length too. The wrong one of the two would drop a label dragged
            // over an elbow's short arm half a line from the pointer.
            Line::Around(path) => nearest(path, p, true),
        }
    }

    /// A point along it and the way it is heading there, for an arrowhead.
    ///
    /// `t` runs from the first card to the second, so `1.0` is the arrival and
    /// `0.0` the departure — which is what makes `dir` readable against the
    /// pair in the order the file carries it.
    pub fn at(&self, t: f32) -> (WorldPoint, WorldPoint) {
        match &self.line {
            Line::Curve(rope) => (rope.at(t), rope.heading(t)),
            Line::Around(path) => walk(path, t),
        }
    }
}

/// The fraction of the way along a polyline that lies nearest a point.
///
/// Nearest **on** the line, not nearest vertex. A route has a handful of
/// vertices and nothing at all between them, so answering with one would let a
/// label be dropped only onto the corners the line bends at.
fn nearest(points: &[WorldPoint], p: WorldPoint, by_length: bool) -> f32 {
    let step = |a: WorldPoint, b: WorldPoint| if by_length { span_between(a, b) } else { 1.0 };
    let total: f32 = points.windows(2).map(|w| step(w[0], w[1])).sum();
    if total <= f32::EPSILON {
        return mbrd_core::model::LABEL_MIDDLE;
    }
    let mut run = 0.0;
    let mut best = (f32::MAX, mbrd_core::model::LABEL_MIDDLE);
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let (dx, dy) = (b.x - a.x, b.y - a.y);
        let square = dx * dx + dy * dy;
        // Where along this one segment the point falls, clamped to its ends —
        // the pointer is wherever the hand is, which includes a long way off
        // the line entirely.
        let t = if square <= f32::EPSILON {
            0.0
        } else {
            (((p.x - a.x) * dx + (p.y - a.y) * dy) / square).clamp(0.0, 1.0)
        };
        let foot = WorldPoint { x: a.x + dx * t, y: a.y + dy * t };
        let away = span_between(foot, p);
        if away < best.0 {
            best = (away, (run + step(a, b) * t) / total);
        }
        run += step(a, b);
    }
    best.1
}

/// How far apart two points are.
fn span_between(a: WorldPoint, b: WorldPoint) -> f32 {
    ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt()
}

/// How far a point is from a segment.
fn near_segment(a: WorldPoint, b: WorldPoint, p: WorldPoint) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = dx * dx + dy * dy;
    let t = if len <= f32::EPSILON {
        0.0
    } else {
        (((p.x - a.x) * dx + (p.y - a.y) * dy) / len).clamp(0.0, 1.0)
    };
    let (cx, cy) = (a.x + dx * t, a.y + dy * t);
    ((p.x - cx).powi(2) + (p.y - cy).powi(2)).sqrt()
}

/// A fraction of the way along a polyline, and the direction there.
///
/// By **length**, not by vertex count: an elbow whose two arms are wildly
/// different lengths would otherwise put its label and its arrowhead at the
/// corner rather than at the middle of the run.
fn walk(path: &[WorldPoint], t: f32) -> (WorldPoint, WorldPoint) {
    if path.len() < 2 {
        return (
            *path.first().unwrap_or(&WorldPoint { x: 0.0, y: 0.0 }),
            WorldPoint { x: 1.0, y: 0.0 },
        );
    }
    let lengths: Vec<f32> = path
        .windows(2)
        .map(|w| ((w[1].x - w[0].x).powi(2) + (w[1].y - w[0].y).powi(2)).sqrt())
        .collect();
    let total: f32 = lengths.iter().sum();
    if total <= f32::EPSILON {
        return (path[0], WorldPoint { x: 1.0, y: 0.0 });
    }
    let mut want = (t.clamp(0.0, 1.0)) * total;
    for (n, run) in lengths.iter().enumerate() {
        if want <= *run || n == lengths.len() - 1 {
            let f = if *run <= f32::EPSILON { 0.0 } else { (want / run).clamp(0.0, 1.0) };
            let (a, b) = (path[n], path[n + 1]);
            let (dx, dy) = (b.x - a.x, b.y - a.y);
            let len = (dx * dx + dy * dy).sqrt().max(f32::EPSILON);
            return (
                WorldPoint { x: a.x + dx * f, y: a.y + dy * f },
                WorldPoint { x: dx / len, y: dy / len },
            );
        }
        want -= run;
    }
    (path[0], WorldPoint { x: 1.0, y: 0.0 })
}

/// A line, and the two boxes it was worked out for.
/// How long a line takes to change shape, once the board has settled.
///
/// The one hard cut this module used to have. A line trails a plain curve for
/// the whole of a drag — that is the rule at the top of this file, and it is
/// the right one — and then the router runs and the curve becomes an elbow
/// between two frames. It is the only thing on the board that changes shape
/// without anybody touching it, and at the exact moment the hand comes off,
/// which is when somebody is looking.
///
/// Short, because it is a correction rather than a move: the line is not going
/// anywhere, it is being redrawn, and a slow crossfade would read as two lines.
const RESHAPING: Duration = Duration::from_millis(180);

struct Cached {
    a: Rect,
    b: Rect,
    /// The board this shape was worked out against.
    ///
    /// The two boxes above say where its own cards were, and for a long time
    /// that was the whole of the freshness test — which quietly assumed that
    /// the only thing that can change a line is one of the two things it is
    /// tied to. It is not: drop a card across the middle of a settled line and
    /// nothing about either end has moved, so the line went on being drawn
    /// straight through the new card until somebody nudged one of its ends.
    at: u64,
    line: Line,
    /// The shape this one replaced, and how far the new one has come in,
    /// `0.0..=1.0` — while it is still on screen.
    ///
    /// Only ever the plain curve, because the plain curve is what was actually
    /// being drawn during the gesture. Keeping the *routed* line it had before
    /// the drag would crossfade from a shape nobody has seen since the press.
    ///
    /// The progress is advanced by [`Wires::tick`] from the frame's own `dt`
    /// rather than read off the wall clock at plan time. Reading `Instant`
    /// here used to mean reduced motion's one enormous `dt` — which does not
    /// touch a clock — could not land a reshape in a single frame the way
    /// every other animation on the board does; this was the one straggler
    /// still waiting on real time to actually pass.
    leaving: Option<(Line, f32)>,
}

/// Everything a call to [`Wires::plan`] answered, and everything its answer
/// depended on.
///
/// `plan` used to redo the whole of its work — the item scan, the routing,
/// the sort — on every single frame, whether or not a board had done
/// anything since the last one. Most frames it had not: a card sitting still
/// asks nothing of this file. What is here is the steady state's entire
/// answer, kept so it can be handed back instead of worked out again the
/// moment nothing that could have changed it has.
struct Snapshot {
    /// The board this plan was worked out against. See [`Cached::at`] for why
    /// this is the whole board's revision rather than anything finer.
    revision: u64,
    /// The window the plan chose an order and a set of visible lines for.
    visible: Rect,
    /// One flag per connection, in `board.connections` order, from the same
    /// `lit` the caller passed in that frame.
    ///
    /// Selection and hover are not carried on the board, so they are not
    /// covered by `revision` — this is the one input `plan` takes that a
    /// board revision does not already answer for. Checked by calling `lit`
    /// again rather than by hashing the closure, which cannot be done at all.
    lit: Vec<bool>,
    wires: Vec<Wire>,
}

#[derive(Default)]
pub struct Wires {
    settled: HashMap<(String, String), Cached>,
    /// Whether any line was mid-change last time this ran, so the frame clock
    /// knows to keep asking.
    fading: bool,
    /// Whether the last pass left routing work for a later frame because
    /// [`SETTLE_BUDGET`] ran out with more of the board still waiting its
    /// turn.
    ///
    /// Tracked apart from `fading`: a line waiting on the budget has nothing
    /// to crossfade yet, but the plan is not finished either, and a frame
    /// where nothing else changed still has to keep going until it is —
    /// otherwise the budget would let a board arriving at a bend simply stop
    /// partway there.
    catching_up: bool,
    /// The last full answer, and what it was worked out against — see
    /// [`Snapshot`]. `None` whenever the last pass could not vouch for its own
    /// staleness test, which is any pass made while `catching_up` or `fading`
    /// was left true: a cached answer nobody will admit is stale is worse
    /// than no cache at all.
    cache: Option<Snapshot>,
}

/// Two boxes are the same box if nothing about them moved a thousandth.
fn same(a: &Rect, b: &Rect) -> bool {
    (a.x0 - b.x0).abs() < 0.001
        && (a.y0 - b.y0).abs() < 0.001
        && (a.x1 - b.x1).abs() < 0.001
        && (a.y1 - b.y1).abs() < 0.001
}

impl Wires {
    /// Throw the cache away — for a board being closed, where every id in here
    /// is about to mean something else.
    pub fn forget(&mut self) {
        self.settled.clear();
        self.cache = None;
        self.fading = false;
        self.catching_up = false;
    }

    /// How many lines are remembered. For the tests, which assert on when the
    /// router was asked and when it was not.
    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.settled.len()
    }

    /// Whether any line is still changing shape.
    pub fn fading(&self) -> bool {
        self.fading
    }

    /// Bring every crossfade in progress `dt` seconds nearer done.
    ///
    /// Called from `BoardView::advance` with the frame's own `dt` — see
    /// [`Cached::leaving`] for the bug this replaced: reading `Instant` at
    /// plan time meant reduced motion's one enormous `dt`, which never
    /// touches a clock, could not land a reshape in a single frame the way
    /// every other animation on the board does. Guarded on `self.fading` so
    /// the ordinary frame, where nothing is crossfading, costs one comparison
    /// rather than a walk of every settled line.
    pub fn tick(&mut self, dt: f32) {
        if !self.fading || dt <= 0.0 {
            return;
        }
        let step = dt / RESHAPING.as_secs_f32();
        for cached in self.settled.values_mut() {
            if let Some((_, t)) = &mut cached.leaving {
                *t = (*t + step).min(1.0);
            }
        }
    }

    /// What to draw this frame.
    ///
    /// `settled` is false while a gesture is in flight, and is the whole of the
    /// performance rule: with it false, nothing is asked of the router and
    /// every line whose ends have moved falls back to the plain curve.
    ///
    /// `obstacles` is asked about the neighbourhood of one line at a time
    /// rather than handed the board, so the cost of a line is the cost of what
    /// is near it — the spatial index is on the other side of that closure.
    ///
    /// `revision` is what tells a settled line that the board it was worked
    /// out against is not the board any more. It is deliberately the whole
    /// board's revision rather than anything finer: a line does not know which
    /// cards it would have to watch, since the set of cards that could get in
    /// its way is exactly what the router works out. Asking again is cheap —
    /// `route::line` tests the plain curve first and only searches when
    /// something is genuinely across it — and it is bounded by
    /// [`SETTLE_BUDGET`] either way.
    pub fn plan(
        &mut self,
        board: &Board,
        revision: u64,
        visible: Rect,
        settled: bool,
        lit: impl Fn(&Connection) -> bool,
        obstacles: impl Fn(Rect) -> Vec<Rect>,
    ) -> Vec<Wire> {
        if board.connections.is_empty() {
            self.settled.clear();
            self.cache = None;
            return Vec::new();
        }

        // Steady state: the board has not moved, the window has not moved,
        // nothing selected or hovered has changed, and there is no unfinished
        // business — a route still waiting on the budget, or a shape still
        // crossfading — that has to keep running to make progress on its own.
        // Everything below this line is what cost nine hundred microseconds a
        // frame on a full board of lines that had not changed since the frame
        // before: the item scan, the hashing, the sort, the routing. None of
        // it has anything new to say, so it is not run — the last answer is
        // cloned instead.
        //
        // Cheap checks first, so a board that is genuinely moving — where
        // this can never pay off — does not pay for the `lit` walk below to
        // find that out.
        if settled && !self.fading && !self.catching_up {
            if let Some(cache) = &self.cache {
                let still = cache.revision == revision
                    && same(&cache.visible, &visible)
                    && board.connections.len() == cache.lit.len()
                    && board
                        .connections
                        .iter()
                        .zip(cache.lit.iter())
                        .all(|(c, &was)| lit(c) == was);
                if still {
                    return cache.wires.clone();
                }
            }
        }

        // Every card a line names, and where it is. One walk, because a `find`
        // per end over twenty thousand items would be by far the expensive part
        // of this function.
        //
        // **Only the cards a line names.** The obvious spelling files all
        // twenty thousand items and then looks up the handful that are joined
        // to something, which costs a string hash and a map insert per item on
        // every frame of every drag — measured at nine hundred microseconds on
        // a full board, for an answer about a few dozen cards. Asking first
        // which ids are wanted turns the insert into a lookup and leaves the
        // map the size of the question.
        let wanted: HashSet<&str> =
            board.connections.iter().flat_map(|c| [c.a.as_str(), c.b.as_str()]).collect();
        let mut where_is: HashMap<&str, Rect> = HashMap::with_capacity(wanted.len());
        for item in &board.items {
            if wanted.contains(item.id.as_str()) {
                where_is.insert(item.id.as_str(), Rect::of_item(item));
            }
        }

        // A line whose card has been deleted is simply not drawn, and is
        // deliberately not removed — that is what lets delete, undo, bin and
        // restore work with no bookkeeping at any of them.
        let live: Vec<(&Connection, Rect, Rect)> = board
            .connections
            .iter()
            .filter_map(|c| Some((c, *where_is.get(c.a.as_str())?, *where_is.get(c.b.as_str())?)))
            .collect();

        let room = visible.inflate(MARGIN);
        let middle = visible.centre();
        // Nearest the middle of the window first, so that if the budget runs
        // out it runs out at the edges. The distance is worked out once per
        // line here rather than inside the comparator below — a comparator
        // recomputes it on every pair it is asked about, which for a sort is
        // several times per line rather than once.
        let mut order: Vec<(f32, usize)> = (0..live.len())
            .filter(|&n| span(&live[n].1, &live[n].2).intersects(&room))
            .map(|n| {
                let c = span(&live[n].1, &live[n].2).centre();
                ((c.x - middle.x).powi(2) + (c.y - middle.y).powi(2), n)
            })
            .collect();
        order.sort_by(|p, q| p.0.total_cmp(&q.0));

        let mut spent = 0;
        let mut fading = false;
        let mut out = Vec::with_capacity(order.len());
        for (_, n) in order {
            let (conn, a, b) = &live[n];
            let key = (conn.a.clone(), conn.b.clone());
            let (line, leaving) = match self.settled.get_mut(&key) {
                Some(had) if same(&had.a, a) && same(&had.b, b) && had.at == revision => {
                    // Still the shape it was. The only thing that can have
                    // changed since last frame is how far through its change
                    // it is — advanced by `Wires::tick`, not by reading the
                    // clock here — and reaching the end of that is what takes
                    // the old shape off the books.
                    let through = match &had.leaving {
                        Some((_, t)) if *t >= 1.0 => {
                            had.leaving = None;
                            None
                        }
                        Some((_, t)) => Some(*t),
                        None => None,
                    };
                    let leaving =
                        through.and_then(|t| had.leaving.as_ref().map(|(was, _)| (was.clone(), t)));
                    (had.line.clone(), leaving)
                }
                // Its own ends have not moved; the board around it has. Worth
                // asking the router again, but there is no budget for it this
                // frame — so what is on screen stays on screen. A line whose
                // cards are both exactly where they were, snapping straight
                // because something was dropped somewhere else, would be a
                // worse answer than a bend that is one frame out of date.
                Some(had)
                    if same(&had.a, a)
                        && same(&had.b, b)
                        && (!settled || spent >= SETTLE_BUDGET) =>
                {
                    (had.line.clone(), None)
                }
                _ if !settled || spent >= SETTLE_BUDGET => {
                    // Trailing: the plain curve, with nothing asked about what
                    // is in the way. This is the frame budget.
                    (Line::Curve(Rope::auto(*a, *b)), None)
                }
                had => {
                    // A line this cache has *routed* before is one whose ends
                    // have since moved, so what was on screen a frame ago was
                    // the plain curve the trailing arm draws. One it has never
                    // routed is a board being opened — nothing is on screen to
                    // change from, and fading in from a curve nobody has seen
                    // would put a flourish on every board opening.
                    //
                    // Only the routing arm below fills the cache, which is
                    // exactly what makes this the right question to ask.
                    let was = had.map(|c| c.line.clone());
                    let known = was.is_some();
                    spent += 1;
                    let walls: Vec<Rect> = obstacles(span(a, b).inflate(MARGIN))
                        .into_iter()
                        .filter(|r| !same(r, a) && !same(r, b))
                        .collect();
                    let line = route::line(a, b, &walls);
                    // Only a detour is a visible change, and only a detour this
                    // line was not already drawing. A settle that produces the
                    // plain curve produces exactly what was already being
                    // drawn, and crossfading a shape with itself would be two
                    // half-strength lines for a fifth of a second — which is
                    // what every line on the board would now do on every edit,
                    // since an edit is what sends them all back through here.
                    let leaving =
                        (known && was.as_ref() != Some(&line) && matches!(line, Line::Around(_)))
                            .then(|| (Line::Curve(Rope::auto(*a, *b)), 0.0));
                    self.settled.insert(
                        key,
                        Cached {
                            a: *a,
                            b: *b,
                            at: revision,
                            line: line.clone(),
                            leaving: leaving.clone(),
                        },
                    );
                    (line, leaving.map(|(was, _)| (was, 0.0)))
                }
            };
            fading |= leaving.is_some();
            out.push(Wire {
                a: conn.a.clone(),
                b: conn.b.clone(),
                line,
                leaving,
                meta: conn.meta.clone(),
                lit: lit(conn),
            });
        }
        self.fading = fading;
        // Whether the budget ran out with `settled` true — a gesture ending
        // is `!settled`, which is the ordinary trailing curve and never
        // "waiting", so only a genuinely exhausted budget counts. A pass that
        // used every last bit of it and still finished everything is
        // mistaken for one more frame of catching up than it needed; the
        // alternative — checking whether anything was actually deferred — is
        // the same walk over `self.settled` this function just did the work
        // to avoid, so a slightly late all-clear is the cheaper of the two.
        self.catching_up = settled && spent >= SETTLE_BUDGET;

        // Anything whose line was deleted, or whose cards are gone, stops being
        // remembered. Without this the cache is a leak with the same lifetime
        // as the window.
        if self.settled.len() > live.len() * 2 + 8 {
            let keep: std::collections::HashSet<(String, String)> =
                live.iter().map(|(c, _, _)| (c.a.clone(), c.b.clone())).collect();
            self.settled.retain(|k, _| keep.contains(k));
        }

        // Lit lines last, so that what a selected card is joined to is drawn
        // over the lines it is not.
        out.sort_by_key(|w| w.lit);

        // Only a plan that is not itself mid-change is one whose freshness
        // test — see [`Snapshot`] — can be trusted by a future frame. A plan
        // taken while still fading or still catching up is stale by
        // construction the moment it is produced, and caching it would only
        // give a later frame a wrong reason to stop redoing this work.
        self.cache = (settled && !self.fading && !self.catching_up).then(|| Snapshot {
            revision,
            visible,
            lit: board.connections.iter().map(&lit).collect(),
            wires: out.clone(),
        });

        out
    }
}

/// The box that holds both ends of a line.
fn span(a: &Rect, b: &Rect) -> Rect {
    Rect::new(a.x0.min(b.x0), a.y0.min(b.y0), a.x1.max(b.x1), a.y1.max(b.y1))
}

/// How thick a line of this weight is drawn, in screen pixels.
///
/// In **pixels**, not world units, so a fine line is a fine line at every zoom.
/// A weight that thickened as you zoomed in would make the three settings mean
/// nothing but "how far in are you".
pub fn thickness(weight: mbrd_core::model::ConnWeight) -> f32 {
    use mbrd_core::model::ConnWeight::*;
    match weight {
        Fine => 1.0,
        Normal => 2.0,
        Bold => 3.5,
    }
}

/// The dash and the gap for a style, in screen pixels, or `None` for solid.
pub fn dashes(style: mbrd_core::model::ConnStyle) -> Option<(f32, f32)> {
    use mbrd_core::model::ConnStyle::*;
    match style {
        Solid => None,
        Dashed => Some((8.0, 6.0)),
        Dotted => Some((2.0, 5.0)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mbrd_core::model::{Item, ItemType};

    fn board_with(places: &[(f32, f32)], joins: &[(usize, usize)]) -> Board {
        let mut b = Board::default();
        for (n, (x, y)) in places.iter().enumerate() {
            let mut item = Item::new(format!("i{n}"), ItemType::Image);
            item.x = *x;
            item.y = *y;
            item.w = 120.0;
            item.h = 90.0;
            b.items.push(item);
        }
        for (a, c) in joins {
            b.connections.push(Connection {
                a: format!("i{a}"),
                b: format!("i{c}"),
                meta: ConnMeta::default(),
            });
        }
        b
    }

    fn everything() -> Rect {
        Rect::new(-10_000.0, -10_000.0, 10_000.0, 10_000.0)
    }

    /// One pass, at a revision the caller chooses.
    ///
    /// The revision is the test's way of saying "the board changed", which in
    /// the app is `BoardState`'s own counter — a plain `Board` has none, so
    /// every test that mutates one has to bump this by hand.
    fn plan_at(wires: &mut Wires, board: &Board, revision: u64, settled: bool) -> Vec<Wire> {
        wires.plan(board, revision, everything(), settled, |_| false, |_| Vec::new())
    }

    fn plan(wires: &mut Wires, board: &Board, settled: bool) -> Vec<Wire> {
        plan_at(wires, board, 1, settled)
    }

    #[test]
    fn a_clear_board_is_all_curves() {
        let board = board_with(&[(0.0, 0.0), (600.0, 0.0)], &[(0, 1)]);
        let mut wires = Wires::default();
        let out = plan(&mut wires, &board, true);
        assert_eq!(out.len(), 1);
        assert!(matches!(out[0].line, Line::Curve(_)));
    }

    #[test]
    fn nothing_is_asked_of_the_router_while_something_is_moving() {
        let board = board_with(&[(0.0, 0.0), (600.0, 0.0)], &[(0, 1)]);
        let mut wires = Wires::default();
        let asked = std::cell::Cell::new(0);
        let out = wires.plan(
            &board,
            1,
            everything(),
            false,
            |_| false,
            |_| {
                asked.set(asked.get() + 1);
                Vec::new()
            },
        );
        assert_eq!(asked.get(), 0, "a moving board asked the router anyway");
        assert_eq!(out.len(), 1);
        assert_eq!(wires.len(), 0, "and nothing was cached from it");
    }

    #[test]
    fn a_settled_line_is_not_worked_out_again_for_nothing() {
        // The performance rule: a frame on a board where nothing has happened
        // asks the router nothing at all.
        let board = board_with(&[(0.0, 0.0), (600.0, 0.0), (0.0, 400.0)], &[(0, 1)]);
        let mut wires = Wires::default();
        plan(&mut wires, &board, true);
        assert_eq!(wires.len(), 1);

        let asked = std::cell::Cell::new(0);
        wires.plan(
            &board,
            1,
            everything(),
            true,
            |_| false,
            |_| {
                asked.set(asked.get() + 1);
                Vec::new()
            },
        );
        assert_eq!(asked.get(), 0, "a line was worked out again for nothing");
    }

    #[test]
    fn a_card_dropped_across_a_settled_line_makes_it_think_again() {
        // This used to assert the opposite, and the opposite was a bug you
        // could see: put a third card down on top of a line between two
        // others and the line went on running straight through it until one
        // of its own ends was nudged. Neither end had moved, so nothing about
        // the line's own geometry had changed — which is exactly why the
        // freshness test could not be about its own geometry alone.
        let mut board = board_with(&[(0.0, 0.0), (900.0, 0.0), (0.0, 400.0)], &[(0, 1)]);
        let mut wires = Wires::default();
        let out = plan_at(&mut wires, &board, 1, true);
        assert!(matches!(out[0].line, Line::Curve(_)), "nothing is in the way yet");

        // The third card lands on the line. Its own two ends have not moved.
        board.items[2].x = 450.0;
        board.items[2].y = 0.0;
        let wall = Rect::centred(450.0, 0.0, 220.0, 500.0);
        let out = wires.plan(&board, 2, everything(), true, |_| false, |_| vec![wall]);
        assert!(matches!(out[0].line, Line::Around(_)), "it stayed drawn through the card");
    }

    #[test]
    fn a_line_whose_shape_did_not_change_does_not_crossfade_when_the_board_does() {
        // Every line on the board goes back through the router on any edit,
        // so the "has it changed shape" test has to be about the shape and
        // not merely about having been asked — or an edit anywhere would set
        // every line on the board crossfading with itself.
        let board = board_with(&[(0.0, 0.0), (900.0, 0.0)], &[(0, 1)]);
        let wall = Rect::centred(450.0, 0.0, 220.0, 500.0);
        let mut wires = Wires::default();
        wires.plan(&board, 1, everything(), true, |_| false, |_| vec![wall]);
        let out = wires.plan(&board, 2, everything(), true, |_| false, |_| vec![wall]);
        assert!(matches!(out[0].line, Line::Around(_)));
        assert!(out[0].leaving.is_none(), "it crossfaded out of the shape it already was");
        assert!(!wires.fading());
    }

    #[test]
    fn a_card_across_a_line_turns_it_into_an_elbow() {
        let board = board_with(&[(0.0, 0.0), (900.0, 0.0)], &[(0, 1)]);
        let wall = Rect::centred(450.0, 0.0, 220.0, 500.0);
        let mut wires = Wires::default();
        let out = wires.plan(&board, 1, everything(), true, |_| false, |_| vec![wall]);
        assert!(matches!(out[0].line, Line::Around(_)), "a rope was drawn through a card");
    }

    #[test]
    fn a_line_that_becomes_an_elbow_crossfades_out_of_the_curve() {
        // The hard cut this replaced: at the release the router runs and a
        // curve becomes an elbow between two frames. What has to be true is
        // that the frame after the settle still has the curve on it.
        let mut board = board_with(&[(0.0, 0.0), (900.0, 0.0)], &[(0, 1)]);
        let wall = Rect::centred(450.0, 0.0, 220.0, 500.0);
        let mut wires = Wires::default();

        // The board, open and settled. This is what puts the line in the cache
        // — the trailing arm never does — and a board cannot be opened
        // half-way through somebody's drag.
        wires.plan(&board, 1, everything(), true, |_| false, |_| vec![wall]);

        // A drag: the ends move, nothing is routed, and the plain curve trails.
        board.items[1].x = 901.0;
        let out = wires.plan(&board, 2, everything(), false, |_| false, |_| vec![wall]);
        assert!(matches!(out[0].line, Line::Curve(_)), "it routed during a gesture");
        assert!(out[0].leaving.is_none(), "nothing has changed shape yet");

        // The release. Now it routes, and the curve it is replacing is still
        // there to fade out of.
        board.items[1].x = 902.0;
        let out = wires.plan(&board, 3, everything(), true, |_| false, |_| vec![wall]);
        assert!(matches!(out[0].line, Line::Around(_)));
        let Some((was, through)) = &out[0].leaving else {
            panic!("the elbow appeared with nothing to come out of");
        };
        assert!(matches!(was, Line::Curve(_)), "it is coming out of the wrong shape");
        assert_eq!(*through, 0.0, "it should start with none of the new one showing");
        assert!(wires.fading(), "the frame clock was not told to keep going");
    }

    #[test]
    fn reduced_motion_lands_a_reshape_in_a_single_tick() {
        // The bug this replaced: the crossfade used to read `Instant::elapsed`,
        // so reduced motion's one enormous `dt` — which never touches a clock
        // — could not land it instantly the way every other animation on the
        // board does. Ticking once with a `dt` far larger than `RESHAPING` is
        // exactly what `BoardView::advance` does under reduced motion, and it
        // has to be enough on its own, in one call, for the very next `plan`.
        let mut board = board_with(&[(0.0, 0.0), (900.0, 0.0)], &[(0, 1)]);
        let wall = Rect::centred(450.0, 0.0, 220.0, 500.0);
        let mut wires = Wires::default();
        wires.plan(&board, 1, everything(), true, |_| false, |_| vec![wall]);
        board.items[1].x = 901.0;
        wires.plan(&board, 2, everything(), false, |_| false, |_| vec![wall]);
        board.items[1].x = 902.0;
        wires.plan(&board, 3, everything(), true, |_| false, |_| vec![wall]);
        assert!(wires.fading(), "set up wrong: nothing is crossfading");

        wires.tick(10.0);
        let out = wires.plan(&board, 3, everything(), true, |_| false, |_| vec![wall]);
        assert!(out[0].leaving.is_none(), "reduced motion did not land the reshape in one tick");
    }

    #[test]
    fn a_line_opened_already_bent_does_not_crossfade_out_of_nothing() {
        // Opening a board routes every line on the first pass. There is no
        // previous frame for those to have come from, and fading them in from
        // a curve nobody has seen would be a flourish on every board opening.
        let board = board_with(&[(0.0, 0.0), (900.0, 0.0)], &[(0, 1)]);
        let wall = Rect::centred(450.0, 0.0, 220.0, 500.0);
        let mut wires = Wires::default();
        let out = wires.plan(&board, 1, everything(), true, |_| false, |_| vec![wall]);
        assert!(matches!(out[0].line, Line::Around(_)));
        assert!(out[0].leaving.is_none(), "it faded in on a board that just opened");
        assert!(!wires.fading());
    }

    #[test]
    fn a_line_that_settles_back_to_a_curve_does_not_crossfade() {
        // The curve is what was already being drawn, so there is nothing to
        // change from — and crossfading a shape with itself would draw it
        // twice at half strength for a fifth of a second.
        let mut board = board_with(&[(0.0, 0.0), (900.0, 0.0)], &[(0, 1)]);
        let mut wires = Wires::default();
        wires.plan(&board, 1, everything(), true, |_| false, |_| Vec::new());
        board.items[1].x = 901.0;
        wires.plan(&board, 2, everything(), false, |_| false, |_| Vec::new());
        board.items[1].x = 902.0;
        let out = wires.plan(&board, 3, everything(), true, |_| false, |_| Vec::new());
        assert!(matches!(out[0].line, Line::Curve(_)));
        assert!(out[0].leaving.is_none());
    }

    #[test]
    fn a_line_naming_a_card_that_is_gone_is_not_drawn_and_not_removed() {
        let mut board = board_with(&[(0.0, 0.0), (600.0, 0.0)], &[(0, 1)]);
        board.items.remove(1);
        let mut wires = Wires::default();
        assert!(plan(&mut wires, &board, true).is_empty());
        assert_eq!(board.connections.len(), 1, "the line itself is untouched");
    }

    #[test]
    fn a_line_far_off_screen_is_not_worked_out() {
        let board = board_with(&[(9000.0, 9000.0), (9600.0, 9000.0)], &[(0, 1)]);
        let mut wires = Wires::default();
        let window = Rect::new(-800.0, -450.0, 800.0, 450.0);
        assert!(wires.plan(&board, 1, window, true, |_| false, |_| Vec::new()).is_empty());
    }

    #[test]
    fn the_middle_of_an_elbow_is_halfway_along_it_and_not_at_its_corner() {
        // An L with a very short arm: by vertex count the middle would be the
        // corner, which is where a label reads worst.
        let wire = Wire {
            a: "a".into(),
            b: "b".into(),
            line: Line::Around(vec![
                WorldPoint { x: 0.0, y: 0.0 },
                WorldPoint { x: 10.0, y: 0.0 },
                WorldPoint { x: 10.0, y: 990.0 },
            ]),
            meta: ConnMeta::default(),
            lit: false,
            leaving: None,
        };
        let mid = wire.at(0.5).0;
        assert!((mid.x - 10.0).abs() < 0.01, "{mid:?}");
        assert!((mid.y - 490.0).abs() < 1.0, "{mid:?}");
    }

    /// An L with one very short arm, so that "halfway along" and "at the
    /// corner" are different answers and a test can tell which one it got.
    fn elbow() -> Wire {
        Wire {
            a: "a".into(),
            b: "b".into(),
            line: Line::Around(vec![
                WorldPoint { x: 0.0, y: 0.0 },
                WorldPoint { x: 10.0, y: 0.0 },
                WorldPoint { x: 10.0, y: 990.0 },
            ]),
            meta: ConnMeta::default(),
            lit: false,
            leaving: None,
        }
    }

    #[test]
    fn a_label_nobody_has_moved_sits_where_the_middle_is() {
        let wire = elbow();
        assert_eq!(wire.label_spot(), wire.at(mbrd_core::model::LABEL_MIDDLE).0);
    }

    #[test]
    fn a_label_slid_to_one_end_sits_on_that_end() {
        let mut wire = elbow();
        wire.meta.label_at = 0.0;
        let at = wire.label_spot();
        assert!(at.x.abs() < 0.01 && at.y.abs() < 0.01, "{at:?}");

        wire.meta.label_at = 1.0;
        let at = wire.label_spot();
        assert!((at.x - 10.0).abs() < 0.01 && (at.y - 990.0).abs() < 0.01, "{at:?}");
    }

    #[test]
    fn where_a_label_was_dropped_is_where_it_is_picked_up_from() {
        // The drag reads `how_far_along` and writes it straight back into
        // `label_at`, so the two have to agree within a sample: a label that
        // jumped a little every time it was grabbed would be unusable.
        let mut wire = elbow();
        for want in [0.1, 0.25, 0.5, 0.75, 0.9] {
            wire.meta.label_at = want;
            let back = wire.how_far_along(wire.label_spot());
            assert!((back - want).abs() < 0.05, "{want} came back as {back}");
        }
    }

    #[test]
    fn a_label_dragged_past_either_end_of_a_line_stops_at_it() {
        // The pointer goes wherever the hand goes, including a long way off
        // the line. What comes back is still somewhere on the line.
        let wire = elbow();
        let far = wire.how_far_along(WorldPoint { x: -5000.0, y: -5000.0 });
        assert_eq!(far, 0.0);
        let far = wire.how_far_along(WorldPoint { x: 5000.0, y: 5000.0 });
        assert_eq!(far, 1.0);
    }

    #[test]
    fn a_label_on_a_line_of_no_length_does_not_divide_by_it() {
        let mut wire = elbow();
        wire.line = Line::Around(vec![WorldPoint { x: 7.0, y: 7.0 }; 3]);
        let back = wire.how_far_along(WorldPoint { x: 100.0, y: 0.0 });
        assert_eq!(back, mbrd_core::model::LABEL_MIDDLE);
    }

    #[test]
    fn a_label_measured_along_an_elbow_goes_by_length_and_not_by_corners() {
        // Two thirds of the way along an L whose long arm is ninety-nine
        // hundredths of it is two thirds of the way *up the long arm* — not
        // two vertices in, which on a routed line means at the corner.
        let wire = elbow();
        let at = wire.at(2.0 / 3.0).0;
        assert!((at.x - 10.0).abs() < 0.01, "{at:?}");
        // Six hundred and sixty-six along, ten of which the short arm spent.
        assert!((at.y - 656.67).abs() < 0.5, "{at:?}");
    }

    #[test]
    fn a_point_on_a_line_is_found_and_one_beside_it_is_not() {
        let board = board_with(&[(0.0, 0.0), (600.0, 0.0)], &[(0, 1)]);
        let mut wires = Wires::default();
        let out = plan(&mut wires, &board, true);
        let mid = out[0].label_spot();
        assert!(out[0].near(mid, 4.0));
        assert!(!out[0].near(WorldPoint { x: mid.x, y: mid.y + 400.0 }, 4.0));
    }

    #[test]
    fn a_weight_is_the_same_thickness_at_every_zoom() {
        use mbrd_core::model::ConnWeight;
        assert!(thickness(ConnWeight::Fine) < thickness(ConnWeight::Normal));
        assert!(thickness(ConnWeight::Normal) < thickness(ConnWeight::Bold));
        assert!(dashes(mbrd_core::model::ConnStyle::Solid).is_none());
    }
}
