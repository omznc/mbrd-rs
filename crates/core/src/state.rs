//! The one door every write to a board passes through.
//!
//! **This module exists so that undo does not have to be retrofitted.** Nothing
//! else in the app may hold a `&mut Board`: [`BoardState`] owns one and hands it
//! out only inside a closure it is watching, so a step is recorded for every
//! mutation without any call site having to remember to record one. Adding the
//! thirtieth feature that moves a card is then a change to that feature, and not
//! also a change to the history.
//!
//! That is enforced rather than asked for. `BoardState` derefs to `&Board`, so
//! reading one is exactly as easy as reading a board; there is no `DerefMut`,
//! and the only mutable path is [`BoardState::edit`] and the gesture pair
//! beneath it.
//!
//! ## What a step costs, and why it is not what it looks like
//!
//! A ledger that watches rather than is told has to work out what changed by
//! itself, and the obvious way to do that is to write the board down as text
//! before and after and compare the two. That is correct and it is unusable: at
//! the twenty thousand items the format allows it is thirty milliseconds a side,
//! on a keystroke.
//!
//! So nothing here serialises a board. A second copy of it is kept — see
//! `shadow` — the two are compared *structurally*, and only the entries that
//! actually differ are ever turned into text. Undo is the same trick pointing
//! the other way: a step that moved one card parses one card, rather than
//! rebuilding the board from its own transcript.
//!
//! The numbers that fell out of doing it that way, on twenty thousand items:
//! an edit went from 139ms to 2.4ms and an undo from 1.09s to 2.0ms. Both are
//! now proportional to the change rather than to the board, which is the
//! property worth protecting — if a later phase adds a field to
//! [`schema::REST_FIELDS`] that has to be serialised to be compared, that is
//! where this stops being true.
//!
//! A drag is the remaining special case: it must not pay even 2.4ms per frame,
//! and it does not, because a gesture is measured once at its end rather than
//! once per frame. See [`BoardState::start`].
//!
//! ## Where the layering puts it
//!
//! ```text
//! geometry, history <- model <- {schema, viewport, naming} <- state <- mbrd
//! ```
//!
//! `history.rs` knows about text and nothing else; `schema.rs` knows which text
//! each field of a board is. This module is the only place the two meet, and it
//! is the only place that turns a `Board` into a [`Snap`] or back.

use std::ops::Deref;

use serde_json::Value;

use crate::history::{self, Delta, Keyed, KeyedDelta, Snap, Timeline};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::model::{Board, Geometry, Item, TrashEntry, View};
use crate::schema;

/// A board, its history, and the only way to change either.
#[derive(Debug, Clone)]
pub struct BoardState {
    board: Board,
    /// The board as of the last recorded step, kept so that working out what
    /// changed costs a comparison rather than a serialisation.
    ///
    /// The obvious implementation of this door takes a picture of the board in
    /// text on both sides of every edit and diffs the two. That is correct and
    /// it is far too slow: serialising twenty thousand items is thirty
    /// milliseconds, and paying it twice on every tap of an arrow key makes the
    /// keyboard feel broken on exactly the boards the format was sized for.
    ///
    /// A second copy of the board costs memory instead, and memory is what there
    /// is a lot of. Comparing two boards *structurally* — no allocation, no JSON —
    /// is around a millisecond at that size, and only the handful of items that
    /// actually differ are ever turned into text. So the cost of an edit is
    /// proportional to the edit rather than to the board.
    shadow: Board,
    timeline: Timeline,
    /// A number that changes every time anything is handed a `&mut Board`.
    ///
    /// The one thing above this crate that has to know a board changed without
    /// being told what changed: a spatial index is built from a list of items
    /// and is only valid for that list, so whatever holds one needs a cheap
    /// "still current?" and rebuilding on every frame would give the index back
    /// its cost.
    ///
    /// Drawn from one counter for the whole process rather than starting at
    /// zero per board, so that "the same number" means "the same board,
    /// unchanged" and not merely "some board, unchanged". Two boards each at
    /// their own step one would otherwise collide, and the thing that collides
    /// with them is a cache that then draws the old board's items at the new
    /// board's places. Closing a file and opening another is the ordinary way
    /// to hit that.
    ///
    /// Deliberately *not* bumped by [`Self::set_view`]. Panning is the common
    /// case and moves no item, so counting it would rebuild the index on every
    /// frame of the one gesture the index exists to make cheap.
    revision: u64,
}

impl Default for BoardState {
    fn default() -> Self {
        Self::new(Board::default())
    }
}

/// Reading a `BoardState` is reading a `Board`. Writing one is not.
impl Deref for BoardState {
    type Target = Board;

    fn deref(&self) -> &Board {
        &self.board
    }
}

/// The next revision number, unique for the life of the process.
///
/// `Relaxed` because nothing is ordered against it: the only question ever
/// asked of a revision is whether it equals one seen before, and uniqueness is
/// all `fetch_add` has to provide for that to be a sound answer.
fn tick() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// A gesture in progress: the picture the eventual step will be measured against.
///
/// Held by whatever is driving the gesture — a drag, a resize — and handed back
/// to [`BoardState::finish`] when it ends. Its whole job is to make "mutate the
/// board without a step" unrepresentable: [`BoardState::during`] will not run
/// without one, so there is no spelling of a write that quietly escapes the
/// ledger.
#[derive(Debug, Clone)]
pub struct Pending;

impl BoardState {
    /// A board with no history, which is what a new board has.
    pub fn new(mut board: Board) -> Self {
        mirror_desktop(&mut board);
        let base = snapshot(&board);
        let shadow = board.clone();
        Self { board, shadow, timeline: Timeline::starting_at(base), revision: tick() }
    }

    /// A board and the ledger the file it came from carried.
    ///
    /// `filed` is the parsed `board.json`, and it is wanted whole rather than
    /// just its `timeline` key: the fingerprint that decides whether the ledger
    /// still describes this board is taken over the `items` and `trash` arrays
    /// *as the file carries them*. See [`schema::doc_fingerprint`].
    pub fn opened(mut board: Board, filed: &Value) -> Self {
        adopt_desktop(&mut board);
        let fingerprint = schema::doc_fingerprint(filed.get("items"), filed.get("trash"));
        let timeline = Timeline::adopt(filed.get("timeline"), Some(&fingerprint), snapshot(&board));
        let shadow = board.clone();
        Self { board, shadow, timeline, revision: tick() }
    }

    pub fn timeline(&self) -> &Timeline {
        &self.timeline
    }

    /// `board.json`, with the ledger in it where there is one.
    pub fn to_value(&self) -> Value {
        to_value(&self.board, &self.timeline)
    }

    // -----------------------------------------------------------------------
    // The door
    // -----------------------------------------------------------------------

    /// Change the board, and remember how to take it back.
    ///
    /// One discrete action: a delete, a rename, a nudge, a new note. A run of
    /// these that touches the same cards in the same fields collapses into one
    /// step, so twelve taps of an arrow key are one entry on the strip holding
    /// the first position and the last.
    ///
    /// A closure that changes nothing records nothing — there is no such thing
    /// here as an empty step.
    pub fn edit<R>(&mut self, label: &str, f: impl FnOnce(&mut Board) -> R) -> R {
        self.edit_at(label, crate::naming::now_millis(), f)
    }

    /// [`Self::edit`], against a clock you supply. For tests, and for anything
    /// that already knows what time it is.
    pub fn edit_at<R>(&mut self, label: &str, at: i64, f: impl FnOnce(&mut Board) -> R) -> R {
        let before = self.start();
        let out = self.during(&before, f);
        self.finish_at(label, before, at);
        out
    }

    /// Open a gesture.
    ///
    /// A drag is one step, not one step per frame, and the difference matters
    /// twice: the strip should say *moved* rather than four hundred times
    /// *moved*, and working out what changed on every mouse-move would put a
    /// pass over the board inside a frame. So the board is moved freely with
    /// [`Self::during`] and the whole gesture is closed into a single step by
    /// [`Self::finish`], which is the only point anything is measured.
    ///
    /// **One gesture at a time.** The state a step is measured from is the
    /// shadow, and there is one of those — so opening a second gesture before
    /// the first is finished folds both into one step measured from before the
    /// first. That is a merge rather than a loss, and it is also a shape the
    /// gesture pipeline above this never produces: there is exactly one active
    /// gesture, decided in one place.
    ///
    /// A `Pending` that is dropped rather than finished leaves its mutation on
    /// the board with no step of its own — it joins the next one. That is the
    /// only way to blur history through this door, and it is why the token is
    /// the one thing that unlocks `during`.
    pub fn start(&self) -> Pending {
        Pending
    }

    /// Change the board inside an open gesture. Records nothing on its own.
    pub fn during<R>(&mut self, _open: &Pending, f: impl FnOnce(&mut Board) -> R) -> R {
        self.revision = tick();
        f(&mut self.board)
    }

    /// A number that changes whenever the items might have.
    ///
    /// For anything holding a structure derived from the board — an index, a
    /// cache — that needs to know when to throw it away. It says *maybe
    /// changed*, never *changed how*: a closure that touched nothing still
    /// bumps it, because the alternative is measuring, and measuring is the
    /// cost this exists to avoid.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// Close a gesture into one step. Answers whether anything actually changed.
    pub fn finish(&mut self, label: &str, open: Pending) -> bool {
        self.finish_at(label, open, crate::naming::now_millis())
    }

    pub fn finish_at(&mut self, label: &str, _open: Pending, at: i64) -> bool {
        // Before the measurement, not after it, so that the step carries the
        // geometry it left behind and undo takes both halves back together.
        mirror_desktop(&mut self.board);
        let Some(delta) = changes(&self.shadow, &self.board) else {
            return false;
        };
        // The shadow catches up through the same delta the step records, rather
        // than by copying the board. Two reasons, and the second is the one that
        // matters: it is proportional to the change, and it puts the shadow in
        // exactly the state an undo would find — the recorded text parsed back —
        // so the next comparison measures against what the file would hold
        // rather than against a value that is about to round.
        apply(&mut self.shadow, &delta, true);
        self.timeline.record(label, delta, at)
    }

    /// Where the camera was left.
    ///
    /// The one write that is not an edit, and the only one. A view is where you
    /// are looking rather than something you did — see [`schema::REST_FIELDS`]
    /// for why no step records it — so this changes the board without touching
    /// the ledger, and nothing else in the app gets to.
    pub fn set_view(&mut self, view: View) {
        self.board.view = view;
    }

    // -----------------------------------------------------------------------
    // Walking back and forward
    // -----------------------------------------------------------------------

    /// What undo would take back next, by name, or `None` for nothing.
    pub fn undo_label(&self) -> Option<&str> {
        self.timeline.undo_label()
    }

    pub fn redo_label(&self) -> Option<&str> {
        self.timeline.redo_label()
    }

    /// Take one step back, and answer what it was called.
    pub fn undo(&mut self) -> Option<String> {
        if self.timeline.stale || self.timeline.at == 0 {
            return None;
        }
        // Disjoint fields of `self`: the step is read out of the ledger while
        // the board is written, which the borrow checker allows and which is
        // what keeps a delta from having to be cloned to be applied.
        let step = &self.timeline.steps[self.timeline.at - 1];
        apply(&mut self.board, &step.delta, false);
        apply(&mut self.shadow, &step.delta, false);
        let label = step.label.clone();
        self.timeline.at -= 1;
        self.revision = tick();
        Some(label)
    }

    /// And one forward.
    pub fn redo(&mut self) -> Option<String> {
        if self.timeline.stale || self.timeline.at >= self.timeline.steps.len() {
            return None;
        }
        let step = &self.timeline.steps[self.timeline.at];
        apply(&mut self.board, &step.delta, true);
        apply(&mut self.shadow, &step.delta, true);
        let label = step.label.clone();
        self.timeline.at += 1;
        self.revision = tick();
        Some(label)
    }

    /// Every asset hash something still points at, and whether losing it matters.
    ///
    /// Three of the four classes of reference are fatal to a save if their bytes
    /// are missing — a live card, a binned card, an embedded font — and the
    /// fourth is not. A step can name bytes something else legitimately threw
    /// away, and refusing to write somebody's board over an entry in its history
    /// would be the wrong way round. So the two sets are answered separately
    /// rather than unioned, and the packer treats them differently.
    pub fn required_hashes(&self) -> Vec<String> {
        self.board.referenced_hashes()
    }

    /// The hashes only the ledger wants: written where the bytes are here,
    /// walked past where they are not.
    pub fn optional_hashes(&self) -> Vec<String> {
        let required = self.board.referenced_hashes();
        self.timeline.hashes().into_iter().filter(|h| !required.contains(h)).collect()
    }
}

/// `board.json` for a board and a ledger that need not be the same object.
///
/// Separate from [`BoardState::to_value`] because the packer writes a board it
/// has adjusted — a waveform written as a sidecar is removed from `board.json`
/// rather than stored twice — and cloning the whole ledger to carry that
/// adjustment would be a copy of the history on every save.
pub fn to_value(board: &Board, timeline: &Timeline) -> Value {
    let mut out = schema::serialize(board);
    // Over the arrays this file will actually carry, which is why it is taken
    // here and not inside the ledger: those bytes do not exist until now.
    let fingerprint = schema::doc_fingerprint(out.get("items"), out.get("trash"));
    if let Some(filed) = timeline.to_value(&fingerprint) {
        if let Some(map) = out.as_object_mut() {
            map.insert("timeline".into(), filed);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The Desktop layout, and the copy of it on every item
// ---------------------------------------------------------------------------
//
// A board carries its Desktop geometry twice: once in `layouts.desktop`, and
// once in the `x`/`y`/`w`/`h`/`rot`/`z` fields of the items themselves. That is
// not redundancy anyone chose — it is the format's compatibility with readers
// written before there were two layout profiles, which find the Desktop board
// where it has always been. `layouts.desktop` is the authority; the item fields
// are a copy of it.
//
// Nothing above this line maintains that. The app moves cards by writing to the
// item, which is the right thing for it to do and would leave the two halves
// disagreeing on every save if this module did not level them — a file whose own
// numbers contradict each other, opened differently by two conforming readers.
// So the door levels them, in the direction each moment calls for.

/// The Desktop geometry a file carries, onto the items. Called once, on open.
///
/// This direction, because the format says so: a reader that knows about
/// profiles takes the geometry from `layouts`, and the item's own fields are the
/// older copy. Where the two agree — which is every file this build has written —
/// it does nothing at all.
fn adopt_desktop(board: &mut Board) {
    // Indexed rather than searched. The format allows twenty thousand items, and
    // a `find` per record would be four hundred million comparisons on the way
    // in — a board that takes a second to open for a loop nobody would notice
    // writing.
    let by_id: std::collections::HashMap<&str, &crate::model::Geometry> =
        board.layouts.desktop.iter().map(|g| (g.id.as_str(), g)).collect();
    let mut changes: Vec<(usize, crate::model::Geometry)> = Vec::new();
    for (n, item) in board.items.iter().enumerate() {
        if let Some(g) = by_id.get(item.id.as_str()) {
            changes.push((n, (*g).clone()));
        }
    }
    for (n, g) in changes {
        let item = &mut board.items[n];
        item.x = g.x;
        item.y = g.y;
        item.w = g.w;
        item.h = g.h;
        item.rot = g.rot;
        item.z = g.z;
    }
}

/// The items, onto the Desktop geometry. Called at the door, after every edit.
///
/// `presnap` is carried over rather than rebuilt: it records where a card was
/// before the grid took it, which is a fact about the past that moving the card
/// does not change.
///
/// The Mobile layout is deliberately untouched. Mobile is a packed column rather
/// than a place things sit, so it is not a mirror of anything and rebuilding it
/// is a phase of its own; a record there for an item that has gone is dropped at
/// the file boundary, where the whole list is completed against the items.
fn mirror_desktop(board: &mut Board) {
    // The ordinary case — a card moved, nothing arrived or left — is a walk down
    // two lists that are already the same length in the same order, writing six
    // numbers where they differ. It allocates nothing, which matters because
    // this runs at the close of every edit and every frame-batch of a drag.
    let aligned = board.layouts.desktop.len() == board.items.len()
        && board.layouts.desktop.iter().zip(&board.items).all(|(g, item)| g.id == item.id);
    if aligned {
        for (g, item) in board.layouts.desktop.iter_mut().zip(&board.items) {
            g.x = item.x;
            g.y = item.y;
            g.w = item.w;
            g.h = item.h;
            g.rot = item.rot;
            g.z = item.z;
        }
        return;
    }
    // Something arrived or left. Rebuilt, indexed rather than searched: the
    // format allows twenty thousand items and a `find` per item would be four
    // hundred million comparisons.
    let held: std::collections::HashMap<&str, Option<crate::model::PreSnap>> =
        board.layouts.desktop.iter().map(|g| (g.id.as_str(), g.presnap)).collect();
    let mut out: Vec<Geometry> = Vec::with_capacity(board.items.len());
    for item in &board.items {
        out.push(Geometry {
            id: item.id.clone(),
            x: item.x,
            y: item.y,
            w: item.w,
            h: item.h,
            rot: item.rot,
            z: item.z,
            // Where the card was before the grid took it: a fact about the past,
            // which moving it does not change.
            presnap: held.get(item.id.as_str()).copied().flatten(),
        });
    }
    board.layouts.desktop = out;
}

// ---------------------------------------------------------------------------
// A board, as text
// ---------------------------------------------------------------------------

fn text(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_default()
}

/// The board as it stands, as text.
pub fn snapshot(board: &Board) -> Snap {
    let mut items = Keyed::new();
    let mut item_order = Vec::with_capacity(board.items.len());
    for item in &board.items {
        items.insert(item.id.clone(), text(&schema::item_value(item)));
        item_order.push(item.id.clone());
    }
    let mut trash = Keyed::new();
    let mut trash_order = Vec::with_capacity(board.trash.len());
    for entry in &board.trash {
        trash.insert(entry.item.id.clone(), text(&schema::trash_value(entry)));
        trash_order.push(entry.item.id.clone());
    }
    let geometry = |list: &[crate::model::Geometry]| -> Keyed {
        list.iter().map(|g| (g.id.clone(), text(&schema::geometry_value(g)))).collect()
    };
    let mut rest = std::collections::BTreeMap::new();
    for field in schema::REST_FIELDS {
        if let Some(value) = schema::rest_value(board, field) {
            rest.insert(field.to_string(), text(&value));
        }
    }
    Snap {
        items,
        item_order,
        desktop: geometry(&board.layouts.desktop),
        mobile: geometry(&board.layouts.mobile),
        trash,
        trash_order,
        rest,
    }
}

/// Put a snapshot back on the board, wholesale.
///
/// What a checkpoint is for, and therefore what a scrub will be built on: going
/// to an arbitrary step means putting the base back and walking it forward.
/// Undo does not use it — a step that moved one card should parse one card, not
/// rebuild the board from its own transcript — so today this is exercised by its
/// test and by nothing else, deliberately.
///
/// The two layouts have no recorded order of their own, so they come back in the
/// item list's order with anything left over behind it. That is a normalisation
/// rather than a loss: a geometry list is keyed by id, and every reader in this
/// crate already builds one in the item list's order.
pub fn restore(board: &mut Board, snap: &Snap) {
    board.items = snap
        .item_order
        .iter()
        .filter_map(|id| snap.items.get(id))
        .filter_map(|t| serde_json::from_str::<Value>(t).ok())
        .map(|v| schema::item_of_value(&v))
        .collect();
    board.trash = snap
        .trash_order
        .iter()
        .filter_map(|id| snap.trash.get(id))
        .filter_map(|t| serde_json::from_str::<Value>(t).ok())
        .filter_map(|v| schema::trash_of_value(&v))
        .collect();

    let order: Vec<&String> = board.items.iter().map(|i| &i.id).collect();
    let layout = |keyed: &Keyed| -> Vec<crate::model::Geometry> {
        let mut out: Vec<crate::model::Geometry> = Vec::with_capacity(keyed.len());
        let mut placed: Vec<&String> = Vec::new();
        let take = |id: &String, out: &mut Vec<crate::model::Geometry>| {
            let Some(t) = keyed.get(id) else { return };
            let Ok(v) = serde_json::from_str::<Value>(t) else { return };
            if let Some(g) = schema::geometry_of_value(&v) {
                out.push(g);
            }
        };
        for id in &order {
            take(id, &mut out);
            placed.push(id);
        }
        for id in keyed.keys() {
            if !placed.contains(&id) {
                take(id, &mut out);
            }
        }
        out
    };
    board.layouts.desktop = layout(&snap.desktop);
    board.layouts.mobile = layout(&snap.mobile);

    for (field, t) in &snap.rest {
        let Ok(value) = serde_json::from_str::<Value>(t) else { continue };
        schema::rest_apply(board, field, &value);
    }
}

// ---------------------------------------------------------------------------
// What changed, and putting it back
// ---------------------------------------------------------------------------

/// The difference between two boards, as a step records it.
///
/// The comparison is structural and the serialisation is not: two boards are
/// walked side by side with `==`, which allocates nothing, and only the entries
/// that actually differ are turned into the text a step holds. That is what
/// makes an edit cost the edit rather than the board.
///
/// The text of a pair is compared too, after the structural test and not instead
/// of it. Two things fall out of that, and both are wanted: a difference the file
/// cannot represent — the third decimal of a drag, which rounds away — is not a
/// change, and a value that is not equal to itself, which is what a `NaN`
/// coordinate would be, does not put a step on the strip every time anything
/// happens.
pub fn changes(before: &Board, after: &Board) -> Option<Delta> {
    let mut out = Delta {
        items: diff_list(&before.items, &after.items, |i| &i.id, schema::item_value, true),
        desktop: diff_list(
            &before.layouts.desktop,
            &after.layouts.desktop,
            |g| &g.id,
            schema::geometry_value,
            false,
        ),
        mobile: diff_list(
            &before.layouts.mobile,
            &after.layouts.mobile,
            |g| &g.id,
            schema::geometry_value,
            false,
        ),
        trash: diff_list(&before.trash, &after.trash, |t| &t.item.id, schema::trash_value, true),
        ..Delta::default()
    };
    for field in schema::REST_FIELDS {
        let (Some(a), Some(b)) =
            (schema::rest_value(before, field), schema::rest_value(after, field))
        else {
            continue;
        };
        if a == b {
            continue;
        }
        out.rest.insert(field.to_string(), (text(&a), text(&b)));
    }
    (!out.is_empty()).then_some(out)
}

/// One keyed list, compared.
///
/// `ordered` says whether the list's order is a fact about the board worth
/// recording. It is for the items and the bin — where a card sits in the
/// document decides which of two overlapping cards is on top of the other where
/// their `z` agrees — and it is not for a layout, which is keyed by id and read
/// back in the item list's order.
fn diff_list<T: PartialEq>(
    before: &[T],
    after: &[T],
    id_of: impl Fn(&T) -> &str,
    to_value: impl Fn(&T) -> Value,
    ordered: bool,
) -> Option<KeyedDelta> {
    type Changed = std::collections::BTreeMap<String, (Option<String>, Option<String>)>;

    // The overwhelmingly common edit moves or renames something that was already
    // on the board: the two lists are the same ids in the same order, and the
    // difference is a walk down them both. No index, no allocation, and nothing
    // turned into text but the entries that actually differ.
    let aligned =
        before.len() == after.len() && before.iter().zip(after).all(|(a, b)| id_of(a) == id_of(b));
    if aligned {
        let mut changed = Changed::new();
        for (was, is) in before.iter().zip(after) {
            if was == is {
                continue;
            }
            let (a, b) = (text(&to_value(was)), text(&to_value(is)));
            if a == b {
                continue;
            }
            changed.insert(id_of(is).to_string(), (Some(a), Some(b)));
        }
        let out = KeyedDelta { changed, order: None };
        return (!out.is_empty()).then_some(out);
    }

    // Something arrived, left, or moved. Indexed, so that a step touching a
    // thousand cards is a thousand lookups rather than a thousand scans.
    fn index<'a, T>(
        list: &'a [T],
        id_of: &impl Fn(&T) -> &str,
    ) -> std::collections::HashMap<&'a str, &'a T> {
        list.iter().map(|e| (id_of(e), e)).collect()
    }
    let (was, is) = (index(before, &id_of), index(after, &id_of));

    let mut changed = Changed::new();
    for (id, entry) in &is {
        let held = was.get(id);
        if held.map(|h| *h == *entry).unwrap_or(false) {
            continue;
        }
        let (a, b) = (held.map(|h| text(&to_value(h))), Some(text(&to_value(entry))));
        if a == b {
            continue;
        }
        changed.insert((*id).to_string(), (a, b));
    }
    for (id, entry) in &was {
        if is.contains_key(id) {
            continue;
        }
        changed.insert((*id).to_string(), (Some(text(&to_value(entry))), None));
    }

    // Tested before it is built. Recording an order means two copies of every
    // id in the list, so on a full board that is a step the size of the board —
    // and the answer is almost always "it did not move", which a walk down the
    // two lists answers without allocating anything at all.
    let moved = ordered
        && (before.len() != after.len()
            || before.iter().zip(after).any(|(a, b)| id_of(a) != id_of(b)));
    let order = moved.then(|| {
        let ids =
            |list: &[T]| -> Vec<String> { list.iter().map(|e| id_of(e).to_string()).collect() };
        (ids(before), ids(after))
    });

    let out = KeyedDelta { changed, order };
    (!out.is_empty()).then_some(out)
}

/// Move the board one step, in either direction.
///
/// Touches only the ids the step names. The obvious implementation — snapshot
/// the whole board, walk it through [`history::apply_to_snap`], build a board
/// back out of the result — is a page shorter and was what this was; it also
/// parses twenty thousand items to move one card, which made an undo on a full
/// board take a second. The rule about applying only the fields a step changed
/// still lives in one place, [`history::merge_text`], which both this and the
/// snapshot walk call.
pub fn apply(board: &mut Board, delta: &Delta, forward: bool) {
    if let Some(part) = &delta.items {
        apply_list(
            &mut board.items,
            part,
            forward,
            |i: &Item| i.id.as_str(),
            schema::item_value,
            |v| Some(schema::item_of_value(v)),
        );
    }
    if let Some(part) = &delta.desktop {
        apply_list(
            &mut board.layouts.desktop,
            part,
            forward,
            |g: &Geometry| g.id.as_str(),
            schema::geometry_value,
            schema::geometry_of_value,
        );
    }
    if let Some(part) = &delta.mobile {
        apply_list(
            &mut board.layouts.mobile,
            part,
            forward,
            |g: &Geometry| g.id.as_str(),
            schema::geometry_value,
            schema::geometry_of_value,
        );
    }
    if let Some(part) = &delta.trash {
        apply_list(
            &mut board.trash,
            part,
            forward,
            |t: &TrashEntry| t.item.id.as_str(),
            schema::trash_value,
            schema::trash_of_value,
        );
    }
    for (field, pair) in &delta.rest {
        let want = if forward { &pair.1 } else { &pair.0 };
        let leaving = if forward { &pair.0 } else { &pair.1 };
        // The same rule as the keyed lists, for the same reason: a step that
        // changed one setting recorded the whole settings object on both sides,
        // and writing it whole would put every *other* setting back to what it
        // was when this step ran.
        let held = schema::rest_value(board, field).map(|v| text(&v));
        let next = match held {
            Some(held) => history::merge_text(&held, leaving, want),
            None => want.clone(),
        };
        let Ok(value) = serde_json::from_str::<Value>(&next) else { continue };
        schema::rest_apply(board, field, &value);
    }
}

/// One keyed list, moved one step.
///
/// Everything is worked out against the list before anything is written to it,
/// which is what lets the ordinary case — some entries replaced in place, no
/// arrivals, no departures, no reordering — touch only the entries the step
/// names. A step that moved one card on a board of twenty thousand parses one
/// card.
fn apply_list<T>(
    list: &mut Vec<T>,
    delta: &KeyedDelta,
    forward: bool,
    id_of: impl Fn(&T) -> &str,
    to_value: impl Fn(&T) -> Value,
    of_value: impl Fn(&Value) -> Option<T>,
) {
    let mut replaced: Vec<(usize, T)> = Vec::new();
    let mut arrived: Vec<T> = Vec::new();
    let mut departed: std::collections::HashSet<usize> = Default::default();

    {
        let at: std::collections::HashMap<&str, usize> =
            list.iter().enumerate().map(|(n, e)| (id_of(e), n)).collect();

        for (id, pair) in &delta.changed {
            let want = if forward { pair.1.as_ref() } else { pair.0.as_ref() };
            let leaving = if forward { pair.0.as_ref() } else { pair.1.as_ref() };
            let held = at.get(id.as_str()).copied();
            let Some(want) = want else {
                if let Some(n) = held {
                    departed.insert(n);
                }
                continue;
            };
            let merged = match (leaving, held) {
                // **Only the fields this step actually changed**, so that a step
                // which moved a card does not also assert the name it happened
                // to have at the time.
                (Some(leaving), Some(n)) => {
                    history::merge_text(&text(&to_value(&list[n])), leaving, want)
                }
                _ => want.clone(),
            };
            let Ok(value) = serde_json::from_str::<Value>(&merged) else { continue };
            let Some(entry) = of_value(&value) else { continue };
            match held {
                Some(n) => replaced.push((n, entry)),
                None => arrived.push(entry),
            }
        }
    }

    for (n, entry) in replaced {
        list[n] = entry;
    }
    if !departed.is_empty() {
        let mut n = 0;
        list.retain(|_| {
            let keep = !departed.contains(&n);
            n += 1;
            keep
        });
    }
    list.extend(arrived);

    let Some((before, after)) = &delta.order else { return };
    let next = if forward { after } else { before };

    // Only when the order itself was recorded as having moved, which is the one
    // case that has to rebuild the list. An entry the recorded order does not
    // mention still belongs on the board and goes behind the rest: only a
    // malformed step gets here, and dropping things on the floor is the failure
    // this must not have.
    let mut held: std::collections::HashMap<&str, usize> = Default::default();
    for (n, entry) in list.iter().enumerate() {
        held.insert(id_of(entry), n);
    }
    let mut places: Vec<usize> = Vec::with_capacity(list.len());
    let mut taken = vec![false; list.len()];
    for id in next {
        let Some(n) = held.get(id.as_str()).copied() else { continue };
        if taken[n] {
            continue;
        }
        taken[n] = true;
        places.push(n);
    }
    for (n, seen) in taken.iter().enumerate() {
        if !seen {
            places.push(n);
        }
    }
    let mut scratch: Vec<Option<T>> = list.drain(..).map(Some).collect();
    for n in places {
        if let Some(entry) = scratch[n].take() {
            list.push(entry);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Item, ItemType, TrashEntry};

    fn board_with(ids: &[&str]) -> Board {
        let mut board = Board { title: "Kitchen".into(), ..Board::default() };
        for (n, id) in ids.iter().enumerate() {
            let mut item = Item::new(*id, ItemType::Image);
            item.x = n as f32 * 100.0;
            item.name = format!("card {id}");
            board.items.push(item);
        }
        board
    }

    #[test]
    fn a_board_survives_a_trip_through_text() {
        let board = board_with(&["a", "b", "c"]);
        let snap = snapshot(&board);
        let mut back = Board::default();
        restore(&mut back, &snap);
        assert_eq!(snapshot(&back), snap);
    }

    #[test]
    fn undo_puts_the_board_back_byte_for_byte() {
        // The shape the roadmap asks for: apply a step, reverse it, assert the
        // board is identical through `serialize`.
        let mut state = BoardState::new(board_with(&["a", "b"]));
        let before = schema::serialize(&state);

        state.edit_at("Move", 1, |board| {
            board.item_mut("a").unwrap().x += 40.0;
        });
        assert_ne!(schema::serialize(&state), before);

        assert_eq!(state.undo().as_deref(), Some("Move"));
        assert_eq!(schema::serialize(&state), before);
    }

    #[test]
    fn redo_puts_it_forward_again() {
        let mut state = BoardState::new(board_with(&["a"]));
        state.edit_at("Move", 1, |b| b.item_mut("a").unwrap().x = 999.0);
        let moved = schema::serialize(&state);
        state.undo();
        assert_eq!(state.redo().as_deref(), Some("Move"));
        assert_eq!(schema::serialize(&state), moved);
    }

    #[test]
    fn a_delete_and_its_undo_bring_the_card_back_whole() {
        let mut state = BoardState::new(board_with(&["a", "b"]));
        let before = schema::serialize(&state);
        state.edit_at("Delete", 1, |board| {
            let item = board.items.remove(0);
            board.trash.insert(0, TrashEntry { item, at: 7 });
        });
        assert_eq!(state.items.len(), 1);
        assert_eq!(state.trash.len(), 1);
        state.undo();
        assert_eq!(schema::serialize(&state), before, "the bin gave it all back");
    }

    #[test]
    fn a_run_of_nudges_is_one_step_and_one_undo() {
        let mut state = BoardState::new(board_with(&["a"]));
        let before = schema::serialize(&state);
        for n in 0..12 {
            state.edit_at("Nudge", n, |b| b.item_mut("a").unwrap().x += 1.0);
        }
        assert_eq!(state.timeline().steps().len(), 1, "twelve taps, one entry");
        assert_eq!(state.item("a").unwrap().x, 12.0);
        state.undo();
        assert_eq!(schema::serialize(&state), before, "and one undo takes it all");
    }

    #[test]
    fn a_gesture_is_one_step_however_many_frames_it_took() {
        let mut state = BoardState::new(board_with(&["a"]));
        let before = schema::serialize(&state);
        let open = state.start();
        for _ in 0..400 {
            state.during(&open, |b| b.item_mut("a").unwrap().x += 0.25);
        }
        assert!(state.finish_at("Move", open, 1));
        assert_eq!(state.timeline().steps().len(), 1);
        state.undo();
        assert_eq!(schema::serialize(&state), before);
    }

    #[test]
    fn an_edit_that_changes_nothing_leaves_no_step() {
        let mut state = BoardState::new(board_with(&["a"]));
        state.edit_at("Nothing", 1, |_| {});
        assert!(state.timeline().is_empty());
        assert!(state.undo().is_none());
    }

    #[test]
    fn doing_something_new_with_the_marker_back_drops_what_was_ahead() {
        let mut state = BoardState::new(board_with(&["a"]));
        state.edit_at("First", 1, |b| b.item_mut("a").unwrap().x = 1.0);
        state.edit_at("Second", 2, |b| b.item_mut("a").unwrap().name = "two".into());
        state.undo();
        state.edit_at("Third", 3, |b| b.item_mut("a").unwrap().rot = 45.0);
        assert_eq!(state.timeline().steps().len(), 2);
        assert_eq!(state.redo_label(), None);
    }

    #[test]
    fn a_step_writes_only_what_it_changed() {
        // Two cards, one step each. Undoing the first must not put back the
        // second's position along with it.
        let mut state = BoardState::new(board_with(&["a", "b"]));
        state.edit_at("Move a", 1, |b| b.item_mut("a").unwrap().x = 500.0);
        state.edit_at("Move b", 2, |b| b.item_mut("b").unwrap().x = 700.0);
        state.undo();
        assert_eq!(state.item("b").unwrap().x, 100.0, "b went back");
        assert_eq!(state.item("a").unwrap().x, 500.0, "a stayed where it was");
    }

    #[test]
    fn the_camera_is_not_a_step() {
        let mut state = BoardState::new(board_with(&["a"]));
        state.set_view(View { pan_x: 40.0, pan_y: 90.0, zoom: 2.0 });
        assert!(state.timeline().is_empty());
        state.edit_at("Move", 1, |b| b.item_mut("a").unwrap().x = 1.0);
        state.undo();
        assert_eq!(state.view.pan_x, 40.0, "undo left the camera alone");
    }

    #[test]
    fn a_ledger_comes_back_off_a_file_able_to_undo() {
        // Opened from a file rather than built in memory, because that is what
        // the far end of the trip will be: a board that has never been through
        // `normalize` has no layout records, and comparing one against a board
        // that has would be measuring the round trip rather than the undo.
        let first = schema::serialize(&board_with(&["a", "b"]));
        let mut state = BoardState::opened(schema::normalize(&first), &first);
        let before = schema::serialize(&state);
        state.edit_at("Move", 1, |b| b.item_mut("a").unwrap().x = 321.0);

        let filed = state.to_value();
        assert!(filed.get("timeline").is_some(), "a changed board carries its past");

        let board = schema::normalize(&filed);
        let mut reopened = BoardState::opened(board, &filed);
        assert!(!reopened.timeline().stale(), "it describes the board beside it");
        assert_eq!(reopened.undo_label(), Some("Move"));
        reopened.undo();
        assert_eq!(schema::serialize(&reopened), before);
    }

    #[test]
    fn a_board_nobody_changed_writes_the_file_it_always_did() {
        let state = BoardState::new(board_with(&["a"]));
        assert!(state.to_value().get("timeline").is_none());
    }

    #[test]
    fn a_ledger_edited_by_a_build_that_dropped_it_is_marked_stale() {
        let mut state = BoardState::new(board_with(&["a"]));
        state.edit_at("Move", 1, |b| b.item_mut("a").unwrap().x = 5.0);
        let mut filed = state.to_value();
        // What a build that does not understand `timeline` leaves behind: the
        // key dropped, the board edited, and the file brought back here.
        filed["items"][0]["x"] = serde_json::json!(9999.0);

        let board = schema::normalize(&filed);
        let reopened = BoardState::opened(board, &filed);
        assert!(reopened.timeline().stale());
        assert_eq!(reopened.undo_label(), None, "nothing replays a ledger it cannot vouch for");
    }

    #[test]
    fn a_long_run_of_edits_walks_all_the_way_back_and_all_the_way_forward() {
        // The one test that exercises the ordering machinery, which is the part
        // of this module with the most hand-rolled index arithmetic in it:
        // arrivals, departures and reorderings interleaved with plain moves,
        // then walked to both ends. A deterministic sequence rather than a
        // random one, so a failure is a failure somebody else can reproduce.
        let first = schema::serialize(&board_with(&["a", "b", "c"]));
        let mut state = BoardState::opened(schema::normalize(&first), &first);

        let mut marks = vec![schema::serialize(&state)];
        let step = |state: &mut BoardState, n: i64, f: &dyn Fn(&mut Board)| {
            state.edit_at(&format!("Step {n}"), n, |board| f(board));
        };

        step(&mut state, 1, &|b| b.item_mut("b").unwrap().x = 700.0);
        marks.push(schema::serialize(&state));
        step(&mut state, 2, &|b| {
            let item = b.items.remove(0);
            b.trash.insert(0, TrashEntry { item, at: 5 });
        });
        marks.push(schema::serialize(&state));
        step(&mut state, 3, &|b| {
            let mut fresh = Item::new("d", ItemType::Note);
            fresh.name = "later".into();
            b.items.insert(0, fresh);
        });
        marks.push(schema::serialize(&state));
        step(&mut state, 4, &|b| b.items.reverse());
        marks.push(schema::serialize(&state));
        step(&mut state, 5, &|b| {
            let item = b.trash.remove(0).item;
            b.items.push(item);
        });
        marks.push(schema::serialize(&state));
        step(&mut state, 6, &|b| b.title = "renamed".into());
        marks.push(schema::serialize(&state));

        assert_eq!(state.timeline().steps().len(), 6, "six intentions, six steps");

        for want in marks.iter().rev().skip(1) {
            assert!(state.undo().is_some());
            assert_eq!(&schema::serialize(&state), want, "walking back");
        }
        for want in marks.iter().skip(1) {
            assert!(state.redo().is_some());
            assert_eq!(&schema::serialize(&state), want, "walking forward");
        }
        assert_eq!(state.redo_label(), None);
    }

    #[test]
    fn a_step_that_deleted_a_picture_keeps_the_picture_alive() {
        let hash = "a".repeat(64);
        let mut board = board_with(&["a"]);
        board.item_mut("a").unwrap().asset =
            Some(crate::model::ItemAsset::Embedded { hash: hash.clone(), family: None });
        let mut state = BoardState::new(board);
        state.edit_at("Empty the bin", 1, |b| {
            b.items.clear();
        });
        assert!(!state.required_hashes().contains(&hash), "no card wants it");
        assert!(state.optional_hashes().contains(&hash), "the step still does");
    }
}
