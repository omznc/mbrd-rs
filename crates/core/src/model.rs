//! The board, and everything on it.
//!
//! This is a port of `web/assets/js/board-model.ts` from the original mbrd, and
//! it keeps that file's shape deliberately: the types here describe what the
//! app *writes*, while `schema.rs` describes what a reader will *accept*, which
//! is a much wider thing. Keeping the two apart is what makes a malformed file
//! impossible to half-load — `schema::normalize` builds a replacement board and
//! hands it back, so there is no point at which a load can give up in the middle.
//!
//! Two conventions in here surprise everybody exactly once, so they are stated
//! at the top rather than at their use:
//!
//! 1. **`x` and `y` are the item's centre**, not its top-left corner.
//! 2. **`y` points up.** A card at `y: 100` sits *above* the origin. Anything
//!    drawing to a screen lays the item out at `-y`. See `viewport.rs`, which is
//!    the only place in this crate that flips it.
//!
//! Both are load-bearing across the file format, so neither may be quietly
//! normalised away here.

use serde_json::{Map, Value};

/// The longest a note may be, enforced at every door onto the board.
pub const NOTE_MAX: usize = 512;
/// The longest a board's title may be. Also bounds the exported filename.
pub const BOARD_TITLE_MAX: usize = 32;
/// Past this many items a board stops being a board.
pub const MAX_ITEMS: usize = 20_000;
/// How many deleted items the bin keeps before the oldest fall out.
///
/// A ceiling on a thing that now lives only in memory and only until the app
/// closes — see [`TrashEntry`] — so this bounds what one long session can hold
/// on to rather than what a file may carry.
pub const TRASH_LIMIT: usize = 60;
/// Every entry costs a route to work out and a subpath to draw.
pub const MAX_CONNECTIONS: usize = 2_000;
/// How many faces one board may carry with it.
pub const MAX_FONTS: usize = 8;

/// The smallest and largest an item may be, in world units.
pub const MIN_SIZE: f32 = 48.0;
pub const MAX_SIZE: f32 = 20_000.0;

/// Every type the original's `classify()` can produce.
///
/// `Other` is the extension point and is not a failure case: a reader that does
/// not recognise a type draws a plain named card and **writes the type back out
/// untouched**. That is what let `swatch` and then `sticker` ship without older
/// builds losing those items, and it is the one thing that must survive any
/// refactor of this enum.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemType {
    Image,
    Video,
    Audio,
    Note,
    Link,
    Text,
    Model,
    Title,
    Ghost,
    Swatch,
    Sticker,
    Fence,
    StyleTile,
    /// What is left of an item after the bin was emptied on it: a name, a size,
    /// and no asset at all. Nothing can ever resolve it to a file again.
    Gone,
    Generic,
    /// A type written by some build that is not this one. Carried through.
    Other(String),
}

impl ItemType {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Note => "note",
            Self::Link => "link",
            Self::Text => "text",
            Self::Model => "model",
            Self::Title => "title",
            Self::Ghost => "ghost",
            Self::Swatch => "swatch",
            Self::Sticker => "sticker",
            Self::Fence => "fence",
            Self::StyleTile => "style-tile",
            Self::Gone => "gone",
            Self::Generic => "generic",
            Self::Other(s) => s,
        }
    }

    /// Never fails. An unrecognised name becomes `Other` rather than `Generic`,
    /// because the two are different claims: `Generic` is "this build knows the
    /// type and it is nothing in particular", `Other` is "this build does not
    /// know the type", and only the second must round-trip verbatim.
    pub fn parse(s: &str) -> Self {
        match s {
            "image" => Self::Image,
            "video" => Self::Video,
            "audio" => Self::Audio,
            "note" => Self::Note,
            "link" => Self::Link,
            "text" => Self::Text,
            "model" => Self::Model,
            "title" => Self::Title,
            "ghost" => Self::Ghost,
            "swatch" => Self::Swatch,
            "sticker" => Self::Sticker,
            "fence" => Self::Fence,
            "style-tile" => Self::StyleTile,
            "gone" => Self::Gone,
            "generic" => Self::Generic,
            other => Self::Other(other.to_string()),
        }
    }

    /// Whether this type carries content, as opposed to being furniture the
    /// board draws around it. Used to decide what a fit or a count includes.
    pub fn is_content(&self) -> bool {
        !matches!(self, Self::Title | Self::Ghost | Self::Fence | Self::StyleTile)
    }
}

/// The bytes an item points at, or nothing for one that is only geometry and text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemAsset {
    /// `hash` names the content in the asset store; `family` is the format
    /// catalogue entry the original writes alongside it.
    Embedded { hash: String, family: Option<String> },
    /// The reserved link-instead-of-embed form. Nothing reads it yet, and
    /// carrying it through is the whole of what this arm is for: a board saved
    /// by a newer build has to survive a round trip here.
    External(Value),
}

impl ItemAsset {
    /// The content hash, where there is one. `External` has no local bytes.
    pub fn hash(&self) -> Option<&str> {
        match self {
            Self::Embedded { hash, .. } => Some(hash),
            Self::External(_) => None,
        }
    }
}

/// Per-type extras, unknown per key on purpose.
///
/// **Unknown keys are carried through untouched.** That is the format's other
/// extension point, alongside an unknown `ItemType`, and the reason this is a
/// JSON map rather than a struct per type: a future version can add a key
/// without this build losing it.
pub type ItemMeta = Map<String, Value>;

/// One thing on the board.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Unique within the board. `[A-Za-z0-9_-]{1,64}`.
    pub id: String,
    pub kind: ItemType,
    /// The item's **centre**, in world units, with **y pointing up**.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    /// Degrees, anticlockwise-positive.
    pub rot: f32,
    /// Stacking order. Higher is nearer.
    pub z: f32,
    /// The label on the card. Editable, and independent of any filename.
    pub name: String,
    pub asset: Option<ItemAsset>,
    pub meta: ItemMeta,
}

impl Item {
    /// A card of the given type at the origin, with nothing on it.
    pub fn new(id: impl Into<String>, kind: ItemType) -> Self {
        Self {
            id: id.into(),
            kind,
            x: 0.0,
            y: 0.0,
            w: 320.0,
            h: 240.0,
            rot: 0.0,
            z: 0.0,
            name: String::new(),
            asset: None,
            meta: Map::new(),
        }
    }

    /// The note's text, for a note. `meta.rich` is authoritative where it
    /// exists, but it flattens to exactly this, so a reader that only wants the
    /// words can stop here.
    pub fn note_text(&self) -> Option<&str> {
        self.meta.get("text").and_then(Value::as_str)
    }

    /// The address, for a link. Deliberately **not** validated at rest: the
    /// original revalidates on every render rather than trusting the file, and
    /// so should anything here that puts it in front of a user.
    pub fn url(&self) -> Option<&str> {
        self.meta.get("url").and_then(Value::as_str)
    }

    /// The id of the fence this item is inside, when the file claims one.
    ///
    /// A record of a measurement rather than the authority for it — membership
    /// is "the item's centre falls inside the fence's rectangle", which the
    /// geometry already answers. Kept because a pixel of drift across a save
    /// must not lose a grouping somebody plainly made.
    pub fn fence(&self) -> Option<&str> {
        self.meta.get("fence").and_then(Value::as_str)
    }

    /// Whether the author has nailed this item down.
    ///
    /// **A decision, never a measurement**, which is what separates it from
    /// `fence` above: nothing about where the item sits can imply it, and
    /// nothing but the author asking may set it. A locked item cannot be
    /// moved, resized or binned, and no layout deals it a new slot — it is
    /// still selectable, because unlocking it is a thing you do to it.
    ///
    /// Off unless the key is there and true, so a board that has never heard
    /// of locking reads as a board with nothing locked.
    pub fn locked(&self) -> bool {
        self.meta.get("locked").and_then(Value::as_bool).unwrap_or(false)
    }
}

/// Which of the two geometry profiles is being talked about.
///
/// A board carries both. Which one is *shown* is a device preference and is
/// deliberately not saved, so one file can sit in Mobile on a phone and Desktop
/// on a laptop.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayoutMode {
    Desktop,
    Mobile,
}

impl LayoutMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Mobile => "mobile",
        }
    }
}

/// One item's place in one layout.
///
/// `presnap` is where the item was before the grid took it, kept so that
/// turning snapping off puts it back rather than leaving it on the lattice.
#[derive(Debug, Clone, PartialEq)]
pub struct Geometry {
    pub id: String,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub rot: f32,
    pub z: f32,
    pub presnap: Option<PreSnap>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreSnap {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// A face this board carries with it, and the axes its `fvar` declares.
#[derive(Debug, Clone, PartialEq)]
pub struct FontSpec {
    /// Names bytes in the asset store.
    pub hash: String,
    /// Becomes a CSS family name in the original; a family name here.
    pub family: String,
    /// The variable axes, where the file's `fvar` could be read.
    pub axes: Vec<FontAxis>,
    /// `true` where it could not — meaning only "this file has an `fvar`".
    /// At most one of `axes` and `variable` is ever written.
    pub variable: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FontAxis {
    /// Four characters from `[A-Za-z0-9 ]`.
    pub tag: String,
    pub min: f32,
    pub default: f32,
    pub max: f32,
}

/// The board's own look: a palette name and a bag of custom properties.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Look {
    pub palette: String,
    pub vars: Map<String, Value>,
}

/// The board's settings, per layout profile.
///
/// Note what the original's reader actually enforces, because this type is
/// narrower than what a hand-edited file can put here: only the fields that
/// reach a stylesheet, a filename or the geometry are re-validated. The flags
/// are carried as they arrived and should be read as truthiness tests.
#[derive(Debug, Clone, PartialEq)]
pub struct BoardSettings {
    pub grid: bool,
    pub axes: bool,
    pub snap: bool,
    /// Whether hand-drawn connections are drawn. Still called `web` because
    /// that is the key an older build reads; renaming it would open every board
    /// that had them switched on with nothing between the cards.
    pub web: bool,
    /// Whether the scale bar is drawn over the canvas — see `paper::scale_bar`
    /// for the length and label it reads off `scale` and `units` below.
    pub hud: bool,
    /// Whether a dragged card lines itself up with its neighbours, and draws a
    /// rule to say what it lined up with. See [`crate::guides`].
    ///
    /// The sibling of `snap` and never on with it: a card cannot be on the
    /// lattice and flush with its neighbour at the same time, so the grid wins
    /// where both are asked for. Kept as its own flag rather than folded into
    /// `snap` because the two are opposite tastes — one is for a board being
    /// built to a measure, the other for a board being arranged by eye — and
    /// somebody who wants neither has to be able to say so.
    pub guides: bool,
    /// Only ever had one value. Kept because older files carry it.
    pub grid_style: String,
    /// World units between minor grid lines, before zoom quantisation.
    pub grid_step: f32,
    pub mobile_columns: u32,
    /// The gap the arrangement engine leaves between cards.
    pub spacing: f32,
    /// World units per millimetre. Geometry never reads it — it is a lens over
    /// numbers that were always unitless.
    pub scale: f32,
    /// `metric` or `imperial`.
    pub units: String,
    /// A sheet of standard paper outlined around the origin, or `""` for none.
    pub paper: String,
    pub paper_landscape: bool,
    pub paper_resize: bool,
    pub appearance: Look,
    pub fonts: Vec<FontSpec>,
}

impl Default for BoardSettings {
    fn default() -> Self {
        Self {
            grid: true,
            axes: true,
            snap: false,
            web: true,
            hud: false,
            guides: true,
            grid_style: "dots".into(),
            grid_step: 64.0,
            mobile_columns: 6,
            spacing: 12.0,
            scale: DEFAULT_SCALE,
            units: "metric".into(),
            paper: String::new(),
            paper_landscape: false,
            paper_resize: false,
            appearance: Look::default(),
            fonts: Vec::new(),
        }
    }
}

/// World units per millimetre, before anybody has calibrated the board against
/// a sheet of paper.
pub const DEFAULT_SCALE: f32 = 4.0;

/// The typography of the board's name, shared by the Mobile masthead and the
/// Desktop title card. Board-level: one style for both layouts.
#[derive(Debug, Clone, PartialEq)]
pub struct MobileHeader {
    pub font: String,
    pub size: f32,
    pub stretch: f32,
    pub leading: f32,
    pub weight: f32,
    pub offset: f32,
    pub italic: bool,
    pub wrap: bool,
    pub axes: Map<String, Value>,
}

impl Default for MobileHeader {
    fn default() -> Self {
        Self {
            font: String::new(),
            size: 13.0,
            stretch: 100.0,
            leading: 100.0,
            weight: 700.0,
            offset: 0.0,
            italic: false,
            wrap: true,
            axes: Map::new(),
        }
    }
}

/// How one connection is drawn, where it is drawn as anything but a plain line.
///
/// **Every value is a name, never a value** — a colour is `Leaf` and not a hex
/// triple. This object comes out of a file somebody else wrote, and in the
/// original a string that reached a stroke would be a string that reached the
/// CSSOM. Modelling them as enums is how that stays true here.
///
/// Not `Eq`, and the reason is [`label_at`](Self::label_at): where along a line
/// its label sits is a fraction, and a fraction is the one thing here that is
/// a measurement rather than a name.
#[derive(Debug, Clone, PartialEq)]
pub struct ConnMeta {
    pub dir: ConnDir,
    pub style: ConnStyle,
    pub color: ConnColor,
    pub weight: ConnWeight,
    /// Whitespace collapsed, 60 characters.
    pub label: Option<String>,
    /// Where the label sits along the line, from `0.0` at the first card to
    /// `1.0` at the second. Measured along the line's own length rather than
    /// across the gap, so it stays put as the line bends.
    ///
    /// Stored because it is a choice somebody made: the middle is where a
    /// label goes when nobody has said, and it is the wrong place often enough
    /// — over a card it crosses, on top of another line's label — that being
    /// able to slide it is the difference between a label and a label you can
    /// use.
    pub label_at: f32,
}

/// Where a label sits when nobody has moved it.
pub const LABEL_MIDDLE: f32 = 0.5;

impl Default for ConnMeta {
    fn default() -> Self {
        Self {
            dir: ConnDir::default(),
            style: ConnStyle::default(),
            color: ConnColor::default(),
            weight: ConnWeight::default(),
            label: None,
            label_at: LABEL_MIDDLE,
        }
    }
}

impl ConnMeta {
    /// Whether this is a plain line. Defaults are omitted on the way out, not
    /// written, so an ordinary board's connections stay two-element arrays.
    ///
    /// Spelled out rather than compared against [`Self::default`] so that
    /// `label_at` can be left out of it: with no label there is nothing at
    /// that position, and a line that once carried a label somebody slid
    /// should go back to being a two-element array when the words come off.
    pub fn is_default(&self) -> bool {
        let d = Self::default();
        self.dir == d.dir
            && self.style == d.style
            && self.color == d.color
            && self.weight == d.weight
            && self.label.is_none()
    }
}

macro_rules! named_enum {
    ($name:ident, $default:ident, { $($variant:ident => $text:literal),+ $(,)? }) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum $name { $($variant),+ }

        impl Default for $name {
            fn default() -> Self { Self::$default }
        }

        impl $name {
            /// Every value, in the order they are declared above.
            ///
            /// Generated from the same variant list that defines the enum, so
            /// a value added to the format is in here the moment it exists.
            /// That is the whole point of it being here rather than written
            /// out again at each place that wants the set: a menu row and a
            /// palette both build themselves by mapping over this, and neither
            /// can fall behind what the format defines.
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            pub fn as_str(&self) -> &'static str {
                match self { $(Self::$variant => $text),+ }
            }
            /// An unknown name is `None`, and every caller turns that into the
            /// default rather than into a failure: a colour a newer build knows
            /// should draw the ordinary line, not nothing at all.
            pub fn parse(s: &str) -> Option<Self> {
                match s { $($text => Some(Self::$variant),)+ _ => None }
            }
        }
    };
}

named_enum!(ConnDir, None, { None => "none", Fwd => "fwd", Back => "back", Both => "both" });
named_enum!(ConnStyle, Solid, { Solid => "solid", Dashed => "dashed", Dotted => "dotted" });
named_enum!(ConnColor, Line, {
    Line => "line", Accent => "accent", Warm => "warm", Leaf => "leaf", Danger => "danger",
});
named_enum!(ConnWeight, Normal, { Fine => "fine", Normal => "normal", Bold => "bold" });

/// A line somebody drew between two cards.
///
/// The pair is unordered — `(a, b)` and `(b, a)` are the same connection — but
/// the order is preserved anyway, because `dir` is read against it and `"fwd"`
/// means "points at the second id".
///
/// Nothing about the path is stored: where a line runs is a function of where
/// the two cards are now, so there is nothing to invalidate.
#[derive(Debug, Clone, PartialEq)]
pub struct Connection {
    pub a: String,
    pub b: String,
    pub meta: ConnMeta,
}

impl Connection {
    /// Order-independent identity, for collapsing duplicates.
    pub fn key(&self) -> (&str, &str) {
        if self.a <= self.b {
            (&self.a, &self.b)
        } else {
            (&self.b, &self.a)
        }
    }
}

/// One thing in the bin: the item as it was, and when it went in.
///
/// **The bin lasts as long as the app is open and no longer.** It is not
/// written to the file — see `mbrd::to_bytes`, which empties it on the way out
/// — so nothing binned survives a save, and a board opened with one in it loses
/// it the first time this app writes the file.
///
/// That is a retreat from what this section is in the format, and it is an
/// honest one. A bin earns its keep by being a place you can take things back
/// out of, and this app has no such place: `Del` bins, nothing unbins, and the
/// only route back is undo. What was left was a section of the file that
/// quietly kept every deleted photograph's bytes alive forever, priced as
/// though a restore were coming. Undo is the recovery route, it works within a
/// session and across a reopen — the ledger is written and the bytes a step
/// still names are written with it — and it does not need this.
///
/// Kept in memory rather than deleted outright because `delete_selection` puts
/// an item here in one move, and one place holding the item as it was is what
/// makes that a single undo step rather than a small pile of bookkeeping.
#[derive(Debug, Clone, PartialEq)]
pub struct TrashEntry {
    pub item: Item,
    /// Milliseconds since the Unix epoch, as the format writes it.
    pub at: i64,
}

/// Where the camera was left.
///
/// `zoom` is the raw world-to-screen scale, **not** the percentage the corner
/// prints. See `viewport.rs`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub pan_x: f32,
    pub pan_y: f32,
    pub zoom: f32,
}

impl Default for View {
    fn default() -> Self {
        Self { pan_x: 0.0, pan_y: 0.0, zoom: crate::viewport::BASE_ZOOM }
    }
}

/// A whole board.
#[derive(Debug, Clone, PartialEq)]
pub struct Board {
    pub title: String,
    pub view: View,
    /// Per-layout settings. Both profiles always exist.
    pub settings: LayoutPair<BoardSettings>,
    /// The named arrangement each layout was left in. On Mobile this names an
    /// *order* rather than a shape — the column is always packed the same way.
    pub arrangements: LayoutPair<String>,
    /// Per-layout geometry, keyed by item id. Content is shared; only where a
    /// card sits is per-layout.
    pub layouts: LayoutPair<Vec<Geometry>>,
    pub items: Vec<Item>,
    pub mobile_header: MobileHeader,
    /// `true` only when the Desktop title card has been deleted.
    pub title_hidden: bool,
    /// How photos and videos sit in their cards board-wide: `contain` or
    /// `cover`. One card can override it with `meta.fit`.
    pub media_fit: String,
    /// How many of the board's pictures the dynamic palette reads, newest
    /// first. `0` means every picture — the slider's stop past the top, not a
    /// count below its bottom.
    pub palette_sources: u32,
    pub connections: Vec<Connection>,
    /// The order the playlist plays the board's audio in.
    pub audio_order: Vec<String>,
    /// Somebody's route through the board, as a flat list of item ids.
    pub tour: Vec<String>,
    pub trash: Vec<TrashEntry>,
}

/// A value held once per layout profile.
///
/// A struct rather than a map because both profiles always exist and neither is
/// ever absent — a `HashMap` here would make every read a lookup that cannot
/// fail but has to be written as though it could.
#[derive(Debug, Clone, PartialEq)]
pub struct LayoutPair<T> {
    pub desktop: T,
    pub mobile: T,
}

impl<T> LayoutPair<T> {
    pub fn get(&self, mode: LayoutMode) -> &T {
        match mode {
            LayoutMode::Desktop => &self.desktop,
            LayoutMode::Mobile => &self.mobile,
        }
    }

    pub fn get_mut(&mut self, mode: LayoutMode) -> &mut T {
        match mode {
            LayoutMode::Desktop => &mut self.desktop,
            LayoutMode::Mobile => &mut self.mobile,
        }
    }
}

impl<T: Default> Default for LayoutPair<T> {
    fn default() -> Self {
        Self { desktop: T::default(), mobile: T::default() }
    }
}

impl Default for Board {
    fn default() -> Self {
        Self {
            title: String::new(),
            view: View::default(),
            settings: LayoutPair {
                desktop: BoardSettings::default(),
                mobile: BoardSettings {
                    mobile_columns: 8,
                    // A file with no Mobile settings record of its own is read
                    // at zero rather than inheriting Desktop's gap: that is
                    // what every board written before Mobile had a gap at all
                    // was actually saved looking like.
                    spacing: 0.0,
                    ..BoardSettings::default()
                },
            },
            arrangements: LayoutPair { desktop: "free".into(), mobile: "fit".into() },
            layouts: LayoutPair::default(),
            items: Vec::new(),
            mobile_header: MobileHeader::default(),
            title_hidden: false,
            media_fit: "contain".into(),
            palette_sources: 12,
            connections: Vec::new(),
            audio_order: Vec::new(),
            tour: Vec::new(),
            trash: Vec::new(),
        }
    }
}

impl Board {
    pub fn item(&self, id: &str) -> Option<&Item> {
        self.items.iter().find(|it| it.id == id)
    }

    pub fn item_mut(&mut self, id: &str) -> Option<&mut Item> {
        self.items.iter_mut().find(|it| it.id == id)
    }

    /// Every asset hash a **live** card on this board points at.
    ///
    /// This is the reference set the packer treats as required, and getting it
    /// wrong deletes somebody's photograph — so any other place that can hold
    /// an item and must survive a save has to be added here rather than
    /// checked separately at the call site.
    ///
    /// The bin is deliberately **not** one of those places. It is a
    /// within-a-session thing now — see [`TrashEntry`] — and its cards do not
    /// reach the file at all, so requiring their bytes would be requiring bytes
    /// for something nothing is going to read back. What a step of the undo
    /// ledger still names is answered separately, by
    /// [`BoardState::optional_hashes`](crate::state::BoardState::optional_hashes),
    /// and that is what actually carries a deleted picture far enough for an
    /// undo to find it.
    pub fn referenced_hashes(&self) -> Vec<String> {
        // A set for the dedup rather than a scan of what is already collected:
        // this runs on the autosave timer, and on a board where every card
        // carries a picture the scan was quadratic in the cards.
        let mut known: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut seen: Vec<String> = Vec::new();
        let mut push = |hash: Option<&str>| {
            if let Some(h) = hash {
                if !known.contains(h) {
                    known.insert(h.to_string());
                    seen.push(h.to_string());
                }
            }
        };
        for item in &self.items {
            push(item.asset.as_ref().and_then(ItemAsset::hash));
            // `meta.cover` is an asset hash too — album art, or a video's
            // poster. Missing it here would drop the picture from the file
            // while leaving the card that names it.
            push(item.meta.get("cover").and_then(Value::as_str));
        }
        for profile in [&self.settings.desktop, &self.settings.mobile] {
            for font in &profile.fonts {
                push(Some(font.hash.as_str()));
            }
        }
        seen
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Locking is a decision, and only a decision.
    ///
    /// Which is the whole of the rule: nothing about where an item sits may
    /// imply it, an absent key means unlocked, and a key holding anything but
    /// `true` is not an author saying yes. The last of those matters because
    /// `meta` is the format's extension point — an unknown key rides through
    /// untouched, so a later build's `locked: "sometimes"` must not read here
    /// as a card nobody can move.
    #[test]
    fn an_item_is_locked_only_where_its_author_said_so() {
        let mut item = Item::new("a", ItemType::Note);
        assert!(!item.locked(), "a board that has never heard of locking");

        item.meta.insert("locked".into(), Value::Bool(true));
        assert!(item.locked());

        item.meta.insert("locked".into(), Value::Bool(false));
        assert!(!item.locked());

        item.meta.insert("locked".into(), Value::String("yes".into()));
        assert!(!item.locked(), "a value from somewhere else is not a yes");

        item.meta.remove("locked");
        assert!(!item.locked());
    }
}
