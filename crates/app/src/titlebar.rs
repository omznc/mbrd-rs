//! The app's own top bar, on every platform.
//!
//! **This bar is always drawn.** Not the platform's — this one. A window whose
//! chrome is a GNOME headerbar on one desk, an NSWindow titlebar on another and
//! a Windows caption on a third is three different applications wearing the
//! same icon, and the thing that lives at the top left of it — the board you
//! have open, the way to open another, and the two ways to reach everything
//! else — is part of the app rather than part of the window manager. So the
//! bar, its title, its project switcher and the two buttons beside it are ours
//! everywhere.
//!
//! What is *not* always ours is the three buttons on the right, and that is the
//! one thing in here that varies by platform:
//!
//! - **macOS** keeps its traffic lights. They are real `NSButton`s the system
//!   draws over our bar and cannot be replaced, only hidden — and hiding them
//!   is how you ship a Mac app nobody can close with the shortcut they know. So
//!   we draw none of our own and leave [`LEFT_INSET`] of room for them.
//! - **Windows** hides its caption via `TitlebarOptions::appears_transparent`,
//!   so the three buttons are ours.
//! - **Linux** is the one that has to be asked. The compositor advertises
//!   whether it does server-side decorations, and several common ones — GNOME's
//!   mutter among them — do not. Under `Decorations::Client` the buttons are
//!   ours; under `Server` the compositor has drawn its own and ours would be a
//!   second set.
//!
//! The resize edges follow the same rule as those buttons and for the same
//! reason: server-side decorations carry their own drag targets, client-side
//! ones do not, and a window that cannot be resized from its edges is not
//! obviously a window.

use gpui::{
    div, prelude::*, px, Context, CursorStyle, Decorations, MouseButton, ResizeEdge, Window,
    WindowControlArea,
};

use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;

use crate::board_view::{BoardView, UpdateBadge};
use crate::color::Tint;
use crate::command::Command;
use crate::icons::{icon, Icon};
use crate::tip::tip;

/// How tall the drawn titlebar is.
pub const TITLEBAR_HEIGHT: f32 = 34.0;

/// How much room to leave at the left before anything of ours is drawn.
///
/// On macOS this is the traffic lights, which the system draws *over* this bar
/// at the position `main.rs` asks for. Everywhere else it is ordinary padding.
/// Getting this wrong on a Mac puts the project switcher underneath the close
/// button, which is the sort of thing that only shows up on the one machine
/// nobody testing has.
pub const LEFT_INSET: f32 = if cfg!(target_os = "macos") { 78.0 } else { 12.0 };

// Checked where it cannot be got wrong quietly. Too little room on a Mac puts
// the project switcher underneath the close button, and that is invisible on
// every machine this is likely to be written on.
const _: () = assert!(!cfg!(target_os = "macos") || LEFT_INSET > 60.0);

/// Whether the three window buttons are this app's to draw.
///
/// See the module note: never on macOS, always on Windows, and on Linux only
/// where the compositor said it was leaving them to us.
fn buttons_are_ours(window: &Window) -> bool {
    // Never in a browser, where there is no window for any of the three to
    // act on: the tab is the window. Minimise and zoom have nothing to do,
    // and a close that removed the gpui window would leave a blank page with
    // no way to bring a board back into it. The browser's own three are an
    // inch above ours and they work.
    if cfg!(target_family = "wasm") {
        return false;
    }
    if cfg!(target_os = "macos") {
        return false;
    }
    if cfg!(target_os = "linux") {
        return matches!(window.window_decorations(), Decorations::Client { .. });
    }
    true
}

/// What the project switcher calls the board that is open.
///
/// The board's own title first, because that is what somebody named it; the
/// file name next, because a board saved but never titled is still a board you
/// recognise; and "untitled" last, which is the honest answer for one that is
/// neither. The extension is dropped — `.mbrd` on every row of a list of
/// nothing but `.mbrd` files is a column of noise.
pub fn project_name(title: &str, path: Option<&Path>) -> String {
    if !title.trim().is_empty() {
        return title.to_string();
    }
    path.and_then(Path::file_stem)
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "untitled".into())
}

/// How wide the invisible grab strip along each edge is.
///
/// Wide enough to hit without aiming — a one-pixel target is a resize handle
/// only in principle.
const RESIZE_GRAB: f32 = 5.0;

/// The top bar. Always drawn; see the module note.
///
/// Returns an `AnyElement` because the right-hand side is present on some
/// platforms and absent on others, and erasing the type here is cheaper than
/// making the absent case pretend to be a row of buttons.
pub fn render(view: &BoardView, window: &Window, cx: &mut Context<BoardView>) -> gpui::AnyElement {
    let controls = window.window_controls();
    let ours = buttons_are_ours(window);
    let theme = &view.theme;

    div()
        .id("titlebar")
        .flex()
        .items_center()
        .justify_between()
        .gap(px(8.0))
        .h(px(TITLEBAR_HEIGHT))
        .flex_shrink_0()
        .pl(px(LEFT_INSET))
        // Where the three buttons are ours they run to the window's edge, the
        // way every titlebar's do. Where they are not, the bar needs an edge of
        // its own on that side.
        .when(!ours, |d| d.pr(px(12.0)))
        .bg(theme.chrome)
        .border_b_1()
        .border_color(theme.chrome_edge)
        .text_size(px(12.0))
        .text_color(theme.muted)
        // Dragging the bar moves the window, and a double-click maximises it.
        // Both are conventions the platform would otherwise have provided, and
        // a bar of our own that only *looks* like one is worse than none: a
        // window you cannot move is the failure this whole module exists to
        // avoid. The switcher and the buttons stop this from firing under them
        // by claiming the press themselves.
        .on_mouse_down(MouseButton::Left, |event, window, _cx| {
            // Both of these are Windows' own, and it does them from the
            // caption strip below rather than from here — see [`caption`].
            // Doing them here as well would zoom the window twice on a
            // double-click and leave it exactly where it started.
            if cfg!(target_os = "windows") {
                return;
            }
            if event.click_count == 2 {
                window.zoom_window();
            } else {
                window.start_window_move();
            }
        })
        // The board you have open, and the two ways to reach everything else.
        // One group rather than three children of the bar, because the bar
        // spreads its children apart and these three belong together.
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(2.0))
                .child(switcher_button(view, cx))
                // Which group you are standing in is *not* here. It was, and it
                // was wrong: the titlebar is where the things that never change
                // live, and a crumb that appears and disappears as you step in
                // and out of groups makes the whole row jump. It belongs with
                // the other passing facts, in the status bar — see
                // `BoardView::inside_line` and `BoardView::inside_count`, which
                // together say the group's name and the group's count down
                // there beside the rest of them.
                // Every surface you *go to* rather than do something with, in
                // the order they are reached for. A button here has to earn
                // its two dozen pixels twice over — the bar is the one piece
                // of chrome nobody can put away — so the rule is that it
                // opens a whole screen and there is no other way to it than
                // a name typed into the palette. Everything that acts on the
                // board stays out: those have keys, and a row of verbs up
                // here would be a toolbar, which is the thing this app got
                // rid of. See the note over `BoardView::status_bar`.
                .child(reach(view, "commands", Command::Palette, Icon::Commands, cx))
                .child(reach(view, "find", Command::Search, Icon::Find, cx))
                .child(reach(view, "inventory", Command::Inventory, Icon::Cards, cx))
                .child(reach(view, "tour", Command::Tour, Icon::Tour, cx))
                .child(reach(view, "settings", Command::Settings, Icon::Settings, cx)),
        )
        // The right-hand side: the update badge, then the window buttons where
        // they are ours. The badge sits before the buttons on Linux and
        // Windows and is simply the rightmost thing on a Mac, which is the
        // same sentence both ways: as far right as this app's own chrome goes.
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .children(update_badge(view, cx))
                .children(download_badge(view, cx))
                .children(source_button(view, cx))
                .when(ours, |d| {
                    d.child(
                        div()
                            .flex()
                            .items_center()
                            .when(controls.minimize, |d| {
                                d.child(control(
                                    "minimise",
                                    Icon::Minimise,
                                    theme.muted,
                                    theme.muted,
                                    cx,
                                    |_view, window, _cx| window.minimize_window(),
                                ))
                            })
                            .when(controls.maximize, |d| {
                                d.child(control(
                                    "maximise",
                                    Icon::Maximise,
                                    theme.muted,
                                    theme.muted,
                                    cx,
                                    |_view, window, _cx| window.zoom_window(),
                                ))
                            })
                            .child(control(
                                "close",
                                Icon::Close,
                                theme.muted,
                                theme.accent,
                                cx,
                                // The board goes to disk before the window goes away.
                                // This button does not route through the compositor, so
                                // the `on_window_should_close` hook `main.rs` registers
                                // never sees it — see `BoardView::flush`, which both
                                // paths call and which is a no-op when the autosave
                                // timer has already been round.
                                |view, window, cx| {
                                    view.flush(cx);
                                    window.remove_window();
                                },
                            )),
                    )
                }),
        )
        // Last, and painted over everything above it, which is the point: gpui
        // answers a Windows hit test with the *first* region marked under the
        // pointer, in paint order, so a caption declared here can never take a
        // press away from a button declared before it.
        .when(cfg!(target_os = "windows"), |d| d.child(caption(window)))
        .into_any_element()
}

/// The part of the bar Windows is told is a caption. Windows only.
///
/// [`Window::start_window_move`], which the bar calls on press everywhere
/// else, is a Linux thing: X11 and Wayland both implement it, macOS does not
/// need it — an `appears_transparent` window keeps a real NSWindow titlebar
/// under ours and drags from it — and on Windows gpui inherits the empty
/// default, so the press did nothing at all and the window could not be moved.
///
/// What Windows wants instead is an answer to `WM_NCHITTEST`: the window says
/// which parts of its client area are caption, and the OS drags, snaps and
/// double-click-zooms from them exactly as it would from a native titlebar.
/// [`WindowControlArea::Drag`] is how gpui asks for that, and letting the OS
/// do all three is why the bar's own press handler stands down there.
///
/// This is a child of the bar rather than the bar itself because of the top
/// edge. gpui only falls through to its own resize handling when *no* marked
/// region is under the pointer, so a caption that reached the top of the
/// window would be a window that could not be resized from the top. The strip
/// left uncovered is that resize border — and only while there is one worth
/// leaving: a maximised window cannot be resized, and the gap would be a dead
/// line across the top of the bar.
fn caption(window: &Window) -> impl IntoElement {
    div()
        .absolute()
        .left_0()
        .right_0()
        .bottom_0()
        .top(px(if window.is_maximized() { 0.0 } else { RESIZE_GRAB }))
        .window_control_area(WindowControlArea::Drag)
}

/// The project switcher: what board is open, and the way to open another.
///
/// Zed's, in shape and in placement, because the shape is right — the name of
/// the thing you have open is the most useful label a title bar can carry, and
/// making it the *button* means the way to swap it is where you were already
/// looking. It opens the same [`crate::switcher::Switcher`] that `Ctrl P`
/// does rather than a surface of its own; two board pickers that could
/// disagree about what "recent" means is one more than this app needs.
///
/// **There used to be no dot, on purpose.** This button used to carry one to
/// say the board had changes that were not on disk, which was the only
/// unsaved-work indicator in the app. The board is written a second after the
/// last change to it — see `BoardView::arm_autosave` — so that dot spent its
/// life either absent or a second from being absent, and an indicator that is
/// almost always off is one that has stopped being read by the time it means
/// something.
///
/// The dot below is not that dot. It answers to `BoardView::save_failing`
/// rather than to `unsaved()`, and a failing save is nothing like a second
/// from being absent — it stands for as long as the disk keeps refusing the
/// board, from a few seconds to the rest of the session, which is exactly the
/// condition an indicator here can actually be read under. Off stays the
/// ordinary state; this only lights up when it has something to say.
fn switcher_button(view: &BoardView, cx: &mut Context<BoardView>) -> impl IntoElement {
    let theme = view.theme;
    let name = project_name(&view.doc.board.title, view.path.as_deref());
    let failing = view.save_failing();

    div()
        .id("project")
        .flex()
        .items_center()
        .gap(px(6.0))
        .px(px(8.0))
        .py(px(3.0))
        .rounded(px(5.0))
        .text_color(theme.text)
        .text_size(px(12.0))
        .hover(|s| s.bg(theme.text.opacity(0.08)))
        .active(|s| s.bg(theme.text.opacity(0.14)))
        // Mouse-down rather than click, and before the bar's own handler gets
        // it: the bar starts a window move on press, so a button that waited
        // for the release would drag the window a pixel on the way to opening.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _event, window, cx| {
                cx.stop_propagation();
                this.open_switcher(window, cx);
            }),
        )
        // Says what the button is for rather than what it says, which is the
        // one thing the label cannot: the name on it is the *board's*, and a
        // board's name is no help to somebody wondering what pressing it does.
        // While a save is failing that stops being the useful thing to say —
        // the tooltip names the problem instead, since that is what anybody
        // pausing over this button at that moment actually wants to know.
        .when_else(
            failing,
            |d| d.tooltip(tip(theme, "save is failing", "check the disk and try again")),
            |d| d.tooltip(tip(theme, Command::OpenBoard.label(), Command::OpenBoard.hint())),
        )
        .child(div().child(name))
        // Only while `failed_at` stands — see `BoardView::save_failing`. A
        // small accent dot rather than another line of text, because the
        // status bar under the board already spelled out "could not save" in
        // full when this began; the dot's whole job is to still be true after
        // that line fades at `WARN_FOR`, which a save that is still failing
        // easily outlasts.
        .when(failing, |d| {
            d.child(
                div()
                    .id("save-failing")
                    .size(px(6.0))
                    .rounded_full()
                    .bg(theme.accent)
                    .tooltip(tip(theme, "save is failing", "check the disk and try again")),
            )
        })
        // The chevron a menu button has, which is what says this one opens
        // something rather than merely reporting a name. Decorative rather
        // than read — the board's own name beside it already says what this
        // is — so it draws in `tertiary` rather than `muted`.
        .child(icon(Icon::CaretDown, crate::icons::ICON_SM, theme.tertiary))
}

/// One of the two wordless buttons beside the switcher.
///
/// The command palette and the board search, which are the two things in this
/// app that a keystroke was previously the only way to reach — and one of those
/// keystrokes is a *double tap of Shift*, which is a lovely gesture and one
/// nobody discovers by accident. A button is how somebody who has never read
/// the roadmap finds out either exists.
///
/// Wordless because the bar is 34 tall and the board's name already lives on
/// it; two more labels would push the name of the thing you are working on into
/// the middle of the window. What replaces the words is the tooltip, and the
/// tooltip carries the key — so the button is a way in for somebody who does
/// not know the chord *and* the place they learn it, which is the only reason a
/// button for something with a key is worth its room.
///
/// The command it runs, its name and its key all come from the one entry in
/// `command.rs`. See that module's opening note: a second copy of a keystroke
/// is a keystroke that will eventually be wrong.
fn reach(
    view: &BoardView,
    id: &'static str,
    command: Command,
    mark: Icon,
    cx: &mut Context<BoardView>,
) -> impl IntoElement {
    let theme = view.theme;

    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(24.0))
        .h(px(22.0))
        .rounded(px(crate::theme::RADIUS_XS))
        .hover(|s| s.bg(theme.text.opacity(0.08)))
        .active(|s| s.bg(theme.text.opacity(0.14)))
        .tooltip(tip(theme, command.label(), command.hint()))
        // Mouse-down and stopped here, for the reason the switcher gives: the
        // bar starts a window move on press, so a button that waited for the
        // release would drag the window a pixel on the way to opening.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _event, window, cx| {
                cx.stop_propagation();
                command.run(this, window, cx);
            }),
        )
        .child(icon(mark, crate::icons::ICON_MD, theme.muted))
}

/// The chip that leads out of the browser and onto a desktop.
///
/// `None` everywhere but the web, where the same slot holds the update badge —
/// which is the point of them sharing it. A native build can update itself and
/// has nowhere to send you; a page cannot update anything and has one thing
/// worth pointing at. Neither platform draws both, so the corner of the bar
/// holds one chip either way.
#[cfg(not(target_family = "wasm"))]
fn download_badge(_view: &BoardView, _cx: &mut Context<BoardView>) -> Option<gpui::AnyElement> {
    None
}

/// See the note on its twin above.
///
/// Two faces rather than four, unlike the update badge: *quiet*, which is
/// every state where the file has not been named yet and the press goes to the
/// releases page, and *offered*, which is a build for this machine that one
/// press downloads. Asking has no face of its own on purpose — it takes a
/// moment and a chip that flickered on every launch is one nobody reads.
#[cfg(target_family = "wasm")]
fn download_badge(view: &BoardView, cx: &mut Context<BoardView>) -> Option<gpui::AnyElement> {
    use crate::webget::Getting;
    let theme = view.theme;

    let pill = div()
        .id("download")
        .flex()
        .items_center()
        .gap(px(6.0))
        .px(px(9.0))
        .h(px(22.0))
        .rounded(px(crate::theme::RADIUS_XS))
        .text_size(px(11.0))
        // Mouse-down and claimed, for the reason every button on this bar
        // gives: the bar starts a window move on press.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _event, _window, cx| {
                cx.stop_propagation();
                this.get_the_app(cx);
            }),
        );

    Some(match view.getting() {
        Getting::Found(build) => pill
            .text_color(theme.accent_text)
            .bg(theme.accent.opacity(0.10))
            .border_1()
            .border_color(theme.accent.opacity(0.35))
            .hover(|s| s.bg(theme.accent.opacity(0.18)))
            .active(|s| s.bg(theme.accent.opacity(0.26)))
            .tooltip(tip(
                theme,
                format!(
                    "mbrd {} for {} \u{00b7} {}",
                    build.version,
                    build.desk.label(),
                    crate::webget::size(build.bytes)
                ),
                build.desk.caveat(),
            ))
            .child(icon(Icon::Drop, crate::icons::ICON_SM, theme.accent_text))
            .child(format!("Get for {}", build.desk.label()))
            .into_any_element(),

        // Cold, asking, or nothing to name. One face, because from the outside
        // they are one thing: the file has not been named and the press goes
        // to the page that names all of them.
        _ => pill
            .text_color(theme.tertiary)
            .hover(|s| s.text_color(theme.muted).bg(theme.text.opacity(0.06)))
            .tooltip(tip(theme, "mbrd on your desktop", "opens the releases page"))
            .child(icon(Icon::Drop, crate::icons::ICON_SM, theme.tertiary))
            .child("Get the app")
            .into_any_element(),
    })
}

/// Where this came from. `None` off the web.
///
/// Only in a browser, and the reason is what a browser is: somebody who
/// downloaded a build chose it from a page that had the repository on it, and
/// somebody who opened a link has been shown a whole application by a stranger
/// with no way to see what it is. One press is that way.
#[cfg(not(target_family = "wasm"))]
fn source_button(_view: &BoardView, _cx: &mut Context<BoardView>) -> Option<gpui::AnyElement> {
    None
}

/// See the note on its twin above.
#[cfg(target_family = "wasm")]
fn source_button(view: &BoardView, cx: &mut Context<BoardView>) -> Option<gpui::AnyElement> {
    let theme = view.theme;
    Some(
        div()
            .id("source")
            .flex()
            .items_center()
            .justify_center()
            .w(px(24.0))
            .h(px(22.0))
            .rounded(px(crate::theme::RADIUS_XS))
            .hover(|s| s.bg(theme.text.opacity(0.08)))
            .active(|s| s.bg(theme.text.opacity(0.14)))
            .tooltip(tip(theme, "Source", "every line of this, on GitHub"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_this, _event, _window, cx| {
                    cx.stop_propagation();
                    crate::webget::go(crate::webget::SOURCE);
                }),
            )
            .child(icon(Icon::OpenOut, crate::icons::ICON_MD, theme.muted))
            .into_any_element(),
    )
}

/// How wide the download's progress track is.
///
/// Narrow on purpose. It stands where the dot stands in the two states either
/// side of it, so the badge changes what it is saying without changing how
/// much room it takes — see [`update_badge`]. This is the corner of a
/// titlebar, not a download manager.
const PROGRESS_TRACK: f32 = 34.0;

/// What the bar says about an update.
///
/// `None` only on a build with no updater in it — see `BoardView::update_badge`.
/// Otherwise it is always drawn, and the five states are one chip walking a
/// stepper rather than five different pieces of chrome: *resting* (the version
/// you are running; press to check), *available* (press to download), the bar
/// filling, *ready* (press to save, install and restart), and — on a `.deb` or
/// `.rpm` install, where the last step belongs to a package manager —
/// *installing*.
///
/// **Loudness is earned.** Resting has no border and no fill. The middle two
/// have a wash and an edge. Only Ready fills, and it is also the only one
/// whose words are a verb — which is the whole of why it is allowed to.
/// Installing goes quiet again: the verb has been pressed, and what is being
/// waited on is a password box belonging to something else on the screen.
///
/// The dot and the progress track occupy the same slot, so the chip's width
/// barely moves as it walks; a badge that jumped the window title sideways
/// every few seconds during a download would be its own small emergency.
///
/// Clicks land on [`BoardView::update_step`], the same door `Ctrl U` uses, so
/// the badge cannot drift from what the key does: each is a face on the one
/// state machine.
fn update_badge(view: &BoardView, cx: &mut Context<BoardView>) -> Option<gpui::AnyElement> {
    let badge = view.update_badge()?;
    let theme = view.theme;

    let pill = div()
        .id("update")
        .flex()
        .items_center()
        .gap(px(6.0))
        .px(px(9.0))
        .h(px(22.0))
        .rounded(px(crate::theme::RADIUS_XS))
        .text_size(px(11.0))
        // Mouse-down and claimed, for the reason every button on this bar
        // gives: the bar starts a window move on press, and on Windows the
        // caption strip would otherwise take the press as a drag.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, _event, _window, cx| {
                cx.stop_propagation();
                this.update_step(cx);
            }),
        );

    /// The mark that stands in the slot the progress track takes over.
    fn dot(colour: gpui::Hsla) -> gpui::Div {
        div().size(px(5.0)).rounded_full().bg(colour)
    }

    Some(match badge {
        UpdateBadge::Resting { version } => pill
            .text_color(theme.tertiary)
            // No border and no wash. It is the version, sitting there, and it
            // has to be ignorable for a week without becoming invisible.
            .hover(|s| s.text_color(theme.muted).bg(theme.text.opacity(0.06)))
            .tooltip(tip(theme, format!("mbrd {version}"), "click to check for updates"))
            .child(dot(theme.chrome_edge))
            .child(format!("v{version}"))
            .into_any_element(),

        UpdateBadge::Available { version } => pill
            .text_color(theme.accent_text)
            .bg(theme.accent.opacity(0.10))
            .border_1()
            .border_color(theme.accent.opacity(0.35))
            .hover(|s| s.bg(theme.accent.opacity(0.18)))
            .active(|s| s.bg(theme.accent.opacity(0.26)))
            .tooltip(tip(theme, format!("mbrd {version} is out"), "click to download"))
            // The version is the whole message. "update available" said the
            // same thing in two words that the changed colour was already
            // saying, and left out the one fact somebody wants: which one.
            .child(dot(theme.accent))
            .child(format!("v{version}"))
            .into_any_element(),

        UpdateBadge::Downloading { fraction } => pill
            .text_color(theme.accent_text)
            .bg(theme.accent.opacity(0.10))
            .border_1()
            .border_color(theme.accent.opacity(0.35))
            .tooltip(tip(theme, "downloading the update", ""))
            .child(
                div()
                    .w(px(PROGRESS_TRACK))
                    .h(px(3.0))
                    .rounded_full()
                    .overflow_hidden()
                    .bg(theme.accent.opacity(0.25))
                    .child(
                        div()
                            // Never fully empty: a hairline of accent from the
                            // first frame is what says "started" while the
                            // first bytes are still on their way.
                            .w(px((PROGRESS_TRACK * fraction).max(2.0)))
                            .h_full()
                            .rounded_full()
                            .bg(theme.accent),
                    ),
            )
            .child(
                div()
                    .font(crate::opened::mono())
                    .text_size(px(10.5))
                    .child(format!("{:.0}%", fraction * 100.0)),
            )
            .into_any_element(),

        UpdateBadge::Ready { version } => pill
            // The only filled state in the whole titlebar, and the only one of
            // these four with a verb in it. Both for the same reason: this is
            // the step where there is a finished download sitting on the disk
            // and one press between it and being the app you are running.
            .text_color(theme.ground)
            .bg(theme.accent)
            .hover(|s| s.bg(theme.accent.opacity(0.88)))
            .active(|s| s.bg(theme.accent.opacity(0.78)))
            .font_weight(gpui::FontWeight::MEDIUM)
            .tooltip(tip(
                theme,
                format!("mbrd {version} is ready"),
                "click to save, install and restart",
            ))
            .child(icon(Icon::Restart, crate::icons::ICON_SM, theme.ground))
            .child("Restart to update")
            .into_any_element(),

        // Quiet, and nothing to press: the click still lands on the stepper,
        // where it only describes what is already happening. The dot stays
        // accent because something *is* in flight — it is simply not ours.
        UpdateBadge::Installing { version } => pill
            .text_color(theme.muted)
            .tooltip(tip(
                theme,
                format!("installing mbrd {version}"),
                "answer the permission prompt",
            ))
            .child(dot(theme.accent))
            .child("Installing…")
            .into_any_element(),
    })
}

/// One of the three window buttons.
///
/// `mark` is what the picture is drawn in and `hover` is what the wash behind
/// it is drawn in, and they are two arguments because on the close button they
/// differ: every window in the world lights that one red, and none of them
/// draws its cross red all the time.
fn control(
    id: &'static str,
    mark: Icon,
    colour: gpui::Hsla,
    hover: gpui::Hsla,
    cx: &mut Context<BoardView>,
    action: impl Fn(&mut BoardView, &mut Window, &mut Context<BoardView>) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(44.0))
        .h(px(TITLEBAR_HEIGHT))
        .hover(move |s| s.bg(hover.opacity(0.18)))
        .active(move |s| s.bg(hover.opacity(0.34)))
        // Mouse-down rather than click, and claimed rather than left to
        // bubble, and both matter here: the bar starts a window move on
        // mouse-down, so a button that waited for the release would drag the
        // window a pixel on the way to firing. On Windows the press that has
        // to be claimed is the *OS*'s — these buttons sit inside the caption
        // strip, and a press that reached it would put the window into a drag
        // instead of pressing the button. `stop_propagation` is what tells
        // gpui to answer the platform that the press was handled here.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |view, _event, window, cx| {
                cx.stop_propagation();
                action(view, window, cx)
            }),
        )
        .child(icon(mark, crate::icons::ICON_MD, colour))
}

/// The eight invisible grab strips around the window.
///
/// Skipped entirely under server-side decorations, and each one skipped
/// individually where the window is tiled against that edge — a tiled window
/// cannot be resized outwards there, and leaving a live handle on an edge that
/// does nothing is a control that lies.
pub fn resize_handles(window: &Window) -> Vec<gpui::AnyElement> {
    let Decorations::Client { tiling } = window.window_decorations() else {
        return Vec::new();
    };

    let mut out: Vec<gpui::AnyElement> = Vec::new();
    let g = px(RESIZE_GRAB);

    let mut edge = |id: &'static str,
                    edge: ResizeEdge,
                    cursor: CursorStyle,
                    place: fn(gpui::Div, gpui::Pixels) -> gpui::Div,
                    suppressed: bool| {
        if suppressed {
            return;
        }
        out.push(
            place(div().absolute(), g)
                .id(id)
                .cursor(cursor)
                .on_mouse_down(MouseButton::Left, move |_e, window, _cx| {
                    window.start_window_resize(edge)
                })
                .into_any_element(),
        );
    };

    edge(
        "resize-top",
        ResizeEdge::Top,
        CursorStyle::ResizeUpDown,
        |d, g| d.top_0().left_0().right_0().h(g),
        tiling.top,
    );
    edge(
        "resize-bottom",
        ResizeEdge::Bottom,
        CursorStyle::ResizeUpDown,
        |d, g| d.bottom_0().left_0().right_0().h(g),
        tiling.bottom,
    );
    edge(
        "resize-left",
        ResizeEdge::Left,
        CursorStyle::ResizeLeftRight,
        |d, g| d.left_0().top_0().bottom_0().w(g),
        tiling.left,
    );
    edge(
        "resize-right",
        ResizeEdge::Right,
        CursorStyle::ResizeLeftRight,
        |d, g| d.right_0().top_0().bottom_0().w(g),
        tiling.right,
    );

    // The corners last, so they sit above the edges they overlap and a diagonal
    // drag does not turn into a one-axis one.
    edge(
        "resize-top-left",
        ResizeEdge::TopLeft,
        CursorStyle::ResizeUpLeftDownRight,
        |d, g| d.top_0().left_0().w(g * 2.).h(g * 2.),
        tiling.top || tiling.left,
    );
    edge(
        "resize-top-right",
        ResizeEdge::TopRight,
        CursorStyle::ResizeUpRightDownLeft,
        |d, g| d.top_0().right_0().w(g * 2.).h(g * 2.),
        tiling.top || tiling.right,
    );
    edge(
        "resize-bottom-left",
        ResizeEdge::BottomLeft,
        CursorStyle::ResizeUpRightDownLeft,
        |d, g| d.bottom_0().left_0().w(g * 2.).h(g * 2.),
        tiling.bottom || tiling.left,
    );
    edge(
        "resize-bottom-right",
        ResizeEdge::BottomRight,
        CursorStyle::ResizeUpLeftDownRight,
        |d, g| d.bottom_0().right_0().w(g * 2.).h(g * 2.),
        tiling.bottom || tiling.right,
    );

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_names_the_board_by_what_somebody_called_it() {
        let path = PathBuf::from("/a/kitchen.mbrd");
        assert_eq!(project_name("Kitchen rebuild", Some(&path)), "Kitchen rebuild");
    }

    #[test]
    fn a_board_never_titled_is_named_by_its_file() {
        // Without the extension: a list of nothing but `.mbrd` files has a
        // column of `.mbrd` in it, which tells you nothing.
        let path = PathBuf::from("/a/kitchen.mbrd");
        assert_eq!(project_name("", Some(&path)), "kitchen");
        // And whitespace is not a title. Somebody who cleared the field left it
        // untitled, whatever the string says.
        assert_eq!(project_name("   ", Some(&path)), "kitchen");
    }

    #[test]
    fn everything_the_bar_offers_can_say_what_it_does_and_what_key_does_it() {
        // The three buttons at the top left are wordless or named after the
        // board rather than after what they do, so the tooltip is the whole of
        // what they say — and it is built out of the command table. An entry
        // that lost its hint would leave a button that teaches nobody the key,
        // which is the only reason a button for something with a key is here.
        for command in [Command::OpenBoard, Command::Palette, Command::Search] {
            assert!(!command.label().is_empty(), "{command:?} has no name to show");
            assert!(!command.hint().is_empty(), "{} advertises no key", command.label());
        }
    }

    #[test]
    fn a_board_that_is_neither_says_so_rather_than_being_blank() {
        // The state a fresh window is in. An empty button is a button nobody
        // can see is a button.
        assert_eq!(project_name("", None), "untitled");
    }
}
