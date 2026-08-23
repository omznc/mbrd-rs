//! The settings page.
//!
//! Every switch the app has, on one surface, split the way `prefs.rs` is
//! built: **Board** rows write into the `.mbrd` through the ledger — they
//! travel with the file and undo can take them back — and **Application**
//! rows are about the person sitting here, live in their config directory,
//! and do neither. Each is a section in the sidebar, so the split is
//! navigation rather than small print.
//!
//! The shape is the one settings screens have converged on — a nav column on
//! the left, and on the right a column of rows where each setting is a name
//! with a sentence under it and its control at the far edge. The sentence is
//! the point: a switch called "Axes" tells you nothing at 2am, and this page
//! is the one place in the app with room to say what a thing does.
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
//! gap, media fit) go through their own `BoardView` setters, which go
//! through the one door every board edit goes through.

use gpui::{div, prelude::*, px, AnyElement, Context, FontWeight, MouseButton, SharedString};

use crate::board_view::{BoardView, UpdateBadge};
use crate::command::Command;
use crate::icons::{icon, Icon};
use crate::theme::Theme;

/// One of the sidebar's entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Board,
    Application,
}

impl Section {
    pub const ALL: [Self; 2] = [Self::Board, Self::Application];

    pub fn label(self) -> &'static str {
        match self {
            Self::Board => "Board",
            Self::Application => "Application",
        }
    }

    /// The sentence under the section's title, which is where the
    /// board/person split gets said in words.
    fn blurb(self) -> &'static str {
        match self {
            Self::Board => "Saved in the board's own file. These travel with the .mbrd, and undo can take them back.",
            Self::Application => "About this computer, not the board. Kept in your config directory and never saved into a file.",
        }
    }
}

/// What the page is currently showing. Lives inside `Overlay::Settings`.
#[derive(Debug, Clone)]
pub struct Page {
    pub section: Section,
}

impl Page {
    pub fn open() -> Self {
        Self { section: Section::Board }
    }
}

/// The steps the grid can be set to.
///
/// A short ladder rather than a free field, because the number is a lattice
/// pitch and not a measurement: any value *works*, but the ones anybody
/// chooses on purpose are the halvings and doublings around the default. A
/// board whose file carries some other number simply shows no choice lit.
const GRID_STEPS: [f32; 5] = [32.0, 48.0, 64.0, 96.0, 128.0];

/// The gaps the arrangement engine can be told to leave between cards.
const GAPS: [f32; 7] = [0.0, 4.0, 8.0, 12.0, 16.0, 24.0, 32.0];

pub fn render(page: &Page, view: &BoardView, cx: &mut Context<BoardView>) -> impl IntoElement {
    let theme = view.theme;
    let presence = view.overlay_presence.value();
    let section = page.section;

    let rows: Vec<AnyElement> = match section {
        Section::Board => board_rows(view, cx),
        Section::Application => application_rows(view, cx),
    };

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
        .bg(theme.ground)
        .text_color(theme.text)
        .opacity(presence)
        // The wheel and both buttons end here — the board underneath still
        // exists, and a press that fell through would land on a card nobody
        // can see.
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .w_full()
                .max_w(px(880.0))
                .h_full()
                .flex()
                // The same 8px arrival slide as the palette and the
                // switcher: one function of the current presence, so the
                // exit is the entrance played backwards.
                .mt(px(-(8.0 * (1.0 - presence))))
                .child(sidebar(section, view, cx))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .flex()
                        .flex_col()
                        .pl(px(32.0))
                        .pr(px(24.0))
                        .child(
                            div()
                                .flex()
                                .items_start()
                                .justify_between()
                                .pt(px(26.0))
                                .pb(px(14.0))
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap(px(3.0))
                                        .child(
                                            div()
                                                .text_size(px(16.0))
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .child(section.label()),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(11.0))
                                                .text_color(theme.muted)
                                                .child(section.blurb()),
                                        ),
                                )
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
                                        .child(icon(
                                            Icon::Close,
                                            crate::icons::ICON_MD,
                                            theme.muted,
                                        )),
                                ),
                        )
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
                                .children(rows),
                        ),
                ),
        )
}

// ---------------------------------------------------------------------------
// The sidebar
// ---------------------------------------------------------------------------

fn sidebar(current: Section, view: &BoardView, cx: &mut Context<BoardView>) -> AnyElement {
    let theme = view.theme;
    div()
        .flex_none()
        .w(px(190.0))
        .h_full()
        .flex()
        .flex_col()
        .justify_between()
        .pt(px(26.0))
        .pb(px(16.0))
        .pr(px(20.0))
        .border_r_1()
        .border_color(theme.chrome_edge)
        .child(
            div()
                .flex()
                .flex_col()
                .child(
                    div()
                        .px(px(10.0))
                        .pb(px(12.0))
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("Settings"),
                )
                .children(Section::ALL.map(|section| {
                    let active = section == current;
                    div()
                        .id(SharedString::from(section.label()))
                        .px(px(10.0))
                        .py(px(5.0))
                        .mb(px(2.0))
                        .rounded(px(crate::theme::RADIUS_SM))
                        .text_size(px(13.0))
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
                        .child(section.label())
                        .into_any_element()
                })),
        )
        .child(
            div()
                .px(px(10.0))
                .text_size(px(10.0))
                .text_color(theme.muted)
                .child(format!("mbrd {}", crate::update::version::Version::current())),
        )
        .into_any_element()
}

// ---------------------------------------------------------------------------
// The two sections
// ---------------------------------------------------------------------------

fn board_rows(view: &BoardView, cx: &mut Context<BoardView>) -> Vec<AnyElement> {
    let theme = view.theme;
    let settings = &view.doc.board.settings.desktop;
    let step = settings.grid_step;
    let gap = settings.spacing;
    let fit = view.doc.board.media_fit.clone();
    vec![
        toggle_row(
            Command::ToggleGrid,
            "Draw the dot lattice behind the board.",
            None,
            view,
            cx,
        ),
        toggle_row(
            Command::ToggleSnap,
            "Pull cards onto the grid as they are moved and resized. Turning it on snaps the whole board; turning it off puts everything back.",
            None,
            view,
            cx,
        ),
        toggle_row(Command::ToggleAxes, "Show the world axes through the origin.", None, view, cx),
        toggle_row(
            Command::ToggleWeb,
            "Draw the ropes between connected cards.",
            None,
            view,
            cx,
        ),
        toggle_row(
            Command::ToggleGuides,
            "Flash a guide when a drag lines up with a neighbour's edge or centre.",
            None,
            view,
            cx,
        ),
        row(
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
            theme,
        ),
        row(
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
            theme,
        ),
        row(
            "Media fit",
            "How photos and videos sit in their cards: the whole picture with margins, or the whole card with crops. A card's own menu can override it.",
            fit_control(&fit, view, cx),
            theme,
        ),
    ]
}

fn application_rows(view: &BoardView, cx: &mut Context<BoardView>) -> Vec<AnyElement> {
    // A preference the environment has pinned should say so on the row,
    // rather than appearing to take and then not surviving a restart — the
    // same warning `toggle_pref` says after the fact, said before it instead.
    let motion_note = crate::prefs::Prefs::forced(true)
        .map(|var| format!("Set by {var}, which wins at startup."));
    let update_note = crate::prefs::Prefs::forced(false)
        .map(|var| format!("Set by {var}, which wins at startup."));
    vec![
        toggle_row(
            Command::ToggleMotion,
            "Let the interface move. Turn off to land every change instantly.",
            motion_note,
            view,
            cx,
        ),
        toggle_row(
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
// Row chrome
// ---------------------------------------------------------------------------

/// One setting: a name, the sentence under it, and its control at the edge.
///
/// The ruled line belongs to the row rather than the list so every row is
/// the same shape; the last one's rule reads as the section's own edge.
fn row(
    title: impl Into<SharedString>,
    about: impl Into<SharedString>,
    control: AnyElement,
    theme: Theme,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(32.0))
        .py(px(13.0))
        .border_b_1()
        .border_color(theme.chrome_edge.opacity(0.6))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .min_w_0()
                .child(
                    div().text_size(px(13.0)).font_weight(FontWeight::MEDIUM).child(title.into()),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme.muted)
                        .line_height(gpui::relative(1.4))
                        .child(about.into()),
                ),
        )
        .child(div().flex_none().child(control))
        .into_any_element()
}

/// One switch, run through the same `Command` the menus and the palette run.
fn toggle_row(
    command: Command,
    about: &'static str,
    pinned: Option<String>,
    view: &BoardView,
    cx: &mut Context<BoardView>,
) -> AnyElement {
    let theme = view.theme;
    let on = command.ticked(view) == Some(true);
    // The environment note replaces the description rather than joining it:
    // "what this does" matters less than "why flipping it will not hold".
    let about: SharedString = match pinned {
        Some(words) => words.into(),
        None => about.into(),
    };
    row(command.label(), about, switch(command, on, view, cx), theme)
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
    let theme = view.theme;
    let id = command.label();
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
                command.run(this, window, cx);
                // Aimed at where the state *now* is, read back off the same
                // table the tick reads, so a run that did nothing moves
                // nothing.
                let now = command.ticked(this) == Some(true);
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

/// A choice made from a short row of segments, drawn as one control rather
/// than as loose chips: the container is what says the options are one
/// setting.
///
/// A plain `fn` pointer for `pick` rather than a closure, because every
/// segment captures it and a listener must be `'static` — a pointer is
/// `Copy` and carries nothing, which is exactly the amount of state picking
/// from a fixed list needs.
fn segmented(
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
                .when(active, |d| d.text_color(theme.accent).font_weight(FontWeight::MEDIUM))
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

fn fit_control(current: &str, view: &BoardView, cx: &mut Context<BoardView>) -> AnyElement {
    let fits = ["contain".to_string(), "cover".to_string()];
    let chosen = fits.iter().position(|f| f == current);
    segmented("media-fit", &fits, chosen, pick_fit, view, cx)
}

fn pick_fit(view: &mut BoardView, at: usize, cx: &mut Context<BoardView>) {
    view.set_media_fit(if at == 0 { "contain" } else { "cover" }, cx);
}

/// The one row that is a verb rather than a state, so its control is a
/// button — and the button's word follows how far the last press got, the
/// same stepper the titlebar badge walks.
fn update_row(view: &BoardView, cx: &mut Context<BoardView>) -> AnyElement {
    let theme = view.theme;
    let live = Command::CheckForUpdates.available(view);
    let word = match view.update_badge() {
        None => "Check now",
        Some(UpdateBadge::Available { .. }) => "Download",
        Some(UpdateBadge::Downloading { .. }) => "Downloading…",
        Some(UpdateBadge::Ready { .. }) => "Restart to update",
    };
    let about: SharedString = if live {
        format!("You have mbrd {}.", crate::update::version::Version::current()).into()
    } else {
        "This build was not installed from a release, so it has nothing to update.".into()
    };
    let button = div()
        .id("settings-update")
        .px(px(10.0))
        .py(px(4.0))
        .rounded(px(crate::theme::RADIUS_SM))
        .text_size(px(11.0))
        .border_1()
        .border_color(theme.chrome_edge)
        .when(live, |d| {
            d.bg(theme.chrome)
                .hover(|s| s.bg(theme.accent.opacity(0.10)))
                .active(|s| s.bg(theme.accent.opacity(0.18)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.update_step(cx);
                        cx.notify();
                    }),
                )
        })
        .when(!live, |d| d.text_color(theme.muted))
        .child(word)
        .into_any_element();
    row("Check for updates", about, button, theme)
}
