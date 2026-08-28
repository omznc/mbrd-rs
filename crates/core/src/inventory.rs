//! What this board is made of, and what it weighs.
//!
//! A `.mbrd` gets heavy and nothing else in the app says why. There has been a
//! way to make one smaller for a while — see the roadmap's Phase 10 — and it is
//! a thing you have to already know about: nowhere gives a *reason* to press
//! it. **This photograph is 12 MB** is that reason, which is why the two belong
//! together: this reports, and hands off to the shrink.
//!
//! ## A report, not a setting
//!
//! Which is why it is a page of its own rather than a section of `settings.rs`.
//! Every row on that page is a control with a value; nothing here has a value.
//! It is the board described back to you.
//!
//! ## Two rules it may not break
//!
//! **Sizes come from the stored bytes, never from re-reading an original.**
//! An `Asset` carries its own `bytes`, so the whole report is arithmetic over
//! maps that are already in memory.
//!
//! **Building the report may not decode a picture.** That is the load-bearing
//! one. A board with two thousand cards is exactly the board somebody opens
//! this on, and a page that measured pictures by decoding them would stall the
//! window at the precise moment the question was being asked — turning the tool
//! for diagnosing a heavy board into another reason it feels heavy. So there
//! are no thumbnails here: a row is a name, a kind and a number, and it costs
//! one pass over a map however heavy the board is.
//!
//! ## A row is a way to the card
//!
//! The rows are **files, by content hash**, and cards are what somebody can
//! actually go and look at — one file can be under several cards, and an orphan
//! is under none. So a row carries the ids using its hash, and a row with a
//! card behind it is somewhere to travel to. It says how many when there is
//! more than one, because a file under three cards is also a file that deleting
//! one card frees nothing of.
//!
//! ## "Unreferenced" has three meanings, and they all count
//!
//! An asset is an orphan when nothing on the live board, nothing in the bin,
//! and **no step of the history** names it. The third is the one that is easy
//! to forget: a step names cards the board no longer has, which is the whole of
//! what a step is for, so an asset only a step wants is still live.
//!
//! The orphan list is **reported and not offered for deletion**. The union has
//! three members and has grown twice; a "remove unused" button written against
//! it would become a data-loss bug the next time it grows. It stays a report.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use crate::mbrd::Document;
use crate::model::{Item, ItemAsset, ItemType};

/// One stored file, as this page talks about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Weighed {
    pub hash: String,
    /// What the card called it. Never an identity — see [`crate::mbrd::Asset`].
    pub label: String,
    pub ext: String,
    pub bytes: usize,
    /// Nothing on the board, in the bin, or in the history names this one.
    pub orphan: bool,
    /// The **live** cards using this file, in board order.
    ///
    /// Live only: a card in the bin keeps its file off the orphan list, which
    /// is the whole point of a bin, but there is nowhere to fly to for one. Nor
    /// is a step of the history a place — it is a moment. So this is the
    /// narrower of the two questions asked about a hash, and both are asked in
    /// the same pass.
    pub cards: Vec<String>,
}

/// What nothing points at any more.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Orphans {
    pub count: usize,
    pub bytes: usize,
}

/// The whole report.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Inventory {
    /// How many cards of each kind, the biggest count first and alphabetical
    /// within a count.
    pub kinds: Vec<(ItemType, usize)>,
    pub items: usize,
    pub binned: usize,
    pub connections: usize,
    /// Every stored file, and what they add up to.
    pub assets: usize,
    pub bytes: usize,
    /// The measured audio kept beside the recordings, which is a real part of
    /// what a board weighs and is invisible everywhere else.
    pub waveforms: usize,
    /// Every stored file, heaviest first.
    ///
    /// All of them rather than a top ten, because the sheet that draws this
    /// filters and sorts it — and a list that can be sorted by name while only
    /// holding the ten biggest would answer a question nobody asked. The cost
    /// is one small struct per file on a walk that was already happening.
    pub files: Vec<Weighed>,
    /// How many card-to-file references there are, across every file.
    ///
    /// Larger than [`Self::assets`] on any board where two cards show the same
    /// photograph, and the gap between the two numbers is the whole of what
    /// [`Self::shared`] is about.
    pub uses: usize,
    /// What storing each file once instead of once per card saved.
    ///
    /// Every card past the first that names a file is a copy this archive did
    /// not have to keep, so this is the sum of those copies' sizes. It is a
    /// saving that has already happened — not an offer — which is why it sits
    /// in the summary beside what the board weighs rather than beside the
    /// button that would make it weigh less.
    pub shared: usize,
    pub orphans: Orphans,
    /// How many steps of history the file is carrying.
    ///
    /// Not bytes: a step is stored as the text of what changed, and turning
    /// that into a number here would mean serialising the whole ledger to
    /// answer a question about its size. A count is the honest cheap answer,
    /// and it is the one that explains a heavy board with no pictures on it.
    pub steps: usize,
}

/// Take stock, as arithmetic over what is already in memory.
///
/// Pure, and that is the point of it living here: a report about a board is
/// testable without a window, and the page in the app only has to draw what
/// this returns.
pub fn of(doc: &Document) -> Inventory {
    // The live half of the reference union is walked into a map rather than a
    // set, because a row needs to know *which* card and not only that there is
    // one. Same walk, one more line, and no second pass over the items.
    let mut users: HashMap<&str, Vec<String>> = HashMap::new();
    let mut referenced: HashSet<String> = HashSet::new();
    for item in &doc.board.items {
        for hash in hashes_of(item) {
            referenced.insert(hash.to_string());
            users.entry(hash).or_default().push(item.id.clone());
        }
    }
    // The bin. A card in it still owns its bytes, because the whole point of a
    // bin is that the card can come back — even though in this build the bin
    // does not reach the file. See `TrashEntry`.
    for entry in &doc.board.trash {
        for hash in hashes_of(&entry.item) {
            referenced.insert(hash.to_string());
        }
    }
    // And the third. Asked of the ledger, which owns the shape of a step,
    // rather than walked here: a second copy of that walk is exactly how the
    // two would come to disagree about what is rubbish.
    referenced.extend(doc.board.optional_hashes());

    // A list with a linear find rather than a map, because `ItemType` is
    // neither `Hash` nor `Ord` — deliberately, since `Other` carries a string a
    // later build invented — and a board has at most a dozen kinds on it.
    let mut kinds: Vec<(ItemType, usize)> = Vec::new();
    for item in &doc.board.items {
        match kinds.iter_mut().find(|(kind, _)| *kind == item.kind) {
            Some((_, count)) => *count += 1,
            None => kinds.push((item.kind.clone(), 1)),
        }
    }
    // Biggest count first, and by name within a count — so two opens of an
    // unchanged board produce the same list rather than the order the items
    // happened to be stored in.
    kinds.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.as_str().cmp(b.0.as_str())));

    let mut all: Vec<Weighed> = Vec::with_capacity(doc.assets.len());
    let mut bytes = 0;
    let mut orphans = Orphans::default();
    for (hash, asset) in &doc.assets {
        let orphan = !referenced.contains(hash);
        bytes += asset.bytes.len();
        if orphan {
            orphans.count += 1;
            orphans.bytes += asset.bytes.len();
        }
        all.push(Weighed {
            hash: hash.clone(),
            label: asset.label.clone(),
            ext: asset.ext.clone(),
            bytes: asset.bytes.len(),
            orphan,
            cards: users.get(hash.as_str()).cloned().unwrap_or_default(),
        });
    }
    // By weight, then by hash — two files of identical size would otherwise
    // swap places on the whim of the map's iteration order, and a report that
    // reordered itself between two looks at the same board would be one nobody
    // could trust.
    all.sort_by(|a, b| b.bytes.cmp(&a.bytes).then_with(|| a.hash.cmp(&b.hash)));

    // Counted off the finished list rather than off `users`, so both numbers
    // in "184 files, 312 cards point at them" come from the same rows the
    // sheet below is about to draw.
    let uses = all.iter().map(|file| file.cards.len()).sum();
    let shared = all.iter().map(|file| file.bytes * file.cards.len().saturating_sub(1)).sum();

    Inventory {
        kinds,
        items: doc.board.items.len(),
        binned: doc.board.trash.len(),
        connections: doc.board.connections.len(),
        assets: doc.assets.len(),
        bytes,
        waveforms: doc.waveforms.len(),
        files: all,
        uses,
        shared,
        orphans,
        steps: doc.board.timeline().steps().len(),
    }
}

/// Every asset hash one card names.
///
/// Both of them: the file the card *is*, and `meta.cover` — a video's poster or
/// a track's album art, which is an asset in the archive exactly like the file
/// it belongs to. A report that missed the second would call every poster on
/// the board an orphan.
fn hashes_of(item: &Item) -> Vec<&str> {
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

/// A count of bytes, spelled the way a file manager spells it.
///
/// Binary rather than decimal, because everything else in this app that says a
/// size says binary — the import warning, the shrink — and a page that
/// disagreed with the bar two rows away about how big the same picture is would
/// be a worse fault than either convention.
pub fn size(bytes: usize) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    match unit {
        // Bytes are whole things and a fraction of one means nothing.
        0 => format!("{bytes} B"),
        // One decimal up to ten, none above it: "1.4 MB" is a useful
        // distinction and "847.3 MB" is three digits of noise.
        _ if value < 10.0 => format!("{value:.1} {}", UNITS[unit]),
        _ => format!("{value:.0} {}", UNITS[unit]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mbrd::Asset;
    use crate::model::Board;
    use crate::state::BoardState;
    use serde_json::json;

    fn asset(bytes: usize, label: &str) -> Asset {
        Asset { bytes: vec![0; bytes], ext: "png".into(), label: label.into() }
    }

    fn doc(board: Board, assets: &[(&str, Asset)]) -> Document {
        Document {
            manifest: Default::default(),
            board: BoardState::new(board),
            assets: assets.iter().map(|(h, a)| ((*h).to_string(), a.clone())).collect(),
            waveforms: Default::default(),
        }
    }

    fn carded(id: &str, hash: &str) -> Item {
        let mut item = Item::new(id.to_string(), ItemType::Image);
        item.asset = Some(ItemAsset::Embedded { hash: hash.to_string(), family: None });
        item
    }

    #[test]
    fn the_heaviest_file_is_named_first() {
        let board =
            Board { items: vec![carded("a", "aaa"), carded("b", "bbb")], ..Default::default() };
        let report =
            of(&doc(board, &[("aaa", asset(10, "small.png")), ("bbb", asset(400, "big.png"))]));
        assert_eq!(report.files[0].label, "big.png");
        assert_eq!(report.bytes, 410);
        assert_eq!(report.assets, 2);
    }

    /// The whole reason a row carries ids rather than a count: a file under
    /// three cards is a file that deleting one card frees nothing of.
    #[test]
    fn one_file_under_two_cards_names_both() {
        let board =
            Board { items: vec![carded("a", "aaa"), carded("b", "aaa")], ..Default::default() };
        let report = of(&doc(board, &[("aaa", asset(10, "shared.png"))]));
        assert_eq!(report.files[0].cards, ["a", "b"]);
        assert!(!report.files[0].orphan);
    }

    #[test]
    fn a_file_nothing_points_at_is_an_orphan() {
        let report = of(&doc(Board::default(), &[("aaa", asset(64, "loose.png"))]));
        assert_eq!(report.orphans, Orphans { count: 1, bytes: 64 });
        assert!(report.files[0].orphan);
        assert!(report.files[0].cards.is_empty());
    }

    /// The bin keeps a file alive, and has nowhere to fly to.
    #[test]
    fn a_binned_card_keeps_its_file_off_the_orphan_list() {
        let board = Board {
            trash: vec![crate::model::TrashEntry { item: carded("a", "aaa"), at: 0 }],
            ..Default::default()
        };
        let report = of(&doc(board, &[("aaa", asset(64, "binned.png"))]));
        assert_eq!(report.orphans.count, 0);
        assert!(report.files[0].cards.is_empty(), "a moment is not a place");
    }

    #[test]
    fn what_dedupe_saved_is_every_copy_that_was_not_kept() {
        // One 400-byte file under three cards. Two copies of it were never
        // written, so the saving is twice its size — not three times, because
        // the board does still store it once.
        let board = Board {
            items: vec![carded("a", "aaa"), carded("b", "aaa"), carded("c", "aaa")],
            ..Default::default()
        };
        let report = of(&doc(board, &[("aaa", asset(400, "shared.png"))]));
        assert_eq!(report.assets, 1);
        assert_eq!(report.bytes, 400, "what it does weigh");
        assert_eq!(report.shared, 800, "what it would have weighed");
        assert_eq!(report.uses, 3);
    }

    #[test]
    fn a_file_under_one_card_saved_nothing_and_an_orphan_saved_less() {
        // The ordinary case, and the one that would be off by one if the
        // subtraction were not saturating: nought cards is nought copies not
        // kept, not minus one.
        let board = Board { items: vec![carded("a", "aaa")], ..Default::default() };
        let report = of(&doc(board, &[("aaa", asset(400, "one.png")), ("bbb", asset(9, "loose"))]));
        assert_eq!(report.shared, 0);
        assert_eq!(report.uses, 1, "the orphan is used by nothing");
    }

    /// Every file, not the ten heaviest: the sheet sorts and filters this, and
    /// sorting a truncated list by name would answer a different question.
    #[test]
    fn every_stored_file_gets_a_row() {
        let assets: Vec<(String, crate::mbrd::Asset)> =
            (0..24).map(|n| (format!("h{n:02}"), asset(n, &format!("f{n}.png")))).collect();
        let borrowed: Vec<(&str, crate::mbrd::Asset)> =
            assets.iter().map(|(h, a)| (h.as_str(), a.clone())).collect();
        let report = of(&doc(Board::default(), &borrowed));
        assert_eq!(report.files.len(), 24);
        assert_eq!(report.files[0].label, "f23.png", "still heaviest first");
    }

    /// A poster is an asset like any other, and a report that missed it would
    /// call every video's cover rubbish.
    #[test]
    fn a_posters_bytes_are_counted_against_the_card_that_names_it() {
        let mut clip = Item::new("a", ItemType::Video);
        clip.asset = Some(ItemAsset::Embedded { hash: "aaa".into(), family: None });
        clip.meta.insert("cover".into(), json!("ccc"));
        let board = Board { items: vec![clip], ..Default::default() };
        let report =
            of(&doc(board, &[("aaa", asset(90, "clip.mp4")), ("ccc", asset(8, "cover.png"))]));
        assert_eq!(report.orphans.count, 0);
    }

    #[test]
    fn the_kinds_are_counted_biggest_first() {
        let board = Board {
            items: vec![Item::new("a", ItemType::Note), carded("b", "b"), carded("c", "c")],
            ..Default::default()
        };
        let report = of(&doc(board, &[]));
        assert_eq!(report.kinds, vec![(ItemType::Image, 2), (ItemType::Note, 1)]);
        assert_eq!(report.items, 3);
    }

    #[test]
    fn a_size_is_spelled_the_way_a_file_manager_spells_it() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(999), "999 B");
        assert_eq!(size(1024), "1.0 KB");
        assert_eq!(size(1024 * 1024 * 3 / 2), "1.5 MB");
        assert_eq!(size(1024 * 1024 * 847), "847 MB");
    }
}
