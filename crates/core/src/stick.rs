//! Sticky notes pinned to the card they were dropped on.
//!
//! A note the author has marked **sticky** and dropped across a photograph is
//! not near it, it is *on* it: moving the photograph takes the note with it,
//! and a drag on the note takes hold of the photograph. That is the whole
//! feature, and the flag is the whole door onto it.
//!
//! **Stickiness is a decision, not a measurement.** It used to be the other
//! way around — any note lying on a card was pinned, with `meta.loose` as the
//! opt-out — and the first thing everybody hit was two things that merely
//! overlapped refusing to move apart, for a reason nothing on screen could
//! explain. A gesture that grabs more than what was pressed has to have been
//! asked for. So `meta.sticky` is the opt-in, off by default, set from the
//! note's own menu; *which card* a sticky note is on is still measured — a
//! sticky note is stuck to whatever it is lying on — and recorded as
//! `meta.stuckTo` at the file boundary.
//!
//! `meta.loose` is not read any more, and is deliberately not deleted either:
//! it belongs to the old rule, an unknown key rides through `meta` untouched,
//! and a build that still reads it should keep finding what its author wrote.

use std::collections::HashMap;

use crate::geometry::Rect;
use crate::model::{Item, ItemType};

/// How much of the note has to be over a card before it counts as lying on it.
///
/// A sticky note that clips a corner of a photograph on its way past is not
/// stuck to it. A fifth is low enough that a note deliberately tucked into a
/// card's corner still takes, and high enough that a graze does not.
const ENOUGH: f32 = 0.2;

/// Which note is pinned to which card.
#[derive(Debug, Clone, Default)]
pub struct Pins {
    host: HashMap<String, String>,
}

impl Pins {
    /// Measure the board. Only notes whose author said `sticky` take part.
    pub fn measure(items: &[Item]) -> Self {
        let hosts: Vec<(&str, Rect, f32)> = items
            .iter()
            .filter(|it| can_host(it))
            .map(|it| (it.id.as_str(), Rect::of_item(it), it.z))
            .collect();
        if hosts.is_empty() {
            return Self::default();
        }
        let mut host = HashMap::new();
        for note in items.iter().filter(|it| it.kind == ItemType::Note && is_sticky(it)) {
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

/// Whether an author has asked this note to pin to what it lies on.
pub fn is_sticky(item: &Item) -> bool {
    item.meta.get("sticky").and_then(|v| v.as_bool()).unwrap_or(false)
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
/// Called at the file boundary. `sticky` is not touched — it is the author's,
/// not the measurement's — and a note that is not sticky carries no record,
/// because a record is a pin and it has none.
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

    fn sticky(id: &str, x: f32, y: f32) -> Item {
        let mut it = note(id, x, y);
        it.meta.insert("sticky".into(), Value::Bool(true));
        it
    }

    #[test]
    fn a_plain_note_on_a_photograph_is_not_stuck_to_it() {
        // The rule the old default broke: overlap alone is not a request.
        let items = vec![photo("p", 0.0, 0.0), note("n", 40.0, 20.0)];
        assert!(Pins::measure(&items).is_empty());
    }

    #[test]
    fn a_sticky_note_dropped_on_a_photograph_sticks_to_it() {
        let items = vec![photo("p", 0.0, 0.0), sticky("n", 40.0, 20.0)];
        assert_eq!(Pins::measure(&items).host_of("n"), Some("p"));
    }

    #[test]
    fn a_sticky_note_that_merely_grazes_a_card_is_not_on_it() {
        // Overlapping by a sliver of one corner. Somebody who wanted this
        // stuck would have put it on the card.
        let items = vec![photo("p", 0.0, 0.0), sticky("n", 195.0, 125.0)];
        assert_eq!(Pins::measure(&items).host_of("n"), None);
    }

    #[test]
    fn a_sticky_note_over_two_cards_sticks_to_the_one_it_is_mostly_on() {
        let items =
            vec![photo("left", -150.0, 0.0), photo("right", 190.0, 0.0), sticky("n", 100.0, 0.0)];
        assert_eq!(Pins::measure(&items).host_of("n"), Some("right"));
    }

    #[test]
    fn a_sticky_note_on_two_cards_at_the_same_depth_takes_the_nearer_one() {
        let mut back = photo("back", 0.0, 0.0);
        back.z = 1.0;
        let mut front = photo("front", 0.0, 0.0);
        front.z = 9.0;
        let items = vec![back, front, sticky("n", 0.0, 0.0)];
        assert_eq!(Pins::measure(&items).host_of("n"), Some("front"));
    }

    #[test]
    fn sticky_notes_do_not_stick_to_each_other() {
        let items = vec![sticky("a", 0.0, 0.0), sticky("b", 10.0, 5.0)];
        assert!(Pins::measure(&items).is_empty());
    }

    #[test]
    fn the_old_loose_flag_is_not_what_keeps_a_note_free() {
        // A note from an old file: `loose` present, `sticky` absent. It is
        // free because nothing asked it to stick, not because of the old key.
        let mut n = note("n", 40.0, 20.0);
        n.meta.insert("loose".into(), Value::Bool(true));
        let items = vec![photo("p", 0.0, 0.0), n];
        assert_eq!(Pins::measure(&items).host_of("n"), None);
    }

    #[test]
    fn the_record_holds_a_pin_that_the_geometry_no_longer_shows() {
        // A Mobile reflow packs the note somewhere else entirely, and measuring
        // there would unstick every note on the board.
        let mut n = sticky("n", 4000.0, 4000.0);
        n.meta.insert("stuckTo".into(), Value::String("p".into()));
        let items = vec![photo("p", 0.0, 0.0), n];
        assert_eq!(Pins::measure(&items).host_of("n"), Some("p"));
    }

    #[test]
    fn a_record_without_the_decision_is_not_a_pin() {
        // A file stamped by an old build, where every overlapping note got a
        // `stuckTo`. The author never asked, so the note is free — this is
        // the migration, and it is one line: no `sticky`, no pin.
        let mut n = note("n", 40.0, 20.0);
        n.meta.insert("stuckTo".into(), Value::String("p".into()));
        let items = vec![photo("p", 0.0, 0.0), n];
        assert!(Pins::measure(&items).is_empty());
    }

    #[test]
    fn a_pin_to_a_card_that_is_gone_falls_back_to_measuring() {
        let mut n = sticky("n", 40.0, 20.0);
        n.meta.insert("stuckTo".into(), Value::String("deleted".into()));
        let items = vec![photo("p", 0.0, 0.0), n];
        assert_eq!(Pins::measure(&items).host_of("n"), Some("p"));
    }

    #[test]
    fn a_drag_on_a_stuck_note_takes_hold_of_its_host() {
        let items = vec![photo("p", 0.0, 0.0), sticky("n", 40.0, 20.0)];
        let pins = Pins::measure(&items);
        assert_eq!(pins.handle("n"), "p");
        assert_eq!(pins.handle("p"), "p");
        // And a card nothing knows about is its own handle.
        assert_eq!(pins.handle("stranger"), "stranger");
    }

    #[test]
    fn the_stamp_records_the_pin_and_clears_what_is_not_one() {
        let mut items =
            vec![photo("p", 0.0, 0.0), sticky("on", 40.0, 20.0), note("plain", 60.0, 30.0)];
        // The plain note carries a record from an old build.
        items[2].meta.insert("stuckTo".into(), Value::String("p".into()));
        stamp(&mut items);
        assert_eq!(items[1].meta.get("stuckTo"), Some(&Value::String("p".into())));
        // Not sticky, so not pinned, so no record — however it got there.
        assert!(!items[2].meta.contains_key("stuckTo"));
        // And the decision itself is not the stamp's to touch.
        assert!(is_sticky(&items[1]));
    }
}
