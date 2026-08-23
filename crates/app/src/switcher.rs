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
//! The typing is handled by hand rather than by a text field, because GPUI
//! ships neither one. What is here is what a one-line query needs and nothing
//! more: characters, backspace, and a caret that is always at the end.

use std::path::{Path, PathBuf};

use gpui::{div, prelude::*, px, Context, Modifiers, MouseButton};

use crate::board_view::BoardView;

/// How many matches to show. Past this the list is longer than the answer.
const SHOWN: usize = 12;

/// An open switcher: what has been typed, and where the highlight is.
#[derive(Debug, Clone, Default)]
pub struct Switcher {
    pub query: String,
    /// Which of the *matches* is highlighted, not which of the boards.
    pub cursor: usize,
    boards: Vec<PathBuf>,
}

impl Switcher {
    /// Gather the candidates. Done once, when it opens, rather than per
    /// keystroke: the disk does not change while somebody is typing, and
    /// re-reading two directories on every letter is a stutter you can feel.
    pub fn open(current: Option<&Path>) -> Self {
        let mut boards = crate::recent::load();

        let mut add = |extra: Vec<PathBuf>| {
            for path in extra {
                let path = path.canonicalize().unwrap_or(path);
                if !boards_contains(&boards, &path) {
                    boards.push(path);
                }
            }
        };
        if let Some(dir) = current.and_then(Path::parent) {
            add(crate::recent::beside(dir));
        }
        if let Ok(here) = std::env::current_dir() {
            add(crate::recent::beside(&here));
        }

        Self { query: String::new(), cursor: 0, boards }
    }

    /// The boards worth showing, best first.
    pub fn matches(&self) -> Vec<&Path> {
        if self.query.is_empty() {
            return self.boards.iter().take(SHOWN).map(PathBuf::as_path).collect();
        }
        let mut scored: Vec<(i32, usize, &Path)> = self
            .boards
            .iter()
            .enumerate()
            .filter_map(|(i, p)| score(&self.query, p).map(|s| (s, i, p.as_path())))
            .collect();
        // Best score first, and where two score the same the more recent one —
        // which is the order `boards` is already in.
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.into_iter().take(SHOWN).map(|(_, _, p)| p).collect()
    }

    /// What Enter would open.
    pub fn chosen(&self) -> Option<PathBuf> {
        self.matches().get(self.cursor).map(|p| p.to_path_buf())
    }

    /// Take a key press. Answers what the view should do about it.
    pub fn key(&mut self, key: &str, mods: Modifiers, text: Option<&str>) -> Reply {
        match key {
            "escape" => return Reply::Close,
            "enter" => return Reply::Open,
            "up" => self.step(-1),
            "down" => self.step(1),
            "backspace" => {
                self.query.pop();
                self.cursor = 0;
            }
            _ => {
                // The text the platform produced, not the key that produced it,
                // so that a keyboard laid out for another language types what
                // is on its keycaps. Modified presses are somebody reaching for
                // a shortcut rather than typing, and are left alone.
                let Some(text) = text.filter(|t| !t.is_empty()) else { return Reply::Held };
                if mods.control || mods.alt || mods.platform {
                    return Reply::Held;
                }
                self.query.push_str(text);
                self.cursor = 0;
            }
        }
        Reply::Held
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
}

fn boards_contains(boards: &[PathBuf], path: &Path) -> bool {
    boards.iter().any(|p| p == path)
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
    if let Some(points) = subsequence(&query, &name) {
        return Some(points + 1_000);
    }
    subsequence(&query, &whole)
}

/// Points for a subsequence match, or `None` if the letters are not all there.
fn subsequence(needle: &str, haystack: &str) -> Option<i32> {
    let mut points = 0;
    let mut run = 0;
    let mut chars = haystack.char_indices().peekable();

    for want in needle.chars() {
        loop {
            let (i, got) = chars.next()?;
            if got == want {
                points += 1;
                // A letter following the one before it, or starting a word, is
                // what somebody was actually aiming at.
                let starts_word = i == 0
                    || haystack[..i].chars().next_back().is_some_and(|c| !c.is_alphanumeric());
                points += run * 2 + i32::from(starts_word) * 3;
                run += 1;
                break;
            }
            run = 0;
        }
    }
    // Shorter answers, all else equal: an exact name should beat one that
    // merely contains it.
    Some(points - (haystack.chars().count() as i32) / 8)
}

pub fn render(
    switcher: &Switcher,
    view: &BoardView,
    cx: &mut Context<BoardView>,
) -> impl IntoElement {
    let theme = view.theme;
    let matches = switcher.matches();

    let rows: Vec<_> = matches
        .iter()
        .enumerate()
        .map(|(i, path)| {
            let highlighted = i == switcher.cursor;
            let name =
                path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let where_ = path.parent().map(shorten).unwrap_or_default();
            let target = path.to_path_buf();
            div()
                .id(i)
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.0))
                .px(px(12.0))
                .py(px(7.0))
                .mx(px(6.0))
                .rounded(px(5.0))
                .when(highlighted, |d| d.bg(theme.accent.opacity(0.20)))
                .hover(|s| s.bg(theme.accent.opacity(0.12)))
                // Opening a board is the slowest thing in the app — a file to
                // read and a board to build — so the row has to say it was
                // pressed before any of that starts, or the press looks lost.
                .active(|s| s.bg(theme.accent.opacity(0.3)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event, _window, cx| {
                        this.close_switcher();
                        this.open_board(&target, cx);
                    }),
                )
                .child(div().text_size(px(13.0)).child(name))
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme.muted)
                        .overflow_hidden()
                        .child(where_),
                )
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
        // Not centred vertically: a list that grows downward from a fixed point
        // does not move the thing you are aiming at as you type.
        .pt(px(96.0))
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
        .child(
            div()
                .w(px(560.0))
                .max_h(px(440.0))
                .flex()
                .flex_col()
                .rounded(px(10.0))
                .bg(theme.chrome)
                .border_1()
                .border_color(theme.chrome_edge)
                .shadow_lg()
                .text_color(theme.text)
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .px(px(14.0))
                        .py(px(11.0))
                        .border_b_1()
                        .border_color(theme.chrome_edge)
                        .text_size(px(14.0))
                        .child(if switcher.query.is_empty() {
                            div()
                                .text_color(theme.muted)
                                .child("open a board\u{2026}")
                                .into_any_element()
                        } else {
                            div().child(format!("{}\u{2502}", switcher.query)).into_any_element()
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .py(px(6.0))
                        .when(rows.is_empty(), |d| {
                            d.px(px(14.0))
                                .py(px(10.0))
                                .text_size(px(12.0))
                                .text_color(theme.muted)
                                .child(if switcher.query.is_empty() {
                                    "no boards yet \u{2014} save one and it will be here"
                                } else {
                                    "no board by that name"
                                })
                        })
                        .children(rows),
                ),
        )
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

    fn switcher(paths: &[&str]) -> Switcher {
        Switcher {
            query: String::new(),
            cursor: 0,
            boards: paths.iter().map(PathBuf::from).collect(),
        }
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
        s.query = "sh".into();
        // `sketches` genuinely contains an s then an h, so it is a match — but
        // a scattered one, and it sorts below the name that starts with the
        // letters typed. Dropping it would be wrong: the whole point of typing
        // into a list is that you can be approximate.
        assert_eq!(names(&s), ["shelf.mbrd", "sketches.mbrd"]);
        s.query = "shelf".into();
        assert_eq!(names(&s), ["shelf.mbrd"]);
    }

    #[test]
    fn a_name_beats_a_folder_that_merely_contains_the_letters() {
        let mut s = switcher(&["/kitchen-drafts/other.mbrd", "/a/kitchen.mbrd"]);
        s.query = "kitchen".into();
        assert_eq!(names(&s)[0], "kitchen.mbrd");
    }

    #[test]
    fn letters_in_a_row_beat_letters_scattered_about() {
        let mut s = switcher(&["/a/k-i-t-c-h.mbrd", "/a/kitchen.mbrd"]);
        s.query = "kitch".into();
        assert_eq!(names(&s)[0], "kitchen.mbrd");
    }

    #[test]
    fn a_query_that_matches_nothing_offers_nothing() {
        let mut s = switcher(&["/a/one.mbrd"]);
        s.query = "zzz".into();
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
        assert_eq!(s.query, "");
    }

    #[test]
    fn escape_and_enter_say_what_they_are_rather_than_being_typed() {
        let mut s = switcher(&["/a/one.mbrd"]);
        assert_eq!(s.key("escape", Modifiers::default(), None), Reply::Close);
        assert_eq!(s.key("enter", Modifiers::default(), None), Reply::Open);
        assert_eq!(s.query, "");
    }
}
