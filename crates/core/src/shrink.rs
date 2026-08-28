//! Trimming a board down to what a board actually needs.
//!
//! The half with no decoder in it: which files are worth re-encoding, what a
//! run would touch, and what to say about it afterwards. The encoding itself is
//! the app's — see `app/shrink.rs`, which owns the one dependency that can read
//! a photograph — and this is everything around it, which is the part worth
//! testing without a window.
//!
//! ## Strictly asked for
//!
//! Nothing here runs on import, on save, or on a timer. It is a page you open,
//! it says what it is about to do before it does any of it, and it is one
//! `Ctrl Z` away from being undone. That is the whole shape of the feature and
//! the reason it is allowed to be lossy at all: a moodboard is a thing you look
//! at, and a 6000-pixel photograph on a card drawn at 300 is weight the board
//! carries for nothing.
//!
//! ## What makes the undo real rather than nominal
//!
//! **The old bytes are not deleted.** A shrink puts the new file in the asset
//! store under the hash of its own contents and repoints the cards at it,
//! through the ledger, in one step. The original stays exactly where it was —
//! which is what the undo lands back on, and is the same rule
//! `BoardView::write_file` already follows for a note written back to its file.
//!
//! It also means a shrink does not make the *file on disk* smaller until the
//! board is saved: the save writes the assets the board can still reach — see
//! `save::write` — and by then the ledger has been trimmed by its own cap.
//! Saying so is [`Report`]'s job, not a footnote.
//!
//! ## What it will not touch
//!
//! - Anything it cannot make meaningfully smaller. A re-encode that saves four
//!   percent has still spent a generation of quality on a rounding error — see
//!   [`worth_it`].
//! - Anything that moves. An animated GIF or WebP re-encoded through a still
//!   decoder comes back as one frame, and a picture that has stopped moving is
//!   not an optimisation.
//! - Anything a card is not actually using. An orphan is on its way out of the
//!   file already, and re-encoding rubbish is work spent to keep rubbish.
//! - Sound and video. Both want an encoder this build does not have yet; see
//!   the roadmap's Phase 7, which brings one.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::mbrd::Document;
use crate::model::{Board, Item, ItemAsset};

/// The long edge a picture is allowed to keep.
///
/// A card is rarely drawn much past 600 world units and a screen is rarely
/// worth more than two device pixels per unit, so this is the size at which the
/// picture stops being what limits what you can see. Above it you are storing a
/// photograph library rather than a board.
pub const LONG_EDGE: u32 = 1200;

/// The ceiling for a picture that is drawn *inside* something — a video's
/// poster, a track's album art. They are kept, never dropped; they are simply
/// never drawn at the size of the card itself.
pub const COVER_EDGE: u32 = 600;

/// JPEG quality. High enough that a photograph does not band.
pub const QUALITY: u8 = 82;

/// The size below which a picture is not worth touching at all.
///
/// A tenth of a megabyte. A file this small is not why a board is heavy — a
/// hundred of them are a tenth of one photograph — and the two ways it can go
/// wrong are both real: a small PNG of flat colour or text is exactly what JPEG
/// is worst at, and every re-encode spends a generation of quality. The
/// threshold is on the *stored* size rather than on the pixels because that is
/// the number the saving is measured in.
pub const SMALL_ENOUGH: usize = 100 * 1024;

/// How much smaller the new file has to be before the swap is worth making.
///
/// A tenth. Below it the original is kept exactly as it arrived: a re-encode
/// that saves less has still thrown away a generation of quality, and doing
/// that to every picture on a board to save a rounding error is a bad trade.
pub const WORTH_IT: f32 = 0.1;

/// Whether a re-encode this size is worth swapping in.
///
/// `false` for a new file that came out bigger, which is common — a photograph
/// already saved at a sensible quality re-encodes larger as often as not — and
/// is not a failure. It is the answer "leave this one alone".
pub fn worth_it(before: usize, after: usize) -> bool {
    if before == 0 || after == 0 {
        return false;
    }
    after < before && (before - after) as f32 / before as f32 >= WORTH_IT
}

/// The extensions this build is willing to re-encode.
///
/// A list rather than "everything the decoder can read", and the difference is
/// the whole judgement: `gif` is decodable and is not here, because a still
/// frame of an animation is a worse board than a large one. `svg` is text and
/// already small, and rasterising it would throw away the one thing it has.
/// `heic`, `avif` and the camera raws are not here because this build has no
/// decoder for them at all, and a file it cannot read is one it must not claim
/// it could have shrunk.
///
/// `webp` is here even though it may animate: the extension cannot say which,
/// so the refusal is made against the bytes at the point of decoding — see
/// `app/shrink.rs`.
pub fn shrinkable(ext: &str) -> bool {
    matches!(
        ext.trim_start_matches('.').to_ascii_lowercase().as_str(),
        "jpg" | "jpeg" | "jpe" | "png" | "bmp" | "tif" | "tiff" | "tga" | "webp"
    )
}

/// One file a run would try.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub hash: String,
    /// What the card called it. Never an identity — see [`crate::mbrd::Asset`].
    pub label: String,
    pub ext: String,
    pub bytes: usize,
    /// The long edge this one is allowed to keep: [`LONG_EDGE`], or
    /// [`COVER_EDGE`] for a file nothing uses except as a poster or a sleeve.
    ///
    /// The *widest* claim wins. One picture can be a card's own file and
    /// another card's cover at the same time, and shrinking it to the cover's
    /// ceiling would quietly halve the card that shows it full size.
    pub edge: u32,
}

/// What a run would do, worked out before any of it is done.
///
/// This is the thing the page shows and the person agrees to. It carries no
/// promise about the saving, because nothing knows what a picture will weigh
/// until it has been encoded — see [`Report`], which is the after.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Plan {
    /// Heaviest first, so a run that is stopped half way has done the most
    /// good it could have in the time.
    pub jobs: Vec<Job>,
    /// What the jobs weigh now.
    pub bytes: usize,
    /// Files left out of the plan entirely — the animations, the formats with
    /// no decoder, the sound and the video. Reported as a count so the page can
    /// say "and 4 left alone" rather than implying it looked at everything.
    pub left: usize,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }
}

/// What a run actually did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Report {
    /// Files swapped. The rest were tried and left alone, which is an answer
    /// rather than a failure — see [`worth_it`].
    pub changed: usize,
    /// What those files weighed, before and after. Only the ones that changed:
    /// counting the untouched into both halves would bury the saving under the
    /// board's own weight.
    pub before: usize,
    pub after: usize,
}

impl Report {
    /// How much lighter the board's pictures are, in bytes.
    pub fn saved(&self) -> usize {
        self.before.saturating_sub(self.after)
    }
}

/// Every picture a run would try, heaviest first.
///
/// Pure, and cheap: it is one pass over the items and one over the asset map,
/// with no decoding anywhere in it. That matters for the same reason it matters
/// in [`crate::inventory`] — this is asked *about* a heavy board, so it must not
/// be the reason the board feels heavy.
pub fn plan(doc: &Document) -> Plan {
    // Which hashes a live card names, and how. A cover-only file gets the lower
    // ceiling; anything named as a card's own file gets the full one.
    let mut wanted: HashMap<&str, u32> = HashMap::new();
    for item in &doc.board.items {
        if let Some(hash) = item.asset.as_ref().and_then(ItemAsset::hash) {
            wanted.insert(hash, LONG_EDGE);
        }
        if let Some(cover) = item.meta.get("cover").and_then(Value::as_str) {
            // `or_insert` and then a max, rather than a plain insert: the two
            // roles can name the same file, and the order the items happen to
            // be stored in must not decide how big it is allowed to be.
            let edge = wanted.entry(cover).or_insert(COVER_EDGE);
            *edge = (*edge).max(COVER_EDGE);
        }
    }

    let mut jobs = Vec::new();
    let mut bytes = 0;
    let mut left = 0;
    for (hash, asset) in &doc.assets {
        let Some(&edge) = wanted.get(hash.as_str()) else {
            // Not on the board. Orphans and the bin's own files are both here,
            // and neither is counted as "left alone": nothing was considering
            // them. See the module note.
            continue;
        };
        if !shrinkable(&asset.ext) || asset.bytes.len() < SMALL_ENOUGH {
            left += 1;
            continue;
        }
        bytes += asset.bytes.len();
        jobs.push(Job {
            hash: hash.clone(),
            label: asset.label.clone(),
            ext: asset.ext.clone(),
            bytes: asset.bytes.len(),
            edge,
        });
    }
    // Heaviest first, then by hash — the same tie-break `inventory::of` makes,
    // and for the same reason: two opens of an unchanged board must produce the
    // same list rather than the order a map happened to hand them over in.
    jobs.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.hash.cmp(&b.hash)));
    Plan { jobs, bytes, left }
}

/// Point every card that names `from` at `to` instead, and answer how many
/// were changed.
///
/// Both places a hash can be named: the card's own file and `meta.cover`. A
/// swap that missed the second would leave a video wearing a poster nothing
/// else in the board still refers to.
///
/// Takes a `Board` rather than a `Document` because it is called from inside
/// `BoardState::edit` — the one door — and that is what a closure there is
/// handed. The new bytes are put in the asset store by the caller, before the
/// edit opens: an asset store is not board state and has no place in a step.
pub fn swap(board: &mut Board, from: &str, to: &str) -> usize {
    let mut changed = 0;
    for item in &mut board.items {
        if item.asset.as_ref().and_then(ItemAsset::hash) == Some(from) {
            // The `family` the original wrote alongside the hash is dropped
            // rather than carried: it names the format the *old* bytes were,
            // and a JPEG carrying a PNG's catalogue entry would be a lie that
            // outlives the picture. `preview` reads the extension, which is
            // right either way.
            item.asset = Some(ItemAsset::Embedded { hash: to.to_string(), family: None });
            changed += 1;
        }
        if item.meta.get("cover").and_then(Value::as_str) == Some(from) {
            item.meta.insert("cover".into(), Value::String(to.to_string()));
            changed += 1;
        }
    }
    changed
}

/// Every hash a run must not touch, for a caller that wants to check one.
///
/// Not used by [`plan`], which asks the question the other way round. It is
/// here for the tests and for anything that has a hash in hand and wants to
/// know whether it is somebody's picture.
pub fn on_board(board: &Board) -> HashSet<String> {
    let mut out = HashSet::new();
    for item in &board.items {
        for hash in named(item) {
            out.insert(hash.to_string());
        }
    }
    out
}

fn named(item: &Item) -> Vec<&str> {
    let mut out = Vec::new();
    if let Some(hash) = item.asset.as_ref().and_then(ItemAsset::hash) {
        out.push(hash);
    }
    if let Some(cover) = item.meta.get("cover").and_then(Value::as_str) {
        if !out.contains(&cover) {
            out.push(cover);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mbrd::Asset;
    use crate::model::ItemType;
    use crate::state::BoardState;

    fn doc_of(items: Vec<Item>, assets: &[(&str, &str, usize)]) -> Document {
        let board = Board { items, ..Default::default() };
        Document {
            manifest: Default::default(),
            board: BoardState::new(board),
            assets: assets
                .iter()
                .map(|(hash, ext, bytes)| {
                    (
                        (*hash).to_string(),
                        Asset {
                            bytes: vec![0; *bytes],
                            ext: (*ext).to_string(),
                            label: "photo".into(),
                        },
                    )
                })
                .collect(),
            waveforms: Default::default(),
        }
    }

    /// A round number comfortably over [`SMALL_ENOUGH`], so a test about
    /// ordering is not quietly also a test about the floor.
    const MB: usize = 1024 * 1024;

    fn card(id: &str, hash: &str) -> Item {
        let mut item = Item::new(id.to_string(), ItemType::Image);
        item.asset = Some(ItemAsset::Embedded { hash: hash.to_string(), family: None });
        item
    }

    #[test]
    fn a_saving_under_a_tenth_is_not_worth_a_generation_of_quality() {
        assert!(worth_it(1000, 500));
        assert!(worth_it(1000, 900), "exactly a tenth counts");
        assert!(!worth_it(1000, 901));
        assert!(!worth_it(1000, 1200), "bigger is never a saving");
        assert!(!worth_it(0, 0));
    }

    #[test]
    fn the_formats_that_move_are_not_offered() {
        assert!(shrinkable("jpg"));
        assert!(shrinkable("JPEG"), "case is not a format");
        assert!(shrinkable(".png"), "nor is a leading dot");
        assert!(!shrinkable("gif"), "it would come back as one frame");
        assert!(!shrinkable("svg"));
        assert!(!shrinkable("heic"), "nothing here can read one");
        assert!(!shrinkable("mp3"));
    }

    /// The heaviest first, so a run stopped half way has done the most good.
    #[test]
    fn the_plan_is_ordered_by_what_it_would_save() {
        let doc = doc_of(
            vec![card("a", "one"), card("b", "two")],
            &[("one", "jpg", MB), ("two", "jpg", MB * 9)],
        );
        let plan = plan(&doc);
        assert_eq!(plan.jobs.iter().map(|j| j.hash.as_str()).collect::<Vec<_>>(), ["two", "one"]);
        assert_eq!(plan.bytes, MB * 10);
    }

    /// Re-encoding rubbish is work spent to keep rubbish.
    #[test]
    fn a_file_no_card_is_using_is_not_in_the_plan() {
        let doc = doc_of(vec![card("a", "one")], &[("one", "jpg", MB), ("gone", "jpg", MB * 9)]);
        let plan = plan(&doc);
        assert_eq!(plan.jobs.len(), 1);
        assert_eq!(plan.jobs[0].hash, "one");
        assert_eq!(plan.left, 0, "an orphan was never being considered");
    }

    /// A hundred small files are a tenth of one photograph, and JPEG is at its
    /// worst on exactly the small flat pictures this spares.
    #[test]
    fn a_file_too_small_to_be_the_problem_is_left_alone() {
        let doc = doc_of(vec![card("a", "one")], &[("one", "png", 4096)]);
        let plan = plan(&doc);
        assert!(plan.is_empty());
        assert_eq!(plan.left, 1);
    }

    #[test]
    fn a_format_that_cannot_be_read_is_counted_as_left_alone() {
        let doc = doc_of(
            vec![card("a", "one"), card("b", "two")],
            &[("one", "jpg", MB), ("two", "gif", MB * 9)],
        );
        let plan = plan(&doc);
        assert_eq!(plan.jobs.len(), 1);
        assert_eq!(plan.left, 1);
    }

    /// A picture that is somebody's card *and* somebody else's poster is
    /// allowed the bigger of the two ceilings.
    #[test]
    fn the_widest_claim_on_a_picture_decides_how_big_it_may_stay() {
        let mut poster = Item::new("b".to_string(), ItemType::Video);
        poster.meta.insert("cover".into(), Value::String("one".into()));
        let doc = doc_of(vec![card("a", "one"), poster], &[("one", "jpg", MB)]);
        assert_eq!(plan(&doc).jobs[0].edge, LONG_EDGE);

        let mut only_poster = Item::new("b".to_string(), ItemType::Video);
        only_poster.meta.insert("cover".into(), Value::String("one".into()));
        let doc = doc_of(vec![only_poster], &[("one", "jpg", MB)]);
        assert_eq!(plan(&doc).jobs[0].edge, COVER_EDGE);
    }

    #[test]
    fn a_swap_follows_a_hash_into_both_places_it_can_be_named() {
        let mut poster = Item::new("b".to_string(), ItemType::Video);
        poster.meta.insert("cover".into(), Value::String("one".into()));
        let mut board = Board {
            items: vec![card("a", "one"), poster, card("c", "other")],
            ..Default::default()
        };

        assert_eq!(swap(&mut board, "one", "small"), 2);
        assert_eq!(board.items[0].asset.as_ref().and_then(ItemAsset::hash), Some("small"));
        assert_eq!(board.items[1].meta.get("cover").and_then(Value::as_str), Some("small"));
        assert_eq!(
            board.items[2].asset.as_ref().and_then(ItemAsset::hash),
            Some("other"),
            "a card naming something else was not touched"
        );
    }

    #[test]
    fn a_report_saves_the_difference_and_never_a_negative_one() {
        let report = Report { changed: 2, before: 1000, after: 400 };
        assert_eq!(report.saved(), 600);
        let odd = Report { changed: 1, before: 100, after: 400 };
        assert_eq!(odd.saved(), 0);
    }

    #[test]
    fn on_board_names_both_halves_of_a_card() {
        let mut poster = Item::new("b".to_string(), ItemType::Video);
        poster.meta.insert("cover".into(), Value::String("sleeve".into()));
        poster.asset = Some(ItemAsset::Embedded { hash: "clip".into(), family: None });
        let board = Board { items: vec![poster], ..Default::default() };
        let held = on_board(&board);
        assert!(held.contains("clip"));
        assert!(held.contains("sleeve"));
    }
}
