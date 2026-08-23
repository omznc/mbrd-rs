//! Typing into things.
//!
//! GPUI ships no text field, so this is one. It is deliberately *only* the
//! model: a string, a caret, a selection, and the rules for how key presses
//! move them. Nothing here knows what a pixel is, which is the same bargain
//! `mbrd-core` makes with the window and for the same reason — every rule below
//! is testable by writing text into it and asserting what came out, with no
//! window, no font and no event loop.
//!
//! What draws it, and the one thing that genuinely needs a font — working out
//! which character somebody clicked on — lives in `board_view.rs`, where the
//! text system is.
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
//! ## What is not here
//!
//! No input method support. A composing keyboard — Japanese, Korean, Chinese —
//! needs `Window::handle_input` and the `EntityInputHandler` protocol, which is
//! a larger thing and wants to be built once for the whole app rather than
//! twice. Latin, Cyrillic and Greek keyboards type correctly today because the
//! platform hands over finished characters; a composing one will type nothing
//! rather than type something wrong, which is the failure this is willing to
//! have until then.

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
}

impl Editor {
    pub fn new(text: impl Into<String>, limit: usize, multiline: bool) -> Self {
        let text = text.into();
        let caret = text.len();
        Self { text, caret, anchor: caret, goal: None, limit, multiline }
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

    /// Which line the caret is on, and how far along it, in bytes.
    ///
    /// What the drawing side needs to put a caret on screen, and the only shape
    /// in which the answer does not depend on a font.
    pub fn caret_line(&self) -> (usize, usize) {
        let before = &self.text[..self.caret];
        let row = before.matches('\n').count();
        let column = before.rfind('\n').map_or(self.caret, |at| self.caret - at - 1);
        (row, column)
    }

    /// The text as lines, split where somebody actually pressed Enter.
    ///
    /// Deliberately **not** wrapped. A wrap reflows, so a byte offset in the
    /// text no longer names a place on the screen — and a caret that is one
    /// character out is worse than a line that runs off the edge of the card,
    /// which the drawing side handles by scrolling to follow it.
    pub fn lines(&self) -> Vec<&str> {
        self.text.split('\n').collect()
    }

    /// The selection, cut up by line: `(row, start, end)` in bytes within each.
    ///
    /// The shape a painter wants, because it draws the wash one line at a time
    /// and a range spanning three lines is three rectangles rather than one.
    pub fn highlight(&self) -> Vec<(usize, usize, usize)> {
        let Some((from, to)) = self.selection() else { return Vec::new() };
        self.line_spans()
            .into_iter()
            .enumerate()
            .filter_map(|(row, (start, end))| {
                let lit_from = from.max(start);
                let lit_to = to.min(end);
                // A line entirely inside the selection but empty — a blank line
                // in the middle of a selected paragraph — has nothing to draw,
                // and drawing a zero-width wash would be a stray pixel.
                (lit_from < lit_to).then_some((row, lit_from - start, lit_to - start))
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
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
        self.goal = None;
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
            // Copy, cut and paste reach the clipboard, which is the platform's
            // rather than ours. Reported so the caller can do it and then say
            // what happened — see `Edit` in `board_view.rs`.
            "c" | "x" | "v" if mods.secondary => return Reply::Ignored,
            "z" if mods.secondary => return Reply::Ignored,

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
                self.insert(text);
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
        if let Some((from, to)) = self.selection() {
            self.text.replace_range(from..to, "");
            self.caret = from;
        }
        let room = self.limit.saturating_sub(self.text.chars().count());
        let fits: String = text.chars().take(room).collect();
        // A newline in a single-line field is somebody pasting a paragraph into
        // a name. Keep the words, lose the breaks.
        let fits = if self.multiline { fits } else { fits.replace(['\n', '\r'], " ") };
        self.text.insert_str(self.caret, &fits);
        self.caret += fits.len();
        self.anchor = self.caret;
        self.goal = None;
    }

    /// Delete: the selection if there is one, otherwise one character or word.
    fn erase(&mut self, forward: bool, word: bool) {
        if let Some((from, to)) = self.selection() {
            self.text.replace_range(from..to, "");
            self.caret = from;
            self.anchor = from;
            self.goal = None;
            return;
        }
        let to = if word { self.word_edge(forward) } else { self.step(self.caret, forward) };
        let (from, to) = (self.caret.min(to), self.caret.max(to));
        self.text.replace_range(from..to, "");
        self.caret = from;
        self.anchor = from;
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
                return;
            }
        }
        self.caret = if word { self.word_edge(forward) } else { self.step(self.caret, forward) };
        if !extend {
            self.anchor = self.caret;
        }
        self.goal = None;
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
    }

    /// The start or the end of the line the caret is on.
    fn jump_to_edge(&mut self, end: bool, extend: bool) {
        let (start, stop) = self.line_spans()[self.caret_line().0];
        self.caret = if end { stop } else { start };
        if !extend {
            self.anchor = self.caret;
        }
        self.goal = None;
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

    /// The far side of the run of word characters next to the caret.
    ///
    /// Whitespace first, then letters — so `Ctrl` and a left arrow in the
    /// middle of `  hello` lands before `hello` rather than after the spaces.
    fn word_edge(&self, forward: bool) -> usize {
        let mut at = self.caret;
        let class = |c: char| c.is_alphanumeric() || c == '_';
        let peek = |at: usize| -> Option<char> {
            if forward {
                self.text[at..].chars().next()
            } else {
                self.text[..at].chars().next_back()
            }
        };
        while peek(at).is_some_and(|c| !class(c)) {
            at = self.step(at, forward);
        }
        while peek(at).is_some_and(class) {
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

    #[test]
    fn a_selection_across_lines_comes_back_as_one_piece_per_line() {
        let mut e = editor("one\ntwo\nthree");
        e.place(1, false);
        e.place(9, true);
        assert_eq!(e.highlight(), vec![(0, 1, 3), (1, 0, 3), (2, 0, 1)]);
    }

    #[test]
    fn an_empty_line_inside_a_selection_has_nothing_to_draw() {
        let mut e = editor("a\n\nb");
        e.select_all();
        assert_eq!(e.highlight(), vec![(0, 0, 1), (2, 0, 1)]);
    }

    #[test]
    fn no_selection_is_nothing_to_draw_rather_than_an_empty_box() {
        let e = editor("hello");
        assert!(e.highlight().is_empty());
    }

    #[test]
    fn lines_are_where_enter_was_pressed_and_nowhere_else() {
        let e = editor("a very long line that a card would never fit\nand another");
        assert_eq!(e.lines().len(), 2);
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
