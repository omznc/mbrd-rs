//! The inventory sheet: what this board is made of, and what it weighs.
//!
//! The drawing half. Every number on the page comes from
//! [`mbrd_core::inventory`], which is pure and tested without a window — see
//! its own note for the two rules the report may not break, both of which are
//! about not making a heavy board heavier to ask about.
//!
//! ## A page, not a panel
//!
//! The same surface `settings.rs` and `opened.rs` use, and for the same reason
//! they do: this is something you go and look at rather than something you
//! reach past. It is not a section of the settings page, because every row
//! there is a control with a value and nothing here has one.
//!
//! ## Every row that can be is a way to the card
//!
//! A row is a *file*, and a file can be under several cards or under none. One
//! with a card behind it travels there — the board has no edges, so being told
//! that the 12 MB is "beach.jpg" would be no use if you then had to go and find
//! it. One with no card behind it is not a button at all: an orphan has nothing
//! to show, and a control that did nothing would be the page offering to take
//! somebody somewhere that is not there.
//!
//! ## A table, because the questions are comparisons
//!
//! Every question this page is opened with is a *ranking* — what is biggest,
//! what is used least, what all these `.heic` files are. A column of
//! `name — value` lines answers none of them without arithmetic done in
//! somebody's head, so the rows are columns: name, kind, size, how many cards
//! use it, and a bar for its share of the whole. The bar is the one that turns
//! the list into an answer, because it says *how much of this board is this
//! file* without anybody dividing anything.
//!
//! The filter and the four sorts are the same argument continued. They are
//! also why the report holds every file rather than the ten heaviest — see
//! [`mbrd_core::inventory::Inventory::files`].
//!
//! ## And one thing you can do about it
//!
//! The shrink lives at the bottom of this page and nowhere else, which is the
//! whole reason the two shipped together. "Make this board smaller" as a menu
//! row is a button somebody has to already trust; the same button under a list
//! that has just said *this photograph is 12 MB* is one that has given its
//! reason first. That is also how it keeps its promise to say what it will do
//! before it does it — the page above it **is** the saying.

use gpui::{
    div, prelude::*, px, AnyElement, Context, FontWeight, Modifiers, MouseButton, ScrollHandle,
    SharedString,
};

use mbrd_core::inventory::{self, Inventory, Weighed};
use mbrd_core::ItemType;

use crate::board_view::BoardView;
use crate::editor::{self, Editor};
use crate::icons::{icon, Icon, ICON_SM};
use crate::theme::Theme;

/// How long a filter may get. Longer than any file name worth typing all of.
const FILTER_MAX: usize = 64;

/// How wide the share bar's track is.
const SHARE_TRACK: f32 = 96.0;

/// Which way the rows are ordered.
///
/// Four, because they are the four questions the page gets opened with: what
/// is heaviest, where is that file called something, what are all these
/// `.heic`s, and what is nothing using. Nothing here is a *reverse* of another
/// — each already runs the way somebody asking that question wants it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Sort {
    /// Heaviest first, which is what the page is nearly always opened to find.
    #[default]
    Size,
    /// Alphabetical, for looking one up rather than ranking them.
    Name,
    /// Grouped by what the format is called, and by weight inside a group.
    Kind,
    /// Least-used first, so the orphans and the once-used rise to the top.
    Uses,
}

impl Sort {
    pub const ALL: [Sort; 4] = [Sort::Size, Sort::Name, Sort::Kind, Sort::Uses];

    pub fn label(self) -> &'static str {
        match self {
            Sort::Size => "Size",
            Sort::Name => "Name",
            Sort::Kind => "Kind",
            Sort::Uses => "Uses",
        }
    }

    fn at(self) -> usize {
        Self::ALL.iter().position(|&s| s == self).unwrap_or(0)
    }
}

/// What one press on the sheet means to the view around it.
///
/// Its own three rather than `settings::Reply`, which carries a theme picker
/// this page has no business owning: sharing a vocabulary is worth it where
/// the controls are shared, and none of them are here.
pub enum Reply {
    /// Dealt with. Nothing for the view to do but repaint.
    Held,
    /// Put the sheet away.
    Close,
    /// The clipboard is the view's, not the page's.
    Paste,
}

/// The open sheet.
///
/// Carries no numbers. Everything shown is read off the board each frame, the
/// same bargain `settings::Page` makes — a report held in a struct would be a
/// report that went stale the moment somebody undid something behind it. What
/// it does carry is how the reader has asked to *look* at those numbers, which
/// is not a fact about the board and would be wrong to recompute.
#[derive(Debug, Clone)]
pub struct Sheet {
    /// Where the list is scrolled to. A handle, so the clone the painter gets
    /// is the same position the wheel moved.
    pub scroll: ScrollHandle,
    /// What has been typed into the filter.
    pub filter: Editor,
    /// Whether the caret is drawn. See `settings::Page::focused` for the rule:
    /// a press the field did something with is somebody using the field.
    pub focused: bool,
    pub sort: Sort,
}

impl Sheet {
    pub fn open() -> Self {
        Self {
            scroll: ScrollHandle::new(),
            filter: Editor::new("", FILTER_MAX, false),
            focused: false,
            sort: Sort::default(),
        }
    }

    /// Whether the list below is a filtered one.
    pub fn filtering(&self) -> bool {
        !self.filter.text().trim().is_empty()
    }

    /// One key press. The same shape `settings::Page::key` has, and the same
    /// rule about Escape: it clears the filter before it closes the page,
    /// because somebody who typed something and wants the whole list back
    /// should not have to reopen it. The two never collide — an empty field
    /// has nothing to clear.
    pub fn key(&mut self, key: &str, mods: Modifiers, text: Option<&str>) -> Reply {
        if key == "escape" {
            if self.filtering() {
                self.filter = Editor::new("", FILTER_MAX, false);
                self.focused = false;
                return Reply::Held;
            }
            return Reply::Close;
        }
        let reply = self.filter.key(key, editor::Mods::from(mods), text);
        if reply != editor::Reply::Ignored {
            self.focused = true;
        }
        if reply == editor::Reply::Ignored && mods.secondary() && key == "v" {
            return Reply::Paste;
        }
        Reply::Held
    }

    pub fn insert(&mut self, text: &str) {
        self.filter.insert(text);
        self.focused = true;
    }
}

pub fn render(sheet: &Sheet, view: &BoardView, cx: &mut Context<BoardView>) -> impl IntoElement {
    let theme = view.theme;
    let arriving = crate::board_view::arrival(view.overlay_presence.value());
    let report = inventory::of(&view.doc);
    let rows = shown(&report, sheet);
    // The denominator every share bar divides by. The heaviest file's own
    // size, not the board's total: a board of three hundred equal photographs
    // would otherwise draw three hundred bars of one pixel each, and the
    // column would be saying nothing at all. Against the heaviest, the top row
    // is always full and every other row is *relative to the worst offender*,
    // which is the comparison somebody opened this page to make.
    let heaviest = report.files.iter().map(|f| f.bytes).max().unwrap_or(0);

    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        .flex()
        .justify_center()
        .bg(theme.ground.opacity(arriving.ground))
        .text_color(theme.text)
        // The board underneath still exists, and a press that fell through
        // would land on a card nobody can see. Same guard the settings page
        // puts up, for the same reason.
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .w_full()
                .max_w(px(1080.0))
                .h_full()
                .flex()
                .flex_col()
                .opacity(arriving.content)
                .mt(px(arriving.rise))
                .child(heading(&report, view, cx))
                .child(controls(sheet, view, cx))
                .child(columns(theme))
                .child(
                    div()
                        .id("inventory")
                        .track_scroll(&sheet.scroll)
                        .flex_1()
                        .min_h_0()
                        .flex()
                        .flex_col()
                        .overflow_y_scroll()
                        .when(rows.is_empty(), |d| d.child(nothing(sheet, theme)))
                        .children(
                            rows.into_iter()
                                .enumerate()
                                .map(|(i, file)| row(i, file, heaviest, view, theme, cx)),
                        ),
                )
                .child(footer(&report, view, theme, cx)),
        )
}

/// The rows, filtered and ordered the way the sheet has been asked for.
///
/// Split out and taking only what it reads, so the whole of "which rows and in
/// what order" is one function that could be tested without a window if the
/// pieces it leans on were not already.
fn shown<'a>(report: &'a Inventory, sheet: &Sheet) -> Vec<&'a Weighed> {
    let needle = sheet.filter.text().trim().to_lowercase();
    let mut rows: Vec<&Weighed> = report
        .files
        .iter()
        // Name *or* kind, because "heic" and "IMG_4471" are both things
        // somebody types into a box over a list of files, and neither of them
        // is obviously the one this field means.
        .filter(|file| {
            needle.is_empty()
                || naming(file).to_lowercase().contains(&needle)
                || described(file).to_lowercase().contains(&needle)
        })
        .collect();

    // Already heaviest-first out of the report, so `Size` sorts nothing and
    // every other order breaks its ties by falling back to it.
    match sheet.sort {
        Sort::Size => {}
        Sort::Name => rows.sort_by_key(|file| naming(file).to_lowercase()),
        Sort::Kind => rows.sort_by_key(|file| described(file).to_lowercase()),
        // Least first: this order exists to raise what nothing is using, and
        // an orphan at nought cards is exactly the row it is for.
        Sort::Uses => rows.sort_by_key(|file| file.cards.len()),
    }
    rows
}

/// The summary. Four facts, and the biggest of them is the one somebody came
/// for — so it is set in the size a headline is set in and the sentence under
/// the title is not.
fn heading(report: &Inventory, view: &BoardView, cx: &mut Context<BoardView>) -> AnyElement {
    let theme = view.theme;
    div()
        .flex_none()
        .flex()
        .items_end()
        .gap(px(28.0))
        .px(px(20.0))
        .pt(px(20.0))
        .pb(px(14.0))
        .border_b_1()
        .border_color(theme.chrome_edge)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(
                    div().text_size(px(16.0)).font_weight(FontWeight::SEMIBOLD).child("Inventory"),
                )
                // "By content hash" is not jargon for its own sake: it is the
                // reason the two numbers differ, and without it a board with
                // 184 files and 312 cards looks like a board that has lost
                // 128 files.
                .child(div().text_size(px(12.5)).text_color(theme.muted).child(format!(
                    "{} by content hash \u{b7} {} point at {}",
                    plural(report.assets, "file"),
                    plural(report.uses, "card"),
                    match report.assets {
                        1 => "it",
                        _ => "them",
                    },
                )))
                // Everything the table cannot say, because it is not about
                // files: what the cards are, what joins them, what is in the
                // bin, and how long the history is. All of it in one strip
                // rather than four rows — these are read together or not at
                // all, and any of them being nought is a fact worth no space,
                // which is the same rule the status bar's facts follow.
                .child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_size(px(11.5))
                        .text_color(theme.tertiary)
                        .child(facts(report)),
                ),
        )
        .child(stat(inventory::size(report.bytes), "on disk", theme.text, theme))
        // Both of these are nought on most boards, and a nought here would be
        // a number somebody has to read before deciding it says nothing.
        .when(report.shared > 0, |d| {
            d.child(stat(
                inventory::size(report.shared),
                "saved by dedupe",
                theme.accent_text,
                theme,
            ))
        })
        .when(report.orphans.count > 0, |d| {
            d.child(stat(report.orphans.count.to_string(), "orphans", theme.rope_danger, theme))
        })
        .child(
            div()
                .id("inventory-close")
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .size(px(27.0))
                .rounded(px(crate::theme::RADIUS_SM))
                .hover(|s| s.bg(theme.accent.opacity(0.14)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.close_stock();
                        cx.notify();
                        cx.stop_propagation();
                    }),
                )
                .child(icon(Icon::Close, ICON_SM, theme.muted)),
        )
        .into_any_element()
}

/// One of the headline numbers, over the word for what it counts.
fn stat(value: String, name: &'static str, tint: gpui::Hsla, theme: Theme) -> AnyElement {
    div()
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .items_end()
        .child(div().font(crate::opened::mono()).text_size(px(18.0)).text_color(tint).child(value))
        .child(div().text_size(px(11.0)).text_color(theme.muted).child(name))
        .into_any_element()
}

/// The filter and the four orders.
fn controls(sheet: &Sheet, view: &BoardView, cx: &mut Context<BoardView>) -> AnyElement {
    let theme = view.theme;
    let empty = sheet.filter.text().is_empty();
    let labels: Vec<String> = Sort::ALL.iter().map(|s| s.label().to_string()).collect();
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(14.0))
        .px(px(20.0))
        .py(px(9.0))
        .border_b_1()
        .border_color(theme.chrome_edge)
        .child(
            div()
                .id("inventory-filter")
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(7.0))
                .px(px(9.0))
                .py(px(5.0))
                .rounded(px(crate::theme::RADIUS_SM))
                .bg(theme.chrome)
                .border_1()
                // Lit while it holds something, so the list below is never
                // quietly shorter than the board is.
                .border_color(if empty { theme.chrome_edge } else { theme.accent })
                .cursor_text()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.focus_stock_filter(cx);
                        cx.stop_propagation();
                    }),
                )
                .child(icon(Icon::Search, ICON_SM, theme.tertiary))
                .child(div().flex_1().min_w_0().text_size(px(12.0)).child(
                    crate::palette::query_line(
                        &sheet.filter,
                        "Filter by name or kind",
                        12.0,
                        sheet.focused,
                        &theme,
                    ),
                ))
                .when(!empty, |d| {
                    d.child(
                        div()
                            .id("inventory-filter-clear")
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
                                    this.clear_stock_filter(cx);
                                    cx.stop_propagation();
                                }),
                            )
                            .child(icon(Icon::Close, ICON_SM, theme.tertiary)),
                    )
                }),
        )
        .child(div().flex_none().child(crate::settings::segmented(
            "inventory-sort",
            &labels,
            Some(sheet.sort.at()),
            |this, at, cx| this.set_stock_sort(Sort::ALL[at], cx),
            view,
            cx,
        )))
        .into_any_element()
}

/// The column names. Set small, in the same face as the numbers under them,
/// because their job is to be read once and then be a ruler.
fn columns(theme: Theme) -> AnyElement {
    fn head(width: Option<f32>, right: bool, word: &'static str) -> gpui::Div {
        let cell = match width {
            Some(w) => div().flex_none().w(px(w)),
            None => div().flex_1().min_w_0(),
        };
        cell.when(right, |d| d.text_right()).child(word.to_uppercase())
    }
    div()
        .flex_none()
        .flex()
        .items_center()
        .h(px(24.0))
        .px(px(20.0))
        .font(crate::opened::mono())
        .text_size(px(9.5))
        .text_color(theme.tertiary)
        .border_b_1()
        .border_color(theme.chrome_edge)
        .child(head(None, false, "Name"))
        .child(head(Some(190.0), false, "Kind"))
        .child(head(Some(96.0), true, "Size"))
        .child(head(Some(110.0), true, "Used by"))
        .child(head(Some(SHARE_TRACK + 24.0), true, "Share"))
        .into_any_element()
}

/// One file.
///
/// Pressable where there is a card to go to, and a plain line where there is
/// not — see the module note. The arrow is what says which of the two this
/// one is before anybody tries it.
fn row(
    i: usize,
    file: &Weighed,
    heaviest: usize,
    view: &BoardView,
    theme: Theme,
    cx: &mut Context<BoardView>,
) -> AnyElement {
    let first = file.cards.first().cloned();
    let kind =
        first.as_deref().and_then(|id| view.doc.board.item(id)).map(|item| item.kind.clone());
    let mark = match file.orphan {
        true => Icon::Orphan,
        false => kind.as_ref().map_or(Icon::Unknown, Icon::for_kind),
    };
    let tint = match file.orphan {
        true => theme.tertiary,
        false => kind.as_ref().map_or(theme.muted, |k| shelf(k, theme)),
    };
    let used = match file.cards.len() {
        0 => "no card".to_string(),
        n => plural(n, "card"),
    };
    let share = match heaviest {
        0 => 0.0,
        heaviest => (file.bytes as f32 / heaviest as f32).clamp(0.0, 1.0),
    };

    div()
        .id(i)
        .flex()
        .items_center()
        .flex_none()
        .h(px(30.0))
        .px(px(20.0))
        .text_size(px(12.5))
        .border_b_1()
        .border_color(theme.chrome_edge.opacity(0.5))
        // Dimmed as a whole rather than recoloured cell by cell: an orphan is
        // a row that is *less* than the others all the way across, and saying
        // so once is cheaper to read than saying it five times.
        .when(file.orphan, |d| d.opacity(0.55))
        .when_some(first, |d, id| {
            d.hover(|s| s.bg(theme.accent.opacity(0.10)))
                .active(|s| s.bg(theme.accent.opacity(0.20)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event, _window, cx| {
                        // Away first, then travel: the page covers the whole
                        // window, and flying the camera to a card behind it
                        // would be a journey nobody sees the end of.
                        this.close_stock();
                        this.reveal(&id, cx);
                        cx.stop_propagation();
                    }),
                )
        })
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(9.0))
                .child(icon(mark, ICON_SM, tint))
                .child(div().min_w_0().truncate().child(naming(file)))
                .when(!file.orphan && !file.cards.is_empty(), |d| {
                    d.child(icon(Icon::Travel, 10.0, theme.tertiary))
                })
                // Named where the card would be named, because "nothing points
                // at this" is the whole reason its row is interesting.
                .when(file.orphan, |d| d.child(chip("orphan", theme))),
        )
        .child(
            div()
                .flex_none()
                .w(px(190.0))
                .min_w_0()
                .truncate()
                .text_size(px(12.0))
                .text_color(theme.muted)
                .child(described(file)),
        )
        .child(
            div()
                .flex_none()
                .w(px(96.0))
                .text_right()
                .font(crate::opened::mono())
                .text_size(px(11.5))
                .child(inventory::size(file.bytes)),
        )
        .child(
            div()
                .flex_none()
                .w(px(110.0))
                .text_right()
                .text_size(px(12.0))
                .text_color(theme.muted)
                .child(used),
        )
        .child(
            div().flex_none().w(px(SHARE_TRACK + 24.0)).flex().justify_end().child(
                div()
                    .w(px(SHARE_TRACK))
                    .h(px(4.0))
                    .rounded(px(2.0))
                    .bg(theme.chrome_edge)
                    .overflow_hidden()
                    .child(div().h_full().w(gpui::relative(share)).bg(tint)),
            ),
        )
        .into_any_element()
}

/// The little uppercase label an orphan row wears.
fn chip(word: &'static str, theme: Theme) -> AnyElement {
    div()
        .flex_none()
        .px(px(6.0))
        .rounded(px(3.0))
        .border_1()
        .border_color(theme.chrome_edge)
        .font(crate::opened::mono())
        .text_size(px(9.5))
        .text_color(theme.muted)
        .child(word.to_uppercase())
        .into_any_element()
}

/// What stands where the rows would be when there are none.
///
/// Two different sentences, because they are two different situations and only
/// one of them is fixable by pressing Escape.
fn nothing(sheet: &Sheet, theme: Theme) -> AnyElement {
    let words = match sheet.filtering() {
        true => "Nothing here matches that.",
        false => "No files on this board. Notes and links weigh nothing.",
    };
    div()
        .flex()
        .items_center()
        .justify_center()
        .py(px(40.0))
        .text_size(px(12.5))
        .text_color(theme.muted)
        .child(words)
        .into_any_element()
}

/// The strip along the bottom: what the page could not fix, and the one thing
/// it can.
fn footer(
    report: &Inventory,
    view: &BoardView,
    theme: Theme,
    cx: &mut Context<BoardView>,
) -> AnyElement {
    div()
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .px(px(20.0))
        .py(px(9.0))
        .border_t_1()
        .border_color(theme.chrome_edge)
        // Reported, not offered. There is no button here that throws them
        // away, because "delete six things you cannot see" is not a press
        // anybody can make an informed decision about — and the archive
        // already has an honest answer, which is the one this sentence gives.
        .when(report.orphans.count > 0, |d| {
            d.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.0))
                    .text_size(px(11.5))
                    .text_color(theme.muted)
                    .child(icon(Icon::Told, ICON_SM, theme.link))
                    .child(format!(
                        "{} no card. They stay in the file until you save a copy without them.",
                        match report.orphans.count {
                            1 => "One file has".to_string(),
                            n => format!("{n} files have"),
                        }
                    )),
            )
        })
        .children(smaller(view, theme, cx))
        .into_any_element()
}

/// The offer, and what it is an offer to do.
///
/// Three states and each is a sentence: a run in progress counts itself, a
/// board with something to gain says what and how much, and one with nothing
/// to gain says nothing at all rather than showing a button that would do
/// nothing.
///
/// The plan is worked out here, on the frame, and that is safe for the reason
/// `mbrd_core::shrink::plan` gives: it is arithmetic over maps that are already
/// in memory, with no decoding anywhere in it.
fn smaller(view: &BoardView, theme: Theme, cx: &mut Context<BoardView>) -> Option<AnyElement> {
    if let Some((done, total)) = view.squeezing() {
        return Some(
            div()
                .flex()
                .items_center()
                .gap(px(7.0))
                .text_size(px(11.5))
                .text_color(theme.muted)
                .child(format!("Making it smaller \u{2014} {done} of {total}"))
                .into_any_element(),
        );
    }

    let plan = mbrd_core::shrink::plan(&view.doc);
    if plan.is_empty() {
        return None;
    }

    Some(
        div()
            .flex()
            .items_center()
            .gap(px(16.0))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.0))
                    .child(div().text_size(px(12.0)).child(format!(
                        "{} could be re-encoded, saving about {}.",
                        plural(plan.jobs.len(), "picture"),
                        inventory::size(plan.bytes),
                    )))
                    // What it costs, said before it is agreed to rather than
                    // after. The lossy half is the whole reason this is a
                    // button and not a setting.
                    .child(div().text_size(px(11.0)).text_color(theme.muted).child(
                        "Re-encoded at a lower quality and no wider than 1200 pixels. Anything \
                         that would barely shrink is left alone, and one undo puts it all back.",
                    )),
            )
            .child(
                div()
                    .id("shrink")
                    .flex_none()
                    .px(px(12.0))
                    .py(px(6.0))
                    .rounded(px(crate::theme::RADIUS_SM))
                    .bg(theme.accent.opacity(0.16))
                    .border_1()
                    .border_color(theme.chrome_edge)
                    .text_size(px(12.5))
                    .hover(|s| s.bg(theme.accent.opacity(0.26)))
                    .active(|s| s.bg(theme.accent.opacity(0.36)))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _event, _window, cx| {
                            // The page stays up. It is the thing counting the
                            // run, and closing it would put the progress away
                            // with it.
                            this.squeeze_board(cx);
                            cx.stop_propagation();
                        }),
                    )
                    .child("Make it smaller"),
            )
            .into_any_element(),
    )
}

/// What to call a file.
///
/// A file with no name of its own is named by what it is, which is what the
/// extension says and is better than an empty row.
fn naming(file: &Weighed) -> String {
    match file.label.trim().is_empty() {
        true => format!("{} file", file.ext.to_uppercase()),
        false => file.label.clone(),
    }
}

/// What a file *is*, in words.
///
/// Off the format catalogue, which knows 1367 extensions by name — see
/// [`mbrd_core::formats`]. Where it has never heard of one, the extension
/// itself, because "CR3 file" is still an answer and "file" is not.
fn described(file: &Weighed) -> SharedString {
    match mbrd_core::formats::name(&file.ext) {
        Some(name) => name.into(),
        None if file.ext.trim().is_empty() => "Unknown".into(),
        None => SharedString::from(format!("{} file", file.ext.to_uppercase())),
    }
}

/// The colour a kind's mark and its share bar are drawn in.
///
/// Five tints off tokens that already exist, not twelve invented ones: the
/// column only has to make *pictures, film, sound and words* separable at a
/// glance while scrolling, and a palette with a colour for every `ItemType`
/// would be twelve near-neighbours nobody could tell apart anyway. Everything
/// the five do not cover is muted, which is honest — those rows are being
/// grouped as "the rest", and that is what they look like.
fn shelf(kind: &ItemType, theme: Theme) -> gpui::Hsla {
    match kind {
        ItemType::Image | ItemType::Swatch | ItemType::Sticker => theme.rope_leaf,
        ItemType::Video => theme.rope_accent,
        ItemType::Audio => theme.rope_warm,
        ItemType::Note | ItemType::Text | ItemType::Title => theme.note,
        ItemType::Link => theme.link,
        _ => theme.muted,
    }
}

/// The one line under the title that is about the board rather than its files.
///
/// Everything non-zero, joined. Nought is the one number worth no room at all:
/// a board with no connections should not be told it has none.
fn facts(report: &Inventory) -> String {
    let mut out: Vec<String> = report
        .kinds
        .iter()
        .map(|(kind, count)| format!("{count} {}", kind_word(kind, *count).to_lowercase()))
        .collect();
    if report.connections > 0 {
        out.push(plural(report.connections, "connection"));
    }
    if report.binned > 0 {
        out.push(format!("{} in the bin", report.binned));
    }
    // A count rather than a size, for the reason `Inventory::steps` gives —
    // and it is the row that explains a heavy board with no pictures on it.
    if report.steps > 0 {
        out.push(format!("{} of history", plural(report.steps, "step")));
    }
    // Real bytes that are invisible everywhere else on this page: a waveform
    // is kept beside its recording and is not a file with a row of its own.
    if report.waveforms > 0 {
        out.push(plural(report.waveforms, "waveform"));
    }
    match out.is_empty() {
        true => "Nothing on it yet".into(),
        false => out.join(" \u{b7} "),
    }
}

/// The plural of a card kind, for the counts.
///
/// The format's own word where it reads as English, which it mostly does —
/// the same judgement `palette::kind_word` makes one list over.
fn kind_word(kind: &mbrd_core::ItemType, count: usize) -> String {
    use mbrd_core::ItemType as T;
    let word = match kind {
        T::Image => "picture",
        T::Video => "clip",
        T::Audio => "sound",
        T::Note => "note",
        T::Text => "text file",
        T::Link => "link",
        T::Model => "model",
        T::Swatch => "color",
        T::Sticker => "sticker",
        T::Fence => "group",
        T::Title => "title card",
        T::Ghost => "hint",
        T::Generic => "file",
        // A type from a later build, named by what the file calls it — the
        // same rule `ItemType::Other` exists for.
        other => other.as_str(),
    };
    // The noun alone, because the number is the value in the column beside it
    // — "Pictures … 3" rather than "3 pictures … 3".
    let mut out = match count {
        1 => word.to_string(),
        _ => format!("{word}s"),
    };
    // Capitalised to match every other row's name on the page, which is the
    // only reason this is not simply the format's own word.
    if let Some(first) = out.get_mut(0..1) {
        first.make_ascii_uppercase();
    }
    out
}

/// `1 card`, `3 cards`.
fn plural(count: usize, word: &str) -> String {
    match count {
        1 => format!("1 {word}"),
        n => format!("{n} {word}s"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn weighed(label: &str, ext: &str, bytes: usize, cards: usize) -> Weighed {
        Weighed {
            hash: label.to_string(),
            label: label.to_string(),
            ext: ext.to_string(),
            bytes,
            orphan: cards == 0,
            cards: (0..cards).map(|n| format!("card-{n}")).collect(),
        }
    }

    /// Heaviest first, which is how the report already hands them over.
    fn report() -> Inventory {
        Inventory {
            files: vec![
                weighed("fitters-walkthrough", "mp4", 128, 1),
                weighed("worktop-run", "sldprt", 64, 2),
                weighed("IMG_4471", "heic", 42, 0),
                weighed("zellige-green-splashback", "jpg", 2, 3),
            ],
            ..Default::default()
        }
    }

    fn asked(filter: &str, sort: Sort) -> Sheet {
        Sheet { filter: Editor::new(filter, FILTER_MAX, false), sort, ..Sheet::open() }
    }

    fn names(rows: &[&Weighed]) -> Vec<String> {
        rows.iter().map(|f| f.label.clone()).collect()
    }

    #[test]
    fn the_default_order_is_the_one_the_page_is_opened_for() {
        let report = report();
        let rows = shown(&report, &asked("", Sort::Size));
        assert_eq!(names(&rows)[0], "fitters-walkthrough");
        assert_eq!(rows.len(), 4);
    }

    #[test]
    fn sorting_by_uses_raises_what_nothing_is_using() {
        // The whole reason this order exists: the orphan is the row somebody
        // picking "Uses" is looking for, and it is at nought.
        let report = report();
        let rows = shown(&report, &asked("", Sort::Uses));
        assert_eq!(names(&rows)[0], "IMG_4471");
    }

    #[test]
    fn sorting_by_name_is_alphabetical_and_case_blind() {
        // Case-blind, or `IMG_4471` sorts before every lower-case name on the
        // board purely because it was shouted.
        let report = report();
        let rows = shown(&report, &asked("", Sort::Name));
        assert_eq!(
            names(&rows),
            ["fitters-walkthrough", "IMG_4471", "worktop-run", "zellige-green-splashback"]
        );
    }

    #[test]
    fn the_filter_reads_the_kind_as_well_as_the_name() {
        let report = report();
        // By name.
        let rows = shown(&report, &asked("zellige", Sort::Size));
        assert_eq!(names(&rows), ["zellige-green-splashback"]);

        // And by what the format is called, which is the half somebody typing
        // "video" is relying on — nothing on this board is *named* that.
        let rows = shown(&report, &asked("video", Sort::Size));
        assert_eq!(names(&rows), ["fitters-walkthrough"], "MPEG-4 video, off the catalogue");
    }

    #[test]
    fn the_filter_ignores_case_and_surrounding_space() {
        let report = report();
        assert_eq!(shown(&report, &asked("  HEIC  ", Sort::Size)).len(), 1);
        // Whitespace alone is not a filter. A field holding two spaces must
        // not empty the page.
        assert_eq!(shown(&report, &asked("   ", Sort::Size)).len(), 4);
    }

    #[test]
    fn escape_clears_the_filter_before_it_closes_the_page() {
        let mut sheet = asked("heic", Sort::Size);
        assert!(matches!(sheet.key("escape", Modifiers::default(), None), Reply::Held));
        assert!(!sheet.filtering(), "the list is whole again");
        // And the second press, with nothing left to clear, leaves.
        assert!(matches!(sheet.key("escape", Modifiers::default(), None), Reply::Close));
    }

    #[test]
    fn a_press_the_field_ignored_does_not_light_the_caret() {
        // The rule `settings::Page::focused` states: a press the field did
        // something with is somebody using the field, and a key it has no
        // meaning for is not. An arrow key *is* one it has a meaning for —
        // it moves the caret — so the key that proves the rule is one the
        // editor genuinely has nothing to do with.
        let mut sheet = Sheet::open();
        sheet.key("f5", Modifiers::default(), None);
        assert!(!sheet.focused);
        sheet.key("h", Modifiers::default(), Some("h"));
        assert!(sheet.focused);
    }

    /// A file the catalogue has never heard of is still named by what it is.
    #[test]
    fn an_unknown_extension_is_described_by_itself() {
        assert_eq!(described(&weighed("a", "mp4", 1, 1)), "MP4 video");
        assert_eq!(described(&weighed("a", "zzqq", 1, 1)), "ZZQQ file");
        assert_eq!(described(&weighed("a", "", 1, 1)), "Unknown");
    }

    /// A file with no name of its own gets one rather than an empty cell.
    #[test]
    fn a_nameless_file_is_named_by_its_extension() {
        assert_eq!(naming(&weighed("", "png", 1, 1)), "PNG file");
        // Blank is as nameless as empty: an archive written by a build that
        // stored a space would otherwise get a row with nothing in the name
        // column at all.
        let mut file = weighed("x", "png", 1, 1);
        file.label = "   ".into();
        assert_eq!(naming(&file), "PNG file");
        assert_eq!(naming(&weighed("beach.jpg", "jpg", 1, 1)), "beach.jpg");
    }
}
