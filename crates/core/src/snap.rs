//! Snap-to-grid, and putting things back when it is turned off.
//!
//! Snapping is a setting rather than an action, which is the whole of what
//! makes it awkward: turning it *on* moves every card on the board, and a
//! person who turns it on to see what it looks like and then turns it off again
//! has every right to get their board back. So each card that the grid takes
//! remembers where it was, in the layout's `presnap`, and turning the setting
//! off puts it there.
//!
//! Three rules keep that memo honest:
//!
//! - **A card that was already on the lattice gets no memo.** There is nothing
//!   to put back, and a memo saying "where it is now" is a memo that survives a
//!   later hand-move and then undoes it.
//! - **A memo is written once.** Snapping an already-snapped board — a smaller
//!   grid step, say — must not overwrite the first memo with the lattice
//!   position, or releasing lands the card on the old lattice instead of where
//!   the author put it.
//! - **Releasing clears the memo.** It has been spent; keeping it means the
//!   next release moves cards that nothing moved.
//!
//! Sizes snap as well as positions, because a card whose corner is on the
//! lattice and whose opposite corner is not is exactly as untidy as a card that
//! is not on it at all.
//!
//! ## What "on the lattice" means
//!
//! A card is on it when **its box is a whole number of cells and every edge
//! falls on a dot** — see [`geometry::conform`](crate::geometry::conform),
//! which is the one place that is decided.
//!
//! Both halves are load-bearing, and the second is the one this used to get
//! wrong. Rounding an item's `x` and `y` rounds its *centre*, and a card of an
//! odd number of cells centred on a dot hangs half a cell over the pattern on
//! both sides — three cells reaches a cell and a half each way. So a board
//! that had just been snapped could still be visibly halfway onto its own
//! grid, which is the whole of what somebody turning the setting on is asking
//! not to happen. The edge is what a person lines up and the edge is what gets
//! rounded; the centre is wherever that leaves it.
//!
//! And a card is never smaller than one cell. A square of the pattern is the
//! smallest thing the grid can express, so a card below it has a size the grid
//! has no way to describe and no dot it could be lined up against.

use crate::geometry::{clamp_size, conform};
use crate::model::{Board, LayoutMode, PreSnap, MIN_SIZE};

/// Below this there is no lattice, only rounding noise.
const SMALLEST_STEP: f32 = 1.0;

/// Take every card in this layout onto the lattice, remembering where it was.
///
/// Returns whether anything moved, so a caller at the door can decline to
/// record a step for a board that was already square.
pub fn engage(board: &mut Board, mode: LayoutMode, step: f32) -> bool {
    // Spelled out rather than written as a negated comparison, because the
    // interesting input here is a step that is not a number at all — a board
    // from somewhere else may carry one, and `NaN < x` is false, which would
    // let it through.
    if step.is_nan() || step < SMALLEST_STEP {
        return false;
    }
    let mut touched = false;
    for g in board.layouts.get_mut(mode).iter_mut() {
        let (x, y, w, h) = conform(g.x, g.y, g.w, g.h, step);
        if same(x, g.x) && same(y, g.y) && same(w, g.w) && same(h, g.h) {
            // Already on the lattice. No memo, because there is nothing to
            // remember and a memo here would fire on some later release.
            continue;
        }
        // Written once. A second pass at a different step must not record the
        // first pass's lattice position as "where the author put it".
        g.presnap.get_or_insert(PreSnap { x: g.x, y: g.y, w: g.w, h: g.h });
        g.x = x;
        g.y = y;
        g.w = w;
        g.h = h;
        touched = true;
    }
    if touched && mode == LayoutMode::Desktop {
        adopt(board);
    }
    touched
}

/// Put back every card the lattice took, and forget that it did.
pub fn release(board: &mut Board, mode: LayoutMode) -> bool {
    let mut touched = false;
    for g in board.layouts.get_mut(mode).iter_mut() {
        let Some(was) = g.presnap.take() else { continue };
        g.x = was.x;
        g.y = was.y;
        g.w = clamp_size(was.w);
        g.h = clamp_size(was.h);
        touched = true;
    }
    if touched && mode == LayoutMode::Desktop {
        adopt(board);
    }
    touched
}

/// Whether this memo is worth believing.
///
/// Four finite numbers with a size inside the legal range, or it is dropped —
/// a memo is a promise to move a card somewhere, and a board from somewhere
/// else does not get to promise a card to infinity.
pub fn sound(p: &PreSnap) -> bool {
    p.x.is_finite()
        && p.y.is_finite()
        && p.w.is_finite()
        && p.h.is_finite()
        && p.w >= MIN_SIZE
        && p.h >= MIN_SIZE
        && p.w <= crate::model::MAX_SIZE
        && p.h <= crate::model::MAX_SIZE
}

fn same(a: f32, b: f32) -> bool {
    (a - b).abs() < 0.001
}

/// The Desktop layout back onto the items.
///
/// [`state`](crate::state) levels these in the other direction after every
/// edit — items onto layout — because that is the direction the app writes in.
/// This module is the one place that writes the layout first, so it is the one
/// place that has to level them itself.
fn adopt(board: &mut Board) {
    let by_id: std::collections::HashMap<&str, (f32, f32, f32, f32)> =
        board.layouts.desktop.iter().map(|g| (g.id.as_str(), (g.x, g.y, g.w, g.h))).collect();
    let changes: Vec<(usize, (f32, f32, f32, f32))> = board
        .items
        .iter()
        .enumerate()
        .filter_map(|(n, item)| by_id.get(item.id.as_str()).map(|g| (n, *g)))
        .collect();
    for (n, (x, y, w, h)) in changes {
        let item = &mut board.items[n];
        item.x = x;
        item.y = y;
        item.w = w;
        item.h = h;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Item, ItemType};
    use crate::state::BoardState;

    fn board(places: &[(f32, f32)]) -> Board {
        let mut b = Board::default();
        for (n, (x, y)) in places.iter().enumerate() {
            let mut item = Item::new(format!("i{n}"), ItemType::Image);
            item.x = *x;
            item.y = *y;
            item.w = 128.0;
            item.h = 128.0;
            b.items.push(item);
        }
        // Through the door, so `layouts.desktop` exists to snap.
        let mut state = BoardState::new(b);
        state.edit("seed", |_| {});
        (*state).clone()
    }

    #[test]
    fn snapping_puts_a_card_on_the_lattice() {
        let mut b = board(&[(37.0, -91.0)]);
        assert!(engage(&mut b, LayoutMode::Desktop, 64.0));
        assert_eq!((b.items[0].x, b.items[0].y), (64.0, -64.0));
        assert_eq!((b.layouts.desktop[0].x, b.layouts.desktop[0].y), (64.0, -64.0));
    }

    #[test]
    fn turning_it_off_puts_the_card_back_where_it_was() {
        let mut b = board(&[(37.0, -91.0)]);
        engage(&mut b, LayoutMode::Desktop, 64.0);
        assert!(release(&mut b, LayoutMode::Desktop));
        assert_eq!((b.items[0].x, b.items[0].y), (37.0, -91.0));
        assert!(b.layouts.desktop[0].presnap.is_none(), "the memo was spent");
    }

    #[test]
    fn a_card_already_on_the_lattice_gets_no_memo() {
        // Otherwise its memo says "where it is now", survives a hand-move, and
        // then quietly undoes that move the next time snapping is turned off.
        let mut b = board(&[(128.0, 64.0)]);
        engage(&mut b, LayoutMode::Desktop, 64.0);
        assert!(b.layouts.desktop[0].presnap.is_none());
        b.items[0].x = 300.0;
        b.layouts.desktop[0].x = 300.0;
        release(&mut b, LayoutMode::Desktop);
        assert_eq!(b.items[0].x, 300.0);
    }

    #[test]
    fn snapping_twice_still_remembers_the_first_place() {
        let mut b = board(&[(37.0, -91.0)]);
        engage(&mut b, LayoutMode::Desktop, 64.0);
        engage(&mut b, LayoutMode::Desktop, 16.0);
        release(&mut b, LayoutMode::Desktop);
        assert_eq!((b.items[0].x, b.items[0].y), (37.0, -91.0));
    }

    #[test]
    fn a_square_board_is_not_an_edit() {
        let mut b = board(&[(0.0, 0.0), (128.0, 256.0)]);
        assert!(!engage(&mut b, LayoutMode::Desktop, 64.0));
        assert!(!release(&mut b, LayoutMode::Desktop));
    }

    #[test]
    fn a_step_of_nothing_is_not_a_lattice() {
        let mut b = board(&[(37.0, -91.0)]);
        assert!(!engage(&mut b, LayoutMode::Desktop, 0.0));
        assert!(!engage(&mut b, LayoutMode::Desktop, f32::NAN));
        assert_eq!(b.items[0].x, 37.0);
    }

    #[test]
    fn a_memo_that_promises_a_card_to_infinity_is_not_believed() {
        assert!(sound(&PreSnap { x: 1.0, y: 2.0, w: 100.0, h: 100.0 }));
        assert!(!sound(&PreSnap { x: f32::NAN, y: 2.0, w: 100.0, h: 100.0 }));
        assert!(!sound(&PreSnap { x: 1.0, y: 2.0, w: 0.0, h: 100.0 }));
        assert!(!sound(&PreSnap { x: 1.0, y: 2.0, w: 1e9, h: 100.0 }));
    }

    /// Every edge of every card, as a number of cells from the origin.
    ///
    /// The measurement the whole module is about: a card is on the lattice
    /// when these are whole numbers, and the failure this is here to catch is
    /// a half — a card sitting across the squares rather than on them.
    fn edges(b: &Board, step: f32) -> Vec<f32> {
        b.items
            .iter()
            .flat_map(|i| [i.x - i.w / 2.0, i.x + i.w / 2.0, i.y - i.h / 2.0, i.y + i.h / 2.0])
            .map(|e| e / step)
            .collect()
    }

    fn whole(v: f32) -> bool {
        (v - v.round()).abs() < 0.001
    }

    #[test]
    fn a_card_an_odd_number_of_cells_wide_still_lands_its_edges_on_the_lattice() {
        // The bug this module was rewritten for. Three cells is 192, and
        // rounding the *centre* onto a dot leaves both edges 96 from it —
        // dead in the middle of a square, on both sides, every time.
        let mut b = board(&[(37.0, -91.0)]);
        b.items[0].w = 192.0;
        b.items[0].h = 192.0;
        b.layouts.desktop[0].w = 192.0;
        b.layouts.desktop[0].h = 192.0;
        engage(&mut b, LayoutMode::Desktop, 64.0);
        assert_eq!((b.items[0].w, b.items[0].h), (192.0, 192.0), "three cells is already whole");
        for e in edges(&b, 64.0) {
            assert!(whole(e), "an edge {e} cells from the origin is halfway onto the grid");
        }
        // And the centre is where the middle of an odd card actually is:
        // between two dots, not on one.
        assert!(!whole(b.items[0].x / 64.0), "an odd card's centre is not on a dot");
    }

    #[test]
    fn every_size_a_board_can_hold_comes_out_whole_cells_and_on_the_lattice() {
        // Swept rather than sampled: the failure was size-dependent — even
        // counts passed and odd ones did not — so one card of one size proves
        // nothing about the rule.
        let step = 64.0;
        let mut b = board(&[(0.0, 0.0); 12]);
        for (n, item) in b.items.iter_mut().enumerate() {
            let n = n as f32;
            item.x = n * 37.0 - 200.0;
            item.y = 91.0 - n * 53.0;
            item.w = 40.0 + n * 61.0;
            item.h = 300.0 - n * 19.0;
        }
        let sizes: Vec<(f32, f32)> = b.items.iter().map(|i| (i.w, i.h)).collect();
        for (g, (w, h)) in b.layouts.desktop.iter_mut().zip(&sizes) {
            g.w = *w;
            g.h = *h;
        }
        engage(&mut b, LayoutMode::Desktop, step);
        for item in &b.items {
            assert!(whole(item.w / step), "a width of {} is not whole cells", item.w);
            assert!(whole(item.h / step), "a height of {} is not whole cells", item.h);
        }
        for e in edges(&b, step) {
            assert!(whole(e), "an edge {e} cells from the origin is halfway onto the grid");
        }
    }

    #[test]
    fn nothing_ends_up_smaller_than_one_square_of_the_pattern() {
        // A card under half a cell used to round to nothing and then be
        // rescued by the `MIN_SIZE` clamp, which landed it at 48 — smaller
        // than the 64 square it is supposed to fill, and not a multiple of
        // anything.
        let mut b = board(&[(0.0, 0.0)]);
        b.items[0].w = 12.0;
        b.items[0].h = 300.0;
        b.layouts.desktop[0].w = 12.0;
        b.layouts.desktop[0].h = 300.0;
        engage(&mut b, LayoutMode::Desktop, 64.0);
        assert_eq!(b.items[0].w, 64.0, "one whole cell, not MIN_SIZE");
    }

    #[test]
    fn a_lattice_finer_than_the_smallest_card_still_leaves_room_for_the_grips() {
        // The other end of the same rule. At a step of 16 one cell is well
        // under `MIN_SIZE`, so the floor is however many whole cells clear it
        // — three, here — rather than the one cell the grid would allow.
        let mut b = board(&[(0.0, 0.0)]);
        b.items[0].w = 8.0;
        b.items[0].h = 8.0;
        b.layouts.desktop[0].w = 8.0;
        b.layouts.desktop[0].h = 8.0;
        engage(&mut b, LayoutMode::Desktop, 16.0);
        assert_eq!(b.items[0].w, 48.0, "three whole cells, which is also MIN_SIZE");
        assert!(b.items[0].w >= crate::model::MIN_SIZE);
        assert!(whole(b.items[0].w / 16.0));
    }

    #[test]
    fn a_size_snaps_too() {
        let mut b = board(&[(0.0, 0.0)]);
        b.items[0].w = 137.0;
        b.layouts.desktop[0].w = 137.0;
        engage(&mut b, LayoutMode::Desktop, 64.0);
        assert_eq!(b.items[0].w, 128.0);
        release(&mut b, LayoutMode::Desktop);
        assert_eq!(b.items[0].w, 137.0);
    }
}
