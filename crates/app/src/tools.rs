//! What the pointer means right now, and the strip that says so.
//!
//! A tool is a *mode*, which is the thing this app has otherwise been careful
//! not to have — so it earns its place by paying for itself twice: it makes the
//! gestures that were already hidden behind a modifier or a middle button
//! visible and reachable, and it makes the repeated ones repeatable. Drawing
//! nine ropes in a row should not mean nine trips to a card's edge.
//!
//! **[`Tool::Select`] is what the app was before this module existed**, and it
//! is the tool it starts in and returns to. Everything the other three do is
//! also possible from Select — middle-drag pans, an anchor starts a rope, `N`
//! makes a note — which is the rule that keeps the mode from being a trap: no
//! tool is the only way to do anything.
//!
//! It is deliberately not a [`Command`](crate::command::Command). A command is
//! something you *do* and a tool is somewhere you *are*; a list that mixed the
//! two would have to explain, for every entry, which of those it was.

use gpui::{div, prelude::*, px, Context, Modifiers, MouseButton};

use crate::board_view::BoardView;

/// What a press on the board means.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    /// Press to select, drag to move, drag empty space to pan. The default,
    /// and the one every other tool is a shortcut out of.
    #[default]
    Select,
    /// Drag anywhere to move the camera, over a card or not.
    Pan,
    /// Drag from one card to another to join them.
    Connect,
    /// Press to put a sticky note down where you pressed.
    Note,
}

impl Tool {
    /// Every one, in the order the strip shows them.
    ///
    /// Select first because it is the default and the way back; the other three
    /// in the order somebody works — move around, join things up, write
    /// something down.
    pub const ALL: [Tool; 4] = [Tool::Select, Tool::Pan, Tool::Connect, Tool::Note];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Select => "Select",
            Tool::Pan => "Pan",
            Tool::Connect => "Connect",
            Tool::Note => "Note",
        }
    }

    /// The key that reaches it, spelled the way the strip shows it.
    pub fn hint(self) -> &'static str {
        match self {
            Tool::Select => "1",
            Tool::Pan => "2",
            Tool::Connect => "3",
            Tool::Note => "4",
        }
    }

    /// What the status bar says while you are in it.
    ///
    /// `None` for Select, because Select is not somewhere you are — it is the
    /// absence of being anywhere, and a permanent readout saying "select" would
    /// be a line that is true and useless all day.
    pub fn hint_line(self) -> Option<&'static str> {
        match self {
            Tool::Select => None,
            Tool::Pan => Some("pan — drag anywhere; escape for select"),
            Tool::Connect => Some("connect — drag from one card to another; escape for select"),
            Tool::Note => Some("note — press to put one down; escape for select"),
        }
    }

    /// Which key press means this, if any.
    ///
    /// Digits, and the letters the rest of the world uses for the same three
    /// tools. The digits are the ones the strip advertises because they are the
    /// ones that will not collide with a command later — `n` is already a note
    /// and `s` is already snapping, and a tool that stole a letter back from a
    /// command would be a shortcut that changed meaning depending on a mode,
    /// which is the worst kind.
    pub fn for_key(key: &str, mods: Modifiers) -> Option<Self> {
        if mods.modified() {
            return None;
        }
        Some(match key {
            "1" | "v" => Tool::Select,
            "2" | "h" => Tool::Pan,
            "3" | "c" => Tool::Connect,
            "4" => Tool::Note,
            _ => return None,
        })
    }
}

/// The strip, top left, floating over the board.
///
/// Words rather than pictures, for two reasons. The honest one is that a
/// wordless icon for "connect" is a thing you learn rather than read. The other
/// is that this window already draws its own titlebar and its own menu because
/// the platform's could not be relied on; a row of dingbats would be a fifth
/// thing relying on whatever glyphs the font on this machine happens to carry.
///
/// Small and out of the way on purpose. The one strip of chrome along the
/// bottom is there because a permanent panel is a permanent piece of the board
/// you cannot see, and the same argument applies here.
pub fn render(view: &BoardView, cx: &mut Context<BoardView>) -> impl IntoElement {
    let theme = view.theme;
    let current = view.tool;

    div()
        .absolute()
        .top(px(10.0))
        .left(px(10.0))
        .flex()
        .items_center()
        .gap(px(2.0))
        .p(px(3.0))
        .rounded(px(8.0))
        .bg(theme.chrome)
        .border_1()
        .border_color(theme.chrome_edge)
        .shadow_lg()
        .text_size(px(11.0))
        // The canvas beneath listens on mouse-down, so without this a press on
        // the strip would also start a gesture on the board behind it — which
        // for the Pan tool would mean choosing it and immediately panning.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .children(Tool::ALL.into_iter().enumerate().map(|(i, tool)| {
            let chosen = tool == current;
            div()
                .id(i)
                .flex()
                .items_center()
                .gap(px(5.0))
                .h(px(22.0))
                .px(px(8.0))
                .rounded(px(5.0))
                .when(chosen, |d| d.bg(theme.accent.opacity(0.22)))
                .text_color(if chosen { theme.text } else { theme.muted })
                .hover(|s| s.bg(theme.accent.opacity(if chosen { 0.28 } else { 0.12 })))
                // The press, acknowledged on the press. Choosing a tool is
                // instant and the strip redraws with the new one lit, but that
                // is the *result* — this is the button saying it heard you,
                // which is a different thing and has to happen first.
                .active(|s| s.bg(theme.accent.opacity(0.4)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event, _window, cx| {
                        this.choose_tool(tool, cx);
                    }),
                )
                .child(tool.label())
                .child(div().text_size(px(9.0)).text_color(theme.muted).child(tool.hint()))
        }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_key_a_tool_advertises_is_a_key_that_reaches_it() {
        for tool in Tool::ALL {
            assert_eq!(
                Tool::for_key(tool.hint(), Modifiers::default()),
                Some(tool),
                "{} says {}, which lands somewhere else",
                tool.label(),
                tool.hint(),
            );
        }
    }

    #[test]
    fn no_two_tools_claim_the_same_key() {
        for (i, a) in Tool::ALL.iter().enumerate() {
            for b in &Tool::ALL[i + 1..] {
                assert_ne!(a.hint(), b.hint(), "{} and {} share a key", a.label(), b.label());
            }
        }
    }

    #[test]
    fn no_tool_takes_a_key_a_command_already_answers_to() {
        // The rule this module's `for_key` explains, kept honest. A key that
        // meant a tool here and a command there would do one thing or the other
        // depending on which handler ran first, which is not something anybody
        // could learn.
        for key in ["1", "v", "2", "h", "3", "c", "4"] {
            assert_eq!(
                crate::command::Command::for_key(key, Modifiers::default()),
                None,
                "{key} is already a command",
            );
        }
    }

    #[test]
    fn a_modified_press_is_never_a_tool() {
        // `Ctrl V` is paste and `Ctrl C` is copy, and neither of them should
        // land somebody in a different tool on the way past.
        assert_eq!(Tool::for_key("v", Modifiers::secondary_key()), None);
        assert_eq!(Tool::for_key("c", Modifiers::secondary_key()), None);
    }

    #[test]
    fn select_is_where_the_app_starts() {
        assert_eq!(Tool::default(), Tool::Select);
        assert_eq!(Tool::default().hint_line(), None, "the default is not a state to announce");
    }
}
