//! What is truthfully known about a card, as a list somebody can read.
//!
//! The information rail on the open page — see `opened.rs` — and nothing else.
//! It is one list for every type, which is the whole point of it: the button
//! that opens it is in the same place on a photograph, a zip and a note, and
//! what comes out is longer for some of them than for others.
//!
//! ## Absent rather than unknown
//!
//! A fact that is not known is **not in the list**. There is no "Duration: —"
//! row, no empty "Artist", no "Dimensions: unknown". A rail of blanks reads as
//! a form somebody failed to fill in, and it buries the four rows that do say
//! something under nine that do not. Every `push` below is behind the question
//! "is there an answer", and that is why the list is built rather than
//! declared.
//!
//! ## Measured, not decoded
//!
//! Everything here comes off the item, its `meta`, or the length of its bytes.
//! Nothing is opened. A picture's dimensions appear only if something that
//! *did* open it wrote them down — which is what import does — so this stays
//! cheap enough to call while the page is being drawn.

use crate::mbrd::Asset;
use crate::media;
use crate::model::{Item, ItemAsset, ItemType};
use crate::preview;

/// One row of the rail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    pub name: &'static str,
    pub value: String,
    /// Whether the value is a string of characters rather than a phrase — a
    /// hash, a hex colour, a pixel count. Set in a fixed-width face, where the
    /// digits line up and a `0` cannot be read as an `O`.
    pub mono: bool,
}

impl Fact {
    fn said(name: &'static str, value: impl Into<String>) -> Fact {
        Fact { name, value: value.into(), mono: false }
    }

    fn counted(name: &'static str, value: impl Into<String>) -> Fact {
        Fact { name, value: value.into(), mono: true }
    }
}

/// Everything worth saying about this card.
///
/// Ordered the way somebody reads it: what the thing is, then what is in it,
/// then how big it is on the board, and the hash last — it is the row nobody
/// is looking for and the one that is occasionally the only thing that helps.
pub fn of(item: &Item, asset: Option<&Asset>) -> Vec<Fact> {
    let mut out = Vec::new();

    out.push(Fact::said("Type", kind_of(item, asset)));
    if !item.name.trim().is_empty() {
        out.push(Fact::said("Name", item.name.clone()));
    }
    if let Some(url) = item.url() {
        out.push(Fact::said("Address", url.to_string()));
    }
    if item.kind == ItemType::Swatch {
        let hex = item.meta.get("hex").and_then(serde_json::Value::as_str).unwrap_or(&item.name);
        out.push(Fact::counted("Hex", hex.to_uppercase()));
    }

    // What somebody who opened the file would have found in it.
    for (name, key) in [("Title", "title"), ("Artist", "artist"), ("Album", "album")] {
        if let Some(value) = media::tag(item, key) {
            out.push(Fact::said(name, value.to_string()));
        }
    }
    if let Some(seconds) = media::duration(item) {
        out.push(Fact::counted("Length", clock(seconds)));
    }
    if let Some((w, h)) = pixels(item) {
        out.push(Fact::counted("Pixels", format!("{w} × {h}")));
    }
    if let Some(pages) = pages(item) {
        out.push(Fact::counted("Pages", count(pages as usize)));
    }
    if let Some(family) = item.meta.get("family").and_then(serde_json::Value::as_str) {
        out.push(Fact::said("Family", family.to_string()));
    }
    if let Some(triangles) = item.meta.get("triangles").and_then(serde_json::Value::as_u64) {
        out.push(Fact::counted("Triangles", count(triangles as usize)));
    }

    if let Some(asset) = asset {
        // Words before bytes: for the cards this build can read, how much text
        // there is answers more questions than how many kilobytes it took.
        if let Ok(text) = std::str::from_utf8(&asset.bytes) {
            if preview::readable_text(&asset.bytes) {
                out.push(Fact::counted("Lines", count(text.lines().count())));
                out.push(Fact::counted("Characters", count(text.chars().count())));
            }
        }
        out.push(Fact::counted("Size", size(asset.bytes.len())));
    }

    out.push(Fact::counted("Shape", format!("{:.0} × {:.0}", item.w, item.h)));
    if item.rot.abs() >= 0.5 {
        out.push(Fact::counted("Turned", format!("{:.0}°", item.rot)));
    }

    match item.asset.as_ref() {
        Some(ItemAsset::Embedded { hash, .. }) => out.push(Fact::counted("Content hash", hash)),
        // The reserved link-instead-of-embed form, which nothing in this build
        // writes and everything in it carries through. Saying so beats a rail
        // that quietly looks like a card with no file at all.
        Some(ItemAsset::External(_)) => out.push(Fact::said("Bytes", "held elsewhere")),
        None => {}
    }
    out
}

/// What to call this card in one phrase.
///
/// The card's own type where the file adds nothing — "image" over a `.png` is
/// two words for one fact — and the file's where it is more specific than the
/// type is: a `generic` card holding a `.rs` is a Rust file, and saying
/// "generic" would be throwing away the interesting half.
fn kind_of(item: &Item, asset: Option<&Asset>) -> String {
    let kind = item.kind.as_str();
    let Some(asset) = asset else { return kind.to_string() };
    let ext = asset.ext.to_ascii_lowercase();
    if ext.is_empty() {
        return kind.to_string();
    }
    match preview::language(&ext) {
        Some(language) => format!("{language} · {}", ext.to_uppercase()),
        None => format!("{kind} · {}", ext.to_uppercase()),
    }
}

/// The picture's own size, where something that decoded it wrote it down.
fn pixels(item: &Item) -> Option<(u64, u64)> {
    let read =
        |key: &str| item.meta.get(key).and_then(serde_json::Value::as_u64).filter(|n| *n > 0);
    Some((read("naturalWidth")?, read("naturalHeight")?))
}

/// A PDF's page count, where import could parse it.
fn pages(item: &Item) -> Option<u64> {
    item.meta.get("pages").and_then(serde_json::Value::as_u64).filter(|n| *n > 0)
}

/// Bytes, said the way a file manager says them.
pub fn size(bytes: usize) -> String {
    const STEP: f32 = 1024.0;
    let scale = bytes as f32;
    for (n, unit) in ["bytes", "KB", "MB", "GB"].iter().enumerate() {
        let scaled = scale / STEP.powi(n as i32);
        if scaled < STEP || n == 3 {
            return if n == 0 { format!("{bytes} {unit}") } else { format!("{scaled:.1} {unit}") };
        }
    }
    unreachable!("the last arm returns")
}

/// Seconds as a clock reads them. An hour is where the third field appears,
/// rather than a permanent `0:` nothing needs.
pub fn clock(seconds: f32) -> String {
    let whole = seconds.max(0.0).round() as u64;
    let (h, m, s) = (whole / 3600, (whole / 60) % 60, whole % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// A count with thousands separated, because six digits in a row is not a
/// number anybody reads — it is a number they count the digits of.
pub fn count(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn asset(ext: &str, bytes: &[u8]) -> Asset {
        Asset { bytes: bytes.to_vec(), ext: ext.into(), label: "file".into() }
    }

    fn named(facts: &[Fact], name: &str) -> Option<String> {
        facts.iter().find(|fact| fact.name == name).map(|fact| fact.value.clone())
    }

    #[test]
    fn a_size_is_said_the_way_a_file_manager_says_it() {
        assert_eq!(size(512), "512 bytes");
        assert_eq!(size(2048), "2.0 KB");
        assert_eq!(size(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn a_clock_grows_a_field_only_when_it_needs_one() {
        assert_eq!(clock(9.0), "0:09");
        assert_eq!(clock(75.4), "1:15");
        assert_eq!(clock(3661.0), "1:01:01");
        assert_eq!(clock(-3.0), "0:00", "a nonsense duration is not a negative clock");
    }

    #[test]
    fn a_long_count_is_grouped() {
        assert_eq!(count(9), "9");
        assert_eq!(count(1_000), "1,000");
        assert_eq!(count(1_234_567), "1,234,567");
    }

    #[test]
    fn a_fact_nobody_knows_is_not_a_row() {
        // The rule this module is built around: no blanks.
        let facts = of(&Item::new("a", ItemType::Image), None);
        assert!(named(&facts, "Length").is_none());
        assert!(named(&facts, "Artist").is_none());
        assert!(named(&facts, "Name").is_none(), "an unnamed card has no Name row");
        assert!(named(&facts, "Size").is_none(), "and no Size without bytes");
    }

    #[test]
    fn a_file_says_what_it_is_rather_than_what_the_card_is() {
        let mut item = Item::new("a", ItemType::Generic);
        item.name = "main.rs".into();
        let facts = of(&item, Some(&asset("rs", b"fn main() {}\n")));
        assert_eq!(named(&facts, "Type").as_deref(), Some("Rust · RS"));
        assert_eq!(named(&facts, "Lines").as_deref(), Some("1"));
        assert_eq!(named(&facts, "Characters").as_deref(), Some("13"));
    }

    #[test]
    fn bytes_nobody_can_read_are_not_counted_in_lines() {
        let facts = of(&Item::new("a", ItemType::Generic), Some(&asset("bin", b"\0\0\x01\x02")));
        assert!(named(&facts, "Lines").is_none());
        assert_eq!(named(&facts, "Size").as_deref(), Some("4 bytes"));
    }

    #[test]
    fn a_recording_says_how_long_it_is_and_who_made_it() {
        let mut item = Item::new("a", ItemType::Audio);
        media::set_duration(&mut item, 195.0);
        media::set_tag(&mut item, "artist", "Someone");
        let facts = of(&item, None);
        assert_eq!(named(&facts, "Length").as_deref(), Some("3:15"));
        assert_eq!(named(&facts, "Artist").as_deref(), Some("Someone"));
    }

    #[test]
    fn a_picture_says_its_pixels_only_once_something_has_measured_them() {
        let mut item = Item::new("a", ItemType::Image);
        assert!(named(&of(&item, None), "Pixels").is_none());
        item.meta.insert("naturalWidth".into(), Value::from(1920));
        item.meta.insert("naturalHeight".into(), Value::from(1080));
        assert_eq!(named(&of(&item, None), "Pixels").as_deref(), Some("1920 × 1080"));
    }

    #[test]
    fn a_pdf_says_its_page_count_only_once_import_has_measured_it() {
        let mut item = Item::new("a", ItemType::Generic);
        assert!(named(&of(&item, None), "Pages").is_none());
        item.meta.insert("pages".into(), Value::from(12));
        assert_eq!(named(&of(&item, None), "Pages").as_deref(), Some("12"));
    }

    #[test]
    fn a_font_says_its_own_family_name_once_something_has_read_it() {
        let mut item = Item::new("a", ItemType::Generic);
        assert!(named(&of(&item, None), "Family").is_none());
        item.meta.insert("family".into(), Value::from("Fraunces"));
        assert_eq!(named(&of(&item, None), "Family").as_deref(), Some("Fraunces"));
    }

    #[test]
    fn a_mesh_says_its_triangle_count_once_import_has_read_it() {
        let mut item = Item::new("a", ItemType::Model);
        assert!(named(&of(&item, None), "Triangles").is_none());
        item.meta.insert("triangles".into(), Value::from(12));
        assert_eq!(named(&of(&item, None), "Triangles").as_deref(), Some("12"));
    }

    #[test]
    fn a_swatch_says_its_colour() {
        let mut item = Item::new("a", ItemType::Swatch);
        item.meta.insert("hex".into(), Value::from("#ff8800"));
        assert_eq!(named(&of(&item, None), "Hex").as_deref(), Some("#FF8800"));
    }

    #[test]
    fn the_hash_is_the_last_thing_said() {
        let mut item = Item::new("a", ItemType::Image);
        item.asset = Some(ItemAsset::Embedded { hash: "abc123".into(), family: None });
        let facts = of(&item, None);
        assert_eq!(facts.last().map(|fact| fact.name), Some("Content hash"));
        assert_eq!(facts.last().map(|fact| fact.value.as_str()), Some("abc123"));
    }
}
