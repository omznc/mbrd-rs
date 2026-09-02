//! Naming what you want instead of hunting for it.
//!
//! Two lists that are one mechanism. The **command** palette is `command.rs`'s
//! table with a query on it — the way to reach something whose menu you would
//! have to already know to look under, and the only way at all to reach the
//! third of the table that has no key. The **search** palette is the board's
//! own contents, and choosing a result *takes you there*.
//!
//! That second half is the one that matters on a canvas with no edges. A card
//! you cannot see is a card you have no way back to except by remembering
//! which direction you left it in; being told it exists would be no use if you
//! then had to fly across the board by hand. So a result does not merely
//! select — it moves the camera. See `BoardView::reveal`.
//!
//! One struct for both, because everything except *what is listed* and *what
//! Enter does* is identical: the query editing, the highlight, the chrome, the
//! rule that a press outside puts it away. Two structs would be two copies of
//! all of that, agreeing until one of them was changed.
//!
//! Typing is handled by hand rather than by a text field, for the reason
//! `switcher.rs` states: GPUI ships neither. What is here is what a one-line
//! query needs and nothing more — characters, backspace, and a caret that is
//! always at the end.

use gpui::{div, prelude::*, px, Context, FontWeight, Modifiers, MouseButton, ScrollHandle};
use mbrd_core::model::{Item, ItemType};
use mbrd_core::tags;

use crate::board_view::BoardView;
use crate::color::Tint;
use crate::command::Command;
use crate::editor::{self, Editor};
use crate::fuzzy;
use crate::icons::{icon, Icon};

/// How many matches to show.
///
/// Generous because the list scrolls and the highlight is followed into view.
/// It was twelve when it did neither, and twelve is the wrong number once it
/// does: a cap exists to stop the list being longer than the answer, and with
/// scrolling the only real cost is rows nobody looks at.
///
/// **It has to stay clear of `Command::all()`**, which is what
/// `an_empty_query_offers_every_command_rather_than_the_first_screenful` holds
/// it to: an empty query offers the whole table, and that is the point of it —
/// it is how somebody finds a command they did not know the name of. The table
/// passed a hundred entries when tags, the tour, the inventory and the shrink
/// landed, which is why this is no longer the hundred it was. A board is not
/// bounded that way, which is why there is a number here at all.
const SHOWN: usize = 250;

/// The longest a query may be.
///
/// Not a limit anybody will reach by typing — it is here because `Editor` wants
/// one, and a field with no ceiling is one a paste of a whole file fits into.
///
/// `pub(crate)` because the switcher's query is the same kind of field and
/// wants the same ceiling — see `switcher.rs`, which uses this rather than
/// writing its own copy of the same number.
pub(crate) const QUERY_MAX: usize = 256;

/// The header both of this app's modal pickers put their query on.
///
/// One pair of numbers rather than two: this panel and `switcher.rs`'s are the
/// same 560px shape opened by two different keystrokes, and a header that
/// changed padding between them read as the header jumping the instant you
/// pressed the other one — not a new panel, the *same* panel resized under
/// you. `pub(crate)` for the reason [`QUERY_MAX`] is: the switcher wants the
/// same two numbers, not a second guess at them.
pub(crate) const HEADER_PAD_X: f32 = 14.0;
pub(crate) const HEADER_PAD_Y: f32 = 7.0;

/// The longest a result's title may be before it is cut.
///
/// A note may be five hundred characters and a row is one line, so something
/// has to give. The first words of a note are what somebody recognises it by,
/// which is why this cuts the end rather than the middle.
const TITLE_MAX: usize = 64;

/// Which list is open.
///
/// Four now rather than the two the module note describes, and the last two
/// are the reason tags have no text field anywhere in this app. A tag is a word
/// you pick off a list far more often than one you invent — the second and
/// third tag anybody puts on a board are almost always tags already on it — and
/// this is already the surface for "type a few letters, choose from what
/// matches". So the list is the tags, and typing a name nothing matches offers
/// to make it. See [`Palette::coined`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Everything the app can be asked to do.
    Commands,
    /// Everything on the board.
    Search,
    /// Every tag on the board, ticked against what is selected. Choosing one
    /// puts it on the selection or takes it off.
    Tag,
    /// Every tag on the board, ticked against what is being filtered by.
    /// Choosing one adds it to the filter or takes it out.
    Filter,
}

impl Mode {
    /// What the empty query says, which is the only place the palette explains
    /// itself. A placeholder is read once and then never again, so it says
    /// what to type rather than what the thing is called.
    fn prompt(self) -> &'static str {
        match self {
            Self::Commands => "what do you want to do\u{2026}",
            Self::Search => "find something on the board\u{2026}",
            // The one placeholder here that teaches rather than prompts. The
            // separator is the single thing about tags worth knowing, and this
            // is where somebody is standing when it matters.
            Self::Tag => "tag it, or type a new one\u{2026}",
            Self::Filter => "show only what is tagged\u{2026}",
        }
    }

    fn nothing_yet(self) -> &'static str {
        match self {
            Self::Commands => "no command by that name",
            Self::Search => "nothing on this board by that name",
            // Never seen with anything typed — a query that matches no tag
            // becomes the offer to make one, so this only stands on a board
            // that has no tags at all and nothing typed yet.
            Self::Tag => "no tags yet — type one",
            Self::Filter => "nothing on this board is tagged",
        }
    }

    /// The word on the chip beside the query. Named in the same lowercase,
    /// unpunctuated voice as the placeholder beside it — a chip that shouted
    /// its mode in Title Case would read as a label pinned to the palette
    /// rather than as something the palette is quietly telling you.
    fn chip(self) -> &'static str {
        match self {
            Self::Commands => "commands",
            Self::Search => "on this board",
            Self::Tag => "tags",
            Self::Filter => "showing",
        }
    }
}

impl Mode {
    /// What the number at the right of the header counts.
    ///
    /// The chip on the left says which list this is; this says how big the
    /// haystack is, which is the other half of "am I looking in the right
    /// place". `None` for the command list, where the answer is always the
    /// same number and therefore tells nobody anything.
    fn tally(self, view: &BoardView, rows: usize) -> Option<String> {
        match self {
            Self::Commands => None,
            Self::Search => Some(format!("{} on this board", rows)),
            // What the choice will land on, not what the list holds: a tag
            // row is an instruction aimed at the selection, and the size of
            // the selection is the fact that changes what pressing it does.
            Self::Tag => Some(cards(view.selection.len()) + " selected"),
            Self::Filter => Some(format!("{} tagged", rows)),
        }
    }
}

/// One line of an open palette.
#[derive(Debug, Clone)]
pub enum What {
    /// Something to do. Drawn dimmed when it would not achieve anything,
    /// **which is the opposite of what the context menu does with the same
    /// answer** — see `Entry::shown`, which leaves those rows out entirely.
    ///
    /// The two are different surfaces answering different questions. A menu
    /// is about the thing under the pointer, so a row that does not apply to
    /// it is noise. This is the whole table, and the reason somebody opens it
    /// is often to find out *whether* a thing exists and why it is not
    /// working — so a command that vanished here would be a command you
    /// cannot be told about. Dimmed says "yes, and not right now"; absent
    /// says "no such thing".
    Does(Command),
    /// Somewhere to go, named by the item's id.
    Goes { id: String, title: String, kind: &'static str, mark: Icon },
    /// A tag to put on the selection, or take off it.
    ///
    /// `on` is ticked only when **everything** taggable in the selection
    /// carries it, which is what makes the tick mean "true of this selection"
    /// rather than "true of one of them". A partly-tagged selection therefore
    /// shows unticked, and choosing the row tags the rest — which is the way
    /// round somebody means it.
    Marks { tag: String, count: usize, on: bool, coined: bool },
    /// A tag to add to the filter, or take out of it.
    Filters { tag: String, count: usize, on: bool },
}

/// One candidate, with the text it is matched against already folded.
///
/// Folded once at open rather than once per keystroke: a board may hold twenty
/// thousand items, and lowercasing all of them on every letter is a stutter you
/// can feel.
#[derive(Debug, Clone)]
pub struct Row {
    pub what: What,
    hay: String,
}

impl Row {
    /// Whether this row is the tag named. Answers `false` for every row that
    /// is not a tag at all, which is what lets [`Palette::recoin`] ask the
    /// question without caring which list it is looking at.
    fn names(&self, tag: &str) -> bool {
        match &self.what {
            What::Marks { tag: t, .. } | What::Filters { tag: t, .. } => t == tag,
            _ => false,
        }
    }
}

/// An open palette: which list, what has been typed, and where the highlight is.
#[derive(Debug, Clone)]
pub struct Palette {
    pub mode: Mode,
    /// What has been typed, as a real text field rather than a `String`.
    ///
    /// `editor.rs` already answers every question a one-line query asks —
    /// where the caret is, what is selected, what `Ctrl A` and `Ctrl
    /// Backspace` and Home mean — and a palette that reimplemented the easy
    /// third of that would be a text field where half the keys somebody
    /// reaches for silently do nothing. Which is exactly what it was.
    pub query: Editor,
    /// Which of the *matches* is highlighted, not which of the rows.
    pub cursor: usize,
    /// Where the list is scrolled to.
    ///
    /// Shared rather than copied when the palette is cloned for a frame —
    /// `ScrollHandle` is a handle, so the clone the painter gets is the same
    /// scroll position the next keystroke moves.
    pub scroll: ScrollHandle,
    rows: Vec<Row>,
    /// The offer to make a tag that does not exist yet, from whatever is in
    /// the query. `None` in every mode but [`Mode::Tag`], and `None` there
    /// whenever the query is empty or already names a tag the board has.
    ///
    /// Kept beside `rows` rather than in it because it is derived from the
    /// *query*, and `rows` is the list the query is scored against — a
    /// candidate that changed with every keystroke would have to be scored
    /// against itself.
    coined: Option<Row>,
    /// The rows the query currently earns, best first, already cut to
    /// [`SHOWN`]. Scored when the query *changes* rather than when a frame
    /// draws: painting is per-frame and typing is not, and fuzzy-scoring
    /// twenty thousand rows to answer a question whose inputs have not moved
    /// was most of what a frame with the palette open cost.
    found: Vec<usize>,
}

impl Palette {
    /// Gather the candidates. Done once, when it opens, for the reason
    /// `Switcher::open` does it: the board cannot change while the palette owns
    /// the keyboard, so re-deriving the list per keystroke would be the same
    /// answer computed again.
    pub fn open(mode: Mode, view: &BoardView) -> Self {
        let mut open = Self {
            mode,
            query: Editor::new("", QUERY_MAX, false),
            cursor: 0,
            scroll: ScrollHandle::new(),
            rows: rows_for(mode, view),
            coined: None,
            found: Vec::new(),
        };
        open.rescore();
        open
    }

    /// Take a fresh list, keeping the query and the highlight where they are.
    ///
    /// The tag lists are the only ones that need this, and they need it for a
    /// reason the other two do not have: choosing a row **changes the board**
    /// and the palette stays open, so the ticks it is drawing are about a
    /// board one step older than the one on screen. Rows come in from outside
    /// rather than being re-derived here because the caller is the view, and
    /// the view cannot lend itself out while its own overlay is borrowed.
    pub fn refill(&mut self, rows: Vec<Row>) {
        self.rows = rows;
        // The highlight is *not* reset. Somebody ticking three tags in a row
        // is aiming at the same place each time, and a list that jumped back
        // to the top under them would make the second tick land on the wrong
        // one.
        self.rescore();
        self.cursor = self.cursor.min(self.matches().len().saturating_sub(1));
    }

    /// The rows worth showing, best first. Reads the list [`rescore`] left;
    /// see `found` for why the scoring does not happen here.
    ///
    /// The coined row, where there is one, is always first: it is the answer
    /// to what was just typed, and the rows under it are the ones that merely
    /// resemble it.
    pub fn matches(&self) -> Vec<&Row> {
        let mut out = Vec::with_capacity(self.found.len() + 1);
        out.extend(self.coined.as_ref());
        out.extend(self.found.iter().map(|&i| &self.rows[i]));
        out
    }

    /// Score the rows against the query. Called wherever the query can change,
    /// which is `open`, `key` and `insert` — and nowhere per frame.
    fn rescore(&mut self) {
        self.recoin();
        if self.query.text().is_empty() {
            self.found = (0..self.rows.len().min(SHOWN)).collect();
            return;
        }
        let query = self.query.text().to_lowercase();
        let mut scored: Vec<(i32, usize)> = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(i, row)| fuzzy::subsequence(&query, &row.hay).map(|s| (s, i)))
            .collect();
        // Best score first, and where two score the same the one that was
        // already higher up — which for commands is the order `all()` offers
        // them in and for a board is the order the items are stored in.
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        self.found = scored.into_iter().take(SHOWN).map(|(_, i)| i).collect();
    }

    /// What Enter would do.
    pub fn chosen(&self) -> Option<What> {
        self.matches().get(self.cursor).map(|row| row.what.clone())
    }

    /// Take a key press. Answers what the view should do about it.
    ///
    /// Four keys are the palette's and everything else is the text field's.
    /// The four are the ones that mean something different here than they
    /// would inside a line of text: Escape leaves, Enter chooses, and the
    /// arrows move through the *list* rather than through the query — a
    /// one-line field has no line above it to walk to, so Up and Down are free
    /// and choosing a row is what they are obviously for.
    ///
    /// Everything else — the caret, the selection, `Ctrl A`, word motion,
    /// Home and End, deleting a word — goes to [`Editor`], which already
    /// knows all of it.
    pub fn key(&mut self, key: &str, mods: Modifiers, text: Option<&str>) -> Reply {
        match key {
            "escape" => return Reply::Close,
            // Tab used to run the row too, on the theory that it was another
            // way to say "done, go". It is also what `Editor` reads as
            // "commit and hand focus onward" in a form with more than one
            // field, and a query is a form with one — so tab here is asked to
            // mean nothing rather than something a single-field palette has
            // no use for. Enter is the one true "go".
            "enter" => return Reply::Run,
            "up" => {
                self.step(-1);
                return Reply::Held;
            }
            "down" => {
                self.step(1);
                return Reply::Held;
            }
            // A page at a time, for a list long enough that the arrows are
            // the slow way across it — the same ten the note editor scrolls
            // by. Ahead of the text field's own keys, same as Up and Down,
            // because a one-line query has no page of its own to page
            // through.
            "pageup" => {
                self.step(-10);
                return Reply::Held;
            }
            "pagedown" => {
                self.step(10);
                return Reply::Held;
            }
            _ => {}
        }

        let before = self.query.text().to_string();
        let reply = self.query.key(key, editor::Mods::from(mods), text);
        // Only what the text field would not take. Paste is the one worth
        // having — a name or an address copied from somewhere else is exactly
        // what somebody searches for — and it needs the clipboard, which is
        // the view's rather than ours.
        if reply == editor::Reply::Ignored && mods.secondary() && key == "v" {
            return Reply::Paste;
        }
        // The highlight goes back to the top when the *query* changes, and
        // only then. Selecting all of it or walking the caret through it
        // narrows nothing, so it should not move what is highlighted — and
        // the scores cannot have moved either, so they are only re-earned
        // here, behind the same test.
        if self.query.text() != before {
            self.cursor = 0;
            self.rescore();
        }
        Reply::Held
    }

    /// Work out whether what has been typed names a tag that does not exist
    /// yet, and offer to make it if so. See [`Self::coined`].
    ///
    /// Cleaned rather than taken literally, so the row offers the tag that
    /// would actually be stored — somebody typing "Warm Greys!" is shown "warm
    /// greys", which is what they will get and what they will see in the list
    /// next time.
    fn recoin(&mut self) {
        self.coined = None;
        if self.mode != Mode::Tag {
            return;
        }
        let tag = mbrd_core::tags::clean(self.query.text());
        if tag.is_empty() || self.rows.iter().any(|row| row.names(&tag)) {
            return;
        }
        self.coined = Some(Row {
            hay: tag.clone(),
            what: What::Marks { tag, count: 0, on: false, coined: true },
        });
    }

    /// Put text into the query, at the caret. For a paste.
    pub fn insert(&mut self, text: &str) {
        self.query.insert(text);
        self.cursor = 0;
        self.rescore();
    }

    /// Move the highlight, stopping at both ends rather than wrapping.
    ///
    /// Wrapping is the wrong behaviour for a list you are aiming at: holding
    /// Down to reach the bottom should end at the bottom, not start again.
    fn step(&mut self, by: i32) {
        let last = self.matches().len().saturating_sub(1);
        self.cursor = (self.cursor as i32 + by).clamp(0, last as i32) as usize;
    }
}

/// What the view should do with a key the palette was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    /// Dealt with. Redraw.
    Held,
    /// Put it away and change nothing.
    Close,
    /// Do what is highlighted.
    Run,
    /// Put the clipboard into the query. The view has the clipboard.
    Paste,
}

/// Everything a mode lists, gathered off the board.
///
/// Free-standing rather than a method, because it is called twice from two
/// sides: once by [`Palette::open`], which has the view and no palette, and
/// once by the view after a tag has been ticked, which has the view and cannot
/// lend it to the palette it is holding. See [`Palette::refill`].
pub fn rows_for(mode: Mode, view: &BoardView) -> Vec<Row> {
    let items = &view.doc.board.items;
    match mode {
        Mode::Commands => Command::all()
            .into_iter()
            .map(|command| Row {
                // The label and whatever else somebody might call it. See
                // `Command::keywords`, which exists because the board switcher
                // is named "Open board…" and gets hunted for as "project".
                hay: format!("{} {}", command.label().to_lowercase(), command.keywords()),
                what: What::Does(command),
            })
            .collect(),
        Mode::Search => items.iter().filter(|item| item.kind.is_content()).map(row_for).collect(),
        Mode::Tag => {
            // What the tick means: carried by *everything* taggable that is
            // selected. See `What::Marks`.
            let selected: Vec<&Item> = items
                .iter()
                .filter(|item| view.selection.contains(&item.id) && tags::taggable(item))
                .collect();
            tags::census(items)
                .into_iter()
                .map(|(tag, count)| Row {
                    hay: tag.clone(),
                    what: What::Marks {
                        on: !selected.is_empty()
                            && selected.iter().all(|item| tags::of(item).contains(&tag)),
                        tag,
                        count,
                        coined: false,
                    },
                })
                .collect()
        }
        Mode::Filter => tags::census(items)
            .into_iter()
            .map(|(tag, count)| Row {
                hay: tag.clone(),
                what: What::Filters { on: view.tag_filter.contains(&tag), tag, count },
            })
            .collect(),
    }
}

/// How many cards wear a tag, spelled for the right-hand side of a row.
fn cards(count: usize) -> String {
    match count {
        1 => "1 card".into(),
        n => format!("{n} cards"),
    }
}

/// One item, as a row.
///
/// The title is what somebody would recognise the card by, which is not always
/// its name: a note's words are its identity and its `name` is usually empty,
/// and a link is the address rather than whatever it was labelled. The haystack
/// is wider than the title on purpose — a note is findable by any of its words,
/// not only by the ones that fit on the row.
fn row_for(item: &Item) -> Row {
    let kind = kind_word(&item.kind);
    let title = match &item.kind {
        ItemType::Note | ItemType::Text => first_line(item.note_text().unwrap_or(&item.name)),
        ItemType::Link => item.url().unwrap_or(&item.name).to_string(),
        _ if item.name.is_empty() => kind.to_string(),
        _ => item.name.clone(),
    };
    // Name, words and address all together: three fields, one query. Somebody
    // looking for a card does not know or care which of the three the letters
    // they remember are in.
    let mut hay = item.name.to_lowercase();
    for extra in [item.note_text(), item.url()].into_iter().flatten() {
        hay.push(' ');
        hay.push_str(&extra.to_lowercase());
    }
    // And its tags, which are the one part of a card somebody chose *in order
    // to find it again*. Already lower case — see `mbrd_core::tags::clean`.
    for tag in tags::of(item) {
        hay.push(' ');
        hay.push_str(&tag);
    }
    hay.push(' ');
    hay.push_str(kind);

    Row {
        what: What::Goes {
            id: item.id.clone(),
            title: cut(&title),
            kind,
            mark: Icon::for_kind(&item.kind),
        },
        hay,
    }
}

/// The word a row shows down its right-hand side to say what kind of thing it
/// is. The format's own name where that reads as English, which it mostly does.
fn kind_word(kind: &ItemType) -> &'static str {
    match kind {
        ItemType::Image => "image",
        ItemType::Video => "video",
        ItemType::Audio => "audio",
        ItemType::Note => "note",
        ItemType::Link => "link",
        ItemType::Text => "text",
        ItemType::Model => "model",
        ItemType::Swatch => "color",
        ItemType::Sticker => "sticker",
        ItemType::Gone => "gone",
        // Furniture never reaches here — `is_content` filtered it out — and
        // the rest are types this build does not know by name.
        _ => "card",
    }
}

/// The first line of something that may be many, for a row that is one.
fn first_line(text: &str) -> String {
    text.lines().find(|line| !line.trim().is_empty()).unwrap_or("").trim().to_string()
}

/// Cut to something that fits a row, on a character boundary.
fn cut(text: &str) -> String {
    if text.chars().count() <= TITLE_MAX {
        return text.to_string();
    }
    let kept: String = text.chars().take(TITLE_MAX - 1).collect();
    format!("{}\u{2026}", kept.trim_end())
}

/// The query, with its caret and whatever is selected.
///
/// Drawn as elements rather than as text with a bar character in it.
/// `\u{2502}` is a *box-drawing* glyph: it occupies a full character cell with
/// the stroke down the middle, so it renders as a gap and then a line — which
/// is the "space between the caret and the word" this used to show. A caret is
/// not a character and cannot be spelled as one; it is a two-pixel rule
/// between two characters, which is what the note editor paints and what this
/// does now.
///
/// `pub(crate)` and taken generically as an `Editor` rather than a `Palette` —
/// `switcher.rs`'s query is the same one-line field with the same caret and
/// selection, and drawing it a second way there would be two chances for the
/// two to drift apart instead of one place that both call.
/// `focused` is what decides whether the caret and the selection wash are
/// drawn at all. The palette and the switcher are modes — while one is up it
/// is the only thing taking keys, so they pass `true` and never think about
/// it. The settings page is not a mode: it is a page with a search field on
/// it, and a caret parked in that field on every frame it is open says the
/// page has been seized by its own search box.
pub(crate) fn query_line(
    editor: &Editor,
    placeholder: &str,
    size: f32,
    focused: bool,
    theme: &crate::theme::Theme,
) -> gpui::AnyElement {
    // Three pixels taller than the letters, which is roughly the line box they
    // sit in: a caret the exact height of the text reads as a short tick, and
    // one of a fixed height reads as the wrong caret in every field but the
    // one it was measured against — which is what happened when the settings
    // page became the third caller at the third size.
    let caret = || div().flex_none().w(px(2.0)).h(px(size + 3.0)).bg(theme.accent);
    let text = editor.text().to_string();

    if text.is_empty() {
        return div()
            .flex()
            .items_center()
            .text_size(px(size))
            .when(focused, |d| d.child(caret()))
            .child(
                div()
                    .when(focused, |d| d.pl(px(4.0)))
                    .text_color(theme.muted)
                    .child(placeholder.to_string()),
            )
            .into_any_element();
    }

    // Nothing to mark up: no caret to place and no selection to wash, because
    // neither exists anywhere but in the field somebody is in.
    if !focused {
        return div()
            .flex()
            .items_center()
            .text_size(px(size))
            .child(div().child(text))
            .into_any_element();
    }

    // A single line, so the caret's column *is* its byte offset.
    let at = editor.caret_line().1;
    let (from, to) = editor.selection().unwrap_or((at, at));
    let wash = theme.accent.opacity(0.30);

    let mut row = div()
        .flex()
        .items_center()
        .text_size(px(size))
        .child(div().child(text[..from].to_string()));
    if from == to {
        row = row.child(caret());
    } else {
        // The caret sits at whichever end of the selection it was walked to,
        // so extending with Shift grows from the end you are moving.
        if at == from {
            row = row.child(caret());
        }
        row = row.child(div().bg(wash).child(text[from..to].to_string()));
        if at == to {
            row = row.child(caret());
        }
    }
    row.child(div().child(text[to..].to_string())).into_any_element()
}

pub fn render(
    palette: &Palette,
    view: &BoardView,
    cx: &mut Context<BoardView>,
) -> impl IntoElement {
    let theme = view.theme;
    let presence = view.overlay_presence.value();
    let matches = palette.matches();
    palette.scroll.scroll_to_item(palette.cursor);

    // Whether the footer has a second sentence to say. Collected as the rows
    // are built rather than asked again afterwards: `Command::available` is
    // the answer to a question about the whole board, and asking it twice per
    // row per frame is twice the work for a fact already in hand.
    let mut dimmed = false;
    let rows: Vec<_> = matches
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let highlighted = i == palette.cursor;
            // Dimmed rather than left out, which is deliberately not what a
            // menu does with the same answer. See `What::Does`.
            let live = match &row.what {
                What::Does(command) => command.available(view),
                What::Goes { .. } | What::Marks { .. } | What::Filters { .. } => true,
            };
            dimmed |= !live;
            let (title, aside) = match &row.what {
                What::Does(command) => (command.label().to_string(), command.hint().to_string()),
                What::Goes { title, kind, .. } => (title.clone(), kind.to_string()),
                // A tag that does not exist yet says so instead of saying how
                // many cards wear it, which for a new one is always none —
                // and "0 cards" beside a name reads as a tag that failed
                // rather than one about to be made.
                What::Marks { tag, coined: true, .. } => (tag.clone(), "new tag".into()),
                What::Marks { tag, count, .. } | What::Filters { tag, count, .. } => {
                    (tag.clone(), cards(*count))
                }
            };
            // A setting that is on says so, so the row reads as a choice
            // already made rather than as an instruction; and a card says what
            // kind of card it is, which is the same question one step over.
            let leading = match &row.what {
                // Ticked where it names a state, and otherwise the picture the
                // same command wears on a menu — one table, so the two lists
                // cannot end up drawing the same command two different ways.
                // See `Command::mark`.
                What::Does(command) => Some(match command.ticked(view) == Some(true) {
                    true => (Icon::Check, theme.accent),
                    false => (
                        command.mark(),
                        match command {
                            // The swatch, in the colour it stands for. The
                            // menu draws these as a bare square of colour —
                            // see `menu::row` — and this is the nearest a list
                            // of icons in one gutter can get to the same
                            // thing.
                            Command::ConnColour(colour) => theme.rope_for(*colour),
                            c if c.destructive() => theme.rope_danger,
                            _ => theme.muted,
                        },
                    ),
                }),
                What::Goes { mark, .. } => Some((*mark, theme.muted)),
                // Ticked exactly the way a setting is, and for the same
                // reason: the list is a state to read as much as a set of
                // instructions to follow. A tag being coined has nothing to
                // tick — it is not on anything yet — and gets the mark every
                // other tag row wears so the column does not go ragged.
                What::Marks { on, .. } | What::Filters { on, .. } => Some(match on {
                    true => (Icon::Check, theme.accent),
                    false => (Icon::Tag, theme.muted),
                }),
            };
            let what = row.what.clone();
            div()
                .id(i)
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .px(px(12.0))
                .py(px(7.0))
                .mx(px(6.0))
                .rounded(px(crate::theme::RADIUS_SM))
                .when(highlighted && live, |d| d.bg(theme.accent.opacity(0.20)))
                .when(live, |d| {
                    d.hover(|s| s.bg(theme.accent.opacity(0.12)))
                        .active(|s| s.bg(theme.accent.opacity(0.3)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _event, window, cx| {
                                this.run_palette_row(what.clone(), window, cx);
                            }),
                        )
                })
                .when(!live, |d| d.text_color(theme.muted))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .overflow_hidden()
                        // One slot, whichever of the two the row has. A tick
                        // and a kind picture are both "what this row is" and
                        // no row is ever both, so giving them separate gutters
                        // would step every title in the list left and right
                        // depending on its neighbours.
                        .child(
                            div()
                                .flex_none()
                                .w(px(crate::icons::ICON_MD))
                                .flex()
                                .items_center()
                                .when_some(leading, |d, (mark, colour)| {
                                    d.child(icon(mark, crate::icons::ICON_MD, colour))
                                }),
                        )
                        // Medium rather than regular: a row's title is the
                        // thing this list exists to let you aim at, and
                        // weight is the one signal here that thirteen pixels
                        // and a muted aside were not already carrying.
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::MEDIUM)
                                .truncate()
                                .child(title),
                        ),
                )
                .child(div().text_size(px(11.0)).text_color(theme.muted).child(aside))
                .into_any_element()
        })
        .collect();

    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        .flex()
        .justify_center()
        // As tall as what is in it. The cross-axis default is stretch, which
        // takes the panel to its `max_h` however few rows survived the query
        // and leaves the footer stranded above a band of empty chrome.
        .items_start()
        // Not centred vertically: a list that grows downward from a fixed
        // point does not move the thing you are aiming at as you type.
        .pt(px(96.0))
        // A scrim, faded in with the panel, for the reason `switcher.rs`
        // gives its own: the board behind a modal list should read as
        // *behind* it, not merely covered by it.
        .bg(theme.ground.opacity(0.25 * presence))
        // The wheel belongs to the list, not to the board behind it. GPUI
        // registers the list's own scrolling after this and runs it first, so
        // by the time the wheel arrives here the list has already moved and
        // all that is left is to stop it zooming the canvas — see menu.rs,
        // which does the same thing for the same reason.
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        // A press anywhere outside puts it away, which is what every other
        // palette does and what stops it becoming something you have to
        // dismiss deliberately.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _event, _window, cx| {
                this.close_palette();
                cx.notify();
            }),
        )
        // The right button too, or a press outside puts the palette away *and*
        // opens the board's own menu behind it — a menu about a card nobody
        // could see, belonging to a surface nobody was pressing on.
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(|this, _event, _window, cx| {
                cx.stop_propagation();
                this.close_palette();
                cx.notify();
            }),
        )
        .child(
            div()
                .w(px(560.0))
                .max_h(px(440.0))
                .flex()
                .flex_col()
                .rounded(px(crate::theme::RADIUS_LG))
                // Slides down 8px as it arrives and back up the way it came
                // as it leaves — see `switcher.rs`'s `render`, which uses the
                // same offset for the same reason: one function of the
                // current presence, so the exit is the entrance played
                // backwards rather than a second animation to keep in step.
                .mt(px(-(8.0 * (1.0 - presence))))
                .opacity(presence)
                .bg(theme.chrome)
                .border_1()
                .border_color(theme.chrome_edge)
                .shadow(theme.shadow_large())
                .text_color(theme.text)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                // Nothing in here has a context menu, and the board behind it
                // has one — so without this, right-pressing a row opens the
                // menu for whatever card happens to be under the list.
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(HEADER_PAD_X))
                        .py(px(HEADER_PAD_Y))
                        .border_b_1()
                        .border_color(theme.chrome_edge)
                        .text_size(px(14.0))
                        // Which list this is, said once rather than left to
                        // be inferred from the placeholder alone — a chip
                        // that stays put while the query it sits beside is
                        // cleared and retyped a dozen times.
                        .child(
                            div()
                                .flex_none()
                                .px(px(7.0))
                                .py(px(2.0))
                                .rounded(px(4.0))
                                .bg(theme.accent.opacity(0.14))
                                .text_size(px(10.0))
                                .text_color(theme.accent_text)
                                .child(palette.mode.chip()),
                        )
                        .child(div().flex_1().min_w_0().child(query_line(
                            &palette.query,
                            palette.mode.prompt(),
                            14.0,
                            true,
                            &theme,
                        )))
                        // How big the haystack is, set in the numeric face so
                        // it holds still as the query narrows it — the same
                        // argument the status bar's readings make. See
                        // [`Mode::tally`].
                        .when_some(palette.mode.tally(view, matches.len()), |d, tally| {
                            d.child(
                                div()
                                    .flex_none()
                                    .font(crate::opened::mono())
                                    .text_size(px(10.5))
                                    .text_color(theme.tertiary)
                                    .child(tally),
                            )
                        }),
                )
                .child(
                    div()
                        .id("palette-rows")
                        .flex()
                        .flex_col()
                        .py(px(6.0))
                        // Arrowing past the bottom of twelve rows has to bring
                        // the highlight with it, or the list scrolls and the
                        // thing you are aiming at does not.
                        .overflow_y_scroll()
                        .track_scroll(&palette.scroll)
                        .when(rows.is_empty(), |d| {
                            d.px(px(14.0))
                                .py(px(10.0))
                                .text_size(px(12.0))
                                .text_color(theme.muted)
                                // Looser than the app's chrome leading: this
                                // is the one line of prose in the whole
                                // palette, and prose that wraps wants the air
                                // between its lines that a status line does
                                // not.
                                .line_height(gpui::relative(1.45))
                                .child(palette.mode.nothing_yet())
                        })
                        .children(rows),
                )
                // Names the keys that leave this mode — the same rule
                // `switcher.rs` follows in its own footer.
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(14.0))
                        .px(px(14.0))
                        .py(px(7.0))
                        .border_t_1()
                        .border_color(theme.chrome_edge)
                        .text_size(px(10.0))
                        .text_color(theme.muted)
                        // Said only while there is a dimmed row on the screen
                        // to be confused by. Dimming rather than hiding is a
                        // deliberate choice — see `What::Does` — and the whole
                        // of what it buys is that the row is *findable* when
                        // it cannot run; a reader who has just found one and
                        // pressed it needs the reason in the same glance, and
                        // a reader who has not should not be told about a
                        // state the list is not in.
                        .when(dimmed, |d| {
                            d.child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .child("Dimmed rows exist but can't run right now"),
                            )
                        })
                        .when(!dimmed, |d| d.child(div().flex_1()))
                        .child(
                            div()
                                .flex_none()
                                .font(crate::opened::mono())
                                .text_size(px(10.0))
                                .child("\u{2191}\u{2193} move · \u{23ce} run · esc close"),
                        ),
                ),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(id: &str, words: &str) -> Item {
        let mut item = Item::new(id, ItemType::Note);
        item.meta.insert("text".into(), words.into());
        item
    }

    fn palette(rows: Vec<Row>) -> Palette {
        let mut p = Palette {
            mode: Mode::Search,
            query: Editor::new("", QUERY_MAX, false),
            cursor: 0,
            scroll: ScrollHandle::new(),
            rows,
            coined: None,
            found: Vec::new(),
        };
        p.rescore();
        p
    }

    fn titles(p: &Palette) -> Vec<String> {
        p.matches()
            .into_iter()
            .map(|row| match &row.what {
                What::Does(command) => command.label().to_string(),
                What::Goes { title, .. } => title.clone(),
                What::Marks { tag, .. } | What::Filters { tag, .. } => tag.clone(),
            })
            .collect()
    }

    /// Typing a tag the board does not have offers to make it, and the offer
    /// is the first row — it is the answer to what was just typed, and
    /// everything under it merely resembles it.
    #[test]
    fn a_tag_the_board_does_not_have_is_offered_rather_than_refused() {
        let mut p = palette(vec![Row {
            hay: "kitchen".into(),
            what: What::Marks { tag: "kitchen".into(), count: 2, on: false, coined: false },
        }]);
        p.mode = Mode::Tag;
        p.insert("kit");
        assert_eq!(titles(&p), ["kit", "kitchen"], "the offer first, the near miss under it");

        // And once it names one that exists, there is nothing to coin: the
        // row already there is the answer.
        p.insert("chen");
        assert_eq!(titles(&p), ["kitchen"]);
    }

    /// What would be stored, not what was typed. Somebody who types "Warm
    /// Greys!" is shown the tag they will actually get.
    #[test]
    fn the_offer_names_the_tag_that_would_be_stored() {
        let mut p = palette(Vec::new());
        p.mode = Mode::Tag;
        p.insert("Warm Greys!");
        assert_eq!(titles(&p), ["warm greys"]);
    }

    /// Nothing is coined anywhere else, or the command list would grow a row
    /// for every letter typed at it.
    #[test]
    fn only_the_tag_list_offers_to_make_something_up() {
        let mut p = palette(Vec::new());
        p.insert("kitchen");
        assert!(titles(&p).is_empty());
    }

    #[test]
    fn a_note_is_found_by_its_words_and_not_only_by_its_name() {
        // The whole reason search exists on this app: a note's identity is
        // what it says, and its `name` is usually empty.
        let mut p = palette(vec![row_for(&note("a", "buy the good coffee"))]);
        p.insert("coffee");
        assert_eq!(titles(&p), ["buy the good coffee"]);
    }

    #[test]
    fn a_link_is_found_by_its_address() {
        let mut item = Item::new("l", ItemType::Link);
        item.meta.insert("url".into(), "https://example.com/kitchen".into());
        let mut p = palette(vec![row_for(&item)]);
        p.insert("kitchen");
        assert_eq!(titles(&p), ["https://example.com/kitchen"]);
    }

    #[test]
    fn a_card_is_found_by_the_kind_of_thing_it_is() {
        // "image" finds the images, which is what somebody types when they
        // remember the sort of thing but not the name of it.
        let mut p = palette(vec![row_for(&Item::new("i", ItemType::Image))]);
        p.insert("image");
        assert_eq!(titles(&p).len(), 1);
    }

    #[test]
    fn a_long_note_is_cut_to_something_that_fits_a_row() {
        let long = "word ".repeat(40);
        let row = row_for(&note("a", &long));
        let What::Goes { title, .. } = row.what else { panic!("a note is somewhere to go") };
        assert!(title.chars().count() <= TITLE_MAX, "{} chars", title.chars().count());
        assert!(title.ends_with('\u{2026}'));
        // But still findable by a word past the cut.
        let mut p = palette(vec![row_for(&note("a", &long))]);
        p.insert("word");
        assert_eq!(p.matches().len(), 1);
    }

    #[test]
    fn only_the_first_line_of_a_note_becomes_its_title() {
        let row = row_for(&note("a", "the heading\nand then the body"));
        let What::Goes { title, .. } = row.what else { panic!("a note is somewhere to go") };
        assert_eq!(title, "the heading");
    }

    #[test]
    fn a_query_that_matches_nothing_offers_nothing() {
        let mut p = palette(vec![row_for(&note("a", "coffee"))]);
        p.insert("zzz");
        assert!(p.matches().is_empty());
        assert!(p.chosen().is_none());
    }

    #[test]
    fn an_empty_query_offers_every_command_rather_than_the_first_screenful() {
        // The cap is for boards, which have no bound on how many items they
        // hold. The command table has fewer entries than the cap, so opening
        // the palette and typing nothing shows all of it — which is how
        // somebody finds a command they did not know the name of.
        let rows: Vec<Row> = Command::all()
            .into_iter()
            .map(|c| Row { hay: c.label().to_lowercase(), what: What::Does(c) })
            .collect();
        let mut p = Palette {
            mode: Mode::Commands,
            query: Editor::new("", QUERY_MAX, false),
            cursor: 0,
            scroll: ScrollHandle::new(),
            rows,
            coined: None,
            found: Vec::new(),
        };
        p.rescore();
        assert_eq!(p.matches().len(), Command::all().len());
    }

    #[test]
    fn an_empty_query_offers_the_board_in_the_order_it_is_stored() {
        let p = palette(vec![row_for(&note("a", "one")), row_for(&note("b", "two"))]);
        assert_eq!(titles(&p), ["one", "two"]);
    }

    #[test]
    fn the_command_palette_offers_every_command_there_is() {
        // Including the third of the table that has no key and the six that
        // carry values — which is the half a menu cannot show you.
        let rows: Vec<Row> = Command::all()
            .into_iter()
            .map(|c| Row { hay: c.label().to_lowercase(), what: What::Does(c) })
            .collect();
        assert_eq!(rows.len(), Command::all().len());
        let mut p = Palette {
            mode: Mode::Commands,
            query: Editor::new("", QUERY_MAX, false),
            cursor: 0,
            scroll: ScrollHandle::new(),
            rows,
            coined: None,
            found: Vec::new(),
        };
        // A command with no key at all, reachable here and nowhere else by
        // typing.
        p.insert("align tops");
        assert_eq!(titles(&p), ["Align tops"]);
    }

    fn commands() -> Palette {
        let rows: Vec<Row> = Command::all()
            .into_iter()
            .map(|c| Row {
                hay: format!("{} {}", c.label().to_lowercase(), c.keywords()),
                what: What::Does(c),
            })
            .collect();
        let mut p = Palette {
            mode: Mode::Commands,
            query: Editor::new("", QUERY_MAX, false),
            cursor: 0,
            scroll: ScrollHandle::new(),
            rows,
            coined: None,
            found: Vec::new(),
        };
        p.rescore();
        p
    }

    #[test]
    fn the_board_switcher_is_found_by_what_people_call_it() {
        // Reported as "if i do shift shift theres no project switcher". It was
        // there and it is called "Open board…", which is not a word anybody
        // types when they are looking for it.
        for word in ["project", "switcher", "switch", "recent"] {
            let mut p = commands();
            p.insert(word);
            assert!(
                titles(&p).contains(&"Open board…".to_string()),
                "{word} did not find the board switcher",
            );
        }
    }

    #[test]
    fn reduced_motion_is_found_by_the_name_it_is_known_by() {
        // The setting is called "Animation"; the thing somebody looks for is
        // "reduced motion".
        let mut p = commands();
        p.insert("reduced motion");
        assert_eq!(titles(&p)[0], "Animation");
    }

    #[test]
    fn a_label_still_beats_a_keyword_that_merely_matches() {
        // Keywords widen the net; they must not outrank the thing actually
        // named what you typed, or every alias added makes the palette worse.
        let mut p = commands();
        p.insert("save");
        assert_eq!(titles(&p)[0], "Save");
    }

    #[test]
    fn the_highlight_stops_at_both_ends_rather_than_wrapping() {
        let mut p = palette(vec![row_for(&note("a", "one")), row_for(&note("b", "two"))]);
        p.step(-1);
        assert_eq!(p.cursor, 0);
        p.step(1);
        p.step(1);
        p.step(1);
        assert_eq!(p.cursor, 1);
    }

    #[test]
    fn typing_puts_the_highlight_back_at_the_top() {
        // Otherwise a query that narrows the list leaves the highlight
        // pointing at whatever happens to be in that position now.
        let mut p = palette(vec![row_for(&note("a", "one")), row_for(&note("b", "two"))]);
        p.cursor = 1;
        p.key("t", Modifiers::default(), Some("t"));
        assert_eq!(p.cursor, 0);
    }

    #[test]
    fn a_shortcut_typed_into_the_query_is_not_typed_into_the_query() {
        let mut p = palette(vec![row_for(&note("a", "one"))]);
        p.key("s", Modifiers::secondary_key(), Some("s"));
        assert_eq!(p.query.text(), "");
    }

    #[test]
    fn escape_and_enter_say_what_they_are_rather_than_being_typed() {
        let mut p = palette(vec![row_for(&note("a", "one"))]);
        assert_eq!(p.key("escape", Modifiers::default(), None), Reply::Close);
        assert_eq!(p.key("enter", Modifiers::default(), None), Reply::Run);
        assert_eq!(p.query.text(), "");
    }
}
