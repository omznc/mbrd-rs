//! Typing into things.
//!
//! GPUI ships no text field, so this is one. It is deliberately *only* the
//! model: a string, a caret, a selection, and the rules for how key presses
//! move them. Nothing here knows what a pixel is, which is the same bargain
//! `mbrd-core` makes with the window and for the same reason — every rule below
//! is testable by writing text into it and asserting what came out, with no
//! window, no font and no event loop.
//!
//! What draws it lives in `board_view.rs`, where the text system is. The one
//! thing in here that cannot be settled without a font — how wide a character
//! is, and therefore where a line wraps — is asked of a
//! [`crate::metrics::Advance`] that the caller hands in, so the rules stay
//! testable against arithmetic rather than against a machine's font list.
//!
//! ## Offsets are bytes, and the caret never lands inside a character
//!
//! `caret` and `anchor` are byte offsets into a UTF-8 string, because that is
//! what slicing wants and converting on every keystroke would be its own source
//! of bugs. Every move goes through [`Editor::step`] or one of the seek
//! helpers, which move by `char` and therefore always land on a boundary. A
//! caret half-way through an é would panic on the next slice, so the invariant
//! is worth stating: **nothing outside this module writes an offset.**
//!
//! ## Undo is grouped by what the edit was, not by when it happened
//!
//! A text field whose `Ctrl Z` takes back one character is a text field
//! nobody uses twice, and one whose `Ctrl Z` takes back the entire session is
//! the board's own history — which is what this used to fall through to. So
//! there is a stack in here, and the interesting decision is where one step
//! ends and the next begins.
//!
//! Every editor with a timer in it groups by elapsed milliseconds. This one
//! groups by [`Change`] instead: a run of typing is one step, a run of
//! deleting is another, and the group breaks whenever the caret is moved,
//! whenever whitespace is typed, and whenever the *kind* of edit changes.
//! That is deterministic, which is the whole bargain this module makes — a
//! wall clock in here would be a rule that could only be tested by sleeping,
//! and the module doc above promises no event loop.
//!
//! When the stack runs out, `Ctrl Z` is handed back as [`Reply::Ignored`]
//! rather than swallowed, and the board's own undo takes the press. So the
//! ordering a person actually expects — take back my typing first, then take
//! back what I did before I started typing — comes out of the two stacks
//! without either knowing about the other.
//!
//! ## Marked text
//!
//! A composing keyboard — Japanese, Korean, Chinese — hands over a run of
//! provisional text that it will later replace with the finished characters,
//! and expects to be told where that run is. [`Editor::marked`] is that run,
//! and it is the model half of the `EntityInputHandler` protocol; the half
//! that talks to the platform is in `board_view.rs`, where the window is.
//! Nothing here decides how it is drawn, but it is kept as a span so the
//! painter can underline it.

use crate::metrics::Advance;

/// What a key press meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    /// Dealt with. Keep editing.
    Held,
    /// Keep what was typed and stop.
    Commit,
    /// Put back what was there before and stop.
    Revert,
    /// Not ours. The caller may do what it likes with it.
    Ignored,
}

/// What kind of character this is, for deciding where a word stops.
///
/// Three classes rather than two, and that is the whole of the difference
/// between `Ctrl Left` landing inside `foo.bar` and jumping the lot. Treating
/// punctuation as "not a word" merges it with the spaces around it, so a step
/// over `foo, bar` skips the comma and the gap together and there is no way to
/// put the caret between them. Whitespace, word and punctuation each end a run
/// of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Class {
    Space,
    Word,
    Punctuation,
}

fn class(c: char) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punctuation
    }
}

/// What the last edit was, for deciding where one undo step ends.
///
/// See the module doc: this is what is grouped on instead of a clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Change {
    Typing,
    Erasing,
    /// Anything that should stand alone — a paste, a caret move, the start of
    /// a session. Never coalesces, in either direction.
    Other,
}

/// The text and the caret, as they were before some edit.
///
/// The whole string rather than a diff. Every field this editor holds is
/// bounded — `mbrd_core::preview::TEXT_MAX` at the widest — so the stack is
/// bounded by [`Editor::UNDO_MAX`] copies of something that fits in memory
/// many times over, and a diff would be a second representation of the edit
/// that could disagree with the first.
#[derive(Debug, Clone)]
struct Step {
    text: String,
    caret: usize,
    anchor: usize,
}

/// A string being edited, and where the caret is in it.
#[derive(Debug, Clone)]
pub struct Editor {
    text: String,
    /// Byte offset of the caret. Always on a character boundary.
    caret: usize,
    /// The other end of the selection. Equal to `caret` when there is none.
    anchor: usize,
    /// The column an up/down run started from.
    ///
    /// Without it, walking down through a short line and back up lands you at
    /// the end of the short one rather than where you started — the single
    /// most-noticed detail of a text field that gets it wrong.
    goal: Option<usize>,
    /// The most characters this may hold, from the format.
    limit: usize,
    /// Whether Enter is a newline or the end of the edit.
    multiline: bool,
    /// Where a composing keyboard's provisional text is, if any. See the
    /// module doc.
    marked: Option<(usize, usize)>,
    /// What the last edit was, and therefore whether the next one joins it.
    last: Change,
    /// States to go back to, oldest first.
    undo: Vec<Step>,
    /// States to go forward to, dropped by the next edit.
    redo: Vec<Step>,
}

impl Editor {
    /// How many steps back one session keeps.
    ///
    /// A session is one card's text between opening it and committing it, so
    /// this is generous rather than tight: the board's own history takes over
    /// past the end of it, and a step only ever costs a copy of what the
    /// session's own limit allows.
    pub const UNDO_MAX: usize = 256;

    pub fn new(text: impl Into<String>, limit: usize, multiline: bool) -> Self {
        let text = text.into();
        let caret = text.len();
        Self {
            text,
            caret,
            anchor: caret,
            goal: None,
            limit,
            multiline,
            marked: None,
            last: Change::Other,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    /// Editing something, with all of it selected.
    ///
    /// What a rename should open as: the first thing typed replaces the old
    /// name, and an arrow key steps out of the selection to keep it.
    pub fn selecting_all(text: impl Into<String>, limit: usize, multiline: bool) -> Self {
        let mut editor = Self::new(text, limit, multiline);
        editor.anchor = 0;
        editor
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    /// The selection, low end first, or `None` when there is not one.
    pub fn selection(&self) -> Option<(usize, usize)> {
        (self.caret != self.anchor)
            .then(|| (self.caret.min(self.anchor), self.caret.max(self.anchor)))
    }

    /// Which line the caret is on, and how far along it, in bytes — lines as
    /// somebody pressed Enter, with no wrap.
    ///
    /// What a single-line field's drawing side needs, and what the vertical
    /// arrows walk. A card's drawing side wraps, and asks [`Self::caret_in`]
    /// instead.
    pub fn caret_line(&self) -> (usize, usize) {
        let before = &self.text[..self.caret];
        let row = before.matches('\n').count();
        let column = before.rfind('\n').map_or(self.caret, |at| self.caret - at - 1);
        (row, column)
    }

    /// The text as the visual rows a card `width` pixels wide shows: one byte
    /// span per row, breaking where somebody pressed Enter and again where the
    /// words run out of room.
    ///
    /// Spans rather than strings, because everything else about the caret is
    /// a byte offset and a wrap that returned strings would strand it.
    /// [`Self::caret_in`] and [`Self::highlight_in`] turn offsets into places
    /// on these rows, and a click comes back the other way through a span's
    /// start — arithmetic that holds because every byte of the text lands in
    /// exactly one span, newlines excepted.
    ///
    /// Greedy, by word, and *measured*: `adv` says how wide each character is,
    /// so a row of `WWWW` breaks sooner than a row of `iiii` instead of both
    /// being "four characters" — see [`crate::metrics`] for what that division
    /// used to get wrong. A space never forces a break: a run of them hangs
    /// invisibly past the edge instead, so the next word starts its row rather
    /// than a stray indent.
    ///
    /// Every row holds at least one character, however narrow the card and
    /// however wide the character. A row of nothing would be a row the caret
    /// could sit on with no way to leave it, and the painter clips anything
    /// that overhangs anyway.
    pub fn wrapped(&self, width: f32, size: f32, adv: &dyn Advance) -> Vec<(usize, usize)> {
        let width = width.max(0.0);
        let mut out = Vec::new();
        for (start, end) in self.line_spans() {
            let mut row = start;
            let mut used = 0.0f32;
            // The byte after the last space on this row: where a break would
            // rather land than the middle of a word.
            let mut space: Option<usize> = None;
            for (at, c) in self.text[start..end].char_indices() {
                let at = start + at;
                let wide = adv.of(c, size);
                if c == ' ' {
                    used += wide;
                    space = Some(at + 1);
                    continue;
                }
                if used + wide > width && at > row {
                    // This character does not fit. Break after the last
                    // space, or right here for a word longer than the whole
                    // row — a URL, usually.
                    let cut = space.take().filter(|&s| s > row).unwrap_or(at);
                    out.push((row, cut));
                    row = cut;
                    used = adv.width(&self.text[cut..at], size);
                }
                used += wide;
            }
            out.push((row, end));
        }
        out
    }

    /// Which of the given rows the caret is on, and how far along it, in
    /// bytes — [`Self::caret_line`] with a wrap applied. `rows` is what
    /// [`Self::wrapped`] answered, handed back in so the three callers that
    /// need all of caret, wash and rows cut the text up exactly once.
    ///
    /// A caret sitting exactly on a soft break belongs to the row *after*
    /// it: that is where the next character typed will land, so that is
    /// where it should be seen waiting. Across a hard break the newline byte
    /// keeps the two rows apart, and the caret stays where Enter left it.
    pub fn caret_in(&self, rows: &[(usize, usize)]) -> (usize, usize) {
        let row = rows.iter().rposition(|&(start, _)| start <= self.caret).unwrap_or(0);
        (row, self.caret - rows[row].0)
    }

    /// The selection, cut up by the given rows: `(row, start, end)` in bytes
    /// within each.
    ///
    /// The shape a painter wants, because it draws the wash one row at a time
    /// and a range spanning three rows is three rectangles rather than one.
    pub fn highlight_in(&self, rows: &[(usize, usize)]) -> Vec<(usize, usize, usize)> {
        match self.selection() {
            Some((from, to)) => self.span_in(from, to, rows),
            None => Vec::new(),
        }
    }

    /// The composing run, cut up the same way — for the underline that says
    /// which characters are still provisional. See the module doc on marked
    /// text.
    pub fn marked_in(&self, rows: &[(usize, usize)]) -> Vec<(usize, usize, usize)> {
        match self.marked {
            Some((from, to)) => self.span_in(from, to, rows),
            None => Vec::new(),
        }
    }

    /// Any byte range, cut up by the given rows: `(row, start, end)` in bytes
    /// within each. What both of the two above are.
    fn span_in(
        &self,
        from: usize,
        to: usize,
        rows: &[(usize, usize)],
    ) -> Vec<(usize, usize, usize)> {
        rows.iter()
            .enumerate()
            .filter_map(|(row, &(start, end))| {
                let lit_from = from.max(start);
                let lit_to = to.min(end);
                // A row entirely inside the selection but empty — a blank line
                // in the middle of a selected paragraph — has nothing to draw,
                // and drawing a zero-width wash would be a stray pixel.
                //
                // `then` rather than `then_some`, and that is not a style
                // preference: `then_some` takes its argument *by value*, so
                // both subtractions below happen before the test that makes
                // them safe. On any row starting after the end of the span —
                // every row below a selection that does not reach the bottom
                // of the note — `lit_to` is that span's end and `start` is
                // past it, and the subtraction goes negative on a `usize`.
                (lit_from < lit_to).then(|| (row, lit_from - start, lit_to - start))
            })
            .collect()
    }

    /// Put the caret at a byte offset, held to a character boundary.
    ///
    /// For a click. The offset comes from measuring text, which happens where
    /// the font is, so it arrives from outside and cannot be trusted to be on a
    /// boundary — a shaping that reports the middle of a two-byte character
    /// would otherwise panic on the next slice rather than merely being wrong.
    pub fn place(&mut self, at: usize, extend: bool) {
        let at = self.boundary(at);
        self.caret = at;
        if !extend {
            self.anchor = at;
        }
        self.goal = None;
        // Moving the caret ends the run of typing that was going on, so what
        // is typed next is its own undo step. See the module doc.
        self.last = Change::Other;
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
        self.goal = None;
        self.last = Change::Other;
    }

    /// Select the run of like characters at `at` — what a double-click means.
    ///
    /// The *run*, by [`Class`], rather than the word: double-clicking the gap
    /// between two words selects the gap, and double-clicking `::` selects
    /// both colons. Anything else would have to decide which neighbouring word
    /// somebody meant, and there is no right answer to that.
    pub fn select_word_at(&mut self, at: usize) {
        let at = self.boundary(at);
        // Which of the two runs the pointer is between wins, when it is
        // between two. The one after it, unless that is whitespace and the one
        // before is not: pressing just past the `o` of `two` means `two`, not
        // the space that follows it, and that is where a double-click most
        // often lands. At either end of the text there is only one to choose.
        let after = self.text[at..].chars().next().map(class);
        let before = self.text[..at].chars().next_back().map(class);
        let run = match (after, before) {
            (Some(Class::Space), Some(before)) if before != Class::Space => before,
            (Some(after), _) => after,
            (None, Some(before)) => before,
            (None, None) => return,
        };
        let mut from = at;
        while self.text[..from].chars().next_back().is_some_and(|c| class(c) == run) {
            from = self.step(from, false);
        }
        let mut to = at;
        while self.text[to..].chars().next().is_some_and(|c| class(c) == run) {
            to = self.step(to, true);
        }
        self.anchor = from;
        self.caret = to;
        self.goal = None;
        self.last = Change::Other;
    }

    /// Select the line at `at` — what a triple-click means.
    ///
    /// The line somebody pressed Enter to make, not the wrapped row: a row is
    /// a fact about how wide the card is today, and a selection that changed
    /// when the card was resized would be a selection about the wrong thing.
    pub fn select_line_at(&mut self, at: usize) {
        let at = self.boundary(at);
        let spans = self.line_spans();
        let (start, end) =
            spans.iter().rposition(|&(s, _)| s <= at).map_or((0, self.text.len()), |i| spans[i]);
        self.anchor = start;
        self.caret = end;
        self.goal = None;
        self.last = Change::Other;
    }

    /// The selected text, for a copy.
    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|(from, to)| &self.text[from..to])
    }

    /// Take one key press.
    ///
    /// `text` is what the platform says the key would type — which is not the
    /// same as the key: `option-s` is one key and the letter `ß`. Using it is
    /// what makes a keyboard laid out for another language type what is printed
    /// on its keycaps.
    pub fn key(&mut self, key: &str, mods: Mods, text: Option<&str>) -> Reply {
        let shift = mods.shift;
        match key {
            "escape" => return Reply::Revert,
            "enter" if mods.secondary || !self.multiline => return Reply::Commit,
            "enter" => self.insert("\n"),
            "tab" => return Reply::Commit,

            "left" => self.walk(-1, shift, mods.word),
            "right" => self.walk(1, shift, mods.word),
            "up" => self.vertical(-1, shift),
            "down" => self.vertical(1, shift),
            "home" => self.jump_to_edge(false, shift),
            "end" => self.jump_to_edge(true, shift),

            "backspace" => self.erase(false, mods.word),
            "delete" => self.erase(true, mods.word),

            "a" if mods.secondary => self.select_all(),
            // Undo before redo, because the guard on the redo arm is the
            // narrower one and `match` takes the first that fits.
            //
            // Handed back rather than swallowed once the stack is empty, which
            // is what lets the board's own undo carry on from where this one
            // stopped — see the module doc.
            "z" if mods.secondary && mods.shift => {
                return if self.redo() { Reply::Held } else { Reply::Ignored };
            }
            "z" if mods.secondary => {
                return if self.undo() { Reply::Held } else { Reply::Ignored };
            }
            // The other redo. Windows and Linux have both; macOS has only the
            // one above, and nothing there sends this.
            "y" if mods.secondary => {
                return if self.redo() { Reply::Held } else { Reply::Ignored };
            }
            // Copy, cut and paste reach the clipboard, which is the platform's
            // rather than ours. Reported so the caller can do it and then say
            // what happened — see `Edit` in `board_view.rs`.
            "c" | "x" | "v" if mods.secondary => return Reply::Ignored,

            _ => {
                let Some(text) = text.filter(|t| !t.is_empty()) else {
                    return Reply::Ignored;
                };
                if mods.secondary || mods.alt {
                    return Reply::Ignored;
                }
                // A control character that got this far is not something
                // anybody typed on purpose.
                if text.chars().all(char::is_control) {
                    return Reply::Ignored;
                }
                self.put(text, Change::Typing);
                // Whitespace closes the group, so undo comes back a word at a
                // time rather than a sentence at a time.
                if text.chars().all(char::is_whitespace) {
                    self.last = Change::Other;
                }
            }
        }
        Reply::Held
    }

    /// Put text in, replacing the selection, held to the limit.
    ///
    /// The limit is the format's, and it is applied here rather than at the
    /// save so that a note stops accepting characters at the point it stops
    /// being able to keep them — silently dropping the tail of what somebody
    /// typed an hour later is the alternative.
    pub fn insert(&mut self, text: &str) {
        self.put(text, Change::Other);
    }

    /// [`Self::insert`]'s own work, told what kind of edit it is so the undo
    /// stack knows whether this joins the last step or starts a new one.
    ///
    /// Nothing is written and no step is pushed when there is nothing to do —
    /// a keystroke into a field that is already full, which would otherwise
    /// leave an undo step that undid nothing.
    fn put(&mut self, text: &str, change: Change) {
        let selected = self.selection();
        // The room is what is left once the selection has gone, since this is
        // replacing it. Counted rather than sliced so the string is only
        // rewritten once, below.
        let going: usize = selected.map_or(0, |(from, to)| self.text[from..to].chars().count());
        let room = self.limit.saturating_sub(self.text.chars().count() - going);
        let fits: String = text.chars().take(room).collect();
        // A newline in a single-line field is somebody pasting a paragraph into
        // a name. Keep the words, lose the breaks.
        let fits = if self.multiline { fits } else { fits.replace(['\n', '\r'], " ") };
        if fits.is_empty() && selected.is_none() {
            return;
        }
        self.remember(change);
        if let Some((from, to)) = selected {
            self.text.replace_range(from..to, "");
            self.caret = from;
        }
        self.text.insert_str(self.caret, &fits);
        self.caret += fits.len();
        self.anchor = self.caret;
        self.goal = None;
        self.marked = None;
    }

    /// Delete: the selection if there is one, otherwise one character or word.
    fn erase(&mut self, forward: bool, word: bool) {
        let (from, to) = match self.selection() {
            Some(span) => span,
            None => {
                let to =
                    if word { self.word_edge(forward) } else { self.step(self.caret, forward) };
                (self.caret.min(to), self.caret.max(to))
            }
        };
        // Backspace at the very start, or delete at the very end. Nothing to
        // take back, so nothing to remember either.
        if from == to {
            return;
        }
        self.remember(Change::Erasing);
        self.text.replace_range(from..to, "");
        self.caret = from;
        self.anchor = from;
        self.goal = None;
        self.marked = None;
    }

    // -----------------------------------------------------------------------
    // Undo
    // -----------------------------------------------------------------------

    /// Go back one step. `false` when there is nothing left, which is the
    /// caller's cue to let the board's own history have the press.
    pub fn undo(&mut self) -> bool {
        let Some(step) = self.undo.pop() else { return false };
        let now = self.snapshot();
        self.restore(step);
        self.redo.push(now);
        true
    }

    /// Go forward one step, undoing an undo.
    pub fn redo(&mut self) -> bool {
        let Some(step) = self.redo.pop() else { return false };
        let now = self.snapshot();
        self.restore(step);
        self.undo.push(now);
        true
    }

    /// Keep the state an edit is about to leave behind, unless it belongs to
    /// the step already on top of the stack.
    ///
    /// Called *before* the edit, so what lands on the stack is what to go back
    /// to rather than what was arrived at.
    fn remember(&mut self, change: Change) {
        // Any edit at all abandons whatever was ahead: there is no longer one
        // history to walk forward along.
        self.redo.clear();
        let joins = change != Change::Other && change == self.last;
        self.last = change;
        if joins {
            return;
        }
        self.undo.push(self.snapshot());
        if self.undo.len() > Self::UNDO_MAX {
            self.undo.remove(0);
        }
    }

    fn snapshot(&self) -> Step {
        Step { text: self.text.clone(), caret: self.caret, anchor: self.anchor }
    }

    fn restore(&mut self, step: Step) {
        self.text = step.text;
        self.caret = step.caret;
        self.anchor = step.anchor;
        self.goal = None;
        self.marked = None;
        // Whatever is typed after an undo is its own step. Without this a
        // press of `Ctrl Z` followed by a keystroke would coalesce into the
        // step the undo had just landed on and be unreachable.
        self.last = Change::Other;
    }

    // -----------------------------------------------------------------------
    // Marked text, for a composing keyboard
    // -----------------------------------------------------------------------

    // The platform counts in UTF-16 code units and this module counts in
    // UTF-8 bytes. Both conversions live here, with everything else that is
    // allowed to know what an offset is.

    /// One of the platform's offsets, as one of ours.
    ///
    /// Clamped rather than trusted. A platform is allowed to send an offset
    /// past the end of the text — it is describing the string it last heard
    /// about, which may be a keystroke out of date — and the answer to that is
    /// the end of the text rather than a panic on the next slice.
    pub fn utf8_at(&self, utf16: usize) -> usize {
        let mut counted = 0;
        for (at, c) in self.text.char_indices() {
            if counted >= utf16 {
                return at;
            }
            counted += c.len_utf16();
        }
        self.text.len()
    }

    /// And one of ours as one of the platform's.
    pub fn utf16_at(&self, utf8: usize) -> usize {
        self.text[..self.boundary(utf8)].chars().map(char::len_utf16).sum()
    }

    /// The selection in the platform's units, and which end the caret is on.
    ///
    /// A collapsed caret is an empty range at the caret, which is what the
    /// protocol expects rather than `None` — `None` there means "this thing
    /// does not take text at all".
    pub fn selection_utf16(&self) -> (usize, usize, bool) {
        let reversed = self.caret < self.anchor;
        let (from, to) = self.selection().unwrap_or((self.caret, self.caret));
        (self.utf16_at(from), self.utf16_at(to), reversed)
    }

    /// The composing run in the platform's units.
    pub fn marked_utf16(&self) -> Option<(usize, usize)> {
        self.marked().map(|(from, to)| (self.utf16_at(from), self.utf16_at(to)))
    }

    /// A slice of the text by the platform's offsets, and the offsets actually
    /// used — which may be narrower, since they are clamped to the text.
    pub fn text_utf16(&self, from: usize, to: usize) -> (&str, usize, usize) {
        let from = self.utf8_at(from);
        let to = self.utf8_at(to).max(from);
        (&self.text[from..to], self.utf16_at(from), self.utf16_at(to))
    }

    /// The run a composing keyboard is still working on, if there is one.
    ///
    /// Byte offsets, like everything else here. [`Self::marked_in`] is the
    /// same thing cut up by drawn rows, which is what the painter wants.
    pub fn marked(&self) -> Option<(usize, usize)> {
        self.marked
    }

    /// Forget the composing run without touching what it put in.
    pub fn unmark(&mut self) {
        self.marked = None;
    }

    /// The finished characters arriving: put `text` over `range`, or over the
    /// composing run, or over the selection, and stop composing.
    pub fn replace_text(&mut self, range: Option<(usize, usize)>, text: &str) {
        self.aim(range);
        self.marked = None;
        self.put(text, Change::Other);
    }

    /// The same, but what went in is still provisional, so it stays marked.
    ///
    /// `select` is a byte span *within `text`*, which is where the platform
    /// wants the caret while composing — a Japanese IME underlines the whole
    /// run and puts the caret part-way along it. The UTF-16 offsets the
    /// platform actually speaks are converted at the window, not here.
    pub fn replace_marked(
        &mut self,
        range: Option<(usize, usize)>,
        text: &str,
        select: Option<(usize, usize)>,
    ) {
        self.aim(range);
        self.marked = None;
        let start = self.selection().map_or(self.caret, |(from, _)| from);
        self.put(text, Change::Other);
        let end = self.caret;
        self.marked = (end > start).then_some((start, end));
        if let Some((from, to)) = select {
            self.anchor = self.boundary((start + from).min(end));
            self.caret = self.boundary((start + to).min(end));
        }
    }

    /// Point the selection at what the next replacement should land on: the
    /// range asked for, or the composing run, or — with neither — whatever is
    /// selected already.
    fn aim(&mut self, range: Option<(usize, usize)>) {
        let Some((from, to)) = range.or(self.marked) else { return };
        let from = self.boundary(from);
        let to = self.boundary(to.max(from));
        self.anchor = from;
        self.caret = to;
        self.goal = None;
    }

    /// Left or right, by a character or a word.
    fn walk(&mut self, by: i32, extend: bool, word: bool) {
        let forward = by > 0;
        // An unmodified arrow with a selection collapses to its edge rather
        // than moving from the caret. That is what every text field does, and
        // it is the reason opening a rename with everything selected is safe.
        if !extend {
            if let Some((from, to)) = self.selection() {
                self.caret = if forward { to } else { from };
                self.anchor = self.caret;
                self.goal = None;
                self.last = Change::Other;
                return;
            }
        }
        self.caret = if word { self.word_edge(forward) } else { self.step(self.caret, forward) };
        if !extend {
            self.anchor = self.caret;
        }
        self.goal = None;
        self.last = Change::Other;
    }

    /// Up or down a line, keeping the column somebody started from.
    fn vertical(&mut self, by: i32, extend: bool) {
        let lines: Vec<(usize, usize)> = self.line_spans();
        let (row, column) = self.caret_line();
        let column = self.goal.unwrap_or(column);

        let target = row as i32 + by;
        if target < 0 || target as usize >= lines.len() {
            // Off the top or the bottom: go to that end of the text, which is
            // what a single-line field does with up and down and what a
            // multi-line one does at its edges.
            self.caret = if by < 0 { 0 } else { self.text.len() };
            if !extend {
                self.anchor = self.caret;
            }
            self.goal = Some(column);
            self.last = Change::Other;
            return;
        }

        let (start, end) = lines[target as usize];
        // The column is a byte count into the previous line, so it may land
        // inside a character on this one.
        self.caret = self.boundary((start + column).min(end));
        if !extend {
            self.anchor = self.caret;
        }
        self.goal = Some(column);
        self.last = Change::Other;
    }

    /// The start or the end of the line the caret is on.
    fn jump_to_edge(&mut self, end: bool, extend: bool) {
        let (start, stop) = self.line_spans()[self.caret_line().0];
        self.caret = if end { stop } else { start };
        if !extend {
            self.anchor = self.caret;
        }
        self.goal = None;
        self.last = Change::Other;
    }

    /// Where every line starts and ends, in bytes, newlines excluded.
    fn line_spans(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        let mut start = 0;
        for (at, _) in self.text.match_indices('\n') {
            out.push((start, at));
            start = at + 1;
        }
        out.push((start, self.text.len()));
        out
    }

    /// One character along, stopping at either end.
    fn step(&self, from: usize, forward: bool) -> usize {
        if forward {
            self.text[from..].chars().next().map_or(from, |c| from + c.len_utf8())
        } else {
            self.text[..from].chars().next_back().map_or(from, |c| from - c.len_utf8())
        }
    }

    /// The far side of the run of like characters next to the caret.
    ///
    /// Whitespace first, then one run of whatever class follows it — so `Ctrl`
    /// and a left arrow in the middle of `  hello` lands before `hello` rather
    /// than after the spaces, and a step over `foo.bar` stops at the dot
    /// instead of jumping the whole thing. See [`Class`] for why that is three
    /// cases and not two.
    fn word_edge(&self, forward: bool) -> usize {
        self.word_edge_from(self.caret, forward)
    }

    /// The same errand from an arbitrary offset, which is what selecting the
    /// word under a double-click needs — it has two edges to find and the
    /// caret is only ever on one of them.
    fn word_edge_from(&self, from: usize, forward: bool) -> usize {
        let mut at = from;
        let peek = |at: usize| -> Option<char> {
            if forward {
                self.text[at..].chars().next()
            } else {
                self.text[..at].chars().next_back()
            }
        };
        while peek(at).is_some_and(|c| class(c) == Class::Space) {
            at = self.step(at, forward);
        }
        let Some(run) = peek(at).map(class) else { return at };
        while peek(at).is_some_and(|c| class(c) == run) {
            at = self.step(at, forward);
        }
        at
    }

    /// The nearest character boundary at or before `at`.
    fn boundary(&self, at: usize) -> usize {
        let mut at = at.min(self.text.len());
        while at > 0 && !self.text.is_char_boundary(at) {
            at -= 1;
        }
        at
    }
}

/// The modifier state, in the three shapes this module cares about.
///
/// Its own type rather than GPUI's, so that everything above is testable
/// without building a platform `Modifiers` — and so that "the word modifier" is
/// named once here rather than being `control` on one platform and `alt` on
/// another at four call sites.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    /// Ctrl on Linux and Windows, Command on macOS.
    pub secondary: bool,
    pub alt: bool,
    /// Whether to move by words. Alt on macOS, Ctrl elsewhere.
    pub word: bool,
}

impl From<gpui::Modifiers> for Mods {
    fn from(m: gpui::Modifiers) -> Self {
        Self {
            shift: m.shift,
            secondary: m.secondary(),
            alt: m.alt,
            word: if cfg!(target_os = "macos") { m.alt } else { m.control },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROOM: usize = 512;

    /// [`Editor::wrapped`] measured in characters rather than pixels, so the
    /// assertions below stay about where the words broke. See the same shim in
    /// `markdown.rs` for the argument.
    fn wrapped(e: &Editor, columns: usize) -> Vec<(usize, usize)> {
        e.wrapped(columns as f32, 1.0, &crate::metrics::Estimate::columns())
    }

    fn editor(text: &str) -> Editor {
        Editor::new(text, ROOM, true)
    }

    fn plain() -> Mods {
        Mods::default()
    }

    fn shift() -> Mods {
        Mods { shift: true, ..Mods::default() }
    }

    fn word() -> Mods {
        Mods { word: true, ..Mods::default() }
    }

    /// Type a run of characters, the way a keyboard would.
    fn type_in(editor: &mut Editor, text: &str) {
        for c in text.chars() {
            let s = c.to_string();
            editor.key(&s, plain(), Some(&s));
        }
    }

    #[test]
    fn typing_puts_characters_where_the_caret_is() {
        let mut e = editor("hello");
        e.place(0, false);
        type_in(&mut e, "say ");
        assert_eq!(e.text(), "say hello");
        assert_eq!(e.caret, 4);
    }

    #[test]
    fn a_new_editor_starts_at_the_end_of_what_is_there() {
        let e = editor("hello");
        assert_eq!(e.caret, 5);
        assert!(e.selection().is_none());
    }

    #[test]
    fn opening_with_everything_selected_means_typing_replaces_it() {
        let mut e = Editor::selecting_all("old name", ROOM, false);
        assert_eq!(e.selection(), Some((0, 8)));
        type_in(&mut e, "new");
        assert_eq!(e.text(), "new");
    }

    #[test]
    fn an_arrow_key_steps_out_of_a_selection_rather_than_losing_it() {
        // The other half of the rename bargain: everything is selected, so
        // pressing right should keep the name and put the caret at its end.
        let mut e = Editor::selecting_all("old name", ROOM, false);
        e.key("right", plain(), None);
        assert_eq!(e.text(), "old name");
        assert_eq!(e.caret, 8);
        assert!(e.selection().is_none());

        let mut e = Editor::selecting_all("old name", ROOM, false);
        e.key("left", plain(), None);
        assert_eq!(e.caret, 0);
    }

    #[test]
    fn backspace_takes_a_whole_character_however_many_bytes_it_is() {
        let mut e = editor("café");
        e.key("backspace", plain(), None);
        assert_eq!(e.text(), "caf");
        // And the other direction, over the same character.
        let mut e = editor("café!");
        e.place(3, false);
        e.key("delete", plain(), None);
        assert_eq!(e.text(), "caf!");
    }

    #[test]
    fn a_caret_never_lands_inside_a_character() {
        // A click reports a byte offset measured from a font, and a font is
        // allowed to say something this string does not agree with.
        let mut e = editor("café");
        e.place(4, false); // the middle of the é
        assert_eq!(e.caret, 3);
        // The next thing that happens must not panic on a slice.
        e.key("backspace", plain(), None);
        assert_eq!(e.text(), "caé");
    }

    #[test]
    fn deleting_a_selection_deletes_the_selection_and_not_a_character() {
        let mut e = editor("hello there");
        e.place(0, false);
        e.place(5, true);
        assert_eq!(e.selected_text(), Some("hello"));
        e.key("backspace", plain(), None);
        assert_eq!(e.text(), " there");
        assert!(e.selection().is_none());
    }

    #[test]
    fn a_word_step_stops_where_a_word_stops() {
        let mut e = editor("one two  three");
        e.key("left", word(), None);
        assert_eq!(&e.text()[e.caret..], "three");
        e.key("left", word(), None);
        assert_eq!(&e.text()[e.caret..], "two  three");
        e.key("left", word(), None);
        assert_eq!(e.caret, 0);
    }

    #[test]
    fn deleting_a_word_deletes_the_spaces_before_it_too() {
        let mut e = editor("one two  three");
        e.key("backspace", word(), None);
        assert_eq!(e.text(), "one two  ");
        e.key("backspace", word(), None);
        assert_eq!(e.text(), "one ");
    }

    #[test]
    fn up_and_down_keep_the_column_they_started_from() {
        // The detail every text field that gets it wrong is noticed for:
        // through a short line and back out should land where it began.
        let mut e = editor("aaaaaaaaaa\nbb\ncccccccccc");
        e.place(8, false); // column 8 of the first line
        e.key("down", plain(), None);
        assert_eq!(e.caret_line(), (1, 2), "clamped to the short line");
        e.key("down", plain(), None);
        assert_eq!(e.caret_line(), (2, 8), "and back out to where it started");
    }

    #[test]
    fn a_column_that_lands_inside_a_character_moves_to_its_edge() {
        let mut e = editor("xxxx\ncafé");
        e.place(4, false);
        e.key("down", plain(), None);
        // Column 4 of "café" is inside the é, which is two bytes.
        assert_eq!(e.caret, 8);
        assert!(e.text().is_char_boundary(e.caret));
    }

    #[test]
    fn up_from_the_first_line_goes_to_the_start_rather_than_nowhere() {
        let mut e = editor("one\ntwo");
        e.place(2, false);
        e.key("up", plain(), None);
        assert_eq!(e.caret, 0);
        e.place(5, false);
        e.key("down", plain(), None);
        assert_eq!(e.caret, 7);
    }

    #[test]
    fn home_and_end_are_about_the_line_rather_than_the_text() {
        let mut e = editor("one\ntwo three");
        e.place(6, false);
        e.key("home", plain(), None);
        assert_eq!(e.caret, 4);
        e.key("end", plain(), None);
        assert_eq!(e.caret, 13);
    }

    #[test]
    fn shift_and_an_arrow_take_the_selection_with_it() {
        let mut e = editor("hello");
        e.place(0, false);
        e.key("right", shift(), None);
        e.key("right", shift(), None);
        assert_eq!(e.selected_text(), Some("he"));
        // And releasing shift collapses rather than jumping.
        e.key("right", plain(), None);
        assert_eq!(e.caret, 2);
        assert!(e.selection().is_none());
    }

    #[test]
    fn a_field_that_is_full_stops_taking_characters() {
        let mut e = Editor::new("x".repeat(8), 10, true);
        type_in(&mut e, "abcdef");
        assert_eq!(e.text().chars().count(), 10);
        assert_eq!(e.text(), "xxxxxxxxab");
    }

    #[test]
    fn the_limit_counts_characters_rather_than_bytes() {
        // Otherwise a note in Greek holds half as much as one in English.
        let mut e = Editor::new(String::new(), 4, true);
        type_in(&mut e, "αβγδε");
        assert_eq!(e.text(), "αβγδ");
    }

    #[test]
    fn enter_is_a_newline_in_a_note_and_the_end_of_a_name() {
        let mut e = Editor::new("note", ROOM, true);
        assert_eq!(e.key("enter", plain(), None), Reply::Held);
        assert_eq!(e.text(), "note\n");

        let mut e = Editor::new("name", ROOM, false);
        assert_eq!(e.key("enter", plain(), None), Reply::Commit);
        assert_eq!(e.text(), "name", "it should not have typed anything");
    }

    #[test]
    fn a_paragraph_pasted_into_a_name_keeps_its_words() {
        let mut e = Editor::new(String::new(), ROOM, false);
        e.insert("one\ntwo\r\nthree");
        assert_eq!(e.text(), "one two  three");
    }

    #[test]
    fn escape_puts_it_back_and_enter_keeps_it() {
        let mut e = editor("x");
        assert_eq!(e.key("escape", plain(), None), Reply::Revert);
        let mut e = Editor::new("x", ROOM, true);
        assert_eq!(e.key("enter", Mods { secondary: true, ..plain() }, None), Reply::Commit);
    }

    #[test]
    fn what_is_not_typing_is_handed_back_rather_than_swallowed() {
        // Otherwise `Ctrl S` inside a note would type an `s` instead of saving.
        let mut e = editor("");
        let ctrl = Mods { secondary: true, ..plain() };
        assert_eq!(e.key("s", ctrl, Some("s")), Reply::Ignored);
        assert_eq!(e.key("c", ctrl, Some("c")), Reply::Ignored);
        assert_eq!(e.key("v", ctrl, Some("v")), Reply::Ignored);
        assert_eq!(e.key("f5", plain(), None), Reply::Ignored);
        assert_eq!(e.text(), "");
        // But `Ctrl A` is ours, because a text field owns select-all.
        assert_eq!(e.key("a", ctrl, Some("a")), Reply::Held);
    }

    #[test]
    fn a_control_character_is_not_typed_into_the_text() {
        let mut e = editor("");
        e.key("f7", plain(), Some("\u{7}"));
        assert_eq!(e.text(), "");
    }

    #[test]
    fn select_all_then_type_replaces_everything() {
        let mut e = editor("the whole thing");
        e.key("a", Mods { secondary: true, ..plain() }, Some("a"));
        type_in(&mut e, "!");
        assert_eq!(e.text(), "!");
    }

    /// The rows as strings, which is what every assertion below wants to say.
    fn rows_of(e: &Editor, columns: usize) -> Vec<&str> {
        wrapped(e, columns).into_iter().map(|(start, end)| &e.text()[start..end]).collect()
    }

    #[test]
    fn a_selection_across_lines_comes_back_as_one_piece_per_line() {
        let mut e = editor("one\ntwo\nthree");
        e.place(1, false);
        e.place(9, true);
        let rows = wrapped(&e, ROOM);
        assert_eq!(e.highlight_in(&rows), vec![(0, 1, 3), (1, 0, 3), (2, 0, 1)]);
    }

    #[test]
    fn a_selection_across_a_soft_break_comes_back_as_one_piece_per_row() {
        let mut e = editor("one two three");
        e.select_all();
        let rows = wrapped(&e, 5);
        assert_eq!(rows_of(&e, 5), vec!["one ", "two ", "three"]);
        assert_eq!(e.highlight_in(&rows), vec![(0, 0, 4), (1, 0, 4), (2, 0, 5)]);
    }

    #[test]
    fn a_selection_that_stops_short_of_the_last_row_does_not_panic() {
        // The rows below the end of a selection are the case that used to take
        // the whole app down: `then_some` had already worked out an offset
        // into them before the test that says they hold nothing.
        let mut e = editor("one\ntwo\nthree\nfour");
        e.place(0, false);
        e.place(3, true);
        let rows = wrapped(&e, ROOM);
        assert_eq!(e.highlight_in(&rows), vec![(0, 0, 3)]);

        // And the same for a composing run, which reaches the same code.
        let mut e = editor("");
        e.insert("one\ntwo\nthree");
        e.place(0, false);
        e.replace_marked(None, "x", None);
        let rows = wrapped(&e, ROOM);
        assert_eq!(e.marked_in(&rows), vec![(0, 0, 1)]);
    }

    #[test]
    fn an_empty_line_inside_a_selection_has_nothing_to_draw() {
        let mut e = editor("a\n\nb");
        e.select_all();
        let rows = wrapped(&e, ROOM);
        assert_eq!(e.highlight_in(&rows), vec![(0, 0, 1), (2, 0, 1)]);
    }

    #[test]
    fn no_selection_is_nothing_to_draw_rather_than_an_empty_box() {
        let e = editor("hello");
        assert!(e.highlight_in(&wrapped(&e, ROOM)).is_empty());
    }

    #[test]
    fn rows_break_where_enter_was_pressed_and_where_the_room_runs_out() {
        let e = editor("one two three four\nfive");
        assert_eq!(rows_of(&e, 8), vec!["one two ", "three ", "four", "five"]);
        // And with room to spare, only where Enter was.
        assert_eq!(rows_of(&e, ROOM), vec!["one two three four", "five"]);
    }

    #[test]
    fn a_wide_word_breaks_sooner_than_a_narrow_one_of_the_same_length() {
        // The bug this whole measuring business exists to end. Under the old
        // arithmetic these two were both "ten characters" and broke in exactly
        // the same place, so one row ran off the card and the other stopped
        // well short of it.
        let wide = editor("WWWWWWWWWW");
        let narrow = editor("iiiiiiiiii");
        let room = 5.0;
        let rows = |e: &Editor| e.wrapped(room, 1.0, &crate::metrics::Ragged).len();
        assert_eq!(rows(&wide), 2, "ten ems of W into five ems of room");
        assert_eq!(rows(&narrow), 1, "two ems of i fit in five with room to spare");
    }

    #[test]
    fn a_row_always_holds_at_least_one_character() {
        // A card narrower than a single letter. A row of nothing would be a row
        // the caret could sit on with no way to leave it, and the loop that
        // produced it would not terminate.
        let e = editor("WWW");
        let rows = e.wrapped(0.1, 1.0, &crate::metrics::Ragged);
        assert_eq!(rows.len(), 3);
        for &(start, end) in &rows {
            assert!(start < end, "an empty row at {start}..{end}");
        }
    }

    #[test]
    fn a_word_longer_than_the_row_is_cut_rather_than_spun_on() {
        let e = editor("https://example.com/a");
        assert_eq!(rows_of(&e, 10), vec!["https://ex", "ample.com/", "a"]);
    }

    #[test]
    fn every_byte_of_the_text_lands_in_exactly_one_row() {
        // The invariant the caret arithmetic stands on, on a text that wraps
        // every way at once: soft breaks, hard cuts, hard lines, blanks.
        let e = editor("one two three\nhttps://example.com/a\n\nfour");
        for columns in 1..12 {
            let rows = wrapped(&e, columns);
            let mut at = 0;
            for &(start, end) in &rows {
                // Contiguous, separated by at most the newline byte.
                assert!(start == at || start == at + 1, "a gap at {columns} columns");
                assert!(start <= end);
                at = end;
            }
            assert_eq!(at, e.text().len(), "a lost tail at {columns} columns");
        }
    }

    #[test]
    fn accented_rows_wrap_by_characters_rather_than_bytes() {
        let e = editor("café au lait");
        assert_eq!(rows_of(&e, 7), vec!["café au ", "lait"]);
    }

    #[test]
    fn a_caret_on_a_soft_break_waits_at_the_start_of_the_next_row() {
        let mut e = editor("one two three");
        let rows = wrapped(&e, 5);
        e.place(4, false); // between the rows "one " and "two "
        assert_eq!(e.caret_in(&rows), (1, 0));
        e.place(3, false); // still inside the first row
        assert_eq!(e.caret_in(&rows), (0, 3));
    }

    #[test]
    fn a_caret_at_the_end_of_a_line_stays_on_it_across_a_hard_break() {
        let mut e = editor("one\ntwo");
        let rows = wrapped(&e, ROOM);
        e.place(3, false); // before the newline
        assert_eq!(e.caret_in(&rows), (0, 3));
        e.place(4, false); // after it
        assert_eq!(e.caret_in(&rows), (1, 0));
    }

    // -----------------------------------------------------------------------
    // Word boundaries
    // -----------------------------------------------------------------------

    #[test]
    fn a_word_step_stops_at_punctuation_rather_than_jumping_it() {
        // The three-class rule. With two classes the dot belongs to the gap
        // between the words and there is nowhere to put the caret beside it.
        let mut e = editor("foo.bar");
        e.key("left", word(), None);
        assert_eq!(&e.text()[e.caret..], "bar");
        e.key("left", word(), None);
        assert_eq!(&e.text()[e.caret..], ".bar");
        e.key("left", word(), None);
        assert_eq!(e.caret, 0);
    }

    #[test]
    fn a_word_step_takes_the_spaces_with_the_run_that_follows_them() {
        let mut e = editor("one,  two");
        e.key("left", word(), None);
        assert_eq!(&e.text()[e.caret..], "two");
        // The spaces, then the comma — not the two together.
        e.key("left", word(), None);
        assert_eq!(&e.text()[e.caret..], ",  two");
    }

    #[test]
    fn a_run_of_punctuation_is_one_step() {
        let mut e = editor("a::b");
        e.place(4, false);
        e.key("left", word(), None);
        assert_eq!(&e.text()[e.caret..], "b");
        e.key("left", word(), None);
        assert_eq!(&e.text()[e.caret..], "::b", "both colons at once");
    }

    // -----------------------------------------------------------------------
    // Selecting by run and by line
    // -----------------------------------------------------------------------

    #[test]
    fn a_double_click_selects_the_word_it_landed_in() {
        let mut e = editor("one two three");
        e.select_word_at(5);
        assert_eq!(e.selected_text(), Some("two"));
        // From either edge of it, too — the far edge being the case that
        // makes the rule in `select_word_at` worth writing down.
        e.select_word_at(4);
        assert_eq!(e.selected_text(), Some("two"));
        e.select_word_at(7);
        assert_eq!(e.selected_text(), Some("two"));
    }

    #[test]
    fn a_double_click_between_a_word_and_a_space_takes_the_word() {
        let mut e = editor("one two");
        // Between `one` and the space: the word behind, not the gap ahead.
        e.select_word_at(3);
        assert_eq!(e.selected_text(), Some("one"));
    }

    #[test]
    fn a_double_click_in_the_gap_selects_the_gap() {
        // Rather than guessing which of the two neighbouring words was meant.
        let mut e = editor("one   two");
        e.select_word_at(4);
        assert_eq!(e.selected_text(), Some("   "));
    }

    #[test]
    fn a_double_click_at_the_very_end_selects_the_last_word() {
        let mut e = editor("one two");
        e.select_word_at(7);
        assert_eq!(e.selected_text(), Some("two"));
    }

    #[test]
    fn a_double_click_on_nothing_selects_nothing_rather_than_panicking() {
        let mut e = editor("");
        e.select_word_at(0);
        assert!(e.selection().is_none());
    }

    #[test]
    fn a_triple_click_selects_the_line_enter_made_rather_than_the_row() {
        let mut e = editor("one\ntwo three\nfour");
        e.select_line_at(6);
        assert_eq!(e.selected_text(), Some("two three"));
        e.select_line_at(0);
        assert_eq!(e.selected_text(), Some("one"));
    }

    // -----------------------------------------------------------------------
    // Undo
    // -----------------------------------------------------------------------

    #[test]
    fn undo_takes_back_a_word_of_typing_rather_than_a_character() {
        let mut e = editor("");
        type_in(&mut e, "hello");
        assert!(e.undo());
        assert_eq!(e.text(), "", "a run of typing is one step");
        assert!(!e.undo(), "and there was only the one");
    }

    #[test]
    fn a_space_ends_the_group_so_undo_comes_back_a_word_at_a_time() {
        let mut e = editor("");
        type_in(&mut e, "one two");
        assert!(e.undo());
        assert_eq!(e.text(), "one ");
        assert!(e.undo());
        assert_eq!(e.text(), "");
    }

    #[test]
    fn typing_and_deleting_are_two_steps_rather_than_one() {
        let mut e = editor("");
        type_in(&mut e, "abc");
        e.key("backspace", plain(), None);
        e.key("backspace", plain(), None);
        assert_eq!(e.text(), "a");
        assert!(e.undo());
        assert_eq!(e.text(), "abc", "the deleting came back on its own");
        assert!(e.undo());
        assert_eq!(e.text(), "");
    }

    #[test]
    fn moving_the_caret_ends_the_run_of_typing() {
        let mut e = editor("");
        type_in(&mut e, "ab");
        e.place(0, false);
        type_in(&mut e, "xy");
        assert_eq!(e.text(), "xyab");
        assert!(e.undo());
        assert_eq!(e.text(), "ab", "only what was typed after the move");
    }

    #[test]
    fn undo_puts_the_caret_back_where_the_edit_started() {
        let mut e = editor("");
        type_in(&mut e, "hello");
        e.undo();
        assert_eq!(e.caret, 0);
        assert!(e.selection().is_none());
    }

    #[test]
    fn an_empty_stack_is_handed_back_so_the_board_can_take_the_press() {
        // The whole reason `undo` returns a bool: `Ignored` is what lets
        // `Ctrl Z` reach the board's own history once this one is spent.
        let mut e = editor("already here");
        let ctrl = Mods { secondary: true, ..plain() };
        assert_eq!(e.key("z", ctrl, Some("z")), Reply::Ignored);
        type_in(&mut e, "!");
        assert_eq!(e.key("z", ctrl, Some("z")), Reply::Held);
        assert_eq!(e.text(), "already here");
        assert_eq!(e.key("z", ctrl, Some("z")), Reply::Ignored);
    }

    #[test]
    fn redo_goes_back_forward_and_a_new_edit_drops_it() {
        let mut e = editor("");
        type_in(&mut e, "one ");
        type_in(&mut e, "two");
        e.undo();
        assert_eq!(e.text(), "one ");
        assert!(e.redo());
        assert_eq!(e.text(), "one two");

        e.undo();
        type_in(&mut e, "!");
        assert!(!e.redo(), "typing abandoned the branch that was ahead");
    }

    #[test]
    fn both_redo_keys_work() {
        let ctrl = Mods { secondary: true, ..plain() };
        let ctrl_shift = Mods { secondary: true, shift: true, ..plain() };
        for redo in [ctrl_shift, ctrl] {
            let mut e = editor("");
            type_in(&mut e, "x");
            e.key("z", ctrl, Some("z"));
            assert_eq!(e.text(), "");
            let key = if redo.shift { "z" } else { "y" };
            assert_eq!(e.key(key, redo, Some(key)), Reply::Held);
            assert_eq!(e.text(), "x");
        }
    }

    #[test]
    fn a_keystroke_into_a_full_field_leaves_no_step_that_undoes_nothing() {
        let mut e = Editor::new("xx", 2, true);
        type_in(&mut e, "y");
        assert_eq!(e.text(), "xx");
        assert!(!e.undo(), "nothing happened, so there is nothing to take back");
    }

    #[test]
    fn backspace_at_the_start_leaves_no_step_either() {
        let mut e = editor("abc");
        e.place(0, false);
        e.key("backspace", plain(), None);
        assert!(!e.undo());
    }

    #[test]
    fn a_paste_is_its_own_step_rather_than_joining_the_typing() {
        let mut e = editor("");
        type_in(&mut e, "ab");
        e.insert("PASTED");
        assert_eq!(e.text(), "abPASTED");
        assert!(e.undo());
        assert_eq!(e.text(), "ab");
    }

    #[test]
    fn the_stack_does_not_grow_without_end() {
        let mut e = Editor::new(String::new(), 100_000, true);
        // Each `insert` is a discrete step, so this is the fastest way to
        // push more of them than the cap allows.
        for _ in 0..(Editor::UNDO_MAX + 50) {
            e.insert("x");
        }
        let mut steps = 0;
        while e.undo() {
            steps += 1;
        }
        assert_eq!(steps, Editor::UNDO_MAX);
    }

    // -----------------------------------------------------------------------
    // Marked text
    // -----------------------------------------------------------------------

    #[test]
    fn composing_replaces_its_own_run_rather_than_stacking_up() {
        let mut e = editor("");
        e.replace_marked(None, "n", None);
        assert_eq!(e.text(), "n");
        assert_eq!(e.marked(), Some((0, 1)));
        // The next keystroke of the same composition replaces what was there.
        e.replace_marked(None, "に", None);
        assert_eq!(e.text(), "に");
        assert_eq!(e.marked(), Some((0, 3)));
        // And committing ends it.
        e.replace_text(None, "日");
        assert_eq!(e.text(), "日");
        assert!(e.marked().is_none());
    }

    #[test]
    fn composing_leaves_the_caret_where_the_platform_asked_for_it() {
        let mut e = editor("ab");
        e.replace_marked(None, "xyz", Some((1, 2)));
        assert_eq!(e.text(), "abxyz");
        assert_eq!(e.marked(), Some((2, 5)));
        assert_eq!(e.selected_text(), Some("y"));
    }

    #[test]
    fn composing_over_a_selection_replaces_it() {
        let mut e = editor("hello");
        e.select_all();
        e.replace_marked(None, "x", None);
        assert_eq!(e.text(), "x");
    }

    #[test]
    fn cancelling_a_composition_takes_its_text_back_out() {
        let mut e = editor("ab");
        e.replace_marked(None, "xy", None);
        assert_eq!(e.text(), "abxy");
        e.replace_text(None, "");
        assert_eq!(e.text(), "ab");
        assert!(e.marked().is_none());
    }

    #[test]
    fn typing_a_real_key_ends_a_composition() {
        let mut e = editor("");
        e.replace_marked(None, "n", None);
        type_in(&mut e, "!");
        assert!(e.marked().is_none(), "the mark cannot outlive the text it pointed at");
    }

    #[test]
    fn the_caret_never_lands_inside_a_character_after_composing() {
        let mut e = editor("");
        // A select range past the end of what went in, which a platform is
        // allowed to send and which would otherwise slice through the é.
        e.replace_marked(None, "é", Some((0, 99)));
        assert!(e.text().is_char_boundary(e.caret));
        assert!(e.text().is_char_boundary(e.anchor));
    }

    // -----------------------------------------------------------------------
    // The platform counts differently
    // -----------------------------------------------------------------------

    #[test]
    fn a_utf16_offset_comes_back_as_the_right_byte() {
        // `é` is two bytes and one unit; `𝄞` is four bytes and *two* units,
        // which is the case that catches a conversion written as a cast.
        let e = editor("aé𝄞b");
        assert_eq!(e.utf8_at(0), 0);
        assert_eq!(e.utf8_at(1), 1, "after the a");
        assert_eq!(e.utf8_at(2), 3, "after the é");
        assert_eq!(e.utf8_at(4), 7, "after both units of the clef");
        assert_eq!(e.utf8_at(5), 8, "after the b");
    }

    #[test]
    fn a_byte_offset_comes_back_as_the_right_utf16_offset() {
        let e = editor("aé𝄞b");
        for (byte, unit) in [(0, 0), (1, 1), (3, 2), (7, 4), (8, 5)] {
            assert_eq!(e.utf16_at(byte), unit, "byte {byte}");
        }
    }

    #[test]
    fn the_two_conversions_are_each_other_the_whole_way_along() {
        let e = editor("aé𝄞b\nsecond");
        for (byte, _) in e.text().char_indices().chain(std::iter::once((e.text().len(), ' '))) {
            assert_eq!(e.utf8_at(e.utf16_at(byte)), byte, "round trip at {byte}");
        }
    }

    #[test]
    fn an_offset_past_the_end_is_the_end_rather_than_a_panic() {
        // A platform describes the string it last heard about, which may be a
        // keystroke out of date.
        let e = editor("ab");
        assert_eq!(e.utf8_at(99), 2);
        assert_eq!(e.utf16_at(99), 2);
        let (text, from, to) = e.text_utf16(1, 99);
        assert_eq!((text, from, to), ("b", 1, 2));
    }

    #[test]
    fn a_collapsed_caret_is_an_empty_range_rather_than_nothing() {
        // `None` in the protocol means "this does not take text at all", which
        // would turn the input method off rather than tell it where the caret
        // is.
        let mut e = editor("hello");
        e.place(2, false);
        assert_eq!(e.selection_utf16(), (2, 2, false));
        e.place(4, true);
        assert_eq!(e.selection_utf16(), (2, 4, false));
        // And which end the caret is on has to survive, or a platform that
        // extends the selection would extend the wrong edge.
        e.place(0, true);
        assert_eq!(e.selection_utf16(), (0, 2, true));
    }

    #[test]
    fn the_marked_run_is_reported_in_the_platforms_units() {
        let mut e = editor("");
        e.replace_marked(None, "𝄞", None);
        assert_eq!(e.marked(), Some((0, 4)), "four bytes");
        assert_eq!(e.marked_utf16(), Some((0, 2)), "two units");
    }

    #[test]
    fn the_caret_line_is_where_the_drawing_side_would_put_it() {
        let e = {
            let mut e = editor("one\ntwo\nthree");
            e.place(9, false);
            e
        };
        assert_eq!(e.caret_line(), (2, 1));
    }
}
