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
use mbrd_core::arrange::Arrangement;
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
    /// A fresh empty board, in the folder `dirs::boards` names.
    NewBoard,
    Recentre,
    FitBoard,
    /// Zoom in one notch, centred on the middle of the view.
    ///
    /// The wheel's own zoom goes through `Camera::zoom_by` about wherever the
    /// pointer is; this is the same call about the middle of the view instead,
    /// which is what a key press has to aim with — see `BoardView::zoom_in`.
    /// The reason this exists at all: an infinite canvas with no scrollbars
    /// gives a keyboard-only visitor no way to look closer at anything
    /// without it.
    ZoomIn,
    /// The other half of [`Self::ZoomIn`].
    ZoomOut,
    ToggleGrid,
    ToggleAxes,
    ToggleSnap,
    ToggleWeb,
    /// Whether a drag shows what it is lining up with. See `core/guides.rs`.
    ToggleGuides,
    OpenBoard,
    /// Everything the app can be asked to do, as a list you type at.
    ///
    /// The other face of this module. The context menu shows what applies to
    /// what is in your hand; this shows the whole table and lets you name what
    /// you want — which is the only way to reach a command whose menu you
    /// would have to know to look under, and the only way to find one that has
    /// no key at all.
    ///
    /// Opened by tapping Shift twice rather than by a chord, because every
    /// chord worth having is taken and a double-tap costs no key at all.
    Palette,
    /// Find something on the board by name and go to it.
    ///
    /// A board has no edges, so a card you cannot see is a card you have no
    /// way to reach except by remembering which direction you left it in.
    /// This is the answer to that, and it is why choosing a result *moves the
    /// camera* rather than merely selecting: being told where a thing is is no
    /// use on a canvas you then have to fly across by hand.
    Search,
    /// Whether the interface is allowed to move. A preference, not a board
    /// setting — see `prefs.rs` for why the two are kept apart.
    ///
    /// Named for the state it is normally in, like [`Self::DontScaleText`]: it
    /// is on unless somebody turns it off, so the row reads as a switch you
    /// untick rather than an instruction you follow.
    ToggleMotion,
    /// Whether to *look* for new versions. Not whether one can be installed,
    /// which is a fact about how this build was installed rather than a choice.
    ToggleUpdateChecks,
    /// Find out whether a newer version exists, and then — pressed again —
    /// install it and restart into it.
    ///
    /// One command rather than three, because the three are the same intent a
    /// step apart and a menu with `Check`, `Download` and `Install` on it is a
    /// menu where two rows are always wrong. What it does depends on how far
    /// the last press got; see `BoardView::update_step`.
    CheckForUpdates,
    /// Open the settings page — every switch the app has, on one surface.
    ///
    /// The page itself is `settings.rs`; this is only the door onto it. A
    /// command rather than a submenu so the palette can reach it and a key
    /// can open it, and because the board's numbers — grid step, card gap,
    /// media fit — are choices a menu row cannot show.
    Settings,
    /// Join everything selected with the fewest lines that reach all of it.
    Connect,
    /// Put a labelled rectangle around what is selected. Membership is
    /// measured from where the cards are, so there is nothing else to do.
    ///
    /// Called `AddFence` and labelled "Group", and both names are right: the
    /// *thing* is a fence, which is what the format has always called it, and
    /// the *act* is grouping, which is what somebody pressing `Ctrl G` means.
    AddFence,
    /// Take the fence away and leave what was inside it.
    ///
    /// The other half of grouping, and the reason binning a group takes its
    /// contents with it: there is already a word for keeping them.
    Ungroup,
    /// Whether the selected notes pin to whatever they lie on.
    ///
    /// **Off by default.** A note that merely overlaps a photograph is two
    /// things near each other; a note the author has marked sticky is *on*
    /// it, travels with it, and hands a drag to it. The flag is the door
    /// onto all of `core::stick`, and it used to be the other way around —
    /// see that module for what the old default kept breaking.
    ToggleSticky,
    /// Whether this card's words keep their size as the board moves under
    /// them. **On unless somebody turns it off**, which is why it is named
    /// for the state it is normally in rather than for the change it makes.
    ///
    /// A row that is ticked by default is a row you can find by looking for
    /// what to turn off, which is how somebody arrives at this one: the thing
    /// they noticed was words that appeared to change size, and the thing they
    /// go looking for is the switch that stops it. `Scale text`, unticked, was
    /// the same setting spelled as the thing to turn *on*, and nobody reads a
    /// menu that way.
    ///
    /// Per card rather than per board, because the two answers are both right
    /// and which one is right is a fact about the card: a note you wrote to
    /// read is a label on a map and wants to stay the size it is, and a note
    /// you wrote to *be* the card — a title across a section, a word on a
    /// swatch — is part of the picture and wants to grow with it.
    DontScaleText,
    /// Whether this note's height follows what is written on it.
    ///
    /// **Off unless somebody turns it on**, which is the other way round from
    /// [`Self::DontScaleText`] above and deliberately so: a card you can drag
    /// to a size and have it stay there is what a card is, and a note that
    /// resized itself the moment you typed into it would be a surprise on a
    /// board full of ones that do not. So the fixed size is the default and
    /// this is the thing you ask for.
    ///
    /// Per card, for the reason the row above it is: a note written to fit a
    /// gap in a layout wants the size it was given, and a note written to be
    /// read wants to be as tall as its words. Both are right.
    FitText,
    /// Play or pause every selected card that has a play button.
    ///
    /// Play, pause and mute used to be reachable only by a mouse landing on
    /// the strip drawn under a hovered card — see `BoardView::press_control`
    /// and `controls_at` — which put a control this app draws on the card
    /// itself out of reach of anybody not holding a mouse. This runs through
    /// `press_control` exactly the way a click does, so a keystroke changes
    /// the board and the undo strip the same way a press on the button would.
    PlayPause,
    /// Mute or unmute every selected card that has a mute button.
    ///
    /// The other half of [`Self::PlayPause`]'s fix, and unkeyed on purpose —
    /// see [`Self::hint`] — because the letters worth spending are gone and
    /// this is reached through the palette or a card's own menu instead.
    ToggleMute,

    // Arranging. Every one of them carries the axis or the edge it is about,
    // for the reason the connection commands do: a menu builds its row by
    // mapping over the enum rather than over a list here that could fall
    // behind it.
    Align(Edge),
    Distribute(Axis),
    /// Push overlapping cards off each other.
    Separate,

    // The whole-board layouts. `Arrange` carries which one, for the reason
    // `Align` carries its edge: the menu builds its list by mapping over
    // `Arrangement::ALL`, which the *core* defines, so a layout the engine
    // gains is a row here the moment it exists.
    /// Pick a named arrangement and lay the board out in it. Also what the
    /// board remembers in `arrangements.desktop`, so the menu ticks the one
    /// the board was last laid out in.
    Arrange(Arrangement),
    /// Lay the whole board out again in the arrangement it already has, with
    /// a fresh seed — the "shake the board loose" gesture.
    Rearrange,
    /// The same, over only what is selected, centred where the selection is.
    RearrangeSelection,

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
            Self::NewBoard => "New board",
            Self::Recentre => "Recenter",
            Self::FitBoard => "Fit board",
            Self::ZoomIn => "Zoom in",
            Self::ZoomOut => "Zoom out",
            Self::ToggleGrid => "Grid",
            Self::ToggleAxes => "Axes",
            Self::ToggleSnap => "Snap to grid",
            Self::ToggleWeb => "Connections",
            Self::ToggleGuides => "Alignment guides",
            Self::OpenBoard => "Open board…",
            Self::Palette => "All commands…",
            Self::Search => "Find on board…",
            Self::ToggleMotion => "Animation",
            Self::ToggleUpdateChecks => "Look for new versions",
            Self::CheckForUpdates => "Check for updates…",
            Self::Settings => "Settings…",
            Self::Connect => "Connect",
            Self::AddFence => "Group",
            Self::Ungroup => "Ungroup",
            Self::ToggleSticky => "Sticky",
            Self::DontScaleText => "Don't scale text",
            Self::FitText => "Dynamic size",
            Self::PlayPause => "Play / pause",
            Self::ToggleMute => "Mute",
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

            // Named after the shape rather than the act — the menu ticks the
            // one the board is in, so a row reads as a choice already made.
            Self::Arrange(arrangement) => arrangement.label(),
            Self::Rearrange => "Rearrange everything",
            Self::RearrangeSelection => "Rearrange selection",

            Self::ConnLabel => "Label",
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

    /// The label as it should be drawn on a row, with a step's name folded
    /// in for `Undo` and `Redo` — "Undo nudge" rather than bare "Undo" — so
    /// the row says what pressing it is about to take back rather than only
    /// whether anything can.
    ///
    /// The menu used to compute this itself and the palette used to call
    /// [`Self::label`] straight, which meant the menu said "Undo nudge" and
    /// the palette said "Undo" for the same command in the same instant. One
    /// place that both call is the only way the two cannot drift apart again.
    pub fn label_in(self, view: &BoardView) -> String {
        match self {
            Self::Undo => step_label("Undo", view.undo_step()),
            Self::Redo => step_label("Redo", view.redo_step()),
            _ => self.label().to_string(),
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
            Self::NewBoard => "Ctrl N",
            Self::Recentre => "0",
            Self::FitBoard => "F",
            Self::ZoomIn => "Ctrl +",
            Self::ZoomOut => "Ctrl -",
            Self::ToggleGrid => "G",
            Self::ToggleAxes => "X",
            Self::ToggleSnap => "S",
            Self::ToggleWeb => "W",
            Self::OpenBoard => "Ctrl P",
            // A tap of a modifier is not a keystroke, and this is the one
            // hint in the table that a key press cannot satisfy — see
            // `every_key_a_command_advertises_is_a_key_that_reaches_it`,
            // which exempts it by name rather than by rule.
            Self::Palette => "Shift Shift",
            Self::Search => "Ctrl F",
            Self::CheckForUpdates => "Ctrl U",
            // The spelling every application means settings by.
            Self::Settings => "Ctrl ,",
            Self::Connect => "J",
            Self::AddFence => "Ctrl G",
            Self::Ungroup => "Ctrl Shift G",
            Self::ToggleSticky => "U",
            Self::PlayPause => "Space",
            // No key: the letters worth spending are gone, and unlike play or
            // pause this is not the one control every media player binds a
            // key to — reached through the palette or a card's own menu
            // instead.
            Self::ToggleMute => "",
            // No key: the single letters worth spending are gone, and this is
            // a switch you set once rather than one you reach for.
            Self::DontScaleText
            | Self::ToggleGuides
            | Self::FitText
            | Self::ToggleMotion
            | Self::ToggleUpdateChecks => "",
            Self::Align(_) | Self::Distribute(_) | Self::Separate => "",
            // No key: a whole-board relayout is deliberate and rare, and a
            // single letter that scattered twenty thousand cards would be the
            // most expensive typo in the app. Undo covers it, but reaching
            // for it through the palette is the right amount of friction.
            Self::Arrange(_) | Self::Rearrange | Self::RearrangeSelection => "",

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
            // `E` as well as `Ctrl G`: the menu advertises the one everybody
            // arrives already knowing, and the single letter is the one this
            // board's own keyboard has always used.
            "e" if plain => Self::AddFence,
            // Shift first, or the plain-group arm would swallow both.
            "g" if mods.secondary() && mods.shift => Self::Ungroup,
            "g" if mods.secondary() => Self::AddFence,
            "u" if plain => Self::ToggleSticky,
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
            // Plain `n` is a note; the modified form was free, and is what
            // every other application means by it.
            "n" if mods.secondary() => Self::NewBoard,
            "v" if mods.secondary() => Self::Paste,
            // A second door onto the palette, unadvertised: `Shift Shift` is
            // the one spelling meant to be learned and stays the hint, but a
            // double-tap is a gesture sticky keys, an on-screen keyboard and
            // voice input all have trouble producing, and this app should not
            // be closed to any of them. Shift first, or the plain `Ctrl P`
            // arm below would swallow it — same order as `Ungroup` above.
            "p" if mods.secondary() && mods.shift => Self::Palette,
            "p" if mods.secondary() => Self::OpenBoard,
            // Two spellings for one thing: `Ctrl F` is what a hand reaches for
            // and `Ctrl K` is what this app's own roadmap promised. Neither is
            // the alias — they are both simply it.
            "f" | "k" if mods.secondary() => Self::Search,
            // Plain `u` is the sticky toggle; the modified form was free.
            "u" if mods.secondary() => Self::CheckForUpdates,
            "," if mods.secondary() => Self::Settings,
            // Shift first, or the plain-undo arm would swallow both.
            "z" if mods.secondary() && mods.shift => Self::Redo,
            "z" if mods.secondary() => Self::Undo,
            "y" if mods.secondary() => Self::Redo,
            // `+` as well as `=`: on most keyboards `+` is the shifted glyph
            // on the same physical key as `=`, and depending on the layout
            // gpui may report either one. Either spelling reaches the same
            // command.
            "=" | "+" if mods.secondary() => Self::ZoomIn,
            "-" if mods.secondary() => Self::ZoomOut,
            "space" if plain => Self::PlayPause,
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
            // Anything at all to lay out — furniture does not count, because
            // the title card and the hints are exactly what a rearrangement
            // leaves where they are.
            Self::Arrange(_) | Self::Rearrange => view.doc.board.items.iter().any(|i| {
                !matches!(i.kind, mbrd_core::ItemType::Title | mbrd_core::ItemType::Ghost)
            }),
            // Two: one card rearranged alone is a card teleported somewhere
            // arbitrary, which nobody has ever meant by the word.
            Self::RearrangeSelection => view.selection.len() >= 2,
            Self::Ungroup => view.can_ungroup(),
            Self::ToggleSticky => view.can_toggle_sticky(),
            Self::DontScaleText => view.text_unscaled().is_some(),
            Self::FitText => view.text_fitted().is_some(),
            // A note or a swatch selected alone has neither, and a menu that
            // offered a play button on one would be a menu that did nothing
            // when pressed.
            Self::PlayPause | Self::ToggleMute => view.has_media_selected(),
            Self::Undo => view.undo_step().is_some(),
            Self::Redo => view.redo_step().is_some(),
            Self::SelectAll => view.doc.board.items.iter().any(|i| i.kind.is_content()),
            // Dimmed rather than hidden in a build that has no update key —
            // which is every build except a released one. A row that vanishes
            // depending on how the binary was compiled is a menu that changes
            // shape for reasons nobody watching it can see.
            Self::CheckForUpdates => crate::update::possible(),
            // Always. A palette you can only open when something is selected
            // is a palette you cannot use to find out what you could select.
            Self::Palette => true,
            // Even on an empty board: the answer "nothing by that name" is a
            // real answer, and a search that greys out when there is nothing
            // to find is one you have to already know the answer to open.
            Self::Search => true,
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
            Self::ToggleGuides => Some(settings.guides),
            // The two that are about the person rather than about the board,
            // which is why they read from `prefs` and not from `settings`.
            // Per-card rather than board-wide: what the tick shows is whether
            // every selected note is sticky, and the row stays untickable
            // when nothing selected is a note.
            Self::ToggleSticky => view.sticky_state(),
            Self::ToggleMotion => Some(view.prefs.motion),
            Self::ToggleUpdateChecks => Some(view.prefs.update),
            // The arrangement the board was last laid out in, so the Layout
            // list reads as a state and not only as eight verbs.
            Self::Arrange(arrangement) => {
                Some(view.doc.board.arrangements.desktop == arrangement.as_str())
            }
            Self::ConnColour(colour) => Some(view.rope_meta()?.color == colour),
            Self::ConnArrow(dir) => Some(view.rope_meta()?.dir == dir),
            Self::ConnStyleAs(style) => Some(view.rope_meta()?.style == style),
            Self::ConnWeightAs(weight) => Some(view.rope_meta()?.weight == weight),
            Self::DontScaleText => view.text_unscaled(),
            Self::FitText => view.text_fitted(),
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
            Self::NewBoard => view.new_board(cx),
            Self::Recentre => view.go_home(cx),
            Self::FitBoard => view.fit_all(cx),
            Self::ZoomIn => view.zoom_in(cx),
            Self::ZoomOut => view.zoom_out(cx),
            Self::ToggleGrid => view.toggle_setting(Self::ToggleGrid, cx),
            Self::ToggleAxes => view.toggle_setting(Self::ToggleAxes, cx),
            Self::ToggleSnap => view.toggle_setting(Self::ToggleSnap, cx),
            Self::ToggleWeb => view.toggle_setting(Self::ToggleWeb, cx),
            Self::ToggleGuides => view.toggle_setting(Self::ToggleGuides, cx),
            Self::ToggleMotion => view.toggle_pref(Self::ToggleMotion, cx),
            Self::ToggleUpdateChecks => view.toggle_pref(Self::ToggleUpdateChecks, cx),
            Self::OpenBoard => view.open_switcher(window, cx),
            Self::Palette => view.open_palette(crate::palette::Mode::Commands, cx),
            Self::Search => view.open_palette(crate::palette::Mode::Search, cx),
            Self::CheckForUpdates => view.update_step(cx),
            Self::Settings => view.open_settings(cx),
            Self::Connect => view.connect_selection(cx),
            Self::AddFence => view.add_fence(cx),
            Self::Ungroup => view.ungroup(cx),
            Self::ToggleSticky => view.toggle_sticky(cx),
            Self::DontScaleText => view.toggle_text_scaling(cx),
            Self::FitText => view.toggle_fit_text(cx),
            Self::PlayPause => view.play_pause_selection(cx),
            Self::ToggleMute => view.toggle_mute_selection(cx),
            Self::Align(edge) => view.arrange(Self::Align(edge), cx),
            Self::Distribute(axis) => view.arrange(Self::Distribute(axis), cx),
            Self::Separate => view.arrange(Self::Separate, cx),
            Self::Arrange(arrangement) => view.set_arrangement(arrangement, cx),
            Self::Rearrange => view.rearrange(false, cx),
            Self::RearrangeSelection => view.rearrange(true, cx),

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

    /// Other words somebody might type looking for this.
    ///
    /// A palette that matches only the label is a palette you have to already
    /// know the wording of. "Open board…" is the board switcher, and somebody
    /// hunting for it types *project* or *switch* — neither of which is in its
    /// name, so the one command they wanted was the one thing they could not
    /// find. That was reported as "there's no project switcher".
    ///
    /// Deliberately sparse. Every word here is a word that makes some *other*
    /// command harder to find, so this is for the cases where the label is
    /// genuinely not what the thing is called — not a thesaurus.
    pub fn keywords(self) -> &'static str {
        match self {
            Self::OpenBoard => "project switcher switch recent file document",
            Self::Search => "find goto jump locate",
            Self::Palette => "commands actions",
            Self::Recentre => "home origin centre",
            Self::FitBoard => "zoom everything",
            Self::ZoomIn => "enlarge magnify closer",
            Self::ZoomOut => "shrink magnify further",
            Self::PlayPause => "video audio play pause stop",
            Self::ToggleMute => "video audio sound unmute silence",
            Self::Delete => "remove trash",
            Self::AddSwatch => "colour swatch",
            Self::Tint => "colour recolour",
            Self::AddFence => "group frame",
            Self::Connect => "rope line link join",
            Self::ConnLabel => "rename",
            Self::Separate => "overlap unstack",
            Self::Rearrange => "layout shuffle relay tidy",
            Self::RearrangeSelection => "layout shuffle relay",
            // One word for the family, so typing "layout" surfaces the whole
            // list; the labels themselves carry spiral, masonry and the rest.
            Self::Arrange(_) => "layout arrangement",
            Self::ToggleWeb => "ropes lines",
            Self::ToggleGuides => "smart guides rulers",
            // Spelled both ways: the setting is *called* Animation and the
            // thing somebody is looking for is usually "reduced motion".
            Self::ToggleMotion => "reduced motion accessibility animate",
            Self::ToggleUpdateChecks => "updates version automatic",
            Self::CheckForUpdates => "upgrade version new",
            Self::ToggleSticky => "pin stick unstick attach note",
            Self::Settings => "preferences options configure grid step gap spacing media fit",
            Self::DontScaleText => "font size zoom",
            Self::Save => "write disk",
            Self::NewBoard => "create empty fresh blank",
            _ => "",
        }
    }

    /// Every command there is, values and all.
    ///
    /// What the palette lists, and the first list in this module that is
    /// actually the whole of them. The `ALL` this replaced was the *keyed*
    /// commands, and its doc comment claimed to be exhaustive while quietly
    /// missing `ToggleWeb`, `Connect` and `Unstick` — because a list that is
    /// only ever iterated over cannot notice what was never put into it. The
    /// two tests that read it both iterated.
    ///
    /// Two things keep this one honest, and neither is enough alone:
    ///
    /// 1. The six value-carrying commands are **mapped over the format's own
    ///    lists** rather than spelled out. A colour the format gains is a
    ///    command here the moment it exists, with nothing to remember.
    /// 2. The nullary ones are spelled out, and
    ///    `every_command_there_is_is_in_the_list_of_them` matches on all of
    ///    them with no catch-all — so adding a variant fails to *compile*
    ///    until it is named, and then fails the count until it is added here.
    ///
    /// Order is the order the palette offers them in with an empty query, so
    /// it is roughly "what you reach for" rather than the enum's order.
    pub fn all() -> Vec<Self> {
        let mut out = vec![
            Self::AddNote,
            Self::AddSwatch,
            Self::AddFence,
            Self::Ungroup,
            Self::Connect,
            Self::Rename,
            Self::Duplicate,
            Self::Copy,
            Self::Cut,
            Self::Paste,
            Self::Delete,
            Self::Tint,
            Self::DontScaleText,
            Self::FitText,
            Self::PlayPause,
            Self::ToggleMute,
            Self::ToggleSticky,
            Self::BringToFront,
            Self::SendToBack,
            Self::Separate,
            Self::Rearrange,
            Self::RearrangeSelection,
            Self::SelectAll,
            Self::ClearSelection,
            Self::Undo,
            Self::Redo,
            Self::Save,
            Self::NewBoard,
            Self::OpenBoard,
            Self::Search,
            Self::Palette,
            Self::FitBoard,
            Self::Recentre,
            Self::ZoomIn,
            Self::ZoomOut,
            Self::ToggleGrid,
            Self::ToggleAxes,
            Self::ToggleSnap,
            Self::ToggleWeb,
            Self::ToggleGuides,
            Self::ToggleMotion,
            Self::ToggleUpdateChecks,
            Self::CheckForUpdates,
            Self::Settings,
            Self::ConnLabel,
            Self::ConnDelete,
        ];
        // The value-carrying commands, each mapped over the list its own
        // module keeps. See `Edge::ALL` and `model::named_enum`.
        out.extend(Arrangement::ALL.map(Self::Arrange));
        out.extend(Edge::ALL.map(Self::Align));
        out.extend(Axis::ALL.map(Self::Distribute));
        out.extend(ConnColor::ALL.iter().copied().map(Self::ConnColour));
        out.extend(ConnDir::ALL.iter().copied().map(Self::ConnArrow));
        out.extend(ConnStyle::ALL.iter().copied().map(Self::ConnStyleAs));
        out.extend(ConnWeight::ALL.iter().copied().map(Self::ConnWeightAs));
        out
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
const ADD: [Entry; 2] = [Entry::Does(Command::AddNote), Entry::Does(Command::AddSwatch)];

/// What is drawn, and the four ways of getting back to it.
const VIEW: [Entry; 13] = [
    Entry::Does(Command::ToggleGrid),
    Entry::Does(Command::ToggleSnap),
    Entry::Does(Command::ToggleAxes),
    Entry::Does(Command::ToggleWeb),
    Entry::Does(Command::ToggleGuides),
    Entry::Rule,
    Entry::Does(Command::FitBoard),
    Entry::Does(Command::Recentre),
    Entry::Does(Command::ZoomIn),
    Entry::Does(Command::ZoomOut),
    Entry::Rule,
    // Last, and behind their own rule. Everything above this is about the
    // board in front of you; these two are about the application, and they
    // are the only rows on any list that are. The settings *page* replaced
    // the two-row Settings submenu that used to sit here: the preferences it
    // held are still one press away, on a surface that can also show the
    // numbers — grid step, card gap — that a menu row cannot.
    Entry::Does(Command::CheckForUpdates),
    Entry::Does(Command::Settings),
];

/// The eight shapes a board can be laid out in, and the one verb over them.
///
/// Built by hand rather than mapped because `Entry` lists are `const`, but the
/// test module holds it to `Arrangement::ALL`'s order and length — see
/// `the_layout_menu_offers_every_arrangement_the_engine_has`.
const LAYOUTS: [Entry; 8] = [
    Entry::Does(Command::Arrange(Arrangement::Spiral)),
    Entry::Does(Command::Arrange(Arrangement::Free)),
    Entry::Does(Command::Arrange(Arrangement::Grid)),
    Entry::Does(Command::Arrange(Arrangement::Masonry)),
    Entry::Does(Command::Arrange(Arrangement::Type)),
    Entry::Does(Command::Arrange(Arrangement::Tag)),
    Entry::Does(Command::Arrange(Arrangement::Date)),
    Entry::Does(Command::Arrange(Arrangement::Scatter)),
];

/// The same choices with the verb underneath: the one-row form for the lists
/// that are already long. The board menu carries [`LAYOUTS`] and `Rearrange`
/// as two rows instead, because a submenu of pure choices names the current
/// one on its closed row — see [`Entry::hint`] — and the board menu is the
/// one with the room to spend on that readout.
const LAYOUT_MENU: [Entry; 10] = [
    Entry::Does(Command::Arrange(Arrangement::Spiral)),
    Entry::Does(Command::Arrange(Arrangement::Free)),
    Entry::Does(Command::Arrange(Arrangement::Grid)),
    Entry::Does(Command::Arrange(Arrangement::Masonry)),
    Entry::Does(Command::Arrange(Arrangement::Type)),
    Entry::Does(Command::Arrange(Arrangement::Tag)),
    Entry::Does(Command::Arrange(Arrangement::Date)),
    Entry::Does(Command::Arrange(Arrangement::Scatter)),
    Entry::Rule,
    Entry::Does(Command::Rearrange),
];

/// Everything about where several cards are in relation to each other.
const ARRANGE: [Entry; 12] = [
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
    Entry::Rule,
    Entry::Does(Command::RearrangeSelection),
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
pub const BOARD_MENU: [Entry; 11] = [
    Entry::More("Add", &ADD),
    // With nothing in hand this fences off an empty space, which is the way
    // round somebody who wants to lay a board out before filling it works.
    Entry::Does(Command::AddFence),
    Entry::Does(Command::Paste),
    Entry::Does(Command::SelectAll),
    Entry::Rule,
    // The whole-board layouts live on the board's own menu because they are
    // about the board, not about anything in hand — and the submenu, holding
    // only choices, names the arrangement the board is in on its closed row,
    // the way the rope menu names its colour. `Rearrange` sits outside the
    // submenu for exactly that reason: one verb inside the list would cost
    // the readout.
    Entry::More("Layout", &LAYOUTS),
    Entry::Does(Command::Rearrange),
    Entry::Does(Command::Undo),
    Entry::Does(Command::Redo),
    Entry::Rule,
    Entry::More("View", &VIEW),
];

/// What a right-click offers when a **card** is what is in hand.
///
/// Selection-only commands first, because a right-click that landed on a card
/// is usually about that card; the board-wide ones are below the rule.
pub const CARD_MENU: [Entry; 27] = [
    Entry::Does(Command::Rename),
    Entry::Does(Command::Duplicate),
    Entry::Rule,
    Entry::Does(Command::Cut),
    Entry::Does(Command::Copy),
    Entry::Does(Command::Paste),
    Entry::Does(Command::Delete),
    Entry::Rule,
    Entry::More("Add", &ADD),
    Entry::Does(Command::AddFence),
    Entry::Does(Command::Ungroup),
    Entry::Does(Command::Tint),
    Entry::Does(Command::DontScaleText),
    Entry::Does(Command::FitText),
    // Dimmed on anything that is not a video or an audio card — see
    // `Command::available` — the same way `DontScaleText` and `FitText`
    // above them are dimmed off a card that is not a note.
    Entry::Does(Command::PlayPause),
    Entry::Does(Command::ToggleMute),
    Entry::Does(Command::ToggleSticky),
    Entry::Does(Command::BringToFront),
    Entry::Does(Command::SendToBack),
    Entry::Rule,
    Entry::Does(Command::SelectAll),
    // "Select none" used to sit here and paid for the Layout row below: the
    // list has to fit a 700-tall window, Escape does the same thing from
    // anywhere, and with exactly one card in hand it was the least useful row
    // on the longest list. It keeps its place on the many-cards menu, where
    // letting go of a marquee's worth is a real errand.
    //
    // The board-wide layouts, below the card's own rows for the reason the
    // board menu carries them at all: they are about the board the card is
    // on, and the invariant the tests hold is that bare paper offers nothing
    // a card does not. One row here where the board menu spends two.
    Entry::More("Layout", &LAYOUT_MENU),
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
pub const MANY_MENU: [Entry; 15] = [
    Entry::Does(Command::Connect),
    Entry::Does(Command::AddFence),
    Entry::Does(Command::Ungroup),
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

/// "Undo" plus the name of the step it would take back, where there is one.
/// See [`Command::label_in`], the only caller.
fn step_label(verb: &str, step: Option<String>) -> String {
    match step {
        Some(name) => format!("{verb} {}", name.to_lowercase()),
        None => verb.to_string(),
    }
}

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

    /// Every command there is, named once more — and this time the compiler
    /// insists.
    ///
    /// The list this replaced was a hand-written `ALL` whose doc comment said
    /// "adding a variant without adding it here fails the first test rather
    /// than going unnoticed". It did not, and had not for some time: nothing
    /// asserted the list was complete, both tests that read it merely iterated
    /// over it, and a variant that was never added was one no test ever saw.
    /// `ToggleWeb`, `Connect` and `Unstick` had all quietly fallen out.
    ///
    /// Two guards, and neither is enough alone. The match below has **no
    /// catch-all**, so adding a variant fails to compile here until it is
    /// named; the count then fails until it is added to [`Command::all`] too.
    /// The first makes you look, the second makes you place it.
    #[test]
    fn every_command_there_is_is_in_the_list_of_them() {
        for command in Command::all() {
            match command {
                Command::AddNote
                | Command::AddSwatch
                | Command::Tint
                | Command::BringToFront
                | Command::SendToBack
                | Command::Rename
                | Command::Duplicate
                | Command::Copy
                | Command::Cut
                | Command::Delete
                | Command::SelectAll
                | Command::ClearSelection
                | Command::Undo
                | Command::Redo
                | Command::Paste
                | Command::Save
                | Command::NewBoard
                | Command::Recentre
                | Command::FitBoard
                | Command::ZoomIn
                | Command::ZoomOut
                | Command::ToggleGrid
                | Command::ToggleAxes
                | Command::ToggleSnap
                | Command::ToggleWeb
                | Command::ToggleGuides
                | Command::ToggleMotion
                | Command::ToggleUpdateChecks
                | Command::OpenBoard
                | Command::Palette
                | Command::Search
                | Command::CheckForUpdates
                | Command::Settings
                | Command::Connect
                | Command::AddFence
                | Command::Ungroup
                | Command::ToggleSticky
                | Command::DontScaleText
                | Command::FitText
                | Command::PlayPause
                | Command::ToggleMute
                | Command::Align(_)
                | Command::Distribute(_)
                | Command::Separate
                | Command::Arrange(_)
                | Command::Rearrange
                | Command::RearrangeSelection
                | Command::ConnLabel
                | Command::ConnDelete
                | Command::ConnColour(_)
                | Command::ConnArrow(_)
                | Command::ConnStyleAs(_)
                | Command::ConnWeightAs(_) => {
                    assert!(Command::all().contains(&command));
                }
            }
        }
        // 46 nullary, plus the seven that carry values mapped over their own
        // modules' lists: 8 arrangements, 6 edges, 2 axes, 5 colours, 4
        // arrows, 3 styles, 3 weights.
        assert_eq!(
            Command::all().len(),
            46 + 8 + 6 + 2 + 5 + 4 + 3 + 3,
            "a command was added to the enum and not to Command::all",
        );
    }

    /// The Layout submenu is `Arrangement::ALL`, row for row.
    ///
    /// The list is written out because `Entry` tables are `const`; this is
    /// what keeps it from quietly missing a layout the engine gains.
    #[test]
    fn the_layout_menu_offers_every_arrangement_the_engine_has() {
        assert_eq!(LAYOUTS.len(), Arrangement::ALL.len());
        for (entry, arrangement) in LAYOUTS.iter().zip(Arrangement::ALL) {
            assert_eq!(*entry, Entry::Does(Command::Arrange(arrangement)));
        }
    }

    /// Nothing is offered twice by the palette.
    ///
    /// Cheap to get wrong now that the value-carrying commands are mapped over
    /// the format's lists rather than spelled out: a list added to `all()`
    /// twice would draw every colour twice and nothing would complain.
    #[test]
    fn no_command_is_in_the_list_twice() {
        let all = Command::all();
        for (i, command) in all.iter().enumerate() {
            assert!(!all[i + 1..].contains(command), "{} is listed twice", command.label());
        }
    }

    /// The commands that a key actually reaches — which is not all of them.
    ///
    /// A third of the table has no key on purpose: five colours and four
    /// arrows would spend nine letters of the alphabet on choices nobody would
    /// learn. Those return `""` from `hint`, and the two tests below are about
    /// keys, so they filter rather than pretend.
    fn keyed() -> Vec<Command> {
        Command::all().into_iter().filter(|c| !c.hint().is_empty()).collect()
    }

    #[test]
    fn every_key_a_command_advertises_is_a_key_that_reaches_it() {
        for command in keyed() {
            let hint = command.hint();
            // The one hint no key press can satisfy, because it is not a key
            // press: `Shift Shift` is a modifier tapped twice, watched for in
            // `taps.rs`. Exempted by name rather than by rule, so that a
            // second one cannot be added without this line being read.
            if command == Command::Palette {
                assert_eq!(hint, "Shift Shift");
                continue;
            }
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
        let keyed = keyed();
        for (i, a) in keyed.iter().enumerate() {
            for b in &keyed[i + 1..] {
                assert_ne!(a.hint(), b.hint(), "{} and {} share a key", a.label(), b.label());
            }
        }
    }

    /// Every command a menu offers is a command the palette offers too.
    ///
    /// The palette is the whole table and a menu is a view onto it, so a
    /// command that reached a menu without reaching `all()` would be one the
    /// palette could never find.
    #[test]
    fn the_palette_offers_everything_the_menus_do() {
        let all = Command::all();
        for list in [&BOARD_MENU[..], &CARD_MENU[..], &ROPE_MENU[..], &MANY_MENU[..]] {
            for command in everything(list) {
                assert!(
                    all.contains(&command),
                    "{} is on a menu and not in all()",
                    command.label()
                );
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
