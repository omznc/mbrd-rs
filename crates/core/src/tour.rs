//! The board read as a sequence of stops rather than as a surface.
//!
//! `board.tour` — a flat list of item ids, in the order somebody wants them
//! seen — has round-tripped through this build since the format was first read
//! and nothing has ever consumed it. This is the half that was missing: what
//! the list *means* against a live board, which is a question with no window in
//! it and belongs here. The runner, the camera move and the bar are the app's.
//!
//! ## The itinerary is board data; where you have got to is not
//!
//! What goes in the file is the route — the cards, in order — because that is a
//! thing somebody made and would expect to find again. Which stop they were on
//! when they closed the window is not: it is a position in a *reading*, the
//! same kind of fact as a playhead, and writing it down would make opening a
//! board a change to it.
//!
//! ## The list is resolved live, every time
//!
//! [`stops`] maps the ids through the board and drops what is not there, and
//! nothing anywhere holds the resolved list across a change. A tour running
//! while cards are being deleted is exactly the window where a cached list and
//! the board disagree, and resolving costs one pass over a handful of ids.
//! Stepping therefore survives an undo that takes a stop out from under it.

use crate::model::{Board, Item};

/// The most stops a tour may have.
///
/// The same ceiling the format's own `normalizeTour` applies, and it is not a
/// technical one: past a few dozen a route stops being a reading of the board
/// and becomes a second copy of it in a different order.
pub const MAX: usize = 200;

/// The stops that are actually on the board, in the tour's own order.
///
/// Furniture is dropped as well as the missing: the title card and the
/// onboarding hints are the app talking, and a route that stopped at one would
/// be the board showing somebody its own scaffolding.
pub fn stops(board: &Board) -> Vec<&Item> {
    board
        .tour
        .iter()
        .filter_map(|id| board.item(id))
        .filter(|item| item.kind.is_content())
        .collect()
}

/// Whether this card is on the route.
pub fn on_tour(board: &Board, id: &str) -> bool {
    board.tour.iter().any(|stop| stop == id)
}

/// Put a card on the route or take it off, and answer whether anything changed.
///
/// Added at the **end**, which is the only place an order somebody is building
/// can be added to without asking them where. Taking one off closes the gap; a
/// route is a sequence, so there is no hole to leave.
pub fn mark(board: &mut Board, id: &str, on: bool) -> bool {
    let held = on_tour(board, id);
    if held == on {
        return false;
    }
    if on {
        if board.tour.len() >= MAX {
            return false;
        }
        board.tour.push(id.to_string());
    } else {
        board.tour.retain(|stop| stop != id);
    }
    true
}

/// Where a card sits on the route, counting from one, for a card that is on it.
///
/// Over the *resolved* stops rather than the raw ids, so the number matches
/// what the bar counts up to. A card on the route whose neighbours have been
/// deleted is stop 2 of 3 rather than stop 7 of 3.
pub fn position(board: &Board, id: &str) -> Option<usize> {
    stops(board).iter().position(|item| item.id == id).map(|i| i + 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ItemType;

    fn board_of(kinds: &[(&str, ItemType)], tour: &[&str]) -> Board {
        Board {
            items: kinds
                .iter()
                .map(|(id, kind)| Item::new((*id).to_string(), kind.clone()))
                .collect(),
            tour: tour.iter().map(|id| (*id).to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn a_stop_that_is_no_longer_on_the_board_is_walked_past() {
        let board = board_of(&[("a", ItemType::Image), ("c", ItemType::Image)], &["a", "b", "c"]);
        let ids: Vec<&str> = stops(&board).iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["a", "c"]);
    }

    /// The route is the route's order, not the board's.
    #[test]
    fn the_stops_come_back_in_the_order_the_tour_names_them() {
        let board = board_of(&[("a", ItemType::Image), ("b", ItemType::Image)], &["b", "a"]);
        let ids: Vec<&str> = stops(&board).iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["b", "a"]);
    }

    #[test]
    fn the_apps_own_furniture_is_never_a_stop() {
        let board = board_of(&[("t", ItemType::Title), ("a", ItemType::Image)], &["t", "a"]);
        assert_eq!(stops(&board).len(), 1);
    }

    #[test]
    fn a_card_joins_the_route_at_the_end_and_leaves_without_a_hole() {
        let mut board = board_of(
            &[("a", ItemType::Image), ("b", ItemType::Image), ("c", ItemType::Image)],
            &["a", "b"],
        );
        assert!(mark(&mut board, "c", true));
        assert_eq!(board.tour, ["a", "b", "c"]);
        assert!(!mark(&mut board, "c", true), "already on it");
        assert!(mark(&mut board, "b", false));
        assert_eq!(board.tour, ["a", "c"]);
    }

    /// The number the bar counts up to is the number of stops that exist, so
    /// the number it counts *from* has to be too.
    #[test]
    fn a_position_is_counted_over_the_stops_that_are_still_there() {
        let board = board_of(&[("a", ItemType::Image), ("c", ItemType::Image)], &["a", "b", "c"]);
        assert_eq!(position(&board, "c"), Some(2));
        assert_eq!(position(&board, "b"), None);
    }
}
