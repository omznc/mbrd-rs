//! Words somebody puts on a card so they can find it again.
//!
//! A tag is an **identity**, not a label: "Kitchen", "kitchen " and "KITCHEN"
//! are one tag, because a filter that showed them as three would be useless on
//! the first typo. That is the whole of why [`clean`] exists and why every
//! other function here goes through it — a tag read off a file somebody else
//! wrote gets the same treatment as one typed here a second ago.
//!
//! ## Stored in `meta`, and additive
//!
//! `meta.tags` is an array of strings. It is not in the format's own table of
//! per-type extras, and it does not need to be: `meta` carries unknown keys
//! through untouched, which is the entire point of it being a map. The original
//! writes the same key, so a board tagged there arrives here with its tags on —
//! `arrange::Arrangement::Tag` has been clustering by them since before
//! anything here could set one.
//!
//! An item with no tags has **no `tags` key at all** rather than an empty
//! array, which is the same rule the format states for a note's `wash`: an
//! untagged card's `meta` is byte-for-byte what it was before tags existed.
//!
//! ## The filter is not in here
//!
//! Which tags are currently being filtered by is a fact about somebody's
//! session, not about the board — the same kind of state as a playhead, and
//! kept the same way, in the view and out of the file. What is here is the
//! question [`hidden`] answers, and the set is passed in.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::model::{Item, ItemType};

/// The longest a tag may be, in characters.
///
/// A tag is a word you pick off a list, and a list of sentences is a list you
/// read rather than aim at.
pub const TAG_MAX: usize = 24;

/// The most tags one card may carry.
///
/// Not a technical ceiling. Past a dozen the tags stop narrowing anything —
/// a card that is in every group is in no group — and the chips stop fitting
/// on the card that wears them.
pub const TAGS_PER_ITEM: usize = 12;

/// One tag, as it is stored and compared.
///
/// Folded to lower case and squeezed to single spaces, so a tag is an identity
/// rather than a spelling. Everything that is not part of a word becomes a
/// space, which is what makes the comma-separated input of [`split`]
/// unambiguous: a comma cannot survive into a tag, so it can only ever be the
/// separator.
///
/// **Where this deviates from the original**, and deliberately. The original's
/// rule is an allow-list of Unicode categories — letters, numbers, combining
/// marks, and three separators — on the argument that a block-list is a promise
/// the author thought of everything. Rust's standard library has no
/// general-category table, so the faithful version would mean a Unicode crate
/// carried for one rule about tag text, or a hand-copied table of every Mark
/// block that would be stale by the next Unicode release and would silently
/// break a script nobody here tests in.
///
/// So the rule is inverted, over a set small enough to name in full: control
/// characters, whitespace, ASCII punctuation other than `_` and `-`, and the
/// zero-width and bidirectional format characters. That is a *closed* list —
/// it is the set of things that could make a tag ambiguous or make a menu row
/// lie about which way round it reads — and everything else, in every script,
/// is a word character as far as this is concerned. The two rules agree on
/// every input either would call a tag; they differ on emoji and on symbols,
/// which this one keeps.
pub fn clean(raw: &str) -> String {
    let mut out = String::new();
    // Counted as it goes rather than measured at the end. `TAG_MAX` is a
    // character count and a tag out of somebody else's file may be a megabyte
    // of them, so the ceiling is what stops the loop rather than something
    // applied to whatever it produced.
    let mut kept = 0;
    let mut spaced = true; // Leading spaces are dropped by never writing one.
    for c in raw.chars() {
        if kept >= TAG_MAX {
            break;
        }
        if is_word(c) {
            spaced = false;
            // Folded here rather than over the finished string, because a fold
            // can be more than one character — `ß` is `ss` — and the ceiling
            // has to count what actually lands.
            for lower in c.to_lowercase() {
                if kept >= TAG_MAX {
                    break;
                }
                out.push(lower);
                kept += 1;
            }
        } else if !spaced {
            spaced = true;
            out.push(' ');
            kept += 1;
        }
    }
    // Trimmed after the cap rather than before, because cutting mid-phrase can
    // leave a space that was legitimately inside the tag hanging off the end.
    out.trim_end().to_string()
}

/// Whether a character is part of a word, for [`clean`]. See its note.
fn is_word(c: char) -> bool {
    if c == '_' || c == '-' {
        return true;
    }
    !(c.is_control()
        || c.is_whitespace()
        || c.is_ascii_punctuation()
        // Zero-width and bidirectional format characters. Not punctuation and
        // not control by Rust's reckoning, and the reason they are named is
        // that a right-to-left override out of somebody else's file would
        // otherwise reach a menu row and reverse the text around it.
        || matches!(c,
            '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}'))
}

/// A comma-separated line, as the tags it names.
///
/// The one thing about tags worth teaching, and the reason [`clean`] takes
/// commas out of a tag rather than merely discouraging them: "kitchen, blue,
/// warm" can only ever split one way.
pub fn split(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in line.split(',') {
        let tag = clean(part);
        if !tag.is_empty() && !out.contains(&tag) {
            out.push(tag);
        }
        if out.len() >= TAGS_PER_ITEM {
            break;
        }
    }
    out.sort();
    out
}

/// An item's tags: cleaned, deduped, sorted, and bounded.
///
/// Sorted because every reader of this list — the filter, the by-tag
/// arrangement, the line drawn on the card — treats two cards tagged with the
/// same words as the same, and an order nobody chose is an order that shows up
/// as a diff in a saved file for no reason. Cleaned before deduping, so
/// "Kitchen" and "kitchen " collapse into one.
pub fn of(item: &Item) -> Vec<String> {
    let Some(Value::Array(raw)) = item.meta.get("tags") else {
        return Vec::new();
    };
    let mut out = BTreeSet::new();
    for value in raw {
        let Some(text) = value.as_str() else { continue };
        let tag = clean(text);
        if !tag.is_empty() {
            out.insert(tag);
        }
        if out.len() >= TAGS_PER_ITEM {
            break;
        }
    }
    out.into_iter().collect()
}

/// Put exactly these tags on an item, and answer whether anything changed.
///
/// The one door: [`mark`] and everything above this module go through it, so
/// there is one place that decides what a stored tag list looks like. The key
/// is *removed* when the list is empty rather than written as `[]` — see the
/// module note.
pub fn set(item: &mut Item, tags: &[String]) -> bool {
    let mut kept: Vec<String> = Vec::new();
    for tag in tags {
        let tag = clean(tag);
        if !tag.is_empty() && !kept.contains(&tag) {
            kept.push(tag);
        }
        if kept.len() >= TAGS_PER_ITEM {
            break;
        }
    }
    kept.sort();
    if kept == of(item) && item.meta.contains_key("tags") == !kept.is_empty() {
        return false;
    }
    if kept.is_empty() {
        item.meta.remove("tags");
    } else {
        item.meta.insert("tags".into(), Value::Array(kept.into_iter().map(Value::from).collect()));
    }
    true
}

/// Put one tag on an item or take it off, and answer whether anything changed.
///
/// Answering that is what lets a caller record no undo step for a tag that was
/// already there — the same bargain `align` makes by returning only what moved.
pub fn mark(item: &mut Item, tag: &str, on: bool) -> bool {
    let tag = clean(tag);
    if tag.is_empty() {
        return false;
    }
    let mut tags = of(item);
    let held = tags.contains(&tag);
    if held == on {
        return false;
    }
    if on {
        // The ceiling is checked here rather than left to `set`, which would
        // silently drop whichever tag sorted last instead of this one.
        if tags.len() >= TAGS_PER_ITEM {
            return false;
        }
        tags.push(tag);
    } else {
        tags.retain(|t| *t != tag);
    }
    set(item, &tags)
}

/// Whether this item may carry a tag at all.
///
/// Content and fences. What is left out is the app's own furniture — the title
/// card, the onboarding hints, the style tile — which is this build talking
/// rather than anything of somebody's, and which nobody should be able to file
/// under a word of their own.
///
/// **A fence is the most useful thing on a board to tag**, because it is the
/// one item that stands for a batch of work rather than a file: "the splashback
/// options", "rejected". This used to be refused on the grounds that the by-tag
/// arrangement would not know whether a tagged fence went into its own tag's
/// block or travelled with its contents. It does know, and it always did — see
/// `crate::arrange`, where a fence in the set being laid out is dealt a slot
/// and everything it holds is carried along behind it. That is what a fence
/// does under *every* arrangement, so it is not a question the tag mode has to
/// answer differently.
pub fn taggable(item: &Item) -> bool {
    item.kind.is_content() || item.kind == ItemType::Fence
}

/// Every tag on a board, each with how many cards wear it — most-used first,
/// alphabetical within a count.
///
/// Most-used first because the second and third tag anybody adds to a board are
/// almost always tags that already exist on it, and a list in the order they
/// were invented would put the useful ones wherever they happened to land.
pub fn census(items: &[Item]) -> Vec<(String, usize)> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for item in items.iter().filter(|i| taggable(i)) {
        for tag in of(item) {
            *counts.entry(tag).or_default() += 1;
        }
    }
    let mut out: Vec<(String, usize)> = counts.into_iter().collect();
    // The map is already alphabetical, so a stable sort by count descending
    // leaves ties in that order — which is the second half of the rule.
    out.sort_by_key(|(_, count)| std::cmp::Reverse(*count));
    out
}

/// Whether the filter as it stands has this card faded.
///
/// `false` whenever nothing is being filtered, which is the overwhelmingly
/// common case and is why that test comes first. Otherwise a card is faded
/// unless it carries **one of** the filtered tags rather than all of them: a
/// person adding a second tag to a filter is widening the question ("kitchen or
/// blue"), because narrowing it is what the first tag already did.
///
/// Furniture is never faded. The title card and the hints are the app talking
/// rather than anything of somebody's own, and fading its own instructions on
/// an empty board would be the app hiding what it just said.
pub fn hidden(item: &Item, filter: &BTreeSet<String>) -> bool {
    if filter.is_empty() || !taggable(item) {
        return false;
    }
    !of(item).iter().any(|tag| filter.contains(tag))
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    fn item(id: &str) -> Item {
        Item::new(id.to_string(), ItemType::Image)
    }

    fn tagged(id: &str, tags: &[&str]) -> Item {
        let mut it = item(id);
        it.meta.insert("tags".into(), json!(tags));
        it
    }

    #[test]
    fn a_tag_is_an_identity_rather_than_a_spelling() {
        assert_eq!(clean("Kitchen"), "kitchen");
        assert_eq!(clean("  KITCHEN  "), "kitchen");
        assert_eq!(clean("warm   greys"), "warm greys");
    }

    /// The rule the comma-separated input rests on.
    #[test]
    fn a_comma_cannot_survive_into_a_tag() {
        assert_eq!(clean("blue,green"), "blue green");
        assert_eq!(split("kitchen, blue , kitchen"), vec!["blue", "kitchen"]);
    }

    #[test]
    fn what_a_word_is_made_of_is_kept_whatever_the_script() {
        assert_eq!(clean("café"), "café");
        assert_eq!(clean("Кухня"), "кухня");
        assert_eq!(clean("台所"), "台所");
        assert_eq!(clean("mid-century_modern"), "mid-century_modern");
    }

    /// Nothing out of somebody else's file reaches a menu row and reverses it.
    #[test]
    fn a_control_or_an_override_becomes_a_space() {
        assert_eq!(clean("blue\u{202E}green"), "blue green");
        assert_eq!(clean("blue\u{0}green"), "blue green");
        assert_eq!(clean("\u{200B}"), "");
    }

    #[test]
    fn a_tag_is_cut_to_something_a_list_can_show() {
        let long = "a".repeat(100);
        assert_eq!(clean(&long).chars().count(), TAG_MAX);
        // And the cut never leaves a space hanging off the end.
        let words = "aaaaaaaaaaaaaaaaaaaaaaa bbbbbb";
        assert!(!clean(words).ends_with(' '));
    }

    #[test]
    fn an_untagged_card_has_no_key_at_all() {
        let mut it = tagged("a", &["blue"]);
        assert!(set(&mut it, &[]));
        assert!(!it.meta.contains_key("tags"), "an empty list is no list");
    }

    #[test]
    fn setting_what_is_already_there_changes_nothing() {
        let mut it = tagged("a", &["blue", "kitchen"]);
        assert!(!set(&mut it, &["kitchen".into(), "Blue".into()]), "the same two, resorted");
        assert!(!mark(&mut it, "blue", true));
        assert!(mark(&mut it, "blue", false));
        assert_eq!(of(&it), vec!["kitchen"]);
    }

    #[test]
    fn a_card_stops_taking_tags_at_the_ceiling() {
        let mut it = item("a");
        for i in 0..TAGS_PER_ITEM {
            assert!(mark(&mut it, &format!("tag{i}"), true));
        }
        assert!(!mark(&mut it, "one more", true), "the ceiling holds");
        assert_eq!(of(&it).len(), TAGS_PER_ITEM);
    }

    #[test]
    fn a_file_written_by_hand_is_cleaned_on_the_way_in() {
        let it = tagged("a", &["Kitchen", "kitchen ", "", "BLUE"]);
        assert_eq!(of(&it), vec!["blue", "kitchen"]);
    }

    #[test]
    fn the_census_puts_the_most_used_first_and_sorts_the_ties() {
        let board = vec![
            tagged("a", &["blue", "kitchen"]),
            tagged("b", &["blue", "warm"]),
            tagged("c", &["blue"]),
        ];
        assert_eq!(
            census(&board),
            vec![("blue".into(), 3), ("kitchen".into(), 1), ("warm".into(), 1)]
        );
    }

    #[test]
    fn the_census_leaves_out_the_apps_own_furniture() {
        // The title card and the onboarding hints are this build talking, and
        // a word somebody filed their board under should never be one of them.
        for kind in [ItemType::Title, ItemType::Ghost, ItemType::StyleTile] {
            let mut furniture = tagged("f", &["blue"]);
            furniture.kind = kind.clone();
            assert!(census(&[furniture]).is_empty(), "{kind:?} was counted");
            let mut furniture = tagged("f", &["blue"]);
            furniture.kind = kind;
            assert!(!hidden(&furniture, &["red".into()].into()), "furniture was faded");
        }
    }

    /// The one item on a board that stands for a batch of work rather than a
    /// file, and therefore the most useful thing on it to file under a word.
    #[test]
    fn a_fence_can_be_tagged() {
        let mut fence = tagged("f", &["blue"]);
        fence.kind = ItemType::Fence;
        assert!(taggable(&fence));
        assert_eq!(census(std::slice::from_ref(&fence)), vec![("blue".into(), 1)]);
        // And the standing filter treats it like anything else: a fence tagged
        // with none of what is being shown fades with its contents.
        assert!(hidden(&fence, &["red".into()].into()));
        assert!(!hidden(&fence, &["blue".into()].into()));
    }

    #[test]
    fn a_filter_of_two_tags_asks_for_either() {
        let filter: BTreeSet<String> = ["blue".into(), "warm".into()].into();
        assert!(!hidden(&tagged("a", &["blue"]), &filter));
        assert!(!hidden(&tagged("b", &["warm", "kitchen"]), &filter));
        assert!(hidden(&tagged("c", &["kitchen"]), &filter));
        assert!(hidden(&item("d"), &filter), "an untagged card is not in any group");
    }

    #[test]
    fn nothing_is_faded_when_nothing_is_filtered() {
        assert!(!hidden(&item("a"), &BTreeSet::new()));
    }

    #[test]
    fn the_app_never_fades_its_own_furniture() {
        let filter: BTreeSet<String> = ["blue".into()].into();
        let mut title = item("t");
        title.kind = ItemType::Title;
        assert!(!hidden(&title, &filter));
    }
}
