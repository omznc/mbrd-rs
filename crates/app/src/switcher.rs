//! Jumping between boards without leaving the keyboard.
//!
//! A board is this app's project, so this is the thing Zed opens on
//! `Ctrl P`: a list of the ones you have had open, narrowed as you type, chosen
//! with the arrows and Enter. It exists because the alternative — a file
//! picker — is a dialogue somebody else draws, takes a second to appear, and
//! makes moving between two boards you are working on into a chore.
//!
//! The list is the boards remembered from previous runs, plus every `.mbrd`
//! sitting next to the one that is open and in the directory the app was
//! started from. That last part is what makes it useful before there is any
//! history to remember, which is the first time anybody uses it.
//!
//! Typing is handled by [`Editor`], the same one `palette.rs` delegates to and
//! for the reason its own docstring gives: a query is a text field, and a
//! text field that reimplemented the easy third of what one needs — the
//! caret, the selection, word motion, paste — would be one where half the
//! keys somebody reaches for silently do nothing. Which is what this used to
//! be: a hand-rolled `String` with the caret pinned at the end, no arrow keys
//! inside it, no selection, and no paste at all.
//!
//! It is also where boards are made and where they are deleted, which is a
//! surprising amount of filing for a list — but this is the one surface in the
//! app that is *about boards* rather than about what is on one. Somebody who
//! opens it and finds twenty untitled boards from twenty `Ctrl N`s wants to be
//! rid of nineteen of them here, not in a file manager, and a list that could
//! only ever grow would make that true by the end of a week. Deleting asks
//! first — see [`Switcher::confirming`] — because it is the only thing this app
//! does that cannot be undone by doing it again.

use std::path::{Path, PathBuf};

use gpui::{
    div, prelude::*, px, AnyElement, Context, FontWeight, Modifiers, MouseButton, ScrollHandle,
};

use crate::board_view::BoardView;
use crate::command::Command;
use crate::editor::{self, Editor};
use crate::icons::{icon, Icon};
use crate::tip::tip;

/// How many matches to show.
///
/// Past this the list is longer than the answer — but the list *scrolls*, so
/// this is a bound on how much work a keystroke does rather than on what fits
/// on screen. Somebody with forty boards should be able to reach the fortieth.
const SHOWN: usize = 40;

/// An open switcher: what has been typed, and where the highlight is.
#[derive(Debug, Clone)]
pub struct Switcher {
    /// What has been typed, as a real text field. See the module note.
    pub query: Editor,
    /// Which of the *matches* is highlighted, not which of the boards.
    pub cursor: usize,
    /// How far the list has scrolled.
    ///
    /// On the struct rather than in the painter because the painter is handed
    /// a *clone* of this every frame — a handle held there would be a fresh
    /// one each time, and the list would spring back to the top on every
    /// keystroke. `ScrollHandle` is a shared position, so the clone and the
    /// original are the same scroll.
    pub scroll: ScrollHandle,
    boards: Vec<PathBuf>,
    /// How many of [`Self::boards`] came out of `recent.json`.
    ///
    /// Everything from this index on was *found* rather than remembered — see
    /// [`beside_boards`] — and the list says so, because the two are different
    /// claims. A remembered board is one somebody has had open. A found one is
    /// a file that happens to be in the same folder, which might be a
    /// colleague's, a backup, or something they have never opened in their
    /// life. Presenting the second as though it were the first is how a list
    /// of "your boards" quietly stops being one.
    recent: usize,
    /// The board a delete has been asked for and not yet confirmed.
    ///
    /// Deleting is the one thing this list does that cannot be undone by doing
    /// it again, so it is the one thing here that asks twice. The question is
    /// put *in the row*, where the name is, rather than in a dialogue over the
    /// top: a modal on top of a modal would leave the list — the thing you are
    /// checking the name against — behind two layers of chrome.
    confirming: Option<PathBuf>,
    /// The board that is open right now, so that it is not offered for
    /// deletion.
    ///
    /// Deleting the open board would leave the view holding a path that is not
    /// there, and the next autosave would write the file straight back — so the
    /// board would appear to survive its own deletion, then reappear in the
    /// list. Refusing is simpler to explain than either of the alternatives.
    open: Option<PathBuf>,
    /// The board a delete was asked for, and the reason it did not happen.
    ///
    /// Rendered in the row where "delete it?" was — see `confirm` — rather
    /// than in the status bar at the far corner of the window, which is
    /// where this used to go. A board on a read-only disk is a fact about
    /// *that row*, and saying so somewhere else is asking somebody to
    /// remember which of forty rows it was about.
    refused: Option<(PathBuf, String)>,
}

impl Switcher {
    /// Open with the boards `recent.json` already remembered — a read of one
    /// small file, fast enough for the thread that draws. Everything *beside*
    /// the board that is open costs two directory reads and a `canonicalize`
    /// per file found in them, and now runs on the background executor
    /// instead: see `BoardView::open_switcher`, which calls
    /// [`beside_boards`] there and hands the answer to [`Switcher::extend_boards`]
    /// once it is ready.
    pub fn open(current: Option<&Path>) -> Self {
        let boards = crate::recent::load();
        Self {
            query: Editor::new("", crate::palette::QUERY_MAX, false),
            cursor: 0,
            scroll: ScrollHandle::new(),
            // Everything here so far is remembered. Anything the scan adds
            // lands past this mark. See [`Self::recent`].
            recent: boards.len(),
            boards,
            confirming: None,
            refused: None,
            open: current.map(|p| p.canonicalize().unwrap_or_else(|_| p.to_path_buf())),
        }
    }

    /// Whether this row is a board that was found rather than remembered.
    ///
    /// A search over the list rather than a flag on each entry, because the
    /// list is capped at [`SHOWN`] and this is asked once per drawn row: a
    /// parallel structure to keep in step would cost more to be right than
    /// the scan costs to run.
    pub fn found_beside(&self, board: &Path) -> bool {
        self.boards.iter().position(|p| p == board).is_some_and(|at| at >= self.recent)
    }

    /// Add the boards found beside the one that is open, once the background
    /// scan for them — [`beside_boards`] — comes back. Skips anything already
    /// in the list: a board that is both recent and beside the current one
    /// should not appear twice.
    pub fn extend_boards(&mut self, found: Vec<PathBuf>) {
        for path in found {
            if !boards_contains(&self.boards, &path) {
                self.boards.push(path);
            }
        }
    }

    /// Whether this board is one this list is willing to delete.
    ///
    /// Everything but the one that is open. See the field of that name.
    fn deletable(&self, board: &Path) -> bool {
        self.open.as_deref() != Some(board)
    }

    /// The board a delete has been asked for and not answered.
    pub fn doomed(&self) -> Option<PathBuf> {
        self.confirming.clone()
    }

    /// Ask about deleting a board, or answer "no" with `None`.
    ///
    /// The whole of the arming: nothing is touched until the question is
    /// answered, and the answer arrives as a key press or as a press on the
    /// button the row grows while this is set.
    pub fn ask_about(&mut self, board: Option<PathBuf>) {
        self.confirming = board.filter(|b| self.deletable(b));
        self.refused = None;
    }

    /// A delete was tried and the disk said no. See `refused`.
    pub fn refuse(&mut self, board: PathBuf, reason: String) {
        self.confirming = None;
        self.refused = Some((board, reason));
    }

    /// The reason the last delete of `board` failed, where it did and the
    /// row is still the one it happened in.
    pub fn refusal(&self, board: &Path) -> Option<&str> {
        self.refused
            .as_ref()
            .filter(|(doomed, _)| doomed == board)
            .map(|(_, reason)| reason.as_str())
    }

    /// Forget a board that is no longer on disk.
    ///
    /// The list is gathered once when it opens — see [`Switcher::open`] — so
    /// nothing else is going to notice that a file has gone. Without this the
    /// row stays, and pressing it opens a board that is not there.
    pub fn dropped(&mut self, board: &Path) {
        // Before the retain, or the mark that separates remembered from found
        // slides by one every time a remembered board is forgotten — and the
        // section header would start appearing over rows that belong above it.
        if self.boards.iter().position(|p| p == board).is_some_and(|at| at < self.recent) {
            self.recent -= 1;
        }
        self.boards.retain(|p| p != board);
        self.confirming = None;
        self.refused = None;
        self.cursor = self.cursor.min(self.matches().len().saturating_sub(1));
    }

    /// The boards worth showing, best first.
    pub fn matches(&self) -> Vec<&Path> {
        if self.query.text().is_empty() {
            return self.boards.iter().take(SHOWN).map(PathBuf::as_path).collect();
        }
        let recent = self.recent;
        let mut scored: Vec<(bool, i32, usize, &Path)> = self
            .boards
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                score(self.query.text(), p).map(|s| (i >= recent, s, i, p.as_path()))
            })
            .collect();
        // Remembered before found, whatever either scores. The grouping is
        // load-bearing rather than cosmetic: the list draws a heading over the
        // found ones saying what they are, and a heading is a lie if a row
        // above it belongs below. Within a group, best score first, and where
        // two score the same the more recent one — which is the order `boards`
        // is already in.
        scored.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
        scored.into_iter().take(SHOWN).map(|(_, _, _, p)| p).collect()
    }

    /// What Enter would open.
    pub fn chosen(&self) -> Option<PathBuf> {
        self.matches().get(self.cursor).map(|p| p.to_path_buf())
    }

    /// Take a key press. Answers what the view should do about it.
    ///
    /// The list's own keys — Escape, Enter, `Delete`, the arrows and paging —
    /// are read first, ahead of the text field: they mean something different
    /// here than they would inside a line of text, the same division
    /// `Palette::key` draws and for the same reason. Everything else goes to
    /// [`Editor`], which already knows the rest of what a one-line query
    /// needs.
    pub fn key(&mut self, key: &str, mods: Modifiers, text: Option<&str>) -> Reply {
        match key {
            // A pending delete is what escape answers first. It is the more
            // recent of the two questions on screen, and closing the whole
            // switcher would be answering one nobody asked.
            "escape" => {
                return match self.confirming.take() {
                    Some(_) => Reply::Held,
                    None => Reply::Close,
                }
            }
            "enter" if self.confirming.is_some() => return Reply::Delete,
            "enter" => return Reply::Open,
            // The key every list in the world deletes with, and it cannot be
            // typed into the query — so it is free here in a way no letter is.
            "delete" => {
                let chosen = self.chosen();
                self.ask_about(chosen);
                return Reply::Held;
            }
            // Everything below moves the highlight or changes the list, so the
            // row the question was about is no longer the row under it. Left
            // to `Editor` otherwise: a one-line field has no line above it to
            // walk to, so the arrows are free for the list.
            "up" => {
                self.confirming = None;
                self.refused = None;
                self.step(-1);
                return Reply::Held;
            }
            "down" => {
                self.confirming = None;
                self.refused = None;
                self.step(1);
                return Reply::Held;
            }
            "pageup" => {
                self.confirming = None;
                self.refused = None;
                self.step(-10);
                return Reply::Held;
            }
            "pagedown" => {
                self.confirming = None;
                self.refused = None;
                self.step(10);
                return Reply::Held;
            }
            _ => {}
        }

        let before = self.query.text().to_string();
        let reply = self.query.key(key, editor::Mods::from(mods), text);
        // Only what the text field would not take. Paste is the one worth
        // having — a name or a fragment of a path copied from somewhere else
        // is exactly what somebody hunts a board by — and it needs the
        // clipboard, which is the view's rather than ours. Same division
        // `Palette::key` draws.
        if reply == editor::Reply::Ignored && mods.secondary() && key == "v" {
            return Reply::Paste;
        }
        if self.query.text() != before {
            self.confirming = None;
            self.refused = None;
            self.cursor = 0;
        }
        Reply::Held
    }

    /// Put text into the query, at the caret. For a paste.
    pub fn insert(&mut self, text: &str) {
        self.query.insert(text);
        self.confirming = None;
        self.refused = None;
        self.cursor = 0;
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

/// What the view should do with a key the switcher was given.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reply {
    /// Dealt with. Redraw.
    Held,
    /// Put it away and change nothing.
    Close,
    /// Open what is highlighted.
    Open,
    /// Delete the board named by [`Switcher::doomed`], which has been asked
    /// about and answered.
    Delete,
    /// Put the clipboard into the query. The view has the clipboard.
    Paste,
}

fn boards_contains(boards: &[PathBuf], path: &Path) -> bool {
    boards.iter().any(|p| p == path)
}

/// Every `.mbrd` in the boards folder, beside the board that is open, and
/// beside wherever the app was started from, deduplicated and canonicalised.
///
/// Pure disk IO and no `Switcher` in sight, which is what lets
/// `BoardView::open_switcher` hand this to the background executor rather
/// than running it on the thread that draws — the whole point of splitting
/// it out of what [`Switcher::open`] used to do inline. See
/// [`Switcher::extend_boards`], where the answer lands.
///
/// **The boards folder is looked in first**, and that is the promise the
/// welcome screen makes in so many words when it asks where boards should
/// live: a folder somebody nominated as the place their work goes is not
/// worth less than whichever directory a launcher happened to start the app
/// in. `boards` is passed in rather than read off the prefs here, for the
/// reason the rest of this module is a plain struct — it runs on the
/// background executor and has no view to ask.
pub fn beside_boards(current: Option<&Path>, boards: Option<&Path>) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut add = |extra: Vec<PathBuf>| {
        for path in extra {
            let path = path.canonicalize().unwrap_or(path);
            if !boards_contains(&found, &path) {
                found.push(path);
            }
        }
    };
    if let Some(dir) = boards {
        add(crate::recent::beside(dir));
    }
    if let Some(dir) = current.and_then(Path::parent) {
        add(crate::recent::beside(dir));
    }
    if let Ok(here) = std::env::current_dir() {
        add(crate::recent::beside(&here));
    }
    found
}

/// How well a path answers a query, or `None` for not at all.
///
/// A subsequence match, scored so that the obvious thing wins: letters in a row
/// beat letters scattered, and a match in the file name beats one in the
/// directories above it — typing `kit` should find `kitchen.mbrd` before
/// `~/kitchen-drafts/other.mbrd`.
fn score(query: &str, path: &Path) -> Option<i32> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    let whole = path.to_string_lossy().to_lowercase();
    let query = query.to_lowercase();

    // The file name first, at a premium, then the whole path as a fallback.
    if let Some(points) = crate::fuzzy::subsequence(&query, &name) {
        return Some(points + 1_000);
    }
    crate::fuzzy::subsequence(&query, &whole)
}

pub fn render(
    switcher: &Switcher,
    view: &BoardView,
    cx: &mut Context<BoardView>,
) -> impl IntoElement {
    let theme = view.theme;
    let presence = view.overlay_presence.value();
    let matches = switcher.matches();

    // Whether the heading over the found boards has been placed yet. The
    // matches are grouped — see `Switcher::matches` — so the first found board
    // is where the section starts, and there is exactly one of them.
    let mut headed = false;
    let mut rows: Vec<gpui::AnyElement> = Vec::with_capacity(matches.len() + 1);
    for (i, path) in matches.iter().enumerate() {
        if !headed && switcher.found_beside(path) {
            headed = true;
            rows.push(beside_heading(theme));
        }
        rows.push({
            let highlighted = i == switcher.cursor;
            let name =
                path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let where_ = path.parent().map(shorten).unwrap_or_default();
            let target = path.to_path_buf();
            // The question is asked in the row rather than over it. See
            // `Switcher::confirming`.
            let doomed = switcher.confirming.as_deref() == Some(*path);
            let refused = switcher.refusal(path);
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
                .when(highlighted && !doomed, |d| d.bg(theme.accent.opacity(0.20)))
                // Lit in the colour of the thing about to happen, so that the
                // row being asked about is the row you are looking at.
                .when(doomed, |d| d.bg(theme.accent.opacity(0.14)))
                .when(!doomed, |d| {
                    d.hover(|s| s.bg(theme.accent.opacity(0.12)))
                        // Opening a board is the slowest thing in the app — a
                        // file to read and a board to build — so the row has to
                        // say it was pressed before any of that starts, or the
                        // press looks lost. The read itself is off the drawing
                        // thread and draws its own loader; this covers only the
                        // instant between the press and that appearing.
                        .active(|s| s.bg(theme.accent.opacity(0.3)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _event, _window, cx| {
                                this.close_switcher();
                                this.open_board(&target, cx);
                            }),
                        )
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .overflow_hidden()
                        // The same picture on every row, which is the point:
                        // it is not telling one board from another, it is
                        // telling a *board* from the directory path beside it.
                        .child(icon(Icon::Board, crate::icons::ICON_MD, theme.muted))
                        // Medium rather than regular, the same reason the
                        // palette's row title carries the weight now: it is
                        // the thing this list exists to let you aim at.
                        .child(
                            div()
                                .text_size(px(13.0))
                                .font_weight(FontWeight::MEDIUM)
                                .truncate()
                                .child(name),
                        ),
                )
                .child(match refused {
                    // The reason a delete failed, in the row it was asked
                    // for — see `Switcher::refuse` for why here rather than
                    // the status bar.
                    Some(reason) => div()
                        .flex_none()
                        .max_w(px(240.0))
                        .text_size(px(11.0))
                        .text_color(theme.accent_text)
                        .truncate()
                        .child(reason.to_string())
                        .into_any_element(),
                    None => div()
                        .flex()
                        .flex_none()
                        .items_center()
                        .gap(px(4.0))
                        .child(
                            div()
                                .text_size(px(11.0))
                                .text_color(theme.muted)
                                .overflow_hidden()
                                .child(where_),
                        )
                        // No cross while this row is the one being asked
                        // about: the question is at the foot of the panel and
                        // it has its own two buttons, and a third way to
                        // answer it sitting in the row would be a control
                        // whose meaning changed under the pointer.
                        .when(switcher.deletable(path) && !doomed, |d| {
                            d.child(delete_button(cx, theme, path, i))
                        })
                        .into_any_element(),
                })
                .into_any_element()
        });
    }

    // Keep the highlight on screen. The arrows can walk past the bottom of a
    // list this tall, and a selection you cannot see is a selection you have to
    // guess at — Enter would then open a board nobody chose. Asked for every
    // frame rather than only when the cursor moves, because it is a no-op when
    // the row is already in view and it also covers the list *changing* under a
    // fixed cursor, which is what typing does.
    switcher.scroll.scroll_to_item(switcher.cursor);

    // Slides down 8px as it arrives and leaves back up the way it came —
    // the same offset the palette uses and for the same reason: GPUI cannot
    // scale a div, so a fade plus a small motion along the panel's own axis
    // is what "arriving" and "leaving" are made of. One function of the
    // current presence covers both directions, which is what keeps the exit
    // the entrance played backwards rather than a different animation.
    let arrive = 8.0 * (1.0 - presence);

    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        .flex()
        .justify_center()
        // The panel is as tall as what is in it, not as tall as it is allowed
        // to get. Without this the default cross-axis stretch pulls it to its
        // own `max_h` whatever the list holds, and three boards leave the
        // footer floating in the middle of an empty box — see `palette.rs`,
        // which hugs its content for the same reason.
        .items_start()
        // Not centred vertically: a list that grows downward from a fixed point
        // does not move the thing you are aiming at as you type.
        .pt(px(96.0))
        // A scrim, faded in with the panel — the board behind a modal list
        // reads as *behind* it rather than merely covered by it.
        .bg(theme.ground.opacity(0.25 * presence))
        // The wheel belongs to whatever is on top. Without this the board
        // underneath takes it too and the list scrolls while the canvas zooms
        // out from under it — see `menu.rs`, which stops it the same way and
        // for the same reason. The list's own scrolling is unharmed: gpui
        // registers the child's handler first.
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        // A press anywhere outside puts it away, which is what every other
        // palette does and what stops it becoming something you have to
        // dismiss deliberately.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _event, _window, cx| {
                this.close_switcher();
                cx.notify();
            }),
        )
        // The right button too, or a press outside puts the list away *and*
        // opens the board's own menu behind it — which is a menu about a card
        // nobody could see, belonging to a surface nobody was pressing on.
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(|this, _event, _window, cx| {
                cx.stop_propagation();
                this.close_switcher();
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
                .mt(px(-arrive))
                .opacity(presence)
                .bg(theme.chrome)
                .border_1()
                .border_color(theme.chrome_edge)
                .shadow(theme.shadow_large())
                .text_color(theme.text)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                // Nothing here has a context menu, and the board behind it has
                // one — so without this, right-pressing a row opens the menu
                // for whatever card happens to be under the list.
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        // The same pair `palette.rs` puts its own header on —
                        // see [`crate::palette::HEADER_PAD_X`].
                        .px(px(crate::palette::HEADER_PAD_X))
                        .py(px(crate::palette::HEADER_PAD_Y))
                        .border_b_1()
                        .border_color(theme.chrome_edge)
                        .text_size(px(14.0))
                        // The query takes the room the button does not, so the
                        // caret sits where you are typing rather than being
                        // pushed along by a control at the far end of the row.
                        .child(div().flex_1().min_w_0().py(px(4.0)).child(
                            crate::palette::query_line(
                                &switcher.query,
                                "open a board\u{2026}",
                                14.0,
                                true,
                                &theme,
                            ),
                        ))
                        .child(new_board_button(cx, theme)),
                )
                .child(
                    div()
                        .id("boards")
                        .flex()
                        .flex_col()
                        .py(px(6.0))
                        .overflow_y_scroll()
                        .track_scroll(&switcher.scroll)
                        .when(rows.is_empty(), |d| {
                            d.px(px(14.0))
                                .py(px(10.0))
                                .text_size(px(12.0))
                                .text_color(theme.muted)
                                // See `palette.rs`'s own empty state: the one
                                // line of prose in this panel wants the air
                                // between wrapped lines that a status line
                                // does not.
                                .line_height(gpui::relative(1.45))
                                .child(if switcher.query.text().is_empty() {
                                    "no boards yet \u{2014} save one and it will be here"
                                } else {
                                    "no board by that name"
                                })
                        })
                        .children(rows),
                )
                // The question, where there is one, in place of the keys —
                // not under them. While it is up it is the only thing on this
                // panel worth reading, and two strips of small print at the
                // foot of a list is how a question gets missed.
                .children(switcher.doomed().map(|board| confirm(cx, theme, &board)))
                // Names the keys that leave this mode, the same rule
                // `board_view.rs` applies everywhere else a mode is entered:
                // every mode names the key that gets out of it.
                .when(switcher.doomed().is_none(), |d| {
                    d.child(
                        div()
                            .px(px(14.0))
                            .py(px(7.0))
                            .border_t_1()
                            .border_color(theme.chrome_edge)
                            .text_size(px(10.0))
                            .text_color(theme.muted)
                            .child("\u{2191}\u{2193} move · enter open · del remove · esc close"),
                    )
                }),
        )
}

/// The line over the boards that were found rather than remembered.
///
/// It says what they *are* rather than titling them: "sitting next to this
/// one" is the whole difference between these rows and the ones above, and a
/// heading reading "Nearby" would leave somebody to work that out.
fn beside_heading(theme: crate::theme::Theme) -> gpui::AnyElement {
    div()
        .flex()
        .flex_col()
        .child(div().h(px(1.0)).mx(px(12.0)).my(px(5.0)).bg(theme.chrome_edge))
        .child(
            div()
                .px(px(18.0))
                .pb(px(4.0))
                .font(crate::opened::mono())
                .text_size(px(9.5))
                .text_color(theme.tertiary)
                .child("SITTING NEXT TO THIS ONE"),
        )
        .into_any_element()
}

/// The cross that asks whether a board should go.
///
/// Regular weight rather than duotone, like every other cross, plus and minus
/// in the app: those four are *controls* rather than pictures of things, and a
/// second tone on a twelve-pixel glyph made of two strokes reads as a smudge.
///
/// It asks rather than acts, which is the reason it can sit on every row of a
/// list somebody is arrowing through at speed.
fn delete_button(
    cx: &mut Context<BoardView>,
    theme: crate::theme::Theme,
    board: &Path,
    row: usize,
) -> impl IntoElement {
    let board = board.to_path_buf();
    div()
        .id(("delete-board", row))
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(20.0))
        .h(px(20.0))
        .rounded(px(crate::theme::RADIUS_XS))
        .hover(|s| s.bg(theme.accent.opacity(0.2)))
        .active(|s| s.bg(theme.accent.opacity(0.36)))
        // A wordless button, so it names itself the same way every other one
        // in this app does — see `titlebar.rs`'s test that every one of them
        // can. There is no key for this one, so the tip carries only the
        // name.
        .tooltip(tip(theme, "Delete board", ""))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _event, _window, cx| {
                // The row underneath opens the board. Without this, asking to
                // delete one would open it first.
                cx.stop_propagation();
                this.ask_about_board(Some(board.clone()));
                cx.notify();
            }),
        )
        .child(icon(Icon::Close, crate::icons::ICON_SM, theme.muted))
}

/// The two answers, where the directory usually is.
///
/// Words rather than a tick and a cross. Everywhere else in this app a small
/// glyph is a thing you can try and undo; this is the one press that cannot be
/// taken back, and "delete" spelled out is the difference between reading the
/// row and recognising it.
///
/// **Weighted the way the two answers actually differ.** This used to draw
/// both the same size and the same shape — accent-on-accent-wash at eleven
/// pixels regular for "delete", and a quieter version of the same chip for
/// "keep" — which put the loudest treatment on screen nowhere near the one
/// press in this app that cannot be undone. "delete" is now the solid button:
/// [`crate::theme::Theme::accent`] as a fill rather than a wash, the board's
/// text colour reversed out of it, and semibold, because the thing you are
/// one press from doing should look like the thing you are one press from
/// doing. "keep" is deliberately the quieter of the two — the titlebar's own
/// hover wash, [`crate::theme::Theme::text`] at 8% — because it is the answer
/// that changes nothing, and a safe answer dressed up to compete with the
/// dangerous one is not actually safer to press.
/// The question a delete asks before it happens.
///
/// **The one thing in this app that undo cannot bring back**, and the strip
/// says so in those words. Everywhere else the answer to "are you sure" is to
/// let somebody find out and press Ctrl Z; here there is no Ctrl Z, so the
/// question gets asked and the reason it is being asked gets said with it.
///
/// At the foot of the panel rather than in the row it is about — where it used
/// to be — for two reasons. There is room to *name the board*, which is the
/// fact somebody is actually checking, and a list of forty rows arrowed
/// through at speed should not have one row that is a different height and
/// holds two buttons.
///
/// Both answers are buttons and only the destructive one fills, in the colour
/// the app uses for damage. Keep is also what Escape does and what every arrow
/// key does — see `Switcher::key` — so the safe answer has four ways to give it
/// and the other has two.
fn confirm(cx: &mut Context<BoardView>, theme: crate::theme::Theme, board: &Path) -> AnyElement {
    let name = board.file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    div()
        .flex()
        .items_center()
        .gap(px(9.0))
        .px(px(15.0))
        .py(px(11.0))
        .border_t_1()
        .border_color(theme.chrome_edge)
        .bg(theme.rope_danger.opacity(0.10))
        .child(icon(Icon::Warned, crate::icons::ICON_MD, theme.rope_danger))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_baseline()
                .gap(px(4.0))
                .text_size(px(12.0))
                .text_color(theme.text)
                .child("Delete")
                // The name carries the weight, because it is the one word in
                // the sentence somebody has to check before answering.
                .child(div().min_w_0().truncate().font_weight(FontWeight::SEMIBOLD).child(name))
                .child(
                    div()
                        .flex_none()
                        .text_color(theme.muted)
                        .child("\u{2014} this is the one thing undo can't bring back."),
                ),
        )
        .child(
            div()
                .id("delete-no")
                .flex_none()
                .px(px(10.0))
                .py(px(4.0))
                .rounded(px(crate::theme::RADIUS_XS))
                .border_1()
                .border_color(theme.chrome_edge)
                .text_size(px(11.5))
                .text_color(theme.text)
                .hover(|s| s.bg(theme.text.opacity(0.08)))
                .active(|s| s.bg(theme.text.opacity(0.14)))
                .child("Keep")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        cx.stop_propagation();
                        this.ask_about_board(None);
                        cx.notify();
                    }),
                ),
        )
        .child(
            div()
                .id("delete-yes")
                .flex_none()
                .px(px(10.0))
                .py(px(4.0))
                .rounded(px(crate::theme::RADIUS_XS))
                .text_size(px(11.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.ground)
                .bg(theme.rope_danger)
                .hover(|s| s.bg(theme.rope_danger.opacity(0.85)))
                .active(|s| s.bg(theme.rope_danger.opacity(0.7)))
                .child("Delete")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event, _window, cx| {
                        cx.stop_propagation();
                        this.delete_doomed_board(cx);
                        cx.notify();
                    }),
                ),
        )
        .into_any_element()
}

/// The way to make a board, on the surface for choosing one.
///
/// Here rather than only in the command list because this is the one place in
/// the app that is *about* boards rather than about what is on one — somebody
/// who opens this and finds none of theirs is somebody who wants a new one, and
/// making them close it and go looking for a command would be the wrong answer
/// to the question they just asked.
///
/// It runs [`Command::NewBoard`] rather than reaching for the view directly, so
/// there is one description of what making a board means and `Ctrl N` and the
/// palette get the same one.
fn new_board_button(cx: &mut Context<BoardView>, theme: crate::theme::Theme) -> impl IntoElement {
    div()
        .id("new-board")
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(28.0))
        .h(px(28.0))
        .rounded(px(crate::theme::RADIUS_XS))
        .hover(|s| s.bg(theme.accent.opacity(0.16)))
        .active(|s| s.bg(theme.accent.opacity(0.32)))
        .tooltip(tip(theme, Command::NewBoard.label(), Command::NewBoard.hint()))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _event, window, cx| {
                // Closed first, and before the board is made: the switcher
                // lists what is on disk as of the moment it opened, and one
                // left standing over a board that has just been created is a
                // list that does not have it on it.
                this.close_switcher();
                Command::NewBoard.run(this, window, cx);
            }),
        )
        .child(icon(Icon::New, crate::icons::ICON_MD, theme.muted))
}

/// A directory, with the home part written the way people write it.
fn shorten(dir: &Path) -> String {
    let text = dir.to_string_lossy().to_string();
    match std::env::var_os("HOME") {
        Some(home) if !home.is_empty() => {
            let home = home.to_string_lossy().to_string();
            match text.strip_prefix(&home) {
                Some(rest) => format!("~{rest}"),
                None => text,
            }
        }
        _ => text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every path remembered, none found. The split is exercised by
    /// [`beside`] below.
    fn switcher(paths: &[&str]) -> Switcher {
        beside(paths, &[])
    }

    /// `remembered` came out of `recent.json`; `found` was scanned up beside
    /// the open board. See [`Switcher::recent`].
    fn beside(remembered: &[&str], found: &[&str]) -> Switcher {
        let boards: Vec<PathBuf> = remembered.iter().chain(found).map(PathBuf::from).collect();
        Switcher {
            query: Editor::new("", crate::palette::QUERY_MAX, false),
            cursor: 0,
            scroll: ScrollHandle::new(),
            recent: remembered.len(),
            boards,
            confirming: None,
            refused: None,
            open: None,
        }
    }

    #[test]
    fn a_found_board_sits_below_every_remembered_one_however_well_it_matches() {
        // The found board is the better match by every measure the scorer
        // has — the letters start its name rather than sitting in the middle
        // of a longer one — and it still comes second, because the heading
        // over the found group says what those rows *are* and a heading is a
        // lie the moment a row above it belongs below.
        let mut s = beside(&["/b/old-kit-notes.mbrd"], &["/b/kit.mbrd"]);
        s.query = typed("kit");
        assert_eq!(names(&s), ["old-kit-notes.mbrd", "kit.mbrd"]);
        assert!(
            score("kit", Path::new("/b/kit.mbrd"))
                > score("kit", Path::new("/b/old-kit-notes.mbrd")),
            "and it really is the better match"
        );
    }

    #[test]
    fn inside_a_group_the_better_match_still_wins() {
        let mut s = beside(&["/b/studio.mbrd", "/b/kitchen.mbrd"], &[]);
        s.query = typed("kitchen");
        assert_eq!(names(&s), ["kitchen.mbrd"]);
    }

    #[test]
    fn forgetting_a_remembered_board_does_not_move_the_heading() {
        // The retain shifts every index after it. Without the matching
        // adjustment to `recent`, the last remembered board would start
        // claiming to have been found.
        let mut s = beside(&["/b/one.mbrd", "/b/two.mbrd"], &["/b/three.mbrd"]);
        s.dropped(Path::new("/b/one.mbrd"));
        assert!(!s.found_beside(Path::new("/b/two.mbrd")), "still remembered");
        assert!(s.found_beside(Path::new("/b/three.mbrd")), "still found");
    }

    #[test]
    fn forgetting_a_found_board_leaves_the_heading_where_it_was() {
        let mut s = beside(&["/b/one.mbrd"], &["/b/two.mbrd", "/b/three.mbrd"]);
        s.dropped(Path::new("/b/two.mbrd"));
        assert!(!s.found_beside(Path::new("/b/one.mbrd")));
        assert!(s.found_beside(Path::new("/b/three.mbrd")));
    }

    #[test]
    fn a_board_that_is_both_remembered_and_beside_is_only_remembered() {
        // `extend_boards` skips what is already in the list, so the scan
        // cannot demote a board somebody has actually had open.
        let mut s = beside(&["/b/one.mbrd"], &[]);
        s.extend_boards(vec![PathBuf::from("/b/one.mbrd"), PathBuf::from("/b/two.mbrd")]);
        assert!(!s.found_beside(Path::new("/b/one.mbrd")));
        assert!(s.found_beside(Path::new("/b/two.mbrd")));
        assert_eq!(names(&s), ["one.mbrd", "two.mbrd"]);
    }

    fn typed(text: &str) -> Editor {
        Editor::new(text, crate::palette::QUERY_MAX, false)
    }

    fn names(s: &Switcher) -> Vec<String> {
        s.matches()
            .into_iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().to_string())
            .collect()
    }

    #[test]
    fn an_empty_query_offers_the_most_recent_first() {
        let s = switcher(&["/a/one.mbrd", "/b/two.mbrd"]);
        assert_eq!(names(&s), ["one.mbrd", "two.mbrd"]);
    }

    #[test]
    fn typing_narrows_to_what_the_letters_are_in() {
        let mut s = switcher(&["/a/kitchen.mbrd", "/a/shelf.mbrd", "/a/sketches.mbrd"]);
        s.query = typed("sh");
        // `sketches` genuinely contains an s then an h, so it is a match — but
        // a scattered one, and it sorts below the name that starts with the
        // letters typed. Dropping it would be wrong: the whole point of typing
        // into a list is that you can be approximate.
        assert_eq!(names(&s), ["shelf.mbrd", "sketches.mbrd"]);
        s.query = typed("shelf");
        assert_eq!(names(&s), ["shelf.mbrd"]);
    }

    #[test]
    fn a_name_beats_a_folder_that_merely_contains_the_letters() {
        let mut s = switcher(&["/kitchen-drafts/other.mbrd", "/a/kitchen.mbrd"]);
        s.query = typed("kitchen");
        assert_eq!(names(&s)[0], "kitchen.mbrd");
    }

    #[test]
    fn letters_in_a_row_beat_letters_scattered_about() {
        let mut s = switcher(&["/a/k-i-t-c-h.mbrd", "/a/kitchen.mbrd"]);
        s.query = typed("kitch");
        assert_eq!(names(&s)[0], "kitchen.mbrd");
    }

    #[test]
    fn a_query_that_matches_nothing_offers_nothing() {
        let mut s = switcher(&["/a/one.mbrd"]);
        s.query = typed("zzz");
        assert!(s.matches().is_empty());
        assert!(s.chosen().is_none());
    }

    #[test]
    fn the_highlight_stops_at_both_ends_rather_than_wrapping() {
        let mut s = switcher(&["/a/one.mbrd", "/a/two.mbrd"]);
        s.step(-1);
        assert_eq!(s.cursor, 0);
        s.step(1);
        s.step(1);
        s.step(1);
        assert_eq!(s.cursor, 1);
    }

    #[test]
    fn typing_puts_the_highlight_back_at_the_top() {
        // Otherwise a query that narrows the list leaves the highlight pointing
        // at whatever happens to be in that position now.
        let mut s = switcher(&["/a/one.mbrd", "/a/two.mbrd", "/a/three.mbrd"]);
        s.cursor = 2;
        s.key("t", Modifiers::default(), Some("t"));
        assert_eq!(s.cursor, 0);
    }

    #[test]
    fn a_shortcut_typed_into_the_query_is_not_typed_into_the_query() {
        let mut s = switcher(&["/a/one.mbrd"]);
        s.key("s", Modifiers::secondary_key(), Some("s"));
        assert_eq!(s.query.text(), "");
    }

    #[test]
    fn escape_and_enter_say_what_they_are_rather_than_being_typed() {
        let mut s = switcher(&["/a/one.mbrd"]);
        assert_eq!(s.key("escape", Modifiers::default(), None), Reply::Close);
        assert_eq!(s.key("enter", Modifiers::default(), None), Reply::Open);
        assert_eq!(s.query.text(), "");
    }

    /// Press the key that arms a delete, and answer nothing yet.
    fn press(s: &mut Switcher, key: &str) -> Reply {
        s.key(key, Modifiers::default(), None)
    }

    #[test]
    fn enter_opens_a_board_until_a_delete_has_been_asked_about() {
        // The one key that means two things here, and the whole reason the
        // question is a piece of state rather than a dialogue: which of the two
        // it means has to be visible on screen before it is pressed.
        let mut s = switcher(&["/a/one.mbrd", "/b/two.mbrd"]);
        assert_eq!(press(&mut s, "enter"), Reply::Open);
        assert_eq!(press(&mut s, "delete"), Reply::Held, "arming is not doing");
        assert_eq!(s.doomed().as_deref(), Some(Path::new("/a/one.mbrd")));
        assert_eq!(press(&mut s, "enter"), Reply::Delete);
    }

    #[test]
    fn escape_takes_the_question_back_before_it_closes_the_list() {
        let mut s = switcher(&["/a/one.mbrd"]);
        press(&mut s, "delete");
        assert_eq!(press(&mut s, "escape"), Reply::Held, "it closed instead of answering");
        assert_eq!(s.doomed(), None);
        assert_eq!(press(&mut s, "escape"), Reply::Close);
    }

    #[test]
    fn the_question_is_about_a_row_and_does_not_follow_the_highlight() {
        // Otherwise arming a delete, arrowing down and pressing enter deletes
        // a board nobody was looking at when they answered.
        let mut s = switcher(&["/a/one.mbrd", "/b/two.mbrd"]);
        press(&mut s, "delete");
        press(&mut s, "down");
        assert_eq!(s.doomed(), None, "the question moved with the highlight");
        assert_eq!(press(&mut s, "enter"), Reply::Open);
    }

    #[test]
    fn typing_takes_the_question_back_too() {
        // The list under the question is about to be a different list.
        let mut s = switcher(&["/a/one.mbrd", "/b/two.mbrd"]);
        press(&mut s, "delete");
        s.key("t", Modifiers::default(), Some("t"));
        assert_eq!(s.doomed(), None);
    }

    #[test]
    fn the_board_that_is_open_is_never_offered_for_deletion() {
        // Deleting it would leave the view holding a path that is not there,
        // and the next autosave would write the file straight back. See
        // `Switcher::open`.
        let mut s = switcher(&["/a/one.mbrd", "/b/two.mbrd"]);
        s.open = Some(PathBuf::from("/a/one.mbrd"));
        assert!(!s.deletable(Path::new("/a/one.mbrd")));
        assert!(s.deletable(Path::new("/b/two.mbrd")));
        press(&mut s, "delete");
        assert_eq!(s.doomed(), None, "the open board was armed for deletion");
        assert_eq!(press(&mut s, "enter"), Reply::Open, "and enter still opens it");
    }

    #[test]
    fn a_deleted_board_leaves_the_list_it_was_chosen_from() {
        // The list is gathered once, when it opens, so nothing else is going to
        // notice the file has gone — and a row left behind is a row that opens
        // a board that is not there.
        let mut s = switcher(&["/a/one.mbrd", "/b/two.mbrd"]);
        press(&mut s, "down");
        press(&mut s, "delete");
        s.dropped(Path::new("/b/two.mbrd"));
        assert_eq!(names(&s), ["one.mbrd"]);
        assert_eq!(s.doomed(), None, "the question outlived its board");
        assert_eq!(s.chosen().as_deref(), Some(Path::new("/a/one.mbrd")), "the cursor ran off");
    }
}
