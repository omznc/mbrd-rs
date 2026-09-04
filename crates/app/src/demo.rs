//! A board to look at before there is one to open.
//!
//! Built through [`mbrd_core::schema::normalize`] rather than by assembling
//! structs by hand, and deliberately so: it means the thing the app shows on a
//! cold start has been through exactly the same door as a file off a disk. A
//! board built by hand would be the one board in existence that never
//! exercises the reader.
//!
//! ## One note, and nothing else
//!
//! This used to be a dozen cards: photographs drawn at runtime, a fence, a
//! rope with a label on it, a card of a type no build has ever heard of. All
//! of it was a demonstration, and that was the problem — the first thing
//! anybody saw was somebody else's board, and the first thing they had to do
//! was clear it off.
//!
//! What is left says the one thing worth saying on an empty canvas and then
//! gets out of the way. Everything the old board showed off is still in the
//! app and still tested, in the tests that were always the real check on it:
//! `schema`, `fence` and `mbrd` in the core, and the round-trip tests here.

use serde_json::json;

use mbrd_core::{schema, BoardState, Document};

/// What the note says.
///
/// Lower case and short on purpose. It is a first sentence, not a manual:
/// there is a command palette two keys away and a whole settings page behind
/// it, and neither of them is something to read before touching anything.
const HELLO: &str =
    "# mbrd\n\nyou'll figure it out as you go along, try dragging things into this window";

/// How big the note is, and therefore where it goes.
///
/// Centred on the origin — which is where the camera starts, so the first
/// thing drawn is in the middle of the window rather than off in a corner
/// somebody has to find.
const WIDE: f64 = 380.0;
const TALL: f64 = 200.0;

pub fn board() -> Document {
    let value = json!({
        "title": "welcome",
        "view": { "pan": { "x": 0, "y": 0 }, "zoom": 1 },
        "settings": { "grid": true, "axes": true, "snap": false, "gridStep": 64 },
        "items": [
            { "id": "hello", "type": "note", "z": 1,
              "x": -WIDE / 2.0, "y": -TALL / 2.0, "w": WIDE, "h": TALL,
              "name": "hello",
              "meta": { "text": HELLO } }
        ],
        "connections": []
    });

    Document { board: BoardState::new(schema::normalize(&value)), ..Document::default() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_board_somebody_opens_first_is_one_note_saying_hello() {
        let doc = board();
        assert_eq!(doc.board.items.len(), 1, "one card, and it is the note");
        assert!(doc.assets.is_empty(), "nothing to carry");

        let note = &doc.board.items[0];
        assert_eq!(note.kind, mbrd_core::ItemType::Note);
        assert_eq!(note.meta.get("text").and_then(serde_json::Value::as_str), Some(HELLO));
    }

    /// The whole reason this is built out of JSON rather than out of structs:
    /// the first board anybody sees has been read by the same code a file off
    /// a disk is read by, so a reader that broke would break here first.
    #[test]
    fn it_is_centred_on_where_the_camera_starts() {
        let doc = board();
        let note = &doc.board.items[0];
        assert!((note.x + note.w / 2.0).abs() < 1.0, "off centre across: {}", note.x);
        assert!((note.y + note.h / 2.0).abs() < 1.0, "off centre down: {}", note.y);
    }
}
