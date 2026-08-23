//! A board, all the way to a real file and back.
//!
//! The unit tests in `mbrd.rs` pack and unpack in memory, which is where the
//! interesting logic is. This one exists for the part they cannot cover: that
//! what lands on a disk is a file other tools agree is a ZIP, and that reading
//! it back through the public API of the crate — not through a private
//! helper — returns the same board.
//!
//! It also leaves the file behind under `target/`, which makes it the quickest
//! way to get something for the app to open.

use std::collections::HashMap;
use std::io::Cursor;

use mbrd_core::mbrd::{self, Asset, Document, Manifest};
use mbrd_core::model::{ItemAsset, ItemType};
use mbrd_core::{naming, schema, BoardState};

fn sample() -> Document {
    let value = serde_json::json!({
        "title": "Kitchen",
        "view": { "pan": { "x": 40, "y": -20 }, "zoom": 0.8 },
        "settings": { "grid": true, "snap": true, "gridStep": 64, "spacing": 32,
                      "appearance": { "palette": "papyrus", "vars": { "--accent": "#b4553a" } } },
        "arrangement": "spiral",
        "items": [
            { "id": "note1", "type": "note", "x": -120, "y": 80, "w": 240, "h": 200, "z": 2,
              "name": "buy the smaller one",
              "meta": { "text": "# buy the smaller one\n\nthe big one does not fit under the shelf",
                        "tint": 2 } },
            { "id": "shelf", "type": "image", "x": 160, "y": -30, "w": 320, "h": 240, "z": 1,
              "name": "shelf.jpg" },
            { "id": "odd", "type": "hologram", "x": 500, "y": 0, "w": 120, "h": 120, "z": 3,
              "name": "from a newer build", "meta": { "unknown": [1, 2, 3] } }
        ],
        "connections": [["note1", "shelf", { "color": "leaf", "label": "same shelf" }]],
        "tour": ["shelf", "note1"]
    });

    let mut board = schema::normalize(&value);

    // Give the photograph some bytes, so the asset path is exercised too.
    let pixels = b"\xff\xd8\xff\xe0 not really a jpeg, but it is consistent".to_vec();
    let hash = mbrd::hash_bytes(&pixels);
    if let Some(item) = board.item_mut("shelf") {
        item.asset = Some(ItemAsset::Embedded { hash: hash.clone(), family: Some("jpeg".into()) });
    }

    let mut assets = HashMap::new();
    assets.insert(hash, Asset { bytes: pixels, ext: "jpg".into(), label: "shelf".into() });

    Document {
        manifest: Manifest::default(),
        board: BoardState::new(board),
        assets,
        waveforms: HashMap::new(),
    }
}

#[test]
fn a_board_survives_a_trip_through_a_real_file() {
    let doc = sample();

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target").join("sample");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(naming::file_name_for(&doc.board));

    let bytes = mbrd::to_bytes(&doc, &naming::now_iso8601()).unwrap();
    std::fs::write(&path, &bytes).unwrap();

    let back = mbrd::read(Cursor::new(std::fs::read(&path).unwrap())).unwrap();

    assert_eq!(back.board.title, "Kitchen");
    assert_eq!(back.board.items.len(), 3);

    // The camera travelled with the board.
    assert_eq!(back.board.view.pan_x, 40.0);
    assert_eq!(back.board.view.zoom, 0.8);

    // The note came back through its sidecar, which is what a reader prefers.
    let note = back.board.item("note1").unwrap();
    assert!(note.note_text().unwrap().contains("does not fit under the shelf"));

    // The unknown type and its unknown meta both survived a build that has
    // never heard of either.
    let odd = back.board.item("odd").unwrap();
    assert_eq!(odd.kind, ItemType::Other("hologram".into()));
    assert_eq!(odd.meta.get("unknown").unwrap(), &serde_json::json!([1, 2, 3]));

    // The bytes came back, byte for byte.
    assert_eq!(back.assets.len(), 1);
    let asset = back.assets.values().next().unwrap();
    assert_eq!(asset.ext, "jpg");
    assert!(asset.bytes.starts_with(b"\xff\xd8\xff\xe0"));

    assert_eq!(back.board.connections.len(), 1);
    assert_eq!(back.board.tour, vec!["shelf", "note1"]);

    eprintln!("wrote {}", path.display());
}

#[test]
fn the_archive_is_legible_to_anything_that_can_open_a_zip() {
    // Promise 2 of the format, asserted rather than assumed. If this test ever
    // fails because somebody minified the JSON or packed the notes into a blob,
    // that is a format change and not an optimisation.
    let bytes = mbrd::to_bytes(&sample(), "2026-07-25T10:04:11.882Z").unwrap();
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();

    let names: Vec<String> =
        (0..zip.len()).map(|i| zip.by_index(i).unwrap().name().to_string()).collect();

    assert_eq!(names[0], "mimetype", "the media type has to be first");
    assert!(names.contains(&"manifest.json".to_string()));
    assert!(names.contains(&"board.json".to_string()));
    assert!(
        names.iter().any(|n| n.starts_with("notes/") && n.ends_with(".md")),
        "a sticky note must land as readable Markdown, got {names:?}"
    );
    assert!(
        names.iter().any(|n| n.starts_with("assets/") && n.ends_with(".jpg")),
        "an image must land under its own extension, got {names:?}"
    );

    // The note is the author's words, not an encoding of them.
    let note_name = names.iter().find(|n| n.starts_with("notes/")).unwrap().clone();
    let mut note = zip.by_name(&note_name).unwrap();
    let mut text = String::new();
    std::io::Read::read_to_string(&mut note, &mut text).unwrap();
    assert!(text.starts_with("# buy the smaller one"), "got {text:?}");

    // The slug is a courtesy to a person reading the archive, so it has to be
    // legible too — and the hash still has to be in there as the identity.
    assert!(note_name.contains("buy-the-smaller-one"), "got {note_name}");
}

#[test]
fn a_board_carries_its_past_through_a_real_file() {
    // Phase 1's promise, end to end: change a board, write it to a disk, open
    // it in what is for all practical purposes a different process, and take the
    // change back. Nothing below reaches for a private helper — this is the trip
    // a person makes.
    let mut doc = sample();
    let before = schema::serialize(&doc.board);

    doc.board.edit_at("Move", 1_755_180_000_000, |board| {
        let shelf = board.item_mut("shelf").unwrap();
        shelf.x = 900.0;
        shelf.y = -640.0;
    });
    doc.board.edit_at("Rename", 1_755_180_001_000, |board| {
        board.item_mut("note1").unwrap().name = "buy neither".into();
    });

    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target").join("sample");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Kitchen-with-a-past.mbrd");
    std::fs::write(&path, mbrd::to_bytes(&doc, &naming::now_iso8601()).unwrap()).unwrap();

    let mut back = mbrd::read(Cursor::new(std::fs::read(&path).unwrap())).unwrap();

    assert!(!back.board.timeline().stale(), "the ledger describes the board it arrived with");
    assert_eq!(back.board.timeline().steps().len(), 2);
    assert_eq!(back.board.undo_label(), Some("Rename"));

    assert_eq!(back.board.undo().as_deref(), Some("Rename"));
    assert_eq!(back.board.item("note1").unwrap().name, "buy the smaller one");
    assert_eq!(back.board.undo().as_deref(), Some("Move"));
    assert_eq!(back.board.item("shelf").unwrap().x, 160.0);

    assert_eq!(
        schema::serialize(&back.board),
        before,
        "walked all the way back, the board is the one that was written"
    );

    // And forward again, which is the half a linear history usually gets wrong.
    back.board.redo();
    back.board.redo();
    assert_eq!(back.board.item("shelf").unwrap().x, 900.0);
    assert_eq!(back.board.item("note1").unwrap().name, "buy neither");
    assert_eq!(back.board.redo_label(), None);

    eprintln!("wrote {}", path.display());
}

#[test]
fn a_board_saved_rolled_back_comes_back_rolled_back() {
    // `at` is the marker, and it travels. A board saved with its history walked
    // back opens showing the board at that point, with the steps ahead of it
    // still there to walk forward into.
    let mut doc = sample();
    doc.board.edit_at("Move", 1, |board| board.item_mut("shelf").unwrap().x = 900.0);
    doc.board.undo();

    let bytes = mbrd::to_bytes(&doc, &naming::now_iso8601()).unwrap();
    let mut back = mbrd::read(Cursor::new(bytes)).unwrap();

    assert_eq!(back.board.item("shelf").unwrap().x, 160.0, "saved as it was left");
    assert_eq!(back.board.timeline().at(), 0);
    assert_eq!(back.board.redo_label(), Some("Move"), "and the step is still there");
    back.board.redo();
    assert_eq!(back.board.item("shelf").unwrap().x, 900.0);
}

#[test]
fn a_step_that_binned_a_photograph_keeps_its_bytes_in_the_file() {
    // The fourth class of reference. A card goes to the bin and then out of the
    // bin entirely; nothing on the board wants its picture any more, and the
    // step that removed it still does — so the bytes have to be in the archive
    // or stepping back comes back to a hole.
    let mut doc = sample();
    doc.board.edit_at("Empty the bin", 1, |board| {
        board.items.retain(|i| i.id != "shelf");
    });
    assert!(doc.board.required_hashes().is_empty(), "no card names it now");

    let bytes = mbrd::to_bytes(&doc, &naming::now_iso8601()).unwrap();
    let back = mbrd::read(Cursor::new(bytes)).unwrap();
    assert_eq!(back.assets.len(), 1, "the ledger kept the photograph alive");

    let mut back = back;
    back.board.undo();
    let restored = back.board.item("shelf").unwrap();
    let hash = restored.asset.as_ref().unwrap().hash().unwrap();
    assert!(back.assets.contains_key(hash), "and the card that came back can find them");
}
