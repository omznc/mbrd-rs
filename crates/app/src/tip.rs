//! The little label a control shows when the pointer rests on it.
//!
//! A tooltip is the answer to one question — *what is this, and what key does
//! it* — and it is the only place in this app where a control gets to explain
//! itself without spending room on the explanation. That matters most for the
//! wordless buttons in the titlebar: an icon nobody can name is a control
//! nobody presses, and the key beside the name is what stops the button from
//! being the *only* way somebody ever reaches the thing behind it.
//!
//! **The key comes from the command table.** Every tip that names one is
//! passed `Command::hint()` rather than a string written here, for the reason
//! `command.rs` opens with: two copies of a keystroke drift, and the drift is
//! invisible until somebody presses what the label promised.
//!
//! GPUI has the machinery — [`gpui::InteractiveElement::tooltip`] takes a
//! builder and handles the half-second delay, the placement and the dismissal —
//! but it wants an `AnyView`, so a tooltip is a tiny entity with a `Render` of
//! its own rather than an element. Hence a module: one view and one helper, so
//! that every hover hint in the app is the same two lines at the call site and
//! the same shape on screen.

use gpui::{div, prelude::*, px, AnyView, App, Context, SharedString, Window};

use crate::theme::Theme;

/// An open tip: what the control is called, and the key that does it.
pub struct Tip {
    what: SharedString,
    /// The keystroke, or empty for a control no key reaches — which is drawn
    /// as nothing at all rather than as an empty chip, the same rule the menu
    /// follows for a command with no key.
    keys: SharedString,
    theme: Theme,
}

impl Render for Tip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = self.theme;
        let keys = self.keys.clone();

        div()
            .flex()
            .items_center()
            .gap(px(8.0))
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(crate::theme::RADIUS_MD))
            .bg(theme.chrome)
            .border_1()
            .border_color(theme.chrome_edge)
            .shadow(theme.shadow_small())
            .text_size(px(11.0))
            .text_color(theme.text)
            .child(self.what.clone())
            .when(!keys.is_empty(), |d| {
                d.child(div().text_size(px(10.0)).text_color(theme.muted).child(keys))
            })
    }
}

/// A tip, ready to hand to [`gpui::InteractiveElement::tooltip`].
///
/// Returns a builder rather than a view because that is what GPUI asks for: the
/// tooltip is built the moment it is shown and dropped when it goes away, so a
/// view made here and held would be one entity per button alive for the life of
/// the window whether anybody hovered it or not.
pub fn tip(
    theme: Theme,
    what: impl Into<SharedString>,
    keys: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let what = what.into();
    let keys = keys.into();
    move |_window, cx| {
        let (what, keys) = (what.clone(), keys.clone());
        cx.new(|_cx| Tip { what, keys, theme }).into()
    }
}
