//! The arrangement engine: every layout is a pure `(items, opts) -> Vec<Point>`
//! in the same order as `items`.
//!
//! A port of the original's `arrange/arrangements.ts` (its Desktop half) and
//! `arrange/columns.ts`, kept pure the way `align.rs` is: no window, no board,
//! just rectangles in and positions out, so the caller can use the result for a
//! fresh import (positions for brand-new items) or for "Rearrange" (new
//! positions for existing ones) without either path knowing how the layout was
//! computed.
//!
//! `spacing` has exactly one meaning everywhere below: the gap left between two
//! neighbouring cards, edge to edge, in world units. Every layout asks for room
//! the same way — `item + spacing` — so the setting means the same thing
//! whichever arrangement is in force. Where a layout needs a second distance it
//! derives it from this one by a named constant.
//!
//! `seed` is permission to move the slots, not just to fill them differently.
//! Without one every layout below is a pure function of the items it is handed,
//! which is what makes an import reproducible: the same drop lands the same way
//! twice. "Rearrange" wants the opposite — it has to look like something
//! happened — so it passes a fresh seed and each layout answers with a
//! different arrangement of the *same* kind. What varies is chosen per layout
//! so that the layout's own identity survives it: a grid stays square, a spiral
//! stays evenly packed, `date` stays in date order.
//!
//! No layout returns overlapping cards, and every one of them gets that the
//! same way: by never placing a card where it would overlap. The four laid out
//! on structure get it from the structure — see [`lattice`] — and the two that
//! have no structure get it from [`slide_out`], which finds each card the
//! nearest place it actually fits. Nothing here separates cards after the fact.
//!
//! A card is a rectangle, and that is a working assumption rather than a
//! remark. A layout that reasons in radii is reasoning about the circle around
//! the card, which on an ordinary card is three times its area, and the board
//! comes out mostly gap however tight the spacing is set.
//!
//! `Free` is the one exception, and only because the positions it starts from
//! are yours: two cards you deliberately stacked stay stacked.
//!
//! The original has a second catalogue for the Mobile column — orders rather
//! than shapes. This build cut Mobile, so there is no second catalogue here;
//! the file format's `layouts.mobile.arrangement` still round-trips in
//! `schema.rs`, which is the promise that matters.

use std::collections::HashMap;

use serde_json::Value;

use crate::geometry::{Point, Rect};
use crate::model::Item;

/// How far a shaken item can travel under `Free`, as a fraction of its own
/// size. Half a card: far enough that the board visibly loosens, close enough
/// that something you had put beside something else is still beside it.
const FREE_SHAKE: f32 = 0.5;

/// The seam between two clusters, in gaps.
///
/// A multiple of `spacing` rather than a distance of its own, so the whole
/// layout still answers to the one setting — but it has to be a multiple bigger
/// than one, because a block seam the width of the gaps inside a block is not a
/// seam. Three is the smallest that reads as a gutter at every spacing offered.
const BLOCK_GAP: f32 = 3.0;

/// How much of its disc a scatter aims to cover before anything is placed.
///
/// A target rather than a result: cards are thrown at this disc and any that
/// land on one another slide outward until they do not, so what comes out is
/// always looser than what was asked for. Asking for a disc that is already
/// full is what makes the scatter read as a heap rather than as a ring — the
/// crowding in the middle is what pushes the overflow to the edge, which is
/// where a heap's overflow goes.
const SCATTER_FILL: f32 = 1.0;

/// How much wider than tall a block of items should come out.
///
/// Screens are wider than they are tall and so is the room a board has to grow
/// into, so a block of items squared off exactly wastes the width and then runs
/// off the bottom. Masonry gets the gentler figure because its columns already
/// ragged the bottom edge; a page of dated items gets the fuller one because it
/// is read in rows and long rows are what reading wants.
const MASONRY_ASPECT: f32 = 1.4;
const PAGE_ASPECT: f32 = 1.6;

/// How far either side of its own angle a card in the spiral may look for
/// somewhere to sit, and how many directions it tries in there.
///
/// A quarter turn each way: wide enough that a card blocked straight ahead can
/// see round the obstruction, narrow enough that it is still going roughly
/// where the golden angle sent it — which is the whole of what makes this a
/// spiral rather than a heap. Seven tries, fifteen degrees apart; the
/// original measured thirteen and nineteen as no better and sometimes worse.
const SPIRAL_SWEEP: f32 = std::f32::consts::FRAC_PI_2;
const SPIRAL_TRIES: usize = 7;

/// The block a card with no tags goes in under [`Arrangement::Tag`].
///
/// A leading space, which no real tag can carry — the original's `cleanTag()`
/// trims one off everything typed — so this cannot collide with anything, and
/// it sorts before all of them, which puts the untagged block at the left-hand
/// end.
const UNTAGGED: &str = " untagged";

/// Every layout there is, in the order the menu shows them.
///
/// `Spiral` first because it is the default a new board carries in the
/// original, and a menu whose top entry is not what the thing is currently set
/// to reads as a menu you have to go looking through. `Tag` is here even
/// though this build has no tag editor yet: `meta.tags` round-trips, so a
/// board made in the original arrives with its tags on, and clustering by them
/// costs nothing once [`clustered`] exists for `Type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrangement {
    Spiral,
    /// Not "keep positions": the only promise Free still makes is that it will
    /// not impose a shape on you.
    Free,
    Grid,
    Masonry,
    Type,
    Tag,
    Date,
    Scatter,
}

impl Arrangement {
    /// Every value, in menu order. See the note on `ConnColor::ALL` in
    /// `model.rs` — a menu builds itself by mapping over this, and cannot fall
    /// behind what the format defines.
    pub const ALL: [Self; 8] = [
        Self::Spiral,
        Self::Free,
        Self::Grid,
        Self::Masonry,
        Self::Type,
        Self::Tag,
        Self::Date,
        Self::Scatter,
    ];

    /// The id the file format stores.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Spiral => "spiral",
            Self::Free => "free",
            Self::Grid => "grid",
            Self::Masonry => "masonry",
            Self::Type => "type",
            Self::Tag => "tag",
            Self::Date => "date",
            Self::Scatter => "scatter",
        }
    }

    /// What the menu row says.
    pub fn label(self) -> &'static str {
        match self {
            Self::Spiral => "Spiral",
            Self::Free => "Free (no layout)",
            Self::Grid => "Grid rings",
            Self::Masonry => "Masonry",
            Self::Type => "Cluster by type",
            Self::Tag => "Cluster by tag",
            Self::Date => "By date",
            Self::Scatter => "Random scatter",
        }
    }

    /// An unknown name is `None`, and the caller turns that into a fallback
    /// rather than a failure: a `.mbrd` carrying an arrangement a newer build
    /// invented should still rearrange as *something*.
    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.as_str() == s)
    }
}

/// What a caller may ask for. Every field has a default or a meaning when
/// absent.
#[derive(Debug, Clone)]
pub struct Opts {
    /// Where the block is built around.
    pub center: Point,
    /// Edge-to-edge gap, always.
    pub spacing: f32,
    /// Snap lattice cell size, `0.0` for none. When set, the layout reserves
    /// each item whole cells plus one — see [`to_cells`] for why one whole
    /// extra cell where the original manages with a seam.
    pub cell_step: f32,
    /// Makes a layout move its slots; seedless calls stay reproducible.
    pub seed: Option<u32>,
    /// Boxes already on the board that a freshly laid block must not land on.
    pub obstacles: Vec<Rect>,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            center: Point { x: 0.0, y: 0.0 },
            spacing: 12.0,
            cell_step: 0.0,
            seed: None,
            obstacles: Vec::new(),
        }
    }
}

/// As much of an item as a layout reads: a rectangle, and where it currently
/// is. The three fields the ordering layouts sort on — type, name, `mtime` —
/// are read off the borrowed [`Item`] directly. Sizes are copied because
/// [`to_cells`] grows them without touching anybody's board.
struct Card<'a> {
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    it: &'a Item,
}

/// One box already down, in the coordinates the slide is computed in: a centre
/// and half-extents (the item's rectangle plus half a gap).
#[derive(Clone, Copy)]
struct Placed {
    x: f32,
    y: f32,
    hw: f32,
    hh: f32,
}

/// The extent of a laid block, low and high on each axis.
#[derive(Clone, Copy)]
struct Extent {
    x0: f32,
    x1: f32,
    y0: f32,
    y1: f32,
}

/// World y points up, but every layout below is written the way you read a
/// page: successive rows go *down*. So the centre goes in negated and the
/// results come back negated, and each layout gets to stay in the orientation
/// it reads best in. `Free` is exempt — it hands back real world coordinates
/// untouched — but not from the obstacle pass: on a board set to Free with
/// something to avoid, a shaken card must still not land on it.
///
/// One position per item, in input order.
pub fn arrange(items: &[&Item], name: Arrangement, o: &Opts) -> Vec<Point> {
    if items.is_empty() {
        return Vec::new();
    }
    if name == Arrangement::Free {
        let cards: Vec<Card> = items.iter().map(|it| card(it)).collect();
        let loose = free(&cards, o.center, o.spacing, &mut variation(o.seed));
        return if o.obstacles.is_empty() { loose } else { avoid_obstacles(&cards, &loose, o) };
    }
    // When the caller is about to snap the result to a grid (`cell_step` is
    // that grid's cell size), the layout reserves each item whole cells rather
    // than its bare rectangle — see to_cells(). The positions returned are
    // still the real items' — only the room set aside for them grew.
    let cards: Vec<Card> = items
        .iter()
        .map(|it| {
            let mut c = card(it);
            if o.cell_step > 0.0 {
                to_cells(&mut c, o.cell_step);
            }
            c
        })
        .collect();
    let center = Point { x: o.center.x, y: -o.center.y };
    let mut rnd = variation(o.seed);
    let out = match name {
        Arrangement::Free => unreachable!("handled above"),
        Arrangement::Grid => grid(&cards, center, o.spacing, &mut rnd),
        Arrangement::Spiral => spiral(&cards, center, o.spacing, &mut rnd),
        Arrangement::Masonry => masonry(&cards, center, o.spacing, &mut rnd),
        Arrangement::Type => {
            clustered(&cards, center, o.spacing, &mut rnd, |it| it.kind.as_str().to_string())
        }
        Arrangement::Tag => clustered(&cards, center, o.spacing, &mut rnd, first_tag),
        Arrangement::Date => date(&cards, center, o.spacing, &mut rnd),
        Arrangement::Scatter => scatter(&cards, center, o.spacing, o.seed),
    };
    let world: Vec<Point> = out.iter().map(|p| Point { x: p.x, y: -p.y }).collect();
    if o.obstacles.is_empty() {
        world
    } else {
        avoid_obstacles(&cards, &world, o)
    }
}

fn card<'a>(it: &'a Item) -> Card<'a> {
    Card { x: it.x, y: it.y, w: it.w, h: it.h, it }
}

/// An item's box grown to whole grid cells, plus one.
///
/// The original anchors a snapped card's *edge* to the lattice and keeps a seam
/// inside each cell, so whole-cell footprints survive two independent
/// edge-snaps. This build's lattice takes **centres** — `snap::engage` rounds
/// `x` and `y` to the nearest multiple of the step — so two neighbours can each
/// move up to half a step *towards each other* when the board is snapped
/// afterwards. One whole extra cell per footprint is what absorbs that: bodies
/// end up separated by at least a step before the snap, and at worst touching
/// after it. The proof is in `tests::a_snapped_layout_does_not_overlap`.
fn to_cells(c: &mut Card, step: f32) {
    let cells = |v: f32| ((v / step).round().max(1.0)) * step + step;
    c.w = cells(c.w);
    c.h = cells(c.h);
}

/// The room an item wants, as half-extents: its rectangle plus half a gap.
fn room_for(c: &Card, gap: f32) -> (f32, f32) {
    ((c.w + gap) / 2.0, (c.h + gap) / 2.0)
}

// ---------------------------------------------------------------------------
// The layouts
// ---------------------------------------------------------------------------

/// Free imposes no structure, so unseeded it hands back exactly what the items
/// already have.
///
/// A seed is Rearrange asking it to do something anyway, and the only thing
/// "no structure" can honestly do is loosen: every item shaken off its own
/// position, nothing collected anywhere. Free that gathered the board into a
/// disc round a centre would be `Scatter` under a second name, and would throw
/// away the arrangement you made by hand — which is the one thing a layout
/// called Free must not do. This is also the only layout that may hand back
/// overlapping items, and that is not a lapse: the positions it starts from are
/// yours and may already overlap, so refusing to would mean tidying, not
/// shaking.
fn free(cards: &[Card], _center: Point, spacing: f32, rnd: &mut Option<Mulberry>) -> Vec<Point> {
    let Some(rnd) = rnd else {
        return cards.iter().map(|c| Point { x: c.x, y: c.y }).collect();
    };
    cards
        .iter()
        .map(|c| {
            let reach = (c.w.max(c.h) + spacing) * FREE_SHAKE;
            let a = rnd.draw() * std::f32::consts::TAU;
            let r = rnd.draw().sqrt() * reach;
            Point { x: c.x + r * a.cos(), y: c.y + r * a.sin() }
        })
        .collect()
}

/// Square spiral of cells, filling ring by ring outward from centre.
fn grid(cards: &[Card], center: Point, spacing: f32, rnd: &mut Option<Mulberry>) -> Vec<Point> {
    // A quarter turn of the ring pattern. A ring that is completely full maps
    // onto itself under it, so a grid that comes out square comes out square
    // again — what moves is the last, unfinished ring, which is the only part
    // of a grid's outline there is to see change. Nothing else here can vary
    // without the result ceasing to be a grid.
    let turn = match rnd {
        Some(r) => (r.draw() * 4.0).floor() as u32,
        None => 0,
    };
    // Cell (0, 0) is the origin of the lattice, so the first item lands exactly
    // on the point asked for — which for an import is the point you dropped on.
    let cells: Vec<(i32, i32)> = (0..cards.len()).map(|n| spin(ring_cell(n), turn)).collect();
    let (pos, _) = lattice(&cells, cards, spacing);
    pos.iter().map(|p| Point { x: center.x + p.x, y: center.y + p.y }).collect()
}

/// Phyllotaxis: the golden angle, so nothing lines up into visual rows.
///
/// Only the angles come from the phyllotaxis. The radius is asked rather than
/// computed: each card slides out from the centre and stops at the first
/// distance where its rectangle is clear of every rectangle already down.
/// Cards fall into the gaps their neighbours leave, and no two of them can
/// overlap — not because a pass afterwards pulled them apart, but because no
/// card was ever put anywhere it would have to be pulled from.
///
/// And it looks a little either side of its own angle before choosing, which is
/// where most of the tightening comes from: one fixed ray is one degree of
/// freedom, and a card whose ray happens to point down a corridor between two
/// others rides it all the way out, past pockets a few degrees round that it
/// would have dropped straight into.
fn spiral(cards: &[Card], center: Point, spacing: f32, rnd: &mut Option<Mulberry>) -> Vec<Point> {
    let golden = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
    // The whole spiral turned on its centre. Rotation is the one change a
    // phyllotaxis cannot be spoiled by: it deals the same sequence of
    // directions starting somewhere else, so the packing is exactly as good.
    let phase = match rnd {
        Some(r) => r.draw() * std::f32::consts::TAU,
        None => 0.0,
    };
    let mut placed: Vec<Placed> = Vec::with_capacity(cards.len());
    cards
        .iter()
        .enumerate()
        .map(|(n, c)| {
            let (hw, hh) = room_for(c, spacing);
            let mut best = Point { x: 0.0, y: 0.0 };
            let mut near = f32::INFINITY;
            for k in 0..SPIRAL_TRIES {
                let a = n as f32 * golden
                    + phase
                    + SPIRAL_SWEEP * (k as f32 / (SPIRAL_TRIES - 1) as f32 - 0.5);
                let at = slide_out(a.cos(), a.sin(), hw, hh, &placed, 0.0);
                // Compared only against `near`, so the squared magnitude
                // serves and the square root is skipped.
                let r = at.x * at.x + at.y * at.y;
                if r < near {
                    near = r;
                    best = at;
                }
            }
            // The first try always lands: a slide comes to rest at a finite
            // distance whenever the ray is not exactly on an axis, and no
            // angle the golden sequence deals is.
            placed.push(Placed { x: best.x, y: best.y, hw, hh });
            Point { x: center.x + best.x, y: center.y + best.y }
        })
        .collect()
}

/// Columns, each item dropped into the currently shortest one.
fn masonry(cards: &[Card], center: Point, spacing: f32, rnd: &mut Option<Mulberry>) -> Vec<Point> {
    // One column wider or narrower re-flows every item, because which column
    // is shortest changes the moment the first one does. It is the only change
    // masonry can make that is still masonry: the columns are the whole of it,
    // so moving items inside them would read as drift rather than as a layout.
    let mut cols = ((cards.len() as f32 * MASONRY_ASPECT).sqrt().round() as usize).max(1);
    if let Some(r) = rnd {
        cols = reflow(cols, cards.len(), r);
    }
    let cols = cols.min(cards.len());

    // Which column an item lands in depends only on heights, and that half is
    // pack_columns() — shared with nothing else in this build, but kept as the
    // original keeps it: one rule, stated once. No span and no tolerance: a
    // card is one column wide, and two columns level to within a rounding
    // error are two different places.
    let boxes: Vec<ColumnBox> = cards.iter().map(|c| ColumnBox { h: c.h, span: 1 }).collect();
    let pack = pack_columns(&boxes, &PackOpts { cols, gap: spacing, tolerance: 0.0 });

    // The widths are this surface's half: a board of cards has no single
    // column width, so each column comes out exactly as wide as the widest
    // thing that chose it — which is only knowable once everything has chosen.
    let mut widths = vec![0.0_f32; cols];
    for (i, c) in cards.iter().enumerate() {
        let col = pack.spots[i].col;
        widths[col] = widths[col].max(c.w + spacing);
    }
    // The top pack_columns() answers with is a box's leading edge; an item's
    // place on this board is its centre.
    let mut mid = Vec::with_capacity(cols);
    let mut edge = 0.0_f32;
    for w in &widths {
        mid.push(edge + w / 2.0);
        edge += w;
    }
    // Centre the whole block on the target point.
    cards
        .iter()
        .enumerate()
        .map(|(i, c)| Point {
            x: center.x + mid[pack.spots[i].col] - edge / 2.0,
            y: center.y + pack.spots[i].top + c.h / 2.0 - pack.height / 2.0,
        })
        .collect()
}

/// Oldest first, reading order.
fn date(cards: &[Card], center: Point, spacing: f32, rnd: &mut Option<Mulberry>) -> Vec<Point> {
    let order = date_order(cards);
    // Oldest-first is the entire meaning of this layout, so unlike every other
    // one here the items may not be re-dealt — `order` is not the caller's to
    // vary. What can change is the shape of the page they are read on: a wider
    // or narrower block reflows every row while leaving the reading order
    // exactly where it was.
    let mut cols = ((cards.len() as f32 * PAGE_ASPECT).sqrt().ceil() as usize).max(1);
    if let Some(r) = rnd {
        cols = reflow(cols, cards.len(), r);
    }
    let mut cells = vec![(0_i32, 0_i32); cards.len()];
    for (n, &item_index) in order.iter().enumerate() {
        cells[item_index] = ((n % cols) as i32, (n / cols) as i32);
    }
    let (pos, bx) = lattice(&cells, cards, spacing);
    let mx = (bx.x0 + bx.x1) / 2.0;
    let my = (bx.y0 + bx.y1) / 2.0;
    pos.iter().map(|p| Point { x: center.x + p.x - mx, y: center.y + p.y - my }).collect()
}

/// Loose scatter in a disc whose area grows with what is in it.
fn scatter(cards: &[Card], center: Point, spacing: f32, seed: Option<u32>) -> Vec<Point> {
    let area: f32 = cards.iter().map(|c| (c.w + spacing) * (c.h + spacing)).sum();
    let radius = (area / (std::f32::consts::PI * SCATTER_FILL)).sqrt();
    // Seeded, so one scatter is reproducible — but the seed is the caller's to
    // choose. An import wants the default (the same drop lands the same way);
    // "Rearrange" passes a fresh one, because there the whole point is that it
    // comes out different.
    let mut rnd =
        Mulberry::new(seed.unwrap_or_else(|| (cards.len() as u32).wrapping_mul(2_654_435_761)));
    let mut placed: Vec<Placed> = Vec::with_capacity(cards.len());
    cards
        .iter()
        .map(|c| {
            let a = rnd.draw() * std::f32::consts::TAU;
            // sqrt keeps the density even across the disc.
            let r = rnd.draw().sqrt() * radius;
            let (hw, hh) = room_for(c, spacing);
            // Outward from where it fell, never back towards the middle: the
            // drawn point is what makes this a scatter, and moving a card
            // inward would take it somewhere it was not thrown. So a card
            // lands where it was thrown unless somebody is already there, and
            // then it lands just past them.
            let at = slide_out(a.cos(), a.sin(), hw, hh, &placed, r);
            placed.push(Placed { x: at.x, y: at.y, hw, hh });
            Point { x: center.x + at.x, y: center.y + at.y }
        })
        .collect()
}

/// One block per key, blocks laid side by side in a stable order, each block
/// centred on the target line. The body of `Type` and `Tag`, which differ only
/// in what they key on — one asks an item what it is, the other what it was
/// called.
fn clustered(
    cards: &[Card],
    center: Point,
    spacing: f32,
    rnd: &mut Option<Mulberry>,
    key_of: impl Fn(&Item) -> String,
) -> Vec<Point> {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, c) in cards.iter().enumerate() {
        let k = key_of(c.it);
        match groups.iter_mut().find(|(g, _)| *g == k) {
            Some((_, idx)) => idx.push(i),
            None => groups.push((k, vec![i])),
        }
    }
    // Alphabetical, so an unseeded run of the same board deals the blocks the
    // same way twice. Seeded, the blocks change places: the clustering is what
    // these layouts are for and survives untouched, while which cluster you
    // meet first from the left never meant anything and is free to move.
    groups.sort_by(|a, b| a.0.cmp(&b.0));
    if let Some(r) = rnd {
        shuffle(&mut groups, r);
    }

    // Lay every block first, so they can be spaced by the width each one
    // actually came out at rather than by a cell count times a shared cell.
    struct Block {
        idx: Vec<usize>,
        pos: Vec<Point>,
        bx: Extent,
    }
    let laid: Vec<Block> = groups
        .into_iter()
        .map(|(_, idx)| {
            let sub: Vec<Card> = idx
                .iter()
                .map(|&i| Card {
                    x: cards[i].x,
                    y: cards[i].y,
                    w: cards[i].w,
                    h: cards[i].h,
                    it: cards[i].it,
                })
                .collect();
            // Each block reshapes as well as moving. Needed on its own
            // account: a board of one type is a single block, and shuffling a
            // list of one leaves it exactly where it was — so without this,
            // the commonest board there is would be the one board Rearrange
            // could not rearrange.
            let mut cols = ((sub.len() as f32).sqrt().ceil() as usize).max(1);
            if let Some(r) = rnd {
                cols = reflow(cols, sub.len(), r);
            }
            let cells: Vec<(i32, i32)> =
                (0..sub.len()).map(|n| ((n % cols) as i32, (n / cols) as i32)).collect();
            let (pos, bx) = lattice(&cells, &sub, spacing);
            Block { idx, pos, bx }
        })
        .collect();
    let seam = spacing * BLOCK_GAP;
    let width = |b: &Block| b.bx.x1 - b.bx.x0;
    let total: f32 =
        laid.iter().map(width).sum::<f32>() + seam * (laid.len().saturating_sub(1)) as f32;

    let mut out = vec![Point { x: 0.0, y: 0.0 }; cards.len()];
    let mut cursor = center.x - total / 2.0;
    for b in &laid {
        let mid = (b.bx.y0 + b.bx.y1) / 2.0;
        for (n, &item_index) in b.idx.iter().enumerate() {
            out[item_index] =
                Point { x: cursor + b.pos[n].x - b.bx.x0, y: center.y + b.pos[n].y - mid };
        }
        cursor += width(b) + seam;
    }
    out
}

/// Which block a card belongs in under `Tag`: its alphabetically first tag, or
/// [`UNTAGGED`]. Read straight off `meta` and picked by a linear scan rather
/// than trusted to arrive sorted — this module is pure, takes anything shaped
/// like an item, and a `.mbrd` written by hand is a perfectly ordinary thing
/// to open.
fn first_tag(it: &Item) -> String {
    let Some(Value::Array(raw)) = it.meta.get("tags") else {
        return UNTAGGED.to_string();
    };
    let mut best: Option<&str> = None;
    for t in raw {
        if let Some(s) = t.as_str() {
            if !s.is_empty() && best.is_none_or(|b| s < b) {
                best = Some(s);
            }
        }
    }
    best.map_or_else(|| UNTAGGED.to_string(), str::to_string)
}

/// Item indices, oldest first.
///
/// Undated items go last rather than first. A missing modification time is not
/// a time of zero, and treating it as one put every note and every pasted link
/// ahead of a photograph from 1912 — a "By date" layout whose first row is the
/// things that have no date reads as broken from the first glance.
///
/// Equal times fall through to the name, naturally, so that a burst of frames
/// written in the same second comes out 2, 3, 10 rather than 10, 2, 3; and
/// equal names fall through to the order they arrived in, which is stable and
/// is the order the caller chose.
fn date_order(cards: &[Card]) -> Vec<usize> {
    let when = |i: usize| cards[i].it.meta.get("mtime").and_then(Value::as_f64).unwrap_or(0.0);
    let mut order: Vec<usize> = (0..cards.len()).collect();
    order.sort_by(|&a, &b| {
        let (ta, tb) = (when(a), when(b));
        if (ta == 0.0) != (tb == 0.0) {
            return if ta != 0.0 { std::cmp::Ordering::Less } else { std::cmp::Ordering::Greater };
        }
        ta.partial_cmp(&tb)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| natural(&cards[a].it.name, &cards[b].it.name))
            .then(a.cmp(&b))
    });
    order
}

/// Numeric-aware name order, so `frame2` sorts before `frame10`.
///
/// The original leans on `localeCompare(..., { numeric: true })`; this is the
/// same idea by hand — runs of digits compare as numbers, everything else as
/// characters — without a locale library the app has no other use for.
fn natural(a: &str, b: &str) -> std::cmp::Ordering {
    let (mut ai, mut bi) = (a.chars().peekable(), b.chars().peekable());
    loop {
        match (ai.peek().copied(), bi.peek().copied()) {
            (None, None) => return std::cmp::Ordering::Equal,
            (None, Some(_)) => return std::cmp::Ordering::Less,
            (Some(_), None) => return std::cmp::Ordering::Greater,
            (Some(ca), Some(cb)) => {
                if ca.is_ascii_digit() && cb.is_ascii_digit() {
                    let mut na = 0_u128;
                    while let Some(c) = ai.peek().copied().filter(char::is_ascii_digit) {
                        na = na.saturating_mul(10).saturating_add(c as u128 - '0' as u128);
                        ai.next();
                    }
                    let mut nb = 0_u128;
                    while let Some(c) = bi.peek().copied().filter(char::is_ascii_digit) {
                        nb = nb.saturating_mul(10).saturating_add(c as u128 - '0' as u128);
                        bi.next();
                    }
                    match na.cmp(&nb) {
                        std::cmp::Ordering::Equal => {}
                        other => return other,
                    }
                } else {
                    match ca.cmp(&cb) {
                        std::cmp::Ordering::Equal => {
                            ai.next();
                            bi.next();
                        }
                        other => return other,
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Room
// ---------------------------------------------------------------------------

/// Positions for items dealt onto integer cells, on a lattice whose columns and
/// rows are each only as wide and as tall as what actually landed in them.
///
/// `cells` is one `(col, row)` per item, parallel to `cards`. Cell (0, 0) is
/// centred on the origin; the block's own extent comes back so a caller that
/// would rather centre the whole thing can.
///
/// The uniform alternative — one cell for the board, sized from the largest
/// item on it — is what this replaces, and its failure is any board with one
/// big photograph on it: every note is given a photograph's worth of room, and
/// what you get back is a board that is mostly gap. Per column and per row,
/// that photograph widens the one column and heightens the one row it is in,
/// and nothing else on the board moves at all.
///
/// Non-overlap comes free and exactly: two items in the same column are in
/// different rows and each sits inside its own row's height, and the other way
/// about. That is why the four layouts built on this need no separation pass.
fn lattice(cells: &[(i32, i32)], cards: &[Card], gap: f32) -> (Vec<Point>, Extent) {
    let mut col_w: HashMap<i32, f32> = HashMap::new();
    let mut row_h: HashMap<i32, f32> = HashMap::new();
    let (mut c0, mut c1, mut r0, mut r1) = (0_i32, 0_i32, 0_i32, 0_i32);
    for (i, &(c, r)) in cells.iter().enumerate() {
        let w = col_w.entry(c).or_insert(0.0);
        *w = w.max(cards[i].w + gap);
        let h = row_h.entry(r).or_insert(0.0);
        *h = h.max(cards[i].h + gap);
        if c < c0 {
            c0 = c;
        } else if c > c1 {
            c1 = c;
        }
        if r < r0 {
            r0 = r;
        } else if r > r1 {
            r1 = r;
        }
    }
    let (col_x, xs) = track(&col_w, c0, c1);
    let (row_y, ys) = track(&row_h, r0, r1);
    let pos = cells
        .iter()
        // Every column and row between the two extremes got a centre from
        // track(), and no cell is outside them — they were measured off these
        // same cells.
        .map(|(c, r)| Point { x: col_x[c], y: row_y[r] })
        .collect();
    (pos, Extent { x0: xs.0, x1: xs.1, y0: ys.0, y1: ys.1 })
}

/// Cumulative track positions for one axis: the centre of every track and the
/// two outer edges.
///
/// Walked outward from track 0 in both directions rather than accumulated from
/// the low end, because track 0 has to straddle the origin whatever is on
/// either side of it — that is what lets `Grid` promise the first item the
/// exact point it was given while the ring around it sizes itself freely.
fn track(span: &HashMap<i32, f32>, lo: i32, hi: i32) -> (HashMap<i32, f32>, (f32, f32)) {
    let at = |k: i32| span.get(&k).copied().unwrap_or(0.0);
    let mut mid = HashMap::new();
    let first = at(0);
    let mut edge = -first / 2.0;
    for k in 0..=hi {
        let s = at(k);
        mid.insert(k, edge + s / 2.0);
        edge += s;
    }
    let high = edge;
    edge = -first / 2.0;
    for k in (lo..=-1).rev() {
        let s = at(k);
        edge -= s;
        mid.insert(k, edge + s / 2.0);
    }
    (mid, (edge, high))
}

/// Slide a box out along a ray from the origin and stop at the first distance
/// where it is clear of every box already placed.
///
/// This is the whole of how the two unstructured layouts avoid overlap, and it
/// is exact rather than iterative. A box travelling along the ray is at
/// `(t*dx, t*dy)`, so it clashes with a placed box while both
/// `|t*dx - X| < W` and `|t*dy - Y| < H` hold — each of which is an interval
/// of `t`, and the clash is where the two intervals meet. Every placed box
/// therefore bans one interval of the ray, and the answer is the first point
/// at or after `from` that no interval covers: sort by where they open, walk,
/// and jump to the far end of each one still covering you.
///
/// The intervals are open, so a box may come to rest exactly touching — which
/// is what a spacing of zero is supposed to mean. An axis the ray does not
/// move along (`dx` or `dy` of zero) is a standing yes or no rather than an
/// interval, and a standing no rules that box out of the question entirely.
///
/// Exact, so there is no residue to clean up afterwards and no cap to hit. The
/// cost is a pass over what is already down, per direction tried, per item —
/// quadratic, and recorded rather than fixed for the same reason the original
/// records it: the gesture is deliberate and rare, and the fix (bucket
/// `placed` by grid cell and walk only the cells the ray crosses) changes
/// which boxes the arithmetic below is handed, not the arithmetic.
fn slide_out(dx: f32, dy: f32, hw: f32, hh: f32, placed: &[Placed], from: f32) -> Point {
    let mut bans: Vec<(f32, f32)> = Vec::new();
    for p in placed {
        let span = |d: f32, c: f32, reach: f32| -> Option<(f32, f32)> {
            if d == 0.0 {
                return if c.abs() < reach {
                    Some((f32::NEG_INFINITY, f32::INFINITY))
                } else {
                    None
                };
            }
            let a = (c - reach) / d;
            let b = (c + reach) / d;
            Some(if a < b { (a, b) } else { (b, a) })
        };
        let Some(sx) = span(dx, p.x, hw + p.hw) else { continue };
        let Some(sy) = span(dy, p.y, hh + p.hh) else { continue };
        let lo = sx.0.max(sy.0);
        let hi = sx.1.min(sy.1);
        if hi > lo && hi > from {
            bans.push((lo, hi));
        }
    }
    bans.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut t = from;
    for (lo, hi) in bans {
        if lo > t {
            break; // clear from here on out
        }
        if hi > t {
            t = hi;
        }
    }
    Point { x: dx * t, y: dy * t }
}

// A note on what is deliberately *not* here: a pass that walks each card back
// towards the centre after it lands, axis by axis, until something stops it.
// It is the obvious next idea and the original wrote it, measured it and took
// it out again: packing greedily from the middle outward is not helped by
// pulling each card as far in as it will go — the middle clogs, later cards
// are pushed further out than they would otherwise have been, and every board
// tried came out looser for it.

/// Slide each freshly laid item out past what is already on the board.
///
/// The layout has placed the new items among themselves without overlap; this
/// only has to keep them off the `obstacles` — the items already there. Each
/// is pushed straight out from the centre along the ray it already sits on,
/// stopping at the first distance clear of every obstacle and every newcomer
/// placed before it, exactly as [`slide_out`] packs an unstructured layout.
/// One that was already clear does not move at all — `from` is its current
/// distance and a clear ray returns it untouched — so the block keeps its
/// shape and only what would have collided flows around the things in the way.
fn avoid_obstacles(cards: &[Card], world: &[Point], o: &Opts) -> Vec<Point> {
    let c = o.center;
    let mut placed: Vec<Placed> = o
        .obstacles
        .iter()
        .map(|r| Placed {
            x: r.centre().x - c.x,
            y: r.centre().y - c.y,
            hw: r.width() / 2.0,
            hh: r.height() / 2.0,
        })
        .collect();
    world
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let (hw, hh) = room_for(&cards[i], o.spacing);
            let dx = p.x - c.x;
            let dy = p.y - c.y;
            let from = dx.hypot(dy);
            let (dx, dy) = if from < 1e-6 { (0.0, -1.0) } else { (dx / from, dy / from) };
            let at = slide_out(dx, dy, hw, hh, &placed, from);
            placed.push(Placed { x: at.x, y: at.y, hw, hh });
            Point { x: c.x + at.x, y: c.y + at.y }
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Columns
// ---------------------------------------------------------------------------

/// One box to place: its height, and how many columns it wants.
#[derive(Debug, Clone, Copy)]
pub struct ColumnBox {
    pub h: f32,
    /// Clamped rather than trusted, and `1` for every card on a board — the
    /// original keeps spanning for its Feed wall, and the parameter is kept
    /// here so the two packs stay one rule.
    pub span: usize,
}

/// Where one box landed: its first column, and the line it starts on.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColumnSpot {
    pub col: usize,
    pub top: f32,
}

pub struct PackOpts {
    /// How many columns there are. Anything under one is treated as one.
    pub cols: usize,
    /// The space left below each box before the next one in that column.
    pub gap: f32,
    /// How much shorter a column must be to be preferred over one further
    /// left. Zero is a strict win and is the board's masonry.
    pub tolerance: f32,
}

pub struct ColumnPack {
    /// One spot per box, in the order the boxes were given.
    pub spots: Vec<ColumnSpot>,
    /// Where each column has reached, including the gap below the last box.
    pub heights: Vec<f32>,
    /// The tallest column with that trailing gap taken back off.
    pub height: f32,
}

/// Shortest-column-first packing.
///
/// Drop each box into the column that has the least in it so far, leftmost
/// when two are level, and fill that column down by the box's height plus a
/// gap. A box wanting more than one column takes a *run* of adjacent ones and
/// starts below every column in that run, since it has to clear all of them;
/// the run chosen is the one whose tallest column is lowest. Every column the
/// box covers is then filled to the same line, or the next box would tuck
/// under a wide one and overlap it.
///
/// Pure, and it holds no state between calls: the same boxes in the same order
/// pack the same way every time.
pub fn pack_columns(boxes: &[ColumnBox], opts: &PackOpts) -> ColumnPack {
    let cols = opts.cols.max(1);
    let gap = opts.gap;
    let tolerance = opts.tolerance;

    let mut heights = vec![0.0_f32; cols];
    let mut spots = Vec::with_capacity(boxes.len());

    for bx in boxes {
        let span = bx.span.clamp(1, cols);
        // The best run of `span` adjacent columns: the one whose tallest
        // column is lowest. Ties go to the leftmost, which is what
        // `top < best - tolerance` says — a column further right has to
        // actually win, not merely match.
        let mut col = 0;
        let mut best = f32::INFINITY;
        for i in 0..=(cols - span) {
            let top = heights[i..i + span].iter().fold(f32::MIN, |a, &b| a.max(b));
            if top < best - tolerance {
                best = top;
                col = i;
            }
        }
        spots.push(ColumnSpot { col, top: best });
        for h in heights.iter_mut().skip(col).take(span) {
            *h = best + bx.h + gap;
        }
    }

    let height = (heights.iter().fold(0.0_f32, |a, &b| a.max(b)) - gap).max(0.0);
    ColumnPack { spots, heights, height }
}

// ---------------------------------------------------------------------------
// Variation
// ---------------------------------------------------------------------------

/// Small deterministic PRNG, so a scatter re-run looks the same. The same
/// mulberry32 the original uses, bit for bit, so a seed means the same board
/// there and here.
pub struct Mulberry(u32);

impl Mulberry {
    pub fn new(seed: u32) -> Self {
        Self(seed)
    }

    /// The next number in `[0, 1)`.
    pub fn draw(&mut self) -> f32 {
        self.0 = self.0.wrapping_add(0x6D2B_79F5);
        let mut t = (self.0 ^ (self.0 >> 15)).wrapping_mul(1 | self.0);
        t = t.wrapping_add((t ^ (t >> 7)).wrapping_mul(61 | t)) ^ t;
        (f64::from(t ^ (t >> 14)) / 4_294_967_296.0) as f32
    }
}

/// A layout's licence to come out differently, or `None` for the canonical
/// one — so "was I given a seed" is a question each layout answers once and
/// cheaply, and adding variation to a layout can never accidentally cost an
/// unseeded caller its reproducibility.
fn variation(seed: Option<u32>) -> Option<Mulberry> {
    seed.map(Mulberry::new)
}

/// Fisher–Yates over anything, driven by the seeded generator. Public because
/// "Rearrange" also re-deals which item lands in which slot, and the app doing
/// that with a second shuffle implementation is how the two would drift.
pub fn shuffle<T>(v: &mut [T], rnd: &mut Mulberry) {
    for i in (1..v.len()).rev() {
        let j = (rnd.draw() * (i + 1) as f32).floor() as usize;
        v.swap(i, j.min(i));
    }
}

/// How far a column count may move off its natural value. Never onto it: the
/// natural count is what an unseeded run gives, so landing back there would be
/// a rearrangement that did nothing. Two either way rather than one, because a
/// single step leaves only two possible boards and a Rearrange that alternates
/// between two layouts is a toggle rather than a shuffle.
const COL_STEPS: [i32; 4] = [-2, -1, 1, 2];

/// A column count moved off its natural value, kept inside `1..=n`.
///
/// "Never onto it" is the promise above and the clamps can break it: with one
/// item every entry in [`COL_STEPS`] clamps straight back, and there is no
/// other count available — so the honest answer is the natural one, and the
/// callers already treat an unchanged count as an unchanged layout.
fn reflow(cols: usize, n: usize, rnd: &mut Mulberry) -> usize {
    if n <= 1 {
        return cols;
    }
    let step = COL_STEPS[((rnd.draw() * COL_STEPS.len() as f32).floor() as usize).min(3)];
    let moved = (cols as i32 + step).clamp(1, n as i32) as usize;
    // A clamp that landed back on the natural count is not a move. Step the
    // other way instead, which is always available once n > 1.
    if moved != cols {
        return moved;
    }
    if cols > 1 {
        cols - 1
    } else {
        (cols + 1).min(n)
    }
}

/// One ring cell, turned a quarter at a time about the centre.
fn spin((col, row): (i32, i32), turn: u32) -> (i32, i32) {
    match turn & 3 {
        1 => (-row, col),
        2 => (-col, -row),
        3 => (row, -col),
        _ => (col, row),
    }
}

/// The nth cell of a square spiral out from the origin: 0 -> (0,0), then
/// right, up, left, down in growing rings. Gives "outward from the centre"
/// without any sorting.
fn ring_cell(n: usize) -> (i32, i32) {
    if n == 0 {
        return (0, 0);
    }
    let ring = (((n as f64 + 1.0).sqrt() - 1.0) / 2.0).ceil() as i32;
    let side = 2 * ring;
    let prev = (side - 1) * (side - 1); // cells enclosed by the previous ring
    let mut i = n as i32 - prev;
    let per = side; // cells per edge of this ring
    if i < per {
        return (ring, -ring + 1 + i);
    }
    i -= per;
    if i < per {
        return (ring - 1 - i, ring);
    }
    i -= per;
    if i < per {
        return (-ring, ring - 1 - i);
    }
    i -= per;
    (-ring + 1 + i, -ring)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ItemType;
    use serde_json::json;

    fn item(id: &str, x: f32, y: f32, w: f32, h: f32) -> Item {
        let mut it = Item::new(id, ItemType::Image);
        it.x = x;
        it.y = y;
        it.w = w;
        it.h = h;
        it
    }

    /// Seventeen mixed rectangles, the shape the original's tests deal.
    fn mixed() -> Vec<Item> {
        (0..17)
            .map(|n| {
                item(
                    &format!("m{n}"),
                    (n as f32 * 37.0) % 300.0 - 150.0,
                    (n as f32 * 53.0) % 200.0 - 100.0,
                    120.0 + (n as f32 * 61.0) % 260.0,
                    90.0 + (n as f32 * 43.0) % 180.0,
                )
            })
            .collect()
    }

    fn refs(items: &[Item]) -> Vec<&Item> {
        items.iter().collect()
    }

    fn overlap(a: (&Point, &Item), b: (&Point, &Item)) -> bool {
        let eps = 0.01;
        (a.0.x - b.0.x).abs() < (a.1.w + b.1.w) / 2.0 - eps
            && (a.0.y - b.0.y).abs() < (a.1.h + b.1.h) / 2.0 - eps
    }

    fn assert_clear(items: &[Item], out: &[Point], name: Arrangement) {
        for i in 0..items.len() {
            for j in (i + 1)..items.len() {
                assert!(
                    !overlap((&out[i], &items[i]), (&out[j], &items[j])),
                    "{}: items {i} and {j} overlap",
                    name.as_str(),
                );
            }
        }
    }

    #[test]
    fn every_layout_returns_one_finite_point_per_item_in_order() {
        let items = mixed();
        for name in Arrangement::ALL {
            let out = arrange(&refs(&items), name, &Opts::default());
            assert_eq!(out.len(), 17, "{}", name.as_str());
            for (i, p) in out.iter().enumerate() {
                assert!(p.x.is_finite() && p.y.is_finite(), "{}[{i}]", name.as_str());
            }
        }
    }

    #[test]
    fn an_empty_board_arranges_to_nothing() {
        for name in Arrangement::ALL {
            assert!(arrange(&[], name, &Opts::default()).is_empty());
        }
    }

    #[test]
    fn a_single_item_lands_somewhere_finite_in_every_layout() {
        let items = vec![item("a", 5.0, 5.0, 200.0, 100.0)];
        for name in Arrangement::ALL {
            let out = arrange(&refs(&items), name, &Opts::default());
            assert_eq!(out.len(), 1);
            assert!(out[0].x.is_finite() && out[0].y.is_finite());
        }
    }

    #[test]
    fn free_keeps_every_position_exactly() {
        let items = vec![item("a", 13.0, -7.0, 100.0, 100.0), item("b", -200.0, 55.0, 80.0, 60.0)];
        let out = arrange(&refs(&items), Arrangement::Free, &Opts::default());
        assert_eq!(out[0].x, 13.0);
        assert_eq!(out[0].y, -7.0);
        assert_eq!(out[1].x, -200.0);
        assert_eq!(out[1].y, 55.0);
    }

    #[test]
    fn grid_puts_the_first_item_on_the_centre() {
        let items = mixed();
        let opts = Opts { center: Point { x: 100.0, y: 200.0 }, ..Opts::default() };
        let out = arrange(&refs(&items), Arrangement::Grid, &opts);
        assert!((out[0].x - 100.0).abs() < 0.001);
        assert!((out[0].y - 200.0).abs() < 0.001);
    }

    #[test]
    fn no_layout_stacks_everything_on_one_point() {
        let items = mixed();
        for name in Arrangement::ALL {
            if name == Arrangement::Free {
                continue; // free keeps what it was given
            }
            let out = arrange(&refs(&items), name, &Opts::default());
            let distinct: std::collections::HashSet<String> =
                out.iter().map(|p| format!("{:.1},{:.1}", p.x, p.y)).collect();
            assert!(distinct.len() > 1, "{} stacked everything", name.as_str());
        }
    }

    #[test]
    fn structured_layouts_do_not_overlap() {
        let items = mixed();
        for name in [
            Arrangement::Grid,
            Arrangement::Spiral,
            Arrangement::Masonry,
            Arrangement::Type,
            Arrangement::Tag,
            Arrangement::Date,
            Arrangement::Scatter,
        ] {
            let out = arrange(&refs(&items), name, &Opts::default());
            assert_clear(&items, &out, name);
        }
    }

    /// The adapted cell-reservation maths, held to the promise the original's
    /// seam makes: lay out with `cell_step`, snap every centre and size to the
    /// lattice the way `snap::engage` will, and nothing may overlap.
    #[test]
    fn a_snapped_layout_does_not_overlap() {
        let step = 64.0;
        // Pre-sized to the lattice, as the app sizes cards before laying out.
        let mut items = mixed();
        for it in &mut items {
            it.w = crate::geometry::clamp_size(crate::geometry::snap(it.w, step));
            it.h = crate::geometry::clamp_size(crate::geometry::snap(it.h, step));
        }
        for name in [
            Arrangement::Grid,
            Arrangement::Spiral,
            Arrangement::Masonry,
            Arrangement::Type,
            Arrangement::Date,
            Arrangement::Scatter,
        ] {
            let opts = Opts { cell_step: step, spacing: 8.0, ..Opts::default() };
            let out = arrange(&refs(&items), name, &opts);
            let snapped: Vec<Point> = out
                .iter()
                .map(|p| Point {
                    x: crate::geometry::snap(p.x, step),
                    y: crate::geometry::snap(p.y, step),
                })
                .collect();
            assert_clear(&items, &snapped, name);
        }
    }

    #[test]
    fn obstacles_keep_a_fresh_block_off_what_is_already_there() {
        let items = mixed();
        let obstacles =
            vec![Rect::centred(0.0, 0.0, 400.0, 300.0), Rect::centred(600.0, 100.0, 300.0, 300.0)];
        let ob_items = [item("o1", 0.0, 0.0, 400.0, 300.0), item("o2", 600.0, 100.0, 300.0, 300.0)];
        for name in Arrangement::ALL {
            let opts = Opts { obstacles: obstacles.clone(), ..Opts::default() };
            let out = arrange(&refs(&items), name, &opts);
            for (i, p) in out.iter().enumerate() {
                for (k, ob) in ob_items.iter().enumerate() {
                    let at = Point { x: ob.x, y: ob.y };
                    assert!(
                        !overlap((p, &items[i]), (&at, ob)),
                        "{}: card {i} landed on obstacle {k}",
                        name.as_str(),
                    );
                }
            }
            // The pushed cards must also stay clear of each other.
            if name != Arrangement::Free {
                assert_clear(&items, &out, name);
            }
        }
    }

    #[test]
    fn every_layout_answers_a_seed_with_a_different_arrangement() {
        let items = mixed();
        for name in Arrangement::ALL {
            let plain = arrange(&refs(&items), name, &Opts::default());
            let moved = (1..=5).any(|seed| {
                let opts = Opts { seed: Some(seed), ..Opts::default() };
                let out = arrange(&refs(&items), name, &opts);
                out.iter()
                    .zip(&plain)
                    .any(|(a, b)| (a.x - b.x).abs() > 1.0 || (a.y - b.y).abs() > 1.0)
            });
            assert!(moved, "{} ignored every seed it was given", name.as_str());
        }
    }

    #[test]
    fn the_same_seed_lays_out_the_same_way() {
        let items = mixed();
        for name in Arrangement::ALL {
            let opts = Opts { seed: Some(7), ..Opts::default() };
            let a = arrange(&refs(&items), name, &opts);
            let b = arrange(&refs(&items), name, &opts);
            assert_eq!(
                a.iter().map(|p| (p.x.to_bits(), p.y.to_bits())).collect::<Vec<_>>(),
                b.iter().map(|p| (p.x.to_bits(), p.y.to_bits())).collect::<Vec<_>>(),
                "{}",
                name.as_str(),
            );
        }
    }

    #[test]
    fn unseeded_runs_are_reproducible() {
        let items = mixed();
        for name in Arrangement::ALL {
            let a = arrange(&refs(&items), name, &Opts::default());
            let b = arrange(&refs(&items), name, &Opts::default());
            assert_eq!(
                a.iter().map(|p| (p.x.to_bits(), p.y.to_bits())).collect::<Vec<_>>(),
                b.iter().map(|p| (p.x.to_bits(), p.y.to_bits())).collect::<Vec<_>>(),
                "{}",
                name.as_str(),
            );
        }
    }

    #[test]
    fn a_seeded_free_shakes_items_loose_without_relocating_them() {
        let items = mixed();
        let opts = Opts { seed: Some(3), spacing: 12.0, ..Opts::default() };
        let out = arrange(&refs(&items), Arrangement::Free, &opts);
        let travelled: Vec<f32> =
            out.iter().zip(&items).map(|(p, it)| (p.x - it.x).hypot(p.y - it.y)).collect();
        assert!(travelled.iter().any(|&d| d > 1.0), "a seeded free must actually move something");
        for (i, (d, it)) in travelled.iter().zip(&items).enumerate() {
            let reach = (it.w.max(it.h) + 12.0) * FREE_SHAKE;
            assert!(*d <= reach + 0.001, "item {i} travelled {d}, past the {reach} it may shake");
        }
    }

    #[test]
    fn a_seeded_date_layout_is_still_oldest_first() {
        let mut items: Vec<Item> =
            (0..8).map(|n| item(&format!("d{n}"), 0.0, 0.0, 100.0, 100.0)).collect();
        for (n, it) in items.iter_mut().enumerate() {
            it.meta.insert("mtime".into(), json!(1000 + n as i64 * 100));
        }
        for seed in 1..=4_u32 {
            let opts = Opts { seed: Some(seed), ..Opts::default() };
            let out = arrange(&refs(&items), Arrangement::Date, &opts);
            // World y points up, and the layout reads downward: the oldest may
            // not sit *below* the newest.
            assert!(out[0].y >= out[7].y, "seed {seed} put the oldest below the newest",);
        }
    }

    #[test]
    fn date_order_puts_undated_items_last_and_counts_naturally() {
        let mut a = item("a", 0.0, 0.0, 100.0, 100.0);
        a.name = "frame10".into();
        a.meta.insert("mtime".into(), json!(500));
        let mut b = item("b", 0.0, 0.0, 100.0, 100.0);
        b.name = "frame2".into();
        b.meta.insert("mtime".into(), json!(500));
        let undated = item("c", 0.0, 0.0, 100.0, 100.0);
        let items = [a, undated, b];
        let cards: Vec<Card> = items.iter().map(|it| card(it)).collect();
        let order = date_order(&cards);
        // frame2 before frame10 (numeric), the undated card last.
        assert_eq!(order, vec![2, 0, 1]);
    }

    #[test]
    fn cluster_by_type_groups_the_same_types_together() {
        let mut items = Vec::new();
        for n in 0..4 {
            let mut it = item(&format!("i{n}"), 0.0, 0.0, 100.0, 100.0);
            it.kind = if n % 2 == 0 { ItemType::Image } else { ItemType::Note };
            items.push(it);
        }
        let out = arrange(&refs(&items), Arrangement::Type, &Opts::default());
        let spread = |ix: &[usize]| -> f32 {
            let xs: Vec<f32> = ix.iter().map(|&i| out[i].x).collect();
            xs.iter().fold(f32::MIN, |a, &b| a.max(b)) - xs.iter().fold(f32::MAX, |a, &b| a.min(b))
        };
        // The two images sit closer together than the whole row does.
        assert!(spread(&[0, 2]) < spread(&[0, 1, 2, 3]));
    }

    #[test]
    fn cluster_by_tag_reads_the_alphabetically_first_tag() {
        let mut a = item("a", 0.0, 0.0, 100.0, 100.0);
        a.meta.insert("tags".into(), json!(["kitchen", "blue"]));
        let mut b = item("b", 0.0, 0.0, 100.0, 100.0);
        b.meta.insert("tags".into(), json!(["blue"]));
        let untagged = item("c", 0.0, 0.0, 100.0, 100.0);
        assert_eq!(first_tag(&a), "blue");
        assert_eq!(first_tag(&b), "blue");
        assert_eq!(first_tag(&untagged), UNTAGGED);
        // And the untagged sentinel sorts before every real tag, which is what
        // puts that block at the left-hand end.
        assert!(UNTAGGED < "blue");
    }

    #[test]
    fn scatter_is_reproducible_from_a_seed_and_different_without_one() {
        let items = mixed();
        let with = |seed: Option<u32>| {
            let opts = Opts { seed, ..Opts::default() };
            arrange(&refs(&items), Arrangement::Scatter, &opts)
        };
        let a = with(Some(11));
        let b = with(Some(11));
        let c = with(Some(12));
        assert_eq!(
            a.iter().map(|p| (p.x.to_bits(), p.y.to_bits())).collect::<Vec<_>>(),
            b.iter().map(|p| (p.x.to_bits(), p.y.to_bits())).collect::<Vec<_>>(),
        );
        assert!(a.iter().zip(&c).any(|(p, q)| p.x != q.x || p.y != q.y));
    }

    #[test]
    fn pack_columns_is_shortest_first_leftmost_on_a_tie() {
        let boxes = [
            ColumnBox { h: 100.0, span: 1 },
            ColumnBox { h: 50.0, span: 1 },
            ColumnBox { h: 10.0, span: 1 },
            ColumnBox { h: 10.0, span: 1 },
        ];
        let pack = pack_columns(&boxes, &PackOpts { cols: 2, gap: 10.0, tolerance: 0.0 });
        // First two open the columns; the third goes under the *shorter* one
        // (column 1, at 60), and the fourth under the new shorter (column 1
        // again at 80, still under column 0's 110).
        assert_eq!(pack.spots[0], ColumnSpot { col: 0, top: 0.0 });
        assert_eq!(pack.spots[1], ColumnSpot { col: 1, top: 0.0 });
        assert_eq!(pack.spots[2], ColumnSpot { col: 1, top: 60.0 });
        assert_eq!(pack.spots[3], ColumnSpot { col: 1, top: 80.0 });
        // The tallest column less the trailing gap.
        assert!((pack.height - 100.0).abs() < 0.001);
    }

    #[test]
    fn pack_columns_spans_start_below_every_column_they_cover() {
        let boxes = [
            ColumnBox { h: 100.0, span: 1 },
            ColumnBox { h: 40.0, span: 2 },
            ColumnBox { h: 10.0, span: 1 },
        ];
        let pack = pack_columns(&boxes, &PackOpts { cols: 2, gap: 0.0, tolerance: 0.0 });
        // The span must clear the 100-tall first column even though column 1
        // is empty, and it fills both columns to the same line.
        assert_eq!(pack.spots[1], ColumnSpot { col: 0, top: 100.0 });
        assert_eq!(pack.spots[2].top, 140.0);
    }

    #[test]
    fn an_unknown_stored_name_parses_to_none_and_known_ones_round_trip() {
        assert_eq!(Arrangement::parse("constructor"), None);
        for a in Arrangement::ALL {
            assert_eq!(Arrangement::parse(a.as_str()), Some(a));
        }
    }

    #[test]
    fn mulberry_matches_the_original_bit_for_bit() {
        // The first three draws of mulberry32(1) in the original's JS,
        // computed there and pinned here, so a seed means the same board in
        // both implementations.
        let mut r = Mulberry::new(1);
        let got: Vec<f32> = (0..3).map(|_| r.draw()).collect();
        let want = [0.627_073_94_f32, 0.002_735_721_2, 0.527_447_04];
        for (g, w) in got.iter().zip(&want) {
            assert!((g - w).abs() < 1e-6, "got {got:?}, want {want:?}");
        }
    }

    #[test]
    fn reflow_never_answers_the_natural_count_once_there_is_room_to_move() {
        let mut r = Mulberry::new(9);
        for _ in 0..50 {
            let cols = 4;
            let moved = reflow(cols, 20, &mut r);
            assert_ne!(moved, cols);
            assert!((1..=20).contains(&moved));
        }
        // One item has nowhere to go, and the honest answer is no move.
        assert_eq!(reflow(1, 1, &mut r), 1);
    }

    #[test]
    fn ring_cells_walk_outward_ring_by_ring() {
        assert_eq!(ring_cell(0), (0, 0));
        // The first ring holds cells 1..=8; every one is within one step.
        for n in 1..=8 {
            let (c, r) = ring_cell(n);
            assert!(c.abs() <= 1 && r.abs() <= 1 && (c, r) != (0, 0), "cell {n} strayed");
        }
        // The second ring holds 9..=24, all at Chebyshev distance 2.
        for n in 9..=24 {
            let (c, r) = ring_cell(n);
            assert_eq!(c.abs().max(r.abs()), 2, "cell {n} is not on ring 2");
        }
    }
}
