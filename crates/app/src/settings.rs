//! The settings page.
//!
//! Every switch the app has, on one surface, split the way `prefs.rs` is
//! built: **Board** rows write into the `.mbrd` through the ledger — they
//! travel with the file and undo can take them back — and **Application**
//! rows are about the person sitting here, live in their config directory,
//! and do neither. Each is a group in the sidebar, so the split is navigation
//! rather than small print.
//!
//! The shape is the one settings screens have converged on — a nav column on
//! the left, and on the right a column of rows where each setting is a name
//! with a sentence under it and its control at the far edge. The sentence is
//! the point: a switch called "Axes" tells you nothing at 2am, and this page
//! is the one place in the app with room to say what a thing does.
//!
//! ## Why the sidebar has two levels
//!
//! It had one, with two entries on it, which was the right shape for eight
//! rows and stopped being it somewhere around fifteen. A flat list scales by
//! making each page longer, and a page long enough to scroll is one where the
//! nav column has stopped answering "where is that setting" — the two-level
//! version answers it in the sidebar instead, which is what every settings
//! screen with more than a screenful of settings ends up doing.
//!
//! The groups are the two that already existed and the split they already
//! meant. Nothing was regrouped; the sections underneath are the *pages* that
//! used to be scroll positions.
//!
//! ## Why there is a search field
//!
//! Because two levels of navigation is two levels of guessing. Somebody
//! looking for the grid step knows the words "grid step" and does not
//! necessarily know it lives under Board rather than Application — so typing
//! flattens the whole page back into one list, and matches on the
//! descriptions as well as the titles, which is where the words people
//! actually remember tend to be. It is the same `fuzzy` matcher the palette
//! and the switcher use, over the same rows this page was going to draw
//! anyway.
//!
//! An overlay like the palette and the switcher — see `Overlay` in
//! `board_view.rs` for why there can only ever be one — but a whole page
//! rather than a floating panel, and not a list you aim at: pressing a row
//! does not close it, because settings are adjusted in twos and threes and a
//! page that shut on the first flip would have to be reopened for the
//! second. Escape and the close button are the ways out; there is no
//! "outside" left to press.
//!
//! Nothing here is a second implementation of anything. A toggle row *is*
//! its `Command` — current state and effect both read from the same table
//! the menus and the palette read — so this page cannot drift from what `G`
//! or the View menu does. The rows that are not commands (grid step, card
//! gap, media fit, the two themes) go through their own `BoardView` setters,
//! which go through the one door their kind of change goes through.

use std::collections::HashMap;

use gpui::{
    div, prelude::*, px, AnyElement, Context, FontWeight, Modifiers, MouseButton, SharedString,
};

use crate::board_view::BoardView;
// Only the update row reads it, and only a build that can update has one.
#[cfg(not(target_family = "wasm"))]
use crate::board_view::UpdateBadge;
use crate::color::Tint;
use crate::command::Command;
use crate::editor::{self, Editor};
use crate::icons::{icon, Icon};
use crate::prefs::Mode;
use crate::theme::Theme;
use crate::themes::Appearance;

/// One of the two halves the sidebar is divided into, which is the same
/// division `prefs.rs` is built on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Board,
    Application,
}

impl Group {
    pub const ALL: [Self; 2] = [Self::Board, Self::Application];

    pub fn label(self) -> &'static str {
        match self {
            Self::Board => "Board",
            Self::Application => "Application",
        }
    }

    /// Which slot of [`Page::open`] says whether this group is expanded.
    fn slot(self) -> usize {
        match self {
            Self::Board => 0,
            Self::Application => 1,
        }
    }

    fn sections(self) -> &'static [Section] {
        match self {
            Self::Board => &[Section::Canvas, Section::Arranging, Section::Media],
            // No Updates in a browser. A page has no version of itself on a
            // disk to replace, so a section about replacing one is a section
            // whose every row would have to explain why it does nothing. See
            // `Command::available`, which hides the same two rows everywhere
            // else they are reachable from.
            #[cfg(target_family = "wasm")]
            Self::Application => &[Section::General, Section::Appearance],
            #[cfg(not(target_family = "wasm"))]
            Self::Application => &[Section::General, Section::Appearance, Section::Updates],
        }
    }
}

/// One of the sidebar's pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(target_family = "wasm", allow(dead_code))]
pub enum Section {
    Canvas,
    Arranging,
    Media,
    General,
    Appearance,
    Updates,
}

impl Section {
    pub fn group(self) -> Group {
        match self {
            Self::Canvas | Self::Arranging | Self::Media => Group::Board,
            Self::General | Self::Appearance | Self::Updates => Group::Application,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Canvas => "Canvas",
            Self::Arranging => "Arranging",
            Self::Media => "Media",
            Self::General => "General",
            Self::Appearance => "Appearance",
            Self::Updates => "Updates",
        }
    }

    /// The picture on the section's row in the sidebar.
    ///
    /// So a section is found by shape rather than by reading six words every
    /// time. Never the only thing naming a row — the word is right beside it —
    /// which is what makes it safe for two of these to be marks that already
    /// mean something elsewhere in the app: a board's worth of cards *is* what
    /// Arranging is about, and a thing coming down *is* what Updates does.
    pub fn mark(self) -> Icon {
        match self {
            Self::Canvas => Icon::SectionCanvas,
            Self::Arranging => Icon::Board,
            Self::Media => Icon::SectionMedia,
            Self::General => Icon::SectionGeneral,
            Self::Appearance => Icon::SectionAppearance,
            Self::Updates => Icon::Drop,
        }
    }

    /// The sentence under the section's title, which is where the
    /// board/person split gets said in words.
    fn blurb(self) -> &'static str {
        match self {
            Self::Canvas => "The lattice behind the board, and what a drag lines up with. Saved in the board's own file, where undo can take it back.",
            Self::Arranging => "What Rearrange leaves between cards. Saved in the board's own file, where undo can take it back.",
            Self::Media => "How photographs and videos sit in their cards. Saved in the board's own file, where undo can take it back.",
            Self::General => "About this computer, not the board. Kept in your config directory and never saved into a file you send.",
            Self::Appearance => "What colours the app is made of, and who decides — you or your desktop.",
            Self::Updates => "Whether this build goes looking for a newer one, and the button that fetches it.",
        }
    }
}

// ---------------------------------------------------------------------------
// The page's own state
// ---------------------------------------------------------------------------

/// A theme being chosen off a list.
///
/// Its own small modal *inside* the page rather than a popup hanging off the
/// row, and that is a deliberate departure from the screen this page is
/// modelled on. Two reasons, and neither is taste. The rows scroll, and a
/// popup anchored to a row is a popup clipped by the container the row
/// scrolls in — which is fine until the row somebody wants is the last one.
/// And the list has no bound: a dropdown is the right control for three
/// choices and the wrong one for however many theme files a person has
/// collected, which is what makes a *searchable* list the honest shape.
///
/// The button that opens it still wears the two carets a dropdown wears,
/// because from where somebody is sitting that is what it is.
#[derive(Debug, Clone)]
pub struct Picker {
    /// Which of the two slots is being filled. Not necessarily the one the
    /// app is currently wearing — somebody pinned to dark can still be
    /// choosing what their light one will be.
    pub appearance: Appearance,
    pub query: Editor,
    /// Which of the *matches* is highlighted, not which of the themes.
    pub cursor: usize,
    /// The name that was chosen when this opened.
    ///
    /// What abandoning it puts back. Kept here rather than read off the prefs
    /// when needed, because that is exactly what it is protecting: the point
    /// of a picker that previews live is that the app is wearing something
    /// nobody has agreed to yet, and the only record of what they had is the
    /// one taken before the first preview.
    pub was: String,
}

impl Picker {
    /// Open on the theme that is already chosen, rather than at the top.
    ///
    /// A list of forty that always opened at the first row would make
    /// "where am I" the first question every time, and — because arrowing
    /// previews — would leave somebody who opened it to look at their options
    /// one keystroke away from having silently changed nothing back.
    pub(crate) fn open(appearance: Appearance, was: impl Into<String>, names: &[String]) -> Self {
        let was = was.into();
        let cursor = names.iter().position(|name| *name == was).unwrap_or(0);
        Self { appearance, query: Editor::new("", 64, false), cursor, was }
    }

    /// The theme names on offer, narrowed by what has been typed.
    ///
    /// Names rather than palettes, because the caller has the registry and
    /// this does not — and because a name is what ends up written down. With
    /// an empty query the order is the registry's, which puts the built-in
    /// first.
    pub(crate) fn matches(&self, names: &[String]) -> Vec<String> {
        let query = self.query.text().to_lowercase();
        if query.is_empty() {
            return names.to_vec();
        }
        // Folded on both sides, for the reason the page's own search gives:
        // `fuzzy::subsequence` takes two lowercase strings and every theme in
        // the list is named with a capital letter.
        let mut scored: Vec<(i32, &String)> = names
            .iter()
            .filter_map(|name| {
                crate::fuzzy::subsequence(&query, &name.to_lowercase()).map(|s| (s, name))
            })
            .collect();
        scored.sort_by_key(|a| std::cmp::Reverse(a.0));
        scored.into_iter().map(|(_, name)| name.clone()).collect()
    }

    pub(crate) fn step(&mut self, by: isize, len: usize) {
        if len == 0 {
            self.cursor = 0;
            return;
        }
        let at = self.cursor as isize + by;
        self.cursor = at.clamp(0, len as isize - 1) as usize;
    }
}

/// The open-vsx panel: a search field, a list of extensions, and what the
/// last press of it did.
///
/// A second panel rather than more rows on the page, and the same panel shape
/// the theme picker uses, because it is the same gesture — type, arrow, press
/// — aimed at a list nobody can see the whole of. The differences are the two
/// that matter: this list comes over a network, so it has a *waiting* state
/// and a *failed* state that a list of files on disk cannot have, and pressing
/// a row writes a file rather than choosing one.
///
/// **No live preview.** Arrowing the theme picker puts each palette on as it
/// goes; arrowing this one cannot, because none of these themes is on this
/// computer yet. That is why [`Reply::Preview`] has no counterpart here, and
/// why the footer says "Enter to install" rather than "Enter to keep".
#[derive(Debug, Clone)]
pub struct Getting {
    pub query: Editor,
    /// Which of the results is highlighted.
    pub cursor: usize,
    /// The query the rows below are the answer to.
    ///
    /// This is what makes one key do both jobs: while it disagrees with the
    /// field, Enter runs a search, and once it agrees, Enter installs the row
    /// under the highlight. Typing, then Enter, then arrows, then Enter — in
    /// that order, which is the order somebody does it in anyway. See
    /// [`Getting::searches`].
    pub searched: String,
    pub found: Vec<crate::openvsx::Found>,
    /// Whether something is in flight, and which thing.
    pub doing: Doing,
    /// The last thing worth saying, under the field. Empty on the first frame
    /// and after that never a lie: a search that failed says so here, and so
    /// does an install that worked.
    pub said: String,
}

/// What the panel is waiting on.
///
/// Held so the panel can say so. Both of these hold a thread of the background
/// pool for as long as they last — see `openvsx.rs` — and a panel that gave no
/// sign of it would be a panel somebody presses Enter on four more times.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Doing {
    Nothing,
    Searching,
    Installing,
}

impl Getting {
    pub(crate) fn open() -> Self {
        Self {
            query: Editor::new("", 64, false),
            cursor: 0,
            searched: String::new(),
            found: Vec::new(),
            doing: Doing::Nothing,
            said: String::new(),
        }
    }

    /// Whether Enter searches rather than installs.
    ///
    /// True while the field says something the list is not the answer to,
    /// which includes the moment the panel opens and the moment after any
    /// letter is typed.
    pub(crate) fn searches(&self) -> bool {
        let asked = self.query.text();
        let asked = asked.trim();
        !asked.is_empty() && asked != self.searched
    }

    pub(crate) fn step(&mut self, by: isize) {
        let len = self.found.len();
        if len == 0 {
            self.cursor = 0;
            return;
        }
        let at = self.cursor as isize + by;
        self.cursor = at.clamp(0, len as isize - 1) as usize;
    }
}

/// What the page is currently showing. Lives inside `Overlay::Settings`.
#[derive(Debug, Clone)]
pub struct Page {
    pub section: Section,
    /// Which groups are expanded, by [`Group::slot`].
    ///
    /// Both, until somebody folds one. A settings page that opens with its
    /// contents hidden is one where the first thing everybody does is open
    /// them again.
    pub open: [bool; 2],
    /// The sidebar's search field. Empty on nearly every frame.
    pub query: Editor,
    /// Whether the search field is wearing the keyboard.
    ///
    /// The page routes every press to `query` regardless — there is nothing
    /// else on it to type into, and being able to open Settings and start
    /// typing is the whole point of a search field over a nav. So this does
    /// not decide *where the keys go*; it decides whether the field is drawn
    /// as one somebody is in. Without it the caret sits there permanently on
    /// an empty field, which reads as a text box that has seized the page
    /// rather than one waiting to be used.
    pub focused: bool,
    /// Which of the shown rows the keyboard is on.
    ///
    /// `None` until somebody presses Tab or an arrow, so a page opened to be
    /// read is not a page with a ring already sitting on its first row. An
    /// index into the list `render` draws, which is why [`Page::shown`] exists.
    pub focus: Option<usize>,
    /// How many rows are on screen, written by `render` and read by
    /// [`Page::key`].
    ///
    /// A `Cell` because `render` takes `&Page` — the page is behind the
    /// overlay and the paint does not own it — and because this is a fact about
    /// the last frame rather than a decision. The alternative is for `key` to
    /// rebuild every row to count them, which means building forty `div`s to
    /// answer "is there a row below this one".
    pub shown: std::cell::Cell<usize>,
    pub picking: Option<Picker>,
    /// The open-vsx panel, when it is up. Never at the same time as
    /// [`Page::picking`] — both are the same rectangle over the same page, and
    /// opening either closes nothing because neither row can be reached while
    /// the other is on screen.
    pub getting: Option<Getting>,
}

impl Page {
    pub fn open() -> Self {
        Self {
            section: Section::Canvas,
            open: [true; 2],
            query: Editor::new("", 64, false),
            focused: false,
            focus: None,
            shown: std::cell::Cell::new(0),
            picking: None,
            getting: None,
        }
    }

    /// Open straight onto one section.
    pub fn onto(section: Section) -> Self {
        Self { section, ..Self::open() }
    }

    /// Whether the page is showing search results rather than a section.
    pub fn searching(&self) -> bool {
        !self.query.text().trim().is_empty()
    }

    /// Fold or unfold one group.
    pub fn fold(&mut self, group: Group) {
        let slot = group.slot();
        self.open[slot] = !self.open[slot];
    }

    /// Start choosing a theme for one of the two slots.
    pub fn pick_theme(&mut self, appearance: Appearance, was: &str, names: &[String]) {
        self.picking = Some(Picker::open(appearance, was, names));
    }

    /// Open the open-vsx panel.
    pub fn get_themes(&mut self) {
        self.getting = Some(Getting::open());
    }
}

/// What a key press on the settings page meant.
///
/// Richer than a text field's reply, because this page has two things that
/// take keys and one of them changes what the app *looks like* as the
/// highlight moves. The view resolves the names — it is the one holding the
/// registry — which is why these carry a name rather than a palette.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reply {
    /// Dealt with. Nothing for the view to do but repaint.
    Held,
    /// Put the page away.
    Close,
    /// The picker's highlight moved. Try this on, without choosing it.
    Preview(Appearance, String),
    /// The picker was accepted.
    Choose(Appearance, String),
    /// The picker was abandoned. Go back to this, which is what was chosen
    /// before it opened.
    Cancel(Appearance, String),
    ///
    /// All three carry the appearance rather than leaving the view to work it
    /// out. By the time any of them is returned the picker has already been
    /// closed — so the only place the answer still existed was inside the
    /// thing that was just thrown away, and a view left to guess would guess
    /// the appearance it is *wearing*. That is right until somebody pinned to
    /// dark edits their light theme, which is exactly what the second row on
    /// the Appearance page is for.
    /// Ctrl V, which needs the clipboard, which is the view's.
    Paste,
    /// Enter on the focused row, by index into what `render` last drew.
    ///
    /// The index rather than the action, because a `Spec` is built from the
    /// view and thrown away every frame — the page has no way to hold one. The
    /// view rebuilds the same list through `shown_rows` and reads the `Does`
    /// off the row this lands on.
    Press(usize),
    /// Left or right on the focused row: `-1` or `1`. Only a segmented control
    /// does anything with it.
    Nudge(usize, isize),
    /// The plus in the theme picker. Open the open-vsx panel over it.
    ///
    /// Over rather than in place of: the picker is still the answer to the
    /// question that was asked, and a theme installed while it is open lands
    /// in the list behind — so Escape out of the search and the thing that was
    /// just downloaded is under the highlight, which is the whole reason the
    /// plus is in the picker rather than only on the row underneath it.
    Get,
    /// Enter in the open-vsx panel, with a query the list is not the answer
    /// to yet. Go and ask the registry.
    ///
    /// The query rather than nothing, because by the time this is answered
    /// the request is on a background thread and the field may have moved on
    /// — so the thing that comes back has to be able to say what it was for.
    Look(String),
    /// Enter in the open-vsx panel, on the result at this index. Install it.
    Install(usize),
    /// The welcome screen's folder field took the key and its text may now be
    /// different. The view writes it through to the prefs — see
    /// `BoardView::commit_welcome_folder`.
    ///
    /// A reply of its own rather than `Held`, because a field that only saved
    /// when somebody happened to press Next would be a field that loses what
    /// was typed into it by the one gesture this screen promises is safe:
    /// pressing Escape.
    Folder,
}

impl Page {
    /// One key press.
    ///
    /// `names` is every theme of the appearance the picker is filling, in the
    /// order the list shows them. Passed in rather than looked up, because
    /// the registry lives on the view and this is a plain struct — the same
    /// division `palette.rs` draws with the command table.
    pub fn key(
        &mut self,
        key: &str,
        mods: Modifiers,
        text: Option<&str>,
        names: &[String],
    ) -> Reply {
        if self.getting.is_some() {
            return self.getting_key(key, mods, text);
        }
        if self.picking.is_some() {
            return self.picker_key(key, mods, text, names);
        }

        // Escape gives up the focus ring first, then the search, then the page.
        // Three things to back out of and one key, in the order they were most
        // recently taken on.
        if key == "escape" {
            if self.focus.take().is_some() {
                return Reply::Held;
            }
            if self.searching() {
                self.query = Editor::new("", 64, false);
                self.focused = false;
                return Reply::Held;
            }
            return Reply::Close;
        }

        // **Walking the rows, which this page could not do at all.** Every
        // control on it is `on_mouse_down`, and everything that was not Escape
        // went into the search field — so the grid step, the card gap, the media
        // fit, the boards folder and all four buttons were reachable with a
        // pointer and by no other means.
        //
        // Tab and the vertical arrows walk; the field keeps every printable key,
        // so typing to search still works from the moment the page opens and
        // does not have to be aimed at first.
        let rows = self.shown.get();
        match key {
            "tab" if rows > 0 => {
                let by = if mods.shift { -1 } else { 1 };
                self.focus = Some(step_focus(self.focus, by, rows));
                self.focused = false;
                return Reply::Held;
            }
            "down" | "up" if rows > 0 => {
                let by = if key == "up" { -1 } else { 1 };
                self.focus = Some(step_focus(self.focus, by, rows));
                self.focused = false;
                return Reply::Held;
            }
            // Only once a row has been aimed at. Left and right are a segmented
            // control's own keys, and Enter answers whatever the row is — the
            // view resolves which, because the rows live there.
            "left" | "right" if self.focus.is_some() => {
                let at = self.focus.unwrap_or(0);
                return Reply::Nudge(at, if key == "left" { -1 } else { 1 });
            }
            "enter" | "space" if self.focus.is_some() => {
                return Reply::Press(self.focus.unwrap_or(0));
            }
            _ => {}
        }

        let reply = self.query.key(key, editor::Mods::from(mods), text);
        // A press the field did something with is somebody using the field,
        // whether or not they ever pointed at it. A press it ignored — an
        // arrow key, a bare modifier — is not, and must not light it up.
        if reply != editor::Reply::Ignored {
            self.focused = true;
            // The list is about to be a different list, and an index into the
            // old one points at whatever happens to be there now.
            self.focus = None;
        }
        if reply == editor::Reply::Ignored && mods.secondary() && key == "v" {
            return Reply::Paste;
        }
        Reply::Held
    }

    fn picker_key(
        &mut self,
        key: &str,
        mods: Modifiers,
        text: Option<&str>,
        names: &[String],
    ) -> Reply {
        picker_key(&mut self.picking, key, mods, text, names)
    }

    /// One key press, while the open-vsx panel is open.
    ///
    /// Not shared with [`picker_key`], and the reason is the one in
    /// [`Getting`]'s note: the two panels look alike and answer the same keys,
    /// but Enter means a different thing in each and the arrows here preview
    /// nothing. Folding them together would be one function with a flag
    /// saying which of the two it was being.
    fn getting_key(&mut self, key: &str, mods: Modifiers, text: Option<&str>) -> Reply {
        let Some(getting) = self.getting.as_mut() else { return Reply::Held };

        match key {
            "escape" => {
                // The one way out, and it is the *only* way out that does not
                // install something. Nothing is undone by it: a theme already
                // written to the folder stays written.
                self.getting = None;
                return Reply::Held;
            }
            "enter" => {
                // A request is already out. A second Enter would put a second
                // one out beside it and the two would land in either order.
                if getting.doing != Doing::Nothing {
                    return Reply::Held;
                }
                if getting.searches() {
                    return Reply::Look(getting.query.text().trim().to_string());
                }
                if getting.found.is_empty() {
                    return Reply::Held;
                }
                return Reply::Install(getting.cursor);
            }
            "down" | "up" => {
                getting.step(if key == "up" { -1 } else { 1 });
                return Reply::Held;
            }
            _ => {}
        }

        let reply = getting.query.key(key, editor::Mods::from(mods), text);
        if reply != editor::Reply::Ignored {
            // The list is about to be the answer to a question nobody asked
            // any more, and an index into it points at whatever is still
            // there. See `searches`.
            getting.cursor = 0;
        }
        if reply == editor::Reply::Ignored && mods.secondary() && key == "v" {
            return Reply::Paste;
        }
        Reply::Held
    }

    /// Paste into whichever field is currently taking keys.
    pub fn insert(&mut self, text: &str) {
        match (&mut self.getting, &mut self.picking) {
            (Some(getting), _) => getting.query.insert(text),
            (None, Some(picker)) => picker.query.insert(text),
            (None, None) => self.query.insert(text),
        }
    }
}

impl Does {
    /// Do it. `by` is the direction for a segmented control and is ignored by
    /// everything else — Enter arrives as `0` and takes the next one round,
    /// so a row reached by Tab alone is still answerable without the arrows.
    pub(crate) fn run(
        self,
        view: &mut BoardView,
        by: isize,
        window: &mut gpui::Window,
        cx: &mut Context<BoardView>,
    ) {
        match self {
            Self::Flip(command) => command.run(view, window, cx),
            Self::Step { pick, count, at } => {
                if count == 0 {
                    return;
                }
                let step = if by == 0 { 1 } else { by };
                let next = (at as isize + step).rem_euclid(count as isize) as usize;
                pick(view, next, cx);
            }
            Self::Press(press) => press(view, cx),
            Self::PickTheme(appearance) => view.pick_theme(appearance, cx),
            Self::Go(go) => go(view, window, cx),
        }
    }
}

/// Where the ring goes next, wrapping at both ends.
///
/// Wrapping rather than stopping, which is the opposite of what the switcher's
/// list does — and the difference is that this is a *ring around a page* rather
/// than an aim at a list. Tab has wrapped since Tab existed, and a Tab that
/// stopped dead on the last row would read as the key having broken.
pub(crate) fn step_focus(from: Option<usize>, by: isize, rows: usize) -> usize {
    match from {
        None if by < 0 => rows - 1,
        None => 0,
        Some(at) => (at as isize + by).rem_euclid(rows as isize) as usize,
    }
}

/// One key press, while a theme picker is open.
///
/// A free function over `&mut Option<Picker>` rather than a method, because
/// the welcome screen has a picker too and it is the *same* picker — see
/// `welcome.rs`. Two copies of this would be two places for the rule that
/// arrowing previews and Escape puts back to drift apart, on the one surface
/// where drifting means the app is left wearing a theme nobody agreed to.
pub(crate) fn picker_key(
    picking: &mut Option<Picker>,
    key: &str,
    mods: Modifiers,
    text: Option<&str>,
    names: &[String],
) -> Reply {
    {
        let Some(picker) = picking.as_mut() else { return Reply::Held };
        match key {
            "escape" => {
                let (appearance, was) = (picker.appearance, picker.was.clone());
                *picking = None;
                return Reply::Cancel(appearance, was);
            }
            "enter" => {
                let chosen = picker.matches(names).get(picker.cursor).cloned();
                let (appearance, was) = (picker.appearance, picker.was.clone());
                *picking = None;
                // Enter on a list with nothing in it is not a choice. It puts
                // back what was there, which is the same thing Escape does —
                // there is no third answer to "keep the theme you cannot see".
                return match chosen {
                    Some(name) => Reply::Choose(appearance, name),
                    None => Reply::Cancel(appearance, was),
                };
            }
            "up" | "down" | "pageup" | "pagedown" => {
                let matched = picker.matches(names);
                let by = match key {
                    "up" => -1,
                    "down" => 1,
                    "pageup" => -10,
                    _ => 10,
                };
                picker.step(by, matched.len());
                return match matched.get(picker.cursor) {
                    Some(name) => Reply::Preview(picker.appearance, name.clone()),
                    None => Reply::Held,
                };
            }
            _ => {}
        }

        // Before the field, not after it: a modifier and a letter is a chord
        // the editor has no answer for, and asking it first only means the
        // answer has to be undone. `Ctrl V` below is the exception because a
        // paste *is* the field's business — it is asked first there so that a
        // field which handled it keeps it.
        if mods.secondary() && key == "n" {
            return Reply::Get;
        }

        let reply = picker.query.key(key, editor::Mods::from(mods), text);
        if reply == editor::Reply::Ignored && mods.secondary() && key == "v" {
            return Reply::Paste;
        }
        // Typing narrows the list, which moves what is under the highlight
        // even though the highlight itself did not move — so the preview
        // follows the *row*, not the keystroke.
        picker.cursor = 0;
        match picker.matches(names).first() {
            Some(name) => Reply::Preview(picker.appearance, name.clone()),
            None => Reply::Held,
        }
    }
}

/// The steps the grid can be set to.
///
/// A short ladder rather than a free field, because the number is a lattice
/// pitch and not a measurement: any value *works*, but the ones anybody
/// chooses on purpose are the halvings and doublings around the default. A
/// board whose file carries some other number simply shows no choice lit.
pub(crate) const GRID_STEPS: [f32; 5] = [32.0, 48.0, 64.0, 96.0, 128.0];

/// The gaps the arrangement engine can be told to leave between cards.
const GAPS: [f32; 7] = [0.0, 4.0, 8.0, 12.0, 16.0, 24.0, 32.0];

// ---------------------------------------------------------------------------
// The page
// ---------------------------------------------------------------------------

pub fn render(page: &Page, view: &BoardView, cx: &mut Context<BoardView>) -> impl IntoElement {
    let theme = view.theme;
    let arriving = crate::board_view::arrival(view.overlay_presence.value());

    // Every row on the page, every frame. Cheap — fifteen rows of `div` — and
    // it is what makes searching possible at all: a row that was only built
    // when its own section was showing could not be matched against while a
    // different section is.
    let all = rows(view, cx);
    let searching = page.searching();
    let shown: Vec<AnyElement> = if searching {
        // Folded on both sides, which `fuzzy::subsequence` requires and does
        // not do: its two arguments are documented as already lowercase,
        // because a caller matching several fields against one query would
        // otherwise fold the query once per field. This is such a caller —
        // two fields per row — so it folds the query once, here, and each
        // field as it goes. Handing it the words as written is why typing
        // `grid` used to find nothing at all: there is no lowercase `g`
        // anywhere in "Grid step".
        let query = page.query.text().trim().to_lowercase();
        let mut hits: Vec<(i32, Spec)> =
            all.into_iter().filter_map(|spec| score(&query, &spec).map(|s| (s, spec))).collect();
        hits.sort_by_key(|a| std::cmp::Reverse(a.0));
        hits.into_iter()
            .enumerate()
            .map(|(i, (_, spec))| spec.into_row(true, page.focus == Some(i), theme))
            .collect()
    } else {
        all.into_iter()
            .filter(|spec| spec.section == page.section)
            .enumerate()
            .map(|(i, spec)| spec.into_row(false, page.focus == Some(i), theme))
            .collect()
    };
    // What the arrows are walking, recorded where the key path can read it.
    // `render` is the only thing that knows how long the list is — the filter
    // and the search both live here — and `Page::key` is a plain struct with no
    // view to ask. See `Page::focus`.
    page.shown.set(shown.len());
    let nothing_matched = shown.is_empty();

    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        .flex()
        .justify_center()
        // A page, not a panel: it owns the whole space below the titlebar,
        // so the ground is solid and there is nothing behind to scrim.
        .bg(theme.ground.opacity(arriving.ground))
        .text_color(theme.text)
        // The wheel and both buttons end here — the board underneath still
        // exists, and a press that fell through would land on a card nobody
        // can see.
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        // A press anywhere else on the page is somebody who has finished with
        // the search field, so it stops being drawn as one they are in. The
        // field's own handler stops the press before it reaches here.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _event, _window, cx| {
                this.blur_settings_search(cx);
                cx.stop_propagation();
            }),
        )
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .w_full()
                .max_w(px(920.0))
                .h_full()
                .flex()
                // The page's contents, over a ground that is already
                // solid: they fade and rise the last few pixels into place
                // rather than dissolving with the board. See
                // `board_view::Arrival`.
                .opacity(arriving.content)
                .mt(px(arriving.rise))
                .child(sidebar(page, view, cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .pl(px(32.0))
                        .pr(px(24.0))
                        .child(header(page, view, cx))
                        .child(
                            div()
                                .id("settings-rows")
                                .flex_1()
                                .min_h_0()
                                .flex()
                                .flex_col()
                                .pb(px(32.0))
                                // A whisker of air on the right, because the
                                // scroll container clips at its edge and a
                                // switch drawn flush against it loses the
                                // curve of its own track.
                                .pr(px(4.0))
                                .overflow_y_scroll()
                                .children(shown)
                                .when(nothing_matched, |d| {
                                    d.child(
                                        div()
                                            .pt(px(22.0))
                                            .text_size(px(12.0))
                                            .text_color(theme.muted)
                                            .child("No setting says that."),
                                    )
                                }),
                        ),
                ),
        )
        // The picker's plus only where the panel it opens can work, which is
        // the same answer the More themes row gives its Browse button.
        .when_some(page.picking.as_ref(), |d, picker| {
            let can = cfg!(not(target_family = "wasm")) && crate::dirs::themes().is_some();
            d.child(picker_panel(picker, can, view, cx))
        })
        .when_some(page.getting.as_ref(), |d, getting| d.child(getting_panel(getting, view, cx)))
}

// ---------------------------------------------------------------------------
// The header
// ---------------------------------------------------------------------------

/// The strip above the rows: where this section's answers end up, and the way
/// out.
///
/// The two chips are this page's version of the ones the screen it is
/// modelled on puts there. On that screen they name the *scope* being edited —
/// the user's settings, or the project's. Here they say the same thing in
/// this app's terms, which is the Board/Application split the module note is
/// about: a Board section writes into the `.mbrd` and names it, an
/// Application section writes into `settings.json` and names that. It is the
/// one place on the page that answers "where does this end up" without
/// somebody having to read a blurb.
fn header(page: &Page, view: &BoardView, cx: &mut Context<BoardView>) -> AnyElement {
    let theme = view.theme;
    let section = page.section;
    let application = section.group() == Group::Application;
    let scope = if application { "You" } else { "Board" };
    // Through `project_name` rather than the raw title, so a board that was
    // never titled names itself by its file here the way the titlebar does,
    // instead of leaving the chip blank.
    let lands_in: SharedString = if application {
        "settings.json".into()
    } else {
        crate::titlebar::project_name(&view.doc.board.title, view.path.as_deref()).into()
    };
    let (title, blurb): (SharedString, SharedString) = if page.searching() {
        ("Search".into(), "Every setting whose name or description says that.".into())
    } else {
        (section.label().into(), section.blurb().into())
    };

    div()
        .flex()
        .items_start()
        .justify_between()
        .gap(px(16.0))
        .pt(px(26.0))
        .pb(px(14.0))
        .child(
            div()
                .flex()
                .flex_col()
                // Room, because the chip row is a different *kind* of thing
                // from the title under it: one says where this page's answers
                // are kept and the other is the page. At the gap the rest of
                // this file uses between related lines the two read as one
                // stacked heading, which is exactly what they are not.
                .gap(px(18.0))
                .min_w_0()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(7.0))
                        .child(
                            div()
                                .px(px(6.0))
                                .py(px(1.0))
                                .rounded(px(crate::theme::RADIUS_XS))
                                .bg(theme.accent.opacity(0.16))
                                .text_size(px(10.0))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(theme.accent_text)
                                .child(scope),
                        )
                        .child(div().text_size(px(11.0)).text_color(theme.muted).child(lands_in)),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(
                            div()
                                .text_size(px(16.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(title),
                        )
                        .child(div().text_size(px(11.0)).text_color(theme.muted).child(blurb)),
                ),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(6.0))
                // On every section, not only the ones whose answers land in
                // it. It was section-scoped on the reasoning that a Board
                // page's settings are inside the `.mbrd` and offering to open
                // *that* in a text editor would be offering to open a zip —
                // which is true and is not what this button does. It opens the
                // application's settings file, which is one file for the whole
                // app and is no less there for somebody currently looking at
                // the grid step. A door that comes and goes as you move around
                // the page is a door nobody remembers is there.
                // Not on the web: the button hands `settings.json` to whatever
                // opens a `.json` on the computer, and a page may not start a
                // program. The file is still there — `webfs.rs` keeps it — and
                // every setting in it is on this page.
                .when(!cfg!(target_family = "wasm"), |d| {
                    d.child(
                        div()
                            .id("settings-edit-json")
                            .px(px(9.0))
                            .py(px(4.0))
                            .rounded(px(crate::theme::RADIUS_SM))
                            .text_size(px(11.0))
                            .text_color(theme.muted)
                            .border_1()
                            .border_color(theme.chrome_edge)
                            .bg(theme.chrome)
                            .hover(|s| s.bg(theme.accent.opacity(0.10)).text_color(theme.text))
                            .active(|s| s.bg(theme.accent.opacity(0.18)))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _event, _window, cx| {
                                    this.edit_settings_file(cx);
                                }),
                            )
                            .child("Edit in settings.json"),
                    )
                })
                .child(
                    div()
                        .id("settings-close")
                        .flex_none()
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(26.0))
                        .rounded(px(crate::theme::RADIUS_SM))
                        .hover(|s| s.bg(theme.accent.opacity(0.10)))
                        .active(|s| s.bg(theme.accent.opacity(0.18)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                this.close_settings();
                                cx.notify();
                            }),
                        )
                        .child(icon(Icon::Close, crate::icons::ICON_MD, theme.muted)),
                ),
        )
        .into_any_element()
}

// ---------------------------------------------------------------------------
// The sidebar
// ---------------------------------------------------------------------------

fn sidebar(page: &Page, view: &BoardView, cx: &mut Context<BoardView>) -> AnyElement {
    let theme = view.theme;
    let searching = page.searching();
    div()
        .flex_none()
        .w(px(206.0))
        .h_full()
        .flex()
        .flex_col()
        .justify_between()
        .pt(px(22.0))
        .pb(px(16.0))
        .pr(px(20.0))
        .border_r_1()
        .border_color(theme.chrome_edge)
        .child(
            div().flex().flex_col().min_h_0().child(search_field(page, view, cx)).child(
                div()
                    .id("settings-nav")
                    .flex()
                    .flex_col()
                    .min_h_0()
                    .overflow_y_scroll()
                    // While a search is running the nav is still there
                    // and still remembers where you were, but nothing on
                    // it is lit: the list on the right is not a section
                    // any more, and lighting one would be pointing at the
                    // wrong thing.
                    .children(
                        Group::ALL.map(|group| group_block(group, page, searching, view, cx)),
                    ),
            ),
        )
        .child(
            div()
                .px(px(8.0))
                .pt(px(10.0))
                .text_size(px(10.0))
                .text_color(theme.muted)
                .child(format!("mbrd {}", crate::update::version::Version::current())),
        )
        .into_any_element()
}

/// The search field over the nav, with the glass in it.
///
/// Drawn with `palette::query_line` rather than as a plain string, which is
/// the difference between a field and a picture of one: `Ctrl A` used to
/// select all of it and nothing on screen moved. The caret and the wash are
/// gated on [`Page::focused`] rather than always on — see `query_line`.
///
/// Pressing it puts the caret at the end, which is the one placement a field
/// that cannot measure its own text can honestly offer, and is what pressing
/// past the end of a short query means anyway.
fn search_field(page: &Page, view: &BoardView, cx: &mut Context<BoardView>) -> AnyElement {
    let theme = view.theme;
    let empty = page.query.text().is_empty();
    div()
        .id("settings-search")
        .flex()
        .items_center()
        .gap(px(6.0))
        .mb(px(12.0))
        .px(px(8.0))
        .py(px(5.0))
        .rounded(px(crate::theme::RADIUS_SM))
        .bg(theme.chrome)
        .border_1()
        // Lit while it holds something. The caret says where the keys go; this
        // says, from across the page, that the list below is a filtered one.
        .border_color(if empty { theme.chrome_edge } else { theme.accent })
        .cursor_text()
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _event, _window, cx| {
                this.focus_settings_search(cx);
                // Stops the page's own handler below from letting go of the
                // field a moment after this took hold of it: presses bubble
                // outwards, so the root would otherwise have the last word.
                cx.stop_propagation();
            }),
        )
        .child(icon(Icon::Search, crate::icons::ICON_SM, theme.tertiary))
        .child(div().flex_1().min_w_0().text_size(px(12.0)).text_color(theme.text).child(
            crate::palette::query_line(&page.query, "Search settings…", 12.0, page.focused, &theme),
        ))
        // A way out with the mouse, for somebody who typed with the keyboard
        // and then reached for the pointer. Escape does the same thing.
        .when(!empty, |d| {
            d.child(
                div()
                    .id("settings-search-clear")
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(14.0))
                    .rounded(px(crate::theme::RADIUS_XS))
                    .hover(|s| s.bg(theme.accent.opacity(0.14)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            this.clear_settings_search(cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child(icon(Icon::Close, crate::icons::ICON_SM, theme.tertiary)),
            )
        })
        .into_any_element()
}

/// One group, and its sections when it is open.
fn group_block(
    group: Group,
    page: &Page,
    searching: bool,
    view: &BoardView,
    cx: &mut Context<BoardView>,
) -> AnyElement {
    let theme = view.theme;
    let open = page.open[group.slot()];
    div()
        .flex()
        .flex_col()
        .mb(px(4.0))
        .child(
            div()
                .id(SharedString::from(group.label()))
                .flex()
                .items_center()
                .gap(px(6.0))
                .px(px(8.0))
                .py(px(5.0))
                .rounded(px(crate::theme::RADIUS_SM))
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text)
                .hover(|s| s.bg(theme.chrome))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event, _window, cx| {
                        this.fold_settings_group(group, cx);
                    }),
                )
                .child(icon(
                    if open { Icon::CaretDown } else { Icon::CaretRight },
                    crate::icons::ICON_SM,
                    theme.tertiary,
                ))
                .child(group.label()),
        )
        .when(open, |d| {
            d.children(group.sections().iter().map(|&section| {
                let active = !searching && section == page.section;
                div()
                    .id(SharedString::from(section.label()))
                    // Indented to where the group's *word* starts rather than
                    // to where its chevron does, so the children hang off the
                    // name and the chevron column stays a column.
                    .ml(px(18.0))
                    .px(px(8.0))
                    .py(px(4.0))
                    .mb(px(1.0))
                    .rounded(px(crate::theme::RADIUS_SM))
                    .flex()
                    .items_center()
                    .gap(px(8.0))
                    .text_size(px(12.0))
                    .when(active, |d| {
                        d.bg(theme.accent.opacity(0.12))
                            .text_color(theme.text)
                            .font_weight(FontWeight::MEDIUM)
                    })
                    .when(!active, |d| d.text_color(theme.muted).hover(|s| s.bg(theme.chrome)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _event, _window, cx| {
                            this.show_settings_section(section, cx);
                        }),
                    )
                    // The mark lights with the row rather than staying one
                    // colour, so the whole row is one thing that is either
                    // where you are or not — an icon that held its own tint
                    // through the change would read as a second control.
                    .child(icon(
                        section.mark(),
                        crate::icons::ICON_MD,
                        if active { theme.accent_text } else { theme.tertiary },
                    ))
                    .child(section.label())
                    .into_any_element()
            }))
        })
        .into_any_element()
}

// ---------------------------------------------------------------------------
// The rows
// ---------------------------------------------------------------------------

/// One setting, before it is an element.
///
/// A struct rather than a finished `AnyElement`, because the page has to be
/// able to *match* on a row's words — see the module note on searching — and
/// a row that had already become a `div` would have thrown them away.
pub(crate) struct Spec {
    section: Section,
    title: SharedString,
    about: SharedString,
    control: AnyElement,
    /// What the keyboard does to this row, if anything.
    ///
    /// **The page had no keyboard at all.** Every control on it was
    /// `on_mouse_down` and `Page::key` sent everything but Escape into the
    /// search field, so the grid step, the card gap, the media fit, the boards
    /// folder and all four buttons could only be reached with a pointer. The
    /// toggles had a way round — they are `Command`s, so the palette reaches
    /// them — and nothing else did.
    ///
    /// `None` for a row with nothing to do: pressing one is simply nothing,
    /// the same way the menus' keyboard walks past a rule.
    pub(crate) does: Option<Does>,
}

/// What pressing a focused row does.
///
/// An enum rather than a closure because a `Spec` is rebuilt every frame and
/// this has to survive being handed back out of the key path — see
/// `BoardView::press_settings_row`, which rebuilds the list to find the row the
/// focus is on. Four kinds, which is every control this page has.
#[derive(Clone, Copy)]
pub(crate) enum Does {
    /// A switch. Enter flips it.
    Flip(Command),
    /// A segmented control. Left and right walk it; Enter takes the next one
    /// round, so a row reached by Tab alone is still answerable.
    Step { pick: fn(&mut BoardView, usize, &mut Context<BoardView>), count: usize, at: usize },
    /// A button. Enter presses it.
    Press(fn(&mut BoardView, &mut Context<BoardView>)),
    /// A theme dropdown. Enter opens the picker, which has its own keyboard
    /// already and is the model the rest of this is written to match.
    PickTheme(Appearance),
    /// A button that needs the window as well — the welcome screen's four
    /// doors, which open a board, a switcher or the tour.
    Go(fn(&mut BoardView, &mut gpui::Window, &mut Context<BoardView>)),
}

impl Spec {
    /// Say what the keyboard does to this row.
    fn does(mut self, does: Does) -> Self {
        self.does = Some(does);
        self
    }

    /// One setting: a name, the sentence under it, and its control at the
    /// edge.
    ///
    /// The ruled line belongs to the row rather than the list so every row is
    /// the same shape; the last one's rule reads as the section's own edge.
    fn into_row(self, say_where: bool, focused: bool, theme: Theme) -> AnyElement {
        let Self { section, title, about, control, does: _ } = self;
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(32.0))
            .py(px(13.0))
            .border_b_1()
            .border_color(theme.chrome_edge.opacity(0.6))
            // The ring, and it is the row rather than the control: a settings
            // row is a name, a sentence and a switch, and lighting only the
            // switch would put the focus somewhere the eye has to hunt for it
            // on a page where the switches are all in one column.
            .when(focused, |d| {
                d.bg(theme.accent.opacity(0.08))
                    .border_color(theme.accent.opacity(0.5))
                    .rounded(px(crate::theme::RADIUS_SM))
                    .px(px(8.0))
                    .mx(px(-8.0))
            })
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .min_w_0()
                    // Which section a result came from, and only in results:
                    // a flattened list where every row looks alike is one
                    // nobody can navigate back to by hand afterwards.
                    .when(say_where, |d| {
                        d.child(div().text_size(px(9.0)).text_color(theme.tertiary).child(format!(
                            "{} · {}",
                            section.group().label(),
                            section.label()
                        )))
                    })
                    .child(div().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).child(title))
                    .child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.muted)
                            .line_height(gpui::relative(1.4))
                            .child(about),
                    ),
            )
            .child(div().flex_none().child(control))
            .into_any_element()
    }
}

/// The rows the page is currently showing, in the order it shows them.
///
/// **The same filter and the same sort `render` uses**, and deliberately so:
/// the focus ring is an index into this list, and a key path that ordered the
/// rows differently from the paint would activate a row somebody was not
/// looking at. Called once per press rather than once per frame.
pub(crate) fn shown_rows(page: &Page, view: &BoardView, cx: &mut Context<BoardView>) -> Vec<Spec> {
    let all = rows(view, cx);
    if page.searching() {
        let query = page.query.text().trim().to_lowercase();
        let mut hits: Vec<(i32, Spec)> =
            all.into_iter().filter_map(|spec| score(&query, &spec).map(|s| (s, spec))).collect();
        hits.sort_by_key(|a| std::cmp::Reverse(a.0));
        hits.into_iter().map(|(_, spec)| spec).collect()
    } else {
        all.into_iter().filter(|spec| spec.section == page.section).collect()
    }
}

/// How well one row answers a query, or `None` if it does not.
///
/// Title and description both, at the better of the two scores. The words
/// people remember about a setting are as often in the sentence under it as in
/// its name — "the space Rearrange leaves" is how somebody thinks of the card
/// gap.
///
/// `query` is already lowercase and the two fields are folded here.
/// `fuzzy::subsequence` documents that it does no folding of its own, because
/// a caller matching several fields against one query would otherwise fold the
/// query once per field — this is exactly such a caller, and handing it the
/// words as written is why typing `grid` used to find nothing: there is no
/// lowercase `g` anywhere in "Grid step".
fn score(query: &str, spec: &Spec) -> Option<i32> {
    let title = crate::fuzzy::subsequence(query, &spec.title.to_lowercase());
    let about = crate::fuzzy::subsequence(query, &spec.about.to_lowercase());
    title.max(about)
}

fn spec(
    section: Section,
    title: impl Into<SharedString>,
    about: impl Into<SharedString>,
    control: AnyElement,
) -> Spec {
    Spec { section, title: title.into(), about: about.into(), control, does: None }
}

/// Every row the page has, in the order the sections are listed.
fn rows(view: &BoardView, cx: &mut Context<BoardView>) -> Vec<Spec> {
    let mut all = canvas_rows(view, cx);
    all.extend(arranging_rows(view, cx));
    all.extend(media_rows(view, cx));
    all.extend(general_rows(view, cx));
    all.extend(appearance_rows(view, cx));
    // See `Group::sections`, which leaves the section itself out of the sidebar
    // on the same platform and for the same reason.
    #[cfg(not(target_family = "wasm"))]
    all.extend(update_rows(view, cx));
    all
}

fn canvas_rows(view: &BoardView, cx: &mut Context<BoardView>) -> Vec<Spec> {
    let step = view.doc.board.settings.desktop.grid_step;
    vec![
        toggle(Section::Canvas, Command::ToggleGrid, "Draw the dot lattice behind the board.", None, view, cx),
        toggle(
            Section::Canvas,
            Command::ToggleSnap,
            "Pull cards onto the grid as they are moved and resized. Turning it on snaps the whole board; turning it off puts everything back.",
            None,
            view,
            cx,
        ),
        toggle(Section::Canvas, Command::ToggleAxes, "Show the world axes through the origin.", None, view, cx),
        toggle(Section::Canvas, Command::ToggleWeb, "Draw the ropes between connected cards.", None, view, cx),
        toggle(
            Section::Canvas,
            Command::ToggleGuides,
            "Flash a guide when a drag lines up with a neighbour's edge or centre.",
            None,
            view,
            cx,
        ),
        spec(
            Section::Canvas,
            "Grid step",
            "World units between grid lines. Snapped cards land on multiples of this.",
            segmented(
                "grid-step",
                &GRID_STEPS.map(|v| format!("{v}")),
                GRID_STEPS.iter().position(|&v| (v - step).abs() < 0.01),
                pick_step,
                view,
                cx,
            ),
        )
        .does(Does::Step {
            pick: pick_step,
            count: GRID_STEPS.len(),
            at: GRID_STEPS.iter().position(|&v| (v - step).abs() < 0.01).unwrap_or(0),
        }),
    ]
}

fn arranging_rows(view: &BoardView, cx: &mut Context<BoardView>) -> Vec<Spec> {
    let gap = view.doc.board.settings.desktop.spacing;
    vec![spec(
        Section::Arranging,
        "Card gap",
        "The space Rearrange leaves between cards.",
        segmented(
            "card-gap",
            &GAPS.map(|v| format!("{v}")),
            GAPS.iter().position(|&v| (v - gap).abs() < 0.01),
            pick_gap,
            view,
            cx,
        ),
    )
    .does(Does::Step {
        pick: pick_gap,
        count: GAPS.len(),
        at: GAPS.iter().position(|&v| (v - gap).abs() < 0.01).unwrap_or(0),
    })]
}

fn media_rows(view: &BoardView, cx: &mut Context<BoardView>) -> Vec<Spec> {
    let fit = view.doc.board.media_fit.clone();
    let fits = ["contain".to_string(), "cover".to_string()];
    let chosen = fits.iter().position(|f| *f == fit);
    vec![spec(
        Section::Media,
        "Media fit",
        "How photos and videos sit in their cards: the whole picture with margins, or the whole card with crops. A card's own menu can override it.",
        segmented("media-fit", &fits, chosen, pick_fit, view, cx),
    )
    .does(Does::Step { pick: pick_fit, count: fits.len(), at: chosen.unwrap_or(0) })]
}

fn general_rows(view: &BoardView, cx: &mut Context<BoardView>) -> Vec<Spec> {
    // A preference the environment has pinned should say so on the row,
    // rather than appearing to take and then not surviving a restart — the
    // same warning `toggle_pref` says after the fact, said before it instead.
    let motion_note = crate::prefs::Prefs::forced(crate::prefs::Setting::Motion)
        .map(|var| format!("Set by {var} at startup. Changing it here holds until you quit."));
    let theme = view.theme;

    // Where boards go, and what a new one is born with.
    //
    // These are here because the welcome screen says, in so many words, that
    // everything it asks can be changed later in Settings — and a sentence
    // like that is either true or it is the app lying to somebody on their
    // first day. So the three answers that screen collects and this page did
    // not previously own all live here now, in the section that already means
    // "about this computer, not the board".
    //
    // The two new-board rows say *new* in their descriptions rather than
    // relying on being in an Application section. See `prefs::NewBoard`: the
    // settings they seed are Board settings, and the Canvas section three rows
    // up is where the board that is actually open changes its mind. Nothing
    // here touches that board.
    let steps: Vec<String> = GRID_STEPS.iter().map(|s| format!("{}", *s as i32)).collect();
    let chosen = GRID_STEPS.iter().position(|s| (*s - view.prefs.new_board.grid_step).abs() < 0.5);
    let boards_note = match (view.prefs.boards_dir.as_deref(), crate::dirs::boards()) {
        (Some(chosen), _) => format!("New boards go in {}.", chosen.display()),
        (None, Some(fallback)) => {
            format!("New boards go in {} — this computer's usual place.", fallback.display())
        }
        (None, None) => "This computer has no home directory to keep boards in.".to_string(),
    };

    vec![
        toggle(
            Section::General,
            Command::ToggleMotion,
            "Let the interface move. Turn off to land every change instantly.",
            motion_note,
            view,
            cx,
        ),
        // The web has no folder to browse to: a page is not allowed to name a
        // place on the disk, and everything this build writes lives in the
        // store the tab is given. So the row still says where boards go and
        // does not offer to change it — a button that opened nothing would be
        // worse than no button. See `webfs.rs`.
        {
            #[cfg(target_family = "wasm")]
            {
                spec(
                    Section::General,
                    "Boards folder",
                    format!(
                        "{boards_note} In this build they are kept by the browser, on this \
                         device, and this cannot be moved."
                    ),
                    gpui::div().into_any_element(),
                )
            }

            #[cfg(not(target_family = "wasm"))]
            {
                spec(
                    Section::General,
                    "Boards folder",
                    format!("{boards_note} Boards you already have stay where they are."),
                    button("settings-boards-folder", "Browse…", true, theme, cx, |this, cx| {
                        this.browse_for_boards(cx);
                    }),
                )
                .does(Does::Press(|this, cx| this.browse_for_boards(cx)))
            }
        },
        spec(
            Section::General,
            "Snap new boards to the grid",
            "What a board this app makes starts with. It does not change the board you have open.",
            switch_at(
                "settings-new-snap",
                view.prefs.new_board.snap,
                view,
                cx,
                |this, _window, cx| this.set_new_board_snap(!this.prefs.new_board.snap, cx),
            ),
        )
        .does(Does::Press(|this, cx| {
            this.set_new_board_snap(!this.prefs.new_board.snap, cx);
        })),
        spec(
            Section::General,
            "Grid step for new boards",
            "The lattice a board this app makes starts on. It does not change the board you have \
             open.",
            segmented(
                "settings-new-step",
                &steps,
                chosen,
                |this, at, cx| this.set_new_board_step(GRID_STEPS[at], cx),
                view,
                cx,
            ),
        )
        .does(Does::Step {
            pick: |this, at, cx| this.set_new_board_step(GRID_STEPS[at], cx),
            count: GRID_STEPS.len(),
            at: chosen.unwrap_or(0),
        }),
        toggle(
            Section::General,
            Command::ToggleTrackpadPan,
            "Two fingers on a trackpad move the board, the way they move a page. Off, they zoom \
             instead. A pinch zooms either way, and so does a mouse wheel.",
            None,
            view,
            cx,
        ),
        toggle(
            Section::General,
            Command::ToggleLinkFetch,
            "A pasted address that points at a picture or a video becomes that card. Off, every \
             paste is a link — and nothing here contacts the address.",
            None,
            view,
            cx,
        ),
        // The way back to the questions. It is the only route to the
        // demonstration board and the tour standing side by side, and without
        // it the first-run screen is a thing that happened once and cannot be
        // consulted again.
        spec(
            Section::General,
            Command::Welcome.label(),
            "The questions this app asked the first time it opened, and the ways in it offered.",
            button("settings-welcome", "Run setup again", true, theme, cx, |this, cx| {
                this.open_welcome(cx);
            }),
        )
        .does(Does::Press(|this, cx| this.open_welcome(cx))),
    ]
}

#[cfg(not(target_family = "wasm"))]
fn update_rows(view: &BoardView, cx: &mut Context<BoardView>) -> Vec<Spec> {
    let update_note = crate::prefs::Prefs::forced(crate::prefs::Setting::Update)
        .map(|var| format!("Set by {var} at startup. Changing it here holds until you quit."));
    vec![
        toggle(
            Section::Updates,
            Command::ToggleUpdateChecks,
            "Check quietly at startup and say so in the top bar when one exists.",
            update_note,
            view,
            cx,
        ),
        update_row(view, cx),
    ]
}

// ---------------------------------------------------------------------------
// Appearance
// ---------------------------------------------------------------------------

fn appearance_rows(view: &BoardView, cx: &mut Context<BoardView>) -> Vec<Spec> {
    let theme = view.theme;
    let mode_pinned = crate::prefs::Prefs::forced(crate::prefs::Setting::Appearance);
    let theme_pinned = crate::prefs::Prefs::forced(crate::prefs::Setting::Theme);
    let worn = view.appearance();

    let modes = [Mode::System, Mode::Light, Mode::Dark];
    let mode_labels: Vec<String> = modes.iter().map(|m| m.label().to_string()).collect();

    let mut all = vec![spec(
        Section::Appearance,
        "Appearance",
        match mode_pinned {
            Some(var) => format!("Set by {var} at startup. Changing it here holds until you quit."),
            None => "Light, dark, or whatever your desktop is currently set to.".into(),
        },
        segmented(
            "appearance-mode",
            &mode_labels,
            modes.iter().position(|&m| m == view.prefs.mode),
            pick_mode,
            view,
            cx,
        ),
    )
    .does(Does::Step {
        pick: pick_mode,
        count: modes.len(),
        at: modes.iter().position(|&m| m == view.prefs.mode).unwrap_or(0),
    })];

    // Both rows, always, and the one not currently being worn is drawn
    // quieter rather than hidden. A row that disappears is a row somebody
    // cannot find again — and the whole point of keeping two names is that
    // the pair is chosen once and then followed, which means the half you are
    // not looking at has to be reachable while you are not looking at it.
    for appearance in [Appearance::Dark, Appearance::Light] {
        let name = view.prefs.theme_for(appearance).to_string();
        let known = view.themes.knows(&name, appearance);
        let live = worn == appearance;
        let about: SharedString = match (theme_pinned, known) {
            (Some(var), _) => {
                format!("Set by {var} at startup. Changing it here holds until you quit.").into()
            }
            // The one thing a settings page must not do is show a fallback as
            // though it were a choice. The name is still what is written
            // down — it comes back if the file does — and saying so is the
            // difference between "your theme is missing" and "your theme is
            // Ash now", which is what a row showing the fallback would imply.
            (None, false) => format!(
                "“{name}” is not among the themes this app can find. Wearing the built-in one until it turns up."
            )
            .into(),
            (None, true) => {
                let when = appearance.label().to_lowercase();
                match live {
                    true => format!("The palette worn when the app is {when}, which it is now.").into(),
                    false => format!("The palette worn when the app is {when}.").into(),
                }
            }
        };
        let id = match appearance {
            Appearance::Dark => "settings-theme-dark",
            Appearance::Light => "settings-theme-light",
        };
        all.push(
            spec(
                Section::Appearance,
                format!("{} theme", appearance.label()),
                about,
                dropdown(id, appearance, &name, known, theme, cx, |this, appearance, cx| {
                    this.pick_theme(appearance, cx)
                }),
            )
            .does(Does::PickTheme(appearance)),
        );
    }

    // Where a theme comes from, in one row. It was two — a "Themes folder"
    // row carrying the path and its two buttons, and this one under it — which
    // put the way in that needs nothing but a search field *below* the way in
    // that needs a file manager and a text editor. One row, and the verbs in
    // the order somebody reaches for them: search the registry, or open the
    // folder, drop a file in it, come back and reload.
    //
    // A browser tab has neither of the two things installing a theme needs: a
    // request to another origin it is allowed to make, and a folder to put the
    // answer in. The row is still shown there, saying so, and its Reload still
    // works — a tab keeps the themes it knows about in the store `webfs.rs`
    // holds, which is a folder in every sense except the one a file manager
    // cares about.
    let can = cfg!(not(target_family = "wasm")) && crate::dirs::themes().is_some();
    let about: SharedString = match crate::dirs::themes() {
        Some(path) => {
            let said = complained(&view.themes.complaints);
            let how = match can {
                true => format!("Search open-vsx.org, or drop a .json in {}", path.display()),
                false => {
                    format!("This build cannot install themes. Drop a .json in {}", path.display())
                }
            };
            format!("{how} and press Reload. {said}").into()
        }
        None => "There is nowhere on this computer to keep themes.".into(),
    };

    // Enter on the row presses the first of the buttons, which is the verb the
    // row is named for. Where that button is not there at all, it presses the
    // one that is.
    #[cfg(target_family = "wasm")]
    let press: fn(&mut BoardView, &mut Context<BoardView>) = |this, cx| this.reload_themes(cx);
    #[cfg(not(target_family = "wasm"))]
    let press: fn(&mut BoardView, &mut Context<BoardView>) = |this, cx| this.get_themes(cx);

    all.push(
        spec(Section::Appearance, "More themes", about, {
            let reload = button("settings-reload-themes", "Reload", true, theme, cx, |this, cx| {
                this.reload_themes(cx);
            });

            // The web has no folder to open: a page may not start a program,
            // and the themes a tab knows about live in the store `webfs.rs`
            // keeps rather than anywhere a file manager could be pointed at.
            // The same rule the Boards folder row follows — a button that
            // opened nothing would be worse than no button. Browse goes with
            // it, for the reason the note above gives.
            #[cfg(target_family = "wasm")]
            {
                reload
            }

            // The row names the path already, and a path is the one thing on
            // this page nobody can act on — it cannot be pressed, and typing
            // it out somewhere else is what the middle button saves.
            #[cfg(not(target_family = "wasm"))]
            {
                gpui::div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(button("settings-get-themes", "Browse", can, theme, cx, |this, cx| {
                        this.get_themes(cx)
                    }))
                    .child(button(
                        "settings-open-themes",
                        "Open folder",
                        crate::dirs::themes().is_some(),
                        theme,
                        cx,
                        |this, cx| this.open_themes_folder(cx),
                    ))
                    .child(reload)
                    .into_any_element()
            }
        })
        .does(Does::Press(press)),
    );

    all
}

/// What is wrong in the themes folder, in one sentence somebody can act on.
///
/// **Named, and with the reason.** This was a count once, and a count is the
/// one thing nobody can act on: "one file there could not be read" is the same
/// sentence whether the folder holds one theme or forty, and it does not say
/// which of the two silences a misspelled key fell into. See
/// `themes::Complaint`.
///
/// **Then it was one clause per file**, which is how a folder of eight themes
/// written against another app turned this row into five lines of "has no
/// themes in it; has no themes in it; has no themes in it". Eight files with
/// one thing wrong is *one* fact, so it is said once and the files are listed
/// in front of it. Nothing is dropped that changes what to do next: every
/// distinct reason keeps its own sentence, and the first files under each are
/// named, because the fix starts by opening one of them.
///
/// The cut-offs are what fits a row without becoming a wall — three names, and
/// then how many more there are. Somebody with thirty broken files does not
/// need thirty names; they need to open one and find out what this app wanted.
fn complained(complaints: &[crate::themes::Complaint]) -> String {
    if complaints.is_empty() {
        return "Everything there was read.".to_string();
    }

    // Grouped by the reason, in the order the reasons first came up — which is
    // the order the folder was read in, so the sentences follow the files.
    let mut groups: Vec<(&str, Vec<&str>)> = Vec::new();
    for complaint in complaints {
        match groups.iter_mut().find(|(why, _)| *why == complaint.why) {
            Some((_, files)) => files.push(&complaint.file),
            None => groups.push((&complaint.why, vec![&complaint.file])),
        }
    }

    let mut left = 0;
    if groups.len() > MOST_REASONS {
        left = groups[MOST_REASONS..].iter().map(|(_, files)| files.len()).sum();
        groups.truncate(MOST_REASONS);
    }

    let mut said: Vec<String> = groups
        .into_iter()
        .map(|(why, files)| match files.as_slice() {
            // The subject is singular either way — the reasons are written as
            // "has no themes in it", agreeing with one file — so a list of
            // them takes "each" rather than a verb this code would have to
            // rewrite. See `themes::Registry::read`, which writes them.
            [one] => format!("{one} {why}."),
            many => {
                let shown = many.len().min(MOST_FILES);
                let names = many[..shown].join(", ");
                match many.len() - shown {
                    0 => format!("{names}: each {why}."),
                    more => format!("{names} and {more} more: each {why}."),
                }
            }
        })
        .collect();
    if left > 0 {
        said.push(format!("{left} other files there have trouble of their own."));
    }
    said.join(" ")
}

/// How many distinct reasons the row spells out before it starts counting.
const MOST_REASONS: usize = 3;
/// How many files a single reason names before it starts counting.
const MOST_FILES: usize = 3;

/// The control that opens the theme list.
///
/// Wears the two carets a dropdown wears, because from where somebody is
/// sitting that is what it is — what it *opens* is a searchable panel rather
/// than a popup, for the reasons [`Picker`] gives.
pub(crate) fn dropdown(
    id: &'static str,
    appearance: Appearance,
    name: &str,
    known: bool,
    theme: Theme,
    cx: &mut Context<BoardView>,
    open: fn(&mut BoardView, Appearance, &mut Context<BoardView>),
) -> AnyElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .px(px(9.0))
        .py(px(4.0))
        .min_w(px(154.0))
        .rounded(px(crate::theme::RADIUS_SM))
        .text_size(px(11.0))
        .bg(theme.chrome)
        .border_1()
        .border_color(theme.chrome_edge)
        .cursor_pointer()
        // **Both at full strength, and which one is being worn is said in
        // words.** The slot not on screen used to be drawn at 65% — meaning
        // "not the half you are looking at" in the same dimming that means
        // "cannot be pressed" two rows up this page. It could be pressed, and
        // was the only control here that could while looking like it could not.
        .hover(|s| s.bg(theme.accent.opacity(0.10)))
        .active(|s| s.bg(theme.accent.opacity(0.18)))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _event, _window, cx| {
                open(this, appearance, cx);
            }),
        )
        .child(
            div()
                // A name pointing at nothing is drawn in the colour of the
                // sentence that says so, rather than as an ordinary value.
                .when(!known, |d| d.text_color(theme.muted))
                .child(name.to_string()),
        )
        .child(icon(Icon::CaretUpDown, crate::icons::ICON_SM, theme.tertiary))
        .into_any_element()
}

/// What a theme looks like, in three squares.
///
/// The ground it draws on, the card it draws on that, and the accent — which is
/// the smallest set that tells two themes apart at a glance, and the one that
/// answers "is this the light one" without reading a word. Drawn as one chip
/// with a hairline around it rather than three loose squares, so a pale theme
/// on a pale row still has an edge.
fn swatches(palette: Theme) -> AnyElement {
    div()
        .flex()
        .flex_none()
        .items_center()
        .rounded(px(crate::theme::RADIUS_XS))
        .border_1()
        .border_color(palette.chrome_edge)
        .overflow_hidden()
        .children(
            [palette.ground, palette.card, palette.accent]
                .map(|colour| div().w(px(9.0)).h(px(14.0)).bg(colour)),
        )
        .into_any_element()
}

/// The list itself: a panel over the page, searchable, previewing live.
pub(crate) fn picker_panel(
    picker: &Picker,
    can_get: bool,
    view: &BoardView,
    cx: &mut Context<BoardView>,
) -> AnyElement {
    let theme = view.theme;
    let offered = view.themes.of(picker.appearance);
    let names: Vec<String> = offered.iter().map(|t| t.name.clone()).collect();
    let matched = picker.matches(&names);
    // Who each theme is by, for the right-hand side of its row. A map because
    // the matched list is names and the details are on the registry entries,
    // and doing the lookup per row inside the loop would be a linear scan of
    // the registry for every row of it.
    let by: HashMap<&str, SharedString> = offered
        .iter()
        .map(|t| {
            let words: SharedString = match (t.family.as_str(), t.author.as_str()) {
                ("", _) => "Built in".into(),
                (family, "") => family.to_string().into(),
                (family, author) => format!("{family} · {author}").into(),
            };
            (t.name.as_str(), words)
        })
        .collect();
    // The palette behind each name, for the swatches. **A list for choosing
    // colours had no colours in it** — every row was a name and a credit, and
    // every built-in credits "mbrd", so the dark list and the light list read
    // as the same list. The theme is already resolved on the registry entry, so
    // this costs a lookup rather than a merge.
    let palettes: HashMap<&str, Theme> =
        offered.iter().map(|t| (t.name.as_str(), t.theme)).collect();

    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        .flex()
        .items_start()
        .justify_center()
        .bg(theme.ground.opacity(0.55))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _event, _window, cx| {
                // Pressing outside abandons, like Escape. A picker whose
                // answer is already on screen has to make the way out that
                // does *not* commit the easy one.
                this.cancel_theme_pick(cx);
            }),
        )
        .child(
            div()
                .mt(px(96.0))
                .w(px(430.0))
                .flex()
                .flex_col()
                .rounded(px(crate::theme::RADIUS_LG))
                .bg(theme.chrome)
                .border_1()
                .border_color(theme.chrome_edge)
                .shadow(theme.shadow_large())
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(12.0))
                        .py(px(9.0))
                        .border_b_1()
                        .border_color(theme.chrome_edge)
                        .text_size(px(13.0))
                        .text_color(theme.text)
                        .child(div().flex_1().min_w_0().child(crate::palette::query_line(
                            &picker.query,
                            &format!("Search {} themes…", picker.appearance.label().to_lowercase()),
                            13.0,
                            true,
                            &theme,
                        )))
                        .when(can_get, |d| d.child(get_button(theme, cx))),
                )
                .child(
                    div()
                        .id("theme-picker-list")
                        .flex()
                        .flex_col()
                        .p(px(6.0))
                        .max_h(px(320.0))
                        .overflow_y_scroll()
                        .children(matched.iter().enumerate().map(|(i, name)| {
                            let lit = i == picker.cursor;
                            let credit = by.get(name.as_str()).cloned().unwrap_or_default();
                            let palette = palettes.get(name.as_str()).copied();
                            let chosen = name.clone();
                            div()
                                .id(SharedString::from(format!("theme-{i}")))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(12.0))
                                .px(px(8.0))
                                .py(px(5.0))
                                .rounded(px(crate::theme::RADIUS_SM))
                                .text_size(px(12.0))
                                .cursor_pointer()
                                .when(lit, |d| {
                                    d.bg(theme.accent.opacity(0.14)).text_color(theme.text)
                                })
                                .when(!lit, |d| {
                                    d.text_color(theme.muted)
                                        .hover(|s| s.bg(theme.accent.opacity(0.07)))
                                })
                                // The pointer moves the highlight and tries the
                                // theme on, which is what arrowing already did.
                                // Without it the mouse was the one way through
                                // this list that previewed nothing at all.
                                .on_hover(cx.listener(move |this, over: &bool, _window, cx| {
                                    if *over {
                                        this.hover_theme(i, cx);
                                    }
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _event, _window, cx| {
                                        cx.stop_propagation();
                                        this.choose_theme(chosen.clone(), cx);
                                    }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.0))
                                        .min_w_0()
                                        .children(palette.map(swatches))
                                        .child(div().truncate().child(name.clone())),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_size(px(10.0))
                                        .text_color(theme.tertiary)
                                        .child(credit),
                                )
                                .into_any_element()
                        }))
                        .when(matched.is_empty(), |d| {
                            d.child(
                                div()
                                    .px(px(8.0))
                                    .py(px(8.0))
                                    .text_size(px(12.0))
                                    .text_color(theme.muted)
                                    .child("No theme is called that."),
                            )
                        }),
                )
                .child(
                    div()
                        .px(px(12.0))
                        .py(px(7.0))
                        .border_t_1()
                        .border_color(theme.chrome_edge)
                        .text_size(px(10.0))
                        .text_color(theme.tertiary)
                        .child(match can_get {
                            true => {
                                "Arrows to look · Enter to keep · Ctrl N for more · Escape to put \
                                 it back"
                            }
                            false => "Arrows to look · Enter to keep · Escape to put it back",
                        }),
                ),
        )
        .into_any_element()
}

/// The plus in the theme picker's header: the way from choosing a theme to
/// getting one.
///
/// Here because this is the surface that is *about* themes. Somebody who opens
/// the picker and finds none they want is somebody who wants another one, and
/// closing it, finding the row underneath and pressing Browse is a long way
/// round to the question they have already asked. The switcher's plus is the
/// same button for the same reason — see `new_board_button`.
///
/// Wordless, because what it stands beside is a field, and a word in front of
/// a field reads as a label for the field. The tooltip carries the name and
/// the key, which is what every wordless button in this app does.
fn get_button(theme: Theme, cx: &mut Context<BoardView>) -> impl IntoElement {
    div()
        .id("theme-picker-get")
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(22.0))
        .h(px(22.0))
        .rounded(px(crate::theme::RADIUS_XS))
        .hover(|s| s.bg(theme.accent.opacity(0.16)))
        .active(|s| s.bg(theme.accent.opacity(0.32)))
        .tooltip(crate::tip::tip(theme, "Install a theme", "Ctrl N"))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _event, _window, cx| {
                cx.stop_propagation();
                this.get_themes(cx);
            }),
        )
        .child(icon(Icon::New, crate::icons::ICON_SM, theme.muted))
}

/// The open-vsx panel: a search field, what came back, and what it is doing.
///
/// Modelled on [`picker_panel`] down to the width, and deliberately: they are
/// two lists of themes over the same page, and a person who has used one has
/// used the other. What differs is what a row says — a description and a
/// download count instead of a palette, because there is no palette to show
/// until the file is on this computer — and the line under the field, which is
/// the only place a network can be honest about itself.
pub(crate) fn getting_panel(
    getting: &Getting,
    view: &BoardView,
    cx: &mut Context<BoardView>,
) -> AnyElement {
    let theme = view.theme;
    let busy = getting.doing != Doing::Nothing;

    // One line under the field, and the order is what makes it honest: what is
    // happening now beats what happened last, and both beat the standing
    // instruction. A panel that went on saying "Installed Dracula" while a
    // second install was running would be a panel that is out of date at the
    // one moment somebody is reading it.
    let said: SharedString = match (getting.doing, getting.said.is_empty()) {
        (Doing::Searching, _) => "Asking open-vsx…".into(),
        (Doing::Installing, _) => "Downloading…".into(),
        (Doing::Nothing, false) => getting.said.clone().into(),
        (Doing::Nothing, true) => "Type what you are looking for, then press Enter.".into(),
    };

    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        .flex()
        .items_start()
        .justify_center()
        .bg(theme.ground.opacity(0.55))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _event, _window, cx| {
                // Stopped here, because there can be a theme picker under
                // this one and its own outside press abandons the choice. A
                // press that put the search away *and* threw away the theme
                // underneath would be one gesture doing two things, only one
                // of which was aimed at.
                cx.stop_propagation();
                this.close_get_themes(cx);
            }),
        )
        .child(
            div()
                .mt(px(96.0))
                .w(px(430.0))
                .flex()
                .flex_col()
                .rounded(px(crate::theme::RADIUS_LG))
                .bg(theme.chrome)
                .border_1()
                .border_color(theme.chrome_edge)
                .shadow(theme.shadow_large())
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .px(px(12.0))
                        .py(px(9.0))
                        .border_b_1()
                        .border_color(theme.chrome_edge)
                        .text_size(px(13.0))
                        .text_color(theme.text)
                        .child(crate::palette::query_line(
                            &getting.query,
                            "Search open-vsx.org…",
                            13.0,
                            true,
                            &theme,
                        )),
                )
                .child(
                    div()
                        .px(px(12.0))
                        .py(px(6.0))
                        .border_b_1()
                        .border_color(theme.chrome_edge.opacity(0.6))
                        .text_size(px(11.0))
                        .text_color(if busy { theme.accent_text } else { theme.muted })
                        .child(said),
                )
                .child(
                    div()
                        .id("openvsx-list")
                        .flex()
                        .flex_col()
                        .p(px(6.0))
                        .max_h(px(320.0))
                        .overflow_y_scroll()
                        .children(getting.found.iter().enumerate().map(|(i, found)| {
                            let lit = i == getting.cursor;
                            div()
                                .id(SharedString::from(format!("openvsx-{i}")))
                                .flex()
                                .items_center()
                                .justify_between()
                                .gap(px(12.0))
                                .px(px(8.0))
                                .py(px(5.0))
                                .rounded(px(crate::theme::RADIUS_SM))
                                .text_size(px(12.0))
                                .cursor_pointer()
                                .when(lit, |d| {
                                    d.bg(theme.accent.opacity(0.14)).text_color(theme.text)
                                })
                                .when(!lit, |d| {
                                    d.text_color(theme.muted)
                                        .hover(|s| s.bg(theme.accent.opacity(0.07)))
                                })
                                .on_hover(cx.listener(move |this, over: &bool, _window, cx| {
                                    if *over {
                                        this.hover_openvsx(i, cx);
                                    }
                                }))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _event, _window, cx| {
                                        cx.stop_propagation();
                                        this.install_openvsx(i, cx);
                                    }),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(1.0))
                                        .min_w_0()
                                        .child(div().truncate().child(found.display.clone()))
                                        // The publisher and the blurb on one
                                        // quieter line. Two extensions with
                                        // the same name is the ordinary case
                                        // on a registry anybody may publish
                                        // to, and the publisher is the only
                                        // thing that tells them apart.
                                        .child(
                                            div()
                                                .truncate()
                                                .text_size(px(10.0))
                                                .text_color(theme.tertiary)
                                                .child(match found.description.is_empty() {
                                                    true => found.namespace.clone(),
                                                    false => format!(
                                                        "{} · {}",
                                                        found.namespace, found.description
                                                    ),
                                                }),
                                        ),
                                )
                                .child(
                                    div()
                                        .flex_none()
                                        .text_size(px(10.0))
                                        .text_color(theme.tertiary)
                                        .child(installs(found.downloads)),
                                )
                                .into_any_element()
                        }))
                        .when(getting.found.is_empty(), |d| {
                            d.child(
                                div()
                                    .px(px(8.0))
                                    .py(px(8.0))
                                    .text_size(px(12.0))
                                    .text_color(theme.muted)
                                    .child(match getting.searched.is_empty() {
                                        true => "Themes written for VS Code work here.",
                                        false => "Nothing on open-vsx answers to that.",
                                    }),
                            )
                        }),
                )
                .child(
                    div()
                        .px(px(12.0))
                        .py(px(7.0))
                        .border_t_1()
                        .border_color(theme.chrome_edge)
                        .text_size(px(10.0))
                        .text_color(theme.tertiary)
                        .child(match getting.searches() {
                            true => "Enter to search · Escape to close",
                            false => "Arrows to look · Enter to install · Escape to close",
                        }),
                ),
        )
        .into_any_element()
}

/// A download count, short enough for the end of a row.
///
/// Rounded hard, because the exact number is not the question being asked of
/// it. "4.1M" and "2.9k" answer "is this the one everybody uses" in the width
/// a row has; "4,134,872" answers a question nobody asked and pushes the
/// theme's own name out of the row.
fn installs(count: u64) -> SharedString {
    match count {
        0 => "new".into(),
        n if n < 1_000 => format!("{n}").into(),
        n if n < 1_000_000 => format!("{:.0}k", n as f64 / 1_000.0).into(),
        n => format!("{:.1}M", n as f64 / 1_000_000.0).into(),
    }
}

// ---------------------------------------------------------------------------
// Controls
// ---------------------------------------------------------------------------

/// One switch, run through the same `Command` the menus and the palette run.
fn toggle(
    section: Section,
    command: Command,
    about: &'static str,
    pinned: Option<String>,
    view: &BoardView,
    cx: &mut Context<BoardView>,
) -> Spec {
    let on = command.ticked(view) == Some(true);
    // The environment note replaces the description rather than joining it:
    // "what this does" matters less than "why flipping it will not hold".
    let about: SharedString = match pinned {
        Some(words) => words.into(),
        None => about.into(),
    };
    spec(section, command.label(), about, switch(command, on, view, cx)).does(Does::Flip(command))
}

/// The switch's footprint, and how far its knob crosses it. Named because
/// three numbers below have to agree: the width, the knob, and the travel
/// between the two pads is what is left over.
const SWITCH_W: f32 = 32.0;
const SWITCH_PAD: f32 = 2.0;
const SWITCH_KNOB: f32 = 14.0;
const SWITCH_TRAVEL: f32 = SWITCH_W - 2.0 * SWITCH_PAD - SWITCH_KNOB;

/// The state a toggle is in, drawn as the thing it is, and pressable itself.
///
/// A switch rather than the menus' tick, because a menu row is an
/// instruction with a receipt on it and a settings row is the state itself:
/// this page is somewhere you *read* the configuration, and a column of
/// switches reads at a glance in a way a ragged column of ticks does not.
///
/// The knob is drawn at the spring's value — see `BoardView::control_at` —
/// so a flip *crosses* the track, the accent fades up with the crossing
/// rather than switching at the end, and a second press mid-flight bends
/// the knob back out of its own motion instead of teleporting it.
fn switch(command: Command, on: bool, view: &BoardView, cx: &mut Context<BoardView>) -> AnyElement {
    switch_at(command.label(), on, view, cx, move |this, window, cx| {
        command.run(this, window, cx);
        // Aimed at where the state *now* is, read back off the same table the
        // tick reads, so a run that did nothing moves nothing.
        command.ticked(this) == Some(true)
    })
}

/// The same switch, for a state that is not a [`Command`].
///
/// Everything on the settings page is a command and can use [`switch`]. The
/// welcome screen's "Defaults for new boards" rows are not — they set what a
/// board is *born* with, which is a preference rather than something the
/// menus or the palette could ever run. Rather than let that grow a second
/// switch that looks the same and animates differently, the drawing lives
/// here once and the two callers differ only in what a press does.
///
/// `press` performs the change and answers where the state ended up, which is
/// what the knob is then aimed at. Answering rather than assuming is what
/// makes a press that was refused move nothing.
pub(crate) fn switch_at(
    id: &'static str,
    on: bool,
    view: &BoardView,
    cx: &mut Context<BoardView>,
    press: impl Fn(&mut BoardView, &mut gpui::Window, &mut Context<BoardView>) -> bool + 'static,
) -> AnyElement {
    let theme = view.theme;
    let p = view.control_at(id, if on { 1.0 } else { 0.0 }).clamp(0.0, 1.0);
    div()
        .id(SharedString::from(id))
        .flex_none()
        .relative()
        .w(px(SWITCH_W))
        .h(px(18.0))
        .rounded_full()
        .bg(theme.muted.opacity(0.35))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _event, window, cx| {
                let now = press(this, window, cx);
                this.move_control(id, if now { 0.0 } else { 1.0 }, if now { 1.0 } else { 0.0 });
                cx.notify();
            }),
        )
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .rounded_full()
                .bg(theme.accent.opacity(p)),
        )
        .child(
            div()
                .absolute()
                .top(px(SWITCH_PAD))
                .left(px(SWITCH_PAD + p * SWITCH_TRAVEL))
                .size(px(SWITCH_KNOB))
                .rounded_full()
                .bg(theme.ground),
        )
        .into_any_element()
}

fn pick_step(view: &mut BoardView, at: usize, cx: &mut Context<BoardView>) {
    view.set_grid_step(GRID_STEPS[at], cx);
}

fn pick_gap(view: &mut BoardView, at: usize, cx: &mut Context<BoardView>) {
    view.set_spacing(GAPS[at], cx);
}

fn pick_fit(view: &mut BoardView, at: usize, cx: &mut Context<BoardView>) {
    view.set_media_fit(if at == 0 { "contain" } else { "cover" }, cx);
}

fn pick_mode(view: &mut BoardView, at: usize, cx: &mut Context<BoardView>) {
    view.set_mode([Mode::System, Mode::Light, Mode::Dark][at], cx);
}

/// A choice made from a short row of segments, drawn as one control rather
/// than as loose chips: the container is what says the options are one
/// setting.
///
/// A plain `fn` pointer for `pick` rather than a closure, because every
/// segment captures it and a listener must be `'static` — a pointer is
/// `Copy` and carries nothing, which is exactly the amount of state picking
/// from a fixed list needs.
pub(crate) fn segmented(
    name: &'static str,
    labels: &[String],
    chosen: Option<usize>,
    pick: fn(&mut BoardView, usize, &mut Context<BoardView>),
    view: &BoardView,
    cx: &mut Context<BoardView>,
) -> AnyElement {
    let theme = view.theme;
    // Where the wash is, in segment units. Far off the row entirely when the
    // file carries a value none of the segments name, so nothing is lit.
    let slot = view.control_at(name, chosen.map_or(-10.0, |i| i as f32));
    div()
        .flex()
        .items_center()
        .p(px(2.0))
        .gap(px(1.0))
        .rounded(px(crate::theme::RADIUS_SM))
        .bg(theme.chrome)
        .border_1()
        .border_color(theme.chrome_edge)
        .children(labels.iter().enumerate().map(|(i, words)| {
            let active = chosen == Some(i);
            // How lit this segment is: full at the wash's centre, nothing a
            // whole segment away. While the spring crosses, the wash sweeps
            // through the segments in between — the in-between frames point
            // at where the choice is going.
            let lit = (1.0 - (slot - i as f32).abs()).clamp(0.0, 1.0);
            div()
                .id(SharedString::from(format!("{name}-{i}")))
                .px(px(9.0))
                .py(px(3.0))
                .rounded(px(crate::theme::RADIUS_XS))
                .text_size(px(11.0))
                .bg(theme.accent.opacity(0.16 * lit))
                .when(active, |d| d.text_color(theme.accent_text).font_weight(FontWeight::MEDIUM))
                .when(!active, |d| {
                    d.text_color(theme.muted)
                        .hover(|s| s.text_color(theme.text).bg(theme.accent.opacity(0.06)))
                })
                .on_mouse_down(MouseButton::Left, {
                    // Planted at the choice that was lit when this frame was
                    // drawn, aimed at the one pressed — the first press is
                    // what starts the wash crossing; after that the spring
                    // keeps its own place. A row with nothing lit parks at
                    // the target instead: there is nowhere to sweep from.
                    let from = chosen.map_or(i as f32, |c| c as f32);
                    cx.listener(move |this, _event, _window, cx| {
                        pick(this, i, cx);
                        this.move_control(name, from, i as f32);
                        cx.notify();
                    })
                })
                .child(words.clone())
        }))
        .into_any_element()
}

/// A control that is a verb rather than a state.
pub(crate) fn button(
    id: &'static str,
    word: impl Into<SharedString>,
    live: bool,
    theme: Theme,
    cx: &mut Context<BoardView>,
    press: fn(&mut BoardView, &mut Context<BoardView>),
) -> AnyElement {
    div()
        .id(id)
        .px(px(10.0))
        .py(px(4.0))
        .rounded(px(crate::theme::RADIUS_SM))
        .text_size(px(11.0))
        .border_1()
        .when(live, |d| {
            d.border_color(theme.chrome_edge)
                .bg(theme.chrome)
                .cursor_pointer()
                .hover(|s| s.bg(theme.accent.opacity(0.10)))
                .active(|s| s.bg(theme.accent.opacity(0.18)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event, _window, cx| {
                        press(this, cx);
                        cx.notify();
                    }),
                )
        })
        // A button that cannot be pressed has to stop looking like one. It used
        // to keep the border and the fill of a live button and only grey its
        // word, which reads as a button that works — so the two were told apart
        // by pressing, and pressing did nothing. The chrome goes with the
        // handler; *why* it went is in the row's own sentence, which is what
        // that sentence is for.
        .when(!live, |d| d.border_color(theme.chrome_edge.opacity(0.5)).text_color(theme.tertiary))
        .child(word.into())
        .into_any_element()
}

/// The one row that is a verb rather than a state, so its control is a
/// button — and the button's word follows how far the last press got, the
/// same stepper the titlebar badge walks.
#[cfg(not(target_family = "wasm"))]
fn update_row(view: &BoardView, cx: &mut Context<BoardView>) -> Spec {
    let theme = view.theme;
    // **Not `Command::available`, which answers a different question.** That one
    // is "can this build update at all", which is right for the menu row and for
    // `Ctrl U` — both of which should still say so out loud when the answer is
    // no. A button is a press, and a press is only worth offering when it would
    // achieve something, which also takes the switch in the row above this one.
    let possible = Command::CheckForUpdates.available(view);
    let live = possible && view.prefs.update;
    let word = match view.update_badge() {
        // Nothing waiting, or a build with no updater in it at all. Either way
        // the only thing a press can do is ask.
        None | Some(UpdateBadge::Resting { .. }) => "Check now",
        Some(UpdateBadge::Available { .. }) => "Download",
        Some(UpdateBadge::Downloading { .. }) => "Downloading…",
        Some(UpdateBadge::Ready { .. }) => "Restart to update",
        Some(UpdateBadge::Installing { .. }) => "Installing…",
    };
    let about: SharedString = match (possible, view.prefs.update) {
        (true, true) => {
            format!("You have mbrd {}.", crate::update::version::Version::current()).into()
        }
        // The reason a dead button is dead, in the row's own sentence rather
        // than in a status line this page is covering.
        (true, false) => "Looking for new versions is switched off in the row above, so there is \
                          nothing for this to ask."
            .into(),
        (false, _) => {
            "This build was not installed from a release, so it has nothing to update.".into()
        }
    };
    spec(
        Section::Updates,
        "Check for updates",
        about,
        button("settings-update", word, live, theme, cx, |this, cx| {
            this.update_step(cx);
        }),
    )
    .does(Does::Press(|this, cx| this.update_step(cx)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complaint(file: &str, why: &str) -> crate::themes::Complaint {
        crate::themes::Complaint { file: file.to_string(), why: why.to_string() }
    }

    #[test]
    fn one_thing_wrong_with_eight_files_is_said_once() {
        // The row this is for: a folder of themes written for another app,
        // every one of them the same kind of not-a-theme. Said eight times it
        // filled five lines and read as eight different problems.
        let same: Vec<_> = ["a.json", "b.json", "c.json", "d.json", "e.json"]
            .iter()
            .map(|file| complaint(file, "has no themes in it"))
            .collect();
        assert_eq!(
            complained(&same),
            "a.json, b.json, c.json and 2 more: each has no themes in it."
        );
    }

    #[test]
    fn one_file_is_still_named_with_its_own_reason() {
        // The whole point of the list. A count cannot be acted on; a file name
        // can be opened.
        assert_eq!(
            complained(&[complaint("mine.json", "is not readable JSON in this format")]),
            "mine.json is not readable JSON in this format."
        );
        assert_eq!(complained(&[]), "Everything there was read.");
    }

    #[test]
    fn different_reasons_keep_their_own_sentences() {
        let mixed = [
            complaint("a.json", "has no themes in it"),
            complaint("b.json", "is not readable JSON in this format"),
            complaint("c.json", "has no themes in it"),
        ];
        assert_eq!(
            complained(&mixed),
            "a.json, c.json: each has no themes in it. b.json is not readable JSON in this \
             format."
        );
    }

    #[test]
    fn a_folder_of_many_troubles_stops_naming_them_and_counts() {
        // Four reasons is one more than the row spells out, and the files
        // under the ones it drops are counted rather than forgotten: the
        // sentence must never say less is wrong than is wrong.
        let many: Vec<_> = ["a.json", "b.json", "c.json", "d.json", "e.json"]
            .iter()
            .enumerate()
            .map(|(at, file)| complaint(file, &format!("is wrong in way {at}")))
            .collect();
        let said = complained(&many);
        assert!(said.ends_with("2 other files there have trouble of their own."), "{said}");
        assert!(said.starts_with("a.json is wrong in way 0."), "{said}");
    }

    #[test]
    fn every_section_belongs_to_the_group_that_lists_it() {
        // The two directions have to agree, and nothing else checks them: the
        // sidebar walks groups down to sections, and the header walks a
        // section back up to its group to decide which file it says the
        // answers land in. A section listed under Board that thought it was
        // an Application one would offer "Edit in settings.json" for a
        // setting that goes into the `.mbrd`.
        for group in Group::ALL {
            for section in group.sections() {
                assert_eq!(section.group(), group, "{:?}", section.label());
            }
        }
    }

    #[test]
    fn every_section_is_reachable_from_the_sidebar() {
        // A section with no way to it is a page of settings nobody can open.
        // Counted rather than listed, because the listing is the thing under
        // test.
        let listed: usize = Group::ALL.iter().map(|g| g.sections().len()).sum();
        let all = [
            Section::Canvas,
            Section::Arranging,
            Section::Media,
            Section::General,
            Section::Appearance,
            Section::Updates,
        ];
        assert_eq!(listed, all.len());
        for section in all {
            assert!(
                section.group().sections().contains(&section),
                "{:?} is not on its own group's list",
                section.label()
            );
        }
    }

    fn row(title: &str, about: &str) -> Spec {
        spec(Section::Canvas, title.to_string(), about.to_string(), div().into_any_element())
    }

    #[test]
    fn a_page_nobody_has_typed_into_is_not_drawn_as_one_being_typed_into() {
        // The field takes the keys from the moment the page opens, and that
        // is deliberate — but a caret parked in an empty search box on every
        // frame reads as a page its own search field has taken over. It
        // appears when somebody uses the field and goes away when they leave
        // it, which is the only thing `focused` decides.
        let mut page = Page::open();
        assert!(!page.focused);

        page.key("g", Modifiers::default(), Some("g"), &[]);
        assert!(page.focused, "typing is using the field, pointed at or not");
        assert_eq!(page.query.text(), "g");

        page.key("escape", Modifiers::default(), None, &[]);
        assert!(!page.focused, "escape clears the search and lets go of it");
        assert!(!page.searching());
    }

    #[test]
    fn tab_walks_the_rows_and_escape_gives_them_up_before_the_page() {
        // The page had no keyboard at all: every control on it is
        // `on_mouse_down`, and everything but Escape went into the search
        // field. The toggles had a way round — they are `Command`s, so the
        // palette reaches them — and the grid step, the card gap, the media
        // fit, the boards folder and all four buttons did not.
        let mut page = Page::open();
        page.shown.set(5);
        assert_eq!(page.focus, None, "a page opened to be read has no ring on it");

        page.key("tab", Modifiers::default(), None, &[]);
        assert_eq!(page.focus, Some(0));
        page.key("down", Modifiers::default(), None, &[]);
        assert_eq!(page.focus, Some(1));
        page.key("up", Modifiers::default(), None, &[]);
        assert_eq!(page.focus, Some(0));

        // Backwards off the first row is the last one, not a stop.
        let back = Modifiers { shift: true, ..Default::default() };
        page.key("tab", back, None, &[]);
        assert_eq!(page.focus, Some(4));

        // Enter answers the row rather than the page.
        assert_eq!(page.key("enter", Modifiers::default(), None, &[]), Reply::Press(4));
        assert_eq!(page.key("right", Modifiers::default(), None, &[]), Reply::Nudge(4, 1));
        assert_eq!(page.key("left", Modifiers::default(), None, &[]), Reply::Nudge(4, -1));

        // Three things to back out of and one key, most recent first.
        assert_eq!(page.key("escape", Modifiers::default(), None, &[]), Reply::Held);
        assert_eq!(page.focus, None, "the ring goes before the page does");
        assert_eq!(page.key("escape", Modifiers::default(), None, &[]), Reply::Close);
    }

    #[test]
    fn typing_puts_the_ring_down_because_the_list_is_about_to_change() {
        // The ring is an index into what was drawn, and a search rewrites that
        // list — so an index kept across a keystroke points at whatever landed
        // in that slot instead.
        let mut page = Page::open();
        page.shown.set(5);
        page.key("tab", Modifiers::default(), None, &[]);
        assert_eq!(page.focus, Some(0));

        page.key("g", Modifiers::default(), Some("g"), &[]);
        assert_eq!(page.focus, None);
        assert_eq!(page.query.text(), "g");
    }

    #[test]
    fn a_page_with_no_rows_on_it_does_not_take_the_arrows() {
        // A search that matched nothing. Without the guard the ring would be
        // an index into an empty list, and `step_focus` would divide by it.
        let mut page = Page::open();
        page.shown.set(0);
        page.key("tab", Modifiers::default(), None, &[]);
        assert_eq!(page.focus, None);
    }

    #[test]
    fn the_ring_wraps_from_nothing_in_both_directions() {
        assert_eq!(step_focus(None, 1, 4), 0, "forwards from nowhere is the first");
        assert_eq!(step_focus(None, -1, 4), 3, "backwards from nowhere is the last");
        assert_eq!(step_focus(Some(3), 1, 4), 0);
        assert_eq!(step_focus(Some(0), -1, 4), 3);
    }

    #[test]
    fn a_key_the_field_does_nothing_with_does_not_light_it_up() {
        // The line is what the *editor* did, not what the key looks like: an
        // arrow walks the caret and counts as using the field, an `F5` it has
        // never heard of does not. Drawn from the editor's own answer rather
        // than from a list of keys here, which is the list that would drift.
        let mut page = Page::open();
        page.key("f5", Modifiers::default(), None, &[]);
        assert!(!page.focused);
        assert_eq!(page.query.text(), "");

        page.key("left", Modifiers::default(), None, &[]);
        assert!(page.focused, "the caret moved, so there is a caret to show");
    }

    #[test]
    fn searching_finds_a_setting_typed_the_way_anybody_types_it() {
        // Lowercase, because that is how people type into a search field and
        // every setting on the page is named with a capital letter. This is
        // the whole of the bug that made the field appear not to work at all:
        // `fuzzy::subsequence` takes two *already folded* strings, and there
        // is no lowercase `g` anywhere in "Grid step".
        let grid = row("Grid step", "World units between grid lines.");
        assert!(score("grid", &grid).is_some());
        assert!(score("gridstep", &grid).is_some(), "a subsequence, not a substring");
        assert!(score("zzz", &grid).is_none());
    }

    #[test]
    fn searching_reads_the_sentence_under_a_setting_as_well_as_its_name() {
        // The words somebody remembers are as often in the description as in
        // the title — "the space Rearrange leaves" is how people think of the
        // card gap, and nothing in that phrase is in its name.
        let gap = row("Card gap", "The space Rearrange leaves between cards.");
        assert!(score("rearrange", &gap).is_some());
        assert!(score("card", &gap).is_some(), "and the title still counts");
    }

    #[test]
    fn a_theme_is_found_by_typing_its_name_in_lower_case() {
        // The picker had the same folding bug as the page's own search, and
        // for the same reason: every theme in the list is named with a capital
        // letter.
        let names: Vec<String> = ["Ash", "Ink", "Sepia"].map(String::from).to_vec();
        let mut page = Page::open();
        page.pick_theme(Appearance::Dark, "Ash", &names);
        let picker = page.picking.as_mut().unwrap();
        picker.query.insert("sep");
        assert_eq!(picker.matches(&names), vec!["Sepia".to_string()]);
    }

    #[test]
    fn escape_clears_a_search_before_it_closes_the_page() {
        // Two meanings on one key, and they never collide: an empty field has
        // nothing to clear. Somebody who typed something and wants the whole
        // page back should not have to reopen it.
        let mut page = Page::open();
        assert_eq!(page.key("escape", Modifiers::default(), None, &[]), Reply::Close);
        page.query.insert("grid");
        assert!(page.searching());
        assert_eq!(page.key("escape", Modifiers::default(), None, &[]), Reply::Held);
        assert!(!page.searching(), "the first Escape emptied the field");
        assert_eq!(page.key("escape", Modifiers::default(), None, &[]), Reply::Close);
    }

    /// Type into the open-vsx panel, one letter at a time.
    fn typed(page: &mut Page, text: &str) {
        for letter in text.chars() {
            let key = letter.to_string();
            page.key(&key, Modifiers::default(), Some(&key), &[]);
        }
    }

    fn a_result(display: &str) -> crate::openvsx::Found {
        crate::openvsx::Found {
            namespace: "somebody".into(),
            name: display.to_lowercase(),
            display: display.into(),
            description: String::new(),
            downloads: 0,
            vsix: String::new(),
        }
    }

    #[test]
    fn enter_searches_until_the_list_is_the_answer_and_installs_after_that() {
        // The whole of the panel's keyboard in one test, because it is one
        // decision: one key does both jobs, and which one it does is settled
        // by whether the rows below still answer what the field says.
        let mut page = Page::open();
        page.get_themes();
        // Nothing typed: Enter is not a search for nothing.
        assert_eq!(page.key("enter", Modifiers::default(), None, &[]), Reply::Held);

        typed(&mut page, "nord");
        assert_eq!(page.key("enter", Modifiers::default(), None, &[]), Reply::Look("nord".into()));

        // The answer comes back, which is what `settle_search` writes.
        let getting = page.getting.as_mut().unwrap();
        getting.searched = "nord".into();
        getting.found = vec![a_result("Nord"), a_result("Nordic")];
        getting.doing = Doing::Nothing;

        assert_eq!(page.key("enter", Modifiers::default(), None, &[]), Reply::Install(0));
        assert_eq!(page.key("down", Modifiers::default(), None, &[]), Reply::Held);
        assert_eq!(page.key("enter", Modifiers::default(), None, &[]), Reply::Install(1));

        // And one more letter makes the list stale again, so Enter goes back
        // to being a search. Without this, typing past a finished search would
        // install whatever the old list happened to have under the highlight.
        typed(&mut page, "i");
        assert_eq!(page.key("enter", Modifiers::default(), None, &[]), Reply::Look("nordi".into()));
    }

    #[test]
    fn a_second_enter_while_something_is_downloading_does_nothing() {
        // Both of these hold a thread of the background pool, and the panel
        // has one line to say what happened in. See `install_openvsx`.
        let mut page = Page::open();
        page.get_themes();
        typed(&mut page, "nord");
        assert_eq!(page.key("enter", Modifiers::default(), None, &[]), Reply::Look("nord".into()));
        page.getting.as_mut().unwrap().doing = Doing::Searching;
        assert_eq!(page.key("enter", Modifiers::default(), None, &[]), Reply::Held);
    }

    #[test]
    fn the_highlight_stops_at_both_ends_of_what_came_back() {
        // Stopping rather than wrapping, which is the opposite of the page's
        // own Tab ring: this is an aim at a list, and a list that jumped from
        // the last row to the first would install the wrong extension.
        let mut page = Page::open();
        page.get_themes();
        let getting = page.getting.as_mut().unwrap();
        getting.found = vec![a_result("One"), a_result("Two")];
        page.key("up", Modifiers::default(), None, &[]);
        assert_eq!(page.getting.as_ref().unwrap().cursor, 0);
        page.key("down", Modifiers::default(), None, &[]);
        page.key("down", Modifiers::default(), None, &[]);
        assert_eq!(page.getting.as_ref().unwrap().cursor, 1);
    }

    #[test]
    fn escape_closes_the_open_vsx_panel_and_leaves_the_page_open() {
        // Two things to back out of and one key, in the order they were most
        // recently taken on — the same rule the rest of this page follows.
        let mut page = Page::open();
        page.get_themes();
        assert_eq!(page.key("escape", Modifiers::default(), None, &[]), Reply::Held);
        assert!(page.getting.is_none());
        assert_eq!(page.key("escape", Modifiers::default(), None, &[]), Reply::Close);
    }

    #[test]
    fn a_paste_lands_in_whichever_field_is_taking_keys() {
        let mut page = Page::open();
        page.insert("moss");
        assert_eq!(page.query.text(), "moss");
        page.get_themes();
        page.insert("nord");
        assert_eq!(page.getting.as_ref().unwrap().query.text(), "nord");
        assert_eq!(page.query.text(), "moss", "the page's own search is untouched");
    }

    #[test]
    fn a_download_count_is_rounded_to_something_a_row_can_hold() {
        assert_eq!(installs(0), "new");
        assert_eq!(installs(842), "842");
        assert_eq!(installs(4_082), "4k");
        assert_eq!(installs(413_434), "413k");
        assert_eq!(installs(4_134_872), "4.1M");
    }

    #[test]
    fn a_picker_opens_on_the_theme_that_is_already_chosen() {
        // Not at the top. Because arrowing previews, a list that always
        // opened at the first row would leave somebody who opened it merely
        // to look one keystroke away from having changed something.
        let names: Vec<String> = ["Ash", "Ink", "Iron"].map(String::from).to_vec();
        let mut page = Page::open();
        page.pick_theme(Appearance::Dark, "Iron", &names);
        assert_eq!(page.picking.as_ref().map(|p| p.cursor), Some(2));
        // And a name that is no longer in the list starts at the top rather
        // than off the end of it.
        page.pick_theme(Appearance::Dark, "Gone", &names);
        assert_eq!(page.picking.as_ref().map(|p| p.cursor), Some(0));
    }

    #[test]
    fn arrowing_through_the_picker_previews_without_choosing() {
        let names: Vec<String> = ["Ash", "Ink", "Iron"].map(String::from).to_vec();
        let mut page = Page::open();
        page.pick_theme(Appearance::Dark, "Ash", &names);
        assert_eq!(
            page.key("down", Modifiers::default(), None, &names),
            Reply::Preview(Appearance::Dark, "Ink".into())
        );
        // Nothing has been chosen yet — the picker is still open, which is
        // the whole difference between a preview and a choice.
        assert!(page.picking.is_some());
        assert_eq!(
            page.key("enter", Modifiers::default(), None, &names),
            Reply::Choose(Appearance::Dark, "Ink".into())
        );
        assert!(page.picking.is_none());
    }

    #[test]
    fn abandoning_the_picker_names_what_to_go_back_to() {
        // The reason `Picker::was` exists. By the time Escape arrives the app
        // is already wearing something nobody agreed to, and the only record
        // of the real choice is the one taken before the first preview.
        let names: Vec<String> = ["Ash", "Ink"].map(String::from).to_vec();
        let mut page = Page::open();
        page.pick_theme(Appearance::Dark, "Ash", &names);
        page.key("down", Modifiers::default(), None, &names);
        assert_eq!(
            page.key("escape", Modifiers::default(), None, &names),
            Reply::Cancel(Appearance::Dark, "Ash".into())
        );
        assert!(page.picking.is_none());
    }

    #[test]
    fn typing_in_the_picker_previews_whatever_ends_up_under_the_highlight() {
        // The highlight did not move; the list moved under it. Previewing on
        // the keystroke rather than on the row would leave the app wearing
        // the theme that *used* to be first.
        let names: Vec<String> = ["Ash", "Ink", "Iron"].map(String::from).to_vec();
        let mut page = Page::open();
        page.pick_theme(Appearance::Dark, "Ash", &names);
        assert_eq!(
            page.key("r", Modifiers::default(), Some("r"), &names),
            Reply::Preview(Appearance::Dark, "Iron".into())
        );
    }

    #[test]
    fn a_picker_with_nothing_in_it_cannot_be_accepted() {
        // Enter on an empty list is not a choice. It puts back what was
        // there, which is what Escape does — there is no third answer to
        // "keep the theme you cannot see".
        let names: Vec<String> = ["Ash"].map(String::from).to_vec();
        let mut page = Page::open();
        page.pick_theme(Appearance::Dark, "Ash", &names);
        page.key("z", Modifiers::default(), Some("z"), &names);
        assert_eq!(
            page.key("enter", Modifiers::default(), None, &names),
            Reply::Cancel(Appearance::Dark, "Ash".into())
        );
    }

    #[test]
    fn the_plus_in_the_picker_opens_the_search_over_it_rather_than_instead_of_it() {
        // The picker stays. A theme installed from the panel over it lands in
        // the list behind, and a picker that closed to make room for the
        // search would have thrown away the question that was being asked.
        let names: Vec<String> = ["Ash", "Ink"].map(String::from).to_vec();
        let mut page = Page::open();
        page.pick_theme(Appearance::Dark, "Ash", &names);
        assert_eq!(page.key("n", Modifiers::secondary_key(), Some("n"), &names), Reply::Get);
        assert!(page.picking.is_some(), "the picker is still the answer being given");
        // And the letter on its own is a letter, which is the whole reason
        // the plus needs a modifier at all: "Ink" has one in it.
        assert_eq!(
            page.key("n", Modifiers::default(), Some("n"), &names),
            Reply::Preview(Appearance::Dark, "Ink".into())
        );
    }

    #[test]
    fn folding_a_group_leaves_the_other_one_alone() {
        let mut page = Page::open();
        assert_eq!(page.open, [true, true], "a page opens with its contents showing");
        page.fold(Group::Board);
        assert_eq!(page.open, [false, true]);
        page.fold(Group::Board);
        assert_eq!(page.open, [true, true]);
    }
}
