//! Whether there is room to draw a board at all.
//!
//! Every desktop this app runs on is handed a floor — `MIN_SIZE` in `main.rs`,
//! given to the compositor as `xdg_toplevel.set_min_size` or a
//! `WM_SIZE_HINTS` — so the drag that would make a window too small stops at
//! the edge and this file never has anything to say. A browser has no such
//! thing. A tab is whatever size the window is, a phone is whatever size the
//! phone is, and neither will take an instruction about it.
//!
//! So the web gets the same floor by the only means a page has: it stops
//! drawing the board and says so. **This is deliberately not a smaller
//! layout.** Below `MIN_SIZE` the chrome starts colliding with itself — the
//! tool strip is 350 wide and floats over the top left, the right-click menu
//! is 216 and fits itself to what is left, and the titlebar carries five
//! buttons and a board name — and the honest answer to a window that cannot
//! hold those is that it cannot, rather than a board with three of its
//! controls stacked on top of each other.
//!
//! It is not a refusal either. The moment the window is big enough the board
//! is there, with everything on it: nothing is torn down and nothing is
//! reloaded, because this is one branch at the top of a render and the board
//! behind it never went anywhere.

use gpui::{div, prelude::*, px, Window};

use crate::theme::Theme;

/// The panel, where there is not enough room for a board.
///
/// `None` on every window that is big enough, and on every platform but the
/// web — see the module note, which is about why the two are the same
/// question with two different answers.
pub fn cramped(window: &Window, theme: &Theme) -> Option<gpui::AnyElement> {
    if !cfg!(target_family = "wasm") {
        return None;
    }

    let have = window.viewport_size();
    let want = crate::MIN_SIZE;
    if have.width >= want.width && have.height >= want.height {
        return None;
    }

    // Rounded rather than printed as they are: a viewport is a float, and
    // "393.5 × 780.0" is a number nobody can do anything with.
    let round = |value: gpui::Pixels| f32::from(value).round() as i32;
    let (w, h) = (round(have.width), round(have.height));
    let (nw, nh) = (round(want.width), round(want.height));

    Some(
        div()
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(10.0))
            .p(px(24.0))
            .bg(theme.ground)
            .text_color(theme.text)
            .child(
                div()
                    .text_size(px(20.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .child("There is not enough room"),
            )
            .child(
                div()
                    .max_w(px(320.0))
                    .text_size(px(13.0))
                    .text_center()
                    .text_color(theme.muted)
                    .child("A board needs more of a screen than this. Make the window bigger, or open mbrd on a computer."),
            )
            .child(
                // The two numbers, because "too small" on its own is not
                // something anybody can act on: this says how much bigger, and
                // it changes as the window is dragged.
                div()
                    .mt(px(4.0))
                    .text_size(px(11.5))
                    .font(crate::opened::mono())
                    .text_color(theme.tertiary)
                    .child(format!("{w} \u{00d7} {h} \u{2014} needs {nw} \u{00d7} {nh}")),
            )
            .into_any_element(),
    )
}
