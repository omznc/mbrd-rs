//! A board to look at before there is one to open.
//!
//! Built through [`mbrd_core::schema::normalize`] rather than by assembling
//! structs by hand, and deliberately so: it means the thing the app shows on a
//! cold start has been through exactly the same door as a file off a disk. A
//! demonstration board built by hand would be the one board in existence that
//! never exercises the reader.
//!
//! Its pictures are drawn here rather than shipped as files, for the same
//! reason: an asset invented at runtime still has to be content-hashed, still
//! has to be found by the hash a card names, and still has to survive a save —
//! so a cold start exercises the whole asset path rather than the half of it
//! that does not need bytes.

use serde_json::json;
use sha2::{Digest, Sha256};

use mbrd_core::mbrd::Asset;
use mbrd_core::{schema, BoardState, Document};

pub fn board() -> Document {
    // The pictures first, because the cards below have to name their hashes.
    let mut assets = std::collections::HashMap::new();
    let mut add = |w: u32, h: u32, hue: f32, label: &str| -> String {
        let bytes = swatch(w, h, hue);
        let hash = format!("{:x}", Sha256::digest(&bytes));
        assets.insert(hash.clone(), Asset { bytes, ext: "png".into(), label: label.into() });
        hash
    };
    let kitchen = add(1200, 800, 26.0, "kitchen-window");
    let shelf = add(900, 1200, 168.0, "shelf");
    let poster = add(1280, 720, 292.0, "walkthrough-poster");
    let sleeve = add(600, 600, 348.0, "reference-sleeve");

    let value = json!({
        "title": "welcome",
        "view": { "pan": { "x": 0, "y": 0 }, "zoom": 1 },
        "settings": { "grid": true, "axes": true, "snap": false, "gridStep": 64 },
        "mediaFit": "cover",
        "items": [
            { "id": "hello", "type": "note", "x": -260, "y": 150, "w": 260, "h": 190, "z": 3,
              "name": "hello",
              "meta": { "text": "# mbrd\n\ndrag empty space to pan, wheel to zoom.\npress n for a note, right-click for the rest.\ndouble-click to type.\nctrl z takes anything back. shift shift for everything else." } },
            { "id": "photo1", "type": "image", "x": 90, "y": 170, "w": 300, "h": 220, "z": 2,
              "name": "kitchen-window.jpg",
              "asset": { "hash": kitchen, "embedded": true } },
            // `contain` against the board's `cover`, so one card on this board
            // is always taking the per-item override path.
            { "id": "photo2", "type": "image", "x": 130, "y": -110, "w": 240, "h": 300, "z": 1,
              "name": "shelf.jpg",
              "asset": { "hash": shelf, "embedded": true },
              "meta": { "fit": "contain" } },
            // A video draws its poster, not its own bytes. There are no bytes
            // here at all, which is the point: the card is complete without them.
            { "id": "clip", "type": "video", "x": -240, "y": -140, "w": 280, "h": 170, "z": 4,
              "name": "walkthrough.mp4",
              "meta": { "cover": poster } },
            { "id": "track", "type": "audio", "x": -250, "y": -360, "w": 300, "h": 96, "z": 5,
              "name": "reference.mp3",
              "meta": { "cover": sleeve } },
            { "id": "warm", "type": "swatch", "x": 470, "y": 220, "w": 130, "h": 130, "z": 8,
              "name": "#C4713A", "meta": { "hex": "#c4713a" } },
            { "id": "cool", "type": "swatch", "x": 470, "y": 76, "w": 130, "h": 130, "z": 8,
              "name": "#3A6EC4", "meta": { "hex": "#3a6ec4" } },
            { "id": "ref", "type": "link", "x": 120, "y": -400, "w": 260, "h": 90, "z": 6,
              "name": "the smaller one",
              "meta": { "url": "https://example.invalid/shelf" } },
            // Turned, so that the tilted bounding box in `geometry` is exercised
            // by looking at the app rather than only by its tests.
            { "id": "tilted", "type": "note", "x": 470, "y": -260, "w": 200, "h": 140, "z": 7,
              "rot": -12, "name": "tilted",
              "meta": { "tint": 3, "text": "cards can be turned. pressing one still lands where it looks like it should." } },
            // A type this build has never heard of, on purpose: it should draw
            // as a plain named card and survive a save untouched. If this one
            // ever disappears from the demo board, the extension point broke.
            { "id": "future", "type": "hologram", "x": 420, "y": 20, "w": 180, "h": 180, "z": 0,
              "name": "from a newer build" },
            // A fence, and nothing anywhere says what is inside it. Membership
            // is measured from where the cards are — see `core::fence` — so
            // dragging this rectangle takes the two photographs with it and
            // dragging a photograph out of it leaves.
            { "id": "pen", "type": "fence", "x": -60, "y": 140, "w": 700, "h": 460, "z": -1,
              "name": "the shelf" },
            // Lying across a photograph, so it is pinned to it: dragging the
            // picture takes the caption along. `core::stick` measures that too.
            { "id": "caption", "type": "note", "x": -170, "y": 60, "w": 180, "h": 90, "z": 9,
              "meta": { "tint": 2, "text": "**stuck** to the picture under it" } }
        ],
        "connections": [
            ["hello", "photo1"],
            ["photo1", "photo2", { "dir": "fwd", "color": "leaf", "label": "same shelf" }]
        ]
    });

    Document { board: BoardState::new(schema::normalize(&value)), assets, ..Document::default() }
}

/// A PNG of a soft two-tone wash, so the cards have something in them.
///
/// Deliberately not a flat colour: a gradient makes it obvious at a glance
/// whether a picture is being drawn at its own shape or stretched to the card,
/// which is the one thing about the fit path that is easy to get wrong and
/// impossible to see in a solid block.
fn swatch(w: u32, h: u32, hue: f32) -> Vec<u8> {
    let img = image::RgbaImage::from_fn(w, h, |x, y| {
        let across = x as f32 / w.max(1) as f32;
        let down = y as f32 / h.max(1) as f32;
        let (r, g, b) = from_hsl(hue + across * 34.0, 0.42, 0.28 + down * 0.34);
        image::Rgba([r, g, b, 255])
    });
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut out, image::ImageFormat::Png)
        .expect("a png of our own making");
    out.into_inner()
}

/// Hue in degrees, saturation and lightness in `0..=1`, to eight-bit RGB.
fn from_hsl(hue: f32, sat: f32, light: f32) -> (u8, u8, u8) {
    let hue = hue.rem_euclid(360.0) / 60.0;
    let chroma = (1.0 - (2.0 * light - 1.0).abs()) * sat;
    let second = chroma * (1.0 - (hue % 2.0 - 1.0).abs());
    let (r, g, b) = match hue as u32 {
        0 => (chroma, second, 0.0),
        1 => (second, chroma, 0.0),
        2 => (0.0, chroma, second),
        3 => (0.0, second, chroma),
        4 => (second, 0.0, chroma),
        _ => (chroma, 0.0, second),
    };
    let m = light - chroma / 2.0;
    let to_byte = |v: f32| ((v + m).clamp(0.0, 1.0) * 255.0).round() as u8;
    (to_byte(r), to_byte(g), to_byte(b))
}

/// Cards name assets by hash, and a hash that names nothing is a card that
/// never draws. Worth a test, because the two halves are written apart.
#[cfg(test)]
mod tests {
    use super::*;
    use mbrd_core::model::ItemAsset;
    use serde_json::Value;

    #[test]
    fn every_picture_the_demo_board_names_is_a_picture_it_carries() {
        let doc = board();
        let mut named = 0;
        for item in &doc.board.items {
            let hashes = [
                item.asset.as_ref().and_then(ItemAsset::hash),
                item.meta.get("cover").and_then(Value::as_str),
            ];
            for hash in hashes.into_iter().flatten() {
                named += 1;
                let asset = doc.assets.get(hash).unwrap_or_else(|| {
                    panic!("{} names {hash}, which is not in the archive", item.id)
                });
                assert!(
                    crate::images::decode(&asset.bytes).is_some(),
                    "{} names bytes that are not a picture",
                    item.id
                );
            }
        }
        assert_eq!(named, 4, "the demo board should be exercising the asset path");
    }
}
