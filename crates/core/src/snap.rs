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

use crate::geometry::{clamp_size, snap};
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
        let (x, y) = (snap(g.x, step), snap(g.y, step));
        let (w, h) = (clamp_size(snap(g.w, step)), clamp_size(snap(g.h, step)));
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
