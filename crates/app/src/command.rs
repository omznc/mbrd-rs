//! Everything the app can be asked to do, named once.
//!
//! The keyboard, the context menu and — later — anything that searches for a
//! thing to do all need the same list: what it is called, what it does, whether
//! it is available right now, and what key does it. Written three times, those
//! drift, and the drift is invisible: a menu entry that says `Ctrl+Y` for a key
//! that stopped doing that, an entry offered on an empty selection that then
//! does nothing.
//!
//! So this is the list, and the three of them read it. Adding something the app
//! can do means adding a variant and filling in four small functions; forgetting
//! any of them will not compile.
//!
//! It is deliberately *not* GPUI's action system. That is built for a keymap
//! the user edits and a dispatch tree that routes by focus, and this app has
//! neither — one view, one focus, one keymap. What it needs instead is the
//! thing GPUI's actions do not give: a value it can put in a list and draw.

use gpui::{Context, Modifiers, Window};
use mbrd_core::align::{Axis, Edge};
use mbrd_core::model::{ConnColor, ConnDir, ConnStyle, ConnWeight};

use crate::board_view::BoardView;

/// One thing the app can be asked to do.
///
/// Most are nullary; the four that carry a value are the ones that set a
/// connection's appearance, and they carry it for the reason a menu wants:
/// "make this rope green" is one command with five spellings rather than five
/// commands, so the menu builds its row of colours by mapping over the enum the
/// *format* defines instead of over a list here that could fall behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    AddNote,
    AddSwatch,
    Tint,
    BringToFront,
    SendToBack,
    Rename,
    Duplicate,
    Copy,
    Cut,
    Delete,
    SelectAll,
    ClearSelection,
    Undo,
    Redo,
    Paste,
    Save,
    Recentre,
    FitBoard,
    ToggleGrid,
    ToggleAxes,
    ToggleSnap,
    ToggleWeb,
    OpenBoard,
    /// Join everything selected with the fewest lines that reach all of it.
    Connect,
    /// Put a labelled rectangle around what is selected. Membership is
    /// measured from where the cards are, so there is nothing else to do.
    AddFence,
    /// Take a sticky note off the card it is pinned to.
    Unstick,

    // Arranging. Every one of them carries the axis or the edge it is about,
    // for the reason the connection commands do: a menu builds its row by
    // mapping over the enum rather than over a list here that could fall
    // behind it.
    Align(Edge),
    Distribute(Axis),
    /// Push overlapping cards off each other.
    Separate,

    // The connection commands. Every one of them is about the rope that is
    // selected, and every one is unavailable when none is — see `available`.
    /// Type a word onto the middle of it.
    ConnLabel,
    /// Take it off the board. Not the cards it joined.
    ConnDelete,
    ConnColour(ConnColor),
    ConnArrow(ConnDir),
    ConnStyleAs(ConnStyle),
    ConnWeightAs(ConnWeight),
}

impl Command {
    /// What it is called, in a menu.
    ///
    /// Sentence case and a verb, because every one of these is something you
    /// are about to do rather than a place you are about to be.
    pub fn label(self) -> &'static str {
        match self {
            Self::AddNote => "Add note",
            Self::AddSwatch => "Add color",
            Self::Tint => "Next tint",
            Self::BringToFront => "Bring to front",
            Self::SendToBack => "Send to back",
            Self::Rename => "Rename",
            Self::Duplicate => "Duplicate",
            Self::Copy => "Copy",
            Self::Cut => "Cut",
            Self::Delete => "Move to bin",
            Self::SelectAll => "Select all",
            Self::ClearSelection => "Select none",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::Paste => "Paste",
            Self::Save => "Save",
            Self::Recentre => "Recenter",
            Self::FitBoard => "Fit board",
            Self::ToggleGrid => "Grid",
            Self::ToggleAxes => "Axes",
            Self::ToggleSnap => "Snap to grid",
            Self::ToggleWeb => "Connections",
            Self::OpenBoard => "Open board…",
            Self::Connect => "Connect",
            Self::AddFence => "Add fence",
            Self::Unstick => "Unstick note",
            Self::Align(edge) => match edge {
                Edge::Left => "Align left",
                Edge::CentreX => "Align centers",
                Edge::Right => "Align right",
                Edge::Top => "Align tops",
                Edge::Middle => "Align middles",
                Edge::Bottom => "Align bottoms",
            },
            Self::Distribute(axis) => match axis {
                Axis::Horizontal => "Space across",
                Axis::Vertical => "Space down",
            },
            Self::Separate => "Push apart",

            Self::ConnLabel => "Label…",
            Self::ConnDelete => "Remove connection",
            // Named after the colour rather than after what it does to it —
            // the menu ticks the one that is on, so a row reads as a choice
            // already made rather than as five separate instructions.
            Self::ConnColour(colour) => match colour {
                ConnColor::Line => "Plain",
                ConnColor::Accent => "Accent",
                ConnColor::Warm => "Warm",
                ConnColor::Leaf => "Leaf",
                ConnColor::Danger => "Danger",
            },
            Self::ConnArrow(dir) => match dir {
                ConnDir::None => "No arrow",
                ConnDir::Fwd => "Arrow forward",
                ConnDir::Back => "Arrow back",
                ConnDir::Both => "Arrows both ways",
            },
            Self::ConnStyleAs(style) => match style {
                ConnStyle::Solid => "Solid",
                ConnStyle::Dashed => "Dashed",
                ConnStyle::Dotted => "Dotted",
            },
            Self::ConnWeightAs(weight) => match weight {
                ConnWeight::Fine => "Fine",
                ConnWeight::Normal => "Normal",
                ConnWeight::Bold => "Bold",
            },
        }
    }

    /// The key that does it, spelled the way a menu should show it.
    ///
    /// `""` for a command no key reaches, and that is a real answer rather than
    /// an omission: the connection commands are a menu of choices about the one
    /// thing that is selected, and giving each of five colours a letter would
    /// spend five of the alphabet on something nobody would learn. The menu
    /// draws an empty hint as nothing at all.
    pub fn hint(self) -> &'static str {
        match self {
            Self::AddNote => "N",
            Self::AddSwatch => "K",
            Self::Tint => "T",
            Self::BringToFront => "]",
            Self::SendToBack => "[",
            Self::Rename => "F2",
            Self::Duplicate => "Ctrl D",
            Self::Copy => "Ctrl C",
            Self::Cut => "Ctrl X",
            Self::Delete => "Delete",
            Self::SelectAll => "Ctrl A",
            Self::ClearSelection => "Escape",
            Self::Undo => "Ctrl Z",
            Self::Redo => "Ctrl Shift Z",
            Self::Paste => "Ctrl V",
            Self::Save => "Ctrl S",
            Self::Recentre => "0",
            Self::FitBoard => "F",
            Self::ToggleGrid => "G",
            Self::ToggleAxes => "X",
            Self::ToggleSnap => "S",
            Self::ToggleWeb => "W",
            Self::OpenBoard => "Ctrl P",
            Self::Connect => "J",
            Self::AddFence => "E",
            Self::Unstick => "U",
            Self::Align(_) | Self::Distribute(_) | Self::Separate => "",

            Self::ConnLabel
            | Self::ConnDelete
            | Self::ConnColour(_)
            | Self::ConnArrow(_)
            | Self::ConnStyleAs(_)
            | Self::ConnWeightAs(_) => "",
        }
    }

    /// Which key press means this, if any.
    ///
    /// The other half of [`Self::hint`], and the reason they are next to each
    /// other: a menu that promises a key the handler does not answer to is a
    /// lie the compiler cannot catch, so at least keep the two in one place.
    pub fn for_key(key: &str, mods: Modifiers) -> Option<Self> {
        let plain = !mods.modified();
        Some(match key {
            "n" if plain => Self::AddNote,
            "k" if plain => Self::AddSwatch,
            "t" if plain => Self::Tint,
            "f" if plain => Self::FitBoard,
            "g" if plain => Self::ToggleGrid,
            "x" if plain => Self::ToggleAxes,
            "s" if plain => Self::ToggleSnap,
            "w" if plain => Self::ToggleWeb,
            "0" if plain => Self::Recentre,
            "j" if plain => Self::Connect,
            "e" if plain => Self::AddFence,
            "u" if plain => Self::Unstick,
            "]" if plain => Self::BringToFront,
            "[" if plain => Self::SendToBack,
            // Enter as well as F2: F2 is the one to learn and Enter is the
            // one everybody tries first.
            "f2" | "enter" => Self::Rename,
            "d" if mods.secondary() => Self::Duplicate,
            "c" if mods.secondary() => Self::Copy,
            "x" if mods.secondary() => Self::Cut,
            "escape" => Self::ClearSelection,
            "delete" | "backspace" => Self::Delete,
            "a" if mods.secondary() => Self::SelectAll,
            "s" if mods.secondary() => Self::Save,
            "v" if mods.secondary() => Self::Paste,
            "p" if mods.secondary() => Self::OpenBoard,
            // Shift first, or the plain-undo arm would swallow both.
            "z" if mods.secondary() && mods.shift => Self::Redo,
            "z" if mods.secondary() => Self::Undo,
            "y" if mods.secondary() => Self::Redo,
            _ => return None,
        })
    }

    /// Whether doing it right now would achieve anything.
    ///
    /// A menu draws an unavailable command dimmed rather than hiding it, so
    /// that the menu does not change shape as you work — a list whose entries
    /// move is a list you have to read every time instead of aiming at.
    pub fn available(self, view: &BoardView) -> bool {
        let selected = !view.selection.is_empty();
        let roped = view.rope.is_some();
        match self {
            // The bin takes whichever of the two is selected, so it is
            // available for either. Everything else in this arm is about a
            // card and only a card.
            Self::Delete => selected || roped,
            Self::ConnLabel
            | Self::ConnDelete
            | Self::ConnColour(_)
            | Self::ConnArrow(_)
            | Self::ConnStyleAs(_)
            | Self::ConnWeightAs(_) => roped,
            Self::BringToFront
            | Self::SendToBack
            | Self::ClearSelection
            | Self::Rename
            | Self::Duplicate
            | Self::Copy
            | Self::Cut
            | Self::Tint => selected,
            // Two cards at the least: one card is not something to join, and
            // one card is already aligned with itself.
            Self::Connect | Self::Align(_) | Self::Separate => view.selection.len() >= 2,
            // Three, for spacing: with two, the gap between them is the only
            // gap there is and it is already even.
            Self::Distribute(_) => view.selection.len() >= 3,
            Self::Unstick => view.can_unstick(),
            Self::Undo => view.undo_step().is_some(),
            Self::Redo => view.redo_step().is_some(),
            Self::SelectAll => view.doc.board.items.iter().any(|i| i.kind.is_content()),
            _ => true,
        }
    }

    /// Whether it is a setting that is currently on, for a menu to tick.
    ///
    /// The connection commands answer this too, and it is what turns four rows
    /// of choices into four rows that show what the rope already *is* — a menu
    /// you can read the current state off rather than one you have to remember
    /// what you last did to.
    pub fn ticked(self, view: &BoardView) -> Option<bool> {
        let settings = &view.doc.board.settings.desktop;
        match self {
            Self::ToggleGrid => Some(settings.grid),
            Self::ToggleAxes => Some(settings.axes),
            Self::ToggleSnap => Some(settings.snap),
            Self::ToggleWeb => Some(settings.web),
            Self::ConnColour(colour) => Some(view.rope_meta()?.color == colour),
            Self::ConnArrow(dir) => Some(view.rope_meta()?.dir == dir),
            Self::ConnStyleAs(style) => Some(view.rope_meta()?.style == style),
            Self::ConnWeightAs(weight) => Some(view.rope_meta()?.weight == weight),
            _ => None,
        }
    }

    /// Do it.
    pub fn run(self, view: &mut BoardView, window: &mut Window, cx: &mut Context<BoardView>) {
        match self {
            Self::AddNote => view.add_note(cx),
            Self::AddSwatch => view.add_swatch(cx),
            Self::Tint => view.cycle_tint(cx),
            Self::BringToFront => view.raise_selection(true, cx),
            Self::SendToBack => view.raise_selection(false, cx),
            Self::Rename => view.rename(cx),
            Self::Duplicate => view.duplicate_selection(cx),
            Self::Copy => view.copy_selection(false, cx),
            Self::Cut => view.copy_selection(true, cx),
            Self::Delete => view.delete_selection(cx),
            Self::SelectAll => view.select_all(cx),
            Self::ClearSelection => view.clear_selection(cx),
            Self::Undo => view.undo(cx),
            Self::Redo => view.redo(cx),
            Self::Paste => view.paste(cx),
            Self::Save => view.save(cx),
            Self::Recentre => view.go_home(cx),
            Self::FitBoard => view.fit_all(cx),
            Self::ToggleGrid => view.toggle_setting(Self::ToggleGrid, cx),
            Self::ToggleAxes => view.toggle_setting(Self::ToggleAxes, cx),
            Self::ToggleSnap => view.toggle_setting(Self::ToggleSnap, cx),
            Self::ToggleWeb => view.toggle_setting(Self::ToggleWeb, cx),
            Self::OpenBoard => view.open_switcher(window, cx),
            Self::Connect => view.connect_selection(cx),
            Self::AddFence => view.add_fence(cx),
            Self::Unstick => view.unstick(cx),
            Self::Align(edge) => view.arrange(Self::Align(edge), cx),
            Self::Distribute(axis) => view.arrange(Self::Distribute(axis), cx),
            Self::Separate => view.arrange(Self::Separate, cx),

            Self::ConnLabel => view.start_labelling(cx),
            Self::ConnDelete => view.delete_rope(cx),
            // Four axes, one door. `dress` takes the change as a closure over
            // the connection's `meta`, so adding a fifth axis the format grows
            // is a line here and nothing anywhere else.
            Self::ConnColour(colour) => view.dress("Recolor", cx, |meta| meta.color = colour),
            Self::ConnArrow(dir) => view.dress("Point", cx, |meta| meta.dir = dir),
            Self::ConnStyleAs(style) => view.dress("Restyle", cx, |meta| meta.style = style),
            Self::ConnWeightAs(weight) => view.dress("Reweight", cx, |meta| meta.weight = weight),
        }
    }
}

// ---------------------------------------------------------------------------
// The lists
// ---------------------------------------------------------------------------

/// One line of a menu.
///
/// A list of these rather than a list of commands, because two of the three
/// things a line can be are not commands: a rule divides, and a submenu holds.
/// The lists were `Option<Command>` before, which could say "nothing here" but
/// had no way to say "more here" — and a menu that shows everything on one
/// face is a menu you read down rather than aim at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Entry {
    /// A line across, dividing what is above it from what is below.
    Rule,
    /// Something to do.
    Does(Command),
    /// A list that opens beside this one, under a name.
    ///
    /// A noun rather than a verb — "Add", "View", "Colour" — because a submenu
    /// is not something you do, it is where the doing is kept.
    More(&'static str, &'static [Entry]),
}

impl Entry {
    /// Whether the row is worth pressing.
    ///
    /// A submenu is worth opening if anything inside it is; one whose every
    /// entry is unavailable draws dimmed and does not open, which is better
    /// than opening onto a list of grey.
    pub fn available(self, view: &BoardView) -> bool {
        match self {
            Self::Rule => false,
            Self::Does(command) => command.available(view),
            Self::More(_, list) => list.iter().any(|e| e.available(view)),
        }
    }

    /// What the row says down its right-hand side.
    ///
    /// A command's key, and for a submenu the one entry inside it that is
    /// ticked — so the rope's colour reads off the closed menu as `Colour
    /// Accent` and opening it confirms rather than discovers. That is the whole
    /// bargain of moving the choices inside: a submenu that hid what it held
    /// would be four fewer rows and four facts lost.
    ///
    /// Only where the list is a *single* choice, which is what the count asks:
    /// the view submenu holds four switches that are on and off independently,
    /// and naming whichever happened to be first would be a readout of nothing.
    pub fn hint(self, view: &BoardView) -> &'static str {
        match self {
            Self::Rule => "",
            Self::Does(command) => command.hint(),
            Self::More(_, list) => {
                let (mut on, mut chosen) = (0, "");
                for entry in list {
                    match entry {
                        Self::Rule => {}
                        Self::Does(command) => match command.ticked(view) {
                            Some(true) => {
                                on += 1;
                                chosen = command.label();
                            }
                            Some(false) => {}
                            // Something in there is not a setting, so the list
                            // is not one choice and has no single answer.
                            None => return "",
                        },
                        Self::More(..) => return "",
                    }
                }
                if on == 1 {
                    chosen
                } else {
                    ""
                }
            }
        }
    }
}

/// Everything that puts something new on the board.
///
/// Three verbs that were three rows on every list; one row now, on all of
/// them. Adding is a thing you do occasionally and read past constantly, which
/// is exactly what a submenu is for.
const ADD: [Entry; 3] = [
    Entry::Does(Command::AddNote),
    Entry::Does(Command::AddSwatch),
    Entry::Does(Command::AddFence),
];

/// What is drawn, and the two ways of getting back to it.
const VIEW: [Entry; 7] = [
    Entry::Does(Command::ToggleGrid),
    Entry::Does(Command::ToggleSnap),
    Entry::Does(Command::ToggleAxes),
    Entry::Does(Command::ToggleWeb),
    Entry::Rule,
    Entry::Does(Command::FitBoard),
    Entry::Does(Command::Recentre),
];

/// Everything about where several cards are in relation to each other.
const ARRANGE: [Entry; 10] = [
    Entry::Does(Command::Align(Edge::Left)),
    Entry::Does(Command::Align(Edge::CentreX)),
    Entry::Does(Command::Align(Edge::Right)),
    Entry::Does(Command::Align(Edge::Top)),
    Entry::Does(Command::Align(Edge::Middle)),
    Entry::Does(Command::Align(Edge::Bottom)),
    Entry::Rule,
    Entry::Does(Command::Distribute(Axis::Horizontal)),
    Entry::Does(Command::Distribute(Axis::Vertical)),
    Entry::Does(Command::Separate),
];

// The four axes a connection has. Every one of them is a single choice, so
// every one of them names what it is set to on the row that opens it — see
// [`Entry::hint`].
const ARROWS: [Entry; 4] = [
    Entry::Does(Command::ConnArrow(ConnDir::None)),
    Entry::Does(Command::ConnArrow(ConnDir::Fwd)),
    Entry::Does(Command::ConnArrow(ConnDir::Back)),
    Entry::Does(Command::ConnArrow(ConnDir::Both)),
];

const COLOURS: [Entry; 5] = [
    Entry::Does(Command::ConnColour(ConnColor::Line)),
    Entry::Does(Command::ConnColour(ConnColor::Accent)),
    Entry::Does(Command::ConnColour(ConnColor::Warm)),
    Entry::Does(Command::ConnColour(ConnColor::Leaf)),
    Entry::Does(Command::ConnColour(ConnColor::Danger)),
];

const STYLES: [Entry; 3] = [
    Entry::Does(Command::ConnStyleAs(ConnStyle::Solid)),
    Entry::Does(Command::ConnStyleAs(ConnStyle::Dashed)),
    Entry::Does(Command::ConnStyleAs(ConnStyle::Dotted)),
];

const WEIGHTS: [Entry; 3] = [
    Entry::Does(Command::ConnWeightAs(ConnWeight::Fine)),
    Entry::Does(Command::ConnWeightAs(ConnWeight::Normal)),
    Entry::Does(Command::ConnWeightAs(ConnWeight::Bold)),
];

/// What a right-click offers when **nothing** is in hand.
///
/// A fourth list rather than the card list with two thirds of it dimmed. A
/// right-click on bare paper is not a right-click on a card that happens to be
/// missing: rename, duplicate, cut, the bin and the tint are all about a card,
/// and a menu of them greyed out would be a menu that said mostly "not this".
///
/// Reaching it *lets go* of whatever was selected — see `on_mouse_down` —
/// because a menu about the board over a board with three cards selected would
/// be a menu whose every entry meant something other than what it said.
pub const BOARD_MENU: [Entry; 8] = [
    Entry::More("Add", &ADD),
    Entry::Does(Command::Paste),
    Entry::Does(Command::SelectAll),
    Entry::Rule,
    Entry::Does(Command::Undo),
    Entry::Does(Command::Redo),
    Entry::Rule,
    Entry::More("View", &VIEW),
];

/// What a right-click offers when a **card** is what is in hand.
///
/// Selection-only commands first, because a right-click that landed on a card
/// is usually about that card; the board-wide ones are below the rule.
pub const CARD_MENU: [Entry; 21] = [
    Entry::Does(Command::Rename),
    Entry::Does(Command::Duplicate),
    Entry::Rule,
    Entry::Does(Command::Cut),
    Entry::Does(Command::Copy),
    Entry::Does(Command::Paste),
    Entry::Does(Command::Delete),
    Entry::Rule,
    Entry::More("Add", &ADD),
    Entry::Does(Command::Tint),
    Entry::Does(Command::Unstick),
    Entry::Does(Command::BringToFront),
    Entry::Does(Command::SendToBack),
    Entry::Rule,
    Entry::Does(Command::SelectAll),
    Entry::Does(Command::ClearSelection),
    Entry::Rule,
    Entry::Does(Command::Undo),
    Entry::Does(Command::Redo),
    Entry::Rule,
    Entry::More("View", &VIEW),
];

/// What a right-click offers when a **connection** is what is in hand.
///
/// A different list rather than the same one with half of it dimmed. A rope is
/// not a card with fewer options — nothing on the card list applies to it at
/// all — and a menu that was three quarters grey would be a menu that said
/// mostly "not this".
///
/// Two things to do and four things it *is*. The four are submenus rather than
/// nineteen rows of ticks, and each one names its own setting on the row that
/// opens it, so the closed menu is still the readout it always was.
pub const ROPE_MENU: [Entry; 7] = [
    Entry::Does(Command::ConnLabel),
    Entry::Does(Command::ConnDelete),
    Entry::Rule,
    Entry::More("Arrow", &ARROWS),
    Entry::More("Color", &COLOURS),
    Entry::More("Style", &STYLES),
    Entry::More("Weight", &WEIGHTS),
];

/// What a right-click offers when **several cards** are what is in hand.
///
/// A third list rather than the card list with an "Arrange" section bolted on.
/// Several cards is a genuinely different thing to have in hand: half the card
/// list — rename, tint, the note's own commands — is about one of them, and all
/// of this is about the relationship between them, which does not exist when
/// there is only one.
pub const MANY_MENU: [Entry; 13] = [
    Entry::Does(Command::Connect),
    Entry::Does(Command::Duplicate),
    Entry::Rule,
    Entry::Does(Command::Cut),
    Entry::Does(Command::Copy),
    Entry::Does(Command::Delete),
    Entry::Rule,
    Entry::More("Arrange", &ARRANGE),
    Entry::Does(Command::BringToFront),
    Entry::Does(Command::SendToBack),
    Entry::Rule,
    Entry::More("Add", &ADD),
    Entry::Does(Command::ClearSelection),
];

/// Which list a right-click should show.
///
/// Decided by what is in hand rather than by where the press landed, because
/// selecting a rope already cleared the cards and vice versa — so "what is
/// selected" and "what you just pressed" are the same question by the time the
/// menu opens.
pub fn menu_for(view: &BoardView) -> &'static [Entry] {
    match view.selection.len() {
        _ if view.rope.is_some() => &ROPE_MENU,
        0 => &BOARD_MENU,
        1 => &CARD_MENU,
        _ => &MANY_MENU,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every command, for the exhaustiveness checks below. Adding a variant
    /// without adding it here fails the first test rather than going unnoticed.
    const ALL: [Command; 22] = [
        Command::AddNote,
        Command::AddSwatch,
        Command::Tint,
        Command::BringToFront,
        Command::SendToBack,
        Command::Rename,
        Command::Duplicate,
        Command::Copy,
        Command::Cut,
        Command::Delete,
        Command::SelectAll,
        Command::ClearSelection,
        Command::Undo,
        Command::Redo,
        Command::Paste,
        Command::Save,
        Command::Recentre,
        Command::FitBoard,
        Command::ToggleGrid,
        Command::ToggleAxes,
        Command::ToggleSnap,
        Command::OpenBoard,
    ];

    #[test]
    fn every_key_a_command_advertises_is_a_key_that_reaches_it() {
        for command in ALL {
            let hint = command.hint();
            let key = hint.rsplit(' ').next().unwrap().to_lowercase();
            let mut mods = if hint.contains("Ctrl") {
                Modifiers::secondary_key()
            } else {
                Modifiers::default()
            };
            mods.shift = hint.contains("Shift");
            assert_eq!(
                Command::for_key(&key, mods),
                Some(command),
                "{} says {hint}, which lands somewhere else",
                command.label(),
            );
        }
    }

    #[test]
    fn no_two_commands_claim_the_same_key() {
        for (i, a) in ALL.iter().enumerate() {
            for b in &ALL[i + 1..] {
                assert_ne!(a.hint(), b.hint(), "{} and {} share a key", a.label(), b.label());
            }
        }
    }

    /// Every command on a list, submenus and all, in the order they are drawn.
    fn everything(list: &'static [Entry]) -> Vec<Command> {
        list.iter()
            .flat_map(|entry| match entry {
                Entry::Rule => Vec::new(),
                Entry::Does(command) => vec![*command],
                Entry::More(_, inner) => everything(inner),
            })
            .collect()
    }

    /// Every list a right-click can put up, the submenus among them.
    fn all_lists() -> Vec<&'static [Entry]> {
        fn walk(list: &'static [Entry], out: &mut Vec<&'static [Entry]>) {
            out.push(list);
            for entry in list {
                if let Entry::More(_, inner) = entry {
                    walk(inner, out);
                }
            }
        }
        let mut out = Vec::new();
        for list in [&BOARD_MENU[..], &CARD_MENU[..], &ROPE_MENU[..], &MANY_MENU[..]] {
            walk(list, &mut out);
        }
        out
    }

    #[test]
    fn no_menu_draws_a_rule_where_there_is_nothing_to_divide() {
        // A rule at the top or the bottom draws as a stray line, and two in a
        // row draw as a gap nobody asked for. Submenus included: a list is a
        // list wherever it hangs.
        for list in all_lists() {
            assert!(!matches!(list.first(), Some(Entry::Rule) | None));
            assert!(!matches!(list.last(), Some(Entry::Rule) | None));
            assert!(!list.windows(2).any(|w| matches!(w, [Entry::Rule, Entry::Rule])));
        }
    }

    #[test]
    fn no_submenu_is_worth_less_than_the_row_that_opens_it() {
        // One entry behind a name is a row you press twice to reach a row.
        for list in all_lists() {
            for entry in list {
                if let Entry::More(name, inner) = entry {
                    assert!(inner.len() >= 2, "{name} holds one thing");
                }
            }
        }
    }

    #[test]
    fn the_rope_menu_is_about_ropes_and_the_card_menu_is_not() {
        // The two lists are different lists rather than one with half of it
        // dimmed, and this is what keeps them that way: a connection command
        // that turned up on the card menu would draw permanently grey.
        let rope = |c: &Command| {
            matches!(
                c,
                Command::ConnLabel
                    | Command::ConnDelete
                    | Command::ConnColour(_)
                    | Command::ConnArrow(_)
                    | Command::ConnStyleAs(_)
                    | Command::ConnWeightAs(_)
            )
        };
        assert!(everything(&ROPE_MENU).iter().all(rope));
        assert!(!everything(&CARD_MENU).iter().any(rope));
    }

    #[test]
    fn every_row_of_choices_on_the_rope_menu_can_be_ticked() {
        // Four axes, and each of them draws as a readout of what the rope
        // already is rather than as a set of instructions. A row whose entries
        // never tick is a row you have to remember what you last did to.
        let commands = everything(&ROPE_MENU);
        let axes = [
            commands.iter().filter(|c| matches!(c, Command::ConnArrow(_))).count(),
            commands.iter().filter(|c| matches!(c, Command::ConnColour(_))).count(),
            commands.iter().filter(|c| matches!(c, Command::ConnStyleAs(_))).count(),
            commands.iter().filter(|c| matches!(c, Command::ConnWeightAs(_))).count(),
        ];
        // Every value the format defines, so a colour that exists and is not
        // offered would show up here.
        assert_eq!(axes, [4, 5, 3, 3]);
    }

    #[test]
    fn the_board_menu_is_the_card_menu_with_the_card_taken_out() {
        // What a right-click on bare paper offers is what a right-click on a
        // card offers minus everything that is about the card — so a command
        // added to one and forgotten on the other shows up here rather than as
        // a row that is on the board's list and permanently grey.
        let card = everything(&CARD_MENU);
        for command in everything(&BOARD_MENU) {
            assert!(
                card.contains(&command),
                "{} is offered on bare paper and nowhere else",
                command.label(),
            );
        }
        assert!(everything(&BOARD_MENU).len() < card.len());
    }

    #[test]
    fn nothing_is_offered_twice_on_one_list() {
        // A command in a submenu *and* on the face above it is a menu that
        // answers the same question in two places — which is the clutter the
        // submenus were for.
        for list in [&BOARD_MENU[..], &CARD_MENU[..], &ROPE_MENU[..], &MANY_MENU[..]] {
            let all = everything(list);
            for (i, command) in all.iter().enumerate() {
                assert!(
                    !all[i + 1..].contains(command),
                    "{} is on the same list twice",
                    command.label(),
                );
            }
        }
    }
}
