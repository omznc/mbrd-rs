//! The step ledger — how the board got here, as data.
//!
//! This is a port of `web/assets/js/timeline.ts` from the original mbrd, and it
//! keeps that file's central decision: **a step is a difference, keyed by item
//! id, and it holds the state on both sides of what one action touched.** That
//! is what makes it reversible in either direction without knowing what the
//! action meant, and it is what makes a step eighty bytes instead of the sixty
//! kilobytes a pair of whole item arrays would cost.
//!
//! Nothing in this module knows what a [`Board`](crate::model::Board) is. Every
//! recorded value is *text* — one item, one geometry record or one bin entry as
//! `board.json` would carry it — so the diffing, merging, folding and hashing in
//! here are string operations with no model beneath them. Turning a board into
//! those strings and back is [`crate::state`]'s job, which is what keeps this
//! file at the bottom of the stack beside `geometry.rs`.
//!
//! ## The two rules that are easy to break
//!
//! **A delta applies only the fields it changed**, never the whole recorded
//! object. Writing the object whole would make every step assert *everything*
//! about that card, so editing a step in the past would take effect and then be
//! silently undone by the next step that happened to mention the same card. See
//! [`merge_changed`].
//!
//! **A run of changes to one card is one step.** Twelve taps of an arrow key are
//! one entry holding the first position and the last. That is the one place the
//! format deliberately remembers less than the session did, and it is what keeps
//! a file the size of somebody's intentions rather than the size of their
//! keystrokes. See [`run_key`].

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

/// One keyed list of the board, reduced to comparable text: id to JSON.
///
/// Sorted rather than insertion-ordered, so that two boards that differ only in
/// the order a map happened to be built produce byte-identical bases. The order
/// a list is actually *in* is carried separately, in `itemOrder` and
/// `trashOrder`, because that is a fact about the board rather than about the
/// map.
pub type Keyed = BTreeMap<String, String>;

/// The board as it stands, as text. Nothing derived, nothing live.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snap {
    pub items: Keyed,
    pub item_order: Vec<String>,
    pub desktop: Keyed,
    pub mobile: Keyed,
    /// Keyed by the id of the item *inside* the bin entry, since that is what is
    /// unique — a bin entry is a wrapper with a timestamp and has no id of its own.
    pub trash: Keyed,
    pub trash_order: Vec<String>,
    /// Everything else, whole-field, because all of it is small.
    pub rest: BTreeMap<String, String>,
}

/// What changed in one keyed list: `(before, after)` per id, `None` for absent.
///
/// Reversible by construction — going back reads the left of each pair, going
/// forward the right — which is the property the whole module turns on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyedDelta {
    pub changed: BTreeMap<String, (Option<String>, Option<String>)>,
    /// Recorded only when the order itself moved.
    pub order: Option<(Vec<String>, Vec<String>)>,
}

impl KeyedDelta {
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty() && self.order.is_none()
    }
}

/// The difference between two snapshots. Absent sections did not change.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Delta {
    pub items: Option<KeyedDelta>,
    pub desktop: Option<KeyedDelta>,
    pub mobile: Option<KeyedDelta>,
    pub trash: Option<KeyedDelta>,
    pub rest: BTreeMap<String, (String, String)>,
    /// Sections a build that is not this one wrote. Carried, never applied.
    ///
    /// The same extension point `meta` is for items: a newer build that adds a
    /// fifth keyed section should find it intact after a round trip here, and
    /// a reader that applied a section it did not understand would be guessing.
    pub extra: Map<String, Value>,
}

impl Delta {
    pub fn is_empty(&self) -> bool {
        self.items.is_none()
            && self.desktop.is_none()
            && self.mobile.is_none()
            && self.trash.is_none()
            && self.rest.is_empty()
            && self.extra.is_empty()
    }

    /// The four keyed sections, by name, for the walks that treat them alike.
    fn section(&self, name: Section) -> Option<&KeyedDelta> {
        match name {
            Section::Items => self.items.as_ref(),
            Section::Desktop => self.desktop.as_ref(),
            Section::Mobile => self.mobile.as_ref(),
            Section::Trash => self.trash.as_ref(),
        }
    }

    fn set_section(&mut self, name: Section, value: Option<KeyedDelta>) {
        match name {
            Section::Items => self.items = value,
            Section::Desktop => self.desktop = value,
            Section::Mobile => self.mobile = value,
            Section::Trash => self.trash = value,
        }
    }
}

/// Which keyed list of the board is being talked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Items,
    Desktop,
    Mobile,
    Trash,
}

impl Section {
    pub const ALL: [Section; 4] =
        [Section::Items, Section::Desktop, Section::Mobile, Section::Trash];

    pub fn as_str(&self) -> &'static str {
        match self {
            Section::Items => "items",
            Section::Desktop => "desktop",
            Section::Mobile => "mobile",
            Section::Trash => "trash",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "items" => Some(Section::Items),
            "desktop" => Some(Section::Desktop),
            "mobile" => Some(Section::Mobile),
            "trash" => Some(Section::Trash),
            _ => None,
        }
    }
}

/// One thing that was done, as data.
#[derive(Debug, Clone)]
pub struct Step {
    pub id: String,
    /// Milliseconds since the Unix epoch, as the format writes it.
    pub at: i64,
    pub label: String,
    /// What makes two consecutive steps one step. Empty never merges. See [`run_key`].
    pub run: String,
    /// A name somebody gave this point. This is what a saved version became.
    pub name: Option<String>,
    /// The rule this step followed, for the steps that have one.
    ///
    /// Nothing in this build produces or re-runs one — editing the past is a
    /// later phase. It is read and written back verbatim so that a board edited
    /// by a build that *does* understand `align`, `distribute` and `arrange`
    /// does not lose the reason for its steps by passing through here.
    pub op: Option<Value>,
    pub delta: Delta,
}

/// Past this many steps the oldest are folded away rather than kept.
pub const STEP_CAP: usize = 20_000;

/// How the board got here.
///
/// `steps` are in order, oldest first. `at` is how many of them are *on* the
/// board — the marker — so a board can be saved with its history rolled back and
/// comes back that way.
#[derive(Debug, Clone, Default)]
pub struct Timeline {
    pub(crate) steps: Vec<Step>,
    pub(crate) at: usize,
    /// The board before step 0. Every replay starts here.
    pub(crate) base: Snap,
    pub(crate) stale: bool,
    /// Feeds the step ids, so that two steps recorded in the same millisecond
    /// still differ.
    pub(crate) seq: u64,
}

impl Timeline {
    /// An empty ledger over a board that has not been changed yet.
    pub fn starting_at(base: Snap) -> Self {
        Self { steps: Vec::new(), at: 0, base, stale: false, seq: 0 }
    }

    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    /// How many steps are on the board.
    pub fn at(&self) -> usize {
        self.at
    }

    /// Whether the steps still describe the board they arrived with.
    ///
    /// A stale ledger is kept and written back — throwing away somebody's
    /// history because this build cannot vouch for it would be the wrong way
    /// round — but nothing should replay it.
    pub fn stale(&self) -> bool {
        self.stale
    }

    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    pub fn base(&self) -> &Snap {
        &self.base
    }

    /// What undo would take back next, by name, or `None` for nothing.
    pub fn undo_label(&self) -> Option<&str> {
        if self.stale || self.at == 0 {
            return None;
        }
        Some(self.steps[self.at - 1].label.as_str())
    }

    /// What redo would put back next.
    pub fn redo_label(&self) -> Option<&str> {
        if self.stale || self.at >= self.steps.len() {
            return None;
        }
        Some(self.steps[self.at].label.as_str())
    }

    /// Start over with the board as it stands.
    pub fn reset(&mut self, base: Snap) {
        self.steps.clear();
        self.at = 0;
        self.base = base;
        self.stale = false;
    }

    /// Write down what happened, and answer whether a step landed.
    ///
    /// Two consecutive steps merge when they touch the same cards in the same
    /// fields: move a card twice with nothing in between and it is one step,
    /// move it and then recolour it and it is two. Anything else you do lands a
    /// step with a different key, which is what closes a run.
    ///
    /// A run that folds back to nothing — moved it and moved it back — leaves no
    /// step behind at all, which is what stops a dot appearing on the strip for
    /// an action with no effect.
    pub fn record(&mut self, label: &str, delta: Delta, now: i64) -> bool {
        if delta.is_empty() {
            return false;
        }
        // Doing something new with the marker rolled back drops what was ahead
        // of it, the way undo does everywhere.
        if self.at < self.steps.len() {
            self.steps.truncate(self.at);
        }

        let run = run_key(&delta);
        let merges =
            !run.is_empty() && self.steps.last().map(|prev| prev.run == run).unwrap_or(false);

        if merges {
            let prev = self.steps.last_mut().expect("`merges` read the last step");
            prev.delta = merge_delta(&prev.delta, &delta);
            prev.at = now;
            prev.label = label.to_string();
            if prev.delta.is_empty() {
                self.steps.pop();
                self.at = self.steps.len();
                return false;
            }
            return true;
        }

        self.seq = self.seq.wrapping_add(1);
        let id = step_id(self.seq, now, label);
        self.steps.push(Step {
            id,
            at: now,
            label: label.to_string(),
            run,
            name: None,
            op: None,
            delta,
        });
        self.at = self.steps.len();
        if self.steps.len() > STEP_CAP {
            self.fold_oldest(self.steps.len() - STEP_CAP);
        }
        true
    }

    /// Fold the oldest steps into the base, so the ledger stays bounded.
    ///
    /// Folded *forward into the base* rather than dropped: the base is the state
    /// step 0 follows from, so simply deleting the front would leave a ledger
    /// describing a board it can no longer replay to.
    fn fold_oldest(&mut self, count: usize) {
        let count = count.min(self.steps.len());
        if count == 0 {
            return;
        }
        for step in &self.steps[..count] {
            apply_to_snap(&mut self.base, &step.delta, true);
        }
        self.steps.drain(..count);
        self.at = self.at.saturating_sub(count);
    }

    /// Give a step a name, so the strip has a landmark on it.
    pub fn name_step(&mut self, index: usize, name: Option<&str>) -> bool {
        let Some(step) = self.steps.get_mut(index) else {
            return false;
        };
        step.name = name.map(|n| n.chars().take(120).collect::<String>()).filter(|n| !n.is_empty());
        true
    }

    /// Every asset hash the steps point at, on **both** sides of every pair.
    ///
    /// The fourth class of reference, and the one with teeth: a step that
    /// deleted a photograph carries that photograph on its *before* side, and
    /// that is what stepping back puts on the board again. So an asset a step
    /// names is live even when no card and no bin entry wants it.
    ///
    /// Unlike the live board, a hash named only from here that has no bytes is
    /// **not** fatal to a save — a step can legitimately name bytes something
    /// else discarded, and refusing to write somebody's board over an entry in
    /// its history would be the wrong way round.
    ///
    /// **The bin is not walked**, and that is the whole of what keeps a deleted
    /// photograph from following a board around forever. The bin does not reach
    /// the file at all now — see [`TrashEntry`](crate::model::TrashEntry) — so
    /// bytes that only a bin entry names are bytes nothing will ever read back.
    /// Nothing is lost by it: binning a card records the card on the *items*
    /// side of the same step, as the value stepping backwards restores, and
    /// that side is walked. Skipping the trash side skips a second copy of the
    /// same names, plus the one case they differ — a bin somebody filled in
    /// another app, which this app is about to drop anyway.
    pub fn hashes(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for text in self.base.items.values() {
            eat_hashes(text, &mut out);
        }
        for step in &self.steps {
            let Some(part) = step.delta.section(Section::Items) else { continue };
            for (before, after) in part.changed.values() {
                for text in [before, after].into_iter().flatten() {
                    eat_hashes(text, &mut out);
                }
            }
        }
        out
    }

    /// Rewrite items the ledger has written down, wherever it wrote them.
    ///
    /// The one operation here that edits the past on purpose, and it exists for
    /// the one action in the app that destroys something: emptying the bin,
    /// which throws the bytes away rather than merely taking the card off the
    /// board. A step that still named those bytes would come back as a hole, so
    /// the record is brought into line with the truth instead.
    ///
    /// `rewrite` is handed each recorded item and answers a replacement, or
    /// `None` to leave that one alone. Both sides of every pair are offered: a
    /// step's *before* is what going backwards puts on the board, so rewriting
    /// only the after would leave every scrub past the delete mounting the
    /// picture again. Answers how many recorded copies were replaced.
    pub fn rewrite_recorded(&mut self, rewrite: impl Fn(&Value) -> Option<Value>) -> usize {
        let mut count = 0;
        let mut redo = |text: &str| -> Option<String> {
            // Tolerant: a value that will not parse is left exactly as it was
            // rather than throwing out of an action somebody pressed a button for.
            let parsed: Value = serde_json::from_str(text).ok()?;
            let inner = parsed.get("item").filter(|v| v.is_object());
            let next = rewrite(inner.unwrap_or(&parsed))?;
            count += 1;
            let replaced = match inner {
                Some(_) => {
                    let mut wrapper = parsed.as_object().cloned().unwrap_or_default();
                    wrapper.insert("item".into(), next);
                    Value::Object(wrapper)
                }
                None => next,
            };
            serde_json::to_string(&replaced).ok()
        };

        for keyed in [&mut self.base.items, &mut self.base.trash] {
            for text in keyed.values_mut() {
                if let Some(next) = redo(text) {
                    *text = next;
                }
            }
        }
        for step in &mut self.steps {
            for section in [Section::Items, Section::Trash] {
                let Some(part) = (match section {
                    Section::Items => step.delta.items.as_mut(),
                    _ => step.delta.trash.as_mut(),
                }) else {
                    continue;
                };
                for pair in part.changed.values_mut() {
                    for text in [&mut pair.0, &mut pair.1].into_iter().flatten() {
                        if let Some(next) = redo(text) {
                            *text = next;
                        }
                    }
                }
            }
        }
        count
    }

    // -----------------------------------------------------------------------
    // The file
    // -----------------------------------------------------------------------

    /// The ledger as `board.json` carries it, or `None` when there is nothing
    /// to say.
    ///
    /// **`None` rather than a null**, so that a board nobody has changed is
    /// written exactly as it was before this feature existed — the difference
    /// between a format that grew a key and one that grew a key that is usually
    /// empty.
    ///
    /// The fingerprint is handed in rather than taken here, because it is a hash
    /// of the *document* and this module never sees one. See
    /// [`crate::schema::doc_fingerprint`].
    pub fn to_value(&self, fingerprint: &str) -> Option<Value> {
        if self.steps.is_empty() {
            return None;
        }
        Some(json!({
            "base": snap_to_value(&self.base),
            "at": self.at,
            "fingerprint": fingerprint,
            "steps": Value::Array(self.steps.iter().map(step_to_value).collect()),
        }))
    }

    /// Take a stored ledger as this board's history.
    ///
    /// Checks rather than trusts: if the steps do not add up to the board they
    /// arrived with, the ledger is marked stale. A history that quietly
    /// describes the wrong board is worse than no history, which is the whole
    /// reason the fingerprint is written in the first place. A ledger with no
    /// fingerprint at all is taken at its word — that is what a file written
    /// before the check looks like, and refusing those would be treating an old
    /// friend as a forgery.
    ///
    /// Cannot fail, in the same sense [`crate::schema::normalize`] cannot: what
    /// it does not understand becomes an empty ledger over `fallback`.
    pub fn adopt(raw: Option<&Value>, filed: Option<&str>, fallback: Snap) -> Timeline {
        let empty = Timeline::starting_at(fallback);
        let Some(src) = raw.and_then(Value::as_object) else {
            return empty;
        };
        let Some(list) = src.get("steps").and_then(Value::as_array) else {
            return empty;
        };
        // All or nothing rather than field-by-field defaults: a base with half
        // its sections is a base that replays to a board nobody had.
        let Some(base) = snap_of_value(src.get("base")) else {
            return empty;
        };

        let mut steps: Vec<Step> = Vec::new();
        for (n, entry) in list.iter().enumerate() {
            let Some(record) = entry.as_object() else { continue };
            let Some(delta) = record.get("delta").and_then(delta_of_value) else {
                continue;
            };
            steps.push(Step {
                id: match record.get("id").and_then(Value::as_str) {
                    Some(id) if !id.is_empty() => id.chars().take(64).collect(),
                    _ => step_id(n as u64 + 1, 0, "adopted"),
                },
                at: record.get("at").and_then(Value::as_i64).unwrap_or(0),
                label: record
                    .get("label")
                    .and_then(Value::as_str)
                    .map(|s| s.chars().take(120).collect())
                    .unwrap_or_else(|| "Change".to_string()),
                run: record
                    .get("run")
                    .and_then(Value::as_str)
                    .map(|s| s.chars().take(400).collect())
                    .unwrap_or_default(),
                name: record
                    .get("name")
                    .and_then(Value::as_str)
                    .map(|s| s.chars().take(120).collect::<String>())
                    .filter(|s| !s.is_empty()),
                op: record.get("op").filter(|v| v.is_object()).cloned(),
                delta,
            });
            if steps.len() >= STEP_CAP {
                break;
            }
        }
        if steps.is_empty() {
            return empty;
        }

        // Whether the cap actually bit. The oldest steps are the ones kept,
        // because `base` is the state step 0 follows from — so what is lost is
        // the newest steps, which are the ones somebody would want, and `stale`
        // is already the flag that means "this ledger does not describe the
        // board in front of you".
        let truncated = list.len() > steps.len();
        let at = match src.get("at").and_then(Value::as_u64) {
            Some(n) => (n as usize).min(steps.len()),
            None => steps.len(),
        };
        let claimed = src.get("fingerprint").and_then(Value::as_str).unwrap_or("");
        let stale =
            truncated || (!claimed.is_empty() && filed.map(|f| f != claimed).unwrap_or(false));

        Timeline { seq: steps.len() as u64, steps, at, base, stale }
    }
}

// ---------------------------------------------------------------------------
// Diffing
// ---------------------------------------------------------------------------

/// Which half of a recorded pair a direction reads.
fn side<T: Clone>(pair: &(T, T), forward: bool) -> T {
    if forward {
        pair.1.clone()
    } else {
        pair.0.clone()
    }
}

fn diff_keyed(
    before: &Keyed,
    after: &Keyed,
    before_order: Option<&[String]>,
    after_order: Option<&[String]>,
) -> Option<KeyedDelta> {
    let mut changed: BTreeMap<String, (Option<String>, Option<String>)> = BTreeMap::new();
    for id in before.keys().chain(after.keys()) {
        if changed.contains_key(id) {
            continue;
        }
        let a = before.get(id);
        let b = after.get(id);
        if a == b {
            continue;
        }
        changed.insert(id.clone(), (a.cloned(), b.cloned()));
    }
    let moved = match (before_order, after_order) {
        (Some(a), Some(b)) => a != b,
        _ => false,
    };
    if changed.is_empty() && !moved {
        return None;
    }
    Some(KeyedDelta {
        changed,
        order: moved.then(|| {
            (before_order.unwrap_or_default().to_vec(), after_order.unwrap_or_default().to_vec())
        }),
    })
}

/// What it would take to get from one snapshot to another, or `None` for nothing.
///
/// The board's own door does not come through here — it compares two boards
/// structurally and never builds a snapshot at all, because building one costs
/// the whole board. This is the snapshot-level counterpart, and it is the half
/// of the pair a scrub needs: replaying to an arbitrary step means walking the
/// base forward with [`apply_to_snap`], and measuring where that got to means
/// this. Kept and tested for that reason rather than because anything calls it
/// today.
pub fn diff(before: &Snap, after: &Snap) -> Option<Delta> {
    let mut out = Delta {
        items: diff_keyed(
            &before.items,
            &after.items,
            Some(&before.item_order),
            Some(&after.item_order),
        ),
        desktop: diff_keyed(&before.desktop, &after.desktop, None, None),
        mobile: diff_keyed(&before.mobile, &after.mobile, None, None),
        trash: diff_keyed(
            &before.trash,
            &after.trash,
            Some(&before.trash_order),
            Some(&after.trash_order),
        ),
        ..Delta::default()
    };
    for (field, a) in &before.rest {
        match after.rest.get(field) {
            Some(b) if b == a => {}
            Some(b) => {
                out.rest.insert(field.clone(), (a.clone(), b.clone()));
            }
            // A field the later snapshot does not carry at all. Recorded as a
            // move to the empty text rather than dropped, so going back still
            // finds the value that was there.
            None => {
                out.rest.insert(field.clone(), (a.clone(), String::new()));
            }
        }
    }
    for (field, b) in &after.rest {
        if !before.rest.contains_key(field) {
            out.rest.insert(field.clone(), (String::new(), b.clone()));
        }
    }
    (!out.is_empty()).then_some(out)
}

/// `held`, with the fields that differ between `from` and `to` set to `to`'s.
///
/// The one operation that makes a difference behave like a difference rather
/// than like a photograph. Top-level keys only: `meta` is compared and written
/// whole, which is the right grain here — a step that edited a note's text and a
/// step that changed its tint both write the whole `meta` bag, and going finer
/// would mean this module knowing what is in it.
pub fn merge_changed(held: &Value, from: &Value, to: &Value) -> Value {
    let mut next = held.as_object().cloned().unwrap_or_default();
    let empty = Map::new();
    let from = from.as_object().unwrap_or(&empty);
    let to = to.as_object().unwrap_or(&empty);
    for key in from.keys().chain(to.keys()) {
        if from.get(key) == to.get(key) {
            continue;
        }
        match to.get(key) {
            Some(v) => {
                next.insert(key.clone(), v.clone());
            }
            None => {
                next.remove(key);
            }
        }
    }
    Value::Object(next)
}

/// One step folded into the one before it. See [`Step::run`].
pub fn merge_delta(first: &Delta, second: &Delta) -> Delta {
    let mut out = Delta::default();
    for name in Section::ALL {
        let a = first.section(name);
        let b = second.section(name);
        let (a, b) = match (a, b) {
            (None, None) => continue,
            (None, Some(b)) => {
                out.set_section(name, Some(b.clone()));
                continue;
            }
            (Some(a), None) => {
                out.set_section(name, Some(a.clone()));
                continue;
            }
            (Some(a), Some(b)) => (a, b),
        };
        let mut changed = a.changed.clone();
        for (id, pair) in &b.changed {
            // The earliest before and the latest after: that is the whole of
            // what collapsing a run means. `remove` rather than `get` so the
            // prior `from` is taken rather than cloned when there is one.
            let from = changed.remove(id).map(|p| p.0).unwrap_or_else(|| pair.0.clone());
            changed.insert(id.clone(), (from, pair.1.clone()));
        }
        // A card moved out and back is not a change, and leaving the pair in
        // would put an entry on the strip for an action with no effect.
        changed.retain(|_, pair| pair.0 != pair.1);
        let order = match (&a.order, &b.order) {
            (None, None) => None,
            (a_order, b_order) => Some((
                a_order.as_ref().or(b_order.as_ref()).expect("one is Some").0.clone(),
                b_order.as_ref().or(a_order.as_ref()).expect("one is Some").1.clone(),
            )),
        };
        let merged = KeyedDelta { changed, order };
        if !merged.is_empty() {
            out.set_section(name, Some(merged));
        }
    }
    let mut rest = first.rest.clone();
    for (field, pair) in &second.rest {
        let from = rest.remove(field).map(|p| p.0).unwrap_or_else(|| pair.0.clone());
        rest.insert(field.clone(), (from, pair.1.clone()));
    }
    rest.retain(|_, pair| pair.0 != pair.1);
    out.rest = rest;
    out.extra = first.extra.clone();
    for (key, value) in &second.extra {
        out.extra.insert(key.clone(), value.clone());
    }
    out
}

/// What makes two steps one: the ids that changed, and which of their fields.
///
/// Field-level rather than id-level on purpose. Same cards and same fields is a
/// run — twelve taps of an arrow key, a colour slider dragged across its range.
/// Same card and different fields is two intentions and reads as two steps.
///
/// An empty key never merges, and there are three ways to get one: an arriving
/// or departing card, a list whose order moved, and a delta that changed nothing.
pub fn run_key(delta: &Delta) -> String {
    let mut ids: BTreeSet<String> = BTreeSet::new();
    let mut fields: BTreeSet<String> = BTreeSet::new();
    for name in Section::ALL {
        let Some(part) = delta.section(name) else { continue };
        // An arriving or departing card is never part of a run: the two sides of
        // its pair are an object and nothing, and collapsing "added" into
        // "moved" would hide the add.
        if part.order.is_some() {
            return String::new();
        }
        for (id, (before, after)) in &part.changed {
            let (Some(before), Some(after)) = (before, after) else {
                return String::new();
            };
            ids.insert(format!("{}:{id}", name.as_str()));
            for field in changed_fields(before, after) {
                fields.insert(field);
            }
        }
    }
    for field in delta.rest.keys() {
        fields.insert(format!("rest.{field}"));
    }
    if ids.is_empty() && fields.is_empty() {
        return String::new();
    }
    format!(
        "{}|{}",
        ids.into_iter().collect::<Vec<_>>().join(","),
        fields.into_iter().collect::<Vec<_>>().join(",")
    )
}

/// Which top-level keys differ between two serialised records.
///
/// A pair that will not parse, or that is not a pair of objects, answers `?` —
/// one opaque field name, which is a key that can still merge with itself and
/// cannot merge with anything else.
fn changed_fields(before: &str, after: &str) -> Vec<String> {
    let (Ok(a), Ok(b)) =
        (serde_json::from_str::<Value>(before), serde_json::from_str::<Value>(after))
    else {
        return vec!["?".into()];
    };
    let (Some(a), Some(b)) = (a.as_object(), b.as_object()) else {
        return vec!["?".into()];
    };
    let mut out: BTreeSet<String> = BTreeSet::new();
    for key in a.keys().chain(b.keys()) {
        if a.get(key) != b.get(key) {
            out.insert(key.clone());
        }
    }
    out.into_iter().collect()
}

// ---------------------------------------------------------------------------
// Applying, to a snapshot
// ---------------------------------------------------------------------------

/// Move a keyed map one step, in either direction.
///
/// Shared by the snapshot walk here and by the board walk in [`crate::state`],
/// which is what keeps folding a step into the base and undoing it on the live
/// board from ever disagreeing about what a delta means.
pub fn apply_keyed(
    current: &mut Keyed,
    order: &mut Vec<String>,
    delta: &KeyedDelta,
    forward: bool,
) {
    for (id, pair) in &delta.changed {
        let want = if forward { pair.1.as_ref() } else { pair.0.as_ref() };
        let leaving = if forward { pair.0.as_ref() } else { pair.1.as_ref() };
        let Some(want) = want else {
            current.remove(id);
            continue;
        };
        let held = current.get(id);
        match (leaving, held) {
            (Some(leaving), Some(held)) => {
                let merged = merge_text(held, leaving, want);
                current.insert(id.clone(), merged);
            }
            _ => {
                current.insert(id.clone(), want.clone());
            }
        }
    }
    let next = match &delta.order {
        Some(pair) => side(pair, forward),
        None => order.iter().filter(|id| current.contains_key(*id)).cloned().collect(),
    };
    // `placed` rather than a scan of `out`, because a recorded order can be
    // twenty thousand long and searching it per entry would make a single undo
    // quadratic in the size of the board.
    let mut out: Vec<String> = Vec::with_capacity(next.len());
    let mut placed: BTreeSet<&String> = BTreeSet::new();
    for id in &next {
        if current.contains_key(id) && placed.insert(id) {
            out.push(id.clone());
        }
    }
    // Anything the recorded order does not mention still belongs on the board.
    // Only reachable from a malformed step, and dropping items on the floor is
    // the one failure this module must not have.
    for id in current.keys() {
        if !placed.contains(id) {
            out.push(id.clone());
        }
    }
    *order = out;
}

/// [`merge_changed`], on the text either side of it.
///
/// **All three have to be objects for there to be anything to merge**, and
/// anything else takes `want` whole. That is not a guard against nonsense — it
/// is the ordinary case for half the fields a step records: a title is a string,
/// `titleHidden` is a boolean, `tour` is an array, and "the fields that changed"
/// is not a question any of them can answer. Merging one anyway is how a step
/// that renamed a board came to undo it to the empty string, which is what the
/// walk-both-ways test in `state.rs` was written to catch.
///
/// Text that will not parse takes `want` too, for the same reason: there are no
/// fields to compare.
pub fn merge_text(held: &str, leaving: &str, want: &str) -> String {
    let (Ok(held), Ok(leaving), Ok(want_value)) = (
        serde_json::from_str::<Value>(held),
        serde_json::from_str::<Value>(leaving),
        serde_json::from_str::<Value>(want),
    ) else {
        return want.to_string();
    };
    if !(held.is_object() && leaving.is_object() && want_value.is_object()) {
        return want.to_string();
    }
    let merged = merge_changed(&held, &leaving, &want_value);
    serde_json::to_string(&merged).unwrap_or_else(|_| want.to_string())
}

/// Move a whole snapshot one step. What folding the oldest steps away needs.
pub fn apply_to_snap(snap: &mut Snap, delta: &Delta, forward: bool) {
    // The two layouts have no recorded order of their own — a geometry list is
    // keyed by id and read back in the item list's order — so they are walked
    // with a scratch order that is thrown away.
    if let Some(part) = &delta.items {
        apply_keyed(&mut snap.items, &mut snap.item_order, part, forward);
    }
    if let Some(part) = &delta.desktop {
        let mut scratch = Vec::new();
        apply_keyed(&mut snap.desktop, &mut scratch, part, forward);
    }
    if let Some(part) = &delta.mobile {
        let mut scratch = Vec::new();
        apply_keyed(&mut snap.mobile, &mut scratch, part, forward);
    }
    if let Some(part) = &delta.trash {
        apply_keyed(&mut snap.trash, &mut snap.trash_order, part, forward);
    }
    for (field, pair) in &delta.rest {
        let want = side(pair, forward);
        let leaving = side(pair, !forward);
        let next = match snap.rest.get(field) {
            Some(held) => merge_text(held, &leaving, &want),
            None => want,
        };
        snap.rest.insert(field.clone(), next);
    }
}

// ---------------------------------------------------------------------------
// Hashing
// ---------------------------------------------------------------------------

/// FNV-1a over UTF-16 code units.
///
/// Not a cryptographic digest and not trying to be — the question it answers is
/// "is this the same board", and a mismatch only costs a history that will not
/// replay. The code units rather than the bytes are the original's
/// `charCodeAt`, and matching it is what lets the two implementations agree
/// about a board with an accent in its title.
pub fn fnv1a(parts: impl IntoIterator<Item = impl AsRef<str>>) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for part in parts {
        for unit in part.as_ref().encode_utf16() {
            hash ^= unit as u32;
            hash = hash.wrapping_mul(0x0100_0193);
        }
    }
    format!("{hash:x}")
}

/// A hash of a whole snapshot. For the tests, which compare a board reached one
/// way against the same board reached another.
///
/// **Not what the file's fingerprint is taken over** — see
/// [`crate::schema::doc_fingerprint`].
pub fn fingerprint(snap: &Snap) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for id in &snap.item_order {
        if let Some(text) = snap.items.get(id) {
            parts.push(text);
        }
    }
    for id in &snap.trash_order {
        if let Some(text) = snap.trash.get(id) {
            parts.push(text);
        }
    }
    parts.extend(snap.desktop.values().map(String::as_str));
    parts.extend(snap.mobile.values().map(String::as_str));
    parts.extend(snap.rest.values().map(String::as_str));
    fnv1a(parts)
}

/// A step id: unique within a board, and stable for a given clock and sequence.
fn step_id(seq: u64, now: i64, label: &str) -> String {
    format!("s{seq:x}-{}", fnv1a([label, &now.to_string(), &seq.to_string()]))
}

/// Every asset hash one recorded item names, on either shape it can arrive in.
fn eat_hashes(text: &str, out: &mut BTreeSet<String>) {
    let Ok(parsed) = serde_json::from_str::<Value>(text) else { return };
    // A bin entry is a wrapper; an item is itself.
    let item = match parsed.get("item").filter(|v| v.is_object()) {
        Some(inner) => inner,
        None => &parsed,
    };
    let Some(item) = item.as_object() else { return };
    let mut push = |v: Option<&Value>| {
        if let Some(s) = v.and_then(Value::as_str) {
            if crate::schema::is_hash(s) {
                out.insert(s.to_string());
            }
        }
    };
    push(item.get("asset").and_then(|a| a.get("hash")));
    let meta = item.get("meta");
    push(meta.and_then(|m| m.get("cover")));
    // The optimiser's memo of what it replaced, held for the same reason:
    // stepping back through a recompression has to put the original bytes back.
    push(meta.and_then(|m| m.get("was")));
    push(meta.and_then(|m| m.get("wasCover")));
}

// ---------------------------------------------------------------------------
// The file, in both directions
// ---------------------------------------------------------------------------

fn keyed_to_value(keyed: &Keyed) -> Value {
    Value::Object(
        keyed.iter().map(|(id, text)| (id.clone(), Value::String(text.clone()))).collect(),
    )
}

fn snap_to_value(snap: &Snap) -> Value {
    json!({
        "items": keyed_to_value(&snap.items),
        "itemOrder": snap.item_order,
        "desktop": keyed_to_value(&snap.desktop),
        "mobile": keyed_to_value(&snap.mobile),
        "trash": keyed_to_value(&snap.trash),
        "trashOrder": snap.trash_order,
        "rest": keyed_to_value(&snap.rest),
    })
}

/// A map of id to serialised text, as a file claims one.
///
/// `None` for anything else, all or nothing. Every value here is fed to a JSON
/// parser by a replay, so a number or a null among them is a scrub that leaves
/// the board half put back.
fn keyed_of_value(raw: Option<&Value>) -> Option<Keyed> {
    let src = raw?.as_object()?;
    let mut out = Keyed::new();
    for (id, text) in src {
        out.insert(id.clone(), text.as_str()?.to_string());
    }
    Some(out)
}

fn order_of_value(raw: Option<&Value>) -> Option<Vec<String>> {
    raw?.as_array()?.iter().map(|v| v.as_str().map(str::to_string)).collect()
}

fn snap_of_value(raw: Option<&Value>) -> Option<Snap> {
    let src = raw?.as_object()?;
    Some(Snap {
        items: keyed_of_value(src.get("items"))?,
        item_order: order_of_value(src.get("itemOrder"))?,
        desktop: keyed_of_value(src.get("desktop"))?,
        mobile: keyed_of_value(src.get("mobile"))?,
        trash: keyed_of_value(src.get("trash"))?,
        trash_order: order_of_value(src.get("trashOrder"))?,
        rest: keyed_of_value(src.get("rest"))?,
    })
}

fn keyed_delta_to_value(part: &KeyedDelta) -> Value {
    let changed: Map<String, Value> = part
        .changed
        .iter()
        .map(|(id, (before, after))| {
            let half = |v: &Option<String>| match v {
                Some(text) => Value::String(text.clone()),
                None => Value::Null,
            };
            (id.clone(), json!([half(before), half(after)]))
        })
        .collect();
    let mut out = Map::new();
    out.insert("changed".into(), Value::Object(changed));
    if let Some((before, after)) = &part.order {
        out.insert("order".into(), json!([before, after]));
    }
    Value::Object(out)
}

fn keyed_delta_of_value(raw: Option<&Value>) -> Option<KeyedDelta> {
    let src = raw?.as_object()?;
    let mut changed = BTreeMap::new();
    for (id, pair) in src.get("changed").and_then(Value::as_object)?.iter() {
        let Some(pair) = pair.as_array() else { continue };
        let half = |v: Option<&Value>| v.and_then(Value::as_str).map(str::to_string);
        changed.insert(id.clone(), (half(pair.first()), half(pair.get(1))));
    }
    let order = src
        .get("order")
        .and_then(Value::as_array)
        .and_then(|pair| Some((order_of_value(pair.first())?, order_of_value(pair.get(1))?)));
    let out = KeyedDelta { changed, order };
    (!out.is_empty()).then_some(out)
}

fn delta_to_value(delta: &Delta) -> Value {
    let mut out = Map::new();
    for name in Section::ALL {
        if let Some(part) = delta.section(name) {
            out.insert(name.as_str().into(), keyed_delta_to_value(part));
        }
    }
    if !delta.rest.is_empty() {
        let rest: Map<String, Value> =
            delta.rest.iter().map(|(field, (a, b))| (field.clone(), json!([a, b]))).collect();
        out.insert("rest".into(), Value::Object(rest));
    }
    for (key, value) in &delta.extra {
        out.entry(key.clone()).or_insert_with(|| value.clone());
    }
    Value::Object(out)
}

fn delta_of_value(raw: &Value) -> Option<Delta> {
    let src = raw.as_object()?;
    let mut out = Delta::default();
    for name in Section::ALL {
        out.set_section(name, keyed_delta_of_value(src.get(name.as_str())));
    }
    if let Some(rest) = src.get("rest").and_then(Value::as_object) {
        for (field, pair) in rest {
            let Some(pair) = pair.as_array() else { continue };
            let half = |v: Option<&Value>| {
                v.and_then(Value::as_str).map(str::to_string).unwrap_or_default()
            };
            out.rest.insert(field.clone(), (half(pair.first()), half(pair.get(1))));
        }
    }
    for (key, value) in src {
        if key == "rest" || Section::parse(key).is_some() {
            continue;
        }
        out.extra.insert(key.clone(), value.clone());
    }
    (!out.is_empty()).then_some(out)
}

fn step_to_value(step: &Step) -> Value {
    let mut out = Map::new();
    out.insert("id".into(), json!(step.id));
    out.insert("at".into(), json!(step.at));
    out.insert("label".into(), json!(step.label));
    out.insert("run".into(), json!(step.run));
    out.insert("delta".into(), delta_to_value(&step.delta));
    // Written only when there is one, so that an unnamed step and a step named
    // with nothing are the same bytes.
    if let Some(name) = &step.name {
        out.insert("name".into(), json!(name));
    }
    if let Some(op) = &step.op {
        out.insert("op".into(), op.clone());
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap_of(items: &[(&str, &str)]) -> Snap {
        Snap {
            items: items.iter().map(|(id, t)| (id.to_string(), t.to_string())).collect(),
            item_order: items.iter().map(|(id, _)| id.to_string()).collect(),
            ..Snap::default()
        }
    }

    #[test]
    fn a_delta_reverses_exactly() {
        let before = snap_of(&[("a", r#"{"id":"a","x":0}"#)]);
        let after = snap_of(&[("a", r#"{"id":"a","x":10}"#)]);
        let delta = diff(&before, &after).expect("something changed");
        let mut walked = before.clone();
        apply_to_snap(&mut walked, &delta, true);
        assert_eq!(walked.items, after.items);
        apply_to_snap(&mut walked, &delta, false);
        assert_eq!(walked.items, before.items);
    }

    #[test]
    fn a_delta_writes_only_the_fields_it_changed() {
        // The rule the whole module turns on: a step that moved a card must not
        // assert the name it happened to have at the time.
        let before = snap_of(&[("a", r#"{"id":"a","x":0,"name":"old"}"#)]);
        let after = snap_of(&[("a", r#"{"id":"a","x":10,"name":"old"}"#)]);
        let delta = diff(&before, &after).expect("something changed");

        let mut renamed = snap_of(&[("a", r#"{"id":"a","x":0,"name":"new"}"#)]);
        apply_to_snap(&mut renamed, &delta, true);
        let held: Value = serde_json::from_str(&renamed.items["a"]).unwrap();
        assert_eq!(held["x"], json!(10), "the move applied");
        assert_eq!(held["name"], json!("new"), "the rename survived it");
    }

    #[test]
    fn a_value_with_no_fields_is_written_whole_rather_than_merged() {
        // Half the fields a step records are not objects, and "the fields that
        // changed" is not a question a string can answer. Merging one anyway
        // emptied it — a step that renamed a board undid to `""`.
        assert_eq!(merge_text(r#""renamed""#, r#""renamed""#, r#""Kitchen""#), r#""Kitchen""#);
        assert_eq!(merge_text("true", "true", "false"), "false");
        assert_eq!(merge_text("[1,2]", "[1,2]", "[3]"), "[3]");
        // An object still merges field by field, which is the whole point.
        assert_eq!(
            merge_text(r#"{"a":9,"b":2}"#, r#"{"a":1,"b":2}"#, r#"{"a":1,"b":5}"#),
            r#"{"a":9,"b":5}"#,
        );
    }

    #[test]
    fn identical_intentions_merge_and_different_ones_do_not() {
        let a = snap_of(&[("k", r#"{"id":"k","x":0,"name":"n"}"#)]);
        let b = snap_of(&[("k", r#"{"id":"k","x":1,"name":"n"}"#)]);
        let c = snap_of(&[("k", r#"{"id":"k","x":2,"name":"n"}"#)]);
        let d = snap_of(&[("k", r#"{"id":"k","x":2,"name":"m"}"#)]);

        let mut ledger = Timeline::starting_at(a.clone());
        assert!(ledger.record("Nudge", diff(&a, &b).unwrap(), 1));
        assert!(ledger.record("Nudge", diff(&b, &c).unwrap(), 2));
        assert_eq!(ledger.steps().len(), 1, "two nudges are one step");
        assert!(ledger.record("Rename", diff(&c, &d).unwrap(), 3));
        assert_eq!(ledger.steps().len(), 2, "a rename is a second intention");
    }

    #[test]
    fn a_run_that_folds_back_to_nothing_leaves_no_step() {
        let a = snap_of(&[("k", r#"{"id":"k","x":0}"#)]);
        let b = snap_of(&[("k", r#"{"id":"k","x":1}"#)]);
        let mut ledger = Timeline::starting_at(a.clone());
        ledger.record("Nudge", diff(&a, &b).unwrap(), 1);
        ledger.record("Nudge", diff(&b, &a).unwrap(), 2);
        assert!(ledger.is_empty(), "moved it and moved it back is not a step");
        assert_eq!(ledger.at(), 0);
    }

    #[test]
    fn an_arriving_card_never_joins_a_run() {
        let a = snap_of(&[("k", r#"{"id":"k","x":0}"#)]);
        let b = snap_of(&[("k", r#"{"id":"k","x":0}"#), ("j", r#"{"id":"j","x":0}"#)]);
        let c = snap_of(&[
            ("k", r#"{"id":"k","x":0}"#),
            ("j", r#"{"id":"j","x":0}"#),
            ("i", r#"{"id":"i"}"#),
        ]);
        let mut ledger = Timeline::starting_at(a.clone());
        ledger.record("Add", diff(&a, &b).unwrap(), 1);
        ledger.record("Add", diff(&b, &c).unwrap(), 2);
        assert_eq!(ledger.steps().len(), 2, "collapsing adds would hide one");
    }

    #[test]
    fn a_ledger_survives_a_round_trip_through_a_file() {
        let a = snap_of(&[("k", r#"{"id":"k","x":0,"name":"n"}"#)]);
        let b = snap_of(&[("k", r#"{"id":"k","x":9,"name":"n"}"#)]);
        let mut ledger = Timeline::starting_at(a.clone());
        ledger.record("Move", diff(&a, &b).unwrap(), 1_755_180_000_000);
        ledger.name_step(0, Some("before the change"));

        let filed = ledger.to_value("3f9a21c4").expect("a ledger with a step is written");
        let back = Timeline::adopt(Some(&filed), Some("3f9a21c4"), Snap::default());

        assert!(!back.stale());
        assert_eq!(back.at(), 1);
        assert_eq!(back.base(), ledger.base());
        assert_eq!(back.steps().len(), 1);
        assert_eq!(back.steps()[0].id, ledger.steps()[0].id);
        assert_eq!(back.steps()[0].label, "Move");
        assert_eq!(back.steps()[0].name.as_deref(), Some("before the change"));
        assert_eq!(back.steps()[0].delta, ledger.steps()[0].delta);
    }

    #[test]
    fn a_ledger_that_describes_another_board_is_marked_stale() {
        let a = snap_of(&[("k", r#"{"id":"k","x":0}"#)]);
        let b = snap_of(&[("k", r#"{"id":"k","x":9}"#)]);
        let mut ledger = Timeline::starting_at(a.clone());
        ledger.record("Move", diff(&a, &b).unwrap(), 1);
        let filed = ledger.to_value("aaaa").unwrap();

        assert!(Timeline::adopt(Some(&filed), Some("bbbb"), Snap::default()).stale());
        // No fingerprint to check against is not a reason to disbelieve it.
        assert!(!Timeline::adopt(Some(&filed), None, Snap::default()).stale());
    }

    #[test]
    fn a_board_nobody_changed_writes_no_ledger_at_all() {
        assert!(Timeline::default().to_value("x").is_none());
    }

    #[test]
    fn a_section_this_build_does_not_know_survives_the_trip() {
        let filed = json!({
            "base": snap_to_value(&Snap::default()),
            "at": 1,
            "steps": [{
                "id": "s1", "at": 0, "label": "Change", "run": "",
                "delta": { "fences": { "changed": { "f1": ["a", "b"] } } },
            }],
        });
        let back = Timeline::adopt(Some(&filed), None, Snap::default());
        assert_eq!(back.steps().len(), 1);
        let out = back.to_value("").unwrap();
        assert_eq!(out["steps"][0]["delta"]["fences"]["changed"]["f1"], json!(["a", "b"]));
    }

    #[test]
    fn the_fingerprint_matches_the_original_implementation() {
        // FNV-1a over UTF-16 units, which is what `charCodeAt` gives the
        // original. Checked against a known answer so the two builds cannot
        // drift apart without a test saying so.
        assert_eq!(fnv1a(["mbrd"]), "4d1255d0");
        assert_eq!(fnv1a([""]), "811c9dc5");
    }
}
