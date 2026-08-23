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
//! app, and this one cannot. So it is fitted rather than allowed to spill —
//! flipped away from the edge it would cross, then clamped, then, if the
//! window is genuinely shorter than the list, scrolled. Which is why the lists
//! grew submenus: the shortest list is the one least likely to need any of it.

use gpui::{div, prelude::*, px, Bounds, Context, MouseButton, Pixels, Point, Size};

use crate::board_view::BoardView;
use crate::command::{Command, Entry};

/// How wide the list is. Fixed, so the entries line up down the right-hand edge
/// rather than the box changing width with whichever board is open.
const WIDTH: f32 = 216.0;
const ROW_HEIGHT: f32 = 26.0;
const RULE_HEIGHT: f32 = 9.0;
const PADDING: f32 = 6.0;
/// How far a submenu tucks back under the list that opened it.
///
/// A gap between the two would be a gap the pointer crosses on its way, and
/// crossing it means leaving both — which closes the thing being aimed at.
const OVERLAP: f32 = 4.0;
/// The mark on a row that opens onto more.
const CHEVRON: &str = "\u{25b8}";

/// Where an open menu is, and what it offers.
#[derive(Debug, Clone)]
pub struct Menu {
    /// Canvas coordinates of the corner it hangs from.
    pub at: Point<Pixels>,
    /// What it offers. Held rather than looked up at paint time, so that a
    /// menu opened over a rope stays the rope's menu even if the command it
    /// runs changes what is selected underneath it.
    pub entries: &'static [Entry],
    /// The submenu that is open, where one is.
    pub open: Option<Open>,
}

/// A submenu, and the row of its parent it belongs to.
#[derive(Debug, Clone)]
pub struct Open {
    /// Which row opened it, so that row can stay lit while the pointer is
    /// inside the list it opened.
    pub row: usize,
    pub at: Point<Pixels>,
    pub entries: &'static [Entry],
}

impl Menu {
    pub fn new(at: Point<Pixels>, entries: &'static [Entry]) -> Self {
        Self { at, entries, open: None }
    }

    /// How tall the list wants to be, before the window has its say.
    fn height(entries: &[Entry]) -> f32 {
        let rows = entries.iter().filter(|e| !matches!(e, Entry::Rule)).count() as f32;
        let rules = entries.iter().filter(|e| matches!(e, Entry::Rule)).count() as f32;
        rows * ROW_HEIGHT + rules * RULE_HEIGHT + PADDING * 2.0
    }

    /// How far down the list a row starts.
    fn top_of(entries: &[Entry], row: usize) -> f32 {
        entries[..row.min(entries.len())]
            .iter()
            .map(|e| if matches!(e, Entry::Rule) { RULE_HEIGHT } else { ROW_HEIGHT })
            .sum::<f32>()
            + PADDING
    }

    /// Put the corner somewhere the whole list will fit.
    ///
    /// Flipped rather than clamped where there is room the other way: a menu
    /// opened near the bottom of the window grows upward from the pointer,
    /// which is what every other menu does and what stops the first entry
    /// landing under the cursor where a stray click would take it. Clamped
    /// after, for the window too small to flip inside of, and a list taller
    /// than the window starts at the top and scrolls — see [`render`].
    pub fn placed(at: Point<Pixels>, window: Bounds<Pixels>, entries: &[Entry]) -> Point<Pixels> {
        let (w, h) = (px(WIDTH), px(Self::height(entries)));
        let x = if at.x + w > window.size.width { at.x - w } else { at.x };
        let y = if at.y + h > window.size.height { at.y - h } else { at.y };
        gpui::point(
            x.clamp(px(0.0), (window.size.width - w).max(px(0.0))),
            y.clamp(px(0.0), (window.size.height - h).max(px(0.0))),
        )
    }

    /// Where a submenu opened off `row` should hang.
    ///
    /// Beside the row rather than under the pointer, and on the far side of the
    /// list unless that would leave the window — in which case it comes out the
    /// other way, which is the same flip the parent made.
    fn beside(&self, row: usize, list: &[Entry], room: Size<Pixels>) -> Point<Pixels> {
        let (w, h) = (px(WIDTH), px(Self::height(list)));
        let right = self.at.x + px(WIDTH - OVERLAP);
        let x = if right + w > room.width { self.at.x - w + px(OVERLAP) } else { right };
        let y = self.at.y + px(Self::top_of(self.entries, row) - PADDING);
        gpui::point(
            x.clamp(px(0.0), (room.width - w).max(px(0.0))),
            y.clamp(px(0.0), (room.height - h).max(px(0.0))),
        )
    }

    /// The pointer has arrived on `row`. Answers whether anything moved.
    ///
    /// Opening is on arrival and closing is on arriving somewhere *else*,
    /// rather than on leaving: a row that closed its submenu on the way out
    /// would close it as the pointer crossed into it.
    ///
    /// `opens` is asked of the caller rather than worked out here, because
    /// availability is a question about the *board* and this holds none of it.
    pub fn reveal(&mut self, row: usize, room: Size<Pixels>, opens: bool) -> bool {
        match self.entries.get(row).copied() {
            Some(Entry::More(_, list)) if opens => {
                if self.open.as_ref().is_some_and(|open| open.row == row) {
                    return false;
                }
                self.open = Some(Open { row, at: self.beside(row, list, room), entries: list });
                true
            }
            _ => self.open.take().is_some(),
        }
    }
}

pub fn render(menu: &Menu, view: &BoardView, cx: &mut Context<BoardView>) -> impl IntoElement {
    let room = view.room();
    let lit = menu.open.as_ref().map(|open| open.row);

    // Both lists in one wrapper, so the submenu is a sibling of the list that
    // opened it and paints over it rather than inside it. The wrapper fills the
    // canvas and listens to nothing, so a press that misses both goes through
    // to the board underneath — which is what puts the menu away.
    div()
        .absolute()
        .size_full()
        .child(list("menu", menu.at, menu.entries, lit, true, room, view, cx))
        .children(
            menu.open
                .as_ref()
                .map(|open| list("submenu", open.at, open.entries, None, false, room, view, cx)),
        )
}

/// One list, at a place, as tall as the window will allow.
#[allow(clippy::too_many_arguments)]
fn list(
    name: &'static str,
    at: Point<Pixels>,
    entries: &'static [Entry],
    lit: Option<usize>,
    hovers: bool,
    room: Size<Pixels>,
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
                .my(px(4.0))
                .mx(px(8.0))
                .border_t_1()
                .border_color(theme.chrome_edge)
                .into_any_element(),
            entry => row(name, i, *entry, lit == Some(i), hovers, view, cx).into_any_element(),
        })
        .collect();

    div()
        .id(name)
        .absolute()
        .left(at.x)
        .top(at.y)
        .w(px(WIDTH))
        // The last resort, for a window shorter than the list itself. It costs
        // nothing when it is not needed, and what it replaces is entries that
        // are drawn and cannot be reached.
        .max_h((room.height - px(2.0 * PADDING)).max(px(ROW_HEIGHT)))
        .overflow_y_scroll()
        .py(px(PADDING))
        .rounded(px(8.0))
        .bg(theme.chrome)
        .border_1()
        .border_color(theme.chrome_edge)
        .shadow_lg()
        .text_size(px(12.0))
        .text_color(theme.text)
        // The canvas beneath listens on mouse-down, so without this a press
        // anywhere on the menu would also start a gesture on the board behind
        // it — including the press that chooses an entry.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .children(rows)
}

fn row(
    name: &'static str,
    key: usize,
    entry: Entry,
    lit: bool,
    hovers: bool,
    view: &BoardView,
    cx: &mut Context<BoardView>,
) -> impl IntoElement {
    let theme = view.theme;
    let available = entry.available(view);
    let ticked = matches!(entry, Entry::Does(command) if command.ticked(view) == Some(true));
    let more = matches!(entry, Entry::More(..));

    // Named after the step it would take back rather than labelled "Undo", so
    // the entry says what is about to happen instead of only whether anything
    // can. This is the one thing worth keeping from the panel it replaces.
    let label = match entry {
        Entry::Does(Command::Undo) => step_label("Undo", view.undo_step()),
        Entry::Does(Command::Redo) => step_label("Redo", view.redo_step()),
        Entry::Does(command) => command.label().to_string(),
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
        .rounded(px(4.0))
        .when(!available, |d| d.text_color(theme.muted))
        // A row whose submenu is open stays lit while the pointer is off in
        // that submenu, which is what says where the second list came from.
        .when(lit, |d| d.bg(theme.accent.opacity(0.16)))
        // Arriving anywhere on the parent list settles which submenu is open —
        // including on a row that has none, which closes the one that is.
        .when(hovers, |d| {
            d.on_hover(cx.listener(move |this, over: &bool, _window, cx| {
                if *over {
                    this.reveal_menu(key, cx);
                }
            }))
        })
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
                // A fixed-width gutter, so the labels line up whether or not
                // the entry above them happens to be ticked.
                .child(div().w(px(10.0)).text_color(theme.accent).child(if ticked {
                    "\u{2713}"
                } else {
                    ""
                }))
                .child(label),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.0))
                .text_size(px(10.0))
                .text_color(theme.muted)
                .child(entry.hint(view))
                .when(more, |d| d.child(div().text_size(px(11.0)).child(CHEVRON))),
        )
}

fn step_label(verb: &str, step: Option<String>) -> String {
    match step {
        Some(name) => format!("{verb} {}", name.to_lowercase()),
        None => verb.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> Bounds<Pixels> {
        Bounds::new(gpui::point(px(0.0), px(0.0)), gpui::size(px(1000.0), px(700.0)))
    }

    #[test]
    fn a_menu_with_room_hangs_from_where_you_pressed() {
        let at = gpui::point(px(100.0), px(80.0));
        assert_eq!(Menu::placed(at, window(), &crate::command::CARD_MENU), at);
    }

    #[test]
    fn a_menu_near_an_edge_grows_the_other_way() {
        let at = gpui::point(px(960.0), px(690.0));
        let entries = &crate::command::CARD_MENU;
        let placed = Menu::placed(at, window(), entries);
        assert!(placed.x < at.x, "it should have flipped left");
        assert!(placed.y < at.y, "it should have flipped up");
        assert!(placed.x + px(WIDTH) <= window().size.width);
        assert!(placed.y + px(Menu::height(entries)) <= window().size.height);
    }

    #[test]
    fn a_menu_that_cannot_flip_clear_of_an_edge_is_pushed_off_it() {
        // A window narrower than twice the list: flipping left puts the corner
        // at a negative x, so neither side has room and the list is slid back
        // in rather than left hanging over the edge it was drawn past.
        let narrow = Bounds::new(gpui::point(px(0.0), px(0.0)), gpui::size(px(300.0), px(700.0)));
        let placed =
            Menu::placed(gpui::point(px(290.0), px(10.0)), narrow, &crate::command::ROPE_MENU);
        assert!(placed.x >= px(0.0));
        assert!(placed.x + px(WIDTH) <= narrow.size.width);
    }

    #[test]
    fn a_menu_taller_than_the_window_still_starts_on_screen() {
        // Flipping would put the corner above the top of the window, which is
        // worse than a list that runs off the bottom: the first entries would
        // be the unreachable ones. It scrolls instead — see `render`.
        let short = Bounds::new(gpui::point(px(0.0), px(0.0)), gpui::size(px(400.0), px(120.0)));
        let placed =
            Menu::placed(gpui::point(px(10.0), px(100.0)), short, &crate::command::ROPE_MENU);
        assert_eq!(placed.y, px(0.0));
        assert!(placed.x >= px(0.0));
    }

    #[test]
    fn a_submenu_hangs_off_its_own_row_and_beside_the_list() {
        let entries = &crate::command::ROPE_MENU[..];
        let menu = Menu::new(gpui::point(px(100.0), px(80.0)), entries);
        let Entry::More(_, list) = entries[4] else { panic!("row four opens onto more") };
        let at = menu.beside(4, list, window().size);
        assert_eq!(at.x, px(100.0 + WIDTH - OVERLAP), "beside, tucked under the edge");
        assert!(at.y > menu.at.y, "and level with the row rather than the corner");
    }

    #[test]
    fn a_submenu_with_no_room_to_its_right_comes_out_the_left() {
        let entries = &crate::command::ROPE_MENU[..];
        let menu = Menu::new(gpui::point(px(700.0), px(80.0)), entries);
        let Entry::More(_, list) = entries[4] else { panic!("row four opens onto more") };
        let at = menu.beside(4, list, gpui::size(px(800.0), px(700.0)));
        assert!(at.x < menu.at.x, "it should have come out the other side");
        assert!(at.x >= px(0.0));
    }
}
