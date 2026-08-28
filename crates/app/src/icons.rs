//! The pictures, and the one place they come from.
//!
//! Phosphor's set, at `assets/icons`, **compiled into the binary**
//! rather than read off disk. That is not an optimisation: this app ships as a
//! single executable that people download and run from wherever it landed, and
//! an icon loaded relative to the working directory is an icon that is there
//! while you develop and gone the first time somebody double-clicks it.
//!
//! Only the icons that are used are vendored. The full set is around fifteen
//! hundred files, and a `include_bytes!` of all of them would put a megabyte of
//! SVG nobody draws into every build.
//!
//! ## Two weights, and where each one goes
//!
//! Duotone everywhere, **except the marks whose whole content is a shape**:
//! plus, minus, close, maximise, the tick and the two carets. Phosphor draws
//! them all in duotone as the mark over a soft filled square, which is right
//! for an icon that means something and wrong for one that *is* a shape — three
//! window buttons in a row came out as three faint chips, a plus sitting in a
//! square reads as a button drawn inside a button, and a tick in a twelve-pixel
//! menu gutter came out as a smudge with a tick somewhere in it. The caret is
//! the same story one step further: duotone draws it as a filled triangle
//! inside its own outline, and at the eleven pixels the project switcher gives
//! it that is a blob rather than an arrow. Regular draws the plain chevron
//! every dropdown in the world has. Those take the regular weight, under
//! `assets/icons/regular`; everything else is duotone, under
//! `assets/icons/duotone`. The weight is part of the path in the table below,
//! so which one an icon is can be read off the same line that names its file.
//!
//! ## Why duotone survives being drawn in one colour
//!
//! GPUI rasterises an SVG and then throws the colour away: it keeps the alpha
//! channel as a mask and tints the whole thing with `text_color` — see
//! `SvgRenderer::render`, which is literally `pixmap.pixels().map(|p|
//! p.alpha())`. A set whose two tones were two *hues* would come out flat.
//!
//! Phosphor's duotone is not two hues. It is one shape at full opacity over the
//! same shape's mass at `opacity="0.2"`, both `currentColor` — so the mask
//! already carries the two tones as two alphas, and tinting it reproduces the
//! duotone exactly, in whatever colour the call site asks for. The style and
//! the renderer happen to agree, which is the reason this set and not another.

use std::borrow::Cow;

use gpui::{px, svg, AssetSource, Hsla, SharedString, Styled, Svg};

/// The three sizes an icon is drawn at, named rather than picked fresh at
/// every call site.
///
/// Phosphor's set is drawn on a 256-unit grid with a sixteen-unit stroke, so
/// twelve and sixteen land that stroke on three-quarters and one whole device
/// pixel at this app's usual scale factor; the eleven and thirteen and
/// fifteen these replace did not, and smeared for it. [`ICON_SM`] is for a
/// mark beside words that already say what it means — a gutter tick, a status
/// fact, the titlebar's chevron. [`ICON_MD`] is for a mark that *is* the
/// control — a row's leading picture, a titlebar button. [`ICON_LG`] is for
/// the transport strip alone, whose buttons are aimed at rather than read.
pub const ICON_SM: f32 = 12.0;
pub const ICON_MD: f32 = 16.0;
pub const ICON_LG: f32 = 20.0;

/// Declare an icon once and get all three things that have to stay in step:
/// the name it is known by, the path GPUI asks for, and the bytes that answer.
///
/// A macro because the third one is `include_bytes!`, which needs a literal —
/// so a table built any other way would have the file names written twice and
/// no way to notice when the two copies drifted.
macro_rules! icons {
    ($($variant:ident => $file:literal,)*) => {
        /// One of the pictures. See the module note for where they come from.
        ///
        /// **A catalogue, not a call graph.** Dead code is allowed here on
        /// purpose: this enum *is* the manifest of the SVGs compiled into the
        /// binary, and an entry with no caller yet is an icon that arrived
        /// before the screen that will draw it — which is the ordinary order
        /// to do those two things in. Nothing rots unnoticed, because `ALL`
        /// below walks every variant and the test walks `ALL`: a variant whose
        /// file went missing fails the build whether anything draws it or not.
        #[allow(dead_code)]
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Icon {
            $($variant,)*
        }

        impl Icon {
            /// What to hand [`gpui::svg`], and what [`Icons`] answers to.
            pub fn path(self) -> &'static str {
                match self {
                    $(Icon::$variant => concat!("icons/", $file, ".svg"),)*
                }
            }

            /// Every one, for the test below.
            #[cfg(test)]
            const ALL: &'static [Icon] = &[$(Icon::$variant,)*];
        }

        /// The files themselves, in the binary.
        const FILES: &[(&str, &[u8])] = &[
            $((
                concat!("icons/", $file, ".svg"),
                include_bytes!(concat!("../assets/icons/", $file, ".svg")),
            ),)*
        ];
    };
}

icons! {
    // The marks that are a shape rather than a picture of something. See the
    // note above for why these are the regular weight and nothing else is.
    Minimise => "regular/minus",
    Maximise => "regular/square",
    Close => "regular/x",
    New => "regular/plus",
    Check => "regular/check",
    // The update badge's last state. A shape — a loop of arrow — rather than a
    // picture of anything, so regular like the four above it, and the only
    // mark in the titlebar that sits on a filled background.
    Restart => "regular/arrow-clockwise",

    // The marks that say a thing opens onto more. Regular for the same reason
    // the five above are: a caret is a shape rather than a picture of one.
    CaretDown => "regular/caret-down",
    CaretRight => "regular/caret-right",
    // The tour bar's other direction. Its only caller, which is why it is not
    // beside a `CaretUp` that nothing asks for.
    CaretLeft => "regular/caret-left",
    // The inventory sheet's mark for a row that goes somewhere. A shape, and
    // deliberately the plainest one available: it is repeated down a column of
    // thirty rows and has to read as a hint rather than as decoration.
    Travel => "regular/arrow-right",
    // The pair of carets every dropdown in the world wears, which is the whole
    // reason it is a different mark from `CaretDown`: one caret says "this
    // opens downwards" and two say "this is a value you can change to another
    // one off a list", and the settings page has both on the same screen.
    CaretUpDown => "regular/caret-up-down",

    // The settings page's search field. Regular rather than the duotone
    // `Zoom`, which is the same glass and means something else: that one is a
    // *reading* of how far the board is zoomed, sitting in the status bar
    // beside a number, and this is a place to type.
    Search => "regular/magnifying-glass",

    // The wordless buttons beside the project switcher.
    Commands => "duotone/command",
    Find => "duotone/list-magnifying-glass",
    Settings => "duotone/gear",

    // The bottom bar.
    Zoom => "duotone/magnifying-glass",
    Warned => "duotone/warning",
    Told => "duotone/info",
    Mode => "duotone/keyboard",
    Cards => "duotone/stack",
    Selected => "duotone/selection",

    // The tool strip.
    Select => "duotone/cursor",
    Pan => "duotone/hand",
    Connect => "duotone/line-segments",
    Note => "duotone/note",

    // The controls on a card that plays.
    Play => "duotone/play",
    Pause => "duotone/pause",
    Sound => "duotone/speaker-high",
    Muted => "duotone/speaker-slash",
    Loop => "duotone/repeat",

    // The mark a locked card wears in its top corner. See `Command::ToggleLock`.
    Locked => "duotone/lock-simple",

    // Tags: the word itself, and the filter that shows only what wears one.
    // Two marks rather than one because they answer different questions — a
    // tag row that is not on the selection wears the first, and the status bar
    // wears the second whenever a filter is standing.
    Tag => "duotone/tag",
    Filter => "duotone/funnel",

    // The status bar's count of what is in the bin. A bin rather than a cross,
    // because what the number says is "these are still here", and a cross says
    // the opposite. See `BoardView::bin_selection` for how long that lasts.
    Binned => "duotone/trash",

    // What kind of thing a row in a list is.
    Image => "duotone/image",
    Video => "duotone/video",
    Audio => "duotone/music-notes",
    Link => "duotone/link",
    Text => "duotone/text-t",
    Model => "duotone/cube",
    Swatch => "duotone/swatches",
    Sticker => "duotone/sticker",
    Unknown => "duotone/question",
    // A stored file no card points at. The dashed outline is the whole mark:
    // it is the same file shape every other row wears, drawn as an outline of
    // itself, which is what an orphan is.
    Orphan => "duotone/file-dashed",
    Board => "duotone/squares-four",

    // The four doors on the welcome screen's last page, and the three on an
    // empty board. Pictures of things rather than shapes, so duotone like the
    // rest of the table — the plus that is *only* a plus is `New`, above, and
    // this is the one sitting on a card being offered.
    NewBoard => "duotone/plus-square",

    // The settings sidebar's six. A section is a place, and a place with a
    // picture on it is one somebody finds again by shape rather than by
    // reading six words every time — which is the whole of what they are for,
    // since none of these is ever the only thing naming its row.
    //
    // `Board` and `Drop` already stand for two of the six (`Arranging` is a
    // board's worth of cards, `Updates` is a thing coming down), and they are
    // reused rather than duplicated under a second name: an icon means what it
    // is a picture of, and two names for one picture is how a set drifts.
    SectionCanvas => "duotone/grid-nine",
    SectionMedia => "duotone/image-square",
    SectionGeneral => "duotone/sliders",
    SectionAppearance => "duotone/palette",
    Folder => "duotone/folder-open",
    Explore => "duotone/compass",
    Tour => "duotone/path",
    Write => "duotone/note-pencil",
    Paste => "duotone/clipboard",
    Drop => "duotone/download-simple",

    // ---------------------------------------------------------------------
    // The commands
    // ---------------------------------------------------------------------
    //
    // One picture per thing a menu row or a palette row can do — see
    // `Command::mark`. A menu of nothing but words is a menu read one line at
    // a time; a menu with pictures down the left is one found by shape, which
    // is what makes the twentieth visit faster than the first.
    //
    // Reused freely, and deliberately: `Ruler` stands for all four rows that
    // are about measuring, `Crosshair` for both that are about putting the
    // camera somewhere. Two pictures for one idea is how a set stops meaning
    // anything.
    OpenOut => "duotone/arrow-square-out",
    Rename => "duotone/textbox",
    Crop => "duotone/crop",
    Copy => "duotone/copy",
    Scissors => "duotone/scissors",
    Colour => "duotone/drop-half",
    Crosshair => "duotone/crosshair",
    List => "duotone/list-bullets",
    Undo => "duotone/arrow-u-up-left",
    Redo => "duotone/arrow-u-up-right",
    Save => "duotone/floppy-disk",
    SelectAll => "duotone/selection-all",
    SelectNone => "duotone/selection-slash",
    ZoomIn => "duotone/magnifying-glass-plus",
    ZoomOut => "duotone/magnifying-glass-minus",
    Magnet => "duotone/magnet",
    Ruler => "duotone/ruler",
    Sparkle => "duotone/sparkle",
    Eye => "duotone/eye",
    Shrink => "duotone/arrows-in",
    Pin => "duotone/map-pin",
    Move => "duotone/arrows-out-cardinal",
    Align => "duotone/align-left-simple",
    Distribute => "duotone/arrows-out-line-horizontal",
    Weight => "duotone/rows",
    Paper => "duotone/file",
}

impl Icon {
    /// The picture for a card of this type, for a list that names one.
    ///
    /// Every type this build knows, and [`Icon::Unknown`] for one it does not —
    /// which is the format's rule rather than a fallback: a type from a later
    /// build has to show up as *something*, and a question mark is the honest
    /// answer to "what is this".
    pub fn for_kind(kind: &mbrd_core::ItemType) -> Icon {
        use mbrd_core::ItemType as T;
        match kind {
            T::Image => Icon::Image,
            T::Video => Icon::Video,
            T::Audio => Icon::Audio,
            T::Note => Icon::Note,
            T::Link => Icon::Link,
            T::Text => Icon::Text,
            T::Model => Icon::Model,
            T::Swatch => Icon::Swatch,
            T::Sticker => Icon::Sticker,
            _ => Icon::Unknown,
        }
    }
}

/// One picture, at a size and in a colour.
///
/// **The colour is not optional, and that is GPUI's rule rather than a taste.**
/// `Svg::paint` reads `style.text.color` and draws nothing at all when it is
/// unset — text colour does not cascade into an element's own computed style —
/// so an icon that inherited its colour from the row around it would be an icon
/// that is simply absent. Making it an argument is how that stops being a
/// silent blank.
pub fn icon(which: Icon, size: f32, colour: Hsla) -> Svg {
    svg().path(which.path()).size(px(size)).flex_none().text_color(colour)
}

/// The icons, as GPUI asks for them.
///
/// Handed to `Application::with_assets` in `main.rs`. Without it the default
/// source is `()`, which answers `None` to everything — and an SVG that fails
/// to load draws nothing, so the whole set would go quietly missing.
pub struct Icons;

impl AssetSource for Icons {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        // A scan of thirty entries, and it runs once per icon per size: GPUI
        // caches the rasterised mask by path and size, so this is not on the
        // frame path.
        Ok(FILES.iter().find(|(name, _)| *name == path).map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(FILES
            .iter()
            .filter(|(name, _)| name.starts_with(path))
            .map(|(name, _)| SharedString::from(*name))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_a_file_the_source_can_answer_with() {
        // The macro keeps the name and the bytes in step, but nothing keeps
        // `path` and the lookup in step — and a mismatch there is an icon that
        // draws as nothing, which is exactly the failure nobody notices in a
        // screenshot.
        for &which in Icon::ALL {
            let found = Icons.load(which.path()).expect("the source cannot fail");
            assert!(found.is_some(), "{:?} has no file at {}", which, which.path());
            assert!(!found.unwrap().is_empty(), "{:?} is an empty file", which);
        }
    }

    #[test]
    fn no_two_icons_share_a_path() {
        // Two names for one picture is not an error, but it is nearly always a
        // typo in the table — and the one it makes is silent.
        for (i, &a) in Icon::ALL.iter().enumerate() {
            for &b in &Icon::ALL[i + 1..] {
                assert_ne!(a.path(), b.path(), "{a:?} and {b:?} are the same file");
            }
        }
    }

    #[test]
    fn a_card_type_this_build_never_heard_of_still_gets_a_picture() {
        use mbrd_core::ItemType;
        assert_eq!(Icon::for_kind(&ItemType::Image), Icon::Image);
        // Furniture and the format's own tombstone, neither of which is a kind
        // of thing somebody put on a board.
        assert_eq!(Icon::for_kind(&ItemType::Gone), Icon::Unknown);
        assert_eq!(Icon::for_kind(&ItemType::Fence), Icon::Unknown);
    }
}
