//! The list that comes up under a right-click.
//!
//! Drawn here rather than handed to the platform, for the reason the rest of
//! the window is: this app draws its own titlebar because the compositor could
//! not be relied on to draw one, and a menu is the same bargain with the same
//! answer. It also means the menu is styled by [`Theme`](crate::theme::Theme)
//! like everything else, rather than being the one part of the window that
//! looks like it belongs to a different program.
//!
//! It holds no state of its own beyond where it is and what it offers — what it
//! *does* is [`Command`], which the keyboard reads from the same table. See
//! `command.rs` for why that matters.
//!
//! ## Staying inside the window
//!
//! Drawing its own menu costs it the one thing a platform menu gets free: a
//! menu of the platform's is its own window and can hang over the edge of the
//! app, and this one cannot. So both lists are *fitted* rather than allowed to
//! spill, by one rule applied to each axis in turn — see [`fit`]: where it was
//! asked for if the whole of it goes there, on the other side of whatever it
//! hangs off if it goes there instead, and otherwise on the side with more
//! room, cut down to what that side holds.
//!
//! Cut down is the half that matters. `fit` answers with a length as well as a
//! place, so a list too tall for the window becomes a list that scrolls and one
//! too wide becomes a narrower list — rather than either becoming a list with
//! entries drawn where they cannot be reached. Which is also why the lists grew
//! submenus: the shortest list is the one least likely to need any of this.
//!
//! Fitted once, when it opens, against the window as it stands then. A window
//! that changes size under an open menu closes it instead — see
//! [`BoardView::resized`](crate::board_view::BoardView) — because a menu that
//! jumped out from under the pointer on a re-tile would be worse than one that
//! went away. What the fit cannot see is where the *rows* are once the list is
//! scrolled, so a submenu is given the scroll of the list that opened it.

use gpui::{div, prelude::*, px, Context, MouseButton, Pixels, Point, ScrollHandle, Size};

use crate::board_view::BoardView;
use crate::command::Entry;
use crate::icons::{icon, Icon};

/// How wide the list is where the window has room for it. Fixed rather than
/// fitted to the entries, so they line up down the right-hand edge instead of
/// the box changing width with whichever board is open.
const WIDTH: f32 = 216.0;
const ROW_HEIGHT: f32 = 26.0;
const RULE_HEIGHT: f32 = 9.0;
/// The air either side of a rule.
///
/// Counted rather than left to the layout, because [`Menu::height`] works out
/// what the list will measure *before* anything is drawn and everything the
/// list is fitted to rests on that answer. A margin the arithmetic here does
/// not know about is a list fitted to a height it does not have — which shows
/// up as one cut short in a window with room to spare.
const RULE_MARGIN: f32 = 4.0;
const PADDING: f32 = 6.0;
/// The line around the list, which counts towards its size: gpui measures the
/// border box, so a height worked out here without it would be two pixels shy
/// of what gets drawn.
const BORDER: f32 = 1.0;
/// How far a fitted list stays clear of the window's edge.
///
/// The first thing given up when there is not enough of anything: a window
/// narrow enough that the margin is a quarter of it gets a quarter instead, and
/// one narrower still gets a list that touches both edges — which is a worse
/// list than it was, but still one you can read.
const MARGIN: f32 = 8.0;
/// How far a submenu tucks back under the list that opened it.
///
/// A gap between the two would be a gap the pointer crosses on its way, and
/// crossing it means leaving both — which closes the thing being aimed at.
const OVERLAP: f32 = 4.0;
/// The mark on a row that opens onto more, and the one on a row that is on.
///
/// Sized rather than left to the text, because they are pictures now: a glyph
/// takes the row's font size and an icon takes the size it is given, and both
/// of these want to sit a shade under the label beside them so the label stays
/// the thing being read. [`crate::icons::ICON_SM`] — the same size every other
/// inline, gutter-sized mark in the app draws at.
const MARK: f32 = crate::icons::ICON_SM;
/// The room the tick sits in, whether or not the row has one.
///
/// Fixed, so the labels line up down a menu where only some entries are
/// settings — a gutter that collapsed on the untickable rows would step every
/// label in the list left and right depending on its neighbours.
const GUTTER: f32 = 12.0;

/// A list, fitted to the window: where its corner goes, and how much of it
/// there is room to draw.
///
/// The height is an allowance rather than a measurement. A list shorter than
/// its allowance keeps its own height; one taller than it scrolls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub at: Point<Pixels>,
    pub size: Size<Pixels>,
}

/// The band of one axis a list may occupy: the window, less a margin.
fn band(room: Pixels) -> (Pixels, Pixels) {
    let margin = px(MARGIN).min(room * 0.25).max(Pixels::ZERO);
    (margin, (room - margin).max(margin))
}

/// Fit a run of `want` into one axis of the window.
///
/// At `at` if the whole of it goes there; at `alt` — the far side of whatever
/// it hangs off — if the whole of it goes there instead; and if it goes neither
/// way, against the side of `at` with more room, cut to what that side holds.
///
/// The cut is why this answers with a length as well as a place. Sliding an
/// over-long list back inside the window instead — which is what this used to
/// do — puts it over the thing it was opened from and leaves it over-long
/// anyway: the entries past the edge have only moved, not become reachable.
fn fit(at: Pixels, alt: Pixels, want: Pixels, room: Pixels) -> (Pixels, Pixels) {
    let (low, high) = band(room);
    let goes = |start: Pixels| start >= low && start + want <= high;
    if goes(at) {
        (at, want)
    } else if goes(alt) {
        (alt, want)
    } else {
        let at = at.clamp(low, high);
        let (after, before) = (high - at, at - low);
        if after >= before {
            (at, want.min(after))
        } else {
            (low, want.min(before))
        }
    }
}

/// Where an open menu is, and what it offers.
#[derive(Debug, Clone)]
pub struct Menu {
    /// Where it hangs and how much of it there is room for, worked out once
    /// when it opened. See the module note on why once is enough.
    pub list: Placed,
    /// What it offers. Held rather than looked up at paint time, so that a
    /// menu opened over a rope stays the rope's menu even if the command it
    /// runs changes what is selected underneath it.
    pub entries: &'static [Entry],
    /// The submenu that is open, where one is.
    pub open: Option<Open>,
    /// How far the list is scrolled, for the window too short to hold it.
    ///
    /// Held here rather than left to the element's own state because a submenu
    /// has to know how far the row it hangs off has moved — see
    /// [`Menu::beside`].
    pub scroll: ScrollHandle,
    /// The row the pointer **or** the keyboard is on, in `entries` — into the
    /// top list, even while a submenu is open, since that is what lets the
    /// row that opened it stay lit while the keyboard has moved on into the
    /// submenu's own [`Open::cursor`].
    ///
    /// One field rather than two. A menu used to have no keyboard cursor at
    /// all — Escape was the only key it answered, and every other key was
    /// swallowed so it would not reach the board underneath — and the
    /// obvious fix is a second piece of state next to the mouse's hover.
    /// That is the wrong shape: a row lit because the pointer is on it and a
    /// row lit because Down was pressed are the same fact, "this is what
    /// Enter is about to do", and tracking them separately is how a menu ends
    /// up with the mouse over one row and the keyboard highlight on another.
    /// So arriving by either door writes the same field — see
    /// `BoardView::reveal_menu` and [`Menu::step`].
    pub cursor: Option<usize>,
}

/// A submenu, and the row of its parent it belongs to.
#[derive(Debug, Clone)]
pub struct Open {
    /// Which row opened it, so that row can stay lit while the pointer is
    /// inside the list it opened.
    pub row: usize,
    pub list: Placed,
    pub entries: &'static [Entry],
    /// The row of *this* list the pointer or the keyboard is on. See
    /// [`Menu::cursor`], which is the same idea one level up.
    pub cursor: Option<usize>,
}

impl Menu {
    /// Open a list at the pointer, fitted to the window it is opening in.
    pub fn new(at: Point<Pixels>, entries: &'static [Entry], room: Size<Pixels>) -> Self {
        Self {
            list: Self::placed(at, room, entries),
            entries,
            open: None,
            scroll: ScrollHandle::new(),
            cursor: None,
        }
    }

    /// How much of the list's height one entry takes up.
    ///
    /// The one place that says so. Both the height of the whole list and the
    /// distance down to any one row are counted from here, so the two cannot
    /// drift apart — and neither can drift from what gets drawn without this
    /// line and [`list`] disagreeing in plain sight.
    fn extent(entry: &Entry) -> f32 {
        match entry {
            Entry::Rule => RULE_HEIGHT + RULE_MARGIN * 2.0,
            _ => ROW_HEIGHT,
        }
    }

    /// How tall the list wants to be, before the window has its say.
    fn height(entries: &[Entry]) -> f32 {
        entries.iter().map(Self::extent).sum::<f32>() + (PADDING + BORDER) * 2.0
    }

    /// How far down the list a row starts.
    fn top_of(entries: &[Entry], row: usize) -> f32 {
        entries[..row.min(entries.len())].iter().map(Self::extent).sum::<f32>() + PADDING + BORDER
    }

    /// Fit the list to the window, hanging from where the pointer was.
    ///
    /// Down and to the right of the pointer where there is room for it, which
    /// is what every other menu does and what stops the first entry landing
    /// under the cursor where a stray click would take it; the other way where
    /// there is not, and cut to fit where there is room neither way.
    pub fn placed(at: Point<Pixels>, room: Size<Pixels>, entries: &[Entry]) -> Placed {
        let (want_w, want_h) = (px(WIDTH), px(Self::height(entries)));
        let (x, w) = fit(at.x, at.x - want_w, want_w, room.width);
        let (y, h) = fit(at.y, at.y - want_h, want_h, room.height);
        Placed { at: gpui::point(x, y), size: gpui::size(w, h) }
    }

    /// Where a submenu opened off `row` should hang.
    ///
    /// Beside the row rather than under the pointer, and on the far side of the
    /// list unless that would leave the window — in which case it comes out the
    /// other way, which is the same flip the parent made. A window too narrow
    /// to hold two lists side by side gets one on top of the other; there is
    /// nowhere else for the second one to go.
    fn beside(&self, row: usize, list: &[Entry], room: Size<Pixels>, scroll: Pixels) -> Placed {
        let (want_w, want_h) = (px(WIDTH), px(Self::height(list)));

        // Level with the row as it is *drawn*. A list too tall for the window
        // scrolls, so a submenu placed against the row's position in the list
        // rather than its position on the screen would open level with whatever
        // had scrolled into its place.
        let top = self.list.at.y + px(Self::top_of(self.entries, row)) + scroll;
        // Slid up rather than flipped where it runs past the bottom: a submenu
        // that flipped would clear its row entirely, and the row is the only
        // thing saying where the second list came from.
        let (y, h) = fit(top, band(room.height).1 - want_h, want_h, room.height);

        let right = self.list.at.x + self.list.size.width - px(OVERLAP);
        let left = self.list.at.x - want_w + px(OVERLAP);
        let (x, w) = fit(right, left, want_w, room.width);

        Placed { at: gpui::point(x, y), size: gpui::size(w, h) }
    }

    /// The pointer has arrived on `row`. Answers whether anything moved.
    ///
    /// Opening is on arrival and closing is on arriving somewhere *else*,
    /// rather than on leaving: a row that closed its submenu on the way out
    /// would close it as the pointer crossed into it.
    ///
    /// `opens` is asked of the caller rather than worked out here, because
    /// availability is a question about the *board* and this holds none of it.
    pub fn reveal(&mut self, row: usize, room: Size<Pixels>, scroll: Pixels, opens: bool) -> bool {
        match self.entries.get(row).copied() {
            Some(Entry::More(_, list)) if opens => {
                if self.open.as_ref().is_some_and(|open| open.row == row) {
                    return false;
                }
                let placed = self.beside(row, list, room, scroll);
                self.open = Some(Open { row, list: placed, entries: list, cursor: None });
                true
            }
            _ => self.open.take().is_some(),
        }
    }

    /// The pointer has arrived on `row` of the open *submenu*. Only the
    /// keyboard highlight moves — nothing here opens a third list off a
    /// second one, so arrival has nothing else to decide. Answers whether
    /// anything changed, the same shape [`Menu::reveal`] answers in.
    pub fn hover_sub(&mut self, row: usize) -> bool {
        match &mut self.open {
            Some(open) if open.cursor != Some(row) => {
                open.cursor = Some(row);
                true
            }
            _ => false,
        }
    }

    /// Move the keyboard highlight by `by` rows, skipping rules and clamping
    /// at either end rather than wrapping — the same rule the palette and the
    /// switcher follow. Moves the submenu's cursor once one is open, and the
    /// top list's otherwise: whichever list the keyboard is actually on.
    pub fn step(&mut self, by: i32) {
        let (entries, cursor) = match &self.open {
            Some(open) => (open.entries, open.cursor),
            None => (self.entries, self.cursor),
        };
        let Some(last) = entries.len().checked_sub(1) else { return };
        let last = last as i32;
        // Off either end when nothing is highlighted yet, so the first press
        // lands on the first row going down or the last row going up rather
        // than on whichever one happened to be second.
        let mut i = cursor.map_or(if by > 0 { -1 } else { last + 1 }, |c| c as i32);
        loop {
            i += by;
            if i < 0 || i > last {
                i = i.clamp(0, last);
                break;
            }
            if !matches!(entries[i as usize], Entry::Rule) {
                break;
            }
        }
        if matches!(entries[i as usize], Entry::Rule) {
            return;
        }
        match &mut self.open {
            Some(open) => open.cursor = Some(i as usize),
            None => self.cursor = Some(i as usize),
        }
    }

    /// What Enter is about to do: the entry under the keyboard, in whichever
    /// list has it.
    pub fn chosen(&self) -> Option<Entry> {
        let (entries, cursor) = match &self.open {
            Some(open) => (open.entries, open.cursor),
            None => (self.entries, self.cursor),
        };
        cursor.and_then(|i| entries.get(i)).copied()
    }

    /// Open the submenu under the keyboard, where the row it is on has one —
    /// the keyboard's version of arriving on the row with the pointer. Lands
    /// the submenu's own cursor on its first selectable row, so a further
    /// `Down` continues straight into the list rather than starting from
    /// nothing.
    pub fn open_under_cursor(&mut self, room: Size<Pixels>) -> bool {
        if self.open.is_some() {
            return false;
        }
        let Some(row) = self.cursor else { return false };
        let Some(Entry::More(_, list)) = self.entries.get(row).copied() else { return false };
        let scroll = self.scroll.offset().y;
        let placed = self.beside(row, list, room, scroll);
        self.open = Some(Open { row, list: placed, entries: list, cursor: None });
        self.step(1);
        true
    }

    /// Close the open submenu, the way `Left` backs out of one — keyboard
    /// focus returns to the top list, on the row that opened it, since
    /// `cursor` there was never moved while the submenu had the keyboard.
    /// Answers whether there was one to close.
    pub fn close_sub(&mut self) -> bool {
        self.open.take().is_some()
    }
}

pub fn render(menu: &Menu, view: &BoardView, cx: &mut Context<BoardView>) -> impl IntoElement {
    // The row that opened the submenu is what stays lit on the top list
    // while the submenu has the keyboard — and that is exactly `menu.cursor`,
    // since opening a submenu (by hover or by keyboard) always sets it to the
    // row that owns the submenu and nothing else moves it while the submenu
    // is open. One field, one meaning: see [`Menu::cursor`].
    let lit = menu.cursor;
    let presence = view.overlay_presence;

    // Both lists in one wrapper, so the submenu is a sibling of the list that
    // opened it and paints over it rather than inside it. The wrapper fills the
    // canvas and listens to nothing, so a press that misses both goes through
    // to the board underneath — which is what puts the menu away.
    div()
        .absolute()
        .size_full()
        .child(list(
            "menu",
            menu.list,
            menu.entries,
            lit,
            presence,
            false,
            Some(&menu.scroll),
            view,
            cx,
        ))
        .children(menu.open.as_ref().map(|open| {
            list("submenu", open.list, open.entries, open.cursor, presence, true, None, view, cx)
        }))
}

/// One list, where it was fitted, in as much of the window as it was given.
#[allow(clippy::too_many_arguments)]
fn list(
    name: &'static str,
    placed: Placed,
    entries: &'static [Entry],
    lit: Option<usize>,
    presence: f32,
    sub: bool,
    scroll: Option<&ScrollHandle>,
    view: &BoardView,
    cx: &mut Context<BoardView>,
) -> impl IntoElement {
    let theme = view.theme;

    let rows: Vec<_> = entries
        .iter()
        .enumerate()
        .map(|(i, entry)| match entry {
            Entry::Rule => div()
                .h(px(RULE_HEIGHT))
                .my(px(RULE_MARGIN))
                .mx(px(8.0))
                .border_t_1()
                .border_color(theme.chrome_edge)
                .into_any_element(),
            entry => row(name, i, *entry, lit == Some(i), sub, view, cx).into_any_element(),
        })
        .collect();

    // The corner it opens from, offset by a few pixels and faded in with it —
    // GPUI cannot scale a div, so offset-plus-fade is the whole vocabulary
    // for "arriving", the same trade the palette and the switcher make by
    // sliding down instead. `placed.at` already **is** that corner: it is
    // whichever edge of the box ended up nearest the point that was pressed
    // (see `Menu::placed`), so easing in from a few pixels short of it reads
    // as the list growing out of the corner it was asked for.
    let arrive = px(4.0 * (1.0 - presence));

    div()
        .id(name)
        .absolute()
        .left(placed.at.x - arrive)
        .top(placed.at.y - arrive)
        .w(placed.size.width)
        // The last resort, for a window with no side long enough to hold the
        // list. It costs nothing where the allowance is more than the list
        // wants, and what it replaces is entries drawn where they cannot be
        // reached.
        .max_h(placed.size.height)
        .opacity(presence)
        .overflow_y_scroll()
        // Only the list a submenu can hang off needs its scroll read back.
        .when_some(scroll, |d, handle| d.track_scroll(handle))
        .py(px(PADDING))
        .rounded(px(crate::theme::RADIUS_MD))
        .bg(theme.chrome)
        .border_1()
        .border_color(theme.chrome_edge)
        .shadow(crate::theme::shadow_medium())
        .text_size(px(12.0))
        .text_color(theme.text)
        // The canvas beneath listens on mouse-down, so without this a press
        // anywhere on the menu would also start a gesture on the board behind
        // it — including the press that chooses an entry.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        // And the wheel, for the same reason: the canvas zooms on it, so a
        // list scrolled without this would scroll and zoom the board behind it
        // at once. The list's own scrolling is unharmed — gpui registers that
        // after this listener and runs it first, so by the time the wheel
        // reaches here the list has already moved and all that is left to do
        // is keep it from reaching the board.
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .children(rows)
}

fn row(
    name: &'static str,
    key: usize,
    entry: Entry,
    lit: bool,
    sub: bool,
    view: &BoardView,
    cx: &mut Context<BoardView>,
) -> impl IntoElement {
    let theme = view.theme;
    let available = entry.available(view);
    let ticked = matches!(entry, Entry::Does(command) if command.ticked(view) == Some(true));
    let more = matches!(entry, Entry::More(..));

    // Named after the step it would take back rather than labelled "Undo", so
    // the entry says what is about to happen instead of only whether anything
    // can — and named the same way the palette names it, through the one
    // place that labelling lives now. See `Command::label_in`.
    let label = match entry {
        Entry::Does(command) => command.label_in(view),
        Entry::More(name, _) => name.to_string(),
        Entry::Rule => String::new(),
    };

    div()
        .id((name, key))
        .flex()
        .items_center()
        .justify_between()
        .h(px(ROW_HEIGHT))
        .mx(px(4.0))
        .px(px(8.0))
        .rounded(px(crate::theme::RADIUS_XS))
        .when(!available, |d| d.text_color(theme.muted))
        // A row whose submenu is open stays lit while the pointer is off in
        // that submenu, which is what says where the second list came from —
        // and the same highlight the keyboard cursor draws, since the two are
        // one field. See [`Menu::cursor`].
        .when(lit, |d| d.bg(theme.accent.opacity(0.16)))
        // Arriving anywhere on a list settles the highlight, and on the top
        // list it also settles which submenu is open — including on a row
        // that has none, which closes the one that is. A row of the submenu
        // only ever moves its own highlight: nothing here opens a third list
        // off a second one.
        .on_hover(cx.listener(move |this, over: &bool, _window, cx| {
            if *over {
                match sub {
                    true => this.hover_submenu(key, cx),
                    false => this.reveal_menu(key, cx),
                }
            }
        }))
        .when(available, |d| {
            // Lit on the way past, and lit harder the moment it is pressed.
            // The command runs on mouse-*down* here, so without the second one
            // the only acknowledgement of a press is the menu disappearing —
            // and a row that never looked pressed leaves you unsure which one
            // you hit.
            d.hover(|s| s.bg(theme.accent.opacity(0.16)))
                .active(|s| s.bg(theme.accent.opacity(0.32)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event, window, cx| match entry {
                        // A press on a row that opens onto more opens it, for the
                        // pointer that arrived by some route the hover missed.
                        Entry::More(..) => this.reveal_menu(key, cx),
                        Entry::Does(command) => {
                            // Closed first. A command that opens something else —
                            // the board switcher — would otherwise come up
                            // underneath a menu that is still there.
                            this.close_menu();
                            command.run(this, window, cx);
                        }
                        Entry::Rule => {}
                    }),
                )
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                // Takes what the hint beside it does not, and gives the label
                // room to be cut short rather than wrapped: a row that grew to
                // two lines would make every height worked out above it wrong,
                // and every flip decision that rests on one.
                .flex_1()
                .overflow_hidden()
                // A fixed-width gutter, so the labels line up whether or not
                // the entry above them happens to be ticked.
                .child(
                    div()
                        .flex_none()
                        .w(px(GUTTER))
                        .flex()
                        .items_center()
                        .when(ticked, |d| d.child(icon(Icon::Check, MARK, theme.accent))),
                )
                .child(div().truncate().child(label)),
        )
        .child(
            div()
                .flex()
                .flex_none()
                .items_center()
                .gap(px(6.0))
                .pl(px(6.0))
                .text_size(px(10.0))
                .text_color(theme.muted)
                .child(entry.hint(view))
                .when(more, |d| d.child(icon(Icon::CaretRight, MARK, theme.muted))),
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{Command, BOARD_MENU, CARD_MENU, MANY_MENU, ROPE_MENU};

    fn room(width: f32, height: f32) -> Size<Pixels> {
        gpui::size(px(width), px(height))
    }

    fn at(x: f32, y: f32) -> Point<Pixels> {
        gpui::point(px(x), px(y))
    }

    fn inside(placed: Placed, room: Size<Pixels>) -> bool {
        placed.at.x >= px(0.0)
            && placed.at.y >= px(0.0)
            && placed.at.x + placed.size.width <= room.width
            && placed.at.y + placed.size.height <= room.height
    }

    /// The whole point of the file, asked of every list, in every window worth
    /// worrying about, from every corner of it, with every submenu open and the
    /// parent scrolled to each end.
    ///
    /// The named tests below say *why* each rule is the way it is; this one
    /// says that between them they leave no gap. The 640×386 room is the real
    /// floor — `MIN_SIZE` in `main.rs` less the titlebar — and the two smaller
    /// ones are the tiling window manager that ignored the floor.
    #[test]
    fn every_list_stays_inside_every_window() {
        let rooms = [
            room(1000.0, 700.0),
            room(640.0, 386.0),
            room(400.0, 700.0),
            room(300.0, 200.0),
            room(120.0, 90.0),
        ];
        let menus: [&'static [Entry]; 4] = [&BOARD_MENU, &CARD_MENU, &ROPE_MENU, &MANY_MENU];

        for room in rooms {
            let (w, h) = (f32::from(room.width), f32::from(room.height));
            for entries in menus {
                for corner in [at(0.0, 0.0), at(w / 2.0, h / 2.0), at(w - 1.0, h - 1.0)] {
                    let mut menu = Menu::new(corner, entries, room);
                    assert!(inside(menu.list, room), "{:?} at {corner:?} in {room:?}", menu.list);

                    let scrolled = px(Menu::height(entries)) - menu.list.size.height;
                    for scroll in [px(0.0), -scrolled.max(px(0.0))] {
                        for (row, entry) in entries.iter().enumerate() {
                            if !matches!(entry, Entry::More(..)) {
                                continue;
                            }
                            menu.open = None;
                            menu.reveal(row, room, scroll, true);
                            let open = menu.open.as_ref().expect("that row opens onto more");
                            assert!(
                                inside(open.list, room),
                                "submenu {:?} off row {row} at {corner:?} in {room:?}",
                                open.list
                            );
                        }
                    }
                }
            }
        }
    }

    /// What a list measures, in numbers rather than in the constants it is
    /// made of — those would only be the same arithmetic written twice.
    ///
    /// What this pins is the *contributors*. Everything the list is fitted to
    /// rests on [`Menu::height`] being what the list actually draws, and it is
    /// worked out rather than measured, so anything given a margin or a border
    /// without being counted in [`Menu::extent`] silently makes the answer too
    /// small — which is a list cut short in a window with room to spare, and
    /// submenus that open above their own rows. It happened once already: the
    /// rules' eight pixels of air went uncounted, and the card list, which has
    /// five of them, drew forty pixels taller than it thought it was.
    #[test]
    fn a_list_is_as_tall_as_its_rows_its_rules_and_its_own_edges() {
        let entries = [Entry::Does(Command::Undo), Entry::Rule, Entry::Does(Command::Redo)];
        // Two rows of 26, a rule of 9 with 4 either side, and the list's own
        // 6 of padding and 1 of border, top and bottom.
        assert_eq!(Menu::height(&entries), 52.0 + 17.0 + 14.0);
        // And the second row starts under both of the first two, plus the
        // padding and border above them.
        assert_eq!(Menu::top_of(&entries, 2), 26.0 + 17.0 + 7.0);
    }

    #[test]
    fn a_menu_with_room_hangs_from_where_you_pressed() {
        let room = room(1000.0, 700.0);
        assert_eq!(Menu::placed(at(100.0, 80.0), room, &CARD_MENU).at, at(100.0, 80.0));
    }

    #[test]
    fn a_menu_near_an_edge_grows_the_other_way() {
        let room = room(1000.0, 700.0);
        let placed = Menu::placed(at(960.0, 690.0), room, &CARD_MENU);
        assert!(placed.at.x < px(960.0), "it should have flipped left");
        assert!(placed.at.y < px(690.0), "it should have flipped up");
        // And flipping is *all* it did: there was room for the whole list.
        assert_eq!(placed.size, gpui::size(px(WIDTH), px(Menu::height(&CARD_MENU))));
        assert!(inside(placed, room));
    }

    #[test]
    fn a_menu_wider_than_the_window_is_cut_down_to_it() {
        // Neither side holds a 216-wide list, so it takes the wider side of the
        // press and is narrowed to it. Sliding it back inside instead would
        // leave the same list hanging off the same edge, only further along.
        let room = room(180.0, 700.0);
        let placed = Menu::placed(at(100.0, 10.0), room, &ROPE_MENU);
        assert!(placed.size.width < px(WIDTH), "it should have been narrowed");
        assert!(inside(placed, room));
    }

    #[test]
    fn a_menu_taller_than_the_window_takes_the_roomier_side_and_scrolls() {
        // Flipping would put the corner above the top of the window and
        // clamping would put the list over the pointer that opened it. It goes
        // above instead, because that is where more of the window is, and it is
        // cut to what fits there — which is what makes it scroll. See `list`.
        let room = room(400.0, 120.0);
        let placed = Menu::placed(at(10.0, 100.0), room, &ROPE_MENU);
        assert!(placed.size.height < px(Menu::height(&ROPE_MENU)), "it should have been cut");
        assert!(placed.at.y + placed.size.height <= px(100.0), "and left the pointer clear");
        assert!(inside(placed, room));
    }

    #[test]
    fn a_submenu_hangs_off_its_own_row_and_beside_the_list() {
        let room = room(1000.0, 700.0);
        let menu = Menu::new(at(100.0, 80.0), &ROPE_MENU, room);
        let Entry::More(_, list) = ROPE_MENU[4] else { panic!("row four opens onto more") };
        let placed = menu.beside(4, list, room, px(0.0));
        assert_eq!(placed.at.x, px(100.0 + WIDTH - OVERLAP), "beside, tucked under the edge");
        assert!(placed.at.y > menu.list.at.y, "and level with the row rather than the corner");
    }

    #[test]
    fn a_submenu_with_no_room_to_its_right_comes_out_the_left() {
        let room = room(800.0, 700.0);
        let menu = Menu::new(at(700.0, 80.0), &ROPE_MENU, room);
        let Entry::More(_, list) = ROPE_MENU[4] else { panic!("row four opens onto more") };
        let placed = menu.beside(4, list, room, px(0.0));
        assert!(placed.at.x < menu.list.at.x, "it should have come out the other side");
        assert!(inside(placed, room));
    }

    #[test]
    fn a_submenu_off_a_scrolled_list_is_level_with_the_row_as_drawn() {
        // The card list is taller than the smallest window the app allows, so
        // this is not a corner case: it is what the "Add" row does the moment
        // somebody scrolls down to reach the rows under it.
        //
        // The window is **derived** rather than a number: short enough that the
        // list scrolls, tall enough that row eight is still drawn once it has.
        // Both are facts about the list, and a literal here goes stale the next
        // time somebody adds a row to the card menu — which is exactly how this
        // test last broke. Solving `beside`'s own arithmetic for the height
        // that puts the row `SHOW` below the top of the window, the corner the
        // menu hangs from cancels out and this is what is left.
        const SHOW: f32 = 48.0;
        let above = Menu::top_of(&CARD_MENU, 8);
        let room = room(640.0, SHOW + MARGIN + Menu::height(&CARD_MENU) - above);
        let menu = Menu::new(at(10.0, 10.0), &CARD_MENU, room);
        let scroll = menu.list.size.height - px(Menu::height(&CARD_MENU));
        assert!(scroll < px(0.0), "the card list should not fit this window");

        let Entry::More(_, list) = CARD_MENU[8] else { panic!("row eight opens onto more") };
        let drawn = menu.list.at.y + px(above) + scroll;
        // The premise, stated rather than assumed. A row scrolled off the top
        // has no place for a submenu to be level with, and `beside` rightly
        // slides one back inside the window instead — so a fixture that let
        // that happen would be testing the clamp while claiming to test the
        // scroll.
        assert_eq!(drawn, px(SHOW), "row eight has to still be on screen");
        assert_eq!(menu.beside(8, list, room, scroll).at.y, drawn);
        assert_ne!(menu.beside(8, list, room, px(0.0)).at.y, drawn, "the scroll has to count");
    }
}
