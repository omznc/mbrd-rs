//! Which items are near a place, without asking all of them.
//!
//! The board holds up to [`MAX_ITEMS`](crate::model::MAX_ITEMS) things and the
//! window shows a few dozen. Culling and hit-testing both used to walk the
//! whole list, which is fine at fifty cards and is the frame budget at twenty
//! thousand. This is the structure that makes both questions cost what the
//! *answer* costs rather than what the board costs.
//!
//! It is a **uniform grid hash**: the plane is cut into squares of one size,
//! each square remembers which items overlap it, and a query visits only the
//! squares it touches. No tree, no rebalancing, no per-item allocation once
//! it is built. A quadtree would adapt better to a board that is all cards in
//! one corner and empty everywhere else, and would cost a pointer chase per
//! level to buy that; a moodboard is not that shape often enough to pay for it.
//!
//! Three things keep the grid honest, and each is a failure mode it would
//! otherwise have:
//!
//! - **A card much larger than a square** — a fence around a whole cluster —
//!   would be written into hundreds of squares, and every one of those writes
//!   makes every unrelated query slower. Those go in [`Grid::wide`] instead and
//!   are tested on every query, which is cheap precisely because there are few.
//! - **A query much larger than the board** — the whole plane, when zoomed all
//!   the way out — would visit more empty squares than there are items. Past a
//!   ceiling the grid stops pretending and scans, which is exactly the cost of
//!   not having a grid, so it can never be the slower choice.
//! - **A rotated card** reaches past its own width, so items are filed by
//!   [`Rect::of_item`], which accounts for that. The grid narrows the field; it
//!   never decides. A point query still hands back candidates for
//!   [`geometry::hit`] to rule on.
//!
//! ## What it costs
//!
//! Measured by `tests/scale.rs`, on a full board of twenty thousand cards with
//! fifty-five of them in a 1600×900 window:
//!
//! | | before | after |
//! | --- | --- | --- |
//! | a screenful, per frame | 97µs | **2.0µs** |
//! | a press | 97µs | **0.1µs** |
//! | building it | — | 2.6ms |
//!
//! The build cost is the whole of the trade and it is worth stating plainly:
//! this is rebuilt from scratch whenever the board changes, so an edit to a
//! full board pays two and a half milliseconds it did not pay before. That is
//! affordable because of what does *not* change the board — panning, zooming,
//! selecting, and every frame in between — which is the overwhelming majority
//! of frames and the case the index makes forty-eight times cheaper. The day
//! that trade stops holding is the day this grows an `update` that moves one
//! item between squares instead.
//!
//! Nothing in here knows what a pixel is, so all of it is tested without a
//! window.

use std::collections::HashMap;

use crate::geometry::{Point, Rect};
use crate::model::Item;

/// Past this many squares, a query gives up on the grid and scans.
///
/// Sized so that the fallback costs about what a scan of a full board costs:
/// visiting an empty square is much cheaper than testing an item, so this can
/// be well above [`MAX_ITEMS`](crate::model::MAX_ITEMS) and still be the right
/// trade.
const MAX_CELLS_PER_QUERY: i64 = 4_096;

/// How many squares across an item may reach before it is filed as wide.
const WIDE_AT: f32 = 4.0;

/// How many items may have moved before [`Grid::refile`] gives up and the grid
/// is built again.
///
/// Refiling one card is a handful of hash lookups and rebuilding is one per
/// card on the board, so the two cross somewhere in the low thousands. This
/// sits well under that crossing, because the case it is for — a drag — moves
/// what is under the pointer, and the case it is not for — dragging a
/// select-all — is one where the rebuild really is the better answer.
const REFILE_MOST: usize = 2_048;

/// The size of a square when there is nothing to measure one from.
const DEFAULT_CELL: f32 = 512.0;

/// The narrowest and widest a square may be.
///
/// The floor keeps a board of tiny swatches from cutting the plane so fine that
/// a screenful spans more squares than the ceiling above allows. The cap keeps
/// a board of enormous backdrops from putting everything in one square, which
/// is a linear scan wearing a hat.
const MIN_CELL: f32 = 128.0;
const MAX_CELL: f32 = 8_192.0;

/// Where every item on a board is, arranged so that "which are near here" is
/// cheap to answer.
///
/// Indices, not ids: the grid is built from a slice and hands back positions in
/// that same slice, so a caller gets its items back without a lookup. That also
/// means **a grid is only valid for the list it was built from** — see
/// [`crate::state::BoardState::revision`] for how the UI knows when to rebuild.
#[derive(Debug, Clone, Default)]
pub struct Grid {
    cell: f32,
    cells: HashMap<(i32, i32), Vec<u32>>,
    /// Items too large for a square to narrow down. Tested on every query.
    wide: Vec<u32>,
    /// Each item's box, in the order they were given. Parallel to the slice.
    boxes: Vec<Rect>,
    /// The raw `(x, y, w, h, rot)` each entry in `boxes` was filed from — the
    /// fields [`Rect::of_item`] reads. [`Grid::refile`] compares these floats
    /// directly to find what moved, rather than calling `of_item` again for
    /// every item on the board just to throw the answer away when it agrees.
    geom: Vec<(f32, f32, f32, f32, f32)>,
}

/// The fields [`Rect::of_item`] reads, as the plain tuple `refile` compares.
fn geom_of(item: &Item) -> (f32, f32, f32, f32, f32) {
    (item.x, item.y, item.w, item.h, item.rot)
}

impl Grid {
    /// File a list of items. `O(n)` and one pass.
    pub fn build(items: &[Item]) -> Self {
        let boxes: Vec<Rect> = items.iter().map(Rect::of_item).collect();
        let geom: Vec<(f32, f32, f32, f32, f32)> = items.iter().map(geom_of).collect();
        let cell = cell_for(&boxes);

        let mut grid = Self { cell, cells: HashMap::new(), wide: Vec::new(), boxes, geom };
        for i in 0..grid.boxes.len() as u32 {
            grid.file(i);
        }
        grid
    }

    /// Bring a grid level with a list whose items have only *moved*.
    ///
    /// This is the `update` the module header said this would grow the day the
    /// build cost stopped being affordable, and dragging is that day. A gesture
    /// is an edit per frame rather than an edit per gesture, so a board of
    /// twenty thousand cards was paying a millisecond and a half of hashing and
    /// bucket allocation on **every frame** of every drag — to rebuild twenty
    /// thousand entries because two of them were under the pointer. Refiling
    /// costs the cards that actually moved plus one scan of the boxes, and the
    /// scan is floats with no hashing and no allocation.
    ///
    /// Answers `false` when it cannot help, and then the caller rebuilds:
    ///
    /// - the list is a different length, so something was added or removed and
    ///   the indices no longer line up with what is filed;
    /// - more than [`REFILE_MOST`] items moved, past which the rebuild is
    ///   simply the cheaper of the two.
    ///
    /// It never answers `true` having left the grid disagreeing with `items`.
    ///
    /// One thing it deliberately does not redo is [`Grid::cell_size`], which is
    /// chosen from the boxes at build time. A refiled grid keeps the square it
    /// had, which stays *correct* — filing and querying use the same square —
    /// while drifting from ideal as cards are resized. That drift is bounded by
    /// how common a rebuild is: adding or deleting anything is one.
    pub fn refile(&mut self, items: &[Item]) -> bool {
        if items.len() != self.boxes.len() {
            return false;
        }

        // Which boxes are not where they were filed. Compared against the raw
        // geometry `of_item` reads, not `of_item`'s own answer — a rotated
        // item's box costs a `sin`/`cos` to work out, and a board where
        // nothing moved should not pay that for every item just to be told so.
        let mut moved: Vec<(u32, Rect)> = Vec::new();
        for (i, item) in items.iter().enumerate() {
            let g = geom_of(item);
            if g != self.geom[i] {
                if moved.len() == REFILE_MOST {
                    return false;
                }
                // Worked out once, here, and reused below rather than a
                // second time at the point the box is written back.
                moved.push((i as u32, Rect::of_item(item)));
            }
        }

        // Every removal before any insertion, so that an item moving into the
        // square another is moving out of cannot depend on which of the two is
        // handled first.
        for &(i, _) in &moved {
            self.pull(i);
        }
        for (i, rect) in moved {
            self.boxes[i as usize] = rect;
            self.geom[i as usize] = geom_of(&items[i as usize]);
            self.file(i);
        }
        true
    }

    /// Put the item at `index` into the squares its current box reaches.
    ///
    /// The one place that decides which squares a box is in, so that a grid
    /// that was refiled and a grid that was rebuilt cannot answer differently.
    fn file(&mut self, index: u32) {
        let (x0, y0, x1, y1) = self.span(self.boxes[index as usize]);
        if (x1 - x0) as f32 >= WIDE_AT || (y1 - y0) as f32 >= WIDE_AT {
            // Kept ascending, because `gather` merges this list into an already
            // sorted one rather than sorting the pair.
            if let Err(at) = self.wide.binary_search(&index) {
                self.wide.insert(at, index);
            }
            return;
        }
        for cx in x0..=x1 {
            for cy in y0..=y1 {
                self.cells.entry((cx, cy)).or_default().push(index);
            }
        }
    }

    /// Take the item at `index` out of the squares its *filed* box reaches.
    ///
    /// Reads `boxes[index]`, so it has to run before that entry is updated —
    /// which is the whole reason [`Grid::refile`] writes the new box between
    /// this and [`Grid::file`] rather than up front.
    fn pull(&mut self, index: u32) {
        let (x0, y0, x1, y1) = self.span(self.boxes[index as usize]);
        if (x1 - x0) as f32 >= WIDE_AT || (y1 - y0) as f32 >= WIDE_AT {
            if let Ok(at) = self.wide.binary_search(&index) {
                self.wide.remove(at);
            }
            return;
        }
        for cx in x0..=x1 {
            for cy in y0..=y1 {
                if let Some(bucket) = self.cells.get_mut(&(cx, cy)) {
                    // Order inside a square is not load-bearing — `gather`
                    // sorts what it collects, because a card in two squares
                    // would otherwise come back twice regardless.
                    if let Some(at) = bucket.iter().position(|&n| n == index) {
                        bucket.swap_remove(at);
                    }
                }
            }
        }
    }

    /// How many items were filed.
    pub fn len(&self) -> usize {
        self.boxes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.boxes.is_empty()
    }

    /// The size of one square. Chosen from the items; exposed for tests and
    /// for anything that wants to report on the shape of a board.
    pub fn cell_size(&self) -> f32 {
        self.cell
    }

    /// How many items were too large for a square to help with.
    pub fn wide_count(&self) -> usize {
        self.wide.len()
    }

    /// The box of the item at `index`, as it was when the grid was built.
    pub fn box_of(&self, index: u32) -> Rect {
        self.boxes[index as usize]
    }

    /// Every item whose box meets `rect`, by index, ascending and without
    /// repeats.
    ///
    /// **Exact**, not approximate: the grid narrows the field and then the box
    /// test decides, so a caller does not have to repeat the rectangle test. It
    /// is still a *box* test, so a caller that cares about rotation — pressing,
    /// as opposed to culling — has [`geometry::hit`] to finish the job.
    ///
    /// `out` is cleared first, and is a parameter rather than a return value so
    /// that a caller asking this every frame can keep one buffer.
    pub fn in_rect(&self, rect: Rect, out: &mut Vec<u32>) {
        self.gather(rect, out);
        out.retain(|&i| self.boxes[i as usize].intersects(&rect));
    }

    /// Every item whose box holds `p`, by index, ascending.
    ///
    /// The same narrowing as [`Grid::in_rect`], and the same caveat: this is
    /// the box, so the caller still asks [`geometry::hit`] whether the press
    /// actually landed on the card.
    pub fn at(&self, p: Point, out: &mut Vec<u32>) {
        self.gather(Rect { x0: p.x, y0: p.y, x1: p.x, y1: p.y }, out);
        out.retain(|&i| self.boxes[i as usize].contains(p));
    }

    /// The candidates for a rectangle: everything in the squares it touches,
    /// plus everything too wide to be in a square, deduplicated.
    fn gather(&self, rect: Rect, out: &mut Vec<u32>) {
        out.clear();
        if self.boxes.is_empty() {
            return;
        }

        let (x0, y0, x1, y1) = self.span(rect);
        let squares = (x1 as i64 - x0 as i64 + 1) * (y1 as i64 - y0 as i64 + 1);
        if squares <= 0 || squares > MAX_CELLS_PER_QUERY {
            // Either the rectangle is degenerate — inverted, or off in the
            // infinities — or it covers more of the plane than the grid can
            // help with. Both answer the same way: ask everybody.
            out.extend(0..self.boxes.len() as u32);
            return;
        }

        for cx in x0..=x1 {
            for cy in y0..=y1 {
                if let Some(bucket) = self.cells.get(&(cx, cy)) {
                    out.extend_from_slice(bucket);
                }
            }
        }
        // A card spanning two squares is in both, and a query touching both
        // would otherwise hand it back twice.
        out.sort_unstable();
        out.dedup();

        // The wide ones are sorted and disjoint from what the squares hold, so
        // merging them keeps the whole thing ascending without a second sort.
        if !self.wide.is_empty() {
            merge_sorted(out, &self.wide);
        }
    }

    /// The inclusive range of squares a box touches.
    ///
    /// A non-finite edge saturates rather than wrapping, which is what puts a
    /// nonsense rectangle onto the scan path above rather than into a loop of
    /// four billion squares.
    fn span(&self, r: Rect) -> (i32, i32, i32, i32) {
        let q = |v: f32| -> i32 {
            let c = v / self.cell;
            if c.is_nan() {
                0
            } else {
                c.floor().clamp(i32::MIN as f32, i32::MAX as f32) as i32
            }
        };
        (q(r.x0), q(r.y0), q(r.x1), q(r.y1))
    }
}

/// How big a square should be for these boxes.
///
/// Twice the mean of the larger dimension, so that a typical card sits inside
/// one square or straddles two, and a screenful is a couple of dozen squares
/// rather than thousands. Mean rather than median because it is one pass and no
/// allocation, and because the outliers that skew a mean are exactly the ones
/// [`Grid::wide`] takes out of the picture anyway.
fn cell_for(boxes: &[Rect]) -> f32 {
    if boxes.is_empty() {
        return DEFAULT_CELL;
    }
    let mut total = 0.0f64;
    for b in boxes {
        let side = b.width().max(b.height());
        total += if side.is_finite() { side.max(1.0) as f64 } else { 1.0 };
    }
    let mean = (total / boxes.len() as f64) as f32;
    if mean.is_finite() {
        (mean * 2.0).clamp(MIN_CELL, MAX_CELL)
    } else {
        DEFAULT_CELL
    }
}

/// Fold a sorted, disjoint slice into a sorted vector, in place.
fn merge_sorted(out: &mut Vec<u32>, extra: &[u32]) {
    // Two ascending runs with nothing in common, merged from the back rather
    // than appended and sorted — a sort would have to rediscover that they
    // are already two runs, and no allocation beyond the one `reserve` is
    // needed to do the merge in place.
    let old_len = out.len();
    out.reserve(extra.len());
    out.extend_from_slice(extra);
    let mut i = old_len as isize - 1;
    let mut j = extra.len() as isize - 1;
    let mut k = out.len() as isize - 1;
    while j >= 0 {
        if i >= 0 && out[i as usize] > extra[j as usize] {
            out[k as usize] = out[i as usize];
            i -= 1;
        } else {
            out[k as usize] = extra[j as usize];
            j -= 1;
        }
        k -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::{self, point};
    use crate::model::ItemType;

    fn card(id: &str, x: f32, y: f32, w: f32, h: f32) -> Item {
        let mut item = Item::new(id, ItemType::Generic);
        item.x = x;
        item.y = y;
        item.w = w;
        item.h = h;
        item
    }

    /// Everything the grid claims, worked out the slow and obvious way.
    fn by_hand(items: &[Item], rect: Rect) -> Vec<u32> {
        (0..items.len() as u32)
            .filter(|&i| Rect::of_item(&items[i as usize]).intersects(&rect))
            .collect()
    }

    /// A board of cards at pseudo-random places, deterministic across runs.
    fn scattered(count: usize) -> Vec<Item> {
        let mut seed: u64 = 0x2545_F491_4F6C_DD1D;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed >> 11) as f32 / (1u64 << 53) as f32
        };
        (0..count)
            .map(|i| {
                let (x, y) = (next() * 40_000.0 - 20_000.0, next() * 40_000.0 - 20_000.0);
                let (w, h) = (60.0 + next() * 400.0, 60.0 + next() * 400.0);
                card(&format!("i{i}"), x, y, w, h)
            })
            .collect()
    }

    #[test]
    fn an_empty_board_answers_nothing_rather_than_failing() {
        let grid = Grid::build(&[]);
        assert!(grid.is_empty());
        let mut out = Vec::new();
        grid.in_rect(Rect::new(-1e6, -1e6, 1e6, 1e6), &mut out);
        assert!(out.is_empty());
        grid.at(point(0.0, 0.0), &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn every_card_is_found_where_it_actually_is() {
        let items = scattered(400);
        let grid = Grid::build(&items);
        let mut out = Vec::new();
        for (i, item) in items.iter().enumerate() {
            grid.at(point(item.x, item.y), &mut out);
            assert!(out.contains(&(i as u32)), "{} was not at its own centre", item.id);
        }
    }

    #[test]
    fn the_grid_agrees_with_walking_the_whole_list() {
        let items = scattered(1_200);
        let grid = Grid::build(&items);
        let mut out = Vec::new();
        // A spread of window shapes: a sliver, a screenful, and the whole plane.
        for (w, h) in [(40.0, 40.0), (900.0, 600.0), (7_000.0, 4_000.0), (1e6, 1e6)] {
            for step in 0..12 {
                let cx = -18_000.0 + step as f32 * 3_000.0;
                let cy = 9_000.0 - step as f32 * 1_700.0;
                let rect = Rect::centred(cx, cy, w, h);
                grid.in_rect(rect, &mut out);
                assert_eq!(out, by_hand(&items, rect), "at {cx},{cy} over {w}x{h}");
            }
        }
    }

    #[test]
    fn a_card_far_larger_than_a_square_is_still_found_under_the_pointer() {
        let mut items = scattered(300);
        // A fence around the whole board: many squares wide, so it goes in the
        // list the grid tests by hand.
        items.push(card("fence", 0.0, 0.0, 39_000.0, 39_000.0));
        let grid = Grid::build(&items);
        assert_eq!(grid.wide_count(), 1, "the fence should not be in a square");

        let big = items.len() as u32 - 1;
        let mut out = Vec::new();
        for p in [point(0.0, 0.0), point(19_000.0, -19_000.0), point(-4_321.0, 88.0)] {
            grid.at(p, &mut out);
            assert!(out.contains(&big), "the fence was missing at {p:?}");
        }
        // And it does not turn up somewhere it is not.
        grid.at(point(50_000.0, 0.0), &mut out);
        assert!(!out.contains(&big));
    }

    #[test]
    fn a_turned_card_is_offered_at_the_corner_it_reaches() {
        let mut item = card("tilted", 0.0, 0.0, 400.0, 40.0);
        item.rot = 45.0;
        let items = vec![item];
        let grid = Grid::build(&items);

        // Well past the untilted card's own corner, and inside the turned one.
        let far = point(130.0, 130.0);
        let mut out = Vec::new();
        grid.at(far, &mut out);
        assert_eq!(out, vec![0], "the grid did not offer the turned card");
        assert!(geometry::hit(&items[0], far), "and it really is a press");
    }

    #[test]
    fn an_answer_comes_back_in_order_and_says_nothing_twice() {
        let items = scattered(2_000);
        let grid = Grid::build(&items);
        let mut out = Vec::new();
        // Deliberately far wider than one square, so cards straddle the seams.
        grid.in_rect(Rect::centred(0.0, 0.0, 12_000.0, 12_000.0), &mut out);
        assert!(out.len() > 20, "this window should hold a good many");
        assert!(out.windows(2).all(|w| w[0] < w[1]), "out of order or repeated");
    }

    #[test]
    fn asking_for_the_whole_plane_gives_the_whole_board_back() {
        let items = scattered(50);
        let grid = Grid::build(&items);
        let mut out = Vec::new();
        // Far more squares than the ceiling allows, so this takes the scan path.
        grid.in_rect(Rect::new(-1e9, -1e9, 1e9, 1e9), &mut out);
        assert_eq!(out.len(), items.len());
    }

    #[test]
    fn a_nonsense_rectangle_is_answered_rather_than_looped_over() {
        let items = scattered(20);
        let grid = Grid::build(&items);
        let mut out = Vec::new();
        // Inverted, and then not a number at all. Neither may hang.
        grid.in_rect(Rect::new(500.0, 500.0, -500.0, -500.0), &mut out);
        assert!(out.is_empty());
        grid.in_rect(Rect::new(f32::NAN, f32::NAN, f32::NAN, f32::NAN), &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn a_board_of_one_size_of_card_gets_squares_that_suit_it() {
        let small: Vec<Item> =
            (0..200).map(|i| card(&format!("s{i}"), i as f32 * 70.0, 0.0, 60.0, 60.0)).collect();
        assert_eq!(Grid::build(&small).cell_size(), MIN_CELL);

        let large: Vec<Item> = (0..200)
            .map(|i| card(&format!("l{i}"), i as f32 * 9_000.0, 0.0, 8_000.0, 8_000.0))
            .collect();
        assert_eq!(Grid::build(&large).cell_size(), MAX_CELL);
    }

    // -----------------------------------------------------------------------
    // Refiling
    //
    // The rule every one of these is testing is the same rule, and it is worth
    // saying once: **a refiled grid answers exactly what a rebuilt one would.**
    // Anything less and a drag would slowly poison the hit test, which shows up
    // not as a wrong answer but as a click that selects the card that used to
    // be there — the worst kind of bug to go looking for.
    // -----------------------------------------------------------------------

    /// Every question worth asking, asked of a grid and of the slow way.
    fn agrees(grid: &Grid, items: &[Item]) {
        let mut out = Vec::new();
        for rect in [
            Rect::new(-500.0, -500.0, 500.0, 500.0),
            Rect::new(0.0, 0.0, 20_000.0, 20_000.0),
            Rect::new(-20_000.0, -20_000.0, 0.0, 0.0),
            Rect::new(-40_000.0, -40_000.0, 40_000.0, 40_000.0),
        ] {
            grid.in_rect(rect, &mut out);
            assert_eq!(out, by_hand(items, rect), "the grid disagrees about {rect:?}");
        }
        for p in [point(0.0, 0.0), point(1_234.0, -800.0), point(-9_000.0, 400.0)] {
            grid.at(p, &mut out);
            let want: Vec<u32> = (0..items.len() as u32)
                .filter(|&i| Rect::of_item(&items[i as usize]).contains(p))
                .collect();
            assert_eq!(out, want, "the grid disagrees about {p:?}");
        }
    }

    #[test]
    fn a_card_that_moved_is_found_where_it_went_and_not_where_it_was() {
        let mut items = scattered(400);
        let mut grid = Grid::build(&items);

        items[7].x = 12_000.0;
        items[7].y = -3_400.0;
        items[100].x = 0.0;
        items[100].y = 0.0;

        assert!(grid.refile(&items), "two cards moving is refiling's own case");
        agrees(&grid, &items);
    }

    #[test]
    fn a_grid_that_nothing_has_happened_to_is_left_exactly_as_it_was() {
        let items = scattered(200);
        let mut grid = Grid::build(&items);
        assert!(grid.refile(&items));
        agrees(&grid, &items);
    }

    #[test]
    fn a_card_grown_past_a_square_moves_to_the_wide_list_and_back() {
        let mut items = scattered(300);
        let mut grid = Grid::build(&items);
        let was = grid.wide_count();

        // Far larger than `WIDE_AT` squares across, whatever the square is.
        items[3].w = grid.cell_size() * 40.0;
        items[3].h = grid.cell_size() * 40.0;
        assert!(grid.refile(&items));
        assert_eq!(grid.wide_count(), was + 1, "a fence-sized card should be wide");
        agrees(&grid, &items);

        // And back down again, without leaving itself behind in either place.
        items[3].w = 80.0;
        items[3].h = 80.0;
        assert!(grid.refile(&items));
        assert_eq!(grid.wide_count(), was, "it stayed in the wide list");
        agrees(&grid, &items);
    }

    #[test]
    fn a_rotated_card_is_refiled_by_what_it_reaches_rather_than_by_its_width() {
        let mut items = scattered(120);
        let mut grid = Grid::build(&items);
        items[9].rot = 45.0;
        assert!(grid.refile(&items));
        agrees(&grid, &items);
        // The box the grid holds is the reaching one, which is what
        // `Rect::of_item` is for and what a plain w/h would have got wrong.
        assert_eq!(grid.box_of(9), Rect::of_item(&items[9]));
    }

    #[test]
    fn a_card_added_or_taken_away_is_refused_rather_than_answered_wrongly() {
        let items = scattered(50);
        let mut grid = Grid::build(&items);

        let mut more = items.clone();
        more.push(card("new", 0.0, 0.0, 100.0, 100.0));
        assert!(!grid.refile(&more), "the indices no longer line up");

        let fewer = items[..49].to_vec();
        assert!(!grid.refile(&fewer), "the indices no longer line up");
    }

    #[test]
    fn a_whole_board_on_the_move_is_handed_back_to_the_builder() {
        // Past `REFILE_MOST` the rebuild is the cheaper answer, and saying so
        // is the only way the caller knows to take it.
        let mut items = scattered(REFILE_MOST + 200);
        let mut grid = Grid::build(&items);
        for item in items.iter_mut() {
            item.x += 5_000.0;
        }
        assert!(!grid.refile(&items));
    }

    #[test]
    fn a_drag_refiled_frame_by_frame_ends_where_one_rebuild_would_have() {
        // The gesture this exists for, at the length it actually runs: a
        // hundred frames of a handful of cards moving a little each time. Any
        // leak in `pull` shows up here as a card in a square it left ninety
        // frames ago.
        let mut items = scattered(500);
        let mut grid = Grid::build(&items);
        for frame in 0..100 {
            for n in [11usize, 12, 13, 200] {
                items[n].x += 37.0;
                items[n].y -= 21.0;
            }
            assert!(grid.refile(&items), "frame {frame} gave up");
        }
        agrees(&grid, &items);

        // And the squares hold no more entries than a fresh build would, which
        // is the leak an `in_rect` comparison cannot see on its own.
        let fresh = Grid::build(&items);
        let filed: usize = grid.cells.values().map(Vec::len).sum();
        let ought: usize = fresh.cells.values().map(Vec::len).sum();
        assert_eq!(filed, ought, "the squares are holding stale entries");
    }
}
