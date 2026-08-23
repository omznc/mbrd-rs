//! Sticky notes pinned to the card they were dropped on.
//!
//! A note lying across a photograph is not near it, it is *on* it, and moving
//! the photograph has to take the note with it — otherwise the one gesture
//! everybody tries first, dragging the picture somewhere else, leaves its
//! caption behind. So the note is pinned: a drag on it takes hold of its host,
//! and a drag on the host brings it along.
//!
//! Like fence membership, the pin is **measured** rather than listed — a note
//! is stuck to whatever it is lying on — with one exception, and the exception
//! is the whole reason this module is not two functions in `fence.rs`:
//!
//! **Unstickiness is a decision, not a measurement.** The usual reason to
//! unstick a note is to nudge it, so an unstuck note is normally still lying on
//! the very card it was unstuck from, and no geometry could tell you otherwise.
//! That is why `meta.loose` is stored while `meta.stuckTo` is only recorded: one
//! of them is a fact about the board and the other is a fact about the author.
//!
//! A reader that ignores `loose` degrades one way and one way only: it measures
//! the overlap, finds the host, and treats the note as stuck again. So a board
//! unstuck in a new build opens pinned in an old one, and unstuck again in the
//! new one. Nothing is lost, and the older build writes the key back out
//! untouched.

use std::collections::HashMap;

use crate::geometry::Rect;
use crate::model::{Item, ItemType};

/// How much of the note has to be over a card before it counts as lying on it.
///
/// A note that clips a corner of a photograph on its way past is not stuck to
/// it. A fifth is low enough that a note deliberately tucked into a card's
/// corner still takes, and high enough that a graze does not.
const ENOUGH: f32 = 0.2;

/// Which note is pinned to which card.
#[derive(Debug, Clone, Default)]
pub struct Pins {
    host: HashMap<String, String>,
}

impl Pins {
    /// Measure the board, honouring what each note says about itself.
    pub fn measure(items: &[Item]) -> Self {
        Self::measured(items, true)
    }

    /// The same, but deaf to `meta.loose`.
    ///
    /// For the one caller that needs to know where a note *would* stick if it
    /// were not unstuck: a drop that finds a host clears the unstick, and
    /// asking the ordinary way would always answer "nowhere" for exactly the
    /// notes this question is about.
    pub fn measure_ignoring_loose(items: &[Item]) -> Self {
        Self::measured(items, false)
    }

    fn measured(items: &[Item], honour_loose: bool) -> Self {
        let hosts: Vec<(&str, Rect, f32)> = items
            .iter()
            .filter(|it| can_host(it))
            .map(|it| (it.id.as_str(), Rect::of_item(it), it.z))
            .collect();
        if hosts.is_empty() {
            return Self::default();
        }
        let mut host = HashMap::new();
        for note in items.iter().filter(|it| it.kind == ItemType::Note) {
            if honour_loose && is_loose(note) {
                continue;
            }
            // The record comes first, and only where it still names a card the
            // board carries. A pin has to survive a reload — and a Mobile
            // reflow, where nothing overlaps anything and measuring would
            // unstick every note on the board.
            if let Some(named) = note.meta.get("stuckTo").and_then(|v| v.as_str()) {
                if hosts.iter().any(|(id, _, _)| *id == named) {
                    host.insert(note.id.clone(), named.to_string());
                    continue;
                }
                // A dangling id — the host was deleted — falls through to
                // measuring, which is the right answer rather than an error.
            }
            if let Some(found) = lying_on(note, &hosts) {
                host.insert(note.id.clone(), found.to_string());
            }
        }
        Self { host }
    }

    /// The card this note is pinned to, if any.
    pub fn host_of(&self, note: &str) -> Option<&str> {
        self.host.get(note).map(String::as_str)
    }

    pub fn is_empty(&self) -> bool {
        self.host.is_empty()
    }

    /// The notes pinned to this card, in board order.
    pub fn stuck_to<'a>(&self, host: &str, items: &'a [Item]) -> Vec<&'a Item> {
        items.iter().filter(|it| self.host_of(&it.id) == Some(host)).collect()
    }

    /// What a drag on this item should actually take hold of.
    ///
    /// A stuck note hands the gesture to its host, which is what "pinned"
    /// means. Walks the chain, because a note stuck to a note stuck to a card
    /// should still move the card — and is bounded rather than trusted, since
    /// a board that arrives from somewhere else does not get to decide whether
    /// this returns.
    pub fn handle<'a>(&'a self, id: &'a str) -> &'a str {
        let mut at = id;
        for _ in 0..self.host.len() + 1 {
            match self.host_of(at) {
                Some(up) if up != at => at = up,
                _ => break,
            }
        }
        at
    }
}

/// Whether an author has explicitly unstuck this note.
pub fn is_loose(item: &Item) -> bool {
    item.meta.get("loose").and_then(|v| v.as_bool()).unwrap_or(false)
}

/// Whether a note may be pinned to this.
///
/// Not to another note — two notes side by side on a board would otherwise
/// grab each other — and not to the furniture, since a fence already carries
/// what is inside it and a note stuck to a fence would be carried twice.
fn can_host(item: &Item) -> bool {
    item.kind != ItemType::Note && item.kind.is_content()
}

fn lying_on<'a>(note: &Item, hosts: &[(&'a str, Rect, f32)]) -> Option<&'a str> {
    let mine = Rect::of_item(note);
    let area = mine.width() * mine.height();
    if area <= 0.0 {
        return None;
    }
    let mut best: Option<(&str, f32, f32)> = None;
    for (id, box_, z) in hosts {
        if *id == note.id {
            continue;
        }
        let over = (mine.x1.min(box_.x1) - mine.x0.max(box_.x0)).max(0.0)
            * (mine.y1.min(box_.y1) - mine.y0.max(box_.y0)).max(0.0);
        if over / area < ENOUGH {
            continue;
        }
        // Most covered wins; a tie goes to whichever is nearer the front,
        // because that is the card the note visibly sits on.
        let better = match best {
            None => true,
            Some((_, had, had_z)) => over > had || (over == had && *z > had_z),
        };
        if better {
            best = Some((id, over, *z));
        }
    }
    best.map(|(id, _, _)| id)
}

/// Write the measurement onto the notes, as the file wants it.
///
/// Called at the file boundary. `loose` is not touched — it is the author's,
/// not the measurement's.
pub fn stamp(items: &mut [Item]) {
    let pins = Pins::measure(items);
    for item in items.iter_mut() {
        if item.kind != ItemType::Note {
            continue;
        }
        match pins.host_of(&item.id) {
            Some(id) => {
                item.meta.insert("stuckTo".into(), serde_json::Value::String(id.to_string()));
            }
            None => {
                item.meta.remove("stuckTo");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn at(id: &str, kind: ItemType, x: f32, y: f32, w: f32, h: f32) -> Item {
        let mut item = Item::new(id, kind);
        item.x = x;
        item.y = y;
        item.w = w;
        item.h = h;
        item
    }

    fn photo(id: &str, x: f32, y: f32) -> Item {
        at(id, ItemType::Image, x, y, 300.0, 200.0)
    }

    fn note(id: &str, x: f32, y: f32) -> Item {
        at(id, ItemType::Note, x, y, 100.0, 60.0)
    }

    #[test]
    fn a_note_dropped_on_a_photograph_sticks_to_it() {
        let items = vec![photo("p", 0.0, 0.0), note("n", 40.0, 20.0)];
        assert_eq!(Pins::measure(&items).host_of("n"), Some("p"));
    }

    #[test]
    fn a_note_that_merely_grazes_a_card_is_not_on_it() {
        // Overlapping by a sliver of one corner. Somebody who wanted this
        // stuck would have put it on the card.
        let items = vec![photo("p", 0.0, 0.0), note("n", 195.0, 125.0)];
        assert_eq!(Pins::measure(&items).host_of("n"), None);
    }

    #[test]
    fn a_note_over_two_cards_sticks_to_the_one_it_is_mostly_on() {
        let items =
            vec![photo("left", -150.0, 0.0), photo("right", 190.0, 0.0), note("n", 100.0, 0.0)];
        assert_eq!(Pins::measure(&items).host_of("n"), Some("right"));
    }

    #[test]
    fn a_note_on_two_cards_at_the_same_depth_takes_the_nearer_one() {
        let mut back = photo("back", 0.0, 0.0);
        back.z = 1.0;
        let mut front = photo("front", 0.0, 0.0);
        front.z = 9.0;
        let items = vec![back, front, note("n", 0.0, 0.0)];
        assert_eq!(Pins::measure(&items).host_of("n"), Some("front"));
    }

    #[test]
    fn notes_do_not_stick_to_each_other() {
        let items = vec![note("a", 0.0, 0.0), note("b", 10.0, 5.0)];
        assert!(Pins::measure(&items).is_empty());
    }

    #[test]
    fn an_unstuck_note_stays_unstuck_while_lying_on_its_host() {
        // The case no geometry can answer, and the reason `loose` is stored.
        let mut n = note("n", 40.0, 20.0);
        n.meta.insert("loose".into(), Value::Bool(true));
        let items = vec![photo("p", 0.0, 0.0), n];
        assert_eq!(Pins::measure(&items).host_of("n"), None);
    }

    #[test]
    fn the_record_holds_a_pin_that_the_geometry_no_longer_shows() {
        // A Mobile reflow packs the note somewhere else entirely, and measuring
        // there would unstick every note on the board.
        let mut n = note("n", 4000.0, 4000.0);
        n.meta.insert("stuckTo".into(), Value::String("p".into()));
        let items = vec![photo("p", 0.0, 0.0), n];
        assert_eq!(Pins::measure(&items).host_of("n"), Some("p"));
    }

    #[test]
    fn a_pin_to_a_card_that_is_gone_falls_back_to_measuring() {
        let mut n = note("n", 40.0, 20.0);
        n.meta.insert("stuckTo".into(), Value::String("deleted".into()));
        let items = vec![photo("p", 0.0, 0.0), n];
        assert_eq!(Pins::measure(&items).host_of("n"), Some("p"));
    }

    #[test]
    fn where_a_note_would_stick_can_be_asked_even_of_one_that_is_unstuck() {
        // The question a drop asks: putting an unstuck note down on a card is
        // the author taking the unstick back, and the ordinary measurement
        // would always answer "nowhere" for exactly these notes.
        let mut n = note("n", 40.0, 20.0);
        n.meta.insert("loose".into(), Value::Bool(true));
        let items = vec![photo("p", 0.0, 0.0), n];
        assert_eq!(Pins::measure(&items).host_of("n"), None);
        assert_eq!(Pins::measure_ignoring_loose(&items).host_of("n"), Some("p"));
    }

    #[test]
    fn a_drag_on_a_stuck_note_takes_hold_of_its_host() {
        let items = vec![photo("p", 0.0, 0.0), note("n", 40.0, 20.0)];
        let pins = Pins::measure(&items);
        assert_eq!(pins.handle("n"), "p");
        assert_eq!(pins.handle("p"), "p");
        // And a card nothing knows about is its own handle.
        assert_eq!(pins.handle("stranger"), "stranger");
    }

    #[test]
    fn the_stamp_records_the_pin_and_leaves_the_decision_alone() {
        let mut n = note("n", 40.0, 20.0);
        n.meta.insert("loose".into(), Value::Bool(true));
        let mut items = vec![photo("p", 0.0, 0.0), n, note("free", 900.0, 900.0)];
        items[2].meta.insert("stuckTo".into(), Value::String("ghost".into()));
        stamp(&mut items);
        // Unstuck, so nothing is recorded — but `loose` survives, because it
        // is not the measurement's to clear.
        assert!(!items[1].meta.contains_key("stuckTo"));
        assert!(is_loose(&items[1]));
        // And a stale record on a note lying on nothing is cleared.
        assert!(!items[2].meta.contains_key("stuckTo"));
    }
}
