//! The window's own furniture, for compositors that decline to draw it.
//!
//! On Wayland there is no guarantee anybody else will put a title and three
//! buttons at the top of the window. The compositor advertises whether it does
//! server-side decorations, and several of the common ones — GNOME's mutter
//! among them — simply do not, so the application is handed a bare rectangle
//! and is expected to draw its own.
//!
//! So everything here is **conditional on what the compositor actually said**,
//! read fresh each frame from [`gpui::Window::window_decorations`]. Under
//! `Decorations::Server` this module draws nothing at all: a titlebar of our
//! own underneath the compositor's would be two titlebars, which is the failure
//! mode of hard-coding client-side decorations because they worked on one desk.
//!
//! The resize edges are here for the same reason. Server-side decorations carry
//! their own drag targets; client-side ones do not, and a window that cannot be
//! resized from its edges is not obviously a window.

use gpui::{
    div, prelude::*, px, App, Context, CursorStyle, Decorations, MouseButton, ResizeEdge, Window,
};

use crate::board_view::BoardView;

/// How tall the drawn titlebar is.
pub const TITLEBAR_HEIGHT: f32 = 34.0;

/// How wide the invisible grab strip along each edge is.
///
/// Wide enough to hit without aiming — a one-pixel target is a resize handle
/// only in principle.
const RESIZE_GRAB: f32 = 5.0;

/// The titlebar, or nothing at all where the compositor draws its own.
///
/// Returns an `AnyElement` rather than branching inside a builder, because the
/// two arms are genuinely different element types and erasing them here is
/// cheaper than making the empty case pretend to be a titlebar.
pub fn render(view: &BoardView, window: &Window, cx: &mut Context<BoardView>) -> gpui::AnyElement {
    if !matches!(window.window_decorations(), Decorations::Client { .. }) {
        return div().into_any_element();
    }
    let controls = window.window_controls();
    let theme = &view.theme;

    let title = if view.doc.board.title.is_empty() {
        "mbrd".to_string()
    } else {
        format!("{} — mbrd", view.doc.board.title)
    };

    div()
        .id("titlebar")
        .flex()
        .items_center()
        .justify_between()
        .h(px(TITLEBAR_HEIGHT))
        .flex_shrink_0()
        .pl(px(14.0))
        .bg(theme.chrome)
        .border_b_1()
        .border_color(theme.chrome_edge)
        .text_size(px(12.0))
        .text_color(theme.muted)
        // Dragging the bar moves the window, and a double-click maximises it.
        // Both are conventions the compositor would have provided; a
        // client-side titlebar that only looks like one is worse than none.
        .on_mouse_down(MouseButton::Left, |event, window, _cx| {
            if event.click_count == 2 {
                window.zoom_window();
            } else {
                window.start_window_move();
            }
        })
        .child(title)
        .child(
            div()
                .flex()
                .items_center()
                .when(controls.minimize, |d| {
                    d.child(control("minimise", "\u{2013}", theme.muted, cx, |window, _cx| {
                        window.minimize_window()
                    }))
                })
                .when(controls.maximize, |d| {
                    d.child(control("maximise", "\u{25a1}", theme.muted, cx, |window, _cx| {
                        window.zoom_window()
                    }))
                })
                .child(control("close", "\u{00d7}", theme.accent, cx, |window, _cx| {
                    window.remove_window()
                })),
        )
        .into_any_element()
}

fn control(
    id: &'static str,
    glyph: &'static str,
    hover: gpui::Hsla,
    cx: &mut Context<BoardView>,
    action: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let _ = cx;
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .w(px(44.0))
        .h(px(TITLEBAR_HEIGHT))
        .text_size(px(14.0))
        .hover(move |s| s.bg(hover.opacity(0.18)))
        .active(move |s| s.bg(hover.opacity(0.34)))
        // Mouse-down rather than click, and it matters here: the bar itself
        // starts a window move on mouse-down, and an interactive child has to
        // claim the press before that happens or every button press would drag
        // the window a pixel first.
        .on_mouse_down(MouseButton::Left, move |_event, window, cx| action(window, cx))
        .child(glyph)
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
