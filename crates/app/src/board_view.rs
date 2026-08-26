//! The canvas that goes on forever, and the one gesture pipeline that drives it.
//!
//! **There is exactly one active gesture at a time, and it is decided here.**
//! The original states that rule as "do not split `canvas/input.ts`", and it is
//! worth keeping: the alternative is a mouse-down handler on every card racing
//! the one on the background, and a drag that starts on a card but ends on
//! empty space behaving differently from one that does not.
//!
//! So the cards rendered by this module carry **no event handlers at all**.
//! They are presentation. Every press, move and release lands on the container,
//! is converted to world coordinates once, and is hit-tested against the item
//! list by hand. One place decides what a gesture is; one place ends it.

use gpui::{
    canvas, div, fill, prelude::*, px, quad, relative, App, BorderStyle, Bounds, ContentMask,
    Context, FocusHandle, Focusable, Font, FontFallbacks, FontStyle, FontWeight, Hsla,
    KeyDownEvent, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, RenderImage,
    ScrollDelta, ScrollWheelEvent, ShapedLine, SharedString, StrikethroughStyle, TextRun,
    UnderlineStyle, Window,
};

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mbrd_core::align::Axis;
use mbrd_core::arrange::{self as arranging, Arrangement};
use mbrd_core::geometry::{self, point, Point as WorldPoint, Rect};
use mbrd_core::guides::{self, Line, Snap};
use mbrd_core::index::Grid;
use mbrd_core::model::{ConnMeta, Item, ItemAsset, ItemType, TrashEntry, View};
use mbrd_core::motion::{Spring, Sprung};
use mbrd_core::rope::{self, Side};
use mbrd_core::state::Pending;
use mbrd_core::viewport::{ViewSize, Viewport, BASE_ZOOM, MIN_ZOOM};
use mbrd_core::Document;

use crate::anchor;
use crate::camera::{Camera, Trail};
use crate::command::{Command, Entry};
use crate::editor::{self, Editor};
use crate::fetch;
use crate::grips::Grip;
use crate::icons::{icon, Icon};
use crate::images::{Images, Load, THUMB_SIDE};
use crate::import;
use crate::live::Live;
use crate::markdown;
use crate::menu::Menu;
use crate::metrics::{Advance, Measure};
use crate::palette::{Palette, What};
use crate::playback::{Media, Timings};
use crate::prefs::Prefs;
use crate::switcher::{Reply, Switcher};
use crate::taps::{Tap, Taps};
use crate::theme::Theme;
use crate::tools::Tool;
use crate::transport::{self, Face};
use crate::update;
use crate::wires::{self, Wire, Wires};
use mbrd_core::align;
use mbrd_core::fence::Fences;

/// `Pixels` as a plain number.
///
/// The conversions between world units and screen pixels are arithmetic, and
/// doing that arithmetic in a newtype means writing `px()` and `.into()` around
/// every term. The boundary is narrow enough to cross once, here.
fn f(p: Pixels) -> f32 {
    p.into()
}

/// How much world space beyond the visible rectangle still gets drawn.
///
/// Nonzero so that a card half off the edge is not clipped into existence as it
/// scrolls in, which reads as a pop rather than a pan.
const CULL_MARGIN: f32 = 400.0;

/// How many grid dots may be queued in one frame before the grid gets coarser.
///
/// A ceiling rather than a budget: past it the spacing doubles and the count
/// quarters, so the grid answers a window it cannot fill by drawing a wider
/// grid rather than by drawing none. Generous, because the dots are painted in
/// one layer and a quad in a layer costs a push rather than a tree insert —
/// what this now guards is the vertex count, not the frame.
const MOST_DOTS: i64 = 40_000;

/// The longest a connection's label may be. The format's own ceiling.
const LABEL_MAX: usize = 60;

/// The longest address a link card will take.
///
/// Nothing in the format says so. Two thousand and forty-eight is the length
/// past which browsers, proxies and server logs start quietly disagreeing with
/// each other about what the address was, so it is the length past which an
/// address has stopped being one.
const URL_MAX: usize = 2_048;

/// How near a rope the pointer has to be, in screen pixels, to press it.
///
/// Generous, because a rope is a line rather than a shape and there is nothing
/// underneath it to hit by accident — the cards are tested first.
const ROPE_REACH: f32 = 7.0;

/// How far the pointer has to travel, in screen pixels, before a press
/// becomes a drag rather than a click that wobbled.
///
/// Shared by every gesture that has to tell the two apart: a card drag (see
/// [`BoardView::drag_cards`]), a pan on empty paper (see the `Panning` arm of
/// [`BoardView::on_mouse_move`]), and a resize (see the `Sizing` arm). Screen
/// pixels rather than world units, so the same wobble is forgiven at every
/// zoom level — a shake that is four world units at 4x zoom is one screen
/// pixel, and the same four world units unzoomed is most of a hit target.
/// One constant rather than three, because a card that commits sooner than
/// the paper it is sitting on would be a seam nobody could explain.
const ENOUGH: f32 = 4.0;

/// One wheel notch. Small enough that a trackpad's many small deltas do not
/// rocket through the whole zoom range in one flick.
const ZOOM_PER_LINE: f32 = 0.12;

/// How far an arrow key pans the camera, in screen pixels, when there is
/// nothing selected to nudge instead.
///
/// A comfortable glance rather than a crawl or a jump: small enough that a
/// held key sweeps the view smoothly through `Camera::nudge`'s spring, large
/// enough that finding something a few screens away does not take a couple
/// of hundred taps to get there.
const KEY_PAN_STEP: f32 = 160.0;

/// Level of detail: the sizes, on screen and in pixels, at which a card stops
/// being worth the work of drawing properly.
///
/// This is where a board of twenty thousand cards is won or lost. Zoomed out, a
/// card is four pixels across; a border on it is a smudge, a shadow is a
/// smudge, and a label is a text shaping — the single most expensive thing per
/// card by a wide margin — that resolves to a grey blur. Each threshold below
/// is one thing stopping happening, and the order they stop in is the order
/// they stop being legible.
///
/// Below this a card is one flat quad and nothing else.
const LOD_DUST: f32 = 3.0;
/// Below this there is no rounding and no border, just a block of colour.
const LOD_PLAIN: f32 = 8.0;
/// Below this a picture is not worth an atlas tile.
const LOD_PICTURE: f32 = 6.0;

/// How many image decodes may be in flight at once.
///
/// The bound is on the *starts*, not on the wanting: `draw_list` simply stops
/// asking once this many are out, and asks again next frame. High enough to
/// keep every core fed — a decode is CPU work on the background pool — and low
/// enough that the copies of encoded bytes riding along with them stay a
/// handful of files rather than the whole archive at once.
const DECODES_AT_ONCE: usize = 16;
/// The card has to be at least this wide for a label to be worth reading.
///
/// Width only. How short a card may be is not a number here but a question
/// asked of the text — does one line of it fit? — because the two are not the
/// same question and a flat number was answering the wrong one: it hid the
/// words on cards that had room for them, which is the one failure a level of
/// detail must not have.
const LOD_LABEL_W: f32 = 24.0;

/// How big the words on a card are drawn, in screen pixels **before** the
/// card's own answer to [`Command::DontScaleText`] is applied — see [`card_text`].
///
/// By default this does not depend on the zoom, and that is the whole point of
/// it being a constant. Text that scales with the camera turns a note into a
/// picture of a note: zoom out and it is an illegible smudge, zoom in and
/// three words fill the window. Text that stays put is a *label* — the board
/// under it grows and shrinks, and what is written on it stays readable the
/// whole way, which is what a map does and what makes a map legible at every
/// scale.
///
/// The cost is deliberate and worth naming: a card zoomed a long way in has a
/// lot of empty space around a small line of text, because the card is a thing
/// on the board and the words on it are not. That cost is exactly what the
/// per-card setting is for, and why it is per card rather than per board:
/// a caption wants to be a label and a title wants to be part of the picture,
/// and they can sit next to each other on the same board.
const CARD_TEXT: f32 = 13.0;

/// The air between a card's edge and its words, in screen pixels.
///
/// Scaled or not scaled alongside [`CARD_TEXT`], never on its own: padding
/// that stayed put while the text grew would push the words out of the card,
/// and padding that grew while the text stayed put would make them appear to
/// shrink into the corner as the camera came in.
const CARD_PAD: f32 = 8.0;

/// The smallest a word may be drawn before it is not drawn at all.
///
/// Only reachable on a card whose text scales, and it is the same argument as
/// the rest of the `LOD_` block: below this a line of text is a grey smear
/// that costs a full shaping to produce. The card keeps its shape and loses
/// its words, which is what every other threshold here does too.
///
/// Low, deliberately. A word going illegible and a word going *absent* look
/// nothing alike — the first is a board seen from far away and the second is a
/// board that has lost something — so this sits below where the text stops
/// being readable rather than at it.
const LOD_TEXT: f32 = 4.0;

/// The distance from one line of a card's text to the next, as a multiple of
/// the size — and not the same multiple everywhere. A heading is read at a
/// glance and wants to sit close to the words under it; a paragraph is read a
/// line at a time and wants the air that makes the next line easy to find.
/// One flat number for both, which is what this replaces, got that backwards:
/// it was tuned tight enough for a heading and then used, unchanged, on
/// thirteen-pixel body text that wanted to breathe more than that.
///
/// **`size` must be zoom-independent** — [`CARD_TEXT`] times a line's own
/// [`markdown::Line::scale`], never the *zoomed* size a card is currently
/// drawn at. Every call site below multiplies the zoomed size by whatever this
/// returns to get an actual pixel gap, and if the bracket chosen here also
/// moved with the zoom, crossing one of the edges below by moving the camera
/// — rather than by editing a word — would silently change how many lines fit
/// and reflow a note the camera never touched.
///
/// Shared by the painter, the row budget, the caret and [`fitted_height`], for
/// the reason the constant this replaces used to give: a disagreement between
/// any two of them puts the caret on the wrong row.
fn leading(size: f32) -> f32 {
    if size >= 30.0 {
        1.10
    } else if size >= 20.0 {
        1.18
    } else if size >= 16.0 {
        1.28
    } else if size < 8.0 {
        1.50
    } else {
        1.45
    }
}

/// A frame long enough that everything on the clock has already finished.
///
/// What reduced motion runs at. Not infinity: the springs are evaluated with
/// an exponential, and an infinite exponent is a `NaN` camera rather than an
/// arrived one.
const FOREVER: f32 = 10.0;

/// How close an arranged card's presentation has to land to its model
/// position before the catch-up spring counts it as arrived.
///
/// A quarter of a screen pixel, the same precision `camera.rs`'s `PAN_REST`
/// settles the camera to, and converted from screen space to world space the
/// same way: divided by the zoom at the call site. A world-unit constant
/// would settle far too early zoomed in, where a quarter of a *card* is still
/// a visible drift, and never at all zoomed out, where the pixel it is
/// chasing does not exist.
const PRESENTING_REST: f32 = 0.25;

/// How long the marks beside a card take to arrive, in seconds.
///
/// They are an *offer* — see `anchor.rs`, which says so — and an offer that
/// snaps into existence at full strength is a demand. Short enough that
/// pointing at a card and starting a rope is not a wait.
const ANCHOR_IN: f32 = 0.12;

/// How long they take to go, in seconds.
///
/// Longer than they take to arrive, deliberately. A pointer skimming across a
/// crowded board crosses a dozen cards a second, and marks that left as fast
/// as they came would make the whole board flash; leaving slowly turns that
/// into something closer to a wake.
const ANCHOR_OUT: f32 = 0.2;

/// How long the menu, the switcher and the palette take to arrive, in
/// seconds — and, on the loading panel, how long it takes to appear.
///
/// Short, on the same reasoning as [`ANCHOR_IN`]: a surface that snaps into
/// existence at full strength off a key press is fine, but one that snaps in
/// off a stray Shift-Shift while somebody is typing capitals is a flash they
/// did not ask for. The offset that rides along with it — see
/// `BoardView::advance_overlay` — is what actually reads as motion; the fade
/// alone would be too quick to notice either way.
const OVERLAY_IN: f32 = 0.12;

/// How long they take to leave, in seconds.
///
/// Longer than they take to arrive, on the same reasoning as [`ANCHOR_OUT`]:
/// closing is not a thing you aimed at the way opening was, so it gets a
/// beat longer to read as *going away* rather than as a flicker.
const OVERLAY_OUT: f32 = 0.16;

/// How close a presence spring has to be to count as arrived — under half a
/// percent of opacity, which is beneath what a monitor can show.
const PRESENCE_REST: f32 = 0.004;

/// How much of a page's arrival goes on covering the board over. See
/// [`arrival`].
const PAGE_COVER: f32 = 0.35;

/// How far below where it belongs a page's content starts, in pixels.
///
/// Smaller than the 8px a panel slides, and in the other direction: a panel
/// drops in from off the top edge it came from, while a page is already
/// filling the window and only its contents are settling. Six pixels is
/// enough to read as motion on a body of text and not enough to read as the
/// page having been in the wrong place.
const PAGE_RISE: f32 = 6.0;

/// A switch's knob crossing its track, and a segmented row's wash crossing
/// its segments.
///
/// Critically damped and quicker than [`Spring::SURFACE`]: a control this
/// small crosses fourteen pixels, and the motion is there to say *which way
/// it went*, not to be watched.
const KNOB: Spring = Spring::new(1.0, 0.15);

/// The face the board sets its words in, and what to fall back to where the
/// machine has not got it.
///
/// Out here, and read by both the root element and [`BoardView::new`]'s
/// measurer, because those two disagreeing would mean measuring a wrap in a
/// face the painter does not use — which is the entire failure `metrics.rs`
/// was written to end.
const BODY_FAMILY: &str = ".SystemUIFont";
const BODY_FALLBACKS: [&str; 5] =
    ["Inter", "Cantarell", "Adwaita Sans", "Noto Sans", "DejaVu Sans"];

fn body_font() -> Font {
    let mut font = gpui::font(BODY_FAMILY);
    font.fallbacks =
        Some(FontFallbacks::from_fonts(BODY_FALLBACKS.iter().map(|s| s.to_string()).collect()));
    font
}

/// The words on a line: the size they are set at, and the padding of the chip
/// drawn behind them.
///
/// Out here because two places have to agree about where that chip is — the
/// painter that draws it, and `label_at`, which decides whether a press landed
/// on it. A label you can read and cannot grab is worse than no label at all.
const LABEL_TEXT: f32 = 11.0;
const LABEL_PAD: f32 = 5.0;
/// The chip's height, as a multiple of the text size.
const LABEL_LEADING: f32 = 1.5;
/// Below this the label is not drawn at all, so there is nothing to press
/// either — see `label_at`, which asks the same question the painter does.
const LABEL_ZOOM: f32 = 0.25;

/// Where a card lands when a move has carried the pointer `(dx, dy)` from where
/// it was pressed.
///
/// `home` is where the card sat at the press, before the grid had any say, and
/// the offset is measured against that rather than against the previous frame.
/// The difference matters only when `to_grid` is on, and then it is the whole
/// feature: rounding a card's position every frame and adding the next frame's
/// delta to the rounded result quantises the delta itself, so any drag slower
/// than half a step per frame rounds straight back to where it started and the
/// card never leaves the first cell it snapped to.
fn dropped_at(home: WorldPoint, dx: f32, dy: f32, to_grid: Option<f32>) -> WorldPoint {
    let free = point(home.x + dx, home.y + dy);
    match to_grid {
        Some(step) => point(geometry::snap(free.x, step), geometry::snap(free.y, step)),
        None => free,
    }
}

/// Move a presence value a frame nearer 0 or 1, and say whether it moved.
///
/// Linear rather than sprung, on the same reasoning [`BoardView::fade_anchors`]
/// gives for the anchors: this is a light coming up, not a thing being moved,
/// and a spring on an opacity buys overshoot nobody can see and a settle time
/// everybody can. The loading panel is the one thing left on it — the
/// overlay used to share it, until the overlay grew a slide, and a thing
/// that moves wants what a spring has: see `overlay_presence`.
fn step_presence(presence: &mut f32, leaving: bool, dt: f32) -> bool {
    let target = if leaving { 0.0 } else { 1.0 };
    if *presence == target {
        return false;
    }
    let rate = if leaving { OVERLAY_OUT } else { OVERLAY_IN };
    let step = dt / rate;
    *presence = if *presence < target {
        (*presence + step).min(target)
    } else {
        (*presence - step).max(target)
    };
    true
}

/// A full-window page arriving, which is two motions and not one.
///
/// The menu, the switcher and the palette are **panels**: something small
/// over a board that goes on existing behind them, so one opacity across the
/// whole panel is honest — you are meant to see through it while it comes.
///
/// The settings page and the open card are not panels. They take the whole
/// window below the titlebar, and fading one of *those* in on a single number
/// means every frame in the middle is half a board and half a page at once.
/// That is a cross-dissolve, which is what a slideshow does between two
/// photographs and not what a window does when it changes what it is showing:
/// it reads as ghosting rather than as motion, and it reads worse the busier
/// the board behind it is.
///
/// So the two halves come apart. The **ground** goes solid over the first
/// third of the travel — the board is covered decisively, and after that
/// there is nothing left to see through. The **content** then fades and rises
/// the last few pixels onto a page that is already opaque, which is motion
/// against a fixed backing and is the part that actually reads as arriving.
///
/// Both are functions of the one presence spring, so the exit is still the
/// entrance played backwards and an Escape pressed mid-arrival still bends
/// out of the motion it is already in rather than starting a second one.
#[derive(Debug, Clone, Copy)]
pub struct Arrival {
    /// How opaque the page's own ground is, over the board behind it.
    pub ground: f32,
    /// How opaque everything drawn on that ground is.
    pub content: f32,
    /// How far below where it belongs that content still is, in pixels.
    pub rise: f32,
}

/// Split a page's presence into [the two things it is](Arrival).
pub fn arrival(presence: f32) -> Arrival {
    let content = ((presence - PAGE_COVER) / (1.0 - PAGE_COVER)).clamp(0.0, 1.0);
    Arrival {
        ground: (presence / PAGE_COVER).clamp(0.0, 1.0),
        content,
        // Tied to the fade rather than to the presence, so the content stops
        // moving exactly as it finishes appearing instead of creeping the last
        // fraction of a pixel after it is already fully drawn.
        rise: PAGE_RISE * (1.0 - content),
    }
}

/// One card a move is holding: which card, where it sat when the press
/// landed, and where it lives in the item list.
///
/// The index and the frame are what make a drag cost the drag rather than the
/// board. A pointer event used to look every held id up with `Board::item`,
/// which is a scan of the items — selection times cards, per event, twice
/// (once to move and once for the guides). Nothing removes or reorders items
/// while a gesture is open — the one thing a drag adds, its own copies, is
/// appended — so an index taken at the press holds until the release. The id
/// rides along and is checked before every indexed write, so a board that
/// somehow shifted anyway degrades to the scan rather than to writing through
/// a stale index.
#[derive(Debug, Clone)]
struct Grabbed {
    id: String,
    /// Where the card sat when the press landed, before the grid had any say.
    home: WorldPoint,
    /// Where the card lives in `board.items`, taken at the press.
    index: usize,
    /// The card's size when the press landed, for the guides: a card does not
    /// change size mid-move, so the board need not be asked again per event.
    w: f32,
    h: f32,
}

/// What the pointer is currently in the middle of doing.
///
/// An enum rather than a set of booleans, because the states are genuinely
/// exclusive and the bug this prevents — panning and dragging a card at the
/// same time — is one that only shows up on a fast mouse.
#[derive(Debug, Clone)]
enum Gesture {
    None,
    /// Dragging the board itself.
    Panning {
        from: WorldPoint,
        /// Whether the pointer has actually travelled. A press that has not is
        /// a *click*, and a click on the paper means something the drag does
        /// not — see `clearing`.
        moved: bool,
        /// Whether a click that ends this pan should let go of the selection.
        ///
        /// True for the plain press on empty paper, which is the gesture
        /// everybody means as "nothing, thanks"; false for the middle button
        /// and for the Pan tool, which pan from anywhere including from on top
        /// of the card you have selected and did not mean to drop.
        clearing: bool,
    },
    /// Dragging a card, and anything selected along with it.
    Moving {
        /// Where the press landed, in world units. The whole offset is measured
        /// against this rather than against the previous frame, because a
        /// snapped card is not somewhere the next frame's delta can be added
        /// to: rounding after every frame quantises the delta itself, and a
        /// drag slower than half a grid step per frame never moves at all.
        from: WorldPoint,
        /// Each moving card and where it sat when the press landed, before the
        /// grid had any say. The free position the offset is applied to.
        start: Vec<Grabbed>,
        /// Whether the pointer has actually travelled. A press that never moves
        /// is a click, and must not push an undoable move.
        moved: bool,
        /// Which way the drag has been pinned, while `Shift` is down.
        ///
        /// Decided once, the first frame the key is seen, from whichever axis
        /// the pointer has travelled further along — and then *kept* until the
        /// key comes back up. Deciding it every frame would let a drag that
        /// wandered near the diagonal flip between horizontal and vertical
        /// several times a second.
        lock: Option<Axis>,
        /// Whether this drag has already left a copy behind it.
        ///
        /// `Alt` duplicates, and it duplicates **once**: the copies are made
        /// the first frame the key is seen and stay where the press landed
        /// while the originals carry on under the pointer. Same picture as
        /// Figma's — one set left behind, one set moving — and it means the
        /// moving id set never changes mid-gesture, which every other part of
        /// this drag relies on.
        copied: bool,
        /// What the cards lined up with on the last frame, ready to draw.
        ///
        /// Held on the gesture rather than recomputed by the painter, because
        /// it is worked out from a position the painter does not have: where
        /// the pointer *would* have put the cards, before the correction.
        guides: Snap,
        /// The board as it was when the press landed.
        ///
        /// A drag is **one** step rather than one per frame, and this is what
        /// makes it so: the picture is taken once at mouse-down, the cards move
        /// freely under the pointer, and the whole gesture is closed into a
        /// single entry at mouse-up. It is also the only thing that unlocks a
        /// write to the board, so there is no spelling of a drag that quietly
        /// escapes the ledger.
        open: Pending,
    },
    /// Dragging one of the handles around a card.
    ///
    /// One card, not the selection: resizing several at once means deciding
    /// what "the same size" means for cards of different shapes, and every
    /// answer is somebody's wrong one. Pressing a handle is unambiguous about
    /// which card it belongs to, so that is the card that resizes.
    Sizing {
        id: String,
        grip: Grip,
        /// The card as it was when the handle was pressed. Everything is
        /// measured against this rather than against the last frame, so a
        /// clamp partway through a drag does not accumulate.
        start: Rect,
        /// Where the press landed, in world units — for telling a drag from a
        /// click, same as `Moving::from` and `Panning::from`. Left alone by
        /// `hold` below, which corrects the *edge*, not where "far enough to
        /// be a drag" is measured from.
        from: WorldPoint,
        /// The world-space offset from the press to the grip's own spot — see
        /// [`Grip::spot`].
        ///
        /// The hit band around a handle is nine pixels wide (`grips::REACH`),
        /// so a press is rarely on the edge it grabs. `grips::resized` sets
        /// the dragged edge *to the pointer*, so without this the edge jumps
        /// by however far off-centre the press was on the very first frame of
        /// the drag. Added back to the pointer every frame instead, so the
        /// edge keeps the offset the press started with rather than snapping
        /// out from under it.
        hold: WorldPoint,
        /// The shape the card wants to keep, as width over height, where it has
        /// one worth keeping — the picture's own proportions, for a card that
        /// is a picture of something. This is what makes a photograph resize
        /// proportionally *by default*: stretching one is not a thing anybody
        /// means to do, so it is the modified gesture rather than the plain one.
        shape: Option<f32>,
        moved: bool,
        /// Whether this drag has reframed the picture rather than resized it.
        /// Only for the label on the step: "Crop" and "Resize" are different
        /// things to see in an undo history.
        cropping: bool,
        open: Pending,
    },
    /// Sweeping out a selection *of text*, inside the card being typed into.
    ///
    /// Nothing is carried on it. The anchor is already where the press put it —
    /// see [`crate::editor::Editor::place`] — so every frame of the drag is the
    /// same call the press made with `extend` set, and the release has nothing
    /// to close: a text selection is not board state and costs no undo step.
    SelectingText,
    /// Sweeping out a selection rectangle over empty space.
    Marquee {
        from: WorldPoint,
        to: WorldPoint,
        additive: bool,
        /// What the sweep would catch if the hand let go right now — the same
        /// pick the release itself runs, kept up to date every frame instead
        /// of only at the end. See `BoardView::update_marquee`.
        ///
        /// A set rather than a re-check against `self.selection`, because
        /// membership is all the painter asks of it and a set answers that in
        /// one lookup per card drawn instead of a scan of the sweep.
        provisional: HashSet<String>,
    },
    /// Dragging the playhead along a card's scrubber.
    ///
    /// No `Pending`, and not because it was forgotten: a playhead is not board
    /// state. It is not saved, not sent to anybody and not worth an undo step —
    /// see `playback.rs`, which owns the distinction.
    Scrubbing {
        id: String,
    },
    /// Dragging a card's volume slider.
    ///
    /// This one *does* carry a `Pending`, because how loud a card is **is**
    /// board state: it is saved, it travels with the file, and one drag of the
    /// slider should be one thing to take back rather than forty.
    Louder {
        id: String,
        open: Pending,
    },
    /// Turning a mesh card's camera, in Position mode on the board or always
    /// in the opened page — see `Command::Position` and `BoardView::positioning`.
    ///
    /// A `Pending`, like `Louder`: the orbit is board state, saved with the
    /// file, and one drag is one undo step rather than one per frame.
    Orbiting {
        id: String,
        /// Screen pixels, not world units — a mesh has no board-space
        /// rotation to measure a drag against. Every frame's turn is the
        /// distance the pointer has travelled from here, not from the last
        /// frame, for the reason `Sizing::start` measures from the press.
        from: gpui::Point<Pixels>,
        /// The orbit as it was when the press landed.
        start: mbrd_core::media::Orbit,
        /// Shift was held at the press — decided once, there, rather than
        /// read again every frame, so letting go of Shift mid-drag does not
        /// switch a turn into a pan under the pointer.
        panning: bool,
        moved: bool,
        open: Pending,
    },
    /// Dragging a rope out of one of a card's anchors.
    ///
    /// No `Pending` here, unlike every other gesture that changes the board:
    /// nothing is written until the release, because a rope that does not land
    /// on a card is not a rope that was drawn and undone — it is a rope that
    /// was never drawn. Opening a step at the press would leave an empty one
    /// for every stroke that went nowhere.
    Roping {
        /// The card it came out of, and the face it left by.
        from: String,
        side: Side,
        /// Where the loose end is, in world units.
        at: WorldPoint,
        /// The card under the loose end, if it is over one. Worked out on the
        /// move rather than at the release so the far card can be lit up while
        /// the hand is still down — otherwise you find out whether it took
        /// only after letting go.
        over: Option<String>,
    },
    /// Dragging a connection's label along the line it sits on.
    ///
    /// Named by the two cards, like everything else about a connection: a line
    /// has no id of its own, and the pair is what survives the board being
    /// edited underneath the gesture.
    Sliding {
        a: String,
        b: String,
        /// Whether the label has actually gone anywhere. A press that never
        /// moved it is a click on the label, which means what a click anywhere
        /// else on the line means — and must not leave a step in the ledger
        /// saying the label was moved.
        moved: bool,
        /// The fraction the label was already at, minus how far along the
        /// line the press itself landed.
        ///
        /// Without this, every frame writes `how_far_along(pointer)` straight
        /// into `label_at`, which snaps the label's *centre* to the pointer on
        /// the very first frame of the drag — a chip grabbed off-centre jumps
        /// before it ever moves. Added back to `how_far_along` on every frame
        /// instead, the same way `Sizing::hold` keeps a resize from jumping
        /// under an off-centre press on a handle.
        offset: f32,
        open: Pending,
    },
}

/// Which of a card's pieces of text is being typed into.
///
/// Three rather than two, because the window a card opens into types into
/// everything a card *has* rather than only the thing it shows — see
/// [`mbrd_core::preview::editable`], which is where the list of what a given
/// card has lives. A swatch is deliberately not a fourth variant: in this
/// format a swatch's colour and its name are the same value, and [`write_field`]
/// is the one place that has to know it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// The label on the card. For a swatch, also its colour.
    Name,
    /// A note's words — or the whole of the file behind a card that came
    /// from one, which is the same field with a different limit and a different
    /// commit. See `Editing::file`.
    Note,
    /// A link's address.
    Url,
}

/// What a text session is typing into.
///
/// Two shapes rather than one id, because a connection is not a card and has
/// no id of its own: it is named by the two cards it joins. Modelling that as
/// "an id that is sometimes two ids" is how a field ends up written onto the
/// wrong thing, so the two are different variants and the compiler asks which
/// one at every use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Subject {
    /// A card, and which of its two pieces of text.
    Card(String, Field),
    /// A connection's label, named by the two cards it joins in the order the
    /// board carries them.
    Rope(String, String),
}

impl Subject {
    /// The card being typed into, where it is one.
    ///
    /// The painter asks this to decide which card draws a caret, and a rope's
    /// label is not drawn on a card — see `rope_field` in `render`.
    pub fn card(&self) -> Option<(&str, Field)> {
        match self {
            Self::Card(id, field) => Some((id.as_str(), *field)),
            Self::Rope(..) => None,
        }
    }
}

/// Something being typed into.
#[derive(Debug, Clone)]
pub struct Editing {
    pub on: Subject,
    pub editor: Editor,
    /// What was there before, so Escape can put it back.
    before: String,
    /// Whether this is the card's **file** being typed rather than the card's
    /// own words.
    ///
    /// The two are different pieces of state — see the header of `opened.rs` —
    /// and they commit differently: the card's words are a `meta.text` the
    /// ledger writes directly, while a file is new bytes in the archive and a
    /// card repointed at them. Only the open window ever sets this; typing on
    /// the board is always the card's words.
    file: bool,
    /// The whole session is one step. Typing forty characters and pressing
    /// Escape should be one thing to undo, not forty.
    open: Pending,
}

/// A selection that was let go of, kept so that Ctrl Z can hand it back.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Held {
    /// The cards that were selected, in the order they were.
    cards: Vec<String>,
    /// Or the connection that was, since only ever one of the two is live.
    rope: Option<(String, String)>,
    /// The board revision it was let go at. See [`LetGo`].
    at: u64,
}

/// The selections let go of since the board last changed.
///
/// **Selection is not in the ledger and this does not put it there.** A step in
/// `history.rs` is a difference to the board that survives a save, and what was
/// selected on somebody's screen on Tuesday is not a fact about the board. So
/// this is a short stack held beside it, and the board's own revision is what
/// keeps the two in order: a let-go is the newest thing to take back only while
/// the board has not moved since it happened. The moment it has, the stack is
/// out of date — a selection restored across an edit would be restored onto a
/// board that is not the one it was made on — and it is dropped whole.
///
/// Only *losses* go on it. Selecting is a click and reselecting is another one;
/// what is worth a keystroke is the click that let go of forty cards.
#[derive(Debug, Default)]
struct LetGo {
    /// What Ctrl Z would hand back, newest last.
    back: Vec<Held>,
    /// What it already has, for Ctrl Shift Z to let go of again.
    forward: Vec<Held>,
}

impl LetGo {
    /// Record a selection being let go of.
    fn push(&mut self, cards: Vec<String>, rope: Option<(String, String)>, at: u64) {
        self.back.push(Held { cards, rope, at });
        // A new one truncates the other way, exactly as a new edit truncates
        // the ledger's redo: what was taken back is no longer ahead of you.
        self.forward.clear();
    }

    /// Whether Ctrl Z would hand a selection back rather than walk the ledger.
    fn holding(&self, at: u64) -> bool {
        self.back.last().is_some_and(|held| held.at == at)
    }

    /// The newest selection let go of, if the board has not moved since.
    ///
    /// Takes `&mut self` because a stack that has fallen behind the board is
    /// dropped on the way past: leaving it would mean a later undo, at some
    /// revision it happened to match again, handing back a selection from
    /// before an edit nobody connects it to.
    fn take_back(&mut self, at: u64) -> Option<Held> {
        if !self.holding(at) {
            self.back.clear();
            return None;
        }
        let held = self.back.pop()?;
        self.forward.push(held.clone());
        Some(held)
    }

    /// And let go of it again.
    fn again(&mut self, at: u64) -> Option<Held> {
        if !self.forward.last().is_some_and(|held| held.at == at) {
            self.forward.clear();
            return None;
        }
        let held = self.forward.pop()?;
        self.back.push(held.clone());
        Some(held)
    }

    /// Drop the lot — the ids on it name cards on a board that is not open.
    fn forget(&mut self) {
        self.back.clear();
        self.forward.clear();
    }
}

/// A line in the status bar, and how long it has left.
///
/// The two kinds are the point of this being a struct rather than a string.
/// **A completion is transient and a mode is not**: "moved 3" describes
/// something that finished, and describing it half an hour later is a lie
/// about the present; "pan — drag anywhere" describes where you *are*, and
/// timing it out would leave a mode running with nothing on screen to say so.
///
/// Failures get longer than completions rather than forever. Long enough to
/// look up and read; not so long that a board carries somebody's typo around
/// all afternoon.
#[derive(Debug, Clone)]
struct Said {
    text: String,
    /// When to stop saying it. `None` stands until something replaces it.
    until: Option<Instant>,
    /// Which of the three it is, and therefore which picture goes beside it.
    tone: Tone,
}

/// What kind of thing the bar is saying — and therefore whether it is said at
/// all.
///
/// **The division that matters is whether you could have seen it yourself.**
/// The bar used to narrate every action: "moved 3", "tinted 2", "brought to
/// front". Every one of those describes something that had just happened on the
/// board in front of you, so it was a second, slower copy of what you had
/// already watched — arriving in the corner you were not looking at. Those are
/// [`Tone::Done`], and they are no longer drawn; what is on the board is the
/// report. See [`BoardView::status_bar`], which now spends that space on what
/// is *on* the board rather than on what just happened to it.
///
/// The other three are things the board cannot show, and all three are drawn:
///
/// - [`Tone::Wrong`] — a failure. The one thing that did **not** happen, so
///   there is nothing on screen to have watched.
/// - [`Tone::Told`] — something you could not otherwise know: a download's
///   progress, or that a key you just pressed had nothing to do. "Nothing to
///   undo" is not narration, it is the *absence* of the thing narration would
///   have described.
/// - [`Tone::Mode`] — where you are, rather than what happened. It stands until
///   you leave, which is the opposite of a message that pops up: a mode you
///   cannot see is a trap, and every mode line here names the key that leaves
///   it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tone {
    /// Something finished, and you watched it finish. Not drawn.
    Done,
    /// Something did not.
    Wrong,
    /// Something that happened out of sight, or did not happen at all.
    Told,
    /// Where you are, rather than what happened. Stands until replaced.
    Mode,
}

impl Tone {
    /// Whether the bar draws a line of this kind at all.
    fn shown(self) -> bool {
        !matches!(self, Tone::Done)
    }

    /// The picture that goes beside it.
    ///
    /// A mode gets a keyboard because every mode line in this app names a key
    /// to press to leave it — "escape for select", "enter to keep". That is
    /// what makes it a mode rather than an event: there is a way out, and the
    /// line is where it is written.
    fn icon(self) -> Icon {
        match self {
            Tone::Wrong => Icon::Warned,
            Tone::Mode => Icon::Mode,
            Tone::Done | Tone::Told => Icon::Told,
        }
    }

    /// What colour it is drawn in. Only a failure earns the accent.
    fn colour(self, theme: &Theme) -> gpui::Hsla {
        match self {
            Tone::Wrong => theme.accent,
            _ => theme.muted,
        }
    }
}

/// How far along the update is, and therefore what `Ctrl U` does next.
///
/// One command walks this from end to end — see `Command::CheckForUpdates` —
/// because check, download and install are the same intent a step apart. The
/// state is what makes that legible: every press either advances it or says
/// what it is already doing.
///
/// Nothing here is on the board and none of it is saved. An app that had been
/// left open across a release does not owe anybody a resumed download.
#[derive(Debug, Default)]
enum Updating {
    /// Nothing has been asked for.
    #[default]
    Idle,
    /// Asking.
    Looking,
    /// A newer version exists and this install may replace itself with it.
    Offered {
        version: update::version::Version,
        artifact: update::manifest::Artifact,
        target: PathBuf,
    },
    /// Downloading it. `done` and `total` are bytes.
    Fetching { version: update::version::Version, done: u64, total: u64 },
    /// Downloaded, hashed, unpacked, and sitting beside the app.
    Staged(update::install::Staged),
}

/// What the title bar should say about the update, if anything.
///
/// A projection of [`Updating`] rather than the thing itself, so the bar can
/// read the state without the state machine leaking out of this module: the
/// bar needs three sentences and a fraction, not artifacts and paths. `None`
/// covers both `Idle` and `Looking` — a check that is merely running is not
/// worth a badge, because most checks end in nothing and a flicker of chrome
/// on every launch would teach everybody to ignore the spot.
#[derive(Debug, Clone, PartialEq)]
pub enum UpdateBadge {
    /// A newer version exists; clicking downloads it.
    Available { version: String },
    /// It is on its way down. `fraction` is `0.0..=1.0` of the bytes.
    Downloading { fraction: f32 },
    /// Unpacked and waiting; clicking saves the board and restarts into it.
    Ready { version: String },
}

/// A drop that is still arriving.
///
/// **The reason this exists is that a folder is not one file.** Reading, hashing
/// and measuring three hundred photographs is seconds of work, and doing it
/// between two frames is a window that stops answering — which reads as a crash
/// rather than as work. So the reading happens off the drawing thread and the
/// cards land in batches as they come, which turns the same wait into something
/// you can watch and, if it was a mistake, stop.
///
/// One of these covers *every* drop in flight rather than one each, and that is
/// forced: there is one shadow behind the mutation door and therefore one open
/// gesture — see [`mbrd_core::state::BoardState::start`]. Two drops overlapping
/// share the step and close it when the last of them lands.
struct Importing {
    /// The step every drop in flight closes into, held open across the read.
    ///
    /// One step for the whole drop, for the reason [`BoardView::place`] gives:
    /// dropping a folder is one thing somebody did, and taking it back should
    /// be one press rather than forty.
    open: Pending,
    /// Which round of drops this is, so a task whose drop was called off can
    /// tell. See [`BoardView::stop_importing`].
    token: u64,
    /// How many drops are still arriving. The step closes at nought.
    drops: usize,
    /// How many files the drops turned out to point at, once each walk is in.
    found: usize,
    /// How many cards have landed.
    done: usize,
    /// Files that were found and could not be opened at all — permissions, a
    /// broken symlink, a device that went away mid-walk.
    ///
    /// Kept apart from [`Self::heavy`] because the two are different things
    /// with different sentences: a file this app never got to look at did not
    /// arrive, full stop, and saying so is not the same claim as [`Self::heavy`]
    /// makes about a file that is sitting on the board right now.
    unreadable: Vec<String>,
    /// Files that landed anyway but are large enough to be worth a word about
    /// what sending the board on will cost.
    ///
    /// Not a refusal — see the module note at the top of `import.rs`: a file
    /// too large to be reasonable is *reported*, never silently refused. The
    /// card lands exactly like any other; this is only what gets said about it.
    heavy: Vec<String>,
    /// What the last card taken was, for the line at the end when there was
    /// only ever the one file.
    described: Option<&'static str>,
    /// How many of `done` are already in a step that has been closed.
    ///
    /// Nonzero only where somebody worked over the top of the drop — see
    /// [`BoardView::part_import`].
    parted: usize,
    /// The cards placed so far, which are the selection while they are ours.
    placed: Vec<String>,
    /// Whether the selection is still the one this drop has been writing.
    ///
    /// A drop selects what it brings, so it can be moved as a block the moment
    /// it lands. But a big one takes seconds, and somebody who has gone back to
    /// work in the meantime should not have their selection taken off them by a
    /// batch arriving. So the drop stops touching it the moment it finds the
    /// selection is not the one it left.
    ours: bool,
}

/// What the reader sends back as it goes.
///
/// A channel rather than a shared counter, for the reason the download uses one:
/// the view is only ever written from the thread that draws it.
enum Arriving {
    /// How many files this drop turned out to point at. Always first, and it is
    /// what the layout needs before anything can be given a place to land.
    Found(usize),
    /// A file, understood well enough to be a card.
    ///
    /// Boxed because everything else here is a word wide and this is not, and an
    /// enum is as large as its largest arm however rarely that arm is used.
    Ready(Box<import::Ready>),
    /// A file that was found and could not be opened at all.
    Unreadable(String),
    /// A word about a file large enough to be worth one, sent *alongside* the
    /// [`Self::Ready`] that puts it on the board — not instead of it. See the
    /// module note at the top of `import.rs`: too large is a thing to report,
    /// not a limit to enforce.
    Heavy(String),
}

/// A board on its way in from the disk.
///
/// **The board that is open stays open, and stays usable, while this runs.**
/// Opening is the one thing in this app that can take seconds without anybody
/// having asked for seconds — a board of photographs is most of a gigabyte to
/// inflate and to hash — and the version of this that tore the old board down
/// first and read on the drawing thread spent all of it showing a window that
/// had stopped answering. Nothing is given up until there is something to put
/// in its place; see [`BoardView::settle_open`].
struct Opening {
    /// Which open this is. A read that has been overtaken lands nowhere.
    token: u64,
    /// What is being opened, for the line that says so.
    name: String,
    /// Bytes unpacked, and the bytes the archive says it holds.
    ///
    /// A total of nought means the archive declined to say — see
    /// [`mbrd_core::mbrd::read_watched`] — and the loader then shows that it is
    /// working rather than inventing a fraction.
    done: u64,
    total: u64,
}

/// How often the loader takes the newest reading.
///
/// The same thirty-a-second the download and the drop run at, and for the same
/// reason: an entry lands every few hundred microseconds on a board of small
/// files, and repainting per entry would cost more than the read.
const OPENING_EVERY: Duration = Duration::from_millis(33);

/// How wide the opening loader is, and how wide the bar inside it is.
const LOADER_WIDTH: f32 = 300.0;
const LOADER_TRACK: f32 = LOADER_WIDTH - 28.0;

/// How often the cards that have been read are put on the board.
///
/// The same thirty-a-second the download's progress runs at, and for the same
/// reason: a thousand small files read faster than that would otherwise be a
/// thousand repaints, and nobody can see a card land in under a frame anyway.
const ARRIVE_EVERY: Duration = Duration::from_millis(33);

/// How long something that just happened stays on screen.
const SAY_FOR: Duration = Duration::from_secs(4);

/// How long something that went wrong stays on screen.
const WARN_FOR: Duration = Duration::from_secs(10);

/// How long the board has to sit still before it is written to disk.
///
/// A second, and both halves of that matter. Long enough that a burst of
/// changes — a sentence being typed, a drag being made — is one write rather
/// than fifty, because a write deflates every photograph on the board. Short
/// enough that "saved as it happens" is honest: nobody looks away from what
/// they typed, decides they are done and closes the lid inside a second, and
/// the one who does is caught by [`BoardView::flush`] anyway.
const AUTOSAVE_AFTER: Duration = Duration::from_millis(1000);

/// How long a failed save waits before trying again on its own.
///
/// Without a retry, a save that fails because a drive was unplugged, or a
/// network mount hiccuped, sits failed until the next keystroke gives
/// `arm_autosave` a reason to try — which may be minutes away, or may never
/// come if whoever is looking has stepped away from the board entirely. Five
/// seconds is often enough that reconnecting the drive and waiting a moment
/// is indistinguishable from the save having worked the first time, and
/// short enough that it does not read as the app having given up.
const RETRY_AFTER: Duration = Duration::from_secs(5);

/// What is open above the board, if anything — the right-click list, the
/// board switcher, or a palette.
///
/// **One field rather than three.** It used to be three: `menu`, `switcher`
/// and `palette`, each an `Option` closed by its own function. Every one of
/// those functions had to remember to close the *other* two, and one of them
/// did not — `open_switcher` left a palette that was already open standing
/// behind it, visible and unreachable, because nothing forced the two facts
/// "the palette is open" and "the switcher is open" to disagree. An enum
/// makes "at most one of these" a fact about the type rather than a rule
/// somebody has to keep re-checking: there is exactly one field to close, and
/// closing it is the only way to open another.
#[derive(Debug, Clone)]
enum Overlay {
    None,
    Menu(Menu),
    Switcher(Switcher),
    Palette(Palette),
    /// The settings page. The payload is only which section the sidebar has
    /// open — everything the rows show is read off the view each frame. See
    /// `settings.rs`.
    Settings(crate::settings::Page),
    /// One card, opened onto the whole window. The payload is only its id, for
    /// the same reason the settings page carries only its section. See
    /// `opened.rs`.
    Opened(crate::opened::Opened),
}

pub struct BoardView {
    pub doc: Document,
    /// How wide a character is, for every wrap on this board.
    ///
    /// Resolved once, at startup, against [`body_font`]. Held rather than
    /// borrowed because the two callers that need it most — `write_to` on
    /// every keystroke and `refit` inside an `edit` — are already inside a
    /// closure holding the document. See `metrics.rs`.
    measure: Measure,
    pub viewport: Viewport,
    pub theme: Theme,
    /// Selected item ids, in the order they were selected.
    pub selection: Vec<String>,
    /// Selections let go of without the board changing.
    ///
    /// What Ctrl Z takes back before it starts on the ledger. See [`LetGo`].
    let_go: LetGo,
    /// What the status bar is saying, and how long it has left. See [`Said`].
    said: Option<Said>,
    /// What this person has asked for. See `prefs.rs`.
    pub prefs: Prefs,
    /// Every palette this run can offer. See `themes.rs`.
    ///
    /// Read once, at startup. Not re-read on every frame and not watched: a
    /// theme file is a document somebody edits in bursts and then stops, and
    /// the settings page has a row that reloads this on request — which costs
    /// one press on the rare occasion it is wanted, against a filesystem watch
    /// running for the whole life of every session on the chance it is.
    pub themes: crate::themes::Registry,
    /// What the desktop last said it looks like.
    ///
    /// Only consulted when the mode is `System` — see [`Prefs::mode`] — but
    /// tracked always, so that switching *to* `System` is instant rather than
    /// waiting for the desktop to next change its mind.
    pub system: crate::themes::Appearance,
    /// A palette being tried on, which is not yet a choice.
    ///
    /// The theme picker previews live as the highlight moves, and this is what
    /// makes that reversible: [`Self::theme`] is whatever is on screen, and
    /// this remembers what was on screen before the preview started so that
    /// Escape can put it back. `None` on every frame that is not a preview,
    /// which is nearly all of them.
    theme_before_preview: Option<Theme>,
    /// How far in the marks beside each card have faded, by card id.
    ///
    /// A number per card rather than one for the whole board, because hover
    /// and selection both offer marks and the two overlap: pointing at one
    /// card while another is selected has to fade one in without touching the
    /// other. Entries are dropped as they reach zero, so this is empty on the
    /// ordinary board and never grows.
    anchor_fade: HashMap<String, f32>,
    /// The status bar's counts, and the revision they were counted at.
    /// See [`tally`](Self::tally).
    tallied: (u64, usize, usize),
    /// The board being read off the disk, where one is. See [`Opening`].
    ///
    /// Kept a beat past the read finishing (or being called off), so the
    /// panel can fade out instead of vanishing the instant the last byte
    /// lands — see `opening_leaving`, the loader's own version of
    /// `overlay_leaving`.
    opening: Option<Opening>,
    /// How far the loading panel has faded in. The same shape as
    /// `overlay_presence`, for a panel that is not part of `Overlay` because
    /// it takes no input at all rather than owning the keyboard.
    opening_presence: f32,
    /// Whether the loading panel is on its way out. See `opening`.
    opening_leaving: bool,
    /// Which open is the live one.
    ///
    /// Bumped per open, so that asking for a second board while the first is
    /// still being read means the first one lands nowhere rather than landing
    /// after it and winning.
    opens: u64,
    /// The drop or drops still arriving, where any are. See [`Importing`].
    importing: Option<Importing>,
    /// Which round of drops is the live one.
    ///
    /// Bumped when a drop is called off, which is how the tasks still reading
    /// for it find out: there is no way to reach into a spawned read and stop
    /// it, but there is a way to make what it sends land nowhere.
    imports: u64,
    /// How far along the update is. See [`Updating`].
    updating: Updating,
    /// Which revision of the board was last written to disk.
    ///
    /// A comparison against `revision()` rather than a flag set on every
    /// mutation, because the ledger already counts and a second counter is a
    /// second thing to keep in step. What [`Self::unsaved`] answers, and so
    /// what decides whether the autosave timer has anything to do.
    saved_at: u64,
    /// Whether a write is in flight on the background executor.
    ///
    /// One at a time. Two overlapping writes to one path would race on the
    /// rename `save::write` finishes with, and the second would be the one that
    /// won regardless of which board was newer.
    saving: bool,
    /// The revision a write failed on, so the autosave timer does not spend
    /// the rest of the session retrying a broken write once a second and
    /// warning about it every time.
    ///
    /// Not the same as "nobody is trying" — see [`Self::arm_retry`], which
    /// keeps trying on its own, slower clock for as long as this stands.
    /// Cleared by that retry succeeding, or by the next change to the board,
    /// which is the other thing that could make the write worth attempting
    /// again on the ordinary schedule — a full disk that somebody clears does
    /// not notify us, but the next edit does.
    ///
    /// Also what puts the dot next to the board's name in the titlebar — see
    /// [`Self::save_failing`] — since this is the one field that is `Some`
    /// for exactly as long as that dot has anything to mean.
    failed_at: Option<u64>,
    /// Whether something is already waiting to write the board out.
    ///
    /// The same one-flag arrangement as `said_timer` and for the same reason:
    /// four edits in a row must not arm four timers.
    save_timer: bool,
    /// Whether a close was already turned back once because the final write
    /// failed.
    ///
    /// The first refusal is the warning; a second attempt with the flag still
    /// set is somebody who read it and chose to leave anyway, and blocking the
    /// window a second time would just be trapping them behind a message they
    /// already have. Cleared the moment a flush lands.
    close_refused: bool,
    /// Whether something is already waiting to take the line down.
    ///
    /// A timer rather than a frame a sixtieth of a second, because holding a
    /// line of text on screen is not animation: asking for frames until it
    /// expires would repaint the whole board four seconds after every action,
    /// which on a large one is the most expensive thing in the app happening
    /// for no reason at all. One flag, so that four messages in a row do not
    /// arm four timers.
    said_timer: bool,
    /// Where the camera is going, and how it gets there. See `camera.rs`.
    ///
    /// **A resting camera agrees with the viewport**, which is what lets the
    /// gesture below go on writing `viewport.pan` directly: the next frame
    /// absorbs it. Anything that moves the camera *without* a hand on it goes
    /// through here instead.
    camera: Camera,
    /// How fast the board has been dragged lately, so that letting go of it
    /// can carry on at the speed it was going.
    ///
    /// Beside the gesture rather than inside it because a gesture is cloned on
    /// the way through the pipeline, and a history of pointer samples is not
    /// something to copy once a frame.
    pan_trail: Trail,
    gesture: Gesture,
    /// Where the canvas actually is in the window, measured during prepaint.
    ///
    /// Mouse events arrive in *window* coordinates, so without this every
    /// pointer position would be off by the width of the sidebar. Measuring it
    /// rather than assuming it means the layout can change without silently
    /// putting the cursor in the wrong place.
    canvas_bounds: Bounds<Pixels>,
    /// Where the open window's editor draws its text, and how wide one of its
    /// characters is.
    ///
    /// The same trade `canvas_bounds` makes and for the same reason: turning a
    /// press into a caret needs to know where the text starts, and only the
    /// layout knows that. Recorded by the page as it draws — see
    /// `opened::source` — and read by [`Self::place_opened_caret`].
    opened_text: Bounds<Pixels>,
    opened_advance: f32,
    focus_handle: FocusHandle,
    /// The file this board came from, where it came from one. `None` means a
    /// save has to invent a name — see `save::default_path`.
    pub path: Option<PathBuf>,
    /// Where everything is, so that culling and hit-testing do not walk the
    /// whole board. Reached only through [`BoardView::index`], which is what
    /// keeps it from being read while it is out of date.
    grid: Grid,
    /// The board revision `grid` was built from.
    grid_at: u64,
    /// Who holds what, so that the group rule does not re-measure the board
    /// for every question asked of it.
    ///
    /// Reached only through [`BoardView::fences`], for the reason `grid` is
    /// reached only through [`BoardView::index`]: a stale measurement does not
    /// answer a little bit wrong, it answers about a grouping that no longer
    /// exists, which is a press selecting something nobody pointed at.
    ///
    /// Worth caching because the pointer asks once a frame — `cursor_at` has
    /// to know whether the card under it is in a group before a press is made
    /// — and a board with no fences at all still costs a pass over every item
    /// to find that out.
    fences: Fences,
    /// The board revision `fences` was measured from.
    fences_at: u64,
    /// Decoded pictures, keyed by content hash.
    images: Images,
    /// Parsed meshes and their last-rasterised pictures. See `mesh_cache`.
    meshes: crate::mesh_cache::Meshes,
    /// The one mesh card whose drag and scroll orbit its camera instead of
    /// moving it or zooming the board. See `Command::Position`.
    pub positioning: Option<String>,
    /// The card being typed into, where there is one.
    ///
    /// The app's second mode, after the switcher. While this holds a value the
    /// keyboard belongs to the text rather than to the board — see
    /// `on_key_down`, and `editor.rs` for what a text field owns and what it
    /// hands back.
    pub editing: Option<Editing>,
    /// Cards that were copied, waiting to be pasted.
    ///
    /// The app's own clipboard rather than the platform's, because a card is
    /// not text: it has a size, a type, a rotation and possibly several
    /// megabytes of photograph, and the only faithful way to put that on the
    /// system clipboard would be to invent a MIME type nothing else reads.
    /// Copying a card and pasting it into another program still gets its name,
    /// via the system clipboard — see `copy_selection`.
    clipboard: Vec<Item>,
    /// The menu, the switcher or a palette, where one is open. See
    /// [`Overlay`].
    ///
    /// While this holds anything but `Overlay::None` the switcher and the
    /// palette variants take every key press, which is what makes them a
    /// mode rather than a panel — see `on_key_down`, which routes here before
    /// anything else. The menu variant is not a mode in that sense — it
    /// leaves the arrows and Enter to `Entry` navigation rather than the
    /// board — but it still owns Escape.
    overlay: Overlay,
    /// How far the overlay has faded in, from `0.0` (not visible) to `1.0`
    /// (settled). See `advance_overlay`.
    ///
    /// One spring for whichever surface is open rather than one per surface,
    /// because at most one is ever animating — [`Overlay`] again — and a
    /// spring per surface would be two of them permanently at rest doing
    /// nothing. Not reset when the overlay changes: opening the switcher
    /// while the palette is still fading in retargets this rather than
    /// restarting it, which is what keeps the handoff from jumping.
    ///
    /// A [`Sprung`] rather than the plain ramp it used to be, because these
    /// surfaces slide as well as fade and a slide can be caught mid-flight:
    /// Escape during the arrival keeps the value *and the velocity*, so the
    /// panel bends back out along the path it came in on instead of
    /// reversing with a kink. Renders read [`Sprung::value`], never the
    /// target — the value is where the surface *is*.
    pub overlay_presence: Sprung,
    /// Whether the overlay is on its way out.
    ///
    /// Closing does not clear `overlay` — it sets this instead, and
    /// `advance_overlay` is what actually drops it once `overlay_presence`
    /// has faded to nothing. Everything that reads the keyboard checks this
    /// first: a surface that is leaving is still drawn, but it is dead to
    /// input, so a second Escape pressed mid-fade does not double-close it
    /// and a key meant for the board underneath does not vanish into a panel
    /// that is on its way out anyway.
    overlay_leaving: bool,
    /// The little controls on the settings page — a switch's knob, a
    /// segmented row's lit choice — each a spring from the state it showed
    /// to the state it now has, keyed by the control's own id.
    ///
    /// Planted by the press, not by the render: `control_at` answers the
    /// resting state until a press has given the control somewhere to go,
    /// which is what keeps a page opened onto a switch that is already on
    /// from showing it *arriving* on. Bounded by the number of controls the
    /// page has, so it is never cleared.
    settings_motion: HashMap<String, Sprung>,
    /// Whether the previous frame asked for this one.
    ///
    /// What tells [`advance`](Self::advance) the difference between a slow
    /// frame and a fresh start. The window redraws for plenty of reasons that
    /// are not motion — a hover, a key press, a status line — and while the
    /// board is still, no frames are requested at all, so the gap since the
    /// last one is *idle* time. Charging that gap to the first thing that
    /// starts moving lands it at its target inside one invisible frame,
    /// which is how every animation that began from a quiet board managed to
    /// be one nobody had ever actually seen.
    animating: bool,
    /// Which board switcher session is current.
    ///
    /// Bumped every time one opens, so that the background scan for boards
    /// *beside* the open one — see `Switcher::open` and `open_switcher` —
    /// lands nowhere if the switcher has since been closed and opened again,
    /// the same shape `opens` and `imports` use for a read or a drop that has
    /// been overtaken.
    switches: u64,
    /// Watches for a modifier tapped twice. See `taps.rs`.
    ///
    /// Not part of the key table, because a bare Shift is not a key press —
    /// the platform reports it as a change in what is held down.
    taps: Taps,
    /// What a press on the board means. See `tools.rs`.
    pub tool: Tool,
    /// The selected connection, named by the two cards it joins.
    ///
    /// Deliberately *not* part of `selection`: a rope and a card are not the
    /// same kind of thing to have selected, and the commands that apply to one
    /// apply to neither of the other's. Selecting a rope clears the cards and
    /// selecting a card clears the rope, so at most one of the two is ever
    /// live — which is what lets the context menu be about whichever it is.
    pub rope: Option<(String, String)>,
    /// Where the pointer was last seen, in window coordinates.
    ///
    /// For the chip drawn beside it during a gesture — see [`BoardView::badge`].
    /// The painter has the window's own pointer position, but only as of the
    /// frame it runs in, and a readout that is a frame ahead of the cards it is
    /// describing reads as lag in the cards.
    pointer: gpui::Point<Pixels>,
    /// The fences that have been stepped into, outermost first.
    ///
    /// **This is what makes a fence a group rather than a rectangle.** A press
    /// on a card normally selects the outermost fence holding it, so that a
    /// grouping made on purpose behaves like one thing; entering a fence takes
    /// it off that list for as long as you are inside, so the press reaches
    /// what is in it. See [`BoardView::selects`], which is the one place that
    /// rule is written down.
    ///
    /// A stack rather than one id, because fences nest: entering the outer one
    /// and then the inner one is two steps in and two presses of `Escape` back
    /// out, which is what nesting has to mean if it means anything.
    ///
    /// Session state, deliberately. Where somebody had browsed to is not a fact
    /// about the board and has no business in the file — or in the ledger,
    /// where it would make stepping into a group something to undo.
    inside: Vec<String>,
    /// The card the pointer is over, so its anchors can be offered.
    ///
    /// Offered on hover rather than on selection, because starting a rope
    /// should not cost a click first: pointing at a card is already saying
    /// which card you mean.
    pub hovering: Option<String>,
    /// Cached routes, so that a board's lines are worked out when it settles
    /// rather than on every frame of every drag. See `wires.rs`.
    wires: Wires,
    /// The lines exactly as they were last drawn.
    ///
    /// Kept so that pressing a rope can be measured against where it actually
    /// runs. A rope that bends round a card has to be pressable where it *is*,
    /// and the only thing that knows that is the frame that drew it.
    drawn: Vec<Wire>,
    /// Where every playhead is. See `playback.rs`.
    ///
    /// Session state rather than board state, and deliberately: nobody has ever
    /// wanted to undo a playhead, and a `.mbrd` sent to somebody else should not
    /// arrive halfway through a video.
    media: Media,
    /// The measured clock of every animation recently on screen. See
    /// [`Timings`].
    timings: Timings,
    /// How many decodes are out right now. See [`DECODES_AT_ONCE`].
    decoding: usize,
    /// The newest frame of anything that is moving. See `live.rs`.
    live: Live,
    /// Whether an asset carries a sound track, once per asset.
    ///
    /// `import.rs` writes this onto the card, so a file dropped on this build
    /// answers from `meta` and never reaches here. A board saved before that
    /// existed has videos with nothing written, and the answer is still in the
    /// bytes — so it is read once, off the asset, and kept for the session.
    ///
    /// Deliberately *not* written back to the board. It is a measurement, and
    /// re-deriving a measurement is not a reason to make somebody's file dirty
    /// the moment they open it.
    sound: HashMap<String, Option<bool>>,
    /// The card whose volume slider is showing, where one is.
    ///
    /// One at a time, and by id rather than a flag on the card, because two
    /// sliders open at once would be two places a drag could mean.
    volume_on: Option<String>,
    /// The controls exactly as they were last drawn.
    ///
    /// Kept for the reason `drawn` is kept for the lines: a control has to be
    /// pressable where it *is*, and the only thing that knows where that is is
    /// the frame that drew it. Laying them out a second time in the hit test
    /// would be two copies of the same arithmetic, agreeing right up until one
    /// of them is changed.
    ///
    /// Back to front, like the paint order, so the last match wins.
    drawn_controls: Vec<Drawn>,
    /// The play/pause, mute or loop button currently held down, if any.
    ///
    /// Set at the press and cleared at the release — see `on_mouse_down` and
    /// `on_mouse_up` — because unlike a scrub or a volume drag, pressing one
    /// of these three fires immediately and leaves `self.gesture` at `None`
    /// for as long as the button stays down, so there is nothing on the
    /// gesture itself for the painter to read the way it reads `Scrubbing`
    /// or `Louder`. Only these three: the scrubber and the slider already
    /// give positional feedback and do not need a wash as well.
    pressed_control: Option<(String, transport::Hit)>,
    /// Where an arranged card's *drawn* position still has to catch up from,
    /// keyed by card id — see `Self::present_move`.
    ///
    /// Align, distribute, separate and the grid snap all write a card's new
    /// position in one frame, the same as any other edit. Left there, a row
    /// lining itself up would be a jump cut, not a tidy. This is what turns
    /// it into a catch: the model moves at once, same as it always has, and
    /// each card's presentation eases back onto it afterwards, one axis at a
    /// time so a card already easing from an earlier arrange bends into the
    /// next one instead of restarting.
    ///
    /// A map rather than a field on every `Item`, because only a handful of
    /// cards are ever mid-catch at once — everything else on the board would
    /// be carrying two idle springs for no reason. Entries are dropped once
    /// both springs settle, so a board nobody has arranged in an hour costs
    /// nothing here.
    presenting: HashMap<String, (Sprung, Sprung)>,
}

/// One card's controls, as they were painted, and what a press needs to know.
struct Drawn {
    id: String,
    strip: transport::Strip,
    /// The slider, only when it is showing.
    volume: Option<transport::Box2>,
    /// How long the thing on this card is, so a press on the scrubber knows
    /// what a fraction of it means.
    length: Option<Duration>,
    looping: bool,
    /// Whether this card's sound should stop the others when it starts.
    sound: bool,
    /// Whether there is anything behind the playhead yet.
    ///
    /// An animation moves the moment it is decoded. A video does not move at
    /// all until there is a decoder to move it, and starting a playhead over a
    /// still poster would be a scrubber that advances across a picture that
    /// does not — and, worse, a board that repaints sixty times a second
    /// forever with nothing to show for it.
    moves: bool,
}

impl BoardView {
    pub fn new(doc: Document, path: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let grid = Grid::build(&doc.board.items);
        let grid_at = doc.board.revision();
        let fences = Fences::measure(&doc.board.items);
        let saved_at = doc.board.revision();
        let mut view = Self {
            // Resolved here rather than lazily on the first frame: the face is
            // fixed for the life of the window — see [`body_font`] — so there
            // is nothing to wait for, and a board that measured its first
            // frame with a guess would lay that frame out differently from
            // every one after it.
            measure: Measure::new(cx.text_system().clone(), &body_font()),
            grid,
            grid_at,
            fences,
            fences_at: grid_at,
            images: Images::default(),
            meshes: crate::mesh_cache::Meshes::default(),
            positioning: None,
            overlay: Overlay::None,
            overlay_presence: Sprung::at(0.0),
            settings_motion: HashMap::new(),
            animating: false,
            overlay_leaving: false,
            switches: 0,
            taps: Taps::default(),
            editing: None,
            tool: Tool::default(),
            rope: None,
            pointer: gpui::Point::default(),
            inside: Vec::new(),
            hovering: None,
            wires: Wires::default(),
            drawn: Vec::new(),
            media: Media::default(),
            timings: Timings::default(),
            decoding: 0,
            live: Live::default(),
            sound: HashMap::new(),
            volume_on: None,
            drawn_controls: Vec::new(),
            pressed_control: None,
            presenting: HashMap::new(),
            clipboard: Vec::new(),
            doc,
            path,
            viewport: Viewport::default(),
            theme: Theme::default(),
            selection: Vec::new(),
            let_go: LetGo::default(),
            said: None,
            opening: None,
            opening_presence: 0.0,
            opening_leaving: false,
            opens: 0,
            importing: None,
            imports: 0,
            updating: Updating::default(),
            // A board just read off disk is a board that agrees with disk. A
            // new one has never been saved and is dirty from its first edit,
            // which `revision()` already reflects.
            saved_at,
            saving: false,
            failed_at: None,
            save_timer: false,
            close_refused: false,
            prefs: crate::prefs::load(),
            themes: crate::themes::Registry::load(),
            // Replaced by the real answer as soon as there is a window to ask
            // — see `main.rs`, which both seeds this and observes it. Dark
            // rather than light because that is what this app has always been,
            // and a frame or two of the wrong palette on launch should be the
            // one somebody already had.
            system: crate::themes::Appearance::Dark,
            theme_before_preview: None,
            anchor_fade: HashMap::new(),
            // The sentinel no revision can be, so the first frame counts.
            tallied: (u64::MAX, 0, 0),
            said_timer: false,
            // Rebuilt below, once the saved view has been read: a camera made
            // against the default viewport and then not told about the board's
            // own view would spring from the origin on the first thing that
            // moved it.
            camera: Camera::new(&Viewport::default()),
            pan_trail: Trail::default(),
            gesture: Gesture::None,
            canvas_bounds: Bounds::default(),
            opened_text: Bounds::default(),
            opened_advance: 1.0,
            focus_handle: cx.focus_handle(),
        };
        view.restore_saved_view();
        // Before the first paint, so that a chosen theme is what the window
        // opens wearing rather than something it changes into.
        view.theme = view.chosen_theme();
        view.look_on_launch(cx);
        view
    }

    /// Ask about updates a few seconds after the window is up.
    ///
    /// Delayed rather than immediate, and the delay is the point: the first
    /// second of a launch is the one the app is judged on — the window
    /// appearing, the board drawing, the first images decoding — and starting
    /// a TLS handshake in the middle of it competes for exactly the threads
    /// that work is on.
    ///
    /// `update::due` is what decides whether anything actually happens, and
    /// on most launches the answer is no: not more than once a day, never on
    /// the first run, and never at all if it has been turned off or this build
    /// has no key. See `update/mod.rs`.
    fn look_on_launch(&mut self, cx: &mut Context<Self>) {
        if !update::due(self.prefs.update, false) {
            return;
        }
        cx.spawn(async move |view, cx| {
            cx.background_executor().timer(Duration::from_secs(5)).await;
            view.update(cx, |view, cx| view.look_for_update(false, cx)).ok();
        })
        .detach();
    }

    /// Put the camera back where the file left it.
    ///
    /// A board travels with its own view, so opening one puts you back where
    /// you were. The size is not known yet at this point — it is measured on
    /// the first prepaint — and that is fine: `pan` and `zoom` do not depend
    /// on it.
    fn restore_saved_view(&mut self) {
        let saved = self.doc.board.view;
        self.viewport.pan = point(saved.pan_x, saved.pan_y);
        self.viewport.zoom = saved.zoom;
        // Arrived at, not travelled to. Opening a board is not a move across a
        // space somebody is already looking at, so there is no relationship
        // between the old view and the new one worth animating — and animating
        // it anyway would mean every board opening with a lurch.
        self.camera.park(&self.viewport);
    }

    /// Record the camera into the board, so that a save writes where you are
    /// looking rather than where the file was opened.
    pub fn capture_view(&mut self) {
        // Where the camera is *going*, not where this frame caught it. A board
        // saved during a flick would otherwise reopen halfway through somebody
        // else's gesture — and the live zoom may be out on the rubber band at
        // the end of the range, which is not a value the format allows.
        //
        // A still camera is asked the viewport instead rather than trusted to
        // agree with it. It does agree — that is the invariant `Camera::step`
        // keeps — but only as of the last frame, and a save is not a frame.
        // Reading the thing that was actually just written by a drag removes
        // the question entirely.
        let (pan, zoom) = if self.camera.moving() {
            self.camera.resting()
        } else {
            (self.viewport.pan, self.viewport.zoom)
        };
        self.doc.board.set_view(View { pan_x: pan.x, pan_y: pan.y, zoom });
    }

    // -----------------------------------------------------------------------
    // What the status bar is saying
    // -----------------------------------------------------------------------

    /// Report something that just happened. Gone in a few seconds.
    fn say(&mut self, text: String) {
        // Narration does not push anything off the bar. A save that failed is
        // up for ten seconds, and moving a card in the meantime must not clear
        // the one line that says the disk is not taking the board — least of
        // all with a line nobody will see, since `Tone::Done` is not drawn.
        if self.said.as_ref().is_some_and(|said| said.tone.shown()) {
            return;
        }
        self.said = Some(Said { text, until: Some(Instant::now() + SAY_FOR), tone: Tone::Done });
    }

    /// Report something the board itself cannot show.
    ///
    /// A download's progress, or a key press that turned out to have nothing to
    /// do. See [`Tone`] for the division between this and [`Self::say`], which
    /// is the whole reason the bar is quiet now.
    fn tell(&mut self, text: String) {
        self.said = Some(Said { text, until: Some(Instant::now() + SAY_FOR), tone: Tone::Told });
    }

    /// Report something that went wrong. Up for longer.
    fn warn(&mut self, text: String) {
        self.said = Some(Said { text, until: Some(Instant::now() + WARN_FOR), tone: Tone::Wrong });
    }

    /// Say something that stays true until it is replaced — a mode, not an
    /// event. `None` puts the bar back to saying nothing.
    fn hint(&mut self, text: Option<String>) {
        self.said = text.map(|text| Said { text, until: None, tone: Tone::Mode });
    }

    /// Stop saying anything.
    fn hush(&mut self) {
        self.said = None;
    }

    // -----------------------------------------------------------------------
    // Updating
    // -----------------------------------------------------------------------

    /// Whether the board has changes that are not on disk.
    ///
    /// True for at most a second at a time on an ordinary board — see
    /// [`AUTOSAVE_AFTER`] — which is why nothing draws this. It is asked by the
    /// timer that fixes it, and by the updater, which will not restart over
    /// work that is still only in memory.
    pub fn unsaved(&self) -> bool {
        self.doc.board.revision() != self.saved_at
    }

    /// Whether a save is failing right now, for the dot the titlebar draws
    /// beside the board's name — see `titlebar::switcher_button`.
    ///
    /// This is the answer to the argument that removed the old unsaved-work
    /// dot: that one was on the board's name too, but stood for `unsaved()`,
    /// which is true for well under a second at a time — so it "spent its
    /// life either absent or a second from being absent," and an indicator
    /// nobody has time to read is not an indicator. `failed_at` does not have
    /// that problem. It is `Some` for exactly as long as the disk is refusing
    /// the board — which can be seconds or can be the rest of the session —
    /// so a dot keyed to it is on only while it means something, and it means
    /// something the whole time it is on.
    pub fn save_failing(&self) -> bool {
        self.failed_at.is_some()
    }

    /// `Ctrl U`. What it does depends on how far the last press got.
    ///
    /// Check → download → install → restart, one press per step. Deliberately
    /// four presses rather than one: each step is slower and less reversible
    /// than the one before it, and the last of them closes the window. None of
    /// that should happen because somebody pressed a key once to see what it
    /// did.
    /// What the title bar should show about the update. See [`UpdateBadge`].
    pub fn update_badge(&self) -> Option<UpdateBadge> {
        match &self.updating {
            Updating::Idle | Updating::Looking => None,
            Updating::Offered { version, .. } => {
                Some(UpdateBadge::Available { version: version.to_string() })
            }
            Updating::Fetching { done, total, .. } => Some(UpdateBadge::Downloading {
                fraction: (*done as f32 / (*total).max(1) as f32).clamp(0.0, 1.0),
            }),
            Updating::Staged(staged) => {
                Some(UpdateBadge::Ready { version: staged.version.to_string() })
            }
        }
    }

    pub fn update_step(&mut self, cx: &mut Context<Self>) {
        match std::mem::take(&mut self.updating) {
            Updating::Idle => self.look_for_update(true, cx),

            // Mid-flight. Put it back and describe it, rather than starting a
            // second of whatever is already running.
            Updating::Looking => {
                self.updating = Updating::Looking;
                self.tell("looking for updates…".into());
            }
            Updating::Fetching { version, done, total } => {
                self.updating = Updating::Fetching { version, done, total };
                self.tell(format!("downloading {version} — {}", portion(done, total)));
            }

            Updating::Offered { version, artifact, target } => {
                self.fetch_update(version, artifact, target, cx)
            }
            Updating::Staged(staged) => self.apply_update(staged, cx),
        }
        cx.notify();
    }

    /// Ask whether there is anything newer.
    ///
    /// `by_hand` is what separates the launch check from `Ctrl U`: the launch
    /// one is silent about everything except good news, because nobody asked
    /// it a question, and a failed network request is not worth a line on
    /// somebody's screen. A press is a question and gets an answer either way.
    pub fn look_for_update(&mut self, by_hand: bool, cx: &mut Context<Self>) {
        if !update::due(self.prefs.update, by_hand) {
            if by_hand {
                // The three reasons, distinguished, because "nothing happened"
                // is the least useful thing a key can do.
                self.tell(if !update::possible() {
                    "this build cannot check for updates".into()
                } else {
                    "checking for updates is turned off".into()
                });
            }
            return;
        }

        self.updating = Updating::Looking;
        if by_hand {
            self.tell("looking for updates…".into());
        }

        // On the background executor, like the image decode: `look` is two
        // blocking HTTPS requests and a signature check, and none of it may
        // happen on the thread that draws.
        let looking = cx.background_executor().spawn(async move { update::look() });
        cx.spawn(async move |view, cx| {
            let found = looking.await;
            view.update(cx, |view, cx| {
                view.settle_look(found, by_hand);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// What the answer meant.
    fn settle_look(&mut self, found: anyhow::Result<update::Found>, by_hand: bool) {
        self.updating = Updating::Idle;
        match found {
            Ok(update::Found::Nothing) => {
                if by_hand {
                    self.tell(format!(
                        "mbrd {} is the newest version",
                        update::version::Version::current()
                    ));
                }
            }
            // Worth saying whether or not anybody asked — this is the good
            // news the check exists for.
            Ok(update::Found::Ready { version, artifact, target }) => {
                // The badge in the top bar is the durable half of this
                // announcement — see `update_badge` — so the line here only
                // has to break the news, not carry the instructions.
                self.tell(format!("mbrd {version} is out — see the top bar"));
                self.updating = Updating::Offered { version, artifact, target };
            }
            // Also worth saying unasked, and it is the end of the road: this
            // install cannot replace itself, so the sentence has to carry the
            // next step with it. See `update/eligible.rs`.
            Ok(update::Found::Tell { version, why }) => {
                self.tell(format!("mbrd {version} is out — {why}"));
            }
            // Only when asked. A check that ran on its own and failed is a
            // laptop on a train, and reporting it would make the app look
            // broken for something nobody was waiting on.
            Err(err) => {
                if by_hand {
                    self.warn(format!("could not check for updates: {err:#}"));
                }
            }
        }
    }

    /// Download it, hash it, and unpack it beside the app.
    fn fetch_update(
        &mut self,
        version: update::version::Version,
        artifact: update::manifest::Artifact,
        target: PathBuf,
        cx: &mut Context<Self>,
    ) {
        let total = artifact.size;
        self.updating = Updating::Fetching { version, done: 0, total };
        self.tell(format!("downloading mbrd {version}…"));

        // Progress arrives on the background thread and has to cross back to
        // the one that draws. A channel rather than a shared counter, so the
        // view is only ever written from its own thread.
        let (progress, updates) = std::sync::mpsc::channel::<u64>();
        let staging = cx.background_executor().spawn(async move {
            update::stage(&artifact, version, &target, |done| {
                let _ = progress.send(done);
            })
        });

        cx.spawn(async move |view, cx| {
            use std::sync::mpsc::TryRecvError;

            // Drain to the newest value about thirty times a second, rather
            // than notifying per 64 KiB block — which on a fast connection is
            // hundreds of repaints a second for a number four characters wide.
            //
            // The loop ends when the channel *disconnects*, which is the
            // download finishing: the sender lives in the closure handed to
            // `stage`, so it is dropped exactly when that call returns. No
            // second completion flag to keep in step with the first.
            loop {
                let mut latest = None;
                let disconnected = loop {
                    match updates.try_recv() {
                        Ok(done) => latest = Some(done),
                        Err(TryRecvError::Empty) => break false,
                        Err(TryRecvError::Disconnected) => break true,
                    }
                };

                let mut watching = true;
                if let Some(done) = latest {
                    view.update(cx, |view, cx| {
                        // Anything else in `updating` means something replaced
                        // this download — a window closing, or a second press.
                        // Stop drawing progress for work nobody is waiting on.
                        if let Updating::Fetching { done: at, .. } = &mut view.updating {
                            *at = done;
                            cx.notify();
                        } else {
                            watching = false;
                        }
                    })
                    .ok();
                }

                if disconnected || !watching {
                    break;
                }
                cx.background_executor().timer(Duration::from_millis(33)).await;
            }

            let staged = staging.await;
            view.update(cx, |view, cx| {
                view.settle_fetch(staged);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn settle_fetch(&mut self, staged: anyhow::Result<update::install::Staged>) {
        match staged {
            Ok(staged) => {
                let version = staged.version;
                self.updating = Updating::Staged(staged);
                self.tell(format!("mbrd {version} is ready — restart from the top bar to install"));
            }
            Err(err) => {
                self.updating = Updating::Idle;
                self.warn(format!("could not download the update: {err:#}"));
            }
        }
    }

    /// Move it into place and restart into it.
    fn apply_update(&mut self, staged: update::install::Staged, cx: &mut Context<Self>) {
        // The board goes to disk before the restart discards what is in
        // memory — the same write the close button does, for the same reason.
        // It used to *refuse* here and send somebody off to save first, which
        // was one refusal more than the moment needs: pressing "restart to
        // update" already says yes to a restart, and the save is this app's
        // job, not homework. The refusal survives only where the write
        // fails — the staged copy is kept, so fixing the disk and pressing
        // again costs nothing.
        if !self.flush(cx) {
            self.updating = Updating::Staged(staged);
            self.warn("could not save the board — the update is still ready, try again".into());
            return;
        }

        let version = staged.version;
        let target = staged.target().to_path_buf();
        match staged.apply() {
            Ok(()) => {
                // `set_restart_path` before `restart`, because on macOS the
                // thing to reopen is the bundle and on Linux the binary, and
                // gpui's own guess is about where this process came from
                // rather than where the new version went.
                cx.set_restart_path(target);
                self.tell(format!("installed mbrd {version} — restarting"));
                cx.restart();
            }
            Err(err) => {
                self.updating = Updating::Idle;
                self.warn(format!("could not install the update: {err:#}"));
            }
        }
    }

    /// Bring the marks beside each card a frame nearer where they belong.
    ///
    /// Linear rather than sprung, and that is on purpose: this is a light
    /// coming up, not a thing being moved, and a spring on an opacity buys
    /// overshoot nobody can see and a settle time everybody can.
    fn fade_anchors(&mut self, dt: f32) -> bool {
        // Which cards are being offered marks *and could draw them* — the
        // painter's own two rules, applied here so that the map below is
        // bounded by the screen rather than by the selection. Ctrl A on a
        // board of twenty thousand offers twenty thousand cards; at any zoom
        // where a mark is legible, only a screenful of them can be seen, and
        // at any zoom where they are not, none of them qualify.
        //
        // Nothing is offered mid-gesture or mid-edit, which is also the
        // painter's rule — the difference is that the marks that were up now
        // fade out instead of vanishing the instant a drag starts.
        let mut wanted: HashSet<String> = HashSet::new();
        if matches!(self.gesture, Gesture::None)
            && self.editing.is_none()
            && (self.hovering.is_some() || !self.selection.is_empty())
        {
            // Off the index and filtered down to the offered cards, rather
            // than off the offered cards and looked up one by one: this runs
            // every frame, and a walk of the selection with a `Board::item`
            // scan inside it made Ctrl A cost selection times cards.
            let visible = self.viewport.visible();
            let mut found = Vec::new();
            self.index().in_rect(visible, &mut found);
            let selected: HashSet<&str> = self.selection.iter().map(String::as_str).collect();
            let items = &self.doc.board.items;
            for i in found {
                let item = &items[i as usize];
                let offered = self.hovering.as_deref() == Some(item.id.as_str())
                    || selected.contains(item.id.as_str());
                if !offered {
                    continue;
                }
                let card = Rect::of_item(item);
                if anchor::too_small(card, &self.viewport) || !card.intersects(&visible) {
                    continue;
                }
                wanted.insert(item.id.clone());
            }
        }

        let mut live = false;
        for id in &wanted {
            let fade = self.anchor_fade.entry(id.clone()).or_insert(0.0);
            if *fade < 1.0 {
                *fade = (*fade + dt / ANCHOR_IN).min(1.0);
                live = true;
            }
        }
        self.anchor_fade.retain(|id, fade| {
            if wanted.contains(id) {
                return true;
            }
            *fade -= dt / ANCHOR_OUT;
            live = true;
            *fade > 0.0
        });
        live
    }

    /// Take down the line if its time is up.
    fn expire_status(&mut self, now: Instant) {
        if let Some(Said { until: Some(until), .. }) = &self.said {
            if now >= *until {
                self.said = None;
            }
        }
    }

    /// Arrange for the line to come down by itself, if one is on a clock and
    /// nothing is already waiting to do it.
    ///
    /// Called once a frame and cheap in the ordinary case, which is that there
    /// is nothing to say or something is already armed.
    fn arm_status(&mut self, cx: &mut Context<Self>) {
        if self.said_timer {
            return;
        }
        // Nothing to take down that anybody can see. A `Tone::Done` still
        // expires — `expire_status` runs on the next frame there is — but it
        // does not earn a timer and the repaint at the end of one.
        let Some(Said { until: Some(until), tone, .. }) = &self.said else { return };
        if !tone.shown() {
            return;
        }
        let wait = until.saturating_duration_since(Instant::now());
        self.said_timer = true;
        cx.spawn(async move |view, cx| {
            cx.background_executor().timer(wait).await;
            // `ok()` for the same reason the decode does it: a window closing
            // with a timer outstanding is ordinary, not an error.
            view.update(cx, |view, cx| {
                view.said_timer = false;
                view.expire_status(Instant::now());
                // Unconditional, because this is also what re-arms the next
                // one: a message that arrived while this timer was running has
                // its own deadline and nothing waiting on it.
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A window position, in world units.
    fn world_at(&self, position: gpui::Point<Pixels>) -> WorldPoint {
        let local = point(
            f(position.x) - f(self.canvas_bounds.origin.x),
            f(position.y) - f(self.canvas_bounds.origin.y),
        );
        self.viewport.to_world(local)
    }

    /// Bring the spatial index level with the board, and hand it over.
    ///
    /// **The only way to reach the grid.** A grid is built from a list of items
    /// and is valid only for that list, so a stale one does not answer a little
    /// bit wrong — it answers about cards that have moved or are no longer
    /// there, which is a click selecting the wrong thing. Funnelling every
    /// reader through here means there is no way to ask the question without
    /// first checking, rather than a rule three call sites have to remember.
    ///
    /// Rebuilding is `O(n)` and the board's revision does not change while you
    /// pan, which is what makes this worth having: the common gesture reuses
    /// one index across every frame of itself.
    ///
    /// The gesture that *does* change it every frame is a drag, and that is
    /// what [`Grid::refile`] is for: a move writes to the board through
    /// `during`, which bumps the revision on every frame of the gesture, so
    /// the cheap-because-rare rebuild above turns into the most expensive
    /// thing in the frame for exactly as long as somebody is holding a card.
    /// Refiling asks the grid to move the cards that moved instead, and says
    /// so when it cannot — a card added or removed, or a selection so large
    /// that the rebuild is genuinely cheaper — which is when we build.
    fn index(&mut self) -> &Grid {
        if self.grid_at != self.doc.board.revision() {
            if !self.grid.refile(&self.doc.board.items) {
                self.grid = Grid::build(&self.doc.board.items);
            }
            self.grid_at = self.doc.board.revision();
        }
        &self.grid
    }

    /// Who holds what, rebuilt only when the board has changed.
    ///
    /// **The only way to reach the measurement.** Same bargain as
    /// [`index`](Self::index) and for the same reason — see the field.
    ///
    /// Rebuilt rather than patched, unlike the grid: `Fences::measure` is
    /// linear in the items and quadratic only in the *fences*, and a board with
    /// enough fences for that to hurt is a board with other problems. See
    /// `core::fence`, which makes the same argument.
    fn fences(&mut self) -> &Fences {
        if self.fences_at != self.doc.board.revision() {
            self.fences = Fences::measure(&self.doc.board.items);
            self.fences_at = self.doc.board.revision();
        }
        &self.fences
    }

    /// The topmost item under a world point, by id.
    ///
    /// Highest `z` first, so what you press is what you can see. Ties fall back
    /// to document order reversed, which is the same rule the painter uses —
    /// if the two disagreed, you could press one card and select another.
    ///
    /// The grid narrows the field to the cards near the pointer and
    /// [`geometry::hit`] decides between them, because the grid files a rotated
    /// card by the box it reaches into rather than by the card itself.
    fn hit(&mut self, p: WorldPoint) -> Option<String> {
        let mut near = Vec::new();
        self.index().at(p, &mut near);
        let items = &self.doc.board.items;
        near.into_iter()
            .filter(|&i| geometry::hit(&items[i as usize], p))
            .max_by(|&a, &b| by_depth(items, a, b))
            .map(|i| items[i as usize].id.clone())
    }

    /// Everything worth drawing, back to front, as positions in `board.items`.
    ///
    /// Back to front is the paint order and it is the same order [`Self::hit`]
    /// searches in reverse, which is not a coincidence worth losing: the moment
    /// they disagree, pressing a card selects the one behind it.
    fn visible_by_depth(&mut self) -> Vec<u32> {
        let window = self.viewport.visible().inflate(CULL_MARGIN);
        let mut found = Vec::new();
        self.index().in_rect(window, &mut found);
        let items = &self.doc.board.items;
        found.sort_by(|&a, &b| by_depth(items, a, b));
        found
    }

    fn is_selected(&self, id: &str) -> bool {
        self.selection.iter().any(|s| s == id)
    }

    fn select_only(&mut self, id: &str) {
        self.selection.clear();
        self.selection.push(id.to_string());
    }

    fn toggle_selected(&mut self, id: &str) {
        match self.selection.iter().position(|s| s == id) {
            Some(i) => {
                self.selection.remove(i);
            }
            None => self.selection.push(id.to_string()),
        }
    }

    /// The box around everything worth framing.
    ///
    /// Furniture is excluded on purpose: fitting to a board whose title card
    /// sits a long way from its photographs would frame mostly empty paper.
    fn content_bounds(&self) -> Option<Rect> {
        geometry::union(self.doc.board.items.iter().filter(|i| i.kind.is_content()))
    }

    // -----------------------------------------------------------------------
    // Commands
    // -----------------------------------------------------------------------

    pub fn go_home(&mut self, cx: &mut Context<Self>) {
        // Through the camera rather than onto the viewport, so that the origin
        // is somewhere you are taken rather than somewhere you appear. On a
        // canvas that goes on forever the camera is the whole of somebody's
        // sense of place, and a cut throws it away: you arrive at the origin
        // with no idea which direction you came from or how far.
        let mut want = self.viewport;
        want.home();
        self.camera.travel_to(&want);
        cx.notify();
    }

    /// Frame the whole board.
    ///
    /// Capped at 100%, because a board of three cards should open readable
    /// rather than magnified until the grain shows.
    pub fn fit_all(&mut self, cx: &mut Context<Self>) {
        let bounds = self.content_bounds();
        // The arithmetic stays in `core` and only the travelling is new: `fit`
        // decides which viewport, the camera decides how to get there.
        let mut want = self.viewport;
        want.fit(bounds, 80.0, BASE_ZOOM);
        self.camera.travel_to(&want);
        cx.notify();
    }

    /// `Ctrl +`. Zoom in one notch, centred on the middle of the view.
    ///
    /// The wheel was the only way to reach `Camera::zoom_by` — see the module
    /// note on `camera.rs` and the note above `Command::ZoomIn` in
    /// `command.rs` for why that was a gap on an infinite canvas rather than a
    /// missing convenience: a keyboard-only visitor cannot aim a wheel
    /// anywhere, so without this there was no way to look closer at anything
    /// at all. Aimed at the middle of the view rather than at a cursor
    /// position, because a key press has no position to aim with — and going
    /// through `zoom_by` is what makes this cost nothing extra: the spring,
    /// the rubber band at the ends of the range and reduced motion all come
    /// for free, the same way they do for the wheel.
    pub fn zoom_in(&mut self, cx: &mut Context<Self>) {
        self.zoom_by_key(1.0 + ZOOM_PER_LINE, cx);
    }

    /// `Ctrl -`. The other half of [`Self::zoom_in`].
    pub fn zoom_out(&mut self, cx: &mut Context<Self>) {
        self.zoom_by_key(1.0 / (1.0 + ZOOM_PER_LINE), cx);
    }

    /// Shared by [`Self::zoom_in`] and [`Self::zoom_out`]: one notch, about
    /// the middle of the view.
    fn zoom_by_key(&mut self, factor: f32, cx: &mut Context<Self>) {
        let centre = (self.viewport.size.width / 2.0, self.viewport.size.height / 2.0);
        self.camera.zoom_by(factor, centre, &self.viewport);
        cx.notify();
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.selection = self
            .doc
            .board
            .items
            .iter()
            .filter(|i| i.kind.is_content())
            .map(|i| i.id.clone())
            .collect();
        cx.notify();
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        // A group first, because leaving one is the nearer of the two things
        // `Escape` means — the same rule that puts a tool away and closes a
        // menu before either of them reaches here. Stepping out of four nested
        // groups takes four presses and then a fifth to let go, which is what
        // "the nearest thing first" has to mean when the nearest thing nests.
        if self.leave_group(cx) {
            return;
        }
        self.let_go(cx);
    }

    /// Let go of everything, keeping it where Ctrl Z can find it.
    ///
    /// The one door out of a selection — Escape, "Select none", and a click on
    /// the paper all come through here, so all three are equally reversible.
    /// Nothing is recorded when there was nothing to let go of: an empty entry
    /// on the stack would be a press of Ctrl Z that appeared to do nothing.
    fn let_go(&mut self, cx: &mut Context<Self>) {
        if self.selection.is_empty() && self.rope.is_none() {
            return;
        }
        let n = self.selection.len();
        let cards = std::mem::take(&mut self.selection);
        let rope = self.rope.take();
        self.let_go.push(cards, rope, self.doc.board.revision());
        // A deselection is not itself a report of what you just watched — it
        // is the one line that says Ctrl Z still reaches it, which is not
        // otherwise knowable from looking at an empty board.
        self.tell(match n {
            0 => "let go — ctrl z puts it back".to_string(),
            1 => "let go of one — ctrl z puts it back".to_string(),
            n => format!("let go of {n} — ctrl z puts it back"),
        });
        cx.notify();
    }

    /// What Ctrl Z would take back next, by name.
    ///
    /// The menu draws the answer, so this is what makes the entry read "Undo
    /// selection" for a let-go rather than naming the step underneath it that
    /// Ctrl Z is not about to touch.
    pub fn undo_step(&self) -> Option<String> {
        if self.let_go.holding(self.doc.board.revision()) {
            return Some("selection".into());
        }
        self.doc.board.undo_label().map(str::to_string)
    }

    pub fn redo_step(&self) -> Option<String> {
        if self.let_go.forward.last().is_some_and(|held| held.at == self.doc.board.revision()) {
            return Some("selection".into());
        }
        self.doc.board.redo_label().map(str::to_string)
    }

    /// Move the selection to the bin.
    ///
    /// **The bin lasts as long as the app is open and no longer.** It is not
    /// written to the file — see `mbrd_core::model::TrashEntry` — so this is
    /// "delete, and keep the pieces where an undo can reach them" rather than
    /// somewhere to go looking later. There is nowhere to go looking: nothing
    /// in this app takes a card back out of the bin, and `Ctrl Z` is the route
    /// back. The bin is what makes that route one step instead of several.
    ///
    /// A binned item keeps its asset and its place in every connection that
    /// names it, because putting it back has to bring those with it.
    pub fn delete_selection(&mut self, cx: &mut Context<Self>) {
        // A selected rope takes the press. The two are never both live — see
        // `select_rope` — so this is a choice between them rather than a
        // guess at which one was meant.
        if self.rope.is_some() {
            self.delete_rope(cx);
            return;
        }
        if self.selection.is_empty() {
            return;
        }
        // Through `kin`, like a copy and like a drag: binning a group bins what
        // is in it. Leaving the contents behind would empty the rectangle
        // rather than remove the grouping, and there is already a word for
        // removing the grouping — see `ungroup`.
        // Locked cards are left out, for the reason a drag leaves them where
        // they are: a lock says this one is not to be disturbed, and being
        // inside a group somebody binned is not consent. A locked *fence*
        // keeps its contents for the same reason — see `unlocked_kin`.
        let doomed: Vec<String> = self.unlocked_kin(self.selection.clone());
        if doomed.is_empty() {
            self.tell("locked".into());
            cx.notify();
            return;
        }
        self.selection.clear();
        let at = mbrd_core::naming::now_millis();
        let binned = self.doc.board.edit("To the bin", |board| {
            let mut binned = 0;
            for id in &doomed {
                if let Some(i) = board.items.iter().position(|it| &it.id == id) {
                    let item = board.items.remove(i);
                    board.trash.insert(0, TrashEntry { item, at });
                    binned += 1;
                }
            }
            // The bin has a ceiling; past it the oldest fall out. Note that what
            // falls out here still has its bytes in `assets` — nothing is
            // destroyed by this, only forgotten by the bin, and the ledger keeps
            // naming them so an undo can still put them back.
            board.trash.truncate(mbrd_core::model::TRASH_LIMIT);
            binned
        });
        // Connections naming a deleted card are deliberately *not* pruned. That
        // is what lets delete, undo and restore work with no bookkeeping at any
        // of them; the pruning happens once, at the file boundary.
        //
        // A *playhead* is pruned, though, and for the opposite reason: it is
        // not part of the document and an undo does not bring one back, so a
        // card that leaves the board should stop being something that plays.
        // Binning a video that was playing and leaving it playing would be a
        // board making a noise about a card that is not on it.
        for id in &doomed {
            self.media.forget(id);
            self.timings.forget(id);
        }
        self.say(format!("{binned} to the bin"));
        cx.notify();
    }

    /// Take back the last thing that happened.
    ///
    /// The selection is left alone rather than restored with the board, and
    /// that is deliberate: a selection is where your attention is, not part of
    /// the document, and no step records one. What it *is* held to is the board
    /// that came back — an id that undo has taken off the board is dropped from
    /// the selection here, since a selected card that does not exist would be
    /// drawn nowhere and deleted by the next press of `Del`.
    pub fn undo(&mut self, cx: &mut Context<Self>) {
        // Typing is the newest thing there is to take back, so the session is
        // closed into a step of its own before the ledger is walked. Two
        // reasons, and the second is the one that made this a bug: a press
        // that reached past what was being typed would take back something
        // older than the thing on screen, and the editor left open behind it
        // writes its own text onto the card on the next keystroke — over
        // whatever came back. See
        // `an_open_gesture_left_across_an_undo_writes_itself_back`, which is
        // the mechanism, and `save`, which closes the session for the same
        // reason. A no-op when nothing is being typed, which is nearly always.
        self.stop_editing(true, cx);

        // A selection let go of since the last edit is the newest thing there
        // is to take back, so it goes next. It is not in the ledger and never
        // will be — see [`Held`] — which is why this is a branch here rather
        // than a step in `history.rs`. Closing a session above moves the board
        // on, which is what drops the stack where there was one: a selection
        // restored across a step is a selection restored onto a board it was
        // not made on.
        if let Some(held) = self.let_go.take_back(self.doc.board.revision()) {
            self.selection = held.cards;
            self.rope = held.rope;
            self.say("selection back".into());
            cx.notify();
            return;
        }
        match self.doc.board.undo() {
            Some(label) => {
                self.prune_selection();
                self.say(format!("undid {}", label.to_lowercase()));
            }
            // Said rather than silent. An undo that does nothing is either the
            // start of the history or a ledger this build could not vouch for,
            // and the two feel identical from the keyboard.
            None if self.doc.board.timeline().stale() => {
                self.warn("this board's history does not match it".into());
            }
            None => self.tell("nothing to undo".into()),
        }
        cx.notify();
    }

    /// And forward again.
    ///
    /// Closes an open editing session first, exactly as [`Self::undo`] does and
    /// for the same reason. What is ahead of the marker is dropped by the step
    /// that lands, so a redo pressed mid-sentence answers "nothing to redo" —
    /// which is the truth: typing something new is how you leave the future
    /// behind, here as everywhere else in the ledger.
    pub fn redo(&mut self, cx: &mut Context<Self>) {
        self.stop_editing(true, cx);
        if self.let_go.again(self.doc.board.revision()).is_some() {
            self.selection.clear();
            self.rope = None;
            self.say("let go again".into());
            cx.notify();
            return;
        }
        match self.doc.board.redo() {
            Some(label) => {
                self.prune_selection();
                self.say(format!("redid {}", label.to_lowercase()));
            }
            None => self.tell("nothing to redo".into()),
        }
        cx.notify();
    }

    /// Drop from the selection anything the board no longer has.
    fn prune_selection(&mut self) {
        let board = &self.doc.board;
        self.selection.retain(|id| board.item(id).is_some());
        // And where you were standing, for the same reason: a fence undone
        // away while you were inside it would leave presses reaching through a
        // grouping nothing on screen could show you.
        self.prune_inside();
    }

    /// `Ctrl S`. Write the board out now and say so.
    ///
    /// **Everything is written without this.** See [`Self::arm_autosave`]: the
    /// board goes to disk a second after the last change, every time, which is
    /// why there is no unsaved-work indicator anywhere in this app. What the
    /// command is still for is the second in between — somebody who has just
    /// typed something and wants to know it is safe before shutting the lid —
    /// and saying so out loud is most of its value.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        // `Ctrl S` reaches here from inside a note, so what is on the card has
        // to be what is written rather than what was there before typing. The
        // autosave path deliberately does *not* do this — see there.
        self.stop_editing(true, cx);
        self.write_board(true, cx);
    }

    /// Write the board out, off the main thread.
    ///
    /// The camera is captured first, so that a save records where you are
    /// looking rather than where the file was opened. That is done here rather
    /// than left to the callers for the reason the original gives: three call
    /// sites would each have had to remember, and the one that forgot would
    /// ship files a day out of date.
    ///
    /// **The archive is built on the background executor**, and that is what
    /// makes an autosave something you cannot feel. Writing a `.mbrd` means
    /// deflating every asset on it, which on a board of photographs is hundreds
    /// of milliseconds — a hitch you would notice once a second. What happens
    /// on this thread is the `Document` clone that gets handed over, which is a
    /// memcpy of bytes already in memory and costs a frame's worth of nothing.
    ///
    /// `announce` is whether the bar says it happened. False for the timer,
    /// which is the whole point of a timer: an app that reported every autosave
    /// would have replaced the indicator this removed with a noisier one.
    /// Failures are said either way.
    fn write_board(&mut self, announce: bool, cx: &mut Context<Self>) {
        // One at a time. The timer will come back around, and a `Ctrl S`
        // pressed while a write is already going is a `Ctrl S` that has already
        // been answered.
        if self.saving {
            return;
        }
        self.capture_view();

        let Some(path) = self.path.clone().or_else(|| fresh_board_path(&self.doc.board)) else {
            self.warn("nowhere to save: no home directory".into());
            return;
        };
        // Read *before* the clone rather than after the write, so a change made
        // while the archive was being packed is not marked as having been in
        // it. Getting this backwards is the autosave bug that loses the last
        // thing somebody typed.
        let at = self.doc.board.revision();
        let doc = self.doc.clone();

        self.saving = true;
        cx.spawn(async move |view, cx| {
            let written = cx
                .background_executor()
                .spawn(async move { crate::save::write(&path, &doc).map(|()| path) })
                .await;
            view.update(cx, |this, cx| this.wrote_board(at, announce, written, cx)).ok();
        })
        .detach();
    }

    /// What to do with a write that has landed.
    fn wrote_board(
        &mut self,
        at: u64,
        announce: bool,
        written: anyhow::Result<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.saving = false;
        match written {
            Ok(path) => {
                self.saved_at = at;
                self.failed_at = None;
                // Whether this write is the one that gave a pathless board —
                // the demo, or anything opened with nowhere to live — a file
                // for the first time. Read before `self.path` is overwritten
                // below.
                let adopted = self.path.is_none();
                // Only adopt the path once the write has actually landed. A
                // failed first save that still moved the target would send the
                // next one somewhere nobody asked for.
                if self.path.as_deref() != Some(path.as_path()) {
                    self.path = Some(path.clone());
                    crate::recent::remember(&path);
                }
                if announce {
                    // A write to disk is invisible by nature — nothing on the
                    // board changes to show it happened — so this is `tell`,
                    // not `say`: the whole value of `Ctrl S` is saying so.
                    self.tell(format!("saved {}", short_name(&path)));
                } else if adopted {
                    // The one autosave worth announcing even though `announce`
                    // is false: a board that had no file just quietly got one
                    // — see `fresh_board_path` — and finding that out by
                    // accident weeks later, in a file manager, is worse than
                    // one line in the status bar now.
                    self.tell(format!("saved as {} in your boards folder", short_name(&path)));
                }
            }
            // Reported, never swallowed, and left up for longer than a
            // success: a save that silently failed is the one failure mode
            // this app must not have, and it is a worse one now that nothing
            // else on screen is reporting on the state of the disk.
            Err(err) => {
                self.failed_at = Some(at);
                self.warn(format!("could not save: {err:#}"));
                self.arm_retry(at, cx);
            }
        }
        cx.notify();
    }

    /// Try the failed save again in a few seconds, on its own clock.
    ///
    /// Without this, a save that fails because a drive was unplugged, or a
    /// network mount hiccuped, sits failed until the next keystroke gives
    /// `arm_autosave` a reason to try — which might be minutes away, or might
    /// never come if whoever is looking has stepped away from the board
    /// entirely. This is what turns "reconnect the drive and wait a moment"
    /// back into "it saved" without anybody having to touch the board again.
    fn arm_retry(&mut self, at: u64, cx: &mut Context<Self>) {
        cx.spawn(async move |view, cx| {
            cx.background_executor().timer(RETRY_AFTER).await;
            view.update(cx, |this, cx| {
                // Only while the failure this timer was armed for still
                // stands — a normal autosave, or `Ctrl S`, may already have
                // retried it and either cleared it or moved it on to a later
                // revision, in which case this timer has nothing left to do.
                // Not mid-gesture either, for the reason `arm_autosave` isn't.
                if this.failed_at == Some(at) && matches!(this.gesture, Gesture::None) {
                    this.write_board(false, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Set a timer to write the board out, if there is anything to write.
    ///
    /// Called from the paint, beside `arm_status`, because every change to the
    /// board is followed by a repaint — so "there is something to save" and
    /// "there is about to be a frame" are the same moment.
    ///
    /// A timer rather than a write per change, and the gap is doing real work:
    /// a keystroke in a note is a change, and a board written once per
    /// character would spend its life deflating the same photographs. A second
    /// of quiet is under the time it takes to look away from what you typed.
    fn arm_autosave(&mut self, cx: &mut Context<Self>) {
        if self.save_timer || !self.unsaved() {
            return;
        }
        // A write that failed is not retried until the board moves on. See
        // `failed_at`.
        if self.failed_at == Some(self.doc.board.revision()) {
            return;
        }
        self.save_timer = true;
        cx.spawn(async move |view, cx| {
            cx.background_executor().timer(AUTOSAVE_AFTER).await;
            view.update(cx, |this, cx| {
                this.save_timer = false;
                // Not mid-gesture. A hand held still for a second in the middle
                // of a drag would otherwise file a position nobody has let go
                // of yet — and the release is a repaint, which arms this again.
                //
                // Typing is deliberately *not* excluded. An editing session is
                // minutes long where a drag is seconds, and the words on the
                // card are exactly the work that would hurt to lose.
                if matches!(this.gesture, Gesture::None) {
                    this.write_board(false, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Write the board out **now**, on this thread, because the window is
    /// closing. Returns whether it is safe to let the window go.
    ///
    /// The one place the archive is built on the main thread, and the reason is
    /// that there is no later: a background task handed off from a closing
    /// window has nowhere to report back to and may not outlive the process. A
    /// hitch nobody sees, on a window that is going away, is a fair price for
    /// the second of work the timer had not reached yet.
    ///
    /// Quiet on success — there is no bar left to read it on once the window
    /// is gone. A failure is the opposite: this is the last moment there is a
    /// bar at all, so it is the last chance to say the disk did not take the
    /// board, and the caller uses the `false` to keep the window open long
    /// enough for that line to be read.
    pub fn flush(&mut self, cx: &mut Context<Self>) -> bool {
        self.stop_editing(true, cx);
        if !self.unsaved() {
            self.close_refused = false;
            return true;
        }
        self.capture_view();
        let Some(path) = self.path.clone().or_else(|| fresh_board_path(&self.doc.board)) else {
            return true;
        };
        match crate::save::write(&path, &self.doc) {
            Ok(()) => {
                self.saved_at = self.doc.board.revision();
                crate::recent::remember(&path);
                self.close_refused = false;
                true
            }
            Err(err) => {
                // Told once. A second close attempt while this stands is
                // somebody who read the warning and is leaving anyway — the
                // work is already as lost as it is going to get, and refusing
                // again would only be trapping them behind their own choice.
                if self.close_refused {
                    true
                } else {
                    self.close_refused = true;
                    self.warn(format!("could not save — {err:#}"));
                    cx.notify();
                    false
                }
            }
        }
    }

    /// Drop a fresh note in the middle of the view.
    pub fn add_note(&mut self, cx: &mut Context<Self>) {
        let middle = point(self.viewport.pan.x, self.viewport.pan.y);
        self.add_note_at(middle, cx);
    }

    /// Put a note down at a place on the board.
    ///
    /// The middle of the window for the command, and where you pressed for the
    /// Note tool — which is the whole difference between them, and the reason
    /// this takes a point rather than being two functions.
    pub fn add_note_at(&mut self, at: WorldPoint, cx: &mut Context<Self>) {
        let id = self.fresh_id();
        let mut note = Item::new(id.clone(), ItemType::Note);
        note.name = "note".into();
        note.w = 220.0;
        note.h = 180.0;
        note.x = at.x;
        note.y = at.y;
        note.z = self.top_z() + 1.0;
        note.meta.insert("text".into(), serde_json::Value::String("# note".into()));
        self.doc.board.edit("Add note", |board| board.items.push(note));
        self.select_only(&id);
        // Open for typing straight away. A note is a thing you put down in
        // order to write on it, and the alternative was a note that looked
        // ready for words while every letter typed at it was still a shortcut
        // — `h` picking up the Pan tool being the memorable one.
        //
        // Its whole placeholder is selected rather than the caret left at the
        // end, so the first letter *replaces* "# note" instead of continuing
        // it. Clicking away without typing leaves the placeholder, which is
        // what makes the note visible for somebody who only wanted the shape.
        self.edit_card(&id, true, cx);
        cx.notify();
    }

    fn top_z(&self) -> f32 {
        self.doc.board.items.iter().map(|i| i.z).fold(0.0_f32, f32::max)
    }

    /// An id nothing on the board is using, live or binned.
    ///
    /// Both, because a restore from the bin must not collide with something
    /// made since — the same one id space the file reader maintains.
    fn fresh_id(&self) -> String {
        self.fresh_id_from(0)
    }

    /// The same, for the `nth` card of a batch that is not on the board yet.
    ///
    /// A drop of forty files asks forty times before any of them have landed,
    /// so without the offset every one of them would be handed the same id and
    /// thirty-nine would be lost at the next save.
    fn fresh_id_from(&self, nth: usize) -> String {
        let taken = |id: &str| {
            self.doc.board.items.iter().any(|i| i.id == id)
                || self.doc.board.trash.iter().any(|t| t.item.id == id)
        };
        let mut n = self.doc.board.items.len() + nth;
        loop {
            let id = format!("n{n:06}");
            if !taken(&id) {
                return id;
            }
            n += 1;
        }
    }

    /// Put the selection in front of everything, or behind it.
    ///
    /// Relative order within the selection is kept: raising three cards that
    /// were stacked one way should not shuffle them, only lift them together.
    ///
    /// `z` is a number rather than a position in a list, so this hands out
    /// whole numbers past whichever edge it is heading for rather than
    /// renumbering the board. That leaves a board's `z` values slowly spreading
    /// out over a long session, which is fine — they are `f32`, and the
    /// alternative is a step that rewrites every item on the board to move one.
    pub fn raise_selection(&mut self, front: bool, cx: &mut Context<Self>) {
        if self.selection.is_empty() {
            return;
        }
        let ids = self.selection.clone();
        let label = if front { "Bring to front" } else { "Send to back" };
        self.doc.board.edit(label, |board| {
            // The edge of everything that is *not* moving. Where nothing is
            // staying put, everything is already at the front and the back at
            // once and there is nothing to do.
            let edge = board
                .items
                .iter()
                .filter(|i| !ids.iter().any(|id| id == &i.id))
                .map(|i| i.z)
                .fold(f32::NAN, if front { f32::max } else { f32::min });
            if !edge.is_finite() {
                return;
            }

            let mut moving: Vec<usize> = (0..board.items.len())
                .filter(|&i| ids.iter().any(|id| id == &board.items[i].id))
                .collect();
            moving.sort_by(|&a, &b| by_depth(&board.items, a as u32, b as u32));

            let count = moving.len() as f32;
            for (rank, i) in moving.into_iter().enumerate() {
                let step = rank as f32 + 1.0;
                board.items[i].z = if front { edge + step } else { edge - count + step - 1.0 };
            }
        });
        self.say(if front { "brought to front".into() } else { "sent to back".into() });
        cx.notify();
    }

    /// Turn one of the board's own switches.
    ///
    /// Recorded, unlike the camera. These live in the file — `layoutSettings`,
    /// which [`schema::REST_FIELDS`](mbrd_core::schema::REST_FIELDS) lists — so
    /// they are part of the document rather than part of the view, and a
    /// document change that undo could not take back would be the odd one out.
    /// Flip one of the two *preferences* and write it to disk.
    ///
    /// Deliberately not `toggle_setting`, and the difference is the whole point
    /// of `prefs.rs`: a board setting is a fact about the board and goes in the
    /// file, onto the undo ledger, and to whoever the `.mbrd` is sent to. A
    /// preference is a fact about the person sitting here, belongs in their
    /// config directory, and must not travel. Undo does not reach it either —
    /// "how much do I want the screen to move" is not an edit to a moodboard.
    ///
    /// Written straight away rather than at exit, because an app that loses a
    /// setting when it is killed has not really been told.
    pub fn toggle_pref(&mut self, which: Command, cx: &mut Context<Self>) {
        let (label, flag) = match which {
            Command::ToggleMotion => ("Animation", &mut self.prefs.motion),
            Command::ToggleUpdateChecks => ("Looking for new versions", &mut self.prefs.update),
            _ => return,
        };
        *flag = !*flag;
        let now = *flag;
        crate::prefs::save(&self.prefs);

        // An environment variable beats the file at load, so a choice that is
        // being overridden has to say so rather than appearing to take and then
        // silently not surviving a restart.
        match crate::prefs::Prefs::forced(match which {
            Command::ToggleMotion => crate::prefs::Setting::Motion,
            _ => crate::prefs::Setting::Update,
        }) {
            Some(var) => self.warn(format!("{label} is set by {var}, which wins at startup")),
            // The menu this came from closes on the press that got here, so
            // there is nothing left on screen to confirm the flip — the bar
            // is the only place left to say it happened.
            None => self.tell(format!("{label} {}", if now { "on" } else { "off" })),
        }
        cx.notify();
    }

    pub fn toggle_setting(&mut self, which: Command, cx: &mut Context<Self>) {
        let label = match which {
            Command::ToggleGrid => "Grid",
            Command::ToggleAxes => "Axes",
            Command::ToggleSnap => "Snapping",
            Command::ToggleWeb => "Connections",
            Command::ToggleGuides => "Alignment guides",
            Command::ToggleHud => "Scale bar",
            Command::ToggleLandscape => "Landscape",
            _ => return,
        };
        let now = self.doc.board.edit(label, |board| {
            let settings = &mut board.settings.desktop;
            let flag = match which {
                Command::ToggleGrid => &mut settings.grid,
                Command::ToggleAxes => &mut settings.axes,
                Command::ToggleWeb => &mut settings.web,
                Command::ToggleGuides => &mut settings.guides,
                Command::ToggleHud => &mut settings.hud,
                Command::ToggleLandscape => &mut settings.paper_landscape,
                _ => &mut settings.snap,
            };
            *flag = !*flag;
            *flag
        });

        // Snapping is a setting that *moves things*, which is what makes it
        // different from the other three. Turning it on takes the whole board
        // onto the lattice and turning it off puts back everything it took —
        // see `core::snap`, which is where the memo that makes that possible
        // lives. A second step rather than one, so the two are separately
        // undoable: somebody who snapped a board and wants only the cards back
        // should not have to turn the setting off as well.
        if which == Command::ToggleSnap {
            let step = self.doc.board.settings.desktop.grid_step;
            // Wherever every card was, so a whole board snapping onto — or
            // back off — the lattice can be caught the same way `arrange`
            // catches an align. See `Self::present_move`. Taken over the
            // whole board rather than just the selection because the snap
            // itself does not ask what is selected; it moves everything.
            let before: HashMap<String, (f32, f32)> = self
                .doc
                .board
                .items
                .iter()
                .map(|item| (item.id.clone(), (item.x, item.y)))
                .collect();
            let moved =
                self.doc.board.edit(if now { "Snap to grid" } else { "Off the grid" }, |board| {
                    if now {
                        mbrd_core::snap::engage(board, mbrd_core::LayoutMode::Desktop, step)
                    } else {
                        mbrd_core::snap::release(board, mbrd_core::LayoutMode::Desktop)
                    }
                });
            if moved {
                // Cloned out rather than walked in place: the loop below
                // wants `self` mutably for `present_move`, and the board is
                // not one of the fields that call can be split around.
                let after: Vec<(String, f32, f32)> = self
                    .doc
                    .board
                    .items
                    .iter()
                    .map(|item| (item.id.clone(), item.x, item.y))
                    .collect();
                for (id, x, y) in after {
                    if let Some(&(ox, oy)) = before.get(&id) {
                        self.present_move(&id, ox - x, oy - y);
                    }
                }
                self.say(if now { "snapped to the grid" } else { "put back" }.into());
                cx.notify();
                return;
            }
        }

        self.say(format!("{} {}", label.to_lowercase(), if now { "on" } else { "off" }));
        cx.notify();
    }

    /// Outline a different sheet of paper, or none — `Command::Paper`'s door.
    pub fn set_paper(&mut self, size: mbrd_core::paper::PaperSize, cx: &mut Context<Self>) {
        let id = size.id();
        if self.doc.board.settings.desktop.paper == id {
            return;
        }
        self.doc.board.edit("Paper", |board| {
            board.settings.desktop.paper = id.to_string();
        });
        self.say(match size {
            mbrd_core::paper::PaperSize::NoSheet => "no paper".into(),
            _ => format!("{} paper", size.label()),
        });
        cx.notify();
    }

    /// Flip the scale bar between metric and imperial. Not `toggle_setting`:
    /// that one flips a `bool` in place, and this flips a string between two
    /// spellings — see `Command::ToggleUnits`'s own doc for why it is a
    /// switch and not a two-row submenu.
    pub fn toggle_units(&mut self, cx: &mut Context<Self>) {
        let now = self.doc.board.edit("Units", |board| {
            let settings = &mut board.settings.desktop;
            settings.units =
                if settings.units == "imperial" { "metric" } else { "imperial" }.into();
            settings.units.clone()
        });
        self.say(format!("{now} units"));
        cx.notify();
    }

    /// Set the grid's pitch.
    ///
    /// If snapping is on, the board follows the number: a board that says it
    /// is snapped must be snapped to the step it now shows, so the cards are
    /// taken onto the new lattice as a second, separately undoable step —
    /// the same shape `toggle_setting` gives `ToggleSnap`, and for the same
    /// reason: somebody who wants the shuffle back should not lose the
    /// number with it.
    pub fn set_grid_step(&mut self, step: f32, cx: &mut Context<Self>) {
        let step = step.clamp(1.0, 4096.0);
        if self.doc.board.settings.desktop.grid_step == step {
            return;
        }
        self.doc.board.edit("Grid step", |board| {
            board.settings.desktop.grid_step = step;
        });
        if self.doc.board.settings.desktop.snap {
            let before: HashMap<String, (f32, f32)> = self
                .doc
                .board
                .items
                .iter()
                .map(|item| (item.id.clone(), (item.x, item.y)))
                .collect();
            let moved = self.doc.board.edit("Snap to grid", |board| {
                mbrd_core::snap::engage(board, mbrd_core::LayoutMode::Desktop, step)
            });
            if moved {
                let after: Vec<(String, f32, f32)> = self
                    .doc
                    .board
                    .items
                    .iter()
                    .map(|item| (item.id.clone(), item.x, item.y))
                    .collect();
                for (id, x, y) in after {
                    if let Some(&(ox, oy)) = before.get(&id) {
                        self.present_move(&id, ox - x, oy - y);
                    }
                }
            }
        }
        self.say(format!("grid step {step}"));
        cx.notify();
    }

    /// Set the gap the arrangement engine leaves between cards.
    ///
    /// Nothing moves when it changes — the number is read at the next
    /// `Rearrange`, which is also where its effect can actually be seen.
    pub fn set_spacing(&mut self, gap: f32, cx: &mut Context<Self>) {
        let gap = gap.clamp(0.0, 512.0);
        if self.doc.board.settings.desktop.spacing == gap {
            return;
        }
        self.doc.board.edit("Card gap", |board| {
            board.settings.desktop.spacing = gap;
        });
        self.say(format!("card gap {gap}"));
        cx.notify();
    }

    /// Set how photos and videos sit in their cards, board-wide.
    ///
    /// `contain` or `cover`. A card can override it with `meta.fit`, and
    /// those overrides are deliberately left alone: they were each chosen
    /// against a particular picture, which changing the default is not a
    /// reason to forget.
    pub fn set_media_fit(&mut self, fit: &str, cx: &mut Context<Self>) {
        if self.doc.board.media_fit == fit {
            return;
        }
        let chosen = fit.to_string();
        self.doc.board.edit("Media fit", move |board| board.media_fit = chosen);
        self.say(format!("media {fit}"));
        cx.notify();
    }

    /// Drop a colour on the board.
    ///
    /// Grey, because there is no colour picker and inventing one would be a
    /// Phase 4 of its own. It opens for typing straight away, so the thing you
    /// do next is put the colour in — see [`write_field`], where a swatch's
    /// name *is* its colour.
    pub fn add_swatch(&mut self, cx: &mut Context<Self>) {
        let id = self.fresh_id();
        let mut swatch = Item::new(id.clone(), ItemType::Swatch);
        swatch.name = "#8C8C8C".into();
        swatch.meta.insert("hex".into(), serde_json::Value::String("#8c8c8c".into()));
        swatch.w = 160.0;
        swatch.h = 160.0;
        swatch.x = self.viewport.pan.x;
        swatch.y = self.viewport.pan.y;
        swatch.z = self.top_z() + 1.0;
        self.doc.board.edit("Add swatch", |board| board.items.push(swatch));
        self.select_only(&id);
        self.start_editing(&id, cx);
    }

    /// Tear the selected notes off a different part of the pad.
    ///
    /// Cycles rather than choosing, because four colours is a short enough list
    /// that pressing the key again is faster than picking from a menu — and it
    /// is what the original does when a note is made.
    pub fn cycle_tint(&mut self, cx: &mut Context<Self>) {
        let ids = self.selection.clone();
        if ids.is_empty() {
            return;
        }
        let changed = self.doc.board.edit("Tint", |board| {
            let mut changed = 0;
            for id in &ids {
                let Some(item) = board.item_mut(id) else { continue };
                if !matches!(item.kind, ItemType::Note | ItemType::Text | ItemType::Sticker) {
                    continue;
                }
                let now = item.meta.get("tint").and_then(serde_json::Value::as_u64).unwrap_or(0);
                let next = (now as u32 % crate::theme::NOTE_TINT_COUNT) + 1;
                item.meta.insert("tint".into(), serde_json::json!(next));
                changed += 1;
            }
            changed
        });
        self.say(match changed {
            0 => "nothing here takes a tint".into(),
            n => format!("tinted {n}"),
        });
        cx.notify();
    }

    /// Open whatever card is selected onto the whole window. `O`.
    ///
    /// The last one selected, which is the same card [`Self::rename`] picks and
    /// for the same reason: with several in hand, the one you touched most
    /// recently is the one you meant.
    pub fn open_selected(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.selection.last().cloned() else { return };
        self.open_card(&id, cx);
    }

    /// Open whatever is selected for typing. `F2`, and Enter.
    ///
    /// A rope first, because selecting one clears the card selection — so if
    /// there is a rope selected it is unambiguously the thing you meant, and a
    /// key that renames a card should label a rope for the same reason the
    /// menu's first entry does.
    pub fn rename(&mut self, cx: &mut Context<Self>) {
        if self.rope.is_some() {
            return self.start_labelling(cx);
        }
        let Some(id) = self.selection.last().cloned() else { return };
        self.start_editing(&id, cx);
    }

    /// Put the selection on the app's own clipboard, and cut it if asked.
    ///
    /// The cards are copied whole — everything except their ids, which are
    /// minted fresh on the way back in, because two cards with the same id is
    /// the one thing the format cannot represent. Their *names* also go on the
    /// system clipboard, so copying a card and pasting it into a text editor
    /// gets something rather than nothing.
    pub fn copy_selection(&mut self, cut: bool, cx: &mut Context<Self>) {
        // Through `kin`, so that a fence brings what it holds. Copying a group
        // and getting back an empty rectangle is the bug this is here to stop:
        // a fence has no member list to copy, its contents are whatever is
        // geometrically inside it, and nothing but this asks.
        let taken: Vec<Item> = self
            .kin(self.selection.clone())
            .iter()
            .filter_map(|id| self.doc.board.item(id).cloned())
            .collect();
        if taken.is_empty() {
            return;
        }
        let names: Vec<&str> = taken.iter().map(|i| i.name.as_str()).collect();
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(names.join("\n")));
        let count = taken.len();
        self.clipboard = taken;
        if cut {
            self.delete_selection(cx);
            return;
        }
        self.say(format!("copied {count}"));
        cx.notify();
    }

    /// Put the app's clipboard back on the board, offset so it is visible.
    ///
    /// Answers whether it had anything, so that `Ctrl V` can fall through to
    /// the system clipboard when it does not — pasting a photograph from
    /// somewhere else is the more common thing to want, and having one key do
    /// both in the obvious order is better than having two keys.
    pub fn paste_cards(&mut self, cx: &mut Context<Self>) -> bool {
        if self.clipboard.is_empty() {
            return false;
        }
        let count = self.clipboard.len();
        self.duplicate_these(self.clipboard.clone(), "Paste", cx);
        self.say(format!("pasted {count}"));
        cx.notify();
        true
    }

    /// A copy of the selection, a little down and to the right.
    pub fn duplicate_selection(&mut self, cx: &mut Context<Self>) {
        // Through `kin` for the same reason a copy is: `Ctrl D` on a group has
        // to duplicate the group, not leave a second empty rectangle beside it.
        let taken: Vec<Item> = self
            .kin(self.selection.clone())
            .iter()
            .filter_map(|id| self.doc.board.item(id).cloned())
            .collect();
        if taken.is_empty() {
            return;
        }
        let count = taken.len();
        self.duplicate_these(taken, "Duplicate", cx);
        self.say(format!("duplicated {count}"));
        cx.notify();
    }

    /// The shared half of duplicate and paste.
    ///
    /// Offset by a grid step rather than dropped exactly on top, because a copy
    /// somebody cannot see is a copy they make four of before noticing. The new
    /// cards end up selected, so the next thing they do applies to the copy
    /// rather than to the original.
    fn duplicate_these(&mut self, cards: Vec<Item>, label: &str, cx: &mut Context<Self>) {
        let step = self.doc.board.settings.desktop.grid_step.max(16.0);
        let mut z = self.top_z();
        // A fence among them keeps its place at the back rather than being
        // lifted with the rest: a copy of a group whose rectangle came out in
        // front of its own contents would hide them.
        let fresh: Vec<Item> = self
            .respawn(cards)
            .into_iter()
            .map(|mut card| {
                card.x += step;
                card.y -= step;
                if card.kind != ItemType::Fence {
                    z += 1.0;
                    card.z = z;
                }
                card
            })
            .collect();
        // Connections are deliberately not copied. A connection names two ids
        // and a copy is a different card, so carrying them across would either
        // wire the copy to the original's neighbours — which nobody asked for —
        // or need a whole remapping pass for something Phase 5 has not built
        // the drawing side of yet.
        //
        // What ends up selected is the fences among the copies, where there
        // are any, and everything otherwise. Selecting a copied group's forty
        // cards *and* the fence around them would leave the next drag holding
        // each card twice.
        let ids = pick_of(&fresh);
        let n = fresh.len();
        let label = if n == 1 { label.to_string() } else { format!("{label} {n}") };
        self.doc.board.edit(&label, |board| board.items.extend(fresh));
        self.selection = ids;
        cx.notify();
    }

    /// Give these cards ids of their own, keeping the groupings among them.
    ///
    /// The ids are the only thing a copy genuinely cannot share — two cards
    /// with the same id is the one thing the format cannot represent — and
    /// `meta.fence` is the only place an id is written on another card. So it
    /// is remapped here, through the same table.
    ///
    /// Getting that wrong is quiet rather than loud: membership is *measured*
    /// from the geometry, so a copy would look right immediately. But the
    /// stamp is what rescues a card that came back a float's breadth outside
    /// its fence — see `fence::SLACK` — and a stamp naming the *original* fence
    /// would one day rescue a copied card into the group it was copied out of.
    fn respawn(&self, cards: Vec<Item>) -> Vec<Item> {
        let renamed: HashMap<String, String> = cards
            .iter()
            .enumerate()
            .map(|(i, card)| (card.id.clone(), self.fresh_id_from(i)))
            .collect();
        cards
            .into_iter()
            .map(|mut card| {
                if let Some(was) = card.fence().and_then(|f| renamed.get(f)) {
                    let now = serde_json::Value::String(was.clone());
                    card.meta.insert("fence".into(), now);
                } else {
                    // The fence it named was not copied with it, so the copy is
                    // not in it. Measurement will say so on the next frame;
                    // this is only about not leaving a claim that outlives it.
                    card.meta.remove("fence");
                }
                card.id = renamed.get(&card.id).cloned().unwrap_or(card.id);
                card
            })
            .collect()
    }

    // -----------------------------------------------------------------------
    // Boards, menus and modes
    // -----------------------------------------------------------------------

    /// Read another board off the disk and show it.
    ///
    /// The picture cache is deliberately *kept*. It is keyed by content hash,
    /// so an entry in it is the decoded form of exactly those bytes no matter
    /// which board named them — and two boards in a project usually share
    /// photographs. Anything the new board does not want ages out of the cache
    /// on its own.
    /// Everything a drag on these cards should actually take hold of.
    ///
    /// [`kin`](Self::kin) with the locked taken out **before** the
    /// expansion, or a locked fence would hand its contents to a drag it is
    /// not itself taking part in — which is a locked group whose cards slide
    /// out of it. Not filtered again *after* the expansion: a lock says a
    /// card is not to be dragged on its own, not that it can resist the fence
    /// that holds it — a locked card in an unlocked fence is still carried
    /// when the fence moves, or the group would tear itself apart every time
    /// it travelled.
    ///
    /// Deliberately worked out once at the press rather than on every frame:
    /// the set has to stay the same for the length of the gesture, or a card
    /// sliding out of a fence mid-drag would stop moving halfway across the
    /// board.
    fn dragging(&self, ids: Vec<String>) -> Vec<String> {
        self.kin(self.movable(ids))
    }

    /// The set [`dragging`](Self::dragging) describes, less the locked cards
    /// even where they are only along for the ride: what binning asks is a
    /// different question from what dragging asks — a group somebody binned
    /// should not take a locked passenger down with it, where a group somebody
    /// *moved* should.
    fn unlocked_kin(&self, ids: Vec<String>) -> Vec<String> {
        self.movable(self.kin(self.movable(ids)))
    }

    /// These ids, less the ones the author has nailed down.
    ///
    /// One door for every gesture that moves something — the drag, the nudge,
    /// the aligns — so that a lock cannot be honoured in one of them and
    /// forgotten in the next.
    fn movable(&self, ids: Vec<String>) -> Vec<String> {
        ids.into_iter().filter(|id| !self.is_locked(id)).collect()
    }

    /// Whether this card is nailed down. See [`mbrd_core::Item::locked`].
    pub fn is_locked(&self, id: &str) -> bool {
        self.doc.board.item(id).is_some_and(Item::locked)
    }

    /// These cards, plus everything that travels with them.
    ///
    /// One rule, and it is what somebody would expect rather than what the
    /// data structure suggests: **a fence brings what is inside it.** A fence
    /// that moves and leaves its cards behind has not moved a grouping, it has
    /// torn one — and a fence *copied* without them is an empty rectangle,
    /// which is the bug this being shared between the drag and the copy is
    /// here to stop coming back.
    ///
    /// Not recursive, and it does not need to be: `contents` is already
    /// transitive, so a fence holding a fence holding a card yields all three
    /// in one pass.
    fn kin(&self, ids: Vec<String>) -> Vec<String> {
        let items = &self.doc.board.items;
        let fences = Fences::measure(items);

        let mut out: Vec<String> = Vec::with_capacity(ids.len());
        let push = |id: String, out: &mut Vec<String>| {
            if !out.contains(&id) {
                out.push(id);
            }
        };
        for id in ids {
            push(id, &mut out);
        }
        // A separate pass, so that a fence inside a fence is not added twice.
        let mut carried: Vec<String> = Vec::new();
        for id in &out {
            if self.doc.board.item(id).map(|i| &i.kind) == Some(&ItemType::Fence) {
                for held in fences.contents(id, items) {
                    carried.push(held.id.clone());
                }
            }
        }
        for id in carried {
            push(id, &mut out);
        }
        out
    }

    // -----------------------------------------------------------------------
    // Groups
    // -----------------------------------------------------------------------

    /// What pressing this card actually selects.
    ///
    /// **The whole of the group-first rule, in one function.** A card inside a
    /// fence is part of a thing somebody made on purpose, and pressing it means
    /// "that thing" rather than "this card" — so what comes back is the
    /// outermost fence holding it, not the card pressed.
    ///
    /// Except for the fences that have been stepped into, which are exactly the
    /// ones the author has said they want to work inside. Those drop out of the
    /// chain, and what is left is the outermost fence *below* where you are —
    /// so one step into a group selects its members, and one step into a nested
    /// group selects theirs.
    ///
    /// With nothing entered and no fences on the board this is the identity,
    /// which is the common case and costs a measurement to answer.
    fn selects(&mut self, id: &str) -> String {
        // Brought up to date and then read as a field, rather than through the
        // reference `fences()` hands back: that reference is borrowed from
        // `&mut self`, and `pick` wants `self.inside` at the same time. Two
        // disjoint field borrows are fine where a whole-`self` one is not.
        self.fences();
        Self::pick(&self.fences, &self.inside, id)
    }

    /// [`selects`](Self::selects), against a measurement already taken.
    ///
    /// The split exists for the sweep, which asks this of every card it caught:
    /// measuring the board once per card would be the size of the selection
    /// times the size of the board, for an answer that is the same every time.
    fn pick(fences: &Fences, inside: &[String], id: &str) -> String {
        if fences.is_empty() {
            return id.to_string();
        }
        // Innermost first, so the last one still standing is the outermost.
        fences
            .chain(id)
            .into_iter()
            .rfind(|up| !inside.iter().any(|open| open == up))
            .map(str::to_string)
            .unwrap_or_else(|| id.to_string())
    }

    /// The fence a press on this card would have to enter to reach it.
    ///
    /// `None` where the card is already reachable — either it is in no fence,
    /// or every fence holding it has been stepped into. That is also the test
    /// for whether a double-click should enter a group or open the card for
    /// typing, which is the one place the two gestures collide.
    fn enterable(&mut self, id: &str) -> Option<String> {
        let picked = self.selects(id);
        (picked != id).then_some(picked)
    }

    /// Step inside a fence, so that presses reach what is in it.
    fn enter_group(&mut self, fence: &str, cx: &mut Context<Self>) {
        if self.inside.iter().any(|open| open == fence) {
            return;
        }
        self.inside.push(fence.to_string());
        let name = self
            .doc
            .board
            .item(fence)
            .map(|it| it.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "group".into());
        self.say(format!("inside {name}"));
        cx.notify();
    }

    /// Step back out of the innermost fence, and select it.
    ///
    /// Answers whether there was anywhere to go, so that `Escape` can fall
    /// through to clearing the selection when there is not — leaving a group is
    /// the nearer of the two things that key means, in the same way that
    /// putting a menu away is nearer than either.
    ///
    /// The fence you came out of ends up selected rather than nothing, because
    /// stepping out is a statement about that group and the next thing you do
    /// is almost always to it.
    pub fn leave_group(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(left) = self.inside.pop() else { return false };
        if self.doc.board.item(&left).is_some() {
            self.select_only(&left);
        }
        self.say("left the group".into());
        cx.notify();
        true
    }

    /// Forget any group that is no longer on the board.
    ///
    /// Called wherever the selection is pruned, and for the same reason: a
    /// fence deleted or undone away while you were standing inside it would
    /// otherwise leave you inside a group that does not exist, where presses
    /// reach through a grouping nothing on screen can show you.
    fn prune_inside(&mut self) {
        let items = &self.doc.board.items;
        self.inside.retain(|id| items.iter().any(|it| &it.id == id));
    }

    /// Whether anything selected is a fence that could be dissolved.
    pub fn can_ungroup(&self) -> bool {
        self.selection
            .iter()
            .any(|id| self.doc.board.item(id).map(|it| &it.kind) == Some(&ItemType::Fence))
    }

    /// Take the fence away and leave what was in it.
    ///
    /// The cards do not move, and nothing needs to be written onto them:
    /// membership is measured from where things are, so removing the rectangle
    /// *is* dissolving the group. See `core::fence`, which is why this is four
    /// lines rather than a pass over a member list.
    ///
    /// What ends up selected is the contents, which is what Figma does and what
    /// somebody who just dissolved a group is about to want — otherwise the
    /// gesture ends with nothing in hand.
    pub fn ungroup(&mut self, cx: &mut Context<Self>) {
        let pens: Vec<String> = self
            .selection
            .iter()
            .filter(|id| self.doc.board.item(id).map(|it| &it.kind) == Some(&ItemType::Fence))
            .cloned()
            .collect();
        if pens.is_empty() {
            self.tell("nothing here is a group".into());
            cx.notify();
            return;
        }
        // Worked out before the fences go, because afterwards there is nothing
        // left to measure the membership against.
        let freed: Vec<String> = {
            let fences = Fences::measure(&self.doc.board.items);
            let items = &self.doc.board.items;
            let mut out: Vec<String> = Vec::new();
            for pen in &pens {
                for held in fences.contents(pen, items) {
                    if !pens.contains(&held.id) && !out.contains(&held.id) {
                        out.push(held.id.clone());
                    }
                }
            }
            out
        };
        let n = pens.len();
        let label = if n == 1 { "Ungroup".to_string() } else { format!("Ungroup {n}") };
        self.doc.board.edit(&label, |board| {
            board.items.retain(|it| !pens.contains(&it.id));
            // The stamp on anything that named one of them, cleared here as
            // well as at the file boundary: `SLACK` only rescues a card into a
            // fence the file named, and a name that outlives its fence is a
            // grouping that cannot be seen and cannot be undone.
            for item in board.items.iter_mut() {
                if item.fence().is_some_and(|f| pens.iter().any(|p| p == f)) {
                    item.meta.remove("fence");
                }
            }
        });
        self.inside.retain(|id| !pens.contains(id));
        self.selection = freed;
        self.say(match n {
            1 => "ungrouped".to_string(),
            n => format!("ungrouped {n}"),
        });
        cx.notify();
    }

    // -----------------------------------------------------------------------
    // Arranging and fencing
    // -----------------------------------------------------------------------

    /// Line up, space out, or push apart what is selected.
    ///
    /// One method for all nine, because all nine are the same shape: ask
    /// `core::align` where the cards should go and write the answer through
    /// the door. Nothing here decides anything, which is what keeps the
    /// deciding testable without a window.
    pub fn arrange(&mut self, what: Command, cx: &mut Context<Self>) {
        // Locked cards are left out of the measurement as well as out of the
        // move: an alignment that read a nailed-down card's edge and then
        // could not move it would line the others up on an edge nothing
        // explains.
        let picked: Vec<&Item> = self
            .selection
            .iter()
            .filter_map(|id| self.doc.board.item(id))
            .filter(|item| !item.locked())
            .collect();
        let (moves, label) = match what {
            Command::Align(edge) => (align::align(&picked, edge), what.label()),
            Command::Distribute(axis) => (align::distribute(&picked, axis), what.label()),
            Command::Separate => (
                align::separate(&picked, self.doc.board.settings.desktop.spacing.max(8.0)),
                what.label(),
            ),
            _ => return,
        };
        if moves.is_empty() {
            // Already arranged. Not a step that undoes to the same picture —
            // no step at all, which is what `align` returning nothing means.
            self.tell("already there".into());
            cx.notify();
            return;
        }
        let n = moves.len();
        // Where each card was, cloned rather than borrowed, so this does not
        // hold `self.doc.board` immutably across the edit two lines down —
        // see `Self::present_move`, which is what the difference is for.
        let before: HashMap<String, (f32, f32)> =
            picked.iter().map(|item| (item.id.clone(), (item.x, item.y))).collect();
        self.doc.board.edit(label, |board| {
            for m in &moves {
                if let Some(item) = board.item_mut(&m.id) {
                    item.x = m.x;
                    item.y = m.y;
                }
            }
        });
        for m in &moves {
            if let Some(&(ox, oy)) = before.get(&m.id) {
                self.present_move(&m.id, ox - m.x, oy - m.y);
            }
        }
        self.say(format!("moved {n}"));
        cx.notify();
    }

    /// Gives one card's *drawn* position somewhere to catch up from after its
    /// model position just moved — `dx`/`dy` are where it was minus where it
    /// now is, so a spring released there and sent home to zero eases the
    /// difference away instead of drawing it at once.
    ///
    /// Added to whatever offset the card is already carrying rather than
    /// replacing it, so a card mid-catch from one arrange that is caught by
    /// another before it settles bends into the new one rather than jumping
    /// to restart it — the same reasoning `Sprung::retarget` documents for a
    /// camera sent somewhere new mid-flight, worked by hand here because the
    /// model position itself is what moved, not just the target.
    fn present_move(&mut self, id: &str, dx: f32, dy: f32) {
        if dx == 0.0 && dy == 0.0 {
            return;
        }
        let (sx, sy) = self
            .presenting
            .entry(id.to_string())
            .or_insert_with(|| (Sprung::at(0.0), Sprung::at(0.0)));
        let mut x = Sprung::at(sx.value() + dx);
        x.retarget(0.0);
        let mut y = Sprung::at(sy.value() + dy);
        y.retarget(0.0);
        *sx = x;
        *sy = y;
    }

    /// One frame of every card easing its presentation back onto its model
    /// position — see `Self::present_move` for what put it there. Settled
    /// entries are dropped so a board nobody has arranged in an hour costs
    /// nothing here, the same rule `Self::presenting`'s own doc comment
    /// gives for why this is a map and not a field on every card.
    fn advance_presenting(&mut self, dt: f32) -> bool {
        if self.presenting.is_empty() {
            return false;
        }
        let rest = PRESENTING_REST / self.viewport.zoom.max(MIN_ZOOM);
        let mut moving = false;
        self.presenting.retain(|_, (sx, sy)| {
            // Both stepped unconditionally rather than short-circuited: an
            // `||` here would skip `sy`'s step on any frame `sx` had already
            // settled, leaving the y-axis spring frozen mid-catch.
            let a = sx.step(Spring::CAMERA, dt, rest);
            let b = sy.step(Spring::CAMERA, dt, rest);
            moving |= a || b;
            a || b
        });
        moving
    }

    /// Remember the picked arrangement and lay the whole board out in it.
    ///
    /// Picking a layout *applies* it rather than merely recording a
    /// preference, because the menu row is a verb to the person pressing it:
    /// "Masonry" means "make it masonry", not "next time somebody rearranges,
    /// masonry". The stored id is what [`Command::ticked`] reads back.
    pub fn set_arrangement(&mut self, name: Arrangement, cx: &mut Context<Self>) {
        self.lay_out(Some(name), false, cx);
    }

    /// Lay the board — or just the selection — out again, with a fresh seed.
    pub fn rearrange(&mut self, only_selection: bool, cx: &mut Context<Self>) {
        self.lay_out(None, only_selection, cx);
    }

    /// The whole-board relayout: ask `core::arrange` for one position per
    /// card and write the answer through the door as one step.
    ///
    /// The same shape as [`Self::arrange`] one floor up, with two rules
    /// carried over from the original's `rearrange()` because each guards a
    /// grouping somebody made by hand:
    ///
    /// - **A fenced card is not laid out**, when its fence is in the
    ///   set: the fence takes a slot at its own size and its contents keep
    ///   their places inside it. Laid out flat, an arrangement would deal
    ///   every fence a slot as though it were a card and scatter its cards to
    ///   slots of their own — and since membership is measured, one press of
    ///   Rearrange would take every grouping on the board apart.
    /// - **Two things vary, and neither is enough alone.** The shuffle
    ///   changes which card lands in which slot — without it a layout is a
    ///   pure function of the list and feeding it the same board twice puts
    ///   everything back where it was. The seed changes where the slots
    ///   *are* — without it the board comes back in the identical shape with
    ///   the cards swapped, which from any distance is the same picture.
    fn lay_out(&mut self, pick: Option<Arrangement>, only_selection: bool, cx: &mut Context<Self>) {
        let name = pick
            .or_else(|| Arrangement::parse(&self.doc.board.arrangements.desktop))
            .unwrap_or(Arrangement::Grid);
        let items = &self.doc.board.items;
        let fences = Fences::measure(items);

        // Furniture is left out and left alone: the title card and the hints
        // are exactly what a relayout is not about. It is also kept *off* —
        // the title card goes in as an obstacle, so no slot is dealt where it
        // stands. That is the original's anchoring reached without a lock:
        // its title arrives anchored, and this build has no anchors yet.
        let chosen: Vec<&Item> = items
            .iter()
            .filter(|i| !matches!(i.kind, ItemType::Title | ItemType::Ghost))
            // A locked card is furniture the author made: the layout leaves it
            // exactly where it stands, the same way it leaves the title card —
            // and so is everything inside a locked fence, or locking a group
            // would keep its rectangle still while its cards were dealt slots
            // out from under it.
            .filter(|i| !i.locked() && !fences.chain(&i.id).iter().any(|f| self.is_locked(f)))
            .filter(|i| !only_selection || self.selection.contains(&i.id))
            .collect();
        if chosen.is_empty() {
            self.tell("nothing to lay out".into());
            cx.notify();
            return;
        }
        let in_set: HashSet<&str> = chosen.iter().map(|i| i.id.as_str()).collect();
        let carried: Vec<&Item> = chosen
            .iter()
            .copied()
            .filter(|i| fences.owner_of(&i.id).is_some_and(|f| in_set.contains(f)))
            .collect();
        let carried_ids: HashSet<&str> = carried.iter().map(|i| i.id.as_str()).collect();
        let free: Vec<&Item> =
            chosen.iter().copied().filter(|i| !carried_ids.contains(i.id.as_str())).collect();
        if free.is_empty() {
            // Every card in the set was inside a fence that is also in it.
            // There is nothing left to deal a slot to.
            self.tell("nothing to lay out".into());
            cx.notify();
            return;
        }

        let settings = &self.doc.board.settings.desktop;
        let step = if settings.grid_step > 0.0 { settings.grid_step } else { 64.0 };
        let snapped = settings.snap;
        let spacing = settings.spacing;
        // Time is as fresh as a seed needs to be — this is "look different",
        // not cryptography — and the wrap to u32 keeps the whole clock.
        let seed = mbrd_core::naming::now_millis() as u32;

        // Which card lands in which slot. The layouts read the deal through
        // the same PRNG family they vary their slots by, one draw apart.
        let mut order: Vec<usize> = (0..free.len()).collect();
        let mut rng = arranging::Mulberry::new(seed ^ 0x9E37_79B9);
        arranging::shuffle(&mut order, &mut rng);

        // On a snapped board a rearrangement is a *re-lay*, sizes included:
        // placing cards on the lattice and leaving them at 320x240 is the
        // thing snapping is for and does not do. Sized before the layout runs
        // rather than after, because the arrangements read each card's `w`
        // and `h` to decide how much room its slot needs. A fence keeps the
        // size it was drawn at — rounding it to cells could pull an edge in
        // past a card whose centre sat within half a cell of it, dropping the
        // card out of the fence on a gesture that is not about membership.
        let laid: Vec<Item> = order
            .iter()
            .map(|&i| {
                let mut c = free[i].clone();
                if snapped && c.kind != ItemType::Fence {
                    c.w = geometry::clamp_size(geometry::snap(c.w, step));
                    c.h = geometry::clamp_size(geometry::snap(c.h, step));
                }
                c
            })
            .collect();

        // The whole board rebuilds about the origin; a selection rebuilds
        // where it already is.
        let center = if only_selection {
            geometry::union(free.iter().copied())
                .map(|r| r.centre())
                .unwrap_or_else(|| point(0.0, 0.0))
        } else {
            point(0.0, 0.0)
        };
        // What a slot may not be dealt over. The title card, and everything
        // the author has locked — both are things that stay where they are, so
        // both are places the layout has to work around rather than through.
        let obstacles: Vec<Rect> = items
            .iter()
            .filter(|i| i.kind == ItemType::Title || i.locked())
            .map(Rect::of_item)
            .collect();

        let refs: Vec<&Item> = laid.iter().collect();
        let spots = arranging::arrange(
            &refs,
            name,
            &arranging::Opts {
                center,
                spacing,
                // Snapping reserves whole cells so the per-card snap below
                // cannot round two tight cards into an overlap — see
                // `arrange::to_cells`.
                cell_step: if snapped { step } else { 0.0 },
                seed: Some(seed),
                obstacles,
            },
        );

        // Everything that moves, as one owned plan, so nothing borrows the
        // board across the edit: id, where it lands, and the size it was
        // laid out at where the lattice resized it.
        let before: HashMap<String, (f32, f32)> =
            chosen.iter().map(|i| (i.id.clone(), (i.x, i.y))).collect();
        let mut plan: HashMap<String, (f32, f32)> = HashMap::new();
        let mut resized: Vec<(String, f32, f32)> = Vec::new();
        for (slot, &item_index) in order.iter().enumerate() {
            let mut p = spots[slot];
            if snapped {
                // The spots came back clear even for centres each up to half
                // a step from the lattice — that is what the whole-cell
                // reservation above bought.
                p.x = geometry::snap(p.x, step);
                p.y = geometry::snap(p.y, step);
            }
            let it = free[item_index];
            plan.insert(it.id.clone(), (p.x, p.y));
            if snapped && it.kind != ItemType::Fence {
                resized.push((it.id.clone(), laid[slot].w, laid[slot].h));
            }
        }

        // Fences are at their new slots; carry their contents by the same
        // translation, which is what keeps a region a region. In passes,
        // because a fence inside a fence is carried too, and can only carry
        // its own contents once it has been carried itself.
        let mut pending: Vec<&Item> = carried.clone();
        while !pending.is_empty() {
            let mut next: Vec<&Item> = Vec::new();
            let mut grew = false;
            for it in pending {
                let fence_id = fences.owner_of(&it.id).unwrap_or("");
                let (Some(&(fx1, fy1)), Some(&(fx0, fy0))) =
                    (plan.get(fence_id), before.get(fence_id))
                else {
                    next.push(it);
                    continue;
                };
                plan.insert(it.id.clone(), (it.x + fx1 - fx0, it.y + fy1 - fy0));
                grew = true;
            }
            if !grew {
                // A fence that never resolved — its own fence left the plan
                // somehow. Leave the stragglers where they stand rather than
                // loop forever; a card that does not move is a smaller wrong
                // than a hang.
                break;
            }
            pending = next;
        }

        let n = plan.len();
        drop(chosen);
        self.doc.board.edit("Rearrange", |board| {
            if let Some(a) = pick {
                board.arrangements.desktop = a.as_str().into();
            }
            for (id, w, h) in &resized {
                if let Some(item) = board.item_mut(id) {
                    item.w = *w;
                    item.h = *h;
                }
            }
            for (id, (x, y)) in &plan {
                if let Some(item) = board.item_mut(id) {
                    item.x = *x;
                    item.y = *y;
                }
            }
            // A card the engine just placed has been placed on purpose: its
            // memory of where it sat before the lattice took it is describing
            // a board that no longer exists, and releasing snap later must
            // not scatter the fresh layout back to the old one.
            if snapped {
                for g in board.layouts.desktop.iter_mut() {
                    if plan.contains_key(&g.id) {
                        g.presnap = None;
                    }
                }
            }
        });
        for (id, (x, y)) in &plan {
            if let Some(&(ox, oy)) = before.get(id) {
                self.present_move(id, ox - x, oy - y);
            }
        }
        if !only_selection {
            self.fit_all(cx);
        }
        self.say(match n {
            1 => "rearranged 1 card".to_string(),
            n => format!("rearranged {n} cards"),
        });
        cx.notify();
    }

    /// Put a fence around what is selected.
    ///
    /// Nothing is recorded about which cards are in it, because nothing needs
    /// to be: membership is measured from where the cards are, so drawing the
    /// rectangle *is* making the group. See `core::fence`.
    pub fn add_fence(&mut self, cx: &mut Context<Self>) {
        let around = mbrd_core::geometry::union(
            self.selection.iter().filter_map(|id| self.doc.board.item(id)),
        );
        let box_ = match around {
            // Room around the cards, so the fence reads as holding them rather
            // than as being exactly as big as them.
            Some(r) => r.inflate(36.0),
            // Nothing selected: one the size of a card, in the middle of the
            // window, for somebody who wants to fence off a space first and
            // fill it after.
            None => Rect::centred(self.viewport.pan.x, self.viewport.pan.y, 520.0, 380.0),
        };
        let id = self.fresh_id();
        let mut pen = Item::new(id.clone(), ItemType::Fence);
        pen.x = box_.centre().x;
        pen.y = box_.centre().y;
        pen.w = box_.width();
        pen.h = box_.height();
        pen.name = "group".into();
        // Behind everything, because a fence is a thing the board is drawn on
        // rather than a thing on the board.
        pen.z =
            self.doc.board.items.iter().map(|i| i.z).fold(f32::INFINITY, f32::min).min(0.0) - 1.0;
        self.doc.board.edit("Add fence", |board| board.items.push(pen));
        let held =
            Fences::measure(&self.doc.board.items).contents(&id, &self.doc.board.items).len();
        self.select_only(&id);
        self.say(match held {
            0 => "fence added".into(),
            1 => "fence added, holding one".into(),
            n => format!("fence added, holding {n}"),
        });
        cx.notify();
    }

    /// What the Lock row's tick should show: whether *every* selected card is
    /// locked. `None` with nothing selected, so the row does not tick over a
    /// selection it would not act on.
    pub fn lock_state(&self) -> Option<bool> {
        if self.selection.is_empty() {
            return None;
        }
        Some(self.selection.iter().all(|id| self.is_locked(id)))
    }

    /// Nail the selected cards down, or let them go.
    ///
    /// A mixed selection goes to locked, the way every all-or-nothing toggle
    /// in this table resolves ambiguity: the second press then means the other
    /// thing for all of them.
    ///
    /// Not through `kin`. Locking a fence locks the *rectangle* — what is
    /// inside it is a measurement rather than a membership somebody typed, and
    /// a lock that silently reached six other cards would be a lock nobody
    /// could undo by looking at what they selected.
    pub fn toggle_lock(&mut self, cx: &mut Context<Self>) {
        if self.selection.is_empty() {
            return;
        }
        let on = !self.lock_state().unwrap_or(false);
        let ids = self.selection.clone();
        let n = ids.len();
        self.doc.board.edit(if on { "Lock" } else { "Unlock" }, |board| {
            for id in &ids {
                if let Some(item) = board.item_mut(id) {
                    if on {
                        item.meta.insert("locked".into(), serde_json::Value::Bool(true));
                    } else {
                        // Removed rather than set to `false`, so a board that
                        // has nothing locked says nothing about locking.
                        item.meta.remove("locked");
                    }
                }
            }
        });
        self.say(match (on, n) {
            (true, 1) => "locked".into(),
            (true, n) => format!("locked {n}"),
            (false, 1) => "unlocked".into(),
            (false, n) => format!("unlocked {n}"),
        });
        cx.notify();
    }

    /// Whether the one selected card's words keep their size as the board
    /// moves under them, or `None` where the question does not apply.
    ///
    /// Phrased the way the menu row is — [`Command::DontScaleText`] — rather
    /// than the way the stored flag is, so that the tick and the label cannot
    /// drift apart. The flag records the exception; this reports the rule.
    ///
    /// `None` is what makes the row dim rather than tick: the setting is about
    /// words, so a card with none — a photograph, a swatch — has no answer to
    /// give, and neither does a selection of several, since they could
    /// disagree and a tick that meant "some of them" would be a tick that
    /// meant nothing.
    pub fn text_unscaled(&self) -> Option<bool> {
        let [id] = &self.selection[..] else { return None };
        let item = self.doc.board.item(id)?;
        matches!(item.kind, ItemType::Note | ItemType::Text).then(|| !scales_text(item))
    }

    /// Turn that setting over.
    pub fn toggle_text_scaling(&mut self, cx: &mut Context<Self>) {
        let Some(unscaled) = self.text_unscaled() else { return };
        let Some(id) = self.selection.first().cloned() else { return };
        self.doc.board.edit("Scale text", |board| {
            if let Some(item) = board.item_mut(&id) {
                match unscaled {
                    // Turning the row *off*. Taken out rather than written as
                    // `true`, so a card back at the default carries nothing
                    // about it — the file says what somebody chose, not what
                    // they did not.
                    true => {
                        item.meta.remove(SCALE_TEXT);
                    }
                    // And turning it on writes the exception, which is the
                    // only thing on this axis worth a key in the file.
                    false => {
                        item.meta.insert(SCALE_TEXT.into(), serde_json::Value::Bool(false));
                    }
                }
            }
        });
        self.say(match unscaled {
            true => "text now grows and shrinks with the card".into(),
            false => "text now stays the same size however far you zoom".into(),
        });
        cx.notify();
    }

    /// Whether the one selected card is a note whose height follows its words,
    /// or `None` where the row does not apply at all.
    ///
    /// The same shape as [`Self::text_unscaled`] and for the same reason: the
    /// menu asks one question and gets back "not here", "off" or "on".
    pub fn text_fitted(&self) -> Option<bool> {
        let [id] = &self.selection[..] else { return None };
        let item = self.doc.board.item(id)?;
        matches!(item.kind, ItemType::Note | ItemType::Text).then(|| fits_text(item))
    }

    /// Turn that setting over.
    ///
    /// Turning it *on* re-measures immediately rather than waiting for the next
    /// keystroke. A switch whose effect only shows up once you type into the
    /// card is a switch nobody can tell they pressed.
    pub fn toggle_fit_text(&mut self, cx: &mut Context<Self>) {
        let Some(fitted) = self.text_fitted() else { return };
        let Some(id) = self.selection.first().cloned() else { return };
        let measure = self.measure.clone();
        self.doc.board.edit("Dynamic size", |board| {
            if let Some(item) = board.item_mut(&id) {
                match fitted {
                    // Off. Taken out rather than written as `false`, so a card
                    // back at the default carries nothing about it — the file
                    // says what somebody chose, not what they did not. The
                    // height it had stays: the card keeps the shape it was last
                    // measured into rather than jumping back to whatever it was
                    // before, which is nothing anybody asked for.
                    true => {
                        item.meta.remove(FIT_TEXT);
                    }
                    false => {
                        item.meta.insert(FIT_TEXT.into(), serde_json::Value::Bool(true));
                        refit(item, &measure);
                    }
                }
            }
        });
        self.say(match fitted {
            true => "this note keeps the size you give it".into(),
            false => "this note now grows to fit what is written on it".into(),
        });
        cx.notify();
    }

    /// The one selected card, if it is a mesh — what `Command::Position`'s
    /// row asks, and what `Self::toggle_positioning` acts on.
    ///
    /// `None` for anything else selected, nothing selected, or more than one
    /// card at once: Position mode is one card's camera, not a setting
    /// several could disagree about — the same shape `text_unscaled` answers
    /// for its own row, and for the same reason.
    pub fn positionable(&self) -> Option<String> {
        let [id] = &self.selection[..] else { return None };
        let item = self.doc.board.item(id)?;
        (mbrd_core::preview::of(item, self.asset_of(item)) == mbrd_core::preview::Preview::Mesh)
            .then(|| id.clone())
    }

    /// Turn Position mode on for the one selected mesh, or off if it is
    /// already the card in it.
    pub fn toggle_positioning(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.positionable() else { return };
        self.positioning = match self.positioning.as_deref() == Some(id.as_str()) {
            true => None,
            false => Some(id),
        };
        cx.notify();
    }

    // -----------------------------------------------------------------------
    // Ropes
    // -----------------------------------------------------------------------

    /// How the selected connection is dressed, for a menu to read.
    pub fn rope_meta(&self) -> Option<&ConnMeta> {
        let (a, b) = self.rope.as_ref()?;
        rope::between(&self.doc.board, a, b).map(|c| &c.meta)
    }

    /// Select a connection, and nothing else.
    ///
    /// A rope and a card are not the same kind of thing to have selected, and
    /// the commands that apply to one apply to none of the other's — so at most
    /// one of the two is ever live, and this is the half that says so.
    fn select_rope(&mut self, a: &str, b: &str, cx: &mut Context<Self>) {
        self.selection.clear();
        self.rope = Some((a.to_string(), b.to_string()));
        self.say("connection selected — right-click for its look".into());
        cx.notify();
    }

    /// Take the selected connection off the board. The cards stay.
    pub fn delete_rope(&mut self, cx: &mut Context<Self>) {
        let Some((a, b)) = self.rope.take() else { return };
        let gone = self.doc.board.edit("Disconnect", |board| rope::part(board, &a, &b));
        if gone {
            self.say("disconnected".into());
        }
        cx.notify();
    }

    /// Change how the selected connection is drawn.
    ///
    /// One door for all four axes — colour, arrow, style, weight — because the
    /// only thing that differs between them is which field of `meta` is
    /// written, and taking that as a closure means a fifth axis the format
    /// grows is a line in `command.rs` and nothing here.
    pub fn dress(
        &mut self,
        label: &str,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut ConnMeta),
    ) {
        let Some((a, b)) = self.rope.clone() else { return };
        let changed = self.doc.board.edit(label, |board| match rope::between_mut(board, &a, &b) {
            Some(conn) => {
                change(&mut conn.meta);
                true
            }
            None => false,
        });
        if changed {
            self.say(label.to_lowercase());
        }
        cx.notify();
    }

    /// Move a connection's label along its line, inside an open gesture.
    ///
    /// One step for the whole drag, like every other gesture that writes to the
    /// board: forty frames of sliding is one thing to take back.
    fn slide_label(&mut self, a: &str, b: &str, along: f32) {
        let Gesture::Sliding { open, .. } = &self.gesture else { return };
        let open = open.clone();
        let (a, b) = (a.to_string(), b.to_string());
        let along = along.clamp(0.0, 1.0);
        self.doc.board.during(&open, |board| {
            if let Some(conn) = rope::between_mut(board, &a, &b) {
                conn.meta.label_at = along;
            }
        });
    }

    /// Join two cards, if they are not already joined.
    fn join(&mut self, a: &str, b: &str, cx: &mut Context<Self>) {
        let made = self.doc.board.edit("Connect", |board| rope::join(board, a, b));
        if made {
            self.select_rope(a, b, cx);
            self.say("connected".into());
        } else if rope::between(&self.doc.board, a, b).is_some() {
            // Already joined. Selecting the rope that is there is a better
            // answer than doing nothing, because it is what somebody drawing
            // the same line twice is reaching for.
            self.select_rope(a, b, cx);
        }
        cx.notify();
    }

    /// The connection under the pointer, if any.
    ///
    /// Measured against the lines as they were last drawn, which is the only
    /// honest way to do it: a rope that bends round a card has to be pressable
    /// where it *is*, not where a straight line between the two cards would
    /// have been.
    fn rope_at(&self, world: WorldPoint) -> Option<(String, String)> {
        let reach = ROPE_REACH / self.viewport.zoom.max(0.0001);
        self.drawn.iter().rev().find(|w| w.near(world, reach)).map(|w| (w.a.clone(), w.b.clone()))
    }

    /// The connection whose label is under the pointer, if any.
    ///
    /// The chip's width is **added up** rather than shaped: shaping is the
    /// expensive thing and this is asked on every mouse move, but the width of
    /// each character is a cached lookup — see `metrics.rs`. That is not quite
    /// a shaping, since it takes no account of kerning or of ligatures, so
    /// being a pixel out at either end costs a press that lands on the line
    /// running under the chip instead, which selects the connection — the
    /// near-miss answer rather than a wrong one.
    fn label_at(&self, world: WorldPoint) -> Option<(String, String)> {
        let zoom = self.viewport.zoom;
        if zoom <= LABEL_ZOOM {
            return None;
        }
        self.drawn.iter().rev().find_map(|w| {
            let text = w.meta.label.as_ref().filter(|t| !t.is_empty())?;
            // A chip is a fixed size on screen sitting on a board measured in
            // world units, so how far it reaches in world units grows as the
            // board is zoomed out. Same arithmetic as `rope_at`'s.
            let wide = (self.measure.width(text, LABEL_TEXT) + LABEL_PAD * 2.0) / zoom;
            let tall = LABEL_TEXT * LABEL_LEADING / zoom;
            let at = w.label_spot();
            // A few screen pixels of grace on top of the estimate, in world
            // units so it is the same few pixels at every zoom. `wide` is
            // already an estimate rather than a measurement, and a press that
            // falls just short of it used to fall through to the line running
            // underneath the chip — selecting the connection instead of
            // grabbing the label it looked like it was aimed at. The near
            // miss this trades it for is the label winning a press meant for
            // the line a few pixels further out, which is the one of the two
            // a chip sitting on top of its own line should win.
            let pad = 4.0 / zoom;
            ((world.x - at.x).abs() <= wide / 2.0 + pad
                && (world.y - at.y).abs() <= tall / 2.0 + pad)
                .then(|| (w.a.clone(), w.b.clone()))
        })
    }

    /// The anchor under the pointer, and the card offering it.
    ///
    /// Only the hovered card and the selected ones, which is the same rule the
    /// painter applies: something you cannot see must not be something you can
    /// press.
    fn anchor_at(&mut self, at: gpui::Point<Pixels>) -> Option<(String, Side)> {
        if self.hovering.is_none() && self.selection.is_empty() {
            return None;
        }
        let local = point(
            f(at.x) - f(self.canvas_bounds.origin.x),
            f(at.y) - f(self.canvas_bounds.origin.y),
        );
        // Only a card under or near the pointer can answer, so the candidates
        // come off the index rather than off the selection: this runs every
        // frame, and walking the selection with a `Board::item` scan inside it
        // made Ctrl A cost selection times cards. The order stays the old
        // order — the hovered card first, then the selection as it stands —
        // so which of two overlapping anchors wins does not change.
        let world = self.viewport.to_world(local);
        // A mark sits `GAP` outside its card's edge and answers `REACH` past
        // that, so that is how far away a card owning the mark under the
        // pointer can be.
        let reach = (anchor::GAP + anchor::REACH) / self.viewport.zoom.max(0.0001);
        let mut found = Vec::new();
        self.index().in_rect(
            Rect::new(world.x - reach, world.y - reach, world.x + reach, world.y + reach),
            &mut found,
        );
        if found.is_empty() {
            return None;
        }
        let items = &self.doc.board.items;
        let near: Vec<&Item> = found.iter().map(|&i| &items[i as usize]).collect();
        let test = |id: &str| -> Option<(String, Side)> {
            let item = near.iter().find(|it| it.id == id)?;
            anchor::at(local, Rect::of_item(item), &self.viewport)
                .map(|side| (id.to_string(), side))
        };
        if let Some(id) = self.hovering.as_deref() {
            if let Some(hit) = test(id) {
                return Some(hit);
            }
        }
        for id in &self.selection {
            if Some(id) == self.hovering.as_ref() {
                continue;
            }
            if let Some(hit) = test(id) {
                return Some(hit);
            }
        }
        None
    }

    /// Which control the pointer is over, measured against what was drawn.
    ///
    /// Reversed, so the topmost of two overlapping cards wins — the same order
    /// the painter draws them in, and the same rule `grip_at` follows.
    fn controls_at(&self, at: gpui::Point<Pixels>) -> Option<(String, transport::Hit)> {
        let local = point(
            f(at.x) - f(self.canvas_bounds.origin.x),
            f(at.y) - f(self.canvas_bounds.origin.y),
        );
        self.drawn_controls.iter().rev().find_map(|drawn| {
            transport::at(local, &drawn.strip, drawn.volume).map(|hit| (drawn.id.clone(), hit))
        })
    }

    /// What one card's controls know about themselves, as last drawn.
    fn drawn_control(&self, id: &str) -> Option<&Drawn> {
        self.drawn_controls.iter().find(|drawn| drawn.id == id)
    }

    /// Whether the open volume slider still counts as being reached for.
    ///
    /// The `transport::reaching` version of `still_reaching` above: the
    /// slider sits `GAP` above the mute button that opened it, and asking
    /// `controls_at` alone drops it the instant the pointer crosses that gap
    /// on the way to the slider — which is the one motion someone reaching for
    /// it is guaranteed to make.
    fn still_reaching_volume(&self, at: gpui::Point<Pixels>) -> Option<String> {
        let id = self.volume_on.clone()?;
        let drawn = self.drawn_control(&id)?;
        let (mute, slider) = (drawn.strip.mute?, drawn.volume?);
        let local = point(
            f(at.x) - f(self.canvas_bounds.origin.x),
            f(at.y) - f(self.canvas_bounds.origin.y),
        );
        transport::reaching(local, mute, slider).then_some(id)
    }

    /// Act on a press that landed on the strip.
    fn press_control(&mut self, id: &str, hit: transport::Hit, cx: &mut Context<Self>) {
        let Some(drawn) = self.drawn_control(id) else { return };
        let (length, looping, sound, moves) =
            (drawn.length, drawn.looping, drawn.sound, drawn.moves);

        match hit {
            transport::Hit::PlayPause => {
                let playing = self.media.is_playing(id);
                // **The decision goes to the board, not just to the playhead.**
                // Stopping a clip is somebody saying "not this one", and a
                // board where three were stopped and one left running should
                // open that way tomorrow. See `media::set_wants_to_play`.
                self.set_media_flag(id, if playing { "Pause" } else { "Play" }, cx, |item| {
                    mbrd_core::media::set_wants_to_play(item, !playing);
                });

                match playing {
                    true => self.media.pause(id),
                    false => {
                        // Overlapping sound on a moodboard is noise, so
                        // starting one recording stops the others.
                        if sound {
                            self.media.pause_others(id);
                        }
                        // Only where there is something to advance. A playhead
                        // running over a still poster would be a scrubber that
                        // moves across a picture that does not, and a board
                        // that never goes idle again.
                        if moves {
                            self.media.play(id, length, looping);
                        } else {
                            self.tell("that needs a video decoder, which is not here yet".into());
                        }
                    }
                }
            }
            transport::Hit::Scrub(fraction) => {
                self.media.seek(id, fraction, length);
                self.gesture = Gesture::Scrubbing { id: id.to_string() };
            }
            transport::Hit::Mute => {
                let muted =
                    self.doc.board.item(id).map(mbrd_core::media::playback).map(|p| p.muted);
                let Some(muted) = muted else { return };
                self.set_media_flag(id, if muted { "Unmute" } else { "Mute" }, cx, |item| {
                    mbrd_core::media::set_muted(item, !muted);
                });
            }
            transport::Hit::Looping => {
                self.set_media_flag(id, "Loop", cx, |item| {
                    mbrd_core::media::set_looping(item, !looping);
                });
            }
            transport::Hit::Volume(level) => {
                // One step for the whole drag, opened here and closed at the
                // release — the same shape a resize uses, and for the same
                // reason: forty steps for one movement of one slider is an undo
                // history nobody can walk back through.
                let open = self.doc.board.start();
                self.drag_volume(id, level);
                self.gesture = Gesture::Louder { id: id.to_string(), open };
            }
        }
        cx.notify();
    }

    /// Write one playback decision, as one step.
    fn set_media_flag(
        &mut self,
        id: &str,
        label: &str,
        cx: &mut Context<Self>,
        change: impl FnOnce(&mut Item),
    ) {
        self.doc.board.edit(label, |board| {
            if let Some(item) = board.item_mut(id) {
                change(item);
            }
        });
        cx.notify();
    }

    /// Whether the selection holds something with a play button.
    ///
    /// What gates [`Command::PlayPause`] and [`Command::ToggleMute`] — see
    /// `command.rs` — so the two show up dimmed rather than doing nothing on
    /// a board with three notes selected.
    pub fn has_media_selected(&self) -> bool {
        self.selection.iter().any(|id| {
            matches!(
                self.doc.board.item(id).map(|item| &item.kind),
                Some(ItemType::Video) | Some(ItemType::Audio)
            )
        })
    }

    /// `Space`. Play or pause every selected card with a play button.
    ///
    /// Play, pause and mute used to be mouse-only and hover-only — see
    /// `press_control` and `controls_at` — which put them out of reach of
    /// anybody driving this app from a keyboard, on cards that exist
    /// specifically to be played. This goes through [`Self::press_control`]
    /// itself rather than a shortcut around it, so a keystroke changes the
    /// board and the undo strip exactly the way a click already does — one
    /// [`Self::set_media_flag`] step per card, same label, same history.
    ///
    /// Skips a card this window has never drawn a strip for — off the visible
    /// area, say — the same as a click would: there is nothing recorded to
    /// press. Several selected at once each get their own press, which is
    /// what makes this "pause everything" as well as "pause this one".
    pub fn play_pause_selection(&mut self, cx: &mut Context<Self>) {
        for id in self.selection.clone() {
            if self.drawn_control(&id).is_some() {
                self.press_control(&id, transport::Hit::PlayPause, cx);
            }
        }
    }

    /// Mute or unmute every selected card with a mute button.
    ///
    /// No key of its own — see `Command::ToggleMute`'s hint — because the
    /// letters worth spending on a mute button are gone and this one is
    /// reached through the palette or the card menu instead. Otherwise the
    /// same shape as [`Self::play_pause_selection`], for the same reason:
    /// [`Self::press_control`] is the one door every way of pressing a
    /// control goes through.
    pub fn toggle_mute_selection(&mut self, cx: &mut Context<Self>) {
        for id in self.selection.clone() {
            if self.drawn_control(&id).is_some() {
                self.press_control(&id, transport::Hit::Mute, cx);
            }
        }
    }

    /// Move a card's volume slider inside an open gesture.
    fn drag_volume(&mut self, id: &str, level: f32) {
        let Gesture::Louder { open, .. } = &self.gesture else {
            // The first call comes from the press, before the gesture exists.
            self.doc.board.edit("Volume", |board| {
                if let Some(item) = board.item_mut(id) {
                    mbrd_core::media::set_volume(item, level);
                }
            });
            return;
        };
        let open = open.clone();
        let id = id.to_string();
        self.doc.board.during(&open, |board| {
            if let Some(item) = board.item_mut(&id) {
                mbrd_core::media::set_volume(item, level);
            }
        });
    }

    /// Open a mesh's orbiting gesture at the press. Shared by the board's own
    /// `on_mouse_down` — gated on `positioning` — and the opened page, where
    /// a press on the picture always means this.
    ///
    /// `panning` is Shift held at the press: a turn of the camera by default,
    /// a shift of its look-at point instead when held — the same "the drag
    /// happens on the same picture either way, a modifier says which it
    /// means" shape `Gesture::Moving`'s own axis lock is built out of.
    pub(crate) fn begin_mesh_orbit(
        &mut self,
        id: &str,
        position: gpui::Point<Pixels>,
        panning: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self.doc.board.item(id) else { return };
        let start = mbrd_core::media::orbit(item);
        let open = self.doc.board.start();
        self.gesture = Gesture::Orbiting {
            id: id.to_string(),
            from: position,
            start,
            panning,
            moved: false,
            open,
        };
        cx.notify();
    }

    /// Zoom a mesh's camera by one wheel notch. The opened page and the
    /// board's own Position-gated `on_scroll` both land here.
    pub(crate) fn dolly_orbit(&mut self, id: &str, factor: f32, cx: &mut Context<Self>) {
        self.doc.board.edit("Zoom", |board| {
            if let Some(item) = board.item_mut(id) {
                let next = mbrd_core::media::orbit(item).dollied(factor);
                mbrd_core::media::set_orbit(item, next);
            }
        });
        self.meshes.forget(id);
        self.begin_mesh_decode(id, cx);
    }

    /// The opened page's scroll wheel, which always dollies the one mesh it
    /// is showing — the same wheel-to-factor arithmetic `on_scroll` uses for
    /// the board's Position-gated branch, kept in one place since both are
    /// this same "one notch, one small zoom" decision.
    pub(crate) fn dolly_mesh(
        &mut self,
        id: &str,
        event: &ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) {
        let dy = match event.delta {
            ScrollDelta::Pixels(p) => f(p.y) / 40.0,
            ScrollDelta::Lines(p) => p.y,
        };
        let factor = (1.0 + ZOOM_PER_LINE).powf(-dy);
        self.dolly_orbit(id, factor, cx);
    }

    /// Rasterise a mesh's newest, still-being-dragged orbit onto `live` —
    /// never onto `resting`, which is only for a *released* orbit. See
    /// `live.rs` and `mesh_cache.rs`'s own module docs for why the two are
    /// kept apart.
    ///
    /// Sharp where the opened page has this card up, thumb otherwise — the
    /// same two tiers `resting` holds, so swapping from one to the other at
    /// the end of the drag is not a jump in size.
    fn live_orbit_frame(
        &mut self,
        id: &str,
        orbit: mbrd_core::media::Orbit,
        cx: &mut Context<Self>,
    ) {
        let Some(item) = self.doc.board.item(id) else { return };
        let Some(hash) = item.asset.as_ref().and_then(ItemAsset::hash) else { return };
        let Some(mesh) = self.meshes.parsed(hash) else {
            // Not parsed yet — the first decode this card ever asked for is
            // still in flight, or has not been asked for. Nothing to turn
            // yet; the resting picture, once it lands, is where this orbit
            // will show up.
            return;
        };
        let sharp = matches!(&self.overlay, Overlay::Opened(opened) if opened.id.as_str() == id);
        let id = id.to_string();
        let task = cx
            .background_executor()
            .spawn(async move { crate::mesh_cache::rasterize_tiers(&mesh, orbit) });
        cx.spawn(async move |view, cx| {
            let decoded = task.await;
            view.update(cx, |view, cx| {
                if let Some(decoded) = decoded {
                    let frame =
                        if sharp { decoded.sharp.unwrap_or(decoded.thumb) } else { decoded.thumb };
                    view.live.put(&id, frame);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The hovered card, if the pointer has only got as far as its marks.
    ///
    /// The hover's grace band, and the reason it is only ever about the card
    /// already hovered: crossing the band of a card you have never been on
    /// should not summon marks out of nothing, but crossing it on the way out
    /// of a card should not take them away either.
    fn still_reaching(&self, at: gpui::Point<Pixels>) -> Option<String> {
        let id = self.hovering.clone()?;
        let item = self.doc.board.item(&id)?;
        let local = point(
            f(at.x) - f(self.canvas_bounds.origin.x),
            f(at.y) - f(self.canvas_bounds.origin.y),
        );
        anchor::reaching(local, Rect::of_item(item), &self.viewport).then_some(id)
    }

    /// Change what a press on the board means.
    pub fn choose_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        if self.tool == tool {
            return;
        }
        self.tool = tool;
        // A standing note rather than something that just happened: while you
        // are in a tool, what the tool does is true, and a line that timed out
        // would leave a mode running with nothing on screen to say so.
        self.hint(tool.hint_line().map(str::to_string));
        cx.notify();
    }

    /// Join everything selected with the fewest lines that reach all of it.
    ///
    /// A minimum spanning tree rather than a chain in selection order: the
    /// order cards were clicked in is not a fact about the board, and
    /// band-selecting six of them would otherwise produce a shape nobody chose.
    pub fn connect_selection(&mut self, cx: &mut Context<Self>) {
        let ids: Vec<String> = self.selection.clone();
        if ids.len() < 2 {
            return;
        }
        let boxes: Vec<Rect> =
            ids.iter().filter_map(|id| self.doc.board.item(id).map(Rect::of_item)).collect();
        if boxes.len() != ids.len() {
            return;
        }
        let links = mbrd_core::route::spanning(&boxes);
        let made = self.doc.board.edit("Connect", |board| {
            links.iter().filter(|l| rope::join(board, &ids[l.a], &ids[l.b])).count()
        });
        self.say(match made {
            0 => "already connected".into(),
            1 => "connected".into(),
            n => format!("connected {n}"),
        });
        cx.notify();
    }

    /// Open a board, off the thread that draws.
    ///
    /// **Nothing about the board that is open is given up until the new one is
    /// in hand.** Reading a `.mbrd` means inflating every entry and hashing
    /// every asset to check it against its own name, which on a board of
    /// photographs is a second or more — and this used to happen between two
    /// frames, after tearing the old board down, so the whole of that second
    /// was a window that had stopped answering with nothing on it. Now the old
    /// board stays on screen and stays usable, [`Opening`] says how far the new
    /// one has got, and the swap happens in one frame when it lands.
    ///
    /// Asking for a second board part-way through does not queue: the token
    /// moves on and the first read lands nowhere. Somebody who has changed
    /// their mind is waiting for the answer they asked for second.
    pub fn open_board(&mut self, path: &Path, cx: &mut Context<Self>) {
        self.opens = self.opens.wrapping_add(1);
        let token = self.opens;
        self.opening = Some(Opening { token, name: short_name(path), done: 0, total: 0 });
        // Not reset to 0.0: a second open asked for while the first one's
        // panel is still fading out — rare, but a fast double `Ctrl P` does
        // it — retargets whatever presence is already there rather than
        // restarting the fade, the same rule `open_overlay` follows.
        self.opening_leaving = false;

        // Progress arrives on the background thread and has to cross back to
        // the one that draws. A channel rather than a shared counter, so the
        // view is only ever written from its own thread — the same shape the
        // download uses, and for the same reason.
        let (progress, updates) = std::sync::mpsc::channel::<(u64, u64)>();
        let target = path.to_path_buf();
        let reading = {
            let target = target.clone();
            cx.background_executor().spawn(async move {
                crate::save::read_watched(&target, |done, total| {
                    let _ = progress.send((done, total));
                })
            })
        };

        cx.spawn(async move |view, cx| {
            use std::sync::mpsc::TryRecvError;

            // Drain to the newest reading rather than notifying per entry: a
            // board of small files hands one over every few hundred
            // microseconds, and a repaint each would cost more than the read.
            //
            // The loop ends when the channel *disconnects*, which is the read
            // finishing: the sender lives in the closure handed to
            // `read_watched`, so it is dropped exactly when that call returns.
            // No second completion flag to keep in step with the first.
            loop {
                let mut latest = None;
                let disconnected = loop {
                    match updates.try_recv() {
                        Ok(at) => latest = Some(at),
                        Err(TryRecvError::Empty) => break false,
                        Err(TryRecvError::Disconnected) => break true,
                    }
                };

                let mut watching = true;
                if let Some((done, total)) = latest {
                    let alive = view.update(cx, |view, cx| {
                        // Anything else in `opening` means this read has been
                        // overtaken by a later one. Stop drawing progress for a
                        // board nobody is waiting for.
                        match &mut view.opening {
                            Some(open) if open.token == token => {
                                open.done = done;
                                open.total = total;
                                cx.notify();
                            }
                            _ => watching = false,
                        }
                    });
                    if alive.is_err() {
                        return;
                    }
                }

                if disconnected || !watching {
                    break;
                }
                cx.background_executor().timer(OPENING_EVERY).await;
            }

            let read = reading.await;
            view.update(cx, |view, cx| view.settle_open(token, read, &target, cx)).ok();
        })
        .detach();
        cx.notify();
    }

    /// Call off a board still being read. Answers whether there was one.
    ///
    /// Disowned rather than stopped, like a drop that is called off: there is no
    /// way to interrupt an `fs::read` half-way through, so the token moves on,
    /// the loader goes, and the answer lands nowhere when it finally arrives.
    /// What that costs is a background thread finishing work nobody wants, which
    /// is the cheaper half of the trade — the expensive half was the window.
    pub fn stop_opening(&mut self, cx: &mut Context<Self>) -> bool {
        if self.opening_leaving {
            return false;
        }
        let Some(open) = self.opening.as_ref() else { return false };
        self.opens = self.opens.wrapping_add(1);
        // `tell`, not `say`: the read itself keeps running on the background
        // thread — there is no way to interrupt an `fs::read` half-way
        // through — so this is a fact about something that is still
        // happening, not narration of something finished. `say`'s tone is
        // never drawn, so it used to give no feedback that the stop had been
        // heard at all.
        self.tell(format!("stopped opening {}", open.name));
        self.opening_leaving = true;
        cx.notify();
        true
    }

    /// The read has come back. Take the board up, or say why not.
    fn settle_open(
        &mut self,
        token: u64,
        read: anyhow::Result<Document>,
        path: &Path,
        cx: &mut Context<Self>,
    ) {
        // Overtaken. Somebody asked for a different board while this one was
        // being read, and that is the answer they are waiting for.
        if self.opening.as_ref().map(|open| open.token) != Some(token) {
            return;
        }
        // Not cleared outright: `advance_loader` fades the panel out and
        // drops it once the fade finishes, rather than the board underneath
        // hard-cutting from "loading" to "done" between two frames.
        self.opening_leaving = true;
        match read {
            Ok(doc) => {
                // Let go of the outgoing board first, then write it: letting
                // go is what closes a drop still arriving into its step, and a
                // file written before that would carry the cards without the
                // entry on the strip that takes them back.
                self.leaving_board(cx);
                // **Here rather than when the open was asked for**, and the
                // difference is work: the board stayed usable for the whole of
                // the read, so anything typed into it during that second is on
                // the board this is about to replace, and this is the last
                // moment there is to write it down.
                self.flush(cx);
                self.adopt(doc, path);
                self.say(format!("opened {}", short_name(path)));
            }
            // Said, not swallowed, and the board that is open stays open. The
            // failure mode this avoids is losing an hour of work to a typo in
            // somebody else's file name.
            Err(err) => {
                // A terminal launch still deserves the message it always got —
                // this used to be the only report a board named on `argv` and
                // refused to open would leave, back when refusing meant
                // `eprintln!` and `exit(1)` before a window existed at all.
                // Harmless everywhere else: a GUI launch on Windows has no
                // console for this to reach, and one on Linux or macOS is
                // usually piping it to a log nobody is watching while the line
                // in the window is doing the actual telling.
                eprintln!("mbrd: could not open {}: {err:#}", path.display());
                self.warn(format!("could not open: {err:#}"));
            }
        }
        cx.notify();
    }

    /// A new, empty board — with a file of its own from the moment it exists.
    ///
    /// **Written to disk here rather than left in memory**, and that is the
    /// whole design: everything else in this app relies on a board having a
    /// path, because a path is what the autosave timer writes to. A new board
    /// that was only in memory would be the one board in the app that could
    /// lose work, and the indicator that used to warn about exactly that is
    /// gone.
    ///
    /// The write is on this thread, unlike every other one. An empty board is a
    /// few hundred bytes, and the path cannot be adopted until it is known to
    /// have worked.
    pub fn new_board(&mut self, cx: &mut Context<Self>) {
        self.flush(cx);
        let doc = Document::default();
        let Some(path) = fresh_board_path(&doc.board) else {
            self.warn("nowhere to put a new board: no home directory".into());
            return;
        };
        match crate::save::write(&path, &doc) {
            Ok(()) => {
                self.leaving_board(cx);
                self.adopt(doc, &path);
                self.say(format!("new board — {}", short_name(&path)));
            }
            Err(err) => self.warn(format!("could not make a board: {err:#}")),
        }
        cx.notify();
    }

    /// Throw away everything that was about the board on its way out.
    fn leaving_board(&mut self, cx: &mut Context<Self>) {
        // Every id in the route cache is about to mean something else, or
        // nothing. A cache that survives a board switch is a cache that draws
        // the old board's lines between the new board's cards.
        self.wires.forget();
        self.drawn.clear();
        self.rope = None;
        // Keep whatever was being typed. The board it belongs to is about to
        // be replaced, so there is no later at which to keep it.
        self.stop_editing(true, cx);
        // And everything still on its way *in*, which belongs to the board on
        // its way out. A drop still arriving would otherwise put the rest of
        // its folder onto the next board; a read still running would replace
        // the next board with the one after it. Both are disowned by moving
        // their token past them — see `stop_importing` and `open_board`.
        self.stop_importing(cx);
        self.opens = self.opens.wrapping_add(1);
        // Not an immediate `self.opening = None`: whatever read this board
        // came from — or one abandoned in its favour, if a second open or a
        // new board was asked for before the first one landed — fades its
        // panel out rather than vanishing between two frames. See
        // `advance_loader`. A no-op when nothing was loading, since that
        // leaves `opening` at `None` regardless.
        self.opening_leaving = true;
    }

    /// Take up a board that is known to be on disk at `path`.
    ///
    /// One function for the two ways a board arrives — opened, and made — so
    /// that the list of what has to be forgotten is written down once. The one
    /// that forgot half of it would be a build where a group you had stepped
    /// into on one board followed you onto the next.
    fn adopt(&mut self, doc: Document, path: &Path) {
        self.doc = doc;
        self.path = Some(path.to_path_buf());
        // On disk as of this instant, both ways in: read from it, or just
        // written to it. Without this the timer would immediately rewrite a
        // file it had no reason to touch.
        self.saved_at = self.doc.board.revision();
        self.failed_at = None;
        self.selection.clear();
        // Ids from the board that was open name nothing on this one. Or worse,
        // they name something: ids are minted `n000001` upward on every board,
        // so a group you had stepped into on one board would land you inside
        // whatever happens to carry that id on the next.
        self.inside.clear();
        self.let_go.forget();
        self.gesture = Gesture::None;
        self.restore_saved_view();
        crate::recent::remember(path);
    }

    /// Put something above the board, in place of whatever was there. See
    /// [`Overlay`] for why there can only ever be one.
    ///
    /// Retargets rather than restarts: `overlay_presence` is left exactly
    /// where it is, so opening the switcher while the palette is still
    /// fading in — or fading out — bends smoothly onto the new surface
    /// instead of dropping to black and fading up again. See
    /// `advance_overlay`.
    fn open_overlay(&mut self, new: Overlay) {
        self.overlay = new;
        self.overlay_leaving = false;
    }

    /// Start the overlay's exit. It keeps rendering — input-dead — until
    /// `advance_overlay` sees `overlay_presence` reach zero and drops it.
    fn close_overlay(&mut self) {
        if !matches!(self.overlay, Overlay::None) {
            self.overlay_leaving = true;
        }
    }

    pub fn open_switcher(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.switches = self.switches.wrapping_add(1);
        let token = self.switches;
        self.open_overlay(Overlay::Switcher(Switcher::open(self.path.as_deref())));
        cx.notify();

        // The rest of the list — every `.mbrd` beside the board that is
        // open, and beside wherever the app was started from — is disk IO
        // that used to run here, on the thread that draws: a `read_dir` for
        // each directory and a `canonicalize` per file found in them. The
        // switcher now opens at once with what `recent.json` already
        // remembered, and this fills in the rest once it is ready, the same
        // shape `open_board` reads a board in.
        let here = self.path.clone();
        cx.spawn(async move |view, cx| {
            let found = cx
                .background_executor()
                .spawn(async move { crate::switcher::beside_boards(here.as_deref()) })
                .await;
            view.update(cx, |view, cx| {
                // Overtaken: the switcher has been closed and opened again,
                // or closed for good, since this scan was asked for — and
                // this list is about the directories *that* switcher opened
                // beside, which may not even be these ones.
                if view.switches != token {
                    return;
                }
                if let Overlay::Switcher(switcher) = &mut view.overlay {
                    switcher.extend_boards(found);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn close_switcher(&mut self) {
        if matches!(self.overlay, Overlay::Switcher(_)) {
            self.close_overlay();
        }
    }

    /// Put the question about deleting a board, or take it back with `None`.
    ///
    /// Here rather than reaching into the switcher from `switcher.rs`, because
    /// the rows are drawn with a `&BoardView` and the answer has to arrive as a
    /// change to a `&mut` one — this is the door between the two.
    pub fn ask_about_board(&mut self, board: Option<PathBuf>) {
        if let Overlay::Switcher(switcher) = &mut self.overlay {
            switcher.ask_about(board);
        }
    }

    /// Delete the board the switcher has asked about and been answered for.
    pub fn delete_doomed_board(&mut self, cx: &mut Context<Self>) {
        let doomed = match &self.overlay {
            Overlay::Switcher(switcher) => switcher.doomed(),
            _ => None,
        };
        if let Some(board) = doomed {
            self.delete_board(&board, cx);
        }
    }

    /// Take a board off the disk.
    ///
    /// The only thing in this app that destroys something outside its own
    /// window, so it is reached only through a question that has been asked and
    /// answered — see `Switcher::confirming`, which is where the asking lives.
    ///
    /// **Not the open board.** The switcher does not offer it, and this does
    /// not check again: the file would be gone and the next autosave would
    /// write it straight back, which is a worse outcome than either deleting it
    /// or refusing to.
    ///
    /// The switcher stays open. Deleting one board of several is a thing people
    /// do in a run, and a list that put itself away after each would make the
    /// second one a fresh trip through `Ctrl P`.
    fn delete_board(&mut self, board: &Path, cx: &mut Context<Self>) {
        // `trash::delete` first, always — it implements the freedesktop trash
        // spec on Linux and the native Recycle Bin / Trash on Windows and
        // macOS, so the only irreversible action in this app stops being
        // irreversible: a board deleted by mistake, or a board somebody
        // decides they wanted back an hour later, is one trip to the system
        // trash away from existing again. `std::fs::remove_file` only runs
        // when that fails — a network mount or a sandbox with nowhere to put
        // a trashed file — and a permanent delete stays the fallback of last
        // resort rather than the plan.
        let (result, trashed) = match trash::delete(board) {
            Ok(()) => (Ok(()), true),
            Err(_) => (std::fs::remove_file(board), false),
        };
        match result {
            Ok(()) => {
                crate::recent::forget(board);
                if let Overlay::Switcher(switcher) = &mut self.overlay {
                    switcher.dropped(board);
                }
                self.tell(if trashed {
                    format!("moved {} to the trash", short_name(board))
                } else {
                    format!("deleted {}", short_name(board))
                });
            }
            // Said in the row the question was asked in, rather than in the
            // status bar at the far corner of the window — see
            // `Switcher::refused`. A board on a read-only disk, or one
            // somebody else has already removed, is the one case where
            // nothing else on screen would change and the row would simply
            // stay, question and all.
            Err(err) => {
                if let Overlay::Switcher(switcher) = &mut self.overlay {
                    switcher.refuse(board.to_path_buf(), format!("could not delete: {err}"));
                }
            }
        }
        cx.notify();
    }

    /// Open one of the two palettes. See `palette.rs`.
    ///
    /// Whatever else was open closes first — `open_overlay` does that simply
    /// by being the one door onto the field. Two text fields both claiming
    /// the keyboard is a state with no way out of it, and a menu left
    /// standing behind a palette is a menu about a selection you are about
    /// to change.
    pub fn open_palette(&mut self, mode: crate::palette::Mode, cx: &mut Context<Self>) {
        // The gesture that opened this must not still be half-complete when it
        // closes, or the tap that dismissed it starts the next pair.
        self.taps.forget();
        self.open_overlay(Overlay::Palette(Palette::open(mode, self)));
        cx.notify();
    }

    pub fn close_palette(&mut self) {
        if matches!(self.overlay, Overlay::Palette(_)) {
            self.close_overlay();
            self.taps.forget();
        }
    }

    /// Open the settings page — or, from the titlebar button while it is
    /// already up, put it away: a button that can only open is a button that
    /// stops working the moment it has worked. See `settings.rs` for what is
    /// on the page.
    pub fn open_settings(&mut self, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::Settings(_)) && !self.overlay_leaving {
            self.close_settings();
            cx.notify();
            return;
        }
        self.taps.forget();
        self.open_overlay(Overlay::Settings(crate::settings::Page::open()));
        cx.notify();
    }

    /// Show another of the page's sections. A no-op when the page is not up,
    /// which cannot happen from anywhere this is called.
    pub fn show_settings_section(
        &mut self,
        section: crate::settings::Section,
        cx: &mut Context<Self>,
    ) {
        if let Overlay::Settings(page) = &mut self.overlay {
            page.section = section;
            cx.notify();
        }
    }

    /// Where a settings control is drawn, between the states it can be in.
    ///
    /// `resting` is the answer until the control has been pressed — the
    /// state itself, with nothing in flight. **This is the number to draw**,
    /// same rule as every other spring.
    pub fn control_at(&self, id: &str, resting: f32) -> f32 {
        self.settings_motion.get(id).map_or(resting, |s| s.value())
    }

    /// Start a settings control moving: plant it where it *was* if this is
    /// its first press, and aim it where it now is. A press mid-flight keeps
    /// the value and the velocity it already has — the knob bends back out
    /// of its own motion rather than jumping to an end and starting over.
    pub fn move_control(&mut self, id: &str, from: f32, to: f32) {
        let spring = self.settings_motion.entry(id.to_string()).or_insert_with(|| Sprung::at(from));
        spring.retarget(to);
    }

    /// One frame of every settings control that is still travelling.
    fn advance_controls(&mut self, dt: f32) -> bool {
        let mut moving = false;
        for spring in self.settings_motion.values_mut() {
            moving |= spring.step(KNOB, dt, 0.01);
        }
        moving
    }

    /// Whether something is drawn over the whole board rather than beside it.
    ///
    /// The two whole-window overlays, and not the three that float: a menu, a
    /// palette or a switcher leaves most of the board visible and pointing at
    /// that part of it still means what it meant. A settings page or an opened
    /// card does not — there is no board under the pointer any more, only a
    /// picture of one.
    fn covered(&self) -> bool {
        matches!(self.overlay, Overlay::Settings(_) | Overlay::Opened(_))
    }

    pub fn close_settings(&mut self) {
        if matches!(self.overlay, Overlay::Settings(_)) {
            self.close_overlay();
        }
    }

    // -----------------------------------------------------------------------
    // Themes
    // -----------------------------------------------------------------------

    /// Which of the two palettes the app is wearing right now.
    ///
    /// The mode's answer, with the desktop's own only consulted where the mode
    /// says to. Every question about *which* theme — which list the settings
    /// page shows, which name a choice writes into — is this question first,
    /// which is why it is one function and not a `match` repeated at each of
    /// them.
    pub fn appearance(&self) -> crate::themes::Appearance {
        self.prefs.mode.appearance(self.system)
    }

    /// The palette the current settings add up to.
    pub fn chosen_theme(&self) -> Theme {
        let appearance = self.appearance();
        self.themes.resolve(self.prefs.theme_for(appearance), appearance)
    }

    /// Wear what the settings currently say.
    ///
    /// The one door. The settings rows, the picker, the desktop changing its
    /// mind and the reload button all end here rather than each assigning
    /// [`Self::theme`] themselves — which matters more than it looks, because
    /// "which theme is on" is a question with four inputs (the mode, the
    /// desktop, two names and a registry) and any path that answered it
    /// privately would be a path that could answer it differently.
    ///
    /// It also ends any preview: arriving here means a real decision has been
    /// made, and the palette that was being tried on is no longer what is
    /// being asked about.
    pub fn retheme(&mut self, cx: &mut Context<Self>) {
        self.theme_before_preview = None;
        self.theme = self.chosen_theme();
        cx.notify();
    }

    /// What the desktop says, which is only news when the mode is `System`.
    pub fn desktop_appearance(
        &mut self,
        appearance: crate::themes::Appearance,
        cx: &mut Context<Self>,
    ) {
        if self.system == appearance {
            return;
        }
        self.system = appearance;
        // Tracked always, applied only when it is being followed. Somebody who
        // has pinned the app dark has said the desktop does not get a vote,
        // and a repaint on every sunrise would be this app disagreeing.
        if self.prefs.mode == crate::prefs::Mode::System {
            self.retheme(cx);
        }
    }

    /// Choose the mode, and save it.
    pub fn set_mode(&mut self, mode: crate::prefs::Mode, cx: &mut Context<Self>) {
        self.prefs.mode = mode;
        crate::prefs::save(&self.prefs);
        self.retheme(cx);
    }

    /// Choose the theme for one appearance, and save it.
    ///
    /// Takes the appearance rather than assuming the current one, because the
    /// settings page shows both rows at once: somebody pinned to dark can
    /// still be setting which theme their light one will be.
    pub fn set_theme(
        &mut self,
        appearance: crate::themes::Appearance,
        name: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        self.prefs.set_theme(appearance, name);
        crate::prefs::save(&self.prefs);
        self.retheme(cx);
    }

    /// Try a palette on without choosing it.
    ///
    /// What the picker does as the highlight moves. The first preview
    /// remembers what was on screen; the ones after it do not, so that
    /// arrowing through nine themes and pressing Escape returns to where
    /// somebody started rather than to the eighth.
    pub fn preview_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        if self.theme_before_preview.is_none() {
            self.theme_before_preview = Some(self.theme);
        }
        self.theme = theme;
        cx.notify();
    }

    /// Put back whatever was on screen before the preview started.
    ///
    /// Deliberately not [`Self::retheme`]: they agree in every case but one,
    /// and that one is the point. A preview started from a theme that is no
    /// longer in the registry — the file was deleted while the app was open —
    /// would be "restored" by `retheme` to the fallback rather than to what
    /// the person was actually looking at a moment ago.
    pub fn cancel_preview(&mut self, cx: &mut Context<Self>) {
        if let Some(theme) = self.theme_before_preview.take() {
            self.theme = theme;
            cx.notify();
        }
    }

    /// Read the themes directory again.
    ///
    /// The cheap half of watching it. Somebody who has just written a theme
    /// file presses this; nobody else pays anything for it.
    pub fn reload_themes(&mut self, cx: &mut Context<Self>) {
        self.themes = crate::themes::Registry::load();
        self.retheme(cx);
    }

    /// Open the settings page onto Appearance with the theme list already up.
    ///
    /// What `Command::SelectTheme` does. It goes through the settings page
    /// rather than putting a picker of its own over the board, and that is
    /// the point of it: `Overlay` holds one thing at a time — see its note —
    /// so a second, free-standing theme picker would be a second list to keep
    /// saying the same thing as the first, and a person who arrived by the
    /// fast route would have no way onward to the rest of the appearance
    /// settings without closing it and opening something else.
    pub fn open_theme_picker(&mut self, cx: &mut Context<Self>) {
        let appearance = self.appearance();
        let was = self.prefs.theme_for(appearance).to_string();
        let names: Vec<String> =
            self.themes.of(appearance).iter().map(|t| t.name.clone()).collect();
        let mut page = crate::settings::Page::onto(crate::settings::Section::Appearance);
        page.pick_theme(appearance, &was, &names);
        self.taps.forget();
        self.open_overlay(Overlay::Settings(page));
        cx.notify();
    }

    /// Start choosing a theme for one of the two slots.
    pub fn pick_theme(&mut self, appearance: crate::themes::Appearance, cx: &mut Context<Self>) {
        let was = self.prefs.theme_for(appearance).to_string();
        let names: Vec<String> =
            self.themes.of(appearance).iter().map(|t| t.name.clone()).collect();
        if let Overlay::Settings(page) = &mut self.overlay {
            page.pick_theme(appearance, &was, &names);
            cx.notify();
        }
    }

    /// Keep the theme the picker is on.
    pub fn choose_theme(&mut self, name: String, cx: &mut Context<Self>) {
        let Overlay::Settings(page) = &mut self.overlay else { return };
        let Some(appearance) = page.picking.as_ref().map(|p| p.appearance) else { return };
        page.picking = None;
        self.set_theme(appearance, name, cx);
    }

    /// Put back whatever was chosen before the picker opened.
    pub fn cancel_theme_pick(&mut self, cx: &mut Context<Self>) {
        let Overlay::Settings(page) = &mut self.overlay else { return };
        let Some(picker) = page.picking.take() else { return };
        // The *choice* is restored through the ordinary setter, and the
        // palette on screen through `cancel_preview`. Both, and in that
        // order: the setter is what makes `retheme` agree with the prefs
        // again, and the preview is what is actually being looked at.
        self.prefs.set_theme(picker.appearance, picker.was);
        self.cancel_preview(cx);
        self.retheme(cx);
    }

    /// Fold or unfold one of the settings sidebar's groups.
    pub fn fold_settings_group(&mut self, group: crate::settings::Group, cx: &mut Context<Self>) {
        if let Overlay::Settings(page) = &mut self.overlay {
            page.fold(group);
            cx.notify();
        }
    }

    /// Take hold of the settings search field with the mouse.
    ///
    /// The keys were always coming here — the page has one field and nothing
    /// else to type into — so this is not what makes typing work. It is what
    /// makes pressing the field mean something instead of nothing, which is
    /// what anybody who reaches for a search box with the pointer first is
    /// owed. The caret goes to the end because the field cannot measure its
    /// own text to find out which character was pressed, and the end is where
    /// pressing past a short query lands anyway.
    pub fn focus_settings_search(&mut self, cx: &mut Context<Self>) {
        if let Overlay::Settings(page) = &mut self.overlay {
            let end = page.query.text().len();
            page.query.place(end, false);
            page.focused = true;
            cx.notify();
        }
    }

    /// Let go of it again, on a press anywhere else on the page.
    ///
    /// Only the drawing changes: the next letter typed takes it back, because
    /// a settings page you cannot search by simply typing would be a worse
    /// page than one whose caret is sometimes not where you looked last.
    pub fn blur_settings_search(&mut self, cx: &mut Context<Self>) {
        if let Overlay::Settings(page) = &mut self.overlay {
            if page.focused {
                page.focused = false;
                cx.notify();
            }
        }
    }

    /// Empty the settings search field.
    pub fn clear_settings_search(&mut self, cx: &mut Context<Self>) {
        if let Overlay::Settings(page) = &mut self.overlay {
            page.query = crate::editor::Editor::new("", 64, false);
            cx.notify();
        }
    }

    /// Open `settings.json` in whatever this desktop opens `.json` with.
    ///
    /// Written first, and that is the point rather than an implementation
    /// detail: on a fresh install the file does not exist yet, and handing
    /// somebody's editor a path to nothing is a worse answer than handing it
    /// the defaults they are about to change. `prefs::save` is the same
    /// best-effort write every other setting goes through, so this cannot
    /// fail loudly either.
    ///
    /// Shelled out rather than taken as a dependency. This is one command with
    /// three spellings, and the workspace's note about `dirs` applies exactly:
    /// eight direct dependencies, each of them a decision, and this is not
    /// worth being the ninth.
    pub fn edit_settings_file(&mut self, cx: &mut Context<Self>) {
        crate::prefs::save(&self.prefs);
        let Some(path) = crate::dirs::config().map(|dir| dir.join("settings.json")) else {
            self.warn("There is nowhere on this computer to keep settings.".into());
            cx.notify();
            return;
        };

        // Three spellings of one command. Windows needs the extra dance
        // because `start` is a shell builtin rather than a program, and its
        // first argument is taken as the *window title* — hence the empty
        // string, which is the documented way of saying "the path is the
        // path, not the title".
        #[cfg(target_os = "linux")]
        let (program, before): (&str, &[&str]) = ("xdg-open", &[]);
        #[cfg(target_os = "macos")]
        let (program, before): (&str, &[&str]) = ("open", &[]);
        #[cfg(windows)]
        let (program, before): (&str, &[&str]) = ("cmd", &["/C", "start", ""]);
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        let (program, before): (&str, &[&str]) = ("xdg-open", &[]);

        let mut command = std::process::Command::new(program);
        command.args(before);
        // Detached and never waited on. Whatever opens a `.json` here is a
        // text editor somebody will sit in for a while, and a canvas that
        // blocked its own frame loop on one closing would be a hang.
        match command.arg(&path).spawn() {
            Ok(_) => self.tell(format!("Opened {}", path.display())),
            Err(_) => self.warn(format!("Nothing on this computer would open {}", path.display())),
        }
        cx.notify();
    }

    // -----------------------------------------------------------------------
    // A card, opened onto the whole window
    // -----------------------------------------------------------------------

    /// Open a card. What a double-click on one means, whatever type it is.
    ///
    /// The gesture used to open a card for *typing*, which is right for a note
    /// and is very nearly nothing for a photograph — the only thing there was
    /// to type on one is its name. So typing moved to `F2`, which is the key
    /// people already reach for, and this became the thing a double-click means
    /// everywhere: show me this. See `opened.rs`.
    pub fn open_card(&mut self, id: &str, cx: &mut Context<Self>) {
        if self.doc.board.item(id).is_none() {
            return;
        }
        // Any session still open on the board belongs to the board. Left
        // standing it would be a caret on a card behind a page nobody can see,
        // and its `Pending` would join the next edit made up here.
        self.stop_editing(true, cx);
        self.taps.forget();
        // The rail opens with the page for a card there is nothing to draw
        // for, and stays shut for one there is. Both are the same argument: a
        // page should never open onto an empty middle, and a photograph should
        // never open onto a photograph with a panel of numbers across a third
        // of it. See `opened::rail`.
        let bare = self.opened_preview(id) == mbrd_core::preview::Preview::Nothing;
        self.open_overlay(Overlay::Opened(crate::opened::Opened::open(id, bare)));
        cx.notify();
    }

    pub fn close_opened(&mut self, cx: &mut Context<Self>) {
        if matches!(self.overlay, Overlay::Opened(_)) {
            // Keeping what was typed, which is the same thing closing the
            // window with the mouse means everywhere else in this app: the
            // undo history is what puts a change back, not a lost page.
            self.stop_editing(true, cx);
            self.close_overlay();
            cx.notify();
        }
    }

    /// The card the open window is showing, where one is open.
    pub fn opened_id(&self) -> Option<&str> {
        match &self.overlay {
            Overlay::Opened(opened) => Some(opened.id.as_str()),
            _ => None,
        }
    }

    /// What is being typed, for the page that draws it.
    pub fn editor(&self) -> Option<&Editor> {
        self.editing.as_ref().map(|open| &open.editor)
    }

    /// Show or hide it. The page's other button, and the one that is on every
    /// type — see the module header of `opened.rs`.
    pub fn toggle_opened_info(&mut self, cx: &mut Context<Self>) {
        let Overlay::Opened(opened) = &mut self.overlay else { return };
        opened.info = !opened.info;
        let shut = !opened.info;
        // Putting the rail away while a rail field is being typed into would
        // leave the caret behind it, which is exactly the state
        // `edit_opened_field` opens the rail to avoid — the same defect, from
        // the other end. So the rail closing ends that session, keeping what
        // was typed: the words are the one field typed in the page, and they
        // are untouched by this.
        let typing_in_the_rail = self
            .editing
            .as_ref()
            .and_then(|open| open.on.card())
            .is_some_and(|(_, field)| field != Field::Note);
        if shut && typing_in_the_rail {
            self.stop_editing(true, cx);
        }
        cx.notify();
    }

    /// What the open page should draw for a card, or would if it were open.
    ///
    /// A method rather than a call at each site because resolving the asset is
    /// two `Option`s and a map lookup, and three copies of that is how one of
    /// them eventually resolves a different asset than the page is drawing.
    pub fn opened_preview(&self, id: &str) -> mbrd_core::preview::Preview {
        let Some(item) = self.doc.board.item(id) else {
            return mbrd_core::preview::Preview::Nothing;
        };
        mbrd_core::preview::of(item, self.asset_of(item))
    }

    /// The bytes behind a card, where it has any in this document.
    pub fn asset_of(&self, item: &Item) -> Option<&mbrd_core::mbrd::Asset> {
        let hash = item.asset.as_ref().and_then(ItemAsset::hash)?;
        self.doc.assets.get(hash)
    }

    /// Start or stop typing into the open card.
    ///
    /// The page's Edit button, which starts on whatever
    /// [`mbrd_core::preview::editable`] put first — the words for a note, the
    /// address for a link, the colour for a swatch, and the name for everything
    /// whose name is all it has. It is never dead: every card has something.
    pub fn toggle_opened_typing(&mut self, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            self.stop_editing(true, cx);
            return;
        }
        let Some(id) = self.opened_id().map(str::to_string) else { return };
        let Some(what) = self.opened_principal(&id) else { return };
        self.edit_opened_field(&id, what, cx);
    }

    /// Open the *words* of the page for typing. What a double-click on the
    /// shown text asks for.
    ///
    /// Deliberately not [`Self::toggle_opened_typing`], which is the button:
    /// the button toggles whatever session happens to be open, so a
    /// double-click on the words while a name was being typed in the rail
    /// would close the name and open nothing. This one names the field it
    /// wants. [`Self::edit_opened_field`] commits the rail's session on the
    /// way past, so nothing typed there is lost.
    ///
    /// Silent where the words are not the principal field — a ten-megabyte log
    /// is shown and not typed into — so that the double-click and the Edit
    /// button can never disagree about what they open. See
    /// [`crate::opened::typeable`], which is the same condition asked before
    /// the handler is wired at all.
    pub fn edit_opened_words(&mut self, cx: &mut Context<Self>) {
        use mbrd_core::preview::Editable;
        let Some(id) = self.opened_id().map(str::to_string) else { return };
        let Some(what @ Editable::Text { .. }) = self.opened_principal(&id) else { return };
        self.edit_opened_field(&id, what, cx);
    }

    /// The field the Edit button starts on.
    ///
    /// The first of the card's editables, with one exception: a file too long
    /// to hold in a `String` with a caret in it is skipped, so the button falls
    /// through to the next field rather than going grey. A ten-megabyte log
    /// still has a name worth changing.
    pub fn opened_principal(&self, id: &str) -> Option<mbrd_core::preview::Editable> {
        use mbrd_core::preview::Editable;
        let item = self.doc.board.item(id)?;
        let fields = mbrd_core::preview::editable(item, self.asset_of(item));
        let long =
            crate::opened::words_of(item, self).chars().count() > mbrd_core::preview::TEXT_MAX;
        fields.into_iter().find(|what| !(long && matches!(what, Editable::Text { .. })))
    }

    /// Open one of the card's fields for typing, in the window rather than on
    /// the card.
    ///
    /// *Which* fields a card has is `mbrd_core::preview`'s answer and not this
    /// function's, which is the whole point of the split: "a link has an
    /// address and a name" is a fact about the format and testable without a
    /// window, while "the words commit as new bytes in the archive" is a fact
    /// about this editor and cannot be.
    pub fn edit_opened_field(
        &mut self,
        id: &str,
        what: mbrd_core::preview::Editable,
        cx: &mut Context<Self>,
    ) {
        use mbrd_core::preview::Editable;
        self.stop_editing(true, cx);
        let Some(item) = self.doc.board.item(id) else { return };
        let (field, before, limit, multiline, from_file) = match what {
            Editable::Text { limit } => match crate::opened::file_text(item, self) {
                // A file this long is not something a `String` with a caret in
                // it should be asked to hold. It stays readable; the button
                // moves on to the next field.
                Some(text) if text.chars().count() > mbrd_core::preview::TEXT_MAX => return,
                Some(text) => (Field::Note, text, limit, true, true),
                None => {
                    let words = item.note_text().unwrap_or_default().to_string();
                    (Field::Note, words, limit, true, false)
                }
            },
            Editable::Url => {
                (Field::Url, item.url().unwrap_or_default().to_string(), URL_MAX, false, false)
            }
            // A swatch's colour is its name — see `write_field`.
            Editable::Hex | Editable::Name => {
                (Field::Name, item.name.clone(), mbrd_core::model::NOTE_MAX, false, false)
            }
        };
        // The words are typed in the page; everything else is typed in the
        // rail — see `opened::field` — so a session on one of those fields puts
        // the rail out first. Without this the header's Edit button on a link,
        // a swatch or a plain file starts a session whose caret is behind a
        // closed rail: the keys land, the card changes, and nothing on screen
        // says so. Here rather than in `toggle_opened_typing` because it is a
        // fact about where a field is drawn, and the rail's own rows reach this
        // function too — for them it is already true and costs nothing.
        if field != Field::Note {
            if let Overlay::Opened(opened) = &mut self.overlay {
                opened.info = true;
            }
        }
        // A short field opens selected, so typing replaces it; a long one opens
        // with the caret at the end, so typing continues it. The same bargain
        // `edit_card` makes on the board, for the same reason.
        let editor = match multiline {
            true => Editor::new(before.clone(), limit, true),
            false => Editor::selecting_all(before.clone(), limit, false),
        };
        self.rope = None;
        self.editing = Some(Editing {
            on: Subject::Card(id.to_string(), field),
            editor,
            before,
            file: from_file,
            open: self.doc.board.start(),
        });
        self.hint(Some(hint_for(field)));
        cx.notify();
    }

    /// What the open page needs from the window, measured for it.
    ///
    /// Here rather than in `opened.rs` because both halves want a `&mut` the
    /// render pass has already given up by the time it is holding the overlay:
    /// looking a photograph up in the cache is what marks it as wanted, and a
    /// character's width comes from the window's text system.
    fn ready_opened(
        &mut self,
        id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> crate::opened::Ready {
        // The whole window's worth of texels, which is what asks the cache for
        // the sharp copy rather than the thumbnail a card would be happy with.
        let wanted = f(window.viewport_size().width) * window.scale_factor();
        // Through the page's own decision rather than the card's: a `.png` that
        // imported as `generic` draws as a picture here, and `picture_hash`
        // would answer `None` for it because a *card* of that type does not.
        // A mesh's picture depends on more than its bytes — see
        // `mesh_cache`'s own module doc — so it is not `images.look`'s to
        // answer, the way `frame_of` would otherwise have it try to.
        let picture = if self.opened_preview(id) == mbrd_core::preview::Preview::Mesh {
            self.mesh_picture(id, true, cx)
        } else {
            let hash = crate::opened::frame_of(id, self);
            match hash {
                Some(hash) => match self.images.look(&hash, wanted) {
                    Load::Ready(image, _) => Some(image),
                    // Nobody has asked for this one yet — the card may never
                    // have been on screen. Ask now; the page draws its
                    // placeholder for a frame and the decode lands behind it.
                    Load::Cold => {
                        self.begin_decode(&hash, cx);
                        None
                    }
                    Load::Waiting | Load::Failed => None,
                },
                None => None,
            }
        };

        // One character of the editor's face, which is what turns a column into
        // an `x`. Measured rather than assumed: the family that answers is
        // whichever of the fallback chain the machine actually has.
        let run = TextRun {
            len: 1,
            font: crate::opened::mono(),
            color: self.theme.text,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let advance = window
            .text_system()
            .shape_line("0".into(), px(crate::opened::mono_size()), &[run], None)
            .width;

        let advance = f(advance).max(1.0);
        self.opened_advance = advance;
        // Off the card's own playhead, the same read `draw_list` makes for the
        // card behind — so opening a GIF shows it where it already was rather
        // than restarting it, and closing the page does not jump it back.
        let frame = match &picture {
            Some(image) if image.frame_count() > 1 => {
                let item = self.doc.board.item(id);
                let looping = item.is_some_and(|item| mbrd_core::media::playback(item).looping);
                self.timings.of(id, image).frame_at(self.media.at(id), looping)
            }
            _ => 0,
        };
        crate::opened::Ready { picture, frame, advance }
    }

    /// Record where the open editor's text landed. Called by the page's own
    /// canvas as it is laid out; notifies only on a change, or it would ask for
    /// a frame from inside the frame it is drawing.
    /// How wide the open editor's text block is, once it has been drawn once.
    ///
    /// `None` on the frame the page opens, which is the caller's cue to use the
    /// measure the page is capped at — see `opened::source`.
    /// How wide one character of the open editor's face is, once the window has
    /// measured it. See [`Self::ready_opened`].
    pub fn opened_advance(&self) -> f32 {
        self.opened_advance.max(1.0)
    }

    pub fn opened_width(&self) -> Option<f32> {
        let width = f(self.opened_text.size.width);
        (width > 1.0).then_some(width)
    }

    pub fn opened_text_at(&mut self, bounds: Bounds<Pixels>, cx: &mut Context<Self>) {
        if self.opened_text != bounds {
            self.opened_text = bounds;
            cx.notify();
        }
    }

    /// Move the caret to where somebody clicked in the open editor.
    ///
    /// [`Self::place_caret`] is the same errand on a card and cannot be shared
    /// with this: there it lands on a proportional face and the answer has to
    /// come back from the text system, and here the face is fixed-width, so a
    /// column is a division and there is nothing to measure. That is the whole
    /// reason the page is set in that face — see `opened.rs`.
    pub fn place_opened_caret(
        &mut self,
        at: gpui::Point<Pixels>,
        extend: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(offset) = self.opened_caret_at(at) else { return };
        if let Some(open) = &mut self.editing {
            open.editor.place(offset, extend);
        }
        cx.notify();
    }

    /// A second, third or fourth press on the page: the run, the line, the lot.
    /// [`Self::select_run_at`] is the same ladder on a card.
    pub fn select_opened_run_at(
        &mut self,
        at: gpui::Point<Pixels>,
        clicks: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(offset) = self.opened_caret_at(at) else { return };
        if let Some(open) = &mut self.editing {
            match clicks {
                2 => open.editor.select_word_at(offset),
                3 => open.editor.select_line_at(offset),
                _ => open.editor.select_all(),
            }
        }
        cx.notify();
    }

    /// Which byte of the open page's text a point lands on.
    fn opened_caret_at(&self, at: gpui::Point<Pixels>) -> Option<usize> {
        let advance = self.opened_advance.max(1.0);
        let line_height = crate::opened::line_height();
        // The same break the page made when it drew these rows. See
        // `opened::room_in` for why it is one function and not two.
        let (room, per_char) = crate::opened::room_in(
            self.opened_width().unwrap_or(f(self.opened_text.size.width)),
            advance,
        );
        let open = self.editing.as_ref()?;

        let rows = open.editor.wrapped(room, crate::opened::mono_size(), &per_char);
        let local_y = f(at.y) - f(self.opened_text.origin.y);
        let local_x = f(at.x) - f(self.opened_text.origin.x);
        let row =
            ((local_y / line_height).floor().max(0.0) as usize).min(rows.len().checked_sub(1)?);
        let (start, end) = rows[row];

        // Rounded rather than truncated, so pressing in the right-hand half of
        // a character puts the caret after it — which is what everything else
        // with a caret in it does, and the difference between aiming at a
        // character and aiming at the gap before one.
        let column = (local_x / advance).round().max(0.0) as usize;
        let line = &open.editor.text()[start..end];
        Some(line.char_indices().nth(column).map_or(end, |(offset, _)| start + offset))
    }

    /// Whether a drag is sweeping out a text selection right now.
    ///
    /// The page asks before it extends: a pointer crossing the words with no
    /// button down is not a selection, and one whose press landed on the
    /// page's chrome rather than on the text is not one either.
    pub fn selecting_text(&self) -> bool {
        matches!(self.gesture, Gesture::SelectingText)
    }

    /// Arm or disarm the text drag. Called by the page, which has its own
    /// listeners because the board's canvas is behind it.
    pub fn select_text_drag(&mut self, on: bool) {
        match on {
            true => self.gesture = Gesture::SelectingText,
            false if self.selecting_text() => self.gesture = Gesture::None,
            false => {}
        }
    }

    /// Write the words back as the card's **file**, and repoint the card at it.
    ///
    /// Two things happen and both have to, in this order. The bytes go into the
    /// archive under the hash of their own contents, which is the format's own
    /// identity rule and is what makes the same text twice one asset. Then the
    /// card is repointed at that hash *through the ledger*, alongside a
    /// refreshed `meta.text` — so the card behind the page still says what the
    /// file starts with, and one Ctrl Z takes the whole edit back.
    ///
    /// The old bytes are not removed. Nothing here removes an asset: a step in
    /// the history still names it, which is exactly what undo needs to find.
    fn write_file(&mut self, id: &str, text: &str, token: &Pending) {
        let (ext, label) = self
            .doc
            .board
            .item(id)
            .and_then(|item| item.asset.as_ref())
            .and_then(ItemAsset::hash)
            .and_then(|hash| self.doc.assets.get(hash))
            .map(|asset| (asset.ext.clone(), asset.label.clone()))
            .unwrap_or_else(|| ("md".to_string(), "note".to_string()));

        let bytes = text.as_bytes().to_vec();
        let hash = mbrd_core::mbrd::hash_bytes(&bytes);
        self.doc.assets.entry(hash.clone()).or_insert(mbrd_core::mbrd::Asset { bytes, ext, label });

        let head: String = text.chars().take(mbrd_core::model::NOTE_MAX).collect();
        let id = id.to_string();
        self.doc.board.during(token, |board| {
            if let Some(item) = board.item_mut(&id) {
                item.asset = Some(ItemAsset::Embedded { hash: hash.clone(), family: None });
                write_field(item, Field::Note, &head, &self.measure);
            }
        });
    }

    /// Do what a palette row says, and put the palette away.
    ///
    /// Both doors — Enter and a press on the row — come through here, so the
    /// two cannot drift into meaning different things.
    pub fn run_palette_row(&mut self, what: What, window: &mut Window, cx: &mut Context<Self>) {
        self.close_palette();
        match what {
            // Checked again rather than trusted: the row was drawn against the
            // board as it was when the palette opened, and a command that has
            // since stopped applying should do nothing rather than something
            // unexpected.
            What::Does(command) => {
                if command.available(self) {
                    command.run(self, window, cx);
                }
            }
            What::Goes { id, .. } => self.reveal(&id, cx),
        }
        cx.notify();
    }

    /// Select one card and go to it.
    ///
    /// The half of search that makes it worth having. Being told where a thing
    /// is is no use on a canvas with no edges — you would still have to fly
    /// there by hand — so choosing a result moves the camera onto it.
    ///
    /// Through the camera rather than onto the viewport, for the reason
    /// `go_home` gives: on a board that goes on forever the camera is the whole
    /// of somebody's sense of place, and a cut throws it away. You arrive
    /// having *travelled*, which is what tells you where the card was in
    /// relation to where you were.
    ///
    /// Capped at 100% like `fit_all`, and for the same reason: a small note
    /// should arrive readable rather than magnified until the grain shows, and
    /// a wall-sized image should arrive whole.
    pub fn reveal(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(item) = self.doc.board.item(id) else { return };
        let bounds = Rect::of_item(item);
        self.selection = vec![id.to_string()];
        self.rope = None;
        let mut want = self.viewport;
        want.fit(Some(bounds), 160.0, BASE_ZOOM);
        self.camera.travel_to(&want);
        cx.notify();
    }

    pub fn close_menu(&mut self) {
        if matches!(self.overlay, Overlay::Menu(_)) {
            self.close_overlay();
        }
    }

    /// Open the right-click menu at the selection, or at the middle of the
    /// view when nothing is selected — for the platform's own context-menu
    /// key, and for `Shift F10` on the keyboards that have none. The same
    /// list a right-click there would open: see `command::menu_for`.
    ///
    /// The selection itself is left alone. There is no press to read a world
    /// point off, so this asks what is already true rather than pretending a
    /// click happened somewhere.
    fn open_context_menu_at(&mut self, cx: &mut Context<Self>) {
        let items = self.selection.iter().filter_map(|id| self.doc.board.item(id));
        let world = geometry::union(items).map(|r| r.centre()).unwrap_or(self.viewport.pan);
        let screen = self.viewport.to_screen(world);
        let local = gpui::point(px(screen.x), px(screen.y));
        let entries = Entry::shown(crate::command::menu_for(self), self);
        self.open_overlay(Overlay::Menu(Menu::new(local, entries, self.canvas_bounds.size)));
        cx.notify();
    }

    /// The pointer has arrived on a row of the open menu. Moves the keyboard
    /// highlight there too — see [`Menu::cursor`], which is the one concept
    /// behind both — and settles whether a submenu is open and which one:
    /// arriving on a row that opens onto more opens it, and arriving anywhere
    /// else closes what was open. See [`Menu::reveal`] for why it is arrival
    /// rather than departure that decides.
    pub fn reveal_menu(&mut self, row: usize, cx: &mut Context<Self>) {
        if !matches!(self.overlay, Overlay::Menu(_)) {
            return;
        }
        // Worked out while the overlay is only borrowed, because the list a
        // row opens onto is filtered against the board — see `Entry::shown` —
        // and the board is what the mutable borrow below is taken out of.
        let sub = self.submenu_at(row);
        let room = self.canvas_bounds.size;
        let Overlay::Menu(menu) = &self.overlay else { return };
        // How far the list has scrolled, which is the difference between where
        // the row sits in the list and where it sits on the screen. Only a
        // window too short to hold the list makes it anything but zero, and
        // only a submenu cares — see `Menu::beside`.
        let scroll = menu.scroll.offset().y;
        if let Overlay::Menu(menu) = &mut self.overlay {
            let moved = menu.cursor != Some(row);
            menu.cursor = Some(row);
            let opened = menu.reveal(row, room, scroll, sub);
            if moved || opened {
                cx.notify();
            }
        }
    }

    /// The list the row at `row` of the open menu opens onto, filtered to the
    /// rows that apply — `None` where that row opens onto nothing at all.
    fn submenu_at(&self, row: usize) -> Option<Vec<Entry>> {
        let Overlay::Menu(menu) = &self.overlay else { return None };
        match menu.entries.get(row).copied()? {
            Entry::More(_, list) => Some(Entry::shown(list, self)),
            _ => None,
        }
    }

    /// The same, for whichever row the keyboard is on.
    fn submenu_under_cursor(&self) -> Option<Vec<Entry>> {
        let Overlay::Menu(menu) = &self.overlay else { return None };
        self.submenu_at(menu.cursor?)
    }

    /// The pointer has arrived on a row of the open *submenu*. Only the
    /// keyboard highlight moves — nothing opens a third list off a second
    /// one, so arrival has nothing else to decide. See [`Menu::hover_sub`].
    pub fn hover_submenu(&mut self, row: usize, cx: &mut Context<Self>) {
        if let Overlay::Menu(menu) = &mut self.overlay {
            if menu.hover_sub(row) {
                cx.notify();
            }
        }
    }

    // -----------------------------------------------------------------------
    // Getting things onto the board
    // -----------------------------------------------------------------------

    /// Take files somebody dropped on the window.
    ///
    /// A folder brings what is *directly* in it and nothing deeper — see
    /// [`import::walk`] for why.
    ///
    /// **Nothing here touches a disk on the thread that draws.** Reading a
    /// folder of photographs is a `read` and a SHA-256 and a header decode per
    /// file, which for three hundred of them is seconds; doing that between two
    /// frames is a window that has stopped answering, and a window that has
    /// stopped answering is indistinguishable from a broken one. So the whole
    /// walk-read-classify pass runs on the background executor and posts what it
    /// has back over a channel, and the cards land in batches as they turn up.
    /// The wait is the same length. The difference is that you can see it going,
    /// carry on working over it, and press Escape if the folder was the wrong one.
    ///
    /// The channel is unbounded, deliberately. Bytes in flight are bounded in
    /// practice by the drain below keeping up, and the ceiling is the one this
    /// has always had: every file in the drop, which is what ends up in the
    /// archive regardless.
    pub fn take_files(&mut self, paths: &[PathBuf], at: WorldPoint, cx: &mut Context<Self>) {
        let paths = paths.to_vec();
        let token = self.imports;

        // Join the step already open where a drop is still arriving, rather than
        // opening a second one. See [`Importing`] for why there can only be one.
        match &mut self.importing {
            Some(importing) => importing.drops += 1,
            None => {
                self.importing = Some(Importing {
                    open: self.doc.board.start(),
                    token,
                    drops: 1,
                    found: 0,
                    done: 0,
                    parted: 0,
                    unreadable: Vec::new(),
                    heavy: Vec::new(),
                    described: None,
                    placed: Vec::new(),
                    ours: true,
                })
            }
        }
        self.hint(Some("reading…".into()));

        let (arrived, incoming) = std::sync::mpsc::channel::<Arriving>();
        cx.background_executor()
            .spawn(async move {
                let files = import::walk(&paths);
                // A send that fails is a receiver that has gone: the window
                // closed, or the drop was called off. Either way there is
                // nobody left to read the rest, so stop reading it.
                if arrived.send(Arriving::Found(files.len())).is_err() {
                    return;
                }
                for path in &files {
                    let name = path
                        .file_name()
                        .map(|n| n.to_string_lossy().to_string())
                        .unwrap_or_default();
                    let message = match std::fs::read(path) {
                        Ok(bytes) => {
                            let file = import::ready(&name, bytes);
                            // The ceiling reports; this is the layer that
                            // decides, and what it decides is to land the
                            // card anyway and say so by name — see the note
                            // at the top of `import.rs`: too large is worth
                            // asking about, not a limit. So the warning goes
                            // out first, and the card that earned it follows
                            // right behind it rather than instead of it.
                            if file.is_heavy() {
                                let warning = Arriving::Heavy(format!(
                                    "{name} is {} MB — the board will be slow to send",
                                    file.megabytes()
                                ));
                                if arrived.send(warning).is_err() {
                                    return;
                                }
                            }
                            Arriving::Ready(Box::new(file))
                        }
                        Err(_) => Arriving::Unreadable(format!("{name} could not be read")),
                    };
                    if arrived.send(message).is_err() {
                        return;
                    }
                }
            })
            .detach();

        cx.spawn(async move |view, cx| {
            use std::sync::mpsc::TryRecvError;

            // This drop's own share of the layout, which is why it lives here
            // rather than on the view: two folders dropped a second apart are
            // two blocks in two places, and only the task reading one of them
            // knows which block its files belong to.
            let mut across = 1.0_f32;
            let mut taken = 0usize;

            loop {
                // Everything waiting, not one message: a folder of small files
                // reads far faster than a frame, and taking them one at a time
                // would make the drain the slow part.
                let mut news: Vec<Arriving> = Vec::new();
                let disconnected = loop {
                    match incoming.try_recv() {
                        Ok(message) => news.push(message),
                        Err(TryRecvError::Empty) => break false,
                        Err(TryRecvError::Disconnected) => break true,
                    }
                };

                let mut found = None;
                let mut batch = Vec::new();
                let mut unreadable = Vec::new();
                let mut heavy = Vec::new();
                for message in news {
                    match message {
                        // Before any file can arrive, so `across` is settled by
                        // the time the first card needs a place to go.
                        Arriving::Found(n) => {
                            across = import::across(n);
                            found = Some(n);
                        }
                        Arriving::Ready(file) => {
                            batch.push((import::spot(at, across, taken), *file));
                            taken += 1;
                        }
                        Arriving::Unreadable(why) => unreadable.push(why),
                        Arriving::Heavy(why) => heavy.push(why),
                    }
                }

                let wanted = view
                    .update(cx, |view, cx| view.arrive(token, found, batch, unreadable, heavy, cx))
                    .unwrap_or(false);
                // The view has gone, or this drop has been called off. Dropping
                // the receiver is what tells the reader to stop.
                if !wanted {
                    return;
                }
                if disconnected {
                    break;
                }
                cx.background_executor().timer(ARRIVE_EVERY).await;
            }

            view.update(cx, |view, cx| view.settle_import(token, cx)).ok();
        })
        .detach();
    }

    /// Put what has just been read onto the board, mid-drop.
    ///
    /// Answers whether this drop is still wanted, which is the only way the task
    /// doing the reading finds out that it is not.
    ///
    /// Through [`mbrd_core::state::BoardState::during`] rather than `edit`, so
    /// that forty batches are one step rather than forty. The step is closed by
    /// [`Self::settle_import`] when the last drop lands.
    fn arrive(
        &mut self,
        token: u64,
        found: Option<usize>,
        batch: Vec<(WorldPoint, import::Ready)>,
        unreadable: Vec<String>,
        heavy: Vec<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(importing) = &mut self.importing else { return false };
        if importing.token != token {
            return false;
        }
        importing.found += found.unwrap_or(0);
        importing.unreadable.extend(unreadable);
        importing.heavy.extend(heavy);
        if batch.is_empty() {
            // A tick with nothing on it must not repaint. The line only changes
            // when a card lands or when the walk finally says how many there
            // are, and a slow read would otherwise be thirty frames a second of
            // the whole board for a sentence that has not moved.
            if found.is_some() {
                self.show_import();
                cx.notify();
            }
            return true;
        }

        // Once for the batch rather than once per card, which is the difference
        // between a pass over the board per file and a pass per frame.
        let mut z = self.top_z();
        let mut cards = Vec::with_capacity(batch.len());
        let mut ids = Vec::with_capacity(batch.len());
        let mut described = None;
        for (spot, file) in batch {
            z += 1.0;
            let card = import::card(&file, self.fresh_id_from(cards.len()), spot, z);
            // Content-addressed, so a photograph already on the board is not
            // stored twice — the second card simply names the same hash. The
            // bytes go straight in rather than through the mutation door: an
            // asset is additive by construction and there is nothing to undo.
            self.doc.assets.entry(file.hash.clone()).or_insert(file.asset);
            described = Some(file.described);
            ids.push(card.id.clone());
            cards.push(card);
        }

        let Some(importing) = &mut self.importing else { return false };
        importing.done += cards.len();
        importing.described = described;
        importing.placed.extend(ids);
        // Whether the cards this drop has brought are still what is selected. A
        // press anywhere else ends that, and it never resumes — see `Importing`.
        // The first batch takes the selection whatever was selected before it,
        // which is what every other way of adding a card does too.
        let before = &importing.placed[..importing.placed.len() - cards.len()];
        importing.ours = importing.ours && (before.is_empty() || self.selection == *before);
        let ours = importing.ours;
        let placed = ours.then(|| importing.placed.clone());
        let open = importing.open.clone();

        self.doc.board.during(&open, |board| board.items.extend(cards));
        if let Some(placed) = placed {
            self.selection = placed;
        }
        self.show_import();
        cx.notify();
        true
    }

    /// The line that says how far the drop has got.
    ///
    /// A mode rather than a completion, because it is describing where you are
    /// rather than something that finished — and, like every mode line in this
    /// app, it names the key that leaves it.
    fn show_import(&mut self) {
        let Some(importing) = &self.importing else { return };
        let (done, found) = (importing.done, importing.found);
        self.hint(Some(match found {
            0 => "reading…".into(),
            _ => format!("adding {done} of {found} — escape to stop"),
        }));
    }

    /// One drop has finished arriving. Close the step when it was the last.
    fn settle_import(&mut self, token: u64, cx: &mut Context<Self>) {
        let Some(importing) = &mut self.importing else { return };
        if importing.token != token {
            return;
        }
        importing.drops -= 1;
        if importing.drops > 0 {
            return;
        }
        let importing = self.importing.take().expect("read just above");
        let done = importing.done;
        self.doc.board.finish(&Self::add_label(done - importing.parted), importing.open);

        // What one file was, by name, because a single drop is usually somebody
        // checking whether this app knows what their file is.
        let alone = (done == 1).then_some(importing.described).flatten();
        let refused = !importing.unreadable.is_empty();
        let mut message = match (done, alone, refused) {
            (0, _, false) => "nothing to add".into(),
            (1, Some(what), false) => format!("added {what}"),
            (n, _, false) => format!("added {n}"),
            (0, _, true) => Self::refusal_summary(&importing.unreadable),
            (n, _, true) => format!("added {n}; {}", Self::refusal_summary(&importing.unreadable)),
        };
        // Heavy files are not a refusal — they landed like anything else, see
        // the module note at the top of `import.rs` — so this is appended
        // rather than folded into the branches above, and it is what tips an
        // otherwise ordinary "added n" into a message worth standing until
        // it is read.
        if !importing.heavy.is_empty() {
            message = format!("{message}; {}", importing.heavy.join("; "));
        }
        // A drop that left files behind, or landed one large enough to slow
        // down the next send, is not a plain success — somebody dropped a
        // folder and part of it silently would not be here without this
        // line, so it gets the tone that stands until it is read rather than
        // the one that would let it slide by with everything else that just
        // finished.
        if refused || !importing.heavy.is_empty() {
            self.warn(message);
        } else {
            self.say(message);
        }
        cx.notify();
    }

    /// One sentence for the files a drop could not read at all.
    ///
    /// Kept as its own function because `settle_import` already has five
    /// tuples to match on, and folding the wording in would make the one
    /// place that decides *whether* to speak also the place squinting at
    /// *what* to say.
    fn refusal_summary(unreadable: &[String]) -> String {
        if unreadable.is_empty() {
            String::new()
        } else {
            format!("could not read: {}", unreadable.join(", "))
        }
    }

    /// Call off whatever is still arriving. Answers whether there was anything.
    ///
    /// What has already landed stays. It is one step and one press of Ctrl Z
    /// away, which is a better answer than throwing away work somebody has
    /// already watched arrive — and the thing they are usually stopping is the
    /// *rest* of a folder they did not mean to drop.
    ///
    /// The tasks still reading are not stopped so much as disowned: the token
    /// moves on, the next thing they send lands nowhere, and dropping the
    /// receiver closes the channel under them at the next file.
    pub fn stop_importing(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(importing) = self.importing.take() else { return false };
        self.imports = self.imports.wrapping_add(1);
        let done = importing.done;
        self.doc.board.finish(&Self::add_label(done - importing.parted), importing.open);
        self.say(match done {
            0 => "stopped — nothing added".into(),
            n => format!("stopped — added {n}"),
        });
        cx.notify();
        true
    }

    /// Close the step a drop still arriving has been writing into, and open a
    /// fresh one for the rest of it.
    ///
    /// **There is one shadow behind the mutation door and therefore one open
    /// gesture** — see [`mbrd_core::state::BoardState::start`]. A drop is the
    /// only thing in this app that holds one open across seconds rather than
    /// across a drag, so anything somebody does in the meantime would otherwise
    /// fold the cards landed so far into *its* step and call the result "Move"
    /// or "Rename". Splitting the drop in two is the smaller lie: both halves
    /// say Add, and both take back exactly what they say.
    ///
    /// Called from the two funnels every input goes through and nowhere else,
    /// which is also what makes it free on a board with no drop arriving. A
    /// part with nothing in it records nothing — see `BoardState::finish` — so
    /// clicking about before the first card lands costs one board diff and no
    /// entry on the strip.
    fn part_import(&mut self) {
        // Nothing has landed since the last part, so the gesture that is open
        // holds nothing of this drop's and there is nothing for the next thing
        // somebody does to swallow. Less an optimisation than the whole
        // condition: closing a step measures the board against its shadow, and
        // paying for that on every key press is not something to do for a drop
        // that has not moved since the last one.
        if !self.importing.as_ref().is_some_and(|i| i.done > i.parted) {
            return;
        }
        // Opened before the borrow below, because it reads the board.
        let next = self.doc.board.start();
        let Some(importing) = &mut self.importing else { return };
        let since = importing.done - importing.parted;
        importing.parted = importing.done;
        let open = std::mem::replace(&mut importing.open, next);
        self.doc.board.finish(&Self::add_label(since), open);
    }

    /// What the strip calls a drop of `count` files.
    fn add_label(count: usize) -> String {
        if count == 1 {
            "Add".to_string()
        } else {
            format!("Add {count}")
        }
    }

    /// Take whatever is on the clipboard.
    ///
    /// An image becomes a picture, an address becomes what it points at, and
    /// anything else becomes a note — which is the order somebody would guess,
    /// and the reason [`import::as_url`] is deliberately strict about what an
    /// address is.
    pub fn paste(&mut self, cx: &mut Context<Self>) {
        self.paste_from(false, cx);
    }

    /// The same, without following anything.
    ///
    /// The escape hatch for the one place a paste does something more than put
    /// the clipboard down: an address that points at a picture or a video is
    /// fetched and becomes that picture or that video, and this is how to say
    /// you meant the address itself. Everything else on the clipboard pastes
    /// identically either way — there is nothing else this build goes and
    /// looks up — so the two keys differ in exactly one case, which is the
    /// case somebody reaching for `Ctrl Shift V` has in mind.
    pub fn paste_raw(&mut self, cx: &mut Context<Self>) {
        self.paste_from(true, cx);
    }

    /// The whole of both. `raw` means a link stays a link.
    fn paste_from(&mut self, raw: bool, cx: &mut Context<Self>) {
        // The app's own cards first. See `paste_cards` for why one key does
        // both and in this order.
        if self.paste_cards(cx) {
            return;
        }
        let Some(item) = cx.read_from_clipboard() else {
            self.tell("nothing on the clipboard".into());
            cx.notify();
            return;
        };
        let at = self.viewport.pan;

        let images: Vec<Vec<u8>> = item
            .entries()
            .iter()
            .filter_map(|entry| match entry {
                gpui::ClipboardEntry::Image(image) => Some(image.bytes.clone()),
                gpui::ClipboardEntry::String(_) => None,
            })
            .collect();
        if !images.is_empty() {
            // Readied off the thread that draws, the same as a dropped file:
            // `import::ready` hashes every byte and reads the picture's header,
            // and a pasted screenshot is tens of megabytes of raw pixels — done
            // here it froze the window for the length of a SHA-256 over all of
            // them, inside the keystroke.
            self.hint(Some("reading…".into()));
            let readying = cx.background_executor().spawn(async move {
                images
                    .into_iter()
                    // No name and no extension, deliberately: a pasted picture
                    // has neither, and `import::classify` reads the bytes
                    // anyway.
                    .map(|bytes| import::ready("pasted", bytes))
                    .collect::<Vec<import::Ready>>()
            });
            cx.spawn(async move |view, cx| {
                let pictures = readying.await;
                view.update(cx, |view, cx| {
                    view.hint(None);
                    // At where the camera was when the paste was asked for,
                    // not where it has drifted to since.
                    let n = view.place(pictures, at);
                    view.say(format!("pasted {n}"));
                    cx.notify();
                })
                .ok();
            })
            .detach();
            return;
        }

        let Some(text) = item.text().filter(|t| !t.trim().is_empty()) else {
            self.tell("nothing on the clipboard".into());
            cx.notify();
            return;
        };

        let id = self.fresh_id();
        let z = self.top_z() + 1.0;
        // A colour on the clipboard is a swatch. Checked before an address and
        // before words, because `#3a5f2c` is not a sentence anybody meant to
        // keep as one.
        if let Some(hex) = tidy_hex(&text) {
            let mut swatch = Item::new(id.clone(), ItemType::Swatch);
            swatch.name = hex.to_uppercase();
            swatch.meta.insert("hex".into(), serde_json::Value::String(hex));
            swatch.w = 160.0;
            swatch.h = 160.0;
            swatch.x = at.x;
            swatch.y = at.y;
            swatch.z = z;
            self.doc.board.edit("Paste", |board| board.items.push(swatch));
            self.select_only(&id);
            self.say("pasted a color".into());
            cx.notify();
            return;
        }

        if let Some(url) = import::as_url(&text) {
            // An address that points at a file is that file. A link card is
            // what is left when it points at a page, when the fetch fails, or
            // when somebody said they meant the address — see `paste_raw`.
            if !raw && fetch::worth_trying(url) {
                self.fetch_onto(url.to_string(), at, cx);
                return;
            }
            self.put_link(&id, url, at, z);
            self.say("pasted".into());
            cx.notify();
            return;
        }

        let mut card = Item::new(id.clone(), ItemType::Note);
        let words: String = text.trim().chars().take(mbrd_core::model::NOTE_MAX).collect();
        card.name = "note".into();
        card.w = 260.0;
        card.h = 200.0;
        card.meta.insert("text".into(), serde_json::Value::String(words));
        card.x = at.x;
        card.y = at.y;
        card.z = z;
        self.doc.board.edit("Paste", |board| board.items.push(card));
        self.select_only(&id);
        self.say("pasted".into());
        cx.notify();
    }

    /// Put a link card down at `at`, and take hold of it.
    ///
    /// Its own function because two paths reach it: the paste that decided not
    /// to follow the address, and the fetch that tried and could not. Both have
    /// to leave the same card, or a failed download would be a paste that
    /// quietly lost what was on the clipboard.
    fn put_link(&mut self, id: &str, url: &str, at: WorldPoint, z: f32) {
        let mut card = Item::new(id.to_string(), ItemType::Link);
        card.name = url.to_string();
        card.w = 300.0;
        card.h = 96.0;
        card.meta.insert("url".into(), serde_json::Value::String(url.to_string()));
        card.x = at.x;
        card.y = at.y;
        card.z = z;
        self.doc.board.edit("Paste", |board| board.items.push(card));
        self.select_only(id);
    }

    /// Go and get what an address points at, and put *that* on the board.
    ///
    /// Two hops off the main thread and back, for the same reason the image
    /// paste above takes one: `fetch::embed` blocks on a socket for as long as
    /// the other end takes, and `import::ready` hashes every byte it returns.
    /// Neither belongs inside a keystroke.
    ///
    /// Nothing is placed until the bytes are in, so a slow fetch is a status
    /// line rather than a placeholder card that changes shape underneath
    /// somebody — and one step in the history either way, because a paste is
    /// one thing somebody did.
    fn fetch_onto(&mut self, url: String, at: WorldPoint, cx: &mut Context<Self>) {
        self.hint(Some("fetching…".into()));
        cx.notify();
        let fetching = cx.background_executor().spawn({
            let url = url.clone();
            async move { fetch::embed(&url).map(|got| import::ready(&got.name, got.bytes)) }
        });
        cx.spawn(async move |view, cx| {
            let got = fetching.await;
            view.update(cx, |view, cx| {
                view.hint(None);
                match got {
                    Ok(file) => {
                        let described = file.described;
                        // At where the camera was when the paste was asked
                        // for, not where it has drifted to since.
                        view.place(vec![file], at);
                        view.say(format!("pasted {described}"));
                    }
                    Err(why) => {
                        // The address is still worth having, so it lands as a
                        // link — and the reason is said out loud, because a
                        // paste that silently made a different card than the
                        // one before it would be a paste nobody can predict.
                        let id = view.fresh_id();
                        let z = view.top_z() + 1.0;
                        view.put_link(&id, &url, at, z);
                        view.tell(format!("{why} — pasted the link"));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Put a batch of prepared files on the board, laid out around a point.
    ///
    /// One step for the whole batch rather than one per file: a paste is one
    /// thing somebody did, and undoing it should be one press. The bytes go
    /// straight into the archive — assets are not behind the mutation door,
    /// because they are content-addressed and adding one can only ever be
    /// additive. Only the cards go through the door.
    ///
    /// For the clipboard, which arrives already in memory and all at once. A
    /// drop goes through [`Self::take_files`] instead, which lays cards out
    /// with the same two helpers as they are read off the disk.
    fn place(&mut self, files: Vec<import::Ready>, at: WorldPoint) -> usize {
        if files.is_empty() {
            return 0;
        }
        let count = files.len();
        let mut z = self.top_z();
        let mut cards = Vec::with_capacity(count);

        let across = import::across(count);
        for (i, file) in files.into_iter().enumerate() {
            z += 1.0;
            let card = import::card(&file, self.fresh_id_from(i), import::spot(at, across, i), z);
            // Content-addressed, so a photograph already on the board is not
            // stored twice — the second card simply names the same hash.
            self.doc.assets.entry(file.hash.clone()).or_insert(file.asset);
            cards.push(card);
        }

        let ids: Vec<String> = cards.iter().map(|c| c.id.clone()).collect();
        self.doc.board.edit(&Self::add_label(count), |board| board.items.extend(cards));
        self.selection = ids;
        count
    }

    /// Copy the selected text out of a card, and cut it if asked.
    fn copy_text(&mut self, cut: bool, cx: &mut Context<Self>) {
        let Some(open) = &self.editing else { return };
        let Some(text) = open.editor.selected_text() else { return };
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.to_string()));
        if cut {
            if let Some(open) = &mut self.editing {
                open.editor.insert("");
            }
            self.show_edit();
        }
        cx.notify();
    }

    /// Put the clipboard's text into the card being edited.
    fn paste_text(&mut self, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) else { return };
        if let Some(open) = &mut self.editing {
            open.editor.insert(&text);
        }
        self.show_edit();
        cx.notify();
    }

    // -----------------------------------------------------------------------
    // Typing into a card
    // -----------------------------------------------------------------------

    /// Open a card for typing.
    ///
    /// A note gets its words and everything else gets its name — which is the
    /// only thing there is to type on a photograph, and the thing somebody
    /// double-clicking one almost certainly wants.
    pub fn start_editing(&mut self, id: &str, cx: &mut Context<Self>) {
        self.edit_card(id, false, cx);
    }

    /// The same, with a say in where the caret lands.
    ///
    /// `whole` selects the words that are already there, so that typing
    /// replaces them. That is what a card *name* always does — see below — and
    /// what a note wants only when the words in it are a placeholder nobody
    /// wrote.
    fn edit_card(&mut self, id: &str, whole: bool, cx: &mut Context<Self>) {
        // One at a time. Opening a second session would drop the first's
        // `Pending` unfinished, which joins its mutation to whatever step comes
        // next — the one way this door can blur two edits into one.
        self.stop_editing(true, cx);
        let Some(item) = self.doc.board.item(id) else { return };
        let (field, before, limit, multiline) = match item.kind {
            ItemType::Note | ItemType::Text => (
                Field::Note,
                item.note_text().unwrap_or_default().to_string(),
                mbrd_core::model::NOTE_MAX,
                true,
            ),
            _ => (Field::Name, item.name.clone(), mbrd_core::model::NOTE_MAX, false),
        };
        // A name opens selected, so typing replaces it; a note opens with the
        // caret at the end, so typing continues it. Which is what each of them
        // is usually for.
        let editor = match field {
            Field::Name | Field::Url => Editor::selecting_all(before.clone(), limit, multiline),
            Field::Note if whole => Editor::selecting_all(before.clone(), limit, multiline),
            Field::Note => Editor::new(before.clone(), limit, multiline),
        };
        self.rope = None;
        self.editing = Some(Editing {
            on: Subject::Card(id.to_string(), field),
            editor,
            before,
            file: false,
            open: self.doc.board.start(),
        });
        self.hint(Some(hint_for(field)));
        cx.notify();
    }

    /// Open the selected connection's label for typing.
    ///
    /// One line, always: a label is a word on a rope, not a paragraph, and the
    /// format holds it to sixty characters with its whitespace collapsed. So
    /// Enter commits rather than breaking a line, which is the same bargain a
    /// card's *name* makes and for the same reason.
    pub fn start_labelling(&mut self, cx: &mut Context<Self>) {
        self.stop_editing(true, cx);
        let Some((a, b)) = self.rope.clone() else { return };
        let before = rope::between(&self.doc.board, &a, &b)
            .and_then(|c| c.meta.label.clone())
            .unwrap_or_default();
        self.editing = Some(Editing {
            on: Subject::Rope(a, b),
            editor: Editor::selecting_all(before.clone(), LABEL_MAX, false),
            before,
            file: false,
            open: self.doc.board.start(),
        });
        self.hint(Some("labeling — enter to keep, escape to put it back".into()));
        cx.notify();
    }

    /// End an edit, keeping what was typed or putting back what was there.
    pub fn stop_editing(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some(open) = self.editing.take() else { return };
        let typed = open.editor.text().to_string();
        let on = open.on.clone();
        let label = match &open.on {
            Subject::Card(_, Field::Name) => "Rename",
            Subject::Card(_, Field::Note) => "Edit note",
            Subject::Card(_, Field::Url) => "Set address",
            Subject::Rope(..) => "Label",
        };
        // The mode line names its own escape key precisely because it would
        // otherwise stand forever — see `Tone::Mode` — so leaving it is what
        // has to take it down. Done first and unconditionally, rather than
        // folded into the branches below: `say` would refuse to overwrite it
        // anyway (a mode line is `shown()`), so whatever it is guarding would
        // silently lose to a bar still reading "renaming — …".
        self.hush();

        if keep || typed == open.before {
            // Either the typing is being kept, or Escape is putting back text
            // that was never touched — a revert that changes nothing, which
            // is exactly what should record nothing: not a step that undoes
            // another, but no step at all.
            let text = if keep { typed } else { open.before.clone() };
            self.commit_edit(&on, open.file, &text, &open.open);
            if self.doc.board.finish(label, open.open) {
                self.say(if keep { "changed".into() } else { "put back".into() });
            }
        } else {
            // Escape is about to throw away real typing, and a revert that
            // records nothing would throw it away *beyond undo* — a
            // paragraph typed and then reconsidered is simply gone, with
            // nothing for Ctrl+Z to find. So the typed text is committed as
            // its own step first, putting it somewhere undo can see it, and a
            // second step immediately puts `before` back on top of it. From
            // the chair in front of the screen Escape still just puts the
            // text back; underneath, one Ctrl+Z now does exactly what it
            // looks like it should and brings the typing back.
            self.commit_edit(&on, open.file, &typed, &open.open);
            self.doc.board.finish(label, open.open);
            let discard = self.doc.board.start();
            self.commit_edit(&on, open.file, &open.before, &discard);
            self.doc.board.finish("Discard", discard);
            self.say("put back".into());
        }
        cx.notify();
    }

    /// Write a finished edit, whichever of the two texts it was.
    ///
    /// One function rather than a branch at each of the three call sites in
    /// [`Self::stop_editing`], because the third of those is the one that puts
    /// discarded typing into the history where Ctrl Z can find it — and a
    /// commit path that only two of the three took would lose a file's worth of
    /// typing on Escape.
    fn commit_edit(&mut self, on: &Subject, file: bool, text: &str, token: &Pending) {
        match (file, on) {
            (true, Subject::Card(id, _)) => {
                let id = id.clone();
                self.write_file(&id, text, token);
            }
            _ => {
                let on = on.clone();
                let measure = self.measure.clone();
                self.doc.board.during(token, |board| write_to(board, &on, text, &measure));
            }
        }
    }

    /// Put the text as it stands onto the card, without ending the edit.
    ///
    /// Through the open gesture, so nothing is recorded — the whole session is
    /// one step, closed by [`Self::stop_editing`]. This runs on every keystroke
    /// and is what makes the card show what is being typed into it.
    fn show_edit(&mut self) {
        let Some(open) = &self.editing else { return };
        let on = open.on.clone();
        // A file's live preview is only what the card can hold. The bytes are
        // written once, at the commit — hashing two hundred thousand
        // characters on every keystroke would be a text field that got slower
        // the more there was in it, for a picture nobody is looking at while
        // the page in front of them already shows the whole thing.
        let text = match open.file {
            true => open.editor.text().chars().take(mbrd_core::model::NOTE_MAX).collect(),
            false => open.editor.text().to_string(),
        };
        let token = open.open.clone();
        let measure = self.measure.clone();
        self.doc.board.during(&token, |board| write_to(board, &on, &text, &measure));
    }

    /// Move the caret to where somebody clicked.
    ///
    /// The one part of editing that genuinely needs a font: which character a
    /// point is nearest depends on how the text was shaped, so the answer comes
    /// from the same text system that drew it. Everything else about the caret
    /// is in `editor.rs`, without a window.
    fn place_caret(&mut self, at: gpui::Point<Pixels>, extend: bool, window: &mut Window) {
        let Some(offset) = self.caret_at(at, window) else { return };
        if let Some(open) = &mut self.editing {
            open.editor.place(offset, extend);
        }
    }

    /// Which byte of the open editor's text a point lands on.
    ///
    /// Split out from [`Self::place_caret`] because a double-click needs the
    /// offset without moving the caret to it — it selects the run around it
    /// instead, and asking twice would be asking a question with a font in it
    /// twice per press.
    fn caret_at(&self, at: gpui::Point<Pixels>, window: &mut Window) -> Option<usize> {
        let open = self.editing.as_ref()?;
        // A rope's label is not on a card, so there is no card-local geometry
        // to turn a click into a character. It is one short line and the arrow
        // keys reach all of it.
        let (id, _) = open.on.card()?;
        let item = self.doc.board.item(id)?;

        let vp = self.viewport;
        let centre = vp.to_screen(point(item.x, item.y));
        let (w, h) = ((item.w * vp.zoom).max(1.0), (item.h * vp.zoom).max(1.0));
        let (font_size, pad) = card_text(item, vp.zoom, h);
        // The editor's rows are all the body size — see the comment on the
        // render loop's own use of `leading` — so the bracket is the body
        // one, [`CARD_TEXT`] itself.
        let line_height = font_size * leading(CARD_TEXT);

        // Canvas-local, then card-local, then past the padding.
        let local_x = f(at.x) - f(self.canvas_bounds.origin.x) - (centre.x - w / 2.0) - pad;
        let local_y = f(at.y) - f(self.canvas_bounds.origin.y) - (centre.y - h / 2.0) - pad;

        // The same rows the painter drew — same width, same face, same wrap —
        // or the click and the caret would disagree about which row a pixel is
        // on.
        let rows = open.editor.wrapped(text_room(w, pad), font_size, &self.measure);
        let row =
            ((local_y / line_height).floor().max(0.0) as usize).min(rows.len().checked_sub(1)?);
        let (start, end) = rows[row];
        let line = open.editor.text()[start..end].to_string();

        let font = window.text_style().font();
        let run = TextRun {
            len: line.len(),
            font,
            color: self.theme.text,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = window.text_system().shape_line(line.into(), px(font_size), &[run], None);
        let column = shaped.closest_index_for_x(px(local_x.max(0.0)));

        // Back to an offset in the whole text: a row is a byte span, so its
        // start *is* the arithmetic.
        Some(start + column)
    }

    /// What a second, third or fourth press in the same place means.
    ///
    /// The ladder every text field has: a word, then the line it is on, then
    /// all of it. Past four it stays at all of it rather than starting again,
    /// because a person leaning on the button is not asking for anything new.
    fn select_run_at(&mut self, at: gpui::Point<Pixels>, clicks: usize, window: &mut Window) {
        let Some(offset) = self.caret_at(at, window) else { return };
        let Some(open) = &mut self.editing else { return };
        match clicks {
            2 => open.editor.select_word_at(offset),
            3 => open.editor.select_line_at(offset),
            _ => open.editor.select_all(),
        }
    }

    // -----------------------------------------------------------------------
    // The gesture pipeline
    // -----------------------------------------------------------------------

    /// The handle under the pointer, if any, and the card it belongs to.
    ///
    /// Only ever the selected cards: handles are drawn around what is selected,
    /// and something you cannot see must not be something you can press.
    /// The proportions a card should keep while a handle is being dragged.
    ///
    /// The **picture's**, where there is one and it has been decoded, rather
    /// than the card's: a photograph in a card somebody once stretched should
    /// come back to the picture's shape, not preserve the stretch. Where the
    /// bytes have not been decoded yet the card's own shape is the best guess
    /// available and is usually the same number, because that is what it was
    /// imported at. `None` for a card that is not a picture of anything — a
    /// note has no proportions it wants, so a note resizes freely.
    fn shape_of(&mut self, id: &str, start: Rect) -> Option<f32> {
        let hash = self.doc.board.item(id).and_then(picture_hash)?.to_string();
        let card = (start.height() > 0.0).then(|| start.width() / start.height());
        // The thumbnail is enough: both copies of a picture have the same
        // proportions, which is the one thing being asked for here.
        match self.images.look(&hash, 0.0) {
            // The shape is the shape whether or not it has finished arriving:
            // a resize started a tenth of a second after a decode landed must
            // keep the picture's proportions, not the card's.
            Load::Ready(image, _) => {
                let size = image.size(0);
                Some(size.width.0.max(1) as f32 / size.height.0.max(1) as f32)
            }
            _ => card,
        }
    }

    fn grip_at(&mut self, at: gpui::Point<Pixels>) -> Option<(String, Grip, Rect)> {
        if self.selection.is_empty() {
            return None;
        }
        let local = point(
            f(at.x) - f(self.canvas_bounds.origin.x),
            f(at.y) - f(self.canvas_bounds.origin.y),
        );
        // Only a card under or near the pointer can be wearing the grip the
        // pointer is on, so the candidates come off the index rather than off
        // the selection — this runs per frame and per mouse move, and walking
        // the selection with a `Board::item` scan inside it made Ctrl A cost
        // selection times cards.
        let world = self.viewport.to_world(local);
        let reach = crate::grips::REACH / self.viewport.zoom.max(0.0001);
        let mut found = Vec::new();
        self.index().in_rect(
            Rect::new(world.x - reach, world.y - reach, world.x + reach, world.y + reach),
            &mut found,
        );
        if found.is_empty() {
            return None;
        }
        let items = &self.doc.board.items;
        let near: Vec<&Item> = found.iter().map(|&i| &items[i as usize]).collect();
        // Reverse, so the topmost of two overlapping selections wins — the same
        // order the painter draws them in.
        for id in self.selection.iter().rev() {
            let Some(item) = near.iter().find(|it| it.id == *id) else { continue };
            // The untilted box: a turned card's handles are not drawn yet, and
            // offering them where they are not would be worse than not offering
            // them at all.
            // A locked card wears none either — see where `Draw::lock` is
            // decided, which is what stops one from being drawn on it.
            if item.rot != 0.0 || item.locked() {
                continue;
            }
            let box_ = Rect::centred(item.x, item.y, item.w, item.h);
            if let Some(grip) = Grip::at(local, box_, &self.viewport) {
                return Some((id.clone(), grip, box_));
            }
        }
        None
    }

    /// The words to draw beside the pointer, where a gesture has any.
    ///
    /// The half of "what the pointer says" the platform cannot do. A system
    /// cursor is a fixed set of shapes and there is no custom one to be had
    /// here, so the shape says what *kind* of thing is happening and this says
    /// what is happening to *what* — how far a card has come, how big it has
    /// got, how many are moving.
    ///
    /// `None` for the resting board. A chip following the pointer around an
    /// idle canvas would be a permanent fixture rather than feedback, and
    /// feedback that is always on is decoration.
    fn badge(&self) -> Option<String> {
        // The board's own units, so the readout agrees with the ruler somebody
        // has already calibrated. See `BoardSettings::scale`.
        let show = |v: f32| format!("{}", v.round() as i64);
        match &self.gesture {
            Gesture::Moving { start, moved: true, copied, .. } => {
                let held = start.first()?;
                let now = self.doc.board.item(&held.id)?;
                let (dx, dy) = (now.x - held.home.x, now.y - held.home.y);
                let what = if *copied { "copy" } else { "move" };
                Some(match start.len() {
                    1 => format!("{what}  {} , {}", show(dx), show(dy)),
                    n => format!("{what} {n}  {} , {}", show(dx), show(dy)),
                })
            }
            Gesture::Sizing { id, cropping, moved: true, .. } => {
                let item = self.doc.board.item(id)?;
                let what = if *cropping { "crop" } else { "size" };
                Some(format!("{what}  {} × {}", show(item.w), show(item.h)))
            }
            Gesture::Marquee { from, to, .. } => {
                let (w, h) = ((to.x - from.x).abs(), (to.y - from.y).abs());
                // Nothing until the sweep is a rectangle rather than a point,
                // or every click on the paper would flash "0 × 0".
                (w >= 1.0 || h >= 1.0).then(|| format!("{} × {}", show(w), show(h)))
            }
            Gesture::Roping { over, .. } => Some(match over {
                Some(id) => {
                    let name = self.doc.board.item(id).map(|it| it.name.as_str()).unwrap_or("");
                    if name.is_empty() {
                        "join".into()
                    } else {
                        format!("join {name}")
                    }
                }
                None => "drop on a card".into(),
            }),
            Gesture::Sliding { a, b, moved: true, .. } => {
                let along = rope::between(&self.doc.board, a, b)?.meta.label_at;
                Some(format!("label  {}%", (along * 100.0).round() as i64))
            }
            _ => None,
        }
    }

    /// What the pointer should look like where it currently is.
    ///
    /// **The order here is `on_mouse_down`'s order**, and it has to stay that
    /// way: a pointer that promises a resize where a press would start a rope
    /// is worse than no pointer change at all, because it is a promise the app
    /// then breaks. There is no test that can hold the two together — driving
    /// a press needs a window — so the order is written out the same way in
    /// both places and this note is the reason to keep it that way.
    ///
    /// This costs a hit-test per frame that the board was already paying on
    /// every mouse move, and the answer it wants is one `grip_at` and one
    /// `anchor_at` already compute and throw away.
    fn cursor_at(&mut self, at: gpui::Point<Pixels>, mods: gpui::Modifiers) -> gpui::CursorStyle {
        use gpui::CursorStyle;

        // A gesture in flight outranks everything under the pointer, because
        // during one the pointer is not *over* anything — it is holding
        // something, and what it is holding does not change until it is let go.
        match &self.gesture {
            Gesture::Panning { .. } => return CursorStyle::ClosedHand,
            // Sweeping out a text selection: the beam, the same as resting
            // over the words would give. A gesture that changed the pointer
            // half way through would read as having grabbed something else.
            Gesture::SelectingText => return CursorStyle::IBeam,
            Gesture::Sizing { grip, cropping, .. } => {
                // A crop is a reframing rather than a resize, and the two
                // gestures are the same drag with `Alt` held. Saying so with
                // the pointer is the only place the difference is visible
                // before the picture underneath starts moving.
                return if *cropping { CursorStyle::DragCopy } else { grip.cursor() };
            }
            Gesture::Roping { over, .. } => {
                // A rope over a card will take; one over paper will not, and
                // finding that out only after letting go is the thing the
                // pointer is here to prevent.
                return match over {
                    Some(_) => CursorStyle::DragLink,
                    None => CursorStyle::Crosshair,
                };
            }
            // A drag that is leaving a copy behind says so for as long as it
            // is doing it, not just for the frame `Alt` went down.
            Gesture::Moving { copied, .. } => {
                return if *copied { CursorStyle::DragCopy } else { CursorStyle::ClosedHand };
            }
            Gesture::Marquee { .. } => return CursorStyle::Crosshair,
            // Holding a label, which travels along its line rather than in any
            // one direction — so the closed hand rather than either arrow.
            Gesture::Sliding { .. } => return CursorStyle::ClosedHand,
            Gesture::Scrubbing { .. } | Gesture::Louder { .. } => {
                return CursorStyle::ResizeLeftRight
            }
            // Turning a camera is a hand closing on something, the same as
            // dragging a card.
            Gesture::Orbiting { .. } => return CursorStyle::ClosedHand,
            Gesture::None => {}
        }

        match self.tool {
            // The tool *is* the promise, over a card or not, which is the whole
            // reason a mode earns its place: it says what a press means before
            // you make one.
            Tool::Pan => return CursorStyle::OpenHand,
            Tool::Note | Tool::Connect => return CursorStyle::Crosshair,
            Tool::Select => {}
        }

        // A handle before the card it is on, and an anchor after the handle.
        // The same two lines, in the same order, as the press itself.
        if let Some((_, grip, _)) = self.grip_at(at) {
            return grip.cursor();
        }
        if self.anchor_at(at).is_some() {
            return CursorStyle::Crosshair;
        }
        // And the strip after both, which is the order the press uses. The
        // three do not overlap — `transport::INSET` is what keeps the strip
        // clear of the band the grips answer to — so this is a tie-break
        // rather than the whole answer, exactly as with the anchors.
        if let Some((_, hit)) = self.controls_at(at) {
            return match hit {
                transport::Hit::Scrub(_) | transport::Hit::Volume(_) => {
                    CursorStyle::ResizeLeftRight
                }
                _ => CursorStyle::PointingHand,
            };
        }

        let world = self.world_at(at);
        // Over the words being typed, where there are some. A caret is
        // placeable there — see `place_caret` — so the pointer says so.
        if let Some(open) = &self.editing {
            if let Some((id, _)) = open.on.card() {
                let over = self.doc.board.item(id).map(Rect::of_item);
                if over.is_some_and(|card| card.contains(world)) {
                    return CursorStyle::IBeam;
                }
            }
        }

        // A label before the card it may be lying over, which is the order the
        // press uses — and the order the painter uses, since a label is drawn
        // over everything on the board.
        if self.label_at(world).is_some() {
            return CursorStyle::OpenHand;
        }

        if let Some(id) = self.hit(world) {
            // What a press would do, before it is made — which is the whole of
            // what a pointer is for on a canvas where the same button does
            // five things.
            return if mods.alt {
                // `Alt` is about to duplicate rather than move.
                CursorStyle::DragCopy
            } else if self.enterable(&id).is_some() {
                // Inside a group that has not been entered. A press takes hold
                // of the whole group and a second one steps into it, and the
                // menu pointer is the nearest thing the platform has to "there
                // is more than one thing here".
                CursorStyle::ContextualMenu
            } else {
                // A card is dragged rather than clicked through, and the arrow
                // is what every canvas uses for "this is a thing you can take
                // hold of". A hand here would be a promise to pan.
                CursorStyle::Arrow
            };
        }
        if self.rope_at(world).is_some() {
            return CursorStyle::PointingHand;
        }
        CursorStyle::Arrow
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus_handle);
        // Whatever this press turns out to be, it is not part of a drop that is
        // still arriving. See `part_import`.
        self.part_import();
        // A shift-drag is a modifier held, not tapped. See `taps.rs`.
        self.taps.spoil();
        // Whatever the camera was doing, it is doing it no longer. A board
        // still sliding from a flick comes to hand at the pixel it is drawn
        // at, rather than finishing the slide and being grabbed afterwards —
        // and this happens *before* the world point is read, or the press
        // would land on a card that has moved on by the time it is handled.
        self.camera.seize(&self.viewport);
        let world = self.world_at(event.position);

        // A press anywhere on the board puts an open menu away, and does
        // nothing else — the click that dismisses a menu should not also be
        // the click that deselects everything behind it.
        if matches!(self.overlay, Overlay::Menu(_)) {
            self.close_menu();
            cx.notify();
            return;
        }

        if event.button == MouseButton::Right {
            // Right-clicking a card that is not selected selects it first, so
            // that "Move to bin" is about the card you actually pointed at.
            // Right-clicking one that *is* part of a selection leaves the
            // selection alone, which is the same rule the left button follows.
            match self.hit(world) {
                Some(id) => {
                    self.rope = None;
                    // Through the group rule, like the left button: a
                    // right-click on a card inside a group has to put up the
                    // menu for the group, or "Ungroup" would be about
                    // whatever happened to be selected before.
                    let id = self.selects(&id);
                    if !self.is_selected(&id) {
                        self.select_only(&id);
                    }
                }
                // Nothing under the pointer but possibly a line. Right-clicking
                // a rope has to select it, or the menu that comes up would be
                // about whatever happened to be selected before.
                None => match self.rope_at(world) {
                    Some((a, b)) => self.select_rope(&a, &b, cx),
                    // And a right-click on bare paper lets go, for the same
                    // reason: a press that was not aimed at anything should not
                    // put up a menu about the last thing that was. What comes
                    // up is the board's own list — see `command::BOARD_MENU` —
                    // and Ctrl Z puts the selection back.
                    None => self.let_go(cx),
                },
            }
            // Canvas-local, not window: the menu is absolutely positioned
            // inside the same container the canvas fills, so a window
            // coordinate would put it a titlebar's height too low.
            let local = gpui::point(
                event.position.x - self.canvas_bounds.origin.x,
                event.position.y - self.canvas_bounds.origin.y,
            );
            // Which list, decided before it is placed: a rope's menu is a
            // different height from a card's, and the flip near an edge is
            // measured against whichever one is about to be drawn.
            let entries = Entry::shown(crate::command::menu_for(self), self);
            self.open_overlay(Overlay::Menu(Menu::new(local, entries, self.canvas_bounds.size)));
            cx.notify();
            return;
        }

        // Middle-drag pans from anywhere, even over a card. It is the escape
        // hatch for a board so full there is no empty space left to grab.
        if event.button == MouseButton::Middle {
            self.gesture = Gesture::Panning { from: world, moved: false, clearing: false };
            cx.notify();
            return;
        }
        if event.button != MouseButton::Left {
            return;
        }

        let additive = event.modifiers.shift || event.modifiers.secondary();

        // A press inside the card being edited moves the caret rather than
        // starting a gesture. Anywhere else finishes the edit, which is the
        // click-away-to-commit rule every text field on a canvas has.
        if let Some(open) = &self.editing {
            let inside = open
                .on
                .card()
                .and_then(|(id, _)| self.doc.board.item(id))
                .is_some_and(|item| geometry::hit(item, world));
            if inside {
                // One press moves the caret and arms a drag; two, three and
                // four are the run, the line and the lot. `click_count` is the
                // platform's own count, so the double-click interval is the
                // one the rest of this desktop uses.
                match event.click_count {
                    0 | 1 => {
                        self.place_caret(event.position, event.modifiers.shift, window);
                        self.gesture = Gesture::SelectingText;
                    }
                    clicks => self.select_run_at(event.position, clicks, window),
                }
                cx.notify();
                return;
            }
            self.stop_editing(true, cx);
            // The Note tool stamps a note wherever it is pressed, and the press
            // that finishes writing one should not also be the press that puts
            // down the next. Every other tool carries on into the gesture
            // below, which is what click-away-to-commit means everywhere else.
            if self.tool == Tool::Note {
                cx.notify();
                return;
            }
        }

        // The tools that answer a plain press before anything else looks at it.
        // After the text field, though: a press outside an open editor commits
        // it whatever tool is in hand, or choosing the Note tool while typing
        // would put a note down and leave the edit hanging.
        //
        // Select is not in here, because Select is what the rest of this
        // function already was.
        match self.tool {
            Tool::Pan => {
                self.gesture = Gesture::Panning { from: world, moved: false, clearing: false };
                cx.notify();
                return;
            }
            Tool::Note => {
                self.add_note_at(world, cx);
                return;
            }
            Tool::Connect => {
                // A press on a card starts a rope from whichever face is
                // nearest the pointer, so the Connect tool does not also
                // require aiming at one of the four marks.
                if let Some(id) = self.hit(world) {
                    if let Some(item) = self.doc.board.item(&id) {
                        let side = nearest_side(Rect::of_item(item), world);
                        self.gesture = Gesture::Roping { from: id, side, at: world, over: None };
                        cx.notify();
                        return;
                    }
                }
            }
            Tool::Select => {}
        }

        // A handle before the card it is on: the handles stick out past the
        // edge, so a press on one would otherwise be a press on whatever is
        // behind the card.
        //
        // And an anchor after the handle, which is the tie-break `anchor.rs`
        // relies on: the two bands are kept apart by `anchor::GAP`, so the
        // order only matters where they very nearly touch.
        if let Some((id, grip, start)) = self.grip_at(event.position) {
            let open = self.doc.board.start();
            let shape = self.shape_of(&id, start);
            // Where the handle actually is, in world units, versus where the
            // press landed — the gap `hold` exists to keep the edge from
            // jumping to close. Round-tripped through screen space because
            // `Grip::spot` is the one place that already knows where a handle
            // sits, and it only speaks screen pixels.
            let anchor = self.viewport.to_world(grip.spot(start, &self.viewport));
            let hold = point(anchor.x - world.x, anchor.y - world.y);
            self.gesture = Gesture::Sizing {
                id,
                grip,
                start,
                from: world,
                hold,
                shape,
                moved: false,
                cropping: false,
                open,
            };
            cx.notify();
            return;
        }

        if let Some((from, side)) = self.anchor_at(event.position) {
            self.gesture = Gesture::Roping { from, side, at: world, over: None };
            cx.notify();
            return;
        }

        // And the strip after both. It sits *inside* the card while the other
        // two sit on and outside its edge — `transport::INSET` is what holds
        // the bands apart — so this is a tie-break rather than the whole
        // answer, and it has to come before the card itself or a press on the
        // play button would pick the card up instead.
        if let Some((id, hit)) = self.controls_at(event.position) {
            // Play/pause, mute and loop fire immediately and leave no gesture
            // behind for the painter to read a held state off of, unlike a
            // scrub or a volume drag — so this is set here, unconditionally,
            // and cleared at the release in `on_mouse_up`. Held past the
            // three buttons the painter actually washes for is harmless: it
            // is only ever read alongside a check of which `Hit` it is.
            self.pressed_control = Some((id.clone(), hit));
            self.press_control(&id, hit, cx);
            return;
        }

        // Twice on a card either steps into the group holding it or opens the
        // card onto the whole window. The two never both apply: a card you can
        // already reach is one there is no group left to enter, which is
        // exactly what `enterable` answers.
        //
        // Opening rather than typing, on every type there is. A double-click
        // means "show me this" everywhere a person has used a computer, and a
        // card is a thumbnail — so what it means here is the page. Typing is
        // what the page is *for*: double-click the words once they are shown
        // and the session opens there, where there is room for it. See
        // `opened::words` and `VIEWING.md`.
        //
        // `F2` is still the key for typing on the board itself, unchanged, and
        // `Escape` is still the way back out of a group.
        if event.click_count >= 2 {
            if let Some(id) = self.hit(world) {
                match self.enterable(&id) {
                    Some(fence) => {
                        self.enter_group(&fence, cx);
                        // And select what was actually pointed at, which is
                        // the whole reason for entering: a step in that landed
                        // on the group again would be a step that did nothing.
                        let reached = self.selects(&id);
                        self.select_only(&reached);
                    }
                    None => {
                        self.select_only(&id);
                        self.open_card(&id, cx);
                    }
                }
                cx.notify();
                return;
            }
            // And twice on a line — or on the label already sitting on one —
            // opens it for typing. The same session `Label` on its menu opens
            // and the same one `F2` opens: three ways in, one behaviour.
            if let Some((a, b)) = self.label_at(world).or_else(|| self.rope_at(world)) {
                self.select_rope(&a, &b, cx);
                self.start_labelling(cx);
                cx.notify();
                return;
            }
        }

        // A label before the card it may be lying over, because a label is
        // drawn over everything — see the painter, and `cursor_at`, which
        // promises this order before the press is made. Pressing one takes
        // hold of it; a press anywhere else on the line means the line, which
        // is the branch further down.
        if let Some((a, b)) = self.label_at(world) {
            self.select_rope(&a, &b, cx);
            // How far off-centre the press landed, so the drag can add it
            // back every frame instead of snapping the label's centre to the
            // pointer on the first one. `0.0` where either half of the sum is
            // unavailable — a wire that has not been drawn yet, or a
            // connection this exact press has already lost — which is the
            // old, snap-to-pointer behaviour rather than a crash.
            let now = rope::between(&self.doc.board, &a, &b).map(|c| c.meta.label_at);
            let pressed_at = self
                .drawn
                .iter()
                .find(|w| (w.a == a && w.b == b) || (w.a == b && w.b == a))
                .map(|w| w.how_far_along(world));
            let offset = match (now, pressed_at) {
                (Some(now), Some(pressed_at)) => now - pressed_at,
                _ => 0.0,
            };
            let open = self.doc.board.start();
            self.gesture = Gesture::Sliding { a, b, moved: false, offset, open };
            cx.notify();
            return;
        }

        // Position mode. A press elsewhere leaves it — see `Command::Position`
        // — so the check runs unconditionally rather than only while a mesh is
        // actually under the pointer, and falls through to the ordinary press
        // below on anything else, including empty paper.
        if let Some(positioning) = self.positioning.clone() {
            if self.hit(world).as_deref() == Some(positioning.as_str()) {
                self.begin_mesh_orbit(&positioning, event.position, event.modifiers.shift, cx);
                return;
            }
            self.positioning = None;
        }

        match self.hit(world) {
            Some(pressed) => {
                // What the press means, before what it does with it: a card
                // inside a group is part of a thing somebody made on purpose,
                // and pressing it means that thing. See `selects`.
                let id = self.selects(&pressed);
                // Pressing an unselected card selects it and starts a move.
                // Pressing one that is already part of a selection moves the
                // whole selection — otherwise dragging a group would silently
                // collapse it to whichever card you happened to grab.
                self.rope = None;
                if additive {
                    self.toggle_selected(&id);
                } else if !self.is_selected(&id) {
                    self.select_only(&id);
                }
                let ids = self.dragging(if self.is_selected(&id) {
                    self.selection.clone()
                } else {
                    vec![id]
                });
                // The picture, taken before a single pixel of the drag has
                // happened. Everything the pointer does from here until the
                // release is measured against it, once.
                //
                // One walk of the items rather than an `item` scan per held
                // id: a select-all drag on a big board made the press itself
                // cost selection times cards. The held cards come out in item
                // order rather than selection order, which nothing reads —
                // the delta is the same for all of them, and copies appended
                // in item order keep their stacking.
                // Nothing in hand that may move — everything pressed is
                // locked. The press still selects, because unlocking is a
                // thing you do to a locked card, but no gesture begins and no
                // step is opened on the ledger: a drag that moves nothing must
                // not be an undo somebody has to press twice to get past.
                if ids.is_empty() {
                    cx.notify();
                    return;
                }
                let open = self.doc.board.start();
                let wanted: HashSet<&str> = ids.iter().map(String::as_str).collect();
                let start = self
                    .doc
                    .board
                    .items
                    .iter()
                    .enumerate()
                    .filter(|(_, item)| wanted.contains(item.id.as_str()))
                    .map(|(index, item)| Grabbed {
                        id: item.id.clone(),
                        home: point(item.x, item.y),
                        index,
                        w: item.w,
                        h: item.h,
                    })
                    .collect();
                self.gesture = Gesture::Moving {
                    from: world,
                    start,
                    moved: false,
                    lock: None,
                    copied: false,
                    guides: Snap::default(),
                    open,
                };
            }
            None => {
                // A rope, before the empty space it is drawn over. Tested after
                // the cards rather than before them, because a line that
                // passes behind a card must not steal a press meant for it.
                if let Some((a, b)) = self.rope_at(world) {
                    self.select_rope(&a, &b, cx);
                    return;
                }
                // Empty space. A plain drag pans; a modified one sweeps out a
                // selection, which is the original's rule and the reason pan is
                // the unmodified gesture: it is the one you do constantly.
                //
                // Neither of them lets go of anything here. A press is not yet
                // a click, and what a *click* on the paper means is decided at
                // the release — see `on_mouse_up`, and `let_go` for why it has
                // to happen in one place.
                if additive {
                    // Except the rope, since a sweep is about cards and the two
                    // are never both live.
                    self.rope = None;
                    self.gesture = Gesture::Marquee {
                        from: world,
                        to: world,
                        additive: true,
                        provisional: HashSet::new(),
                    };
                    // Seeded even for this zero-area rectangle, so a release
                    // with no move in between still commits whatever a sweep
                    // of nothing catches — nothing — through the same path a
                    // dragged one does, rather than a special case for it.
                    self.update_marquee(world);
                } else {
                    self.gesture = Gesture::Panning { from: world, moved: false, clearing: true };
                }
            }
        }
        cx.notify();
    }

    pub(crate) fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let world = self.world_at(event.position);
        self.pointer = event.position;

        // Dragging out a text selection. First, because it is the one gesture
        // that is not about the board at all: the pointer may leave the card
        // and even leave every card, and the selection should follow it to the
        // end of the text rather than turn into a marquee half way.
        if matches!(self.gesture, Gesture::SelectingText) {
            self.place_caret(event.position, true, window);
            cx.notify();
            return;
        }

        if matches!(self.gesture, Gesture::None) {
            // Which card is offering anchors. Only a change is notified, or
            // every pixel of a pointer crossing an empty board would be a
            // repaint of the whole thing.
            //
            // Off the card is not off the *offer*: the marks sit outside the
            // card, so a pointer heading for one has to leave the card to
            // reach it. Anything actually under the pointer still wins — a
            // mark's band can lie over the card behind it — but empty paper
            // inside the band keeps the card that put the marks there.
            let over = self.hit(world).or_else(|| self.still_reaching(event.position));
            if over != self.hovering {
                self.hovering = over;
                cx.notify();
            }

            // The volume slider is offered by pointing at the mute button and
            // taken away by pointing anywhere else — including at the slider
            // itself, which is why a press on it counts as staying. A slider
            // that needed a click to open would need another to close, and a
            // press on a mute button already means mute.
            //
            // Falling back to `still_reaching_volume` is what keeps the gap
            // between the mute button and the slider from closing the slider
            // the moment somebody reaches across it — see that function.
            let want = match self.controls_at(event.position) {
                Some((id, transport::Hit::Mute)) | Some((id, transport::Hit::Volume(_))) => {
                    Some(id)
                }
                _ => self.still_reaching_volume(event.position),
            };
            if want != self.volume_on {
                self.volume_on = want;
                cx.notify();
            }
            return;
        }

        // The rope being drawn. Kept out of the borrow-splitting match below
        // because it wants `self.hit`, which needs the index and therefore
        // needs `self` whole.
        if let Gesture::Roping { from, .. } = &self.gesture {
            let from = from.clone();
            let over = self.hit(world).filter(|id| *id != from);
            if let Gesture::Roping { at, over: slot, .. } = &mut self.gesture {
                *at = world;
                *slot = over;
            }
            cx.notify();
            return;
        }

        // Dragging a label along its line. Out here with `Roping` rather than
        // in the borrow-splitting match below, because it reads `self.drawn` —
        // the lines as they were last drawn — and then writes the board.
        if let Gesture::Sliding { a, b, offset, .. } = &self.gesture {
            let (a, b, offset) = (a.clone(), b.clone(), *offset);
            // Asked of the line as it is *drawn*, so a label follows the
            // pointer around a detour rather than along the straight line
            // between the two cards it joins. `offset` is added back here
            // rather than baked into the press, so an off-centre grab keeps
            // the same offset to the pointer for the whole drag instead of
            // narrowing back to zero — see `Gesture::Sliding::offset`.
            let along = self
                .drawn
                .iter()
                .find(|w| (w.a == a && w.b == b) || (w.a == b && w.b == a))
                .map(|w| (w.how_far_along(world) + offset).clamp(0.0, 1.0));
            let now = rope::between(&self.doc.board, &a, &b).map(|c| c.meta.label_at);
            // Only where it actually goes somewhere. `how_far_along` answers
            // with the nearest sampled vertex, so a hand that has moved a few
            // pixels within one of them gets the same fraction back — and
            // writing it again would turn a click into a recorded move.
            if let (Some(along), Some(now)) = (along, now) {
                if (along - now).abs() > f32::EPSILON {
                    self.slide_label(&a, &b, along);
                    if let Gesture::Sliding { moved, .. } = &mut self.gesture {
                        *moved = true;
                    }
                }
            }
            cx.notify();
            return;
        }

        // Dragging a scrubber or a slider. Kept out of the borrow-splitting
        // match below because both want `drawn_controls`, which is not one of
        // the four fields it takes — and because a scrub writes to the media
        // rather than to the board.
        match &self.gesture {
            Gesture::Scrubbing { id } => {
                let id = id.clone();
                let local = point(
                    f(event.position.x) - f(self.canvas_bounds.origin.x),
                    f(event.position.y) - f(self.canvas_bounds.origin.y),
                );
                if let Some(drawn) = self.drawn_control(&id) {
                    let (fraction, length) = (drawn.strip.scrub.fraction(local.x), drawn.length);
                    self.media.seek(&id, fraction, length);
                }
                cx.notify();
                return;
            }
            Gesture::Louder { id, .. } => {
                let id = id.clone();
                let local = point(
                    f(event.position.x) - f(self.canvas_bounds.origin.x),
                    f(event.position.y) - f(self.canvas_bounds.origin.y),
                );
                // Against the slider that was drawn, so the level follows the
                // pointer past either end of the track rather than stopping
                // dead the moment it leaves it.
                if let Some(slider) = self.drawn_control(&id).and_then(|drawn| drawn.volume) {
                    let level = slider.fraction(local.x);
                    self.drag_volume(&id, level);
                }
                cx.notify();
                return;
            }
            Gesture::Orbiting { id, from, start, panning, open, .. } => {
                let (id, from, start, panning, open) =
                    (id.clone(), *from, *start, *panning, open.clone());
                // Screen pixels since the press, not since the last frame —
                // the same reason `Sizing` measures from its own `start`
                // rather than accumulating a delta every frame would.
                let dx = f(event.position.x) - f(from.x);
                let dy = f(event.position.y) - f(from.y);
                let next = if panning {
                    // Fractions of the mesh's own span per pixel. Signed the
                    // opposite way turning is: the look-at point moves
                    // against the drag so the picture itself follows the
                    // pointer, the same feel a pan tool anywhere else has.
                    const PAN_UNITS_PER_PIXEL: f32 = 0.004;
                    start.panned(-dx * PAN_UNITS_PER_PIXEL, dy * PAN_UNITS_PER_PIXEL)
                } else {
                    // Degrees per pixel. A drag most of the way across the
                    // window turns the mesh about half way round, which reads
                    // as direct without being able to spin past the angle you
                    // meant.
                    const DEGREES_PER_PIXEL: f32 = 0.35;
                    start.turned(dx * DEGREES_PER_PIXEL, -dy * DEGREES_PER_PIXEL)
                };
                self.doc.board.during(&open, |board| {
                    if let Some(item) = board.item_mut(&id) {
                        mbrd_core::media::set_orbit(item, next);
                    }
                });
                if let Gesture::Orbiting { moved, .. } = &mut self.gesture {
                    *moved = true;
                }
                self.live_orbit_frame(&id, next, cx);
                cx.notify();
                return;
            }
            _ => {}
        }

        // Dragging cards. Kept out of the borrow-splitting match below because
        // two of the three things it does want `self` whole: the guides ask the
        // index where everything else on the board is, and an Alt-drag puts new
        // items down rather than moving existing ones.
        if matches!(self.gesture, Gesture::Moving { .. }) {
            self.drag_cards(world, event.modifiers, cx);
            return;
        }

        // Sweeping a marquee. Kept out of the borrow-splitting match below
        // for the same reason: the preview has to run the same in_rect+pick
        // pass `on_mouse_up` commits at release, which asks the index, the
        // fences and `self.inside` for their say — none of them among the
        // four fields split out below, and none of them worth cloning once
        // a frame just to keep the match uniform.
        if matches!(self.gesture, Gesture::Marquee { .. }) {
            self.update_marquee(world);
            return;
        }

        // Split into fields rather than borrowed whole. The gesture holds the
        // picture the drag is measured against, and the board is the thing that
        // moves; they are different fields, and saying so is what lets the
        // handler reach both without copying either. Borrowing `self` for the
        // match and then reaching for `self.doc` inside it does not compile, and
        // the tempting fix — cloning the gesture — would copy a picture of the
        // whole board on every frame of every drag, which is exactly the cost
        // the gesture exists to avoid.
        let shift = event.modifiers.shift;
        let alt = event.modifiers.alt;
        let free = event.modifiers.secondary() || event.modifiers.control;
        let Self { doc, gesture, viewport, pan_trail, measure, .. } = self;

        match gesture {
            // Both dealt with before this: a text drag returns at the top of
            // `on_mouse_move`, because it is not about the board.
            Gesture::None | Gesture::SelectingText => {}
            Gesture::Panning { from, moved, .. } => {
                // Move the camera so that the world point grabbed at the press
                // stays under the pointer. Working in world units rather than
                // accumulating screen deltas is what stops the board sliding
                // out from under the cursor during a zoom mid-drag.
                let anchor = *from;
                // The same screen-pixel threshold `drag_cards` commits a move
                // on, and for the same reason: exact-zero is not a click test,
                // it is a test a subpixel jitter fails to fail. A click on
                // empty paper deselects — see `clearing` — and a wobble this
                // small must not cost somebody their selection just because
                // the pointer twitched between the press and the release.
                //
                // The flag only. The pan below still tracks the pointer from
                // the first pixel — there is nothing to commit or undo about
                // moving the camera, so there is nothing here for a threshold
                // to protect.
                *moved |= (world.x - anchor.x).abs().max((world.y - anchor.y).abs())
                    * viewport.zoom
                    >= ENOUGH;
                viewport.pan.x -= world.x - anchor.x;
                viewport.pan.y -= world.y - anchor.y;
                // Where the camera is, and when. Sampled here rather than from
                // the pointer because the pan is what carries on afterwards,
                // so this is already in the units the projection wants — and
                // it stays right through a zoom mid-drag, which a screen-space
                // trail would not.
                pan_trail.push(viewport.pan, Instant::now());
            }
            // Handled above, where `self` is still whole.
            Gesture::Marquee { .. } => {}
            // Handled after the destructuring below, where the media and the
            // board are both reachable — a scrub writes to neither of the four
            // fields borrowed here.
            Gesture::Scrubbing { .. } | Gesture::Louder { .. } | Gesture::Orbiting { .. } => {}
            // Handled above, where `self` is still whole.
            Gesture::Roping { .. } => {}
            Gesture::Sizing { id, grip, start, from, hold, shape, moved, cropping, open } => {
                // `free` already escapes a held shape a few lines down; it
                // escapes the grid too, for the same reason a resize needs a
                // way out of either warp mid-drag. Held, this frame's edge
                // tracks the pointer exactly; let go, and the very next frame
                // snaps straight back onto the lattice.
                let to_grid = (doc.board.settings.desktop.snap && !free)
                    .then_some(doc.board.settings.desktop.grid_step);
                // What the modifiers mean, and the order they are asked in.
                // A picture keeps its shape unless somebody says otherwise;
                // anything else is free unless `Shift` says otherwise. Two
                // defaults, because a photograph and a note want
                // opposite things and only one of them is ever stretched on
                // purpose.
                let crop = alt && shape.is_some();
                let keep = if crop || free {
                    None
                } else {
                    shape.or_else(|| shift.then(|| start.width() / start.height()))
                };
                // `hold` keeps the edge at the same offset from the pointer
                // that the press started with, rather than snapping it to the
                // pointer outright — see the field's own doc.
                let pointer = point(world.x + hold.x, world.y + hold.y);
                let box_ = crate::grips::resized(*grip, *start, pointer, keep, to_grid);
                // The same screen-pixel threshold every other gesture commits
                // a move on. A handle is a small, precise target, but the
                // press that lands on one is still a press, and a press that
                // never left it is a click that should not cost an undo step.
                *moved |= (world.x - from.x).abs().max((world.y - from.y).abs()) * viewport.zoom
                    >= ENOUGH;
                *cropping |= crop;
                let id = id.clone();
                doc.board.during(open, |board| {
                    if let Some(item) = board.item_mut(&id) {
                        let centre = box_.centre();
                        item.x = centre.x;
                        item.y = centre.y;
                        item.w = box_.width();
                        item.h = box_.height();
                        // On a fitted note the drag sets the *width* and the
                        // words decide the rest. Letting the handle win instead
                        // would leave a height the next keystroke overwrites,
                        // which is a control that does nothing a moment later.
                        refit(item, measure);
                        if crop {
                            // Cropping is a framing, and the format already has
                            // the word for it: a covered picture fills the card
                            // and the card cuts off the rest. So the drag says
                            // `cover` and the frame is the crop.
                            item.meta.insert("fit".into(), serde_json::json!("cover"));
                        }
                    }
                });
            }
            // Handled above, where `self` is still whole: a smart guide is a
            // question about the rest of the board and an Alt-drag writes new
            // items, and neither is reachable through these four fields. A
            // sliding label is up there for the same reason — it is measured
            // against `self.drawn`, which is not one of them either.
            Gesture::Moving { .. } | Gesture::Sliding { .. } => {}
        }
        cx.notify();
    }

    /// One frame of a sweep on empty space: recomputes what the marquee
    /// would catch if the hand let go right now.
    ///
    /// Kept out of `on_mouse_move`'s borrow-splitting match for the same
    /// reason `drag_cards` is: it needs `self` whole. The pick it runs is
    /// exactly the one `on_mouse_up` runs at release — same rectangle from
    /// the same corner, same index, same fence resolution — because the
    /// point of a live preview is that it is not lying about what letting
    /// go would do.
    fn update_marquee(&mut self, world: WorldPoint) {
        let Gesture::Marquee { from, .. } = &self.gesture else { return };
        let from = *from;
        let rect = Rect::new(
            from.x.min(world.x),
            from.y.min(world.y),
            from.x.max(world.x),
            from.y.max(world.y),
        );
        // Through the index and the fence rule, like the release itself —
        // see the matching pass in `on_mouse_up` for why both matter.
        let mut swept = Vec::new();
        self.index().in_rect(rect, &mut swept);
        let items = &self.doc.board.items;
        let caught: Vec<String> = swept
            .into_iter()
            .map(|i| &items[i as usize])
            .filter(|i| i.kind.is_content())
            .map(|i| i.id.clone())
            .collect();
        self.fences();
        let fences = std::mem::take(&mut self.fences);
        let provisional: HashSet<String> =
            caught.into_iter().map(|id| Self::pick(&fences, &self.inside, &id)).collect();
        self.fences = fences;
        let Gesture::Marquee { to, provisional: slot, .. } = &mut self.gesture else { return };
        *to = world;
        *slot = provisional;
    }

    /// One frame of a drag on the cards.
    ///
    /// Kept out of `on_mouse_move`'s borrow-splitting match because two of the
    /// three things it does need `self` whole: the guides ask the index where
    /// everything else on the board is, and an Alt-drag puts *new* items down
    /// rather than moving existing ones.
    ///
    /// The order below is the order the modifiers are decided in, and it is
    /// not arbitrary:
    ///
    /// 1. **`Alt` leaves a copy**, before anything moves, so the copy is left
    ///    exactly where the press landed.
    /// 2. **`Shift` pins the drag to an axis**, before the guides, so that a
    ///    pinned drag is never nudged off its axis by something it lined up
    ///    with.
    /// 3. **The guides correct what is left**, which is the only step that can
    ///    be overruled — by the grid, which is a setting and outranks it.
    fn drag_cards(&mut self, world: WorldPoint, mods: gpui::Modifiers, cx: &mut Context<Self>) {
        let Gesture::Moving { from, moved, .. } = &self.gesture else { return };
        let (from, moved) = (*from, *moved);
        let (mut dx, mut dy) = (world.x - from.x, world.y - from.y);
        // Screen pixels, so the wait is the same gesture at every zoom. See
        // `ENOUGH`.
        let far = dx.abs().max(dy.abs()) * self.viewport.zoom >= ENOUGH;
        // A press that has not gone far enough to call a drag is still a
        // click, and a click must not push an undoable move onto the ledger.
        // Exact-zero used to be the test, which a subpixel jitter — the
        // pointer moving without anybody meaning it to — defeats: any nonzero
        // delta committed, so a click could record a move nobody made. Once
        // the drag *has* crossed the threshold, `moved` latches true, so a
        // frame that drifts back under it still commits — the gesture does
        // not un-become a drag partway through.
        //
        // `dx`/`dy` are measured from the press the whole time regardless, so
        // the first committed frame jumps straight to wherever the pointer
        // actually is rather than starting over from the threshold — a drag
        // is 1:1 with the pointer from the moment it counts as one.
        if !moved && !far {
            return;
        }

        self.leave_copy(mods.alt);

        // Which way the drag is pinned. Decided once and then kept, so a drag
        // near the diagonal does not flip between the two several times a
        // second; released the moment the key comes up.
        //
        // Not decided until the pointer has actually gone somewhere, though.
        // `Shift` held down *before* the drag has travelled has no direction to
        // pin, and `dx >= dy` on a delta of zero is horizontal — which is how a
        // shift-drag ends up locked to the wrong axis every single time.
        let lock = {
            let Gesture::Moving { lock, .. } = &mut self.gesture else { return };
            if !mods.shift {
                *lock = None;
            } else if lock.is_none() && far {
                *lock = Some(if dx.abs() >= dy.abs() { Axis::Horizontal } else { Axis::Vertical });
            }
            *lock
        };
        match lock {
            Some(Axis::Horizontal) => dy = 0.0,
            Some(Axis::Vertical) => dx = 0.0,
            None => {}
        }

        // The grid outranks the guides, and they are never both on. A card
        // cannot be on the lattice and flush with its neighbour at the same
        // time, so an app that offered both would give whichever ran last —
        // see `core::guides`, which says the same thing from the other side.
        let snap = self.doc.board.settings.desktop.snap;
        let step = self.doc.board.settings.desktop.grid_step;
        // The same escape `Sizing`'s own `free` gives a resize, and for the
        // same complaint: a grid warp with no way out mid-drag is a card that
        // cannot be judged into position by eye, only rounded near it. Held,
        // this drag ignores the grid entirely and the card tracks the pointer
        // 1:1; let go, and the very next frame snaps straight back onto it —
        // there is nothing to un-escape, since nothing about the setting
        // itself changed, only whether this one gesture answers to it.
        //
        // Deliberately not folded into `snap` above: the guides gate a few
        // lines down asks whether grid snapping is the board's *active*
        // system, which the escape does not change — a held modifier is a way
        // out of the grid's own warp for one drag, not a way to swap it for
        // the other snapping system mid-gesture.
        let escape_grid = mods.secondary() || mods.control;
        // And the board can say no to both. `View -> Alignment guides` is off
        // by nobody's default, but a board of overlapping photographs has
        // nothing worth lining up and every rule drawn across it is a rule in
        // the way — so it is a setting rather than a thing you learn to endure.
        let on = self.doc.board.settings.desktop.guides;
        let mut found = if snap || !on { Snap::default() } else { self.guides_at(dx, dy) };
        // A pinned axis takes nothing from a guide, including the guide's own
        // idea of where the card should be. Its lines go too: a rule drawn
        // through an edge the card was not allowed to reach is a rule that
        // lies about what happened.
        match lock {
            Some(Axis::Horizontal) => strip(&mut found, false),
            Some(Axis::Vertical) => strip(&mut found, true),
            None => {}
        }
        dx += found.dx;
        dy += found.dy;

        let Self { doc, gesture, .. } = self;
        let Gesture::Moving { start, moved, guides, open, .. } = gesture else { return };
        *moved = true;
        *guides = found;
        // Through the open gesture: this writes and records nothing, because
        // the step for the whole drag is closed at the release.
        doc.board.during(open, |board| {
            for held in start.iter() {
                // By index, checked against the id — see [`Held`]. The scan
                // is only the fallback for a board that shifted mid-gesture,
                // which nothing does.
                let fits = board.items.get(held.index).is_some_and(|item| item.id == held.id);
                let found =
                    if fits { board.items.get_mut(held.index) } else { board.item_mut(&held.id) };
                if let Some(item) = found {
                    let home = &held.home;
                    // The free position: where the card would be with no
                    // grid at all. Kept off the card's own x/y so that
                    // the next frame has something unrounded to measure
                    // from, and mirrored into `presnap` so that turning
                    // snapping off can put the card back rather than
                    // leaving it on the lattice.
                    let free = point(home.x + dx, home.y + dy);
                    let to = dropped_at(*home, dx, dy, (snap && !escape_grid).then_some(step));
                    if snap {
                        item.meta.insert(
                            "presnap".into(),
                            serde_json::json!({
                                "x": free.x, "y": free.y, "w": item.w, "h": item.h
                            }),
                        );
                    } else {
                        item.meta.remove("presnap");
                    }
                    item.x = to.x;
                    item.y = to.y;
                }
            }
        });
        cx.notify();
    }

    /// What the cards being dragged would line up with, offset by `(dx, dy)`.
    ///
    /// The moving set is measured as **one box**, not card by card. Dragging
    /// four cards and having each of them independently take a different
    /// neighbour's edge would pull the four apart, and what somebody dragging
    /// four cards is moving is the four of them.
    ///
    /// Candidates come off the index and are the ones on screen: a card lining
    /// up with something a mile away, drawing a rule to nowhere, would be a
    /// guide about a coincidence.
    ///
    /// Every box here — the moving one included — is the card's own frame
    /// rather than the area it covers. See [`frame`], which is where that
    /// distinction is argued.
    fn guides_at(&mut self, dx: f32, dy: f32) -> Snap {
        if !matches!(self.gesture, Gesture::Moving { .. }) {
            return Snap::default();
        }
        // The index first, before the gesture is borrowed. It is built lazily
        // and so wants `self` mutably, and the borrow below holds the gesture
        // for the rest of the function — asking in the other order does not
        // compile, and cloning the moving set once a frame to get around that
        // would be a copy of the selection sixty times a second.
        //
        // Unsorted, unlike the painter's cull: the guides measure boxes and
        // never ask which is on top, so paying `visible_by_depth`'s sort per
        // pointer event bought nothing.
        let window = self.viewport.visible().inflate(CULL_MARGIN);
        let mut visible = Vec::new();
        self.index().in_rect(window, &mut visible);

        let Gesture::Moving { start, guides: prior, .. } = &self.gesture else {
            return Snap::default();
        };
        // What each axis was already engaged on, straight from last frame's
        // own answer — see `guides::find_held`, which is what this is for.
        // Read here rather than passed down from `drag_cards`, because this
        // is the one place both `prior` and the fresh candidates are already
        // in hand; threading it through another argument would just move
        // the same lookup somewhere less obvious.
        let held_x = prior.lines.iter().find_map(|l| match l {
            Line::Vertical { x, .. } => Some(*x),
            Line::Horizontal { .. } => None,
        });
        let held_y = prior.lines.iter().find_map(|l| match l {
            Line::Horizontal { y, .. } => Some(*y),
            Line::Vertical { .. } => None,
        });
        // Who is moving, and where they will be. Off `start` rather than off
        // the selection, because a fence's contents move with it and are not
        // themselves selected.
        let held: HashSet<&str> = start.iter().map(|h| h.id.as_str()).collect();
        // Built from where the cards were **when the press landed**, offset by
        // the whole delta — not from where the last frame left them. That is
        // the same rule the move itself follows and for the same reason: a
        // measurement against the previous frame accumulates, and a card that
        // has already been nudged onto a guide would be measured as wanting to
        // be nudged onto it again. The sizes were taken at the press too — see
        // [`Grabbed`] — so this asks the board for nothing.
        let mut home: Option<Rect> = None;
        for h in start {
            let r = Rect::centred(h.home.x + dx, h.home.y + dy, h.w, h.h);
            home = Some(match home {
                None => r,
                Some(acc) => Rect::new(
                    acc.x0.min(r.x0),
                    acc.y0.min(r.y0),
                    acc.x1.max(r.x1),
                    acc.y1.max(r.y1),
                ),
            });
        }
        let Some(free) = home else { return Snap::default() };

        // Everything on screen that is not moving. The visible set is already
        // what the painter culled to, so this costs a filter rather than a
        // walk of the board.
        //
        // Fences are left out of it. A rule is drawn across the union of the
        // card and everything it lined up with, so a fence — which is the size
        // of the group it holds — puts a line the width of the whole group
        // across the board the moment a card inside it moves a few units. That
        // is the complaint that made this a setting, and a fence is anyway not
        // a thing you line up *with*: it is the region the cards you are
        // lining up already sit inside.
        let items = &self.doc.board.items;
        let others: Vec<Rect> = visible
            .into_iter()
            .map(|i| &items[i as usize])
            .filter(|it| it.kind != mbrd_core::ItemType::Fence)
            .filter(|it| !held.contains(it.id.as_str()))
            .map(frame)
            .collect();

        // The tolerance is a distance on **screen**, converted here. A snap
        // that got harder to reach the further you zoomed out would stop
        // working on exactly the boards big enough to need it.
        //
        // `_held` rather than plain `find`: a coordinate already engaged
        // reaches a little further than one found fresh, which is what
        // stops a guide flickering on and off around its own boundary while
        // an otherwise-steady hand has the ordinary amount of tremor in it.
        guides::find_held(
            free,
            &others,
            guides::REACH / self.viewport.zoom.max(0.0001),
            (held_x, held_y),
        )
    }

    /// Leave a copy of everything this drag is holding, once.
    ///
    /// `Alt` duplicates, and the picture it makes is the one Figma makes: one
    /// set left where the press landed, one set following the pointer. What
    /// actually happens is the other way round — the *copies* are the ones left
    /// behind and the originals carry on moving — because that keeps the moving
    /// id set the same for the whole gesture, which everything from `start` to
    /// the guides relies on. On screen the two are indistinguishable.
    ///
    /// Inside the drag's own open step, so one gesture is one thing to undo
    /// rather than a move to take back and then a duplicate.
    fn leave_copy(&mut self, alt: bool) {
        if !alt {
            return;
        }
        let Gesture::Moving { start, copied: false, open, .. } = &self.gesture else { return };
        // At the position the press landed on, not the one the cards have
        // reached: the copy is what stays behind.
        let open = open.clone();
        let cards: Vec<Item> = start
            .iter()
            .filter_map(|held| {
                let item = match self.doc.board.items.get(held.index) {
                    Some(item) if item.id == held.id => Some(item),
                    _ => self.doc.board.item(&held.id),
                };
                item.map(|item| {
                    let mut copy = item.clone();
                    copy.x = held.home.x;
                    copy.y = held.home.y;
                    copy
                })
            })
            .collect();
        if cards.is_empty() {
            return;
        }
        let fresh = self.respawn(cards);
        let n = fresh.len();
        self.doc.board.during(&open, |board| board.items.extend(fresh));
        if let Gesture::Moving { copied, .. } = &mut self.gesture {
            *copied = true;
        }
        self.say(match n {
            1 => "copy left behind".to_string(),
            n => format!("{n} copies left behind"),
        });
    }

    pub(crate) fn on_mouse_up(
        &mut self,
        _event: &MouseUpEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Every gesture ends in exactly one place. A release that lands outside
        // the canvas is wired to this too, so a drag off the edge cannot leave
        // the pipeline stuck mid-gesture.
        //
        // Taken rather than read, so that ending a gesture owns whatever the
        // gesture was holding — the open step, for a move — and the pipeline is
        // back at rest before any of it is acted on.
        let ended = std::mem::replace(&mut self.gesture, Gesture::None);

        // Whatever button was pressed is not held any more, whatever it was
        // and wherever the release landed — a press held down and dragged off
        // the button must not leave the wash lit.
        self.pressed_control = None;

        // A press on empty paper that never travelled is a click on the paper,
        // and a click on the paper is how you say "nothing". Measured at the
        // release rather than at the press, because the same press is also the
        // start of a pan — and losing the selection every time you moved the
        // camera would be the worse of the two bugs.
        if let Gesture::Panning { moved: false, clearing: true, .. } = ended {
            self.pan_trail = Trail::default();
            self.let_go(cx);
            return;
        }

        // A pan that was still travelling when the hand came off keeps
        // travelling. Where it lands is projected from the release speed
        // rather than measured from the release *point*, which is what makes a
        // flick a throw: a small gesture with a large result, and the only
        // thing that tells the two apart is how fast the hand was going.
        //
        // A hand that stopped before letting go gets nothing, and neither does
        // one that was merely still drifting — see `Trail::throw`, which is
        // where both halves of that rule live.
        if let Gesture::Panning { .. } = ended {
            let trail = std::mem::take(&mut self.pan_trail);
            if let Some((vx, vy)) = trail.throw(Instant::now(), self.viewport.zoom) {
                self.camera.fling(self.viewport.pan, vx, vy);
            }
            cx.notify();
            return;
        }

        // A scrub ends where it ends. Nothing to close: a playhead is not
        // board state, so there is no step and nothing to take back.
        if let Gesture::Scrubbing { .. } = ended {
            cx.notify();
            return;
        }

        if let Gesture::Louder { id, open } = ended {
            // One step for the whole drag. `finish` answers `false` where the
            // slider ended where it started, which is a press that changed
            // nothing and must not leave an empty entry in the ledger.
            if self.doc.board.finish("Volume", open) {
                let level = self
                    .doc
                    .board
                    .item(&id)
                    .map(|item| mbrd_core::media::playback(item).volume)
                    .unwrap_or(1.0);
                self.say(format!("volume {}%", (level * 100.0).round() as i32));
            }
            cx.notify();
            return;
        }

        if let Gesture::Orbiting { id, moved, open, .. } = ended {
            // A press that never travelled is a click, same as `Moving`'s —
            // and, unlike a slider, a mesh that never turned has nothing new
            // to rasterise, so the resting picture and the live one it was
            // already showing both still agree.
            let changed = moved && self.doc.board.finish("Orbit", open);
            if moved {
                // The picture this drag was showing lived in `live`, keyed by
                // card rather than content — see `live.rs`. What is on disk
                // now is the released orbit, and `resting` has to catch up to
                // it the same way any other decode catches up to a changed
                // asset.
                self.meshes.forget(&id);
                self.begin_mesh_decode(&id, cx);
            }
            if changed {
                self.say("orbited".into());
            }
            cx.notify();
            return;
        }

        if let Gesture::Sliding { moved, open, .. } = ended {
            // Like a move: a press that never travelled is a click, and a click
            // on a label means the line it is on, which was already selected at
            // the press. Nothing to record for that.
            if moved && self.doc.board.finish("Move label", open) {
                self.say("moved the label".into());
            }
            cx.notify();
            return;
        }

        if let Gesture::Moving { start, moved, copied, open, .. } = ended {
            // A press that never travelled is a click. Closing it would be
            // closing an empty gesture, and the ledger would refuse the step
            // anyway — but saying "moved" about it would still be a lie.
            if moved {
                let n = start.len();
                // An Alt-drag is a duplicate that happens to have been made
                // with the pointer, and the ledger should say so: "Move 4" for
                // a gesture that put four new cards on the board would be an
                // entry nobody could find again.
                let label = match (copied, n) {
                    (true, 1) => "Duplicate".to_string(),
                    (true, n) => format!("Duplicate {n}"),
                    (false, 1) => "Move".to_string(),
                    (false, n) => format!("Move {n}"),
                };
                if self.doc.board.finish(&label, open) {
                    self.say(match (copied, n) {
                        (true, 1) => "duplicated".to_string(),
                        (true, n) => format!("duplicated {n}"),
                        (false, 1) => "moved".to_string(),
                        (false, n) => format!("moved {n}"),
                    });
                }
            }
            cx.notify();
            return;
        }

        if let Gesture::Sizing { moved, cropping, open, .. } = ended {
            let what = if cropping { "Crop" } else { "Resize" };
            if moved && self.doc.board.finish(what, open) {
                self.say(if cropping { "cropped" } else { "resized" }.into());
            }
            cx.notify();
            return;
        }

        if let Gesture::Roping { from, over, .. } = ended {
            match over {
                // A rope that landed on a card is a rope. One that landed on
                // empty space is not a rope that was drawn and undone — it is
                // one that was never drawn, so nothing is recorded and there is
                // nothing to take back.
                Some(to) => self.join(&from, &to, cx),
                None => {
                    // Nothing is left on the board to look at — the rope was
                    // never drawn — so this is the absence of a thing
                    // narration would have described, not a report of one.
                    self.tell("nothing there to join to".into());
                    cx.notify();
                }
            }
            return;
        }

        if let Gesture::Marquee { from, to, additive, .. } = ended {
            let rect =
                Rect::new(from.x.min(to.x), from.y.min(to.y), from.x.max(to.x), from.y.max(to.y));
            if !additive {
                self.selection.clear();
            }
            // Through the index, like every other question about where things
            // are. A sweep over a corner of a large board should cost the
            // corner, not the board.
            //
            // Run again here rather than taking `provisional` off the ended
            // gesture: the preview is a set because membership is all the
            // paint path asks of it, but the order things enter `selection`
            // in is meaningful elsewhere (`.first()`, `.last()`), and a set
            // does not have one. Same rect, same pick, so this lands on
            // exactly what was just shown — just with an order to it.
            let mut swept = Vec::new();
            self.index().in_rect(rect, &mut swept);
            let items = &self.doc.board.items;
            let caught: Vec<String> = swept
                .into_iter()
                .map(|i| &items[i as usize])
                .filter(|i| i.kind.is_content())
                .map(|i| i.id.clone())
                .collect();
            // Through the group rule, like a press: a sweep across a group
            // catches the group rather than the three cards of it the
            // rectangle happened to cross. `selects` collapses them all to the
            // same fence, and the duplicate check below is what makes that one
            // entry rather than three.
            //
            // Measured once for the whole sweep, not once per card — see
            // `selects_in`.
            self.fences();
            let fences = std::mem::take(&mut self.fences);
            for id in caught {
                let id = Self::pick(&fences, &self.inside, &id);
                if !self.is_selected(&id) {
                    self.selection.push(id);
                }
            }
            // Put back rather than dropped: nothing here changed the board, so
            // the measurement is still current and re-taking it next frame
            // would be a pass over the whole board for the same answer.
            self.fences = fences;
        }
        cx.notify();
    }

    fn on_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Position mode claims the wheel too, but only over the one card it
        // names — everywhere else on the board still zooms the camera.
        if let Some(id) = self.positioning.clone() {
            let world = self.world_at(event.position);
            if self.hit(world).as_deref() == Some(id.as_str()) {
                self.dolly_mesh(&id, event, cx);
                cx.notify();
                return;
            }
        }

        let (dx, dy) = match event.delta {
            // A trackpad reports exact pixels; a wheel reports lines. Scaling
            // them the same way is what makes one notch and one flick feel
            // like the same amount of zoom.
            ScrollDelta::Pixels(p) => (f(p.x) / 40.0, f(p.y) / 40.0),
            ScrollDelta::Lines(p) => (p.x, p.y),
        };

        // Both of these go through the camera rather than onto the viewport,
        // because a wheel arrives in notches and a notch applied straight to
        // the camera is a jump. The spring is short — it exists to join the
        // detents up, not to take the scenic route — and it is also what gives
        // the ends of the zoom range something to push against.
        if event.modifiers.shift {
            // Shift is pan-sideways, not zoom. Note that the *vertical* delta
            // drives it: a mouse with one wheel has nothing else to give, and a
            // trackpad's horizontal delta is added on top.
            self.camera.nudge((dy + dx) * 40.0, 0.0, &self.viewport);
        } else {
            // The same window-to-canvas correction `world_at` makes, but
            // stopping at canvas-local pixels: the anchor is a thing to hold
            // still on screen, so it is measured in screen units.
            let local = (
                f(event.position.x) - f(self.canvas_bounds.origin.x),
                f(event.position.y) - f(self.canvas_bounds.origin.y),
            );
            let factor = (1.0 + ZOOM_PER_LINE).powf(dy);
            self.camera.zoom_by(factor, local, &self.viewport);
        }
        cx.notify();
    }

    fn on_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        let mods = event.keystroke.modifiers;

        // Whatever this press turns out to be, it is not part of a drop that is
        // still arriving. See `part_import`.
        self.part_import();

        // Any press at all means the modifier that may be down is being held
        // rather than tapped. Unconditional and first, so that no branch below
        // can return without having said so — which is how a gesture watcher
        // acquires a hole.
        self.taps.spoil();

        // The palette, first of the three modes. Same terms as the switcher
        // below: while it is open it takes every press, because it is a text
        // field and a text field whose letters were shortcuts would be one you
        // cannot type in.
        //
        // Gated on the *logical* state — the variant — rather than on
        // `overlay_leaving`, so Escape and every other key still work while
        // the palette is still fading in. A palette that is on its way out
        // instead falls through here, dead to input, to whatever the board
        // underneath would have done with the key: see `Overlay`.
        if matches!(self.overlay, Overlay::Palette(_)) && !self.overlay_leaving {
            let Overlay::Palette(palette) = &mut self.overlay else { unreachable!() };
            let reply = palette.key(key, mods, event.keystroke.key_char.as_deref());
            match reply {
                crate::palette::Reply::Held => {}
                crate::palette::Reply::Close => self.close_palette(),
                // The clipboard is the view's, not the palette's — same
                // division `paste_text` draws for a card being typed into.
                crate::palette::Reply::Paste => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        if let Overlay::Palette(palette) = &mut self.overlay {
                            palette.insert(&text);
                        }
                    }
                }
                crate::palette::Reply::Run => {
                    let chosen = palette.chosen();
                    if let Some(what) = chosen {
                        self.run_palette_row(what, window, cx);
                    } else {
                        self.close_palette();
                    }
                }
            }
            cx.notify();
            return;
        }

        // The one mode. While the switcher is open it takes every press,
        // because it is a text field and a text field that let some of its
        // letters be shortcuts would be a text field you cannot type in. Same
        // input-dead exception while it is leaving as the palette's, above.
        if matches!(self.overlay, Overlay::Switcher(_)) && !self.overlay_leaving {
            let Overlay::Switcher(switcher) = &mut self.overlay else { unreachable!() };
            let reply = switcher.key(key, mods, event.keystroke.key_char.as_deref());
            match reply {
                Reply::Held => {}
                Reply::Close => self.close_switcher(),
                Reply::Open => {
                    let chosen = switcher.chosen();
                    self.close_switcher();
                    if let Some(path) = chosen {
                        self.open_board(&path, cx);
                    }
                }
                Reply::Delete => self.delete_doomed_board(cx),
                // The clipboard is the view's, not the switcher's — same
                // division the palette draws, above.
                Reply::Paste => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        if let Overlay::Switcher(switcher) = &mut self.overlay {
                            switcher.insert(&text);
                        }
                    }
                }
            }
            cx.notify();
            return;
        }

        // The settings page. It takes every press too, and it used to answer
        // exactly one of them: the board behind it is not on screen, and a
        // shortcut that edited what nobody can see would be an edit nobody
        // watched happen. It answers rather more now, because the page has a
        // search field over its sidebar and, sometimes, a theme picker over
        // the whole of it — both of which are text fields, and a text field
        // whose letters were shortcuts would be one you cannot type in.
        //
        // Same input-dead exception while leaving as the two above.
        if matches!(self.overlay, Overlay::Settings(_)) && !self.overlay_leaving {
            // The candidate list the picker is choosing from, worked out
            // before the page is borrowed: it comes off the registry, which
            // lives on `self`, and the page's `key` needs it as an argument
            // precisely so that a plain struct does not have to reach for a
            // view. Empty except while a picker is open.
            let names: Vec<String> = match &self.overlay {
                Overlay::Settings(page) => page
                    .picking
                    .as_ref()
                    .map(|picker| {
                        self.themes.of(picker.appearance).iter().map(|t| t.name.clone()).collect()
                    })
                    .unwrap_or_default(),
                _ => Vec::new(),
            };
            let Overlay::Settings(page) = &mut self.overlay else { unreachable!() };
            let reply = page.key(key, mods, event.keystroke.key_char.as_deref(), &names);
            match reply {
                crate::settings::Reply::Held => {}
                crate::settings::Reply::Close => self.close_settings(),
                // The clipboard is the view's, not the page's — the same
                // division the palette and the switcher draw.
                crate::settings::Reply::Paste => {
                    if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
                        if let Overlay::Settings(page) = &mut self.overlay {
                            page.insert(&text);
                        }
                    }
                }
                // A name, not a palette: the page never holds colours, so a
                // theme file reloaded underneath it cannot leave a stale one
                // sitting in an overlay.
                crate::settings::Reply::Preview(appearance, name) => {
                    let theme = self.themes.resolve(&name, appearance);
                    self.preview_theme(theme, cx);
                }
                // Ends the preview as a side effect, which is right: a choice
                // is the answer to what a preview was asking.
                crate::settings::Reply::Choose(appearance, name) => {
                    self.set_theme(appearance, name, cx);
                }
                crate::settings::Reply::Cancel(appearance, was) => {
                    // The choice goes back through the prefs and the palette
                    // through `cancel_preview`, in that order and for the
                    // reason `cancel_theme_pick` gives: one of them is what
                    // `retheme` will agree with, the other is what is
                    // actually being looked at.
                    self.prefs.set_theme(appearance, was);
                    self.cancel_preview(cx);
                    self.retheme(cx);
                }
            }
            cx.notify();
            return;
        }

        // A card opened onto the whole window. While it is only being *read*
        // it takes every press for the same reason the settings page does —
        // the board is not on screen, and a shortcut that edited it would be
        // an edit nobody watched happen. Escape is the one key it answers.
        //
        // While it is being typed into it deliberately does not return, and
        // falls through to the editor below: the window is a text field then,
        // and Escape means put the words back rather than close the page.
        if matches!(self.overlay, Overlay::Opened(_))
            && !self.overlay_leaving
            && self.editing.is_none()
        {
            if key == "escape" {
                self.close_opened(cx);
            }
            return;
        }

        // The second mode. A text field owns most of the keyboard while it is
        // open — that is what makes it a text field — but not all of it: `Ctrl
        // S` inside a note should still save, and the editor says so by
        // handing those back rather than swallowing them.
        if self.editing.is_some() {
            let mods = editor::Mods::from(mods);
            let typed = event.keystroke.key_char.clone();
            let reply =
                self.editing.as_mut().map(|open| open.editor.key(key, mods, typed.as_deref()));
            match reply {
                // Every one of these stops the press here. That is not
                // tidiness: while a note is being typed into there is an input
                // handler installed, and gpui hands a press to it *only if the
                // app let it propagate* — so a key this editor has already
                // typed and then propagated would be typed a second time by
                // the platform. See the `EntityInputHandler` impl.
                Some(editor::Reply::Held) => {
                    self.show_edit();
                    cx.stop_propagation();
                    cx.notify();
                    return;
                }
                Some(editor::Reply::Commit) => {
                    self.stop_editing(true, cx);
                    cx.stop_propagation();
                    return;
                }
                Some(editor::Reply::Revert) => {
                    self.stop_editing(false, cx);
                    cx.stop_propagation();
                    return;
                }
                // Copy, cut and paste inside text are the clipboard's, not the
                // board's, and they are the only three commands that mean
                // something different in here than out there.
                Some(editor::Reply::Ignored) if mods.secondary => match key {
                    "c" | "x" => {
                        self.copy_text(key == "x", cx);
                        cx.stop_propagation();
                        return;
                    }
                    "v" => {
                        self.paste_text(cx);
                        cx.stop_propagation();
                        return;
                    }
                    _ => {}
                },
                _ => {}
            }
            // Anything still unclaimed falls through to the board's own
            // shortcuts below, which is how `Ctrl S` saves from inside a note.
        }

        // A tool before a command, and only ever an unmodified digit or letter
        // that no command answers to — `tools.rs` has a test that says so, so
        // the two tables cannot drift into meaning the same key.
        if let Some(tool) = Tool::for_key(key, mods) {
            self.choose_tool(tool, cx);
            return;
        }

        // Escape stops a board still being read before anything else it could
        // mean, and before a drop: it is the one thing on screen with a panel
        // over the board, so it is what the key most obviously points at.
        if key == "escape" && self.stop_opening(cx) {
            return;
        }

        // Escape stops a drop still arriving before anything else it could
        // mean. Everything else the key does is about where you are; this is
        // the one thing on screen that is still happening, and a folder landing
        // card by card is exactly the thing somebody presses Escape at.
        if key == "escape" && self.stop_importing(cx) {
            return;
        }

        // Escape puts a tool away before it clears the selection: leaving a
        // mode is the nearest thing to undo about the press.
        if key == "escape" && self.tool != Tool::Select {
            self.choose_tool(Tool::Select, cx);
            return;
        }

        // Escape leaves Position mode before it touches the selection — the
        // same "leave the mode nearest to what was just pressed" rule the
        // tool check just above follows.
        if key == "escape" && self.positioning.is_some() {
            self.positioning = None;
            cx.notify();
            return;
        }

        // The menu takes the arrows, Enter and Escape before anything else
        // gets them — the same "text field" shape the switcher and the
        // palette take above, and for the same reason: without this, Down
        // nudges the selected cards behind a menu that is floating over
        // them, and Enter renames one, neither of which the menu had
        // anything to do with. The hover highlight and the keyboard cursor
        // are one concept — see [`crate::menu::Menu::cursor`] — so arrowing
        // through the list after moving the mouse over it continues from
        // wherever the pointer left off, and vice versa.
        //
        // Gated on the variant rather than on `overlay_leaving`, same as the
        // palette and the switcher above: Escape must still work while the
        // menu is fading in.
        if matches!(self.overlay, Overlay::Menu(_)) && !self.overlay_leaving {
            match key {
                "escape" => {
                    self.close_menu();
                    cx.notify();
                    return;
                }
                "up" | "down" => {
                    if let Overlay::Menu(menu) = &mut self.overlay {
                        menu.step(if key == "up" { -1 } else { 1 });
                    }
                    cx.notify();
                    return;
                }
                // Opens the submenu under the keyboard, where the row has
                // one — a menu's version of the palette's Enter.
                "right" => {
                    let room = self.canvas_bounds.size;
                    let sub = self.submenu_under_cursor();
                    if let Overlay::Menu(menu) = &mut self.overlay {
                        menu.open_under_cursor(room, sub);
                    }
                    cx.notify();
                    return;
                }
                // Backs out of a submenu without closing the menu itself —
                // the same corner Left turns in a fitted, cut-down list.
                "left" => {
                    if let Overlay::Menu(menu) = &mut self.overlay {
                        menu.close_sub();
                    }
                    cx.notify();
                    return;
                }
                "enter" => {
                    let chosen = match &self.overlay {
                        Overlay::Menu(menu) => menu.chosen(),
                        _ => None,
                    };
                    match chosen {
                        Some(Entry::More(..)) => {
                            let room = self.canvas_bounds.size;
                            let sub = self.submenu_under_cursor();
                            if let Overlay::Menu(menu) = &mut self.overlay {
                                menu.open_under_cursor(room, sub);
                            }
                            cx.notify();
                        }
                        Some(Entry::Does(command)) => {
                            self.close_menu();
                            if command.available(self) {
                                command.run(self, window, cx);
                            }
                        }
                        Some(Entry::Rule) | None => {}
                    }
                    return;
                }
                _ => {}
            }
        }

        // The platform's own context-menu key, and `Shift F10` for the
        // keyboards that have none — the same list a right-click over the
        // selection would open, so that somebody who never touches a mouse
        // is not missing a third of the app.
        if key == "menu" || (key == "f10" && mods.shift) {
            self.open_context_menu_at(cx);
            return;
        }

        // Every shortcut in the app, from the one table the menu draws from.
        if let Some(command) = Command::for_key(key, mods) {
            self.close_menu();
            command.run(self, window, cx);
            return;
        }

        // Nudge. Not a command, because it is four keys that differ only in
        // direction and putting each of them in the table would say less about
        // them than this does.
        if matches!(key, "left" | "right" | "up" | "down") {
            if self.selection.is_empty() {
                // Nothing to nudge, so the arrows are free to mean the other
                // thing an infinite canvas with no scrollbars needs them to
                // mean. Verified before this existed: `Camera::zoom_by` had
                // exactly one caller, the wheel, and panning had exactly one
                // way in too — a drag. Neither reaches a region nobody has
                // selected anything in, which on a board with no edges is
                // most of it, so a keyboard-only visitor had no way to look
                // anywhere they could not already see.
                //
                // Through `Camera::nudge`, the same door the wheel's
                // shift-scroll uses, rather than a direct write to the
                // viewport — so a run of taps glides smoothly through the
                // camera's spring the way a flick does, instead of snapping
                // the view frame to frame, and reduced motion is handled for
                // free the same way it already is for everything else that
                // goes through the camera.
                let (dx, dy) = match key {
                    "left" => (KEY_PAN_STEP, 0.0),
                    "right" => (-KEY_PAN_STEP, 0.0),
                    "up" => (0.0, KEY_PAN_STEP),
                    _ => (0.0, -KEY_PAN_STEP),
                };
                self.camera.nudge(dx, dy, &self.viewport);
                cx.notify();
                return;
            }

            // A whole grid step with shift held, which is the difference
            // between adjusting a layout and building one.
            let step = if mods.shift { self.doc.board.settings.desktop.grid_step } else { 1.0 };
            // World y points up, so "up" is positive.
            let (dx, dy) = match key {
                "left" => (-step, 0.0),
                "right" => (step, 0.0),
                "up" => (0.0, step),
                _ => (0.0, -step),
            };
            // Through `dragging`, like a drag: nudging a group has to move
            // what is in it, or four taps of an arrow key would slide the
            // rectangle off its own contents.
            let ids = self.dragging(self.selection.clone());
            if ids.is_empty() {
                return;
            }
            // One edit per press. A run of them collapses into one entry on
            // the strip, so twelve taps of an arrow key are one undo — see
            // `history::run_key`.
            self.doc.board.edit("Nudge", |board| {
                for id in ids {
                    if let Some(item) = board.item_mut(&id) {
                        item.x += dx;
                        item.y += dy;
                    }
                }
            });
            cx.notify();
        }
    }

    /// A modifier went down or came up. See `taps.rs`.
    ///
    /// The one input the board watches that is not a key press and not a mouse
    /// press. It exists for the two double-tap gestures and nothing else, so it
    /// is switched off entirely whenever the keyboard belongs to something
    /// else: somebody typing capitals into a note taps Shift constantly, and a
    /// palette that ambushed them mid-sentence would be worse than no palette.
    fn on_modifiers(
        &mut self,
        event: &gpui::ModifiersChangedEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The settings page joined this list when it grew a search field over
        // its sidebar. A double-tapped Shift is how the palette opens, and a
        // page you are typing into must not open one on the second capital
        // letter of a word.
        let text_field_open = matches!(
            self.overlay,
            Overlay::Palette(_) | Overlay::Switcher(_) | Overlay::Settings(_)
        );
        if self.editing.is_some() || text_field_open {
            self.taps.forget();
            return;
        }
        let Some(tap) = self.taps.changed(event.modifiers, Instant::now()) else { return };
        let mode = match tap {
            Tap::Shift => crate::palette::Mode::Commands,
            Tap::Secondary => crate::palette::Mode::Search,
        };
        self.open_palette(mode, cx);
    }

    // -----------------------------------------------------------------------
    // The frame clock
    // -----------------------------------------------------------------------

    /// Advance everything that moves by one frame, and say whether to ask for
    /// another.
    ///
    /// One reading of the clock per frame, shared by everything on it. Two
    /// readings would be two slightly different frames animating against each
    /// other, which is the kind of thing that shows up as a shimmer nobody can
    /// account for.
    fn advance(&mut self) -> bool {
        let now = Instant::now();
        // A fresh start rather than a slow frame: nothing was in flight, so
        // nothing is owed the time since the last redraw. See `animating`.
        if !self.animating {
            self.camera.wake(now);
        }
        // Reduced motion is a frame long enough for everything to have already
        // finished. Every spring lands on its target, every fade reaches its
        // end, and each of them still *happens* — the camera goes where it was
        // sent and the marks appear beside the card — which is the difference
        // between reduced motion and less feedback.
        //
        // Written this way rather than as a branch at each of the six places
        // that move, because a branch at each of them is six places for the
        // setting to be forgotten, and the one that forgot it would be the one
        // that made somebody ill.
        let dt = match self.prefs.motion {
            true => self.camera.tick(now),
            false => {
                self.camera.tick(now);
                FOREVER
            }
        };
        self.expire_status(now);
        // Bound to names and *then* combined, deliberately. Written as one
        // `||` chain the short-circuit would skip the fades on every frame the
        // camera happened to be moving, and the marks would advance only while
        // the board was still.
        let camera = self.camera.step(&mut self.viewport, dt);
        let anchors = self.fade_anchors(dt);
        // Playback is bound with the rest rather than short-circuited into the
        // chain below for the same reason the two above are: a video must go on
        // advancing while the camera happens to be moving.
        let playing = self.media.tick(now);
        // Driven by `dt` rather than left to read the wall clock at paint
        // time — see `Images::tick` and `Wires::tick` — which is what lets
        // reduced motion's one enormous `dt` land a picture's arrival and a
        // line's reshape in this single frame, the same as everything else
        // on this clock.
        self.images.tick(dt);
        self.wires.tick(dt);
        let overlay = self.advance_overlay(dt);
        let loader = self.advance_loader(dt);
        let presenting = self.advance_presenting(dt);
        let controls = self.advance_controls(dt);
        self.animating = camera
            || anchors
            || playing
            || self.images.arriving()
            || self.wires.fading()
            || overlay
            || loader
            || presenting
            || controls;
        self.animating
    }

    /// Bring the overlay's presence a frame nearer where it belongs, and drop
    /// it once it has finished leaving. See `Overlay` and `overlay_presence`.
    ///
    /// Retargeted here, every frame, rather than at the moments something
    /// opens or closes — the spring does not care how often it is told where
    /// to go, and one place that says so is one place `overlay_leaving` and
    /// the motion cannot disagree.
    fn advance_overlay(&mut self, dt: f32) -> bool {
        if matches!(self.overlay, Overlay::None) {
            return false;
        }
        self.overlay_presence.retarget(if self.overlay_leaving { 0.0 } else { 1.0 });
        let moving = self.overlay_presence.step(Spring::SURFACE, dt, PRESENCE_REST);
        if self.overlay_leaving && !moving {
            self.overlay = Overlay::None;
            self.overlay_leaving = false;
        }
        moving
    }

    /// The loading panel's own version of `advance_overlay`. A separate
    /// function rather than a shared one across both fields, because the two
    /// have nothing in common but the arithmetic — `Overlay` owns the
    /// keyboard and `opening` never does — and threading one field or the
    /// other through a shared function would be a bigger surface than just
    /// writing the four lines twice.
    fn advance_loader(&mut self, dt: f32) -> bool {
        if self.opening.is_none() {
            return false;
        }
        let moving = step_presence(&mut self.opening_presence, self.opening_leaving, dt);
        if self.opening_leaving && self.opening_presence <= 0.0 {
            self.opening = None;
            self.opening_leaving = false;
        }
        moving
    }

    // -----------------------------------------------------------------------
    // Drawing
    // -----------------------------------------------------------------------

    /// Work out this frame's lines, and the marks beside the hovered card.
    ///
    /// Two things and not one function each, because they share the pointer's
    /// position and the camera, and because both are things drawn *around* the
    /// cards rather than on them — ropes underneath, marks over the top.
    fn wire_list(&mut self, pointer: Option<gpui::Point<Pixels>>) -> (Vec<WireDraw>, Vec<Mark>) {
        // Fresh before the borrow is split, because rebuilding it wants `self`
        // whole and the closure below wants only a piece.
        self.index();

        let vp = self.viewport;
        let theme = self.theme;
        let settled = matches!(self.gesture, Gesture::None);
        let visible = vp.visible();
        let chosen = self.rope.clone();

        // The one setting that turns the whole feature off. Checked here rather
        // than at the painter so that a board with the lines switched off costs
        // nothing to route as well as nothing to draw — including the selection
        // set below, which on a select-all is a walk of every card and has no
        // business being built for a feature that is off.
        if !self.doc.board.settings.desktop.web {
            self.wires.forget();
            self.drawn.clear();
            return (Vec::new(), Vec::new());
        }

        // A set rather than the list, for the same reason `draw_list` keeps
        // one: `lit` is asked once per connection, and a walk of the selection
        // inside it makes a board with everything selected cost connections
        // times cards. Borrowed, like `draw_list`'s, rather than a clone of
        // every selected id per frame.
        let selection: HashSet<&str> = self.selection.iter().map(String::as_str).collect();
        // The label being typed, if one is, so the rope shows what is being
        // written on it rather than what it said before the session started.
        let typing = match &self.editing {
            Some(open) => match &open.on {
                Subject::Rope(a, b) => Some((a.clone(), b.clone(), open.editor.text().to_string())),
                Subject::Card(..) => None,
            },
            None => None,
        };

        // Read before the borrow is split, because `revision` lives on the
        // state and the destructuring below hands out only the board.
        let revision = self.doc.board.revision();
        let Self { doc, grid, wires, .. } = self;
        let items = &doc.board.items;
        let drawn = wires.plan(
            &doc.board,
            revision,
            visible,
            settled,
            |c| {
                chosen
                    .as_ref()
                    .is_some_and(|(a, b)| (a == &c.a && b == &c.b) || (a == &c.b && b == &c.a))
                    || selection.contains(c.a.as_str())
                    || selection.contains(c.b.as_str())
            },
            |near| {
                // Only what a rope could actually be drawn behind. A fence is
                // furniture the board draws around rather than something in
                // the way, and routing every line round the fence enclosing
                // both its ends would be a detour to nowhere.
                let mut found = Vec::new();
                grid.in_rect(near, &mut found);
                found
                    .into_iter()
                    .map(|n| &items[n as usize])
                    .filter(|it| it.kind.is_content())
                    .map(Rect::of_item)
                    .collect()
            },
        );

        let on_screen = |p: WorldPoint| {
            let s = vp.to_screen(p);
            gpui::point(px(s.x), px(s.y))
        };

        let mut out: Vec<WireDraw> = Vec::with_capacity(drawn.len() + 1);
        for wire in &drawn {
            let colour = {
                let base = theme.rope_for(wire.meta.color);
                if wire.lit {
                    base
                } else {
                    base.opacity(0.62)
                }
            };
            let selected = chosen.as_ref().is_some_and(|(a, b)| {
                (a == &wire.a && b == &wire.b) || (a == &wire.b && b == &wire.a)
            });
            let plot = |line: &mbrd_core::route::Line| -> Vec<gpui::Point<Pixels>> {
                match line {
                    mbrd_core::route::Line::Curve(rope) => {
                        rope.samples().into_iter().map(on_screen).collect()
                    }
                    mbrd_core::route::Line::Around(path) => {
                        path.iter().copied().map(on_screen).collect()
                    }
                }
            };
            let points = plot(&wire.line);

            // A line that has just changed shape is drawn twice for a fifth of
            // a second, each at the strength the other is not. Under the new
            // one, so that the shape arriving is the one on top — and with no
            // arrows and no label, because those belong to the line that is
            // staying and drawing them twice would double their weight.
            let (colour, going) = match &wire.leaving {
                Some((was, through)) => {
                    (colour.opacity(*through), Some((plot(was), colour.opacity(1.0 - *through))))
                }
                None => (colour, None),
            };
            if let Some((points, colour)) = going {
                out.push(WireDraw {
                    points,
                    colour,
                    half: wires::thickness(wire.meta.weight) / 2.0,
                    dash: wires::dashes(wire.meta.style),
                    arrows: Vec::new(),
                    label: None,
                    labelling: false,
                    selected: false,
                });
            }

            let mut arrows = Vec::new();
            let mut head = |t: f32, back: bool| {
                let (at, way) = wire.at(t);
                let way = if back { point(-way.x, -way.y) } else { way };
                // The heading is in world units, where y points up, and the
                // arrow is drawn in screen units, where it does not. One
                // negation, here, rather than a flip inside the painter.
                arrows.push((on_screen(at), gpui::point(px(way.x), px(-way.y))));
            };
            match wire.meta.dir {
                mbrd_core::model::ConnDir::None => {}
                mbrd_core::model::ConnDir::Fwd => head(1.0, false),
                mbrd_core::model::ConnDir::Back => head(0.0, true),
                mbrd_core::model::ConnDir::Both => {
                    head(1.0, false);
                    head(0.0, true);
                }
            }

            let being_typed = typing.as_ref().filter(|(a, b, _)| {
                (a == &wire.a && b == &wire.b) || (a == &wire.b && b == &wire.a)
            });
            let words = match being_typed {
                Some((_, _, text)) => Some(text.clone()),
                None => wire.meta.label.clone(),
            };
            let label = words
                .filter(|_| vp.zoom > LABEL_ZOOM)
                .map(|text| (SharedString::from(text), on_screen(wire.label_spot())));

            out.push(WireDraw {
                points,
                colour,
                half: wires::thickness(wire.meta.weight) / 2.0,
                dash: wires::dashes(wire.meta.style),
                arrows,
                label,
                labelling: being_typed.is_some(),
                selected,
            });
        }

        // The rope being drawn, over everything already on the board. Not a
        // connection yet — it becomes one at the release, or it becomes
        // nothing — so it is built here rather than pushed through `wires`.
        if let Gesture::Roping { from, side, at, over } = &self.gesture {
            if let Some(item) = self.doc.board.item(from) {
                let start = Rect::of_item(item);
                let rope = match over.as_ref().and_then(|id| self.doc.board.item(id)) {
                    // Landed on a card: snap to the face it would actually
                    // arrive at, so the stroke shows the rope you are about to
                    // get rather than one ending at the cursor.
                    Some(target) => {
                        let to = Rect::of_item(target);
                        rope::Rope::between(start, *side, to, rope::facing(start, to).1)
                    }
                    None => rope::Rope::loose(start, *side, *at),
                };
                out.push(WireDraw {
                    points: rope.samples().into_iter().map(on_screen).collect(),
                    colour: theme.accent,
                    half: 1.25,
                    dash: if over.is_some() { None } else { Some((7.0, 5.0)) },
                    arrows: Vec::new(),
                    label: None,
                    labelling: false,
                    selected: false,
                });
            }
        }

        // The marks. Only on cards big enough to wear them, which is the same
        // rule `anchor::at` applies — something you cannot see must not be
        // something you can press.
        let mut marks = Vec::new();
        {
            let local = pointer.map(|p| {
                point(
                    f(p.x) - f(self.canvas_bounds.origin.x),
                    f(p.y) - f(self.canvas_bounds.origin.y),
                )
            });
            // Driven by the fade rather than by `offering`, which is what lets
            // a card that has stopped being offered still be on screen on its
            // way out. A card whose fade has reached zero is not in here at
            // all, so the ordinary board pays nothing for this.
            for (id, fade) in &self.anchor_fade {
                let Some(item) = self.doc.board.item(id) else { continue };
                let card = Rect::of_item(item);
                if anchor::too_small(card, &vp) {
                    continue;
                }
                for side in Side::ALL {
                    let spot = anchor::spot(side, card, &vp);
                    // Only a mark that is really there answers to the pointer.
                    // Lighting one that is a tenth of the way in would be
                    // promising a press that `anchor::at` — which knows
                    // nothing about fades — is about to honour anyway, so the
                    // threshold is low and exists only to stop a mark on its
                    // way out looking pressable.
                    let lit = *fade > 0.5
                        && local.is_some_and(|p| {
                            (p.x - spot.x).abs() <= anchor::REACH
                                && (p.y - spot.y).abs() <= anchor::REACH
                        });
                    marks.push(Mark { at: gpui::point(px(spot.x), px(spot.y)), lit, fade: *fade });
                }
            }
        }

        self.drawn = drawn;
        (out, marks)
    }

    /// Reduce every visible card to the handful of numbers the painter needs.
    ///
    /// Two things are happening here and both are load-bearing. The obvious one
    /// is level of detail: a card four pixels across gets one quad and nothing
    /// else, and the thresholds are in the `LOD_*` constants above.
    ///
    /// The less obvious one is the boundary itself. A canvas's painter is
    /// `'static` — it outlives this call and cannot borrow the board — so
    /// whatever it draws has to be *owned* by it. Handing it the items would
    /// mean cloning twenty thousand of them into every frame. So the cull comes
    /// first and only what survived it is copied, which bounds the copying by
    /// the size of the window rather than the size of the board.
    fn draw_list(&mut self, scale: f32, cx: &mut Context<Self>) -> Vec<Draw> {
        // Cloned once for the frame rather than borrowed: the loop below takes
        // `&mut self` for the image and sound caches, and an `Rc` bump is what
        // a wrap costs to be measured against the same face the painter uses.
        let measure = self.measure.clone();
        let vp = self.viewport;
        let theme = self.theme;
        let board_fit = self.doc.board.media_fit.clone();
        // Cloned out of `self`, because the loop below holds the board
        // immutably while `begin_decode` at the end wants it mutably.
        let editing = self.editing.clone();
        let visible = self.visible_by_depth();

        // Hover feedback for the strip's own buttons — read from the *last*
        // frame's drawn strips, the same way `controls_at` always is, and
        // before `self.drawn_controls` is cleared for this one below. `None`
        // mid-gesture for the same reason a card's own hover is: a marquee
        // sweeping across a card is not a reason for its play button to light
        // up. The press, unlike the hover, does not need a gesture check —
        // `pressed_control` is only ever set by a press that landed on one of
        // these three buttons in the first place.
        let hover_control =
            matches!(self.gesture, Gesture::None).then(|| self.controls_at(self.pointer)).flatten();
        let press_control = self.pressed_control.clone();

        // The selection as a set, once, rather than a walk of it per card.
        //
        // A linear scan is the right shape for the one or two cards a
        // selection usually holds and the wrong one for the case that hurts:
        // select everything on a full board and drag it, and a scan per
        // visible card against a selection of twenty thousand is the product
        // of the two, every frame. Building the set is one pass over the
        // selection and makes the loop below flat in it.
        let picked: HashSet<&str> = self.selection.iter().map(String::as_str).collect();
        let stepped_into: HashSet<&str> = self.inside.iter().map(String::as_str).collect();
        // What an in-progress sweep would add to the selection, borrowed
        // rather than walked per card for the same reason `picked` is. Empty
        // whenever the gesture is not a marquee, so a plain lookup below
        // reads as "not being swept" without a match of its own.
        let marqueed: HashSet<&str> = match &self.gesture {
            Gesture::Marquee { provisional, .. } => {
                provisional.iter().map(String::as_str).collect()
            }
            _ => HashSet::new(),
        };

        let mut wanted: Vec<String> = Vec::new();
        // Item ids, not hashes — unlike `wanted`, a mesh's picture depends on
        // its orbit as well as its bytes, so this is keyed the way
        // `mesh_cache::Meshes::resting` is. See `picture_hash`'s own doc for
        // why `ItemType::Model` never appears there.
        let mut mesh_wanted: Vec<String> = Vec::new();
        let mut out = Vec::with_capacity(visible.len());
        // Rebuilt from nothing every frame, like `out` itself: a control on a
        // card that has been deleted, moved off screen or shrunk past the
        // threshold must stop being pressable on the same frame it stops being
        // drawn.
        self.drawn_controls.clear();

        for i in visible {
            let item = &self.doc.board.items[i as usize];
            // Drawn where the model says, plus whatever an arrange still
            // owes this card's presentation — see `Self::present_move`. The
            // model position underneath is exact from the first frame; only
            // where this paints is still catching up.
            let (catch_x, catch_y) = self
                .presenting
                .get(item.id.as_str())
                .map(|(sx, sy)| (sx.value(), sy.value()))
                .unwrap_or((0.0, 0.0));
            let centre = vp.to_screen(point(item.x + catch_x, item.y + catch_y));
            let (w, h) = ((item.w * vp.zoom).max(1.0), (item.h * vp.zoom).max(1.0));
            let body = Bounds::new(
                gpui::point(px(centre.x - w / 2.0), px(centre.y - h / 2.0)),
                gpui::size(px(w), px(h)),
            );
            let smallest = w.min(h);
            let selected = picked.contains(item.id.as_str());
            // A set for the same reason `picked` is one, and a much smaller
            // one: nobody is ever forty groups deep.
            let entered = stepped_into.contains(item.id.as_str());
            // What a sweep in progress would add, if it is not already
            // selected — an already-selected card caught again by an
            // additive sweep has nothing left to preview.
            let previewed = !selected && marqueed.contains(item.id.as_str());

            // Dust. Not worth a border, a corner, a picture or a word — and at
            // this size a selected card still has to be visible, so the accent
            // replaces the fill rather than ringing it.
            if smallest < LOD_DUST {
                out.push(Draw {
                    body,
                    radius: px(0.0),
                    fill: if selected { theme.selected_edge } else { theme.colour_of(item) },
                    edge: theme.card_edge,
                    border: px(0.0),
                    selected: false,
                    picture: None,
                    controls: None,
                    lines: Vec::new(),
                    font_size: px(1.0),
                    pad: px(0.0),
                    text: theme.text,
                    caret: None,
                    highlight: Vec::new(),
                    marked: Vec::new(),
                    dust: true,
                    grips: false,
                    frame: false,
                    entered: false,
                    broken: false,
                    // No padlock at this size. A card below `LOD_DUST` is a
                    // few pixels across, and a mark on it would be the whole
                    // card — see the note above on what dust is worth.
                    lock: None,
                });
                continue;
            }

            let plain = smallest < LOD_PLAIN;
            let radius = if plain { px(0.0) } else { px((4.0 * vp.zoom).clamp(1.0, 8.0)) };
            let border = if plain { px(0.0) } else { px(1.0) };

            // The picture, if there is one and it is worth an atlas tile. The
            // asked-for hashes are collected rather than started here, because
            // starting a decode wants `cx` and this loop is holding the board.
            //
            // `Waiting` and `Failed` used to collapse into the same blank
            // card, forever, and there is no way to tell "still decoding" from
            // "these bytes are not a picture" by looking at one. Only `Failed`
            // sets `broken` — a decode still in flight is not an answer yet
            // and must not flash a warning on every card while it works.
            let mut broken = false;
            let found = if smallest < LOD_PICTURE {
                None
            } else if item.kind == ItemType::Model {
                // Sharp past the same threshold a photograph switches tiers
                // at — see `images::Images::look` — so a mesh card zoomed
                // in or dragged large does not stay pinned to a 256px thumb.
                let wanted = w.max(h) * scale;
                match self.live.get(item.id.as_str()).cloned().or_else(|| {
                    self.meshes.resting(item.id.as_str()).map(|d| {
                        if wanted > THUMB_SIDE as f32 {
                            d.sharp.clone().unwrap_or_else(|| d.thumb.clone())
                        } else {
                            d.thumb.clone()
                        }
                    })
                }) {
                    Some(image) => {
                        let fit = item
                            .meta
                            .get("fit")
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or(&board_fit);
                        let size = image.size(0);
                        let aspect = size.width.0.max(1) as f32 / size.height.0.max(1) as f32;
                        // A mesh's own decode is never `Failed` the way a
                        // picture's can be — nothing here would draw at all
                        // if the bytes could not be parsed as one of the
                        // formats `import.rs` already accepted — and it never
                        // fades in, since there is no thumb-then-sharp arrival
                        // to animate.
                        Some((image, fit_into(body, aspect, fit == "cover"), 1.0))
                    }
                    None => {
                        mesh_wanted.push(item.id.clone());
                        None
                    }
                }
            } else {
                match picture_hash(item) {
                    Some(hash) => match self.images.look(hash, w.max(h) * scale) {
                        Load::Ready(image, arrived) => {
                            let fit = item
                                .meta
                                .get("fit")
                                .and_then(serde_json::Value::as_str)
                                .unwrap_or(&board_fit);
                            let size = image.size(0);
                            let aspect = size.width.0.max(1) as f32 / size.height.0.max(1) as f32;
                            Some((image, fit_into(body, aspect, fit == "cover"), arrived))
                        }
                        Load::Cold => {
                            wanted.push(hash.to_string());
                            None
                        }
                        Load::Waiting => None,
                        Load::Failed => {
                            broken = true;
                            None
                        }
                    },
                    None => None,
                }
            };

            // What this card plays, if anything, and where its playhead is.
            //
            // Asked after the picture rather than before it because the answer
            // depends on what the decode turned out to be: a `.gif` that holds
            // one frame is a photograph, and only the frames can say so.
            let sound = has_sound(&mut self.sound, &self.doc.assets, item);
            // Only this card's own button, and only the three the painter
            // washes for — a scrub or a volume level surviving the filter
            // would be a wash `paint_controls` was never asked to draw.
            let for_this = |held: &Option<(String, transport::Hit)>| {
                held.as_ref().filter(|(id, _)| id == &item.id).map(|(_, hit)| *hit).filter(|hit| {
                    matches!(
                        hit,
                        transport::Hit::PlayPause | transport::Hit::Mute | transport::Hit::Looping
                    )
                })
            };
            let controls = controls_for(
                &mut self.media,
                &mut self.timings,
                self.volume_on.as_deref(),
                item,
                body,
                found.as_ref().map(|(image, ..)| image),
                self.hovering.as_deref() == Some(item.id.as_str()),
                sound,
                for_this(&hover_control),
                for_this(&press_control),
            );
            if let Some(controls) = &controls {
                self.drawn_controls.push(Drawn {
                    id: item.id.clone(),
                    strip: controls.strip,
                    volume: controls.volume,
                    length: controls.length,
                    looping: controls.looping,
                    sound,
                    moves: controls.moves,
                });
            }

            let picture = found.map(|(image, at, arrived)| {
                let frame = match image.frame_count() > 1 {
                    // Off the measured clock rather than a walk of the
                    // delays — see [`Timings`].
                    true => self.timings.of(&item.id, &image).frame_at(
                        self.media.at(&item.id),
                        mbrd_core::media::playback(item).looping,
                    ),
                    false => 0,
                };
                Picture { image, at, arrived, frame }
            });

            let (font_size, pad) = card_text(item, vp.zoom, h);

            // A card being typed into draws the editor's text rather than its
            // own, caret and all — and draws it over a picture, because
            // renaming a photograph is a thing you have to be able to see.
            let being_edited =
                editing.as_ref().filter(|open| open.on.card().is_some_and(|(id, _)| id == item.id));
            let (lines, caret, highlight, marked) = match being_edited {
                Some(open) => {
                    // Raw, and unstyled. What is being typed is the text
                    // itself, marks and all: a note *is* Markdown, so writing
                    // one means seeing the marks you are writing. Rendering
                    // them away under the caret would also move the caret,
                    // since the characters it counts would stop being the
                    // characters on the screen.
                    //
                    // Wrapped to the same columns the label wraps to, though,
                    // because a line running off the edge of the card is a
                    // line being typed blind. The rows are byte spans, so the
                    // caret and the wash keep their arithmetic — and a click
                    // is measured against the same rows in `place_caret`.
                    let rows = open.editor.wrapped(text_room(w, pad), font_size, &measure);
                    (
                        rows.iter()
                            .map(|&(from, to)| markdown::Line::plain(&open.editor.text()[from..to]))
                            .collect(),
                        Some(open.editor.caret_in(&rows)),
                        open.editor.highlight_in(&rows),
                        open.editor.marked_in(&rows),
                    )
                }
                // The label. Skipped where the card is too small to read, and
                // skipped where a picture already fills the card — a photograph
                // does not need its filename written across it.
                // `font_size` as well as the card's own size, because a card
                // whose words scale can be a comfortable forty pixels across
                // while the words on it are one — and a shaping is the most
                // expensive thing this loop can be asked for.
                None if picture.is_none()
                    && w > LOD_LABEL_W
                    && h >= font_size * leading(CARD_TEXT)
                    && font_size >= LOD_TEXT =>
                {
                    let inner_h = (h - pad * 2.0).max(1.0);
                    let room = text_room(w, pad);
                    // A budget in units of the *body* row, same as the height
                    // check above: `markdown::lay_out` charges a heading a
                    // multiple of one of these rather than its own precise
                    // pixel height, which is the coarse half of the
                    // arithmetic this module has always done for how many
                    // lines fit before the ellipsis — see the doc on
                    // `leading` for why the fine half, the actual stacking
                    // below, is not allowed to share this shortcut.
                    let rows =
                        (inner_h / (font_size * leading(CARD_TEXT))).floor().max(1.0) as usize;
                    let words = label_for(item);
                    let lines = match item.kind {
                        // A note is Markdown, and a card is where it is read.
                        // Everything else is a label — a filename with an
                        // underscore in it is a filename, not an italic.
                        ItemType::Note | ItemType::Text => {
                            markdown::lay_out(&words, room, font_size, rows, &measure)
                        }
                        _ => wrap(&words, room, font_size, rows, &measure)
                            .into_iter()
                            .map(markdown::Line::plain)
                            .collect(),
                    };
                    (lines, None, Vec::new(), Vec::new())
                }
                None => (Vec::new(), None, Vec::new(), Vec::new()),
            };

            // Handles, on what is selected and big enough to put them on. Not
            // on a turned card: nothing draws rotation yet, so a handle would
            // be somewhere the card visibly is not.
            let locked = item.locked();
            let grips = selected
                && !locked
                && item.rot == 0.0
                && w >= crate::grips::TOO_SMALL
                && h >= crate::grips::TOO_SMALL;
            // A fence is a wash rather than a block, so the grid and anything
            // behind it still read through.
            let fill = if item.kind == ItemType::Fence {
                theme.colour_of(item).opacity(0.22)
            } else {
                theme.colour_of(item)
            };

            out.push(Draw {
                body,
                radius,
                fill,
                edge: if selected {
                    theme.selected_edge
                } else if previewed {
                    // "Will be caught" reads differently from "is selected"
                    // on purpose — this is a preview of a release that has
                    // not happened yet, not a report of one that has.
                    theme.selected_edge.opacity(0.5)
                } else if entered {
                    // Louder than a resting fence and quieter than a selected
                    // one: being inside a group is a fact about where you are
                    // working, not about what a command would act on.
                    theme.accent
                } else if item.kind == ItemType::Fence {
                    theme.fence
                } else {
                    theme.card_edge
                },
                border,
                selected: selected && !plain,
                picture,
                controls,
                lines,
                font_size: px(font_size),
                pad: px(pad),
                text: theme.text,
                caret,
                highlight,
                marked,
                dust: false,
                grips,
                frame: item.kind == ItemType::Fence,
                entered,
                broken,
                // Black or white rather than the theme's ink, because this
                // one sits *on* the card — see `Theme::ink_on`. A fence is
                // the exception: its fill is a wash the board shows through,
                // so what the mark is really over is the ground.
                lock: locked.then(|| {
                    if item.kind == ItemType::Fence {
                        theme.text
                    } else {
                        theme.ink_on(fill)
                    }
                }),
            });
        }

        // The pictures nobody has decoded yet, and the ones whose sharp copy
        // has been let go of and is wanted back. The second list is why zooming
        // into a card that softened brings it back rather than leaving it soft
        // — see `Images::resharpen`, and it is taken whole: a hash it names has
        // already been marked as on its way, so one dropped here would never be
        // asked for again.
        for hash in self.images.resharpen() {
            self.begin_decode(&hash, cx);
        }
        // The cold ones are throttled. Zooming out over a big board can put
        // thousands of undecoded pictures on screen in one frame, and starting
        // them all at once copies every encoded file out of the archive in the
        // same instant — a memory spike the size of the board — while burying
        // the cores for pixels that are six wide. A card left cold stays cold,
        // and the next frame — which each landing decode requests — asks for
        // it again, so the queue drains at the pace the decodes land.
        for hash in wanted {
            if self.decoding >= DECODES_AT_ONCE {
                break;
            }
            self.begin_decode(&hash, cx);
        }
        // Not throttled the way the loop above is: a board with thousands of
        // undecoded photographs is ordinary, and one with anywhere near that
        // many meshes is not the case `mesh_cache` was written for — see its
        // own module doc. `Meshes::begin`'s claim still keeps a card already
        // in flight from being asked for twice.
        for id in mesh_wanted {
            self.begin_mesh_decode(&id, cx);
        }
        out
    }

    /// Start decoding one asset, off the thread that draws.
    ///
    /// The bytes are copied out of the archive rather than borrowed, because
    /// the decode happens on another thread and the board is not going with it.
    /// That is one copy of one encoded file, once, against a decode that is two
    /// orders of magnitude more expensive — and against the alternative, which
    /// is putting the whole document behind a lock.
    fn begin_decode(&mut self, hash: &str, cx: &mut Context<Self>) {
        if !self.images.begin(hash) {
            return;
        }
        let Some(asset) = self.doc.assets.get(hash) else {
            // Named by a card but not in the archive. That is a broken file
            // rather than a broken picture, and the card draws as a plain one.
            self.images.settle(hash, None);
            return;
        };
        let bytes = asset.bytes.clone();
        let hash = hash.to_string();
        self.decoding += 1;
        let decoding = cx.background_executor().spawn(async move { crate::images::decode(&bytes) });
        cx.spawn(async move |view, cx| {
            let decoded = decoding.await;
            // `ok()` rather than unwrap: the window can close while a decode is
            // in flight, and a picture nobody is going to look at is not an
            // error worth a panic.
            view.update(cx, |view, cx| {
                view.decoding = view.decoding.saturating_sub(1);
                view.images.settle(&hash, decoded);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Start rasterising a mesh at its current orbit, off the thread that
    /// draws — `begin_decode`'s shape, but through `mesh_cache` rather than
    /// `images`, since a mesh's picture depends on more than its bytes.
    ///
    /// Parses once per content hash (`self.meshes.parsed`) and reuses that
    /// parse for every orbit any card sharing the bytes is ever turned to —
    /// see `mesh_cache`'s own module doc for why a mesh cannot share
    /// `images.rs`'s cache outright.
    fn begin_mesh_decode(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.meshes.begin(id) {
            return;
        }
        let Some(item) = self.doc.board.item(id) else {
            self.meshes.settle(id);
            return;
        };
        let orbit = mbrd_core::media::orbit(item);
        let Some(hash) = item.asset.as_ref().and_then(ItemAsset::hash).map(str::to_string) else {
            self.meshes.settle(id);
            return;
        };
        let parsed = self.meshes.parsed(&hash);
        let bytes = match &parsed {
            Some(_) => None,
            None => match self.doc.assets.get(&hash) {
                Some(asset) => Some(asset.bytes.clone()),
                None => {
                    // Named by a card but not in the archive — a broken file
                    // rather than a broken picture, same as `begin_decode`.
                    self.meshes.settle(id);
                    return;
                }
            },
        };
        let id = id.to_string();
        let task = cx.background_executor().spawn(async move {
            let mesh = parsed.or_else(|| bytes.as_deref().and_then(crate::mesh_cache::parse));
            let decoded =
                mesh.as_ref().and_then(|mesh| crate::mesh_cache::rasterize_tiers(mesh, orbit));
            (mesh, decoded)
        });
        cx.spawn(async move |view, cx| {
            let (mesh, decoded) = task.await;
            view.update(cx, |view, cx| {
                if let Some(mesh) = mesh {
                    view.meshes.cache_parsed(&hash, mesh);
                }
                if let Some(decoded) = decoded {
                    view.meshes.set_resting(&id, decoded);
                    // Left alone for a card being dragged right now — that
                    // frame is `live`'s and newer than what this decode
                    // started from, and clearing it here would flash the
                    // picture back to whatever `resting` just became.
                    let dragging =
                        matches!(&view.gesture, Gesture::Orbiting { id: gid, .. } if gid == &id);
                    if !dragging {
                        view.live.clear(&id);
                    }
                }
                view.meshes.settle(&id);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// A mesh's current picture for the open page — live if it is being
    /// turned, otherwise its last-rasterised resting frame — kicking a
    /// background decode where neither has one yet.
    fn mesh_picture(
        &mut self,
        id: &str,
        sharp: bool,
        cx: &mut Context<Self>,
    ) -> Option<Arc<RenderImage>> {
        if let Some(frame) = self.live.get(id) {
            return Some(frame.clone());
        }
        match self.meshes.resting(id) {
            Some(decoded) => Some(if sharp {
                decoded.sharp.clone().unwrap_or_else(|| decoded.thumb.clone())
            } else {
                decoded.thumb.clone()
            }),
            None => {
                self.begin_mesh_decode(id, cx);
                None
            }
        }
    }

    /// The whole board, in one painted layer.
    ///
    /// Painted rather than built from elements, and that is the difference
    /// between this and the version it replaces. A card as a `div` is a laid-out
    /// box, a style resolution and a hit-test region, for something that is a
    /// rectangle and never receives an event — this module's whole premise is
    /// that input is handled in one place, so the per-card machinery was being
    /// paid for nothing. The grid dots were already painted for the same reason
    /// and there were only ever hundreds of those.
    ///
    /// Order is back to front and it matters: ground, then cards, then the
    /// marquee over everything, so a sweep is visible over the cards it is
    /// catching.
    #[allow(clippy::too_many_arguments)]
    fn paint_board(
        &self,
        draws: Vec<Draw>,
        wires: Vec<WireDraw>,
        marks: Vec<Mark>,
        font: Font,
        cursor: gpui::CursorStyle,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let vp = self.viewport;
        let theme = self.theme;
        let theme_grid = self.theme.grid_at(vp.zoom);
        let theme_axis = self.theme.axis;
        let accent = self.theme.accent;
        let step = self.doc.board.settings.desktop.grid_step;
        let show_grid = self.doc.board.settings.desktop.grid;
        let show_axes = self.doc.board.settings.desktop.axes;
        let paper_id = self.doc.board.settings.desktop.paper.clone();
        let paper_landscape = self.doc.board.settings.desktop.paper_landscape;
        let paper_scale = self.doc.board.settings.desktop.scale;
        let marquee = match &self.gesture {
            Gesture::Marquee { from, to, .. } => Some((*from, *to)),
            _ => None,
        };
        // What the drag lined up with, and the chip beside the pointer that
        // says what the drag *is*. Both are cloned out of `self` here rather
        // than reached for inside the painter, which is `'static` and cannot
        // borrow the view. See `draw_list` for the same boundary.
        let guides = match &self.gesture {
            Gesture::Moving { guides, .. } => guides.clone(),
            _ => Snap::default(),
        };
        let badge = self.badge();
        let pointer = self.pointer;

        let entity = cx.entity();
        // For the input handler, which is installed in the paint pass below
        // and only while there is something to type into.
        let typing = self.editing.as_ref().map(|_| (cx.entity(), self.focus_handle.clone()));

        canvas(
            move |bounds, window, cx| {
                // Measuring rather than assuming the sidebar's width. Only
                // notify when it actually changed, or this would redraw forever.
                entity.update(cx, |this, cx| {
                    let size = ViewSize {
                        width: f(bounds.size.width).max(1.0),
                        height: f(bounds.size.height).max(1.0),
                    };
                    if this.canvas_bounds != bounds || this.viewport.size != size {
                        this.canvas_bounds = bounds;
                        this.viewport.size = size;
                        // An open menu was fitted to the window as it stood
                        // when it opened, and this is the only place the app
                        // hears that it is no longer that window. Put away
                        // rather than refitted: a menu that jumped out from
                        // under the pointer because something re-tiled the
                        // window underneath it would be worse than one that
                        // went away. See `menu.rs`.
                        this.close_menu();
                        cx.notify();
                    }
                });
                // A region for the pointer to be over, which is the only way
                // to ask for a cursor: gpui hands the shape to a *hitbox*
                // rather than to an element, so that two overlapping claims
                // resolve the same way a click would.
                (bounds, window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal))
            },
            move |_, (bounds, hitbox), window, cx| {
                // What a press here would do, said before it is made. The
                // canvas is one element covering the whole board, so this is
                // the only place the app can say it — and `cursor_at` has
                // already worked out which of the many things under the
                // pointer would win.
                window.set_cursor_style(cursor, &hitbox);

                // The composing keyboard's way in — see the
                // `EntityInputHandler` impl for the whole of what this is.
                // Here rather than anywhere nicer because gpui takes it during
                // paint and asserts as much, and *only while editing* because
                // a handler standing on a board nobody is typing into would
                // take the `key_char` of every shortcut on it.
                if let Some((entity, focus)) = typing.clone() {
                    window.handle_input(&focus, gpui::ElementInputHandler::new(bounds, entity), cx);
                }

                let origin = bounds.origin;
                // The measured size, not the stale one on `vp`: on the very
                // first frame those disagree, and drawing to the stale one
                // puts the grid a sidebar's width off for one frame.
                let vp = Viewport {
                    size: ViewSize {
                        width: f(bounds.size.width).max(1.0),
                        height: f(bounds.size.height).max(1.0),
                    },
                    ..vp
                };
                let visible = vp.visible();

                if show_grid && theme_grid.a > 0.001 {
                    // Quantise the step so that zooming out does not ask for a
                    // dot every fraction of a pixel. Each doubling keeps the
                    // spacing on screen inside one octave.
                    let mut world_step = step;
                    while world_step * vp.zoom < 12.0 {
                        world_step *= 2.0;
                    }
                    // And keep doubling until the count is one a frame can
                    // afford. This used to be a hard `if` that drew *nothing*
                    // above the ceiling, which meant the grid vanished
                    // altogether on a large or high-density window rather than
                    // getting coarser — the one shape of failure a fallback is
                    // supposed to prevent. Halving both counts per turn, this
                    // settles in a handful of them.
                    let (mut cols, mut rows);
                    loop {
                        cols = ((visible.width() / world_step).ceil() as i64 + 2).max(0);
                        rows = ((visible.height() / world_step).ceil() as i64 + 2).max(0);
                        if cols * rows <= MOST_DOTS || !world_step.is_finite() {
                            break;
                        }
                        world_step *= 2.0;
                    }
                    let dot = (1.5 * vp.zoom).clamp(1.0, 3.0);
                    let first_x = (visible.x0 / world_step).floor() * world_step;
                    let first_y = (visible.y0 / world_step).floor() * world_step;
                    // One layer for the lot, and this is the single most
                    // expensive line in the frame by a wide margin.
                    //
                    // gpui gives every primitive its draw order by inserting
                    // that primitive's box into a bounds tree, and for a field
                    // of dots the insert *is* the cost of the dot: measured on
                    // a 1600×900 window, ten thousand of them spend eleven
                    // milliseconds a frame in `BoundsTree::insert` alone —
                    // the whole budget, before a single card is drawn, on a
                    // background that is the same background it was last
                    // frame. Worse, the tree's cost grows faster than the
                    // count, so a bigger window is punished twice.
                    //
                    // A layer is exactly the escape gpui provides: a batch of
                    // geometry that does not overlap and shares one order, so
                    // the tree is asked once instead of ten thousand times.
                    // The dots qualify by construction — a grid does not
                    // overlap itself, and the axes are stepped over below
                    // rather than drawn under.
                    window.paint_layer(bounds, |window| {
                        for cx_i in 0..cols {
                            let wx = first_x + cx_i as f32 * world_step;
                            for cy_i in 0..rows {
                                let wy = first_y + cy_i as f32 * world_step;
                                // The origin is a multiple of the step, so a
                                // whole row and a whole column of dots land
                                // exactly on the axes. A dot is wider than the
                                // line, so it does not hide under it — it beads
                                // it. Leave the gap and let the line be a line.
                                if show_axes && (wx == 0.0 || wy == 0.0) {
                                    continue;
                                }
                                let s = vp.to_screen(point(wx, wy));
                                window.paint_quad(fill(
                                    Bounds::new(
                                        gpui::point(
                                            origin.x + px(s.x - dot / 2.0),
                                            origin.y + px(s.y - dot / 2.0),
                                        ),
                                        gpui::size(px(dot), px(dot)),
                                    ),
                                    theme_grid,
                                ));
                            }
                        }
                    });
                }

                if show_axes {
                    let o = vp.to_screen(point(0.0, 0.0));
                    if o.y >= 0.0 && o.y <= vp.size.height {
                        window.paint_quad(fill(
                            Bounds::new(
                                gpui::point(origin.x, origin.y + px(o.y)),
                                gpui::size(px(vp.size.width), px(1.0)),
                            ),
                            theme_axis,
                        ));
                    }
                    if o.x >= 0.0 && o.x <= vp.size.width {
                        window.paint_quad(fill(
                            Bounds::new(
                                gpui::point(origin.x + px(o.x), origin.y),
                                gpui::size(px(1.0), px(vp.size.height)),
                            ),
                            theme_axis,
                        ));
                    }
                }

                // A sheet of real-world paper, outlined around the origin —
                // "outlined" and not filled, because it is a reference mark
                // for what will print true size, not a background the board
                // is supposed to sit on. Same colour as the axes: both are
                // structure the board is drawn against rather than content
                // on it, and the eye should file them together.
                if let Some((w, h)) =
                    mbrd_core::paper::outline(&paper_id, paper_landscape, paper_scale)
                {
                    let top_left = vp.to_screen(point(-w / 2.0, -h / 2.0));
                    let bottom_right = vp.to_screen(point(w / 2.0, h / 2.0));
                    window.paint_quad(quad(
                        Bounds::new(
                            gpui::point(origin.x + px(top_left.x), origin.y + px(top_left.y)),
                            gpui::size(
                                px(bottom_right.x - top_left.x),
                                px(bottom_right.y - top_left.y),
                            ),
                        ),
                        px(0.0),
                        gpui::transparent_black(),
                        px(1.0),
                        theme_axis,
                        BorderStyle::Solid,
                    ));
                }

                // The lines, under the cards. That is what makes an elbow
                // drawn behind something read as a connector rather than as
                // damage: the wire goes under the card and out the other side,
                // which is what a wire behind a photograph does.
                for wire in &wires {
                    let at = |p: &gpui::Point<Pixels>| gpui::point(origin.x + p.x, origin.y + p.y);
                    let runs: Vec<[gpui::Point<Pixels>; 2]> = match wire.dash {
                        None => wire.points.windows(2).map(|w| [at(&w[0]), at(&w[1])]).collect(),
                        Some((on, off)) => dashed(&wire.points, on, off)
                            .into_iter()
                            .map(|[a, b]| [at(&a), at(&b)])
                            .collect(),
                    };
                    if runs.is_empty() {
                        continue;
                    }
                    // A selected rope is drawn twice: a wide soft pass
                    // underneath and the line itself over it. A ring the way a
                    // card gets one would have to be a ring around a curve,
                    // and there is no such shape.
                    if wire.selected {
                        if let Some(path) = ribbon(&runs, wire.half + 3.0) {
                            window.paint_path(path, accent.opacity(0.35));
                        }
                    }
                    if let Some(path) = ribbon(&runs, wire.half) {
                        window.paint_path(path, wire.colour);
                    }
                    for (tip, way) in &wire.arrows {
                        window.paint_path(
                            arrowhead(at(tip), *way, wire.half * 3.0 + 3.0),
                            wire.colour,
                        );
                    }
                }

                // Grouped rather than walked one at a time, and the group
                // is a *run* of neighbours rather than every dust card on the
                // board: `draws` is in depth order, so a run keeps its place
                // in that order while a hoist of all the dust to the back
                // would not. Zoomed far enough out that everything is dust —
                // which the range now reaches — that run is the whole board
                // and this is one layer instead of twenty thousand inserts.
                // See the grid below for the measurement that argument rests
                // on; the primitives inside a layer share its order and keep
                // the order they were pushed in, because the scene sorts them
                // by a stable sort.
                for run in draws.chunk_by(|a, b| a.dust && b.dust) {
                    if run[0].dust {
                        window.paint_layer(bounds, |window| {
                            for draw in run {
                                window.paint_quad(quad(
                                    shift(draw.body, origin),
                                    px(0.0),
                                    draw.fill,
                                    px(0.0),
                                    draw.edge,
                                    BorderStyle::Solid,
                                ));
                            }
                        });
                        continue;
                    }
                    // A run of cards that are not dust is one card: the
                    // predicate above only holds between two that are.
                    let draw = &run[0];
                    let body = shift(draw.body, origin);
                    window.paint_quad(quad(
                        body,
                        draw.radius,
                        draw.fill,
                        if draw.frame { draw.border.max(px(2.0)) } else { draw.border },
                        draw.edge,
                        // A fence you are standing inside is drawn solid. The
                        // dashes are what say "this is a region rather than a
                        // card", and having stepped in, it is the thing you are
                        // working in rather than a region you are looking at.
                        if draw.frame && !draw.entered {
                            BorderStyle::Dashed
                        } else {
                            BorderStyle::Solid
                        },
                    ));

                    if let Some(picture) = &draw.picture {
                        let (image, arrived) = (&picture.image, picture.arrived);
                        let at = shift(picture.at, origin);
                        // Clipped to the card, because `cover` deliberately
                        // computes a rectangle larger than the card in one
                        // axis and the overflow is the part being cropped.
                        window.with_content_mask(Some(ContentMask { bounds: body }), |window| {
                            // Best effort: an atlas that will not take another
                            // tile should cost this frame a picture, not the
                            // whole frame.
                            let _ = window.paint_image(
                                at,
                                draw.radius.into(),
                                image.clone(),
                                picture.frame,
                                false,
                            );
                        });
                        // The card's own colour, laid back over the picture and
                        // taken away again — a dissolve from the placeholder to
                        // the photograph rather than a fade from nothing, so
                        // the card never goes darker or lighter than the two
                        // things it is between.
                        //
                        // Done this way round because `paint_image` has no
                        // opacity of its own: the thing that can be faded here
                        // is the quad, so the quad is what moves.
                        if arrived < 1.0 {
                            window.paint_quad(quad(
                                body,
                                draw.radius,
                                draw.fill.opacity(1.0 - arrived),
                                px(0.0),
                                gpui::transparent_black(),
                                BorderStyle::Solid,
                            ));
                        }
                    } else if draw.broken {
                        // Corrupt bytes, or a format nothing here reads — an
                        // answer, centred and at reduced opacity, rather than
                        // the indefinite wait a blank card leaves somebody in
                        // while they cannot tell it apart from a picture that
                        // is merely still decoding. See `Load::Failed`.
                        let side = TRANSPORT_ICON.min(f(body.size.width) * 0.4);
                        let half = side / 2.0;
                        let (mx, my) = (
                            f(body.origin.x) + f(body.size.width) / 2.0,
                            f(body.origin.y) + f(body.size.height) / 2.0,
                        );
                        let mark = Bounds::new(
                            gpui::point(px(mx - half), px(my - half)),
                            gpui::size(px(side), px(side)),
                        );
                        let _ = window.paint_svg(
                            mark,
                            Icon::Warned.path().into(),
                            gpui::TransformationMatrix::unit(),
                            draw.text.opacity(0.5),
                            cx,
                        );
                    }

                    // The padlock, in the top corner of a card the author has
                    // nailed down. Faint on purpose: it is a fact about the
                    // card rather than a control, and a solid badge stamped
                    // over somebody's photograph would be the app writing on
                    // their board. The colour already answers to what it is
                    // sitting on — see `Draw::lock`.
                    //
                    // Skipped on a card too small to carry it, for the reason
                    // the transport strip is: a mark that has to shrink to fit
                    // is a smudge, and a smudge says less than nothing.
                    if let Some(ink) = draw.lock {
                        let side = LOCK_MARK.min(f(body.size.width) * 0.35);
                        if side >= LOCK_MARK * 0.6 {
                            let pad = f(draw.radius).max(4.0);
                            let mark = Bounds::new(
                                gpui::point(
                                    body.origin.x + body.size.width - px(side + pad),
                                    body.origin.y + px(pad),
                                ),
                                gpui::size(px(side), px(side)),
                            );
                            let _ = window.paint_svg(
                                mark,
                                Icon::Locked.path().into(),
                                gpui::TransformationMatrix::unit(),
                                ink.opacity(0.45),
                                cx,
                            );
                        }
                    }

                    if draw.selected {
                        // A ring outside the card rather than a thicker border
                        // inside it, so that selecting something does not
                        // change where its own edge appears to be.
                        window.paint_quad(quad(
                            body.dilate(px(2.0)),
                            draw.radius + px(2.0),
                            gpui::transparent_black(),
                            px(2.0),
                            draw.edge,
                            BorderStyle::Solid,
                        ));
                    }

                    if draw.grips {
                        // Four dots, at the corners, and nothing on the edges.
                        // The edges are still draggable — see `Grip::at`, which
                        // takes an edge along its whole run — they simply do not
                        // need announcing, and eight pieces of furniture around
                        // every selected card was most of what made this look
                        // like a diagram rather than a moodboard.
                        //
                        // Drawn last of the card's own pieces so they sit over
                        // its edge, and centred on the corner so they read as
                        // something to grab rather than as part of the picture.
                        let half = px(crate::grips::DOT / 2.0);
                        for grip in Grip::CORNERS {
                            let spot = grip_spot(grip, body);
                            window.paint_quad(quad(
                                Bounds::new(
                                    gpui::point(spot.0 - half, spot.1 - half),
                                    gpui::size(half * 2.0, half * 2.0),
                                ),
                                // Round, not square. A dot is quieter than a
                                // handle and says the same thing.
                                half,
                                theme.chrome,
                                px(1.5),
                                theme.selected_edge,
                                BorderStyle::Solid,
                            ));
                        }
                    }

                    if let Some(controls) = &draw.controls {
                        paint_controls(window, cx, controls, origin, &theme, &font);
                    }

                    if draw.lines.is_empty() {
                        continue;
                    }
                    let pad = draw.pad;
                    let inner_width = body.size.width - pad * 2.0;

                    // Shape every line once, up front. The caret, the selection
                    // wash and the glyphs all need the same measurements, and
                    // shaping is the expensive part of drawing text.
                    //
                    // A run per span rather than one per line, because a note
                    // is Markdown and the differences between its runs — face,
                    // colour, a wash, a line through — are things the shaper
                    // has to be told before it places a single glyph.
                    let shaped: Vec<(ShapedLine, Pixels)> = draw
                        .lines
                        .iter()
                        .map(|line| {
                            let size = draw.font_size * line.scale;
                            // A quote's bar and a rule's line, not the same
                            // colour as a secondary label — see
                            // [`Theme::quote`], which this used to share with
                            // `muted` for no reason beyond both being quiet.
                            let colour = if line.muted { theme.quote } else { draw.text };
                            let runs: Vec<TextRun> = line
                                .spans
                                .iter()
                                .map(|span| TextRun {
                                    len: span.text.len(),
                                    font: Font {
                                        weight: if span.style.bold {
                                            FontWeight::BOLD
                                        } else {
                                            font.weight
                                        },
                                        style: if span.style.italic {
                                            FontStyle::Italic
                                        } else {
                                            font.style
                                        },
                                        ..font.clone()
                                    },
                                    // The accent means "selected" everywhere
                                    // else on this board, and a link drawn in
                                    // it read as a selection sitting on a
                                    // card nobody had touched. `note_link` is
                                    // the same family of hue without the
                                    // borrowed meaning.
                                    color: if span.style.link { theme.note_link } else { colour },
                                    // A wash rather than a monospaced face:
                                    // asking for a family this build cannot be
                                    // sure is installed gets the body face
                                    // back and no way to know it happened.
                                    // 0.14 rather than a rounder number: it is
                                    // what `muted` at 0.16 used to land at by
                                    // accident, back when `muted` itself was
                                    // translucent and the two opacities
                                    // compounded — this keeps the wash the
                                    // same visible weight now that `muted` is
                                    // solid and no longer does that halving
                                    // for free.
                                    background_color: span
                                        .style
                                        .code
                                        .then(|| theme.muted.opacity(0.14)),
                                    underline: span.style.link.then_some(UnderlineStyle {
                                        thickness: px(1.0),
                                        color: None,
                                        wavy: false,
                                    }),
                                    strikethrough: span.style.strike.then_some(
                                        StrikethroughStyle { thickness: px(1.0), color: None },
                                    ),
                                })
                                .collect();
                            let shaped = window.text_system().shape_line(
                                line.text().into(),
                                size,
                                &runs,
                                None,
                            );
                            // Zoom-independent for the bracket, zoomed for the
                            // pixels it multiplies — see the doc on `leading`.
                            (shaped, size * leading(CARD_TEXT * line.scale))
                        })
                        .collect();

                    // Where each line starts, added up rather than multiplied
                    // out: a heading is taller than a body line, so the rows
                    // are not a grid. While a card is being typed into, every
                    // line is the body size and this is the old arithmetic to
                    // the pixel — which is what keeps the caret where the
                    // pointer put it.
                    let mut tops: Vec<Pixels> = Vec::with_capacity(shaped.len());
                    let mut stacked = px(0.0);
                    for (_, height) in &shaped {
                        tops.push(stacked);
                        stacked += *height;
                    }

                    // Slide the whole block sideways to keep the caret in
                    // sight. The block rather than the line, so that the text
                    // does not shear as the caret moves between rows — and only
                    // while typing, because a label that scrolled itself would
                    // be a label that moved when nothing had happened.
                    let slide = match draw.caret {
                        Some((row, column)) => shaped
                            .get(row)
                            .map(|(line, _)| {
                                let x = line.x_for_index(column);
                                if x > inner_width {
                                    inner_width - x
                                } else {
                                    px(0.0)
                                }
                            })
                            .unwrap_or(px(0.0)),
                        None => px(0.0),
                    };

                    window.with_content_mask(Some(ContentMask { bounds: body }), |window| {
                        let left = body.origin.x + pad + slide;
                        let top = |row: usize| body.origin.y + pad + tops[row];

                        // The wash goes under the glyphs, so it is a highlight
                        // rather than a redaction.
                        for &(row, from, to) in &draw.highlight {
                            let Some((line, height)) = shaped.get(row) else { continue };
                            let (x0, x1) = (line.x_for_index(from), line.x_for_index(to));
                            window.paint_quad(fill(
                                Bounds::new(
                                    gpui::point(left + x0, top(row)),
                                    gpui::size(x1 - x0, *height),
                                ),
                                theme.accent.opacity(0.35),
                            ));
                        }

                        for (row, (line, height)) in shaped.iter().enumerate() {
                            let _ = line.paint(gpui::point(left, top(row)), *height, window, cx);
                        }

                        // The composing run, underlined. Under the glyphs and
                        // after them, because it is a mark *about* the text
                        // rather than part of it — and because a keyboard
                        // still working out which characters these are should
                        // not be able to hide them.
                        for &(row, from, to) in &draw.marked {
                            let Some((line, height)) = shaped.get(row) else { continue };
                            let (x0, x1) = (line.x_for_index(from), line.x_for_index(to));
                            window.paint_quad(fill(
                                Bounds::new(
                                    gpui::point(left + x0, top(row) + *height - px(2.0)),
                                    gpui::size(x1 - x0, px(1.0)),
                                ),
                                draw.text,
                            ));
                        }

                        if let Some((row, column)) = draw.caret {
                            if let Some((line, height)) = shaped.get(row) {
                                window.paint_quad(fill(
                                    Bounds::new(
                                        gpui::point(left + line.x_for_index(column), top(row)),
                                        gpui::size(px(2.0), *height),
                                    ),
                                    theme.accent,
                                ));
                            }
                        }
                    });
                }

                // The marks, over the cards: an anchor is an offer the card is
                // making, and one drawn under the card next to it would be an
                // offer you could not see.
                for mark in &marks {
                    // Half a mark on its way in is drawn at half strength and
                    // half the size. Growing as it arrives is what makes it
                    // read as an offer being made rather than as a dot being
                    // turned up — the same thing arriving, not a different
                    // thing appearing.
                    let scale = 0.6 + 0.4 * mark.fade;
                    let half = px(anchor::DOT / 2.0 * scale);
                    let at = gpui::point(origin.x + mark.at.x, origin.y + mark.at.y);
                    let (fill_, edge) =
                        if mark.lit { (accent, accent) } else { (theme.anchor, theme.chrome_edge) };
                    window.paint_quad(quad(
                        Bounds::new(
                            gpui::point(at.x - half, at.y - half),
                            gpui::size(half * 2.0, half * 2.0),
                        ),
                        half,
                        fill_.opacity(mark.fade),
                        px(1.0),
                        edge.opacity(mark.fade),
                        BorderStyle::Solid,
                    ));
                }

                // The words on the lines, last of all, because a label that a
                // card was drawn over would be a label nobody could read.
                for wire in &wires {
                    let Some((text, at)) = &wire.label else { continue };
                    if text.is_empty() && !wire.labelling {
                        continue;
                    }
                    let run = TextRun {
                        len: text.len(),
                        font: font.clone(),
                        color: theme.text,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let size = px(LABEL_TEXT);
                    let shaped = window.text_system().shape_line(text.clone(), size, &[run], None);
                    let pad = px(LABEL_PAD);
                    let height = size * LABEL_LEADING;
                    let width = shaped.width + pad * 2.0;
                    let left = origin.x + at.x - width / 2.0;
                    let top = origin.y + at.y - height / 2.0;
                    // A chip behind it, in the board's own colour: a word laid
                    // straight over a line is a word with a line through it.
                    window.paint_quad(quad(
                        Bounds::new(gpui::point(left, top), gpui::size(width, height)),
                        px(4.0),
                        theme.chrome,
                        px(1.0),
                        if wire.labelling { accent } else { theme.chrome_edge },
                        BorderStyle::Solid,
                    ));
                    let _ = shaped.paint(
                        gpui::point(left + pad, top + (height - size) / 2.0),
                        height,
                        window,
                        cx,
                    );
                    if wire.labelling {
                        window.paint_quad(fill(
                            Bounds::new(
                                gpui::point(left + pad + shaped.width, top + px(3.0)),
                                gpui::size(px(2.0), height - px(6.0)),
                            ),
                            accent,
                        ));
                    }
                }

                if let Some((from, to)) = marquee {
                    let a = vp.to_screen(from);
                    let b = vp.to_screen(to);
                    let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
                    let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
                    let mut wash = accent;
                    wash.a = 0.14;
                    window.paint_quad(fill(
                        Bounds::new(
                            gpui::point(origin.x + px(x0), origin.y + px(y0)),
                            gpui::size(px(x1 - x0), px(y1 - y0)),
                        ),
                        wash,
                    ));
                }

                // What the drag lined up with, over everything: a rule drawn
                // under the card it is about is a rule about nothing. See
                // `core::guides`, which decides what these are; this only puts
                // them on the screen.
                //
                // A hairline. **One pixel across, whatever the zoom** — a guide
                // is a statement about a coordinate, and a band two pixels wide
                // is a statement about two of them. It is also the difference
                // between feedback and a stripe painted over somebody's board:
                // this used to grow its overhang on *both* axes, which made
                // every vertical rule a sixteen-pixel slab.
                let mark = theme.guide;
                for line in &guides.lines {
                    let (left, top, wide, tall) = guide_bar(*line, &vp);
                    window.paint_quad(fill(
                        Bounds::new(
                            gpui::point(origin.x + px(left), origin.y + px(top)),
                            gpui::size(px(wide), px(tall)),
                        ),
                        mark,
                    ));
                }

                // And the gaps it matched, as a bar with a tick at each end.
                // Two of them side by side saying the same number is the whole
                // message: these are the same distance apart.
                for span in &guides.spans {
                    let (a, b) = if span.horizontal {
                        (
                            vp.to_screen(point(span.from, span.across)),
                            vp.to_screen(point(span.to, span.across)),
                        )
                    } else {
                        (
                            vp.to_screen(point(span.across, span.from)),
                            vp.to_screen(point(span.across, span.to)),
                        )
                    };
                    let bar = |x0: f32, y0: f32, w: f32, h: f32, window: &mut Window| {
                        window.paint_quad(fill(
                            Bounds::new(
                                gpui::point(origin.x + px(x0), origin.y + px(y0)),
                                gpui::size(px(w.max(1.0)), px(h.max(1.0))),
                            ),
                            mark,
                        ));
                    };
                    let tick = 4.0;
                    if span.horizontal {
                        let (x0, x1) = (a.x.min(b.x), a.x.max(b.x));
                        bar(x0, a.y, x1 - x0, 1.0, window);
                        bar(x0, a.y - tick, 1.0, tick * 2.0, window);
                        bar(x1, a.y - tick, 1.0, tick * 2.0, window);
                    } else {
                        let (y0, y1) = (a.y.min(b.y), a.y.max(b.y));
                        bar(a.x, y0, 1.0, y1 - y0, window);
                        bar(a.x - tick, y0, tick * 2.0, 1.0, window);
                        bar(a.x - tick, y1, tick * 2.0, 1.0, window);
                    }
                }

                // The chip beside the pointer, last of everything, because it
                // is the one thing on the frame that is about the gesture
                // rather than about the board. See `BoardView::badge`.
                if let Some(text) = &badge {
                    let text: SharedString = text.clone().into();
                    let run = TextRun {
                        len: text.len(),
                        font: font.clone(),
                        color: theme.chrome,
                        background_color: None,
                        underline: None,
                        strikethrough: None,
                    };
                    let size = px(11.0);
                    let shaped = window.text_system().shape_line(text, size, &[run], None);
                    let pad = px(6.0);
                    let height = size * 1.6;
                    let width = shaped.width + pad * 2.0;
                    // Below and to the right, which is where a cursor's own
                    // hotspot is not — a chip centred on the pointer would be
                    // a chip under the pointer.
                    let gap = px(14.0);
                    let mut left = pointer.x + gap;
                    let mut top = pointer.y + gap;
                    // Flipped rather than clipped near an edge, so the readout
                    // is still readable in the corner somebody dragged into.
                    if left + width > bounds.origin.x + bounds.size.width {
                        left = pointer.x - gap - width;
                    }
                    if top + height > bounds.origin.y + bounds.size.height {
                        top = pointer.y - gap - height;
                    }
                    window.paint_quad(quad(
                        Bounds::new(gpui::point(left, top), gpui::size(width, height)),
                        px(4.0),
                        theme.text,
                        px(0.0),
                        theme.text,
                        BorderStyle::Solid,
                    ));
                    let _ = shaped.paint(
                        gpui::point(left + pad, top + (height - size) / 2.0),
                        height,
                        window,
                        cx,
                    );
                }
            },
        )
        .absolute()
        .size_full()
    }

    /// The one strip of chrome, along the bottom.
    ///
    /// This is where the panel down the left went. A moodboard is a thing you
    /// look at, and a permanent quarter of the window given over to counting
    /// what is on it was a quarter of the board you could not see — while every
    /// control in it was a second way to do something the keyboard already did.
    /// So the controls moved to the right button, where they are next to the
    /// thing they act on, and what is left is the counting: what is on this
    /// board, and how far into it you are.
    ///
    /// **Nothing here is a message.** The bar used to narrate — "moved 3",
    /// "saved kitchen", "undid rename" — a line at a time, four seconds each.
    /// Every one of those described something that had *already happened on the
    /// board in front of you*, so the narration was a second, slower copy of
    /// what you had just watched, and it arrived in the corner you were not
    /// looking at. A readout is the better shape for the same information: the
    /// bin count going from 2 to 5 says "3 to the bin" without a sentence, and
    /// says it for as long as it is true rather than for four seconds.
    ///
    /// The exception is anything the board *cannot* show: a failure, a download
    /// in flight, a key press that turned out to have nothing to do, or the
    /// mode you are in. Those take the readout's place while they are up, and
    /// [`Tone`] is where the division between them and the narration is
    /// written down.
    ///
    /// The segments are divided by rules rather than by dots. A dot is a
    /// *character*: it sits on the text baseline, takes a word-space either
    /// side, and reads as punctuation in a sentence — which is what the version
    /// this replaced was doing wrong. A rule is a division between regions,
    /// drawn at the height of the region rather than the height of an `x`.
    /// The board coming in off the disk, and how far it has got.
    ///
    /// **Drawn rather than said, and that is the whole of why it exists.** A
    /// line in the status bar is the right size for something that *happened*;
    /// this is something that is *happening*, for a second or more, over a
    /// board that still looks finished and still answers the mouse. Somebody
    /// who cannot see that the app is working assumes it has stopped — which is
    /// exactly what the read used to look like when it ran on this thread.
    ///
    /// It answers the keyboard through `stop_opening` alone — Escape is
    /// handled above, before anything else the key could mean — but every
    /// mouse event on the board behind it is swallowed here, the same way
    /// the menu and the palette keep a stray press from reaching the canvas
    /// underneath. It used to take no input at all, which meant the board
    /// could be marquee-selected, panned and right-clicked *through* a panel
    /// that was telling you it was busy.
    fn loader(&self) -> Option<gpui::AnyElement> {
        let open = self.opening.as_ref()?;
        let theme = self.theme;
        let presence = self.opening_presence;

        // A total of nought is an archive that would not say how big it is —
        // see `Opening::total`. The bar then stays at the left and the name
        // carries it, rather than a fraction being invented to fill the space.
        let fraction = match open.total {
            0 => 0.0,
            total => (open.done as f32 / total as f32).clamp(0.0, 1.0),
        };

        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .right_0()
                .bottom_0()
                .flex()
                .items_center()
                .justify_center()
                // A scrim, faded in with the panel, so the board reads as
                // *behind* something rather than merely covered by it — and a
                // full set of catchers so nothing about the board it darkens
                // answers a press, a drag or a wheel while it is up.
                .bg(theme.ground.opacity(0.45 * presence))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_mouse_up(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
                .on_mouse_move(|_, _, cx| cx.stop_propagation())
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .w(px(LOADER_WIDTH))
                        .flex()
                        .flex_col()
                        .gap(px(9.0))
                        .p(px(14.0))
                        .rounded(px(crate::theme::RADIUS_LG))
                        .opacity(presence)
                        .bg(theme.chrome)
                        .border_1()
                        .border_color(theme.chrome_edge)
                        .shadow(theme.shadow_large())
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .min_w_0()
                                .text_size(px(13.0))
                                .text_color(theme.text)
                                .child(icon(Icon::Board, crate::icons::ICON_MD, theme.muted))
                                .child(
                                    div()
                                        .truncate()
                                        .child(format!("opening {}\u{2026}", open.name)),
                                ),
                        )
                        .child(
                            div()
                                .w(px(LOADER_TRACK))
                                .h(px(4.0))
                                .rounded(px(2.0))
                                .bg(theme.chrome_edge)
                                .child(
                                    div()
                                        .h(px(4.0))
                                        .rounded(px(2.0))
                                        .w(px(LOADER_TRACK * fraction))
                                        .bg(theme.accent),
                                ),
                        )
                        .child(div().text_size(px(11.0)).text_color(theme.muted).child(
                            match open.total {
                                0 => "escape to stop".to_string(),
                                total => {
                                    format!("{} \u{2014} escape to stop", portion(open.done, total))
                                }
                            },
                        )),
                )
                .into_any_element(),
        )
    }

    /// Bring the status bar's board-wide counts up to date. Costs a check
    /// while the board is unchanged, two walks of the items when it is not.
    fn tally(&mut self) {
        let revision = self.doc.board.revision();
        if self.tallied.0 == revision {
            return;
        }
        let board = &self.doc.board;
        let cards = board.items.iter().filter(|item| item.kind.is_content()).count();
        let pictures = board.items.iter().filter(|item| picture_hash(item).is_some()).count();
        self.tallied = (revision, cards, pictures);
    }

    fn status_bar(&self) -> impl IntoElement {
        let theme = self.theme;
        let board = &self.doc.board;

        // A nice round real-world length, in pixels rather than world units —
        // `paper::scale_bar` already checked it against the target, so this is
        // the one multiply that turns its answer into something to draw.
        // `Command::ToggleHud` is the only way `hud` changes, and it is off by
        // default: the bar is drawn for someone who asked to calibrate a
        // board, not as a fixture everyone else has to look past.
        let settings = &board.settings.desktop;
        let scale_seg = settings
            .hud
            .then(|| {
                let zoom = self.viewport.zoom;
                mbrd_core::paper::scale_bar(settings.scale, zoom, &settings.units, 80.0)
                    .map(|(world, label)| (world * zoom, label))
            })
            .flatten();

        // Only what the board cannot show for itself. Everything the bar used
        // to narrate is now said by the counts beside it — see the note above
        // and [`Tone`], which is where the division is written down.
        let line = self.said.as_ref().filter(|said| said.tone.shown());

        // Counted by `tally`, which render runs before this every frame, and
        // re-counted only when the board changes: two walks of twenty thousand
        // items per frame for two numbers that move only on an edit.
        let (_, cards, pictures) = self.tallied;

        // Each of these is left out entirely when it is nought, rather than
        // reading "0 in the bin" all day. A count of nothing is the one number
        // that is worth no space at all: the board already says it.
        let mut facts = vec![(Icon::Cards, plural(cards, "card"))];
        if pictures > 0 {
            facts.push((Icon::Image, plural(pictures, "picture")));
        }
        if !board.connections.is_empty() {
            facts.push((Icon::Connect, plural(board.connections.len(), "rope")));
        }
        if !self.selection.is_empty() {
            facts.push((Icon::Selected, format!("{} selected", self.selection.len())));
        }

        div()
            .absolute()
            .bottom_0()
            .left_0()
            .right_0()
            .flex()
            .items_center()
            .h(px(STATUS_HEIGHT))
            .bg(theme.chrome)
            .border_t_1()
            .border_color(theme.chrome_edge)
            .text_size(px(11.0))
            .text_color(theme.muted)
            // The bar is chrome, and the canvas listens underneath it. Without
            // these a press on the readout pans the board, and a right press
            // opens a card's menu from a strip that is not the board at all —
            // the tool strip stops the same three for the same reason.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Middle, |_, _, cx| cx.stop_propagation())
            .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
            // Takes the room the zoom does not, so the rule before the zoom
            // sits hard against it however many facts there are — and so a long
            // failure is cut rather than pushing the zoom off the edge.
            .child(
                div().flex().flex_1().min_w_0().items_center().child(match line {
                    Some(said) => {
                        let colour = said.tone.colour(&theme);
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .min_w_0()
                            .px(px(12.0))
                            .child(icon(said.tone.icon(), 13.0, colour))
                            .child(div().truncate().text_color(colour).child(said.text.clone()))
                            .into_any_element()
                    }
                    None => div()
                        .flex()
                        .items_center()
                        .min_w_0()
                        .children(facts.into_iter().enumerate().map(|(i, (mark, text))| {
                            div()
                                .flex()
                                .items_center()
                                // The rule goes *before* every segment but
                                // the first, rather than after every
                                // segment but the last — same picture,
                                // and this way the last one cannot end in
                                // a rule against the flexible space.
                                .when(i > 0, |d| d.child(rule(theme)))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(5.0))
                                        .px(px(12.0))
                                        // A decorative mark repeating what the
                                        // count beside it already says, so it
                                        // draws in `tertiary` rather than
                                        // `muted` — see the field's own doc.
                                        .child(icon(mark, crate::icons::ICON_SM, theme.tertiary))
                                        // Medium, not the row's usual weight:
                                        // a count is a number worth a glance
                                        // at a distance, and weight is the
                                        // signal a status bar can afford that
                                        // a bigger size is not.
                                        .child(div().font_weight(FontWeight::MEDIUM).child(text)),
                                )
                        }))
                        .into_any_element(),
                }),
            )
            .when_some(scale_seg, |el, (bar_px, label)| {
                el.child(rule(theme)).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.0))
                        .px(px(12.0))
                        .flex_none()
                        .child(div().w(px(bar_px.max(1.0))).h(px(1.0)).bg(theme.tertiary))
                        .child(div().child(label)),
                )
            })
            .child(rule(theme))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(5.0))
                    .px(px(12.0))
                    .flex_none()
                    .child(icon(Icon::Zoom, crate::icons::ICON_SM, theme.tertiary))
                    .child(tabular(
                        div().child(format!("{}%", zoom_reading(self.viewport.percent()))),
                    )),
            )
    }
}

/// `1 card`, `2 cards`. English's rule, for the four nouns this bar counts.
fn plural(n: usize, noun: &str) -> String {
    match n {
        1 => format!("1 {noun}"),
        _ => format!("{n} {noun}s"),
    }
}

/// How tall the bottom bar is.
///
/// Fixed rather than fitted, so that a failure arriving does not shove the
/// canvas up by the height of a line — the board is the thing being looked at,
/// and a bar that resizes is a board that jumps.
const STATUS_HEIGHT: f32 = 26.0;

/// The line between two regions of a bar.
///
/// Zed's, and the reason to copy it is the one in [`BoardView::status_bar`]:
/// a divider is a piece of the *layout*, so it is drawn at the height of the
/// region rather than at the height of a character, and it takes its own
/// margin rather than borrowing the word-spaces on either side of a dot.
fn rule(theme: Theme) -> impl IntoElement {
    div().w(px(1.0)).h(px(12.0)).flex_none().bg(theme.chrome_edge)
}

/// `#rrggbb` in the one spelling the format stores, or `None`.
///
/// Folding `#rgb` out to the long form here, so a swatch renamed `#fa0` is
/// stored the way every other build expects to read it.
fn tidy_hex(text: &str) -> Option<String> {
    let digits = text.trim().strip_prefix('#').unwrap_or(text.trim());
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    match digits.len() {
        3 => Some(format!("#{}", digits.chars().flat_map(|c| [c, c]).collect::<String>())),
        6 => Some(format!("#{}", digits.to_lowercase())),
        _ => None,
    }
}

/// A stroked polyline, as a filled shape.
///
/// GPUI paints a `Path` by filling it, and there is no stroke — so a line has
/// to be *built* as the region a line covers. Each segment becomes two
/// triangles, pushed directly rather than through `line_to`, because `line_to`
/// fans every triangle from the path's first point and a fan only fills a
/// shape that is convex from there. A ribbon is not.
///
/// Each segment is extended by `half` at both ends. That is what fills the
/// notch at a corner: a right-angle turn leaves a square hole exactly `half`
/// on a side, and two overlapping square caps are the same shape as the hole.
///
/// `None` for a run with nothing in it, so the caller does not paint an empty
/// path — which is a draw call for no pixels.
fn ribbon(runs: &[[gpui::Point<Pixels>; 2]], half: f32) -> Option<gpui::Path<Pixels>> {
    let quads = ribbon_quads(runs, half);
    let first = quads.first()?;
    let mut path = gpui::Path::new(first[0]);
    let uv = (gpui::point(0.0, 1.0), gpui::point(0.0, 1.0), gpui::point(0.0, 1.0));
    for [a0, a1, b1, b0] in quads {
        path.push_triangle((a0, a1, b1), uv);
        path.push_triangle((a0, b1, b0), uv);
    }
    Some(path)
}

/// The corners of each segment's quad, anticlockwise from the near-left.
///
/// Split out of [`ribbon`] because a `Path` gives nothing back once it is
/// built — its vertices are private — and the arithmetic here is the part
/// worth asserting.
fn ribbon_quads(runs: &[[gpui::Point<Pixels>; 2]], half: f32) -> Vec<[gpui::Point<Pixels>; 4]> {
    let mut out = Vec::with_capacity(runs.len());
    for [a, b] in runs {
        let (dx, dy) = (f(b.x) - f(a.x), f(b.y) - f(a.y));
        let len = (dx * dx + dy * dy).sqrt();
        if len < 0.001 {
            continue;
        }
        let (ux, uy) = (dx / len, dy / len);
        // The normal is the direction turned a quarter turn; the cap is the
        // direction itself.
        let (nx, ny) = (-uy * half, ux * half);
        let (cx, cy) = (ux * half, uy * half);
        let corner = |p: &gpui::Point<Pixels>, sx: f32, sy: f32| {
            gpui::point(px(f(p.x) + sx), px(f(p.y) + sy))
        };
        out.push([
            corner(a, nx - cx, ny - cy),
            corner(a, -nx - cx, -ny - cy),
            corner(b, -nx + cx, -ny + cy),
            corner(b, nx + cx, ny + cy),
        ]);
    }
    out
}

/// A filled triangle at the end of a line.
///
/// `way` is a unit vector in *screen* directions, already flipped out of world
/// space by the caller — see `wire_list`, which is the one place the flip
/// happens.
fn arrowhead(tip: gpui::Point<Pixels>, way: gpui::Point<Pixels>, size: f32) -> gpui::Path<Pixels> {
    let (ux, uy) = (f(way.x), f(way.y));
    let len = (ux * ux + uy * uy).sqrt().max(0.0001);
    let (ux, uy) = (ux / len, uy / len);
    let (nx, ny) = (-uy, ux);
    let back = gpui::point(px(f(tip.x) - ux * size), px(f(tip.y) - uy * size));
    let wing = size * 0.55;
    let mut path = gpui::Path::new(tip);
    path.push_triangle(
        (
            tip,
            gpui::point(px(f(back.x) + nx * wing), px(f(back.y) + ny * wing)),
            gpui::point(px(f(back.x) - nx * wing), px(f(back.y) - ny * wing)),
        ),
        (gpui::point(0.0, 1.0), gpui::point(0.0, 1.0), gpui::point(0.0, 1.0)),
    );
    path
}

/// Cut a polyline into dashes, measured along its own length.
///
/// By arclength rather than per segment, so a dash carries on round a corner
/// instead of restarting at every vertex — which on a sampled curve, where a
/// "vertex" is every few pixels, would mean no dashes at all.
fn dashed(points: &[gpui::Point<Pixels>], on: f32, off: f32) -> Vec<[gpui::Point<Pixels>; 2]> {
    let period = on + off;
    if period <= 0.0 {
        return points.windows(2).map(|w| [w[0], w[1]]).collect();
    }
    let mut out = Vec::new();
    let mut walked = 0.0f32;
    for w in points.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (dx, dy) = (f(b.x) - f(a.x), f(b.y) - f(a.y));
        let len = (dx * dx + dy * dy).sqrt();
        if len < 0.001 {
            continue;
        }
        let mut at = 0.0f32;
        while at < len {
            let into = (walked + at) % period;
            if into < on {
                // Inside a dash: draw to whichever comes first, the end of the
                // dash or the end of the segment.
                let run = (on - into).min(len - at);
                let t0 = at / len;
                let t1 = (at + run) / len;
                out.push([
                    gpui::point(px(f(a.x) + dx * t0), px(f(a.y) + dy * t0)),
                    gpui::point(px(f(a.x) + dx * t1), px(f(a.y) + dy * t1)),
                ]);
                at += run.max(0.01);
            } else {
                at += (period - into).max(0.01);
            }
        }
        walked += len;
    }
    out
}

/// Put a piece of text back wherever the session was typing into.
///
/// One place, because the alternative is every caller knowing that a name is a
/// field, a note's words are a `meta` key, and a rope's label is on a
/// connection rather than on an item at all.
fn write_to(board: &mut mbrd_core::Board, on: &Subject, text: &str, adv: &dyn Advance) {
    match on {
        Subject::Card(id, field) => {
            if let Some(item) = board.item_mut(id) {
                write_field(item, *field, text, adv);
            }
        }
        Subject::Rope(a, b) => {
            if let Some(conn) = rope::between_mut(board, a, b) {
                let tidy: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                // Absent rather than empty: the format writes a label only when
                // there is one, so clearing the text has to remove the key or
                // an ordinary rope stops being a two-element array.
                conn.meta.label = if tidy.is_empty() { None } else { Some(tidy) };
            }
        }
    }
}

/// Which face of a card a point is nearest to.
///
/// For the Connect tool, where a press lands on the card rather than on one of
/// its marks and the rope still has to leave by a side. Measured against the
/// distance to each edge rather than by quadrant, so a wide flat card offers
/// its long sides for most of its area — which is where you would aim.
fn nearest_side(card: Rect, at: WorldPoint) -> Side {
    let each = [
        (Side::Left, at.x - card.x0),
        (Side::Right, card.x1 - at.x),
        (Side::Bottom, at.y - card.y0),
        (Side::Top, card.y1 - at.y),
    ];
    each.into_iter().min_by(|a, b| a.1.total_cmp(&b.1)).map(|(side, _)| side).unwrap_or(Side::Right)
}

/// Put a piece of text back on a card.
///
/// One function, because the two fields live in different places — a name is a
/// field of the item and a note's words are a `meta` key — and the two call
/// sites that write them would otherwise each have to know that.
/// The mode line for a typing session, by what it is typing into.
///
/// One function rather than a match at each door, because the *board* and the
/// open window both start sessions and a mode line that named the wrong escape
/// key would be worse than none — see `Tone::Mode` for why it stands until
/// something takes it down.
fn hint_for(field: Field) -> String {
    match field {
        Field::Name => "renaming — enter to keep, escape to put it back".into(),
        Field::Note => "editing — escape to put it back, ctrl enter to keep".into(),
        Field::Url => "addressing — enter to keep, escape to put it back".into(),
    }
}

fn write_field(item: &mut Item, field: Field, text: &str, adv: &dyn Advance) {
    match field {
        Field::Name => {
            item.name = text.to_string();
            // A swatch has no name of its own — the format says its `name` and
            // its `meta.hex` carry the same value, one uppercased. So typing a
            // colour into a swatch *is* how you recolour it, which is the whole
            // colour picker this build needs.
            if item.kind == ItemType::Swatch {
                if let Some(hex) = tidy_hex(text) {
                    item.name = hex.to_uppercase();
                    item.meta.insert("hex".into(), serde_json::Value::String(hex));
                }
            }
        }
        Field::Note => {
            item.meta.insert("text".into(), serde_json::Value::String(text.to_string()));
        }
        Field::Url => {
            let tidy = text.trim();
            // Absent rather than empty, the same bargain a rope's label makes:
            // the format writes `url` only where there is one, so clearing the
            // address has to remove the key. A link whose address is the empty
            // string reads, to every other reader, as a link that has one.
            match tidy.is_empty() {
                true => item.meta.remove("url"),
                false => {
                    item.meta.insert("url".into(), serde_json::Value::String(tidy.to_string()))
                }
            };
        }
    }
    // A note set to fit is re-measured here rather than at the two call sites,
    // because this runs on **every keystroke** as well as on the commit — see
    // `show_edit` — and that is the whole feel of the thing: the card grows
    // under the caret as the words reach the end of a line, instead of
    // arriving at its new size once typing has stopped.
    refit(item, adv);
}

/// How far a download has got, for a status line.
///
/// A percentage *and* a total, because a percentage alone does not say whether
/// the remaining 60% is six seconds or six minutes, and a size alone does not
/// say whether it is nearly done.
fn portion(done: u64, total: u64) -> String {
    format!("{}% of {}", done.saturating_mul(100) / total.max(1), megabytes(total))
}

/// A byte count as a download is usually described.
///
/// Megabytes of a million bytes rather than of 1048576, which is what every
/// download in a browser means by MB and therefore what the number will be
/// compared against.
fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

/// Where a board that has never had a file goes.
///
/// `~/mbrd`, named after the board's title, and never over the top of something
/// already there. See `dirs::boards` for why a document does not live in an
/// application data directory.
///
/// `None` only where there is no home directory to put it in, which on a
/// desktop means something is badly wrong — and the caller says so rather than
/// inventing a path relative to whatever directory the app happened to be
/// started from. That is what this used to do, and it scattered boards
/// wherever a launcher's working directory pointed.
fn fresh_board_path(board: &mbrd_core::Board) -> Option<PathBuf> {
    let dir = crate::dirs::boards()?;
    std::fs::create_dir_all(&dir).ok()?;
    Some(unused_in(&dir, &mbrd_core::naming::file_name_for(board)))
}

/// `name.mbrd`, or `name-2.mbrd`, or `name-3.mbrd` — the first one free.
///
/// Two untitled boards in a session must not be one file. The check is a race
/// in principle and not in practice: the other party would have to be a second
/// copy of this app minting a board in the same directory in the same
/// millisecond, and the loser would overwrite a board that is empty.
fn unused_in(dir: &Path, name: &Path) -> PathBuf {
    let taken = dir.join(name);
    if !taken.exists() {
        return taken;
    }
    let stem = name.file_stem().unwrap_or_default().to_string_lossy().to_string();
    // Bounded rather than a `loop`, so a directory that answers "yes, that
    // exists" to everything — a permissions oddity, a filesystem that is not
    // there — cannot spin here forever. A hundred untitled boards is already
    // well past the point where the name was doing anybody any good.
    (2..100)
        .map(|n| dir.join(format!("{stem}-{n}.mbrd")))
        .find(|candidate| !candidate.exists())
        .unwrap_or(taken)
}

/// A path as a person would say it: the file name, and nothing else.
fn short_name(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

/// One card, reduced to what the painter needs and nothing else.
///
/// Owned rather than borrowed, deliberately: see [`BoardView::draw_list`].
/// One line, reduced to what the painter needs, in canvas-local pixels.
///
/// The same bargain as [`Draw`]: the paint closure is `'static` and cannot
/// borrow the board, so what it draws has to be owned by it. Culling happens
/// before this is built, so the copying is bounded by the window.
struct WireDraw {
    /// The line as a run of points. A curve arrives here already sampled —
    /// the painter draws straight ribbons and nothing else, which is what
    /// lets a curve and an elbow share one path through it.
    points: Vec<gpui::Point<Pixels>>,
    colour: Hsla,
    /// Half the stroke width, because every offset from the centre line is a
    /// half and writing it once is one fewer place to halve it wrongly.
    half: f32,
    /// The dash and the gap, in pixels, or `None` for a solid line.
    dash: Option<(f32, f32)>,
    /// Where each arrowhead sits and which way it points.
    arrows: Vec<(gpui::Point<Pixels>, gpui::Point<Pixels>)>,
    /// The word on it, and where the middle of that word goes.
    label: Option<(SharedString, gpui::Point<Pixels>)>,
    /// Whether the caret is in that word.
    labelling: bool,
    selected: bool,
}

/// One of the faint marks beside a card that a rope can be dragged out of.
struct Mark {
    at: gpui::Point<Pixels>,
    /// How far in it is, from nothing to fully offered.
    fade: f32,
    /// Lit while the pointer is on it, so that it reads as something to press
    /// rather than as a decoration the card happens to have.
    lit: bool,
}

/// The picture on a card, reduced to what the painter needs.
///
/// A struct rather than the tuple this was, because it grew a fourth member and
/// `(image, at, arrived, frame)` is a thing you have to count on your fingers
/// at every call site.
struct Picture {
    image: Arc<RenderImage>,
    /// Where it goes, which for `cover` is deliberately larger than the card.
    at: Bounds<Pixels>,
    /// How far it has arrived. See `images::ARRIVING`.
    arrived: f32,
    /// Which frame of it. Always `0` for a still picture; for an animation,
    /// wherever the playhead has got to. See `playback::frame_of`.
    frame: usize,
}

/// The controls on a card that plays, reduced to what the painter needs.
///
/// Everything here is *owned*, like the rest of `Draw`, because the painter
/// outlives this call and cannot borrow the board.
struct Controls {
    face: Face,
    strip: transport::Strip,
    /// The volume slider, where it is showing.
    volume: Option<transport::Box2>,
    playing: bool,
    /// How far through, `0.0..=1.0`.
    progress: f32,
    muted: bool,
    looping: bool,
    /// How loud, for the slider.
    loudness: f32,
    /// How long the thing on the card is, where anybody knows.
    length: Option<Duration>,
    /// Whether there is anything behind the playhead yet. See `Drawn::moves`.
    moves: bool,
    /// `0:12 / 3:40`, or just the length before anything has played.
    time: String,
    /// Which button the pointer is over, of the three with no other
    /// feedback. See `BoardView::draw_list`'s `hover_control`.
    hover: Option<transport::Hit>,
    /// Which button is held down, of the same three. See
    /// `BoardView::pressed_control`.
    press: Option<transport::Hit>,
}

struct Draw {
    body: Bounds<Pixels>,
    radius: Pixels,
    fill: Hsla,
    edge: Hsla,
    border: Pixels,
    /// Whether to ring it. Separate from `edge` because at small sizes the
    /// selection shows as a fill instead and the ring would be larger than the
    /// card it is around.
    selected: bool,
    /// The picture, where there is one. See [`Picture`].
    picture: Option<Picture>,
    /// The controls, on a card that plays and is big enough to carry them.
    controls: Option<Controls>,
    /// The words, already broken into lines that fit and into runs that are
    /// each set one way. A note's Markdown is read here rather than in the
    /// painter, which only knows how to draw a run.
    lines: Vec<markdown::Line>,
    font_size: Pixels,
    /// The air between the card's edge and its words. Carried rather than
    /// taken from `CARD_PAD`, because on a card whose text scales it is a
    /// multiple of the zoom and the painter has no zoom to multiply by.
    pad: Pixels,
    text: Hsla,
    /// The caret, when this card is being typed into: which of `lines`, and
    /// how many bytes into it.
    ///
    /// While this is `Some`, `lines` are the editor's own — split where
    /// somebody pressed Enter and nowhere else. A wrapped line would reflow,
    /// and then a byte offset would no longer name a place on the screen.
    caret: Option<(usize, usize)>,
    /// Selected runs, as `(row, from, to)` in bytes within each line.
    highlight: Vec<(usize, usize, usize)>,
    /// The composing run, if a keyboard is still working one out: the same
    /// shape as `highlight`, drawn as an underline rather than a wash. See the
    /// `EntityInputHandler` impl.
    marked: Vec<(usize, usize, usize)>,
    /// Whether this one is below [`LOD_DUST`] — a flat quad and nothing else.
    ///
    /// Carried rather than measured again in the painter, because it is what
    /// decides whether the card can go in the batched layer, and the painter
    /// asking the question a second way is how the two answers drift apart.
    dust: bool,
    /// Whether to draw the four corner dots. The edges resize too, but they
    /// are not drawn — see [`Grip::at`](crate::grips::Grip::at).
    grips: bool,
    /// Whether this is furniture rather than a card — a fence.
    ///
    /// Drawn as a dashed outline over a wash instead of as a solid block. A
    /// fence is a region of the board rather than a thing on it, and one drawn
    /// as an enormous opaque card reads as a card somebody forgot to fill in.
    frame: bool,
    /// Whether this is a fence that has been stepped into.
    ///
    /// Drawn with a solid edge rather than a dashed one, which is the only
    /// thing on the screen that says where you are: inside a group, presses
    /// reach *through* the grouping, and a board that behaves differently with
    /// nothing to show for it is a board that feels broken. See
    /// [`BoardView::inside`].
    entered: bool,
    /// Whether this card's picture failed to decode — corrupt bytes, or a
    /// format nothing here reads. Distinct from a picture still on its way:
    /// see `Load::Failed` and the comment in `draw_list` that sets this,
    /// which is what stops a broken file from looking identical to a slow
    /// decode forever.
    broken: bool,
    /// The colour to draw the padlock in, on a card the author has locked,
    /// and `None` on one they have not.
    ///
    /// A colour rather than a `bool` because the mark has to stay legible on
    /// a swatch of whatever hex somebody typed, and the painter has neither
    /// the theme nor the item to work that out from — see `Theme::ink_on`,
    /// which does, at the one place that has both.
    lock: Option<Hsla>,
}

/// How big the padlock on a locked card is drawn, in screen pixels.
///
/// A fixed size rather than a fraction of the card, and the same size at every
/// zoom, because it is a mark rather than part of the picture: a lock that grew
/// with the card would be a lock the size of a wall on a photograph blown up,
/// and one that shrank would vanish on the cards most likely to be locked.
const LOCK_MARK: f32 = 14.0;

/// How far past the cards it joins a guide is drawn, in screen pixels.
///
/// A rule through two cards of the same width would otherwise stop exactly at
/// their edges and read as one more edge on the card rather than as a rule
/// across both. Small, because the overhang is the part of a guide that is
/// about nothing: every pixel of it is drawn past the last thing it has
/// anything to say about.
const GUIDE_OVER: f32 = 3.0;

/// One guide as a rectangle to fill, canvas-local: `(left, top, width, height)`.
///
/// **A guide is one pixel across, whatever the zoom.** It is a statement about a
/// coordinate, and a band two pixels wide is a statement about two of them.
///
/// Pulled out of the painter because it is arithmetic, and the bug it is here to
/// stop was arithmetic that had nowhere to be tested: the overhang used to be
/// applied on *both* axes, which turned every vertical rule into a sixteen-pixel
/// slab of colour laid over the board. Nothing caught it, because nothing could
/// — it lived inside a `'static` paint closure that needs a window to run.
fn guide_bar(line: Line, vp: &Viewport) -> (f32, f32, f32, f32) {
    match line {
        Line::Vertical { x, y0, y1 } => {
            let (a, b) = (vp.to_screen(point(x, y0)), vp.to_screen(point(x, y1)));
            let (top, bottom) = (a.y.min(b.y) - GUIDE_OVER, a.y.max(b.y) + GUIDE_OVER);
            (a.x, top, 1.0, (bottom - top).max(1.0))
        }
        Line::Horizontal { y, x0, x1 } => {
            let (a, b) = (vp.to_screen(point(x0, y)), vp.to_screen(point(x1, y)));
            let (left, right) = (a.x.min(b.x) - GUIDE_OVER, a.x.max(b.x) + GUIDE_OVER);
            (left, a.y, (right - left).max(1.0), 1.0)
        }
    }
}

/// Take one axis out of a set of guides, correction and drawing both.
///
/// For the pinned drag: an axis `Shift` has taken away is one nothing may
/// nudge, and a rule drawn through an edge the card was not allowed to reach is
/// a rule that lies about what happened.
fn strip(found: &mut Snap, horizontal: bool) {
    if horizontal {
        found.dx = 0.0;
        found.lines.retain(|line| !matches!(line, Line::Vertical { .. }));
    } else {
        found.dy = 0.0;
        found.lines.retain(|line| !matches!(line, Line::Horizontal { .. }));
    }
    found.spans.retain(|span| span.horizontal != horizontal);
}

/// What to leave in hand after putting a batch of cards down.
///
/// The **outermost fences** among them, where there are any, and everything
/// otherwise. A copied group is one thing, and selecting its rectangle *and*
/// each of the forty cards inside it would be a selection where the next drag
/// takes hold of every card twice and the next `Delete` bins the group twice
/// over.
///
/// Measured against the batch itself rather than the board, which is what makes
/// it right for a paste that has not landed yet.
fn pick_of(fresh: &[Item]) -> Vec<String> {
    let fences = Fences::measure(fresh);
    let outermost: Vec<String> = fresh
        .iter()
        .filter(|card| card.kind == ItemType::Fence && fences.owner_of(&card.id).is_none())
        .map(|card| card.id.clone())
        .collect();
    if outermost.is_empty() {
        return fresh.iter().map(|card| card.id.clone()).collect();
    }
    // The fences, plus anything they do not hold — a paste of a group and a
    // loose card beside it is both, and dropping the loose one would be a
    // paste that half-selected itself.
    let mut out = outermost;
    for card in fresh {
        if card.kind != ItemType::Fence && fences.chain(&card.id).is_empty() {
            out.push(card.id.clone());
        }
    }
    out
}

/// The paint and press order: lower `z` first, and document order within a tie.
///
/// One function rather than the same comparison written twice, because the two
/// places it is used are the painter and the hit test, and the bug where they
/// disagree — press a card, select the one behind it — is invisible until it
/// is somebody's board.
fn by_depth(items: &[Item], a: u32, b: u32) -> Ordering {
    items[a as usize].z.partial_cmp(&items[b as usize].z).unwrap_or(Ordering::Equal).then(a.cmp(&b))
}

/// Where one handle sits on a card that is already in window coordinates.
///
/// The screen-space twin of [`Grip::spot`], which works in world units because
/// that is what the hit test has. Both put the handle in the same place; this
/// one is the half that does not need a camera, because by this point the
/// rectangle has already been through one.
fn grip_spot(grip: Grip, body: Bounds<Pixels>) -> (Pixels, Pixels) {
    let (left, right) = (body.origin.x, body.origin.x + body.size.width);
    let (top, bottom) = (body.origin.y, body.origin.y + body.size.height);
    let (middle_x, middle_y) = ((left + right) / 2.0, (top + bottom) / 2.0);
    match grip {
        // Screen y grows downward while world y grows up, so the grip named
        // `Top` is the one at the *small* y here. Naming them after the world
        // is right — that is where the card lives — but it does mean this
        // table looks upside down.
        Grip::TopLeft => (left, top),
        Grip::Top => (middle_x, top),
        Grip::TopRight => (right, top),
        Grip::Right => (right, middle_y),
        Grip::BottomRight => (right, bottom),
        Grip::Bottom => (middle_x, bottom),
        Grip::BottomLeft => (left, bottom),
        Grip::Left => (left, middle_y),
    }
}

/// Move a rectangle from canvas-local coordinates into window ones.
fn shift(bounds: Bounds<Pixels>, origin: gpui::Point<Pixels>) -> Bounds<Pixels> {
    Bounds::new(gpui::point(bounds.origin.x + origin.x, bounds.origin.y + origin.y), bounds.size)
}

/// The asset whose bytes are a *picture of* this card, if any.
///
/// Not the same question as "what asset does this card carry". A video card
/// carries a video, and a video is not something this build can draw — what it
/// draws is the poster frame in `meta.cover`, which the format stores as an
/// asset in its own right for exactly this reason. Asking the wrong one costs a
/// failed decode of a several-hundred-megabyte file.
fn picture_hash(item: &Item) -> Option<&str> {
    match item.kind {
        ItemType::Video | ItemType::Audio => {
            item.meta.get("cover").and_then(serde_json::Value::as_str)
        }
        ItemType::Image | ItemType::Sticker | ItemType::Swatch => {
            item.asset.as_ref().and_then(ItemAsset::hash)
        }
        _ => None,
    }
}

/// How large the time reads, in pixels, at every zoom.
const TRANSPORT_TEXT: f32 = 11.0;

/// How large the pictures on the strip are drawn.
///
/// A shade under the button they sit in, so the wash a hover would put behind
/// one has room to be a wash rather than a hairline. See [`paint_mark`].
/// [`crate::icons::ICON_LG`] — the transport strip is the one place in the app
/// whose buttons are aimed at rather than read, so it takes the app's largest
/// icon rather than the sixteen a titlebar control does.
const TRANSPORT_ICON: f32 = crate::icons::ICON_LG;

/// Draw one card's controls.
///
/// The buttons are pictures and the rest is primitives, and the split is not
/// arbitrary. This used to draw its own play triangle and its own speaker,
/// because a glyph is at the mercy of whatever face the system hands back and a
/// machine without a symbol font would get a card with a box on it. That risk
/// is gone — `icons.rs` compiles the pictures in, so they are exactly as
/// reliable as the four primitives were — and what is left is that a
/// hand-rolled speaker cone is a hand-rolled speaker cone.
///
/// The scrubber, the head and the volume slider stay primitives, because they
/// are not symbols: they are a measurement of where you are in something, and
/// their shape is their value.
fn paint_controls(
    window: &mut Window,
    cx: &mut App,
    controls: &Controls,
    origin: gpui::Point<Pixels>,
    theme: &Theme,
    font: &Font,
) {
    let strip = &controls.strip;

    // A scrim under the whole strip. Without it a control drawn over the pale
    // half of a photograph is invisible, and which half that is changes every
    // frame of a video — so the backing is unconditional rather than clever.
    slab(window, strip.bar, origin, 6.0, theme.chrome.opacity(0.82));

    // The wash behind whichever of the three buttons the pointer is over or
    // holding down — this strip was the one control surface in the app with
    // no hover or press feedback at all. The scrubber and the slider already
    // answer positionally, which is why only play/pause, mute and loop ever
    // ask this for a colour. Matches the ratio `menu.rs` uses between its own
    // hover and active states; drawn under the icon, like every other hover
    // surface here.
    let wash = |hit: transport::Hit| -> Option<Hsla> {
        if controls.press == Some(hit) {
            Some(theme.text.opacity(0.20))
        } else if controls.hover == Some(hit) {
            Some(theme.text.opacity(0.10))
        } else {
            None
        }
    };

    if let Some(colour) = wash(transport::Hit::PlayPause) {
        slab(window, strip.play, origin, 6.0, colour);
    }
    // Play, or pause. Which of the two is drawn is what the button *means*
    // right now — it shows what pressing it would do, which is the convention
    // every player follows and the opposite of what it reports.
    let mark = if controls.playing { Icon::Pause } else { Icon::Play };
    paint_mark(window, cx, mark, strip.play, origin, theme.text);

    // The scrubber. A track the height of the strip is what the *pointer*
    // answers to — see `transport::at` — but what is drawn is thinner, because
    // a scrubber as tall as the bar reads as a second bar.
    let track = strip.scrub;
    let middle = (track.y0 + track.y1) / 2.0;
    match controls.face {
        // A voice memo has nothing to look at, so the sound is the picture.
        // Bars from the middle outwards, which is how a waveform is read.
        //
        // Phase B measures these off the recording and into the board's
        // `waveforms` sidecar. Until then the bars are level, which is honest:
        // it says "a recording" without claiming to say what is in it.
        Face::Memo => {
            let half = track.height() * 0.42 * 0.35;
            let bars = ((track.width() / 3.0).floor() as usize).clamp(1, 160);
            for i in 0..bars {
                let across = (i as f32 + 0.5) / bars as f32;
                let x = track.x0 + track.width() * across;
                let lit = across <= controls.progress;
                let box2 = transport::Box2::new(x - 1.0, middle - half, x + 1.0, middle + half);
                // `muted` is solid now — see the field's own doc — so this is
                // the wash's real effective weight rather than one more
                // opacity compounding whatever `muted` itself already was.
                let colour = if lit { theme.accent } else { theme.muted.opacity(0.5) };
                slab(window, box2, origin, 1.0, colour);
            }
        }
        Face::Overlay => {
            let half = 2.0;
            let rail = transport::Box2::new(track.x0, middle - half, track.x1, middle + half);
            slab(window, rail, origin, half, theme.muted.opacity(0.45));
            let played = track.along(controls.progress);
            if played > track.x0 {
                let done = transport::Box2::new(track.x0, middle - half, played, middle + half);
                slab(window, done, origin, half, theme.accent);
            }
        }
    }

    // The head, on both faces. The one part of a scrubber people aim at.
    let head = track.along(controls.progress);
    let r = 4.0;
    slab(
        window,
        transport::Box2::new(head - r, middle - r, head + r, middle + r),
        origin,
        r,
        theme.text,
    );

    // The time is the one thing here that *is* text, because it is a number
    // somebody reads. At a fixed size, like the buttons beside it: this is
    // chrome you aim at and read, not part of the card's own typography.
    if let Some(box2) = strip.time {
        // Tabular figures, so the elapsed half of the reading does not
        // shiver sideways against the length beside it once a second — the
        // whole reason this is right-aligned in the first place, which the
        // comment below still explains.
        let run = TextRun {
            len: controls.time.len(),
            font: Font { features: crate::theme::numeric(), ..font.clone() },
            color: theme.text,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let size = px(TRANSPORT_TEXT);
        let shaped =
            window.text_system().shape_line(controls.time.clone().into(), size, &[run], None);
        // Right-aligned against the buttons that follow it, so the length —
        // which is the half that does not change — stays put while the elapsed
        // time widens from `0:09` to `0:10`.
        let left = px(box2.x1 - f(shaped.width)) + origin.x;
        let top = px((box2.y0 + box2.y1) / 2.0 - TRANSPORT_TEXT * 0.62) + origin.y;
        let _ = shaped.paint(gpui::point(left, top), size * 1.3, window, cx);
    }

    if let Some(box2) = strip.mute {
        if let Some(colour) = wash(transport::Hit::Mute) {
            slab(window, box2, origin, 6.0, colour);
        }
        // The struck-through speaker when it is off, rather than the same
        // speaker in a duller colour: "quiet" and "muted" look identical at
        // twenty-two pixels, and only one of them is a thing you did.
        let mark = if controls.muted { Icon::Muted } else { Icon::Sound };
        let colour = if controls.muted { theme.muted } else { theme.text };
        paint_mark(window, cx, mark, box2, origin, colour);
    }

    if let Some(box2) = strip.looping {
        if let Some(colour) = wash(transport::Hit::Looping) {
            slab(window, box2, origin, 6.0, colour);
        }
        // Lit in the accent when it is on and drawn in the muted colour when it
        // is not. This used to be a ring rather than a loop arrow, on the
        // grounds that an arrow small enough for a twenty-two pixel button is a
        // smudge — which was true of one built from four hand-placed
        // primitives, and is not true of a vector rasterised at twice the size
        // it is drawn at. See `Window::paint_svg`, which does exactly that.
        let colour = if controls.looping { theme.accent } else { theme.muted };
        paint_mark(window, cx, Icon::Loop, box2, origin, colour);
    }

    if let Some(slider) = controls.volume {
        slab(window, slider, origin, 4.0, theme.chrome);
        let mid = (slider.y0 + slider.y1) / 2.0;
        let rail = transport::Box2::new(slider.x0 + 6.0, mid - 2.0, slider.x1 - 6.0, mid + 2.0);
        slab(window, rail, origin, 2.0, theme.muted.opacity(0.45));
        let filled = rail.along(controls.loudness);
        slab(
            window,
            transport::Box2::new(rail.x0, mid - 2.0, filled, mid + 2.0),
            origin,
            2.0,
            theme.accent,
        );
        slab(
            window,
            transport::Box2::new(filled - 4.0, mid - 5.0, filled + 4.0, mid + 5.0),
            origin,
            4.0,
            theme.text,
        );
    }
}

/// One picture on the strip, centred in the button it belongs to.
///
/// A free function beside [`slab`] and for the same reason: a closure holding
/// `&mut Window` holds it for as long as the closure exists, and nothing else
/// in the painter can draw meanwhile.
///
/// The failure is swallowed. `paint_svg` reports a missing file and an atlas
/// that would not take another tile, and neither is worth losing the rest of
/// the frame over — a strip with no play triangle on it is still a strip you
/// can scrub. The test in `icons.rs` is what catches the first of those, at the
/// point where it is a name in a table rather than a gap on somebody's card.
fn paint_mark(
    window: &mut Window,
    app: &App,
    which: Icon,
    button: transport::Box2,
    origin: gpui::Point<Pixels>,
    colour: Hsla,
) {
    let half = TRANSPORT_ICON / 2.0;
    let (mx, my) = ((button.x0 + button.x1) / 2.0, (button.y0 + button.y1) / 2.0);
    let bounds = Bounds::new(
        gpui::point(px(mx - half), px(my - half)),
        gpui::size(px(TRANSPORT_ICON), px(TRANSPORT_ICON)),
    );
    let _ = window.paint_svg(
        shift(bounds, origin),
        which.path().into(),
        gpui::TransformationMatrix::unit(),
        colour,
        app,
    );
}

/// One flat rounded rectangle of the strip, in canvas-local pixels.
///
/// A free function rather than a closure over `window`, because a closure that
/// captures a `&mut Window` holds it for as long as the closure exists and
/// nothing else in the painter can draw meanwhile.
fn slab(
    window: &mut Window,
    b: transport::Box2,
    origin: gpui::Point<Pixels>,
    radius: f32,
    colour: Hsla,
) {
    let bounds = Bounds::new(
        gpui::point(px(b.x0), px(b.y0)),
        gpui::size(px(b.width().max(0.0)), px(b.height().max(0.0))),
    );
    window.paint_quad(quad(
        shift(bounds, origin),
        px(radius),
        colour,
        px(0.0),
        gpui::transparent_black(),
        BorderStyle::Solid,
    ));
}

/// What a card that plays should show, and where its controls go.
///
/// A free function taking the two fields it touches rather than a method,
/// because the caller is holding `&self.doc.board` for the item and a `&mut
/// self` here would take that borrow away from it.
///
/// **Returns `None` for a card too small for controls, and starts it playing
/// anyway.** The two are separate questions, and conflating them is the bug
/// where a GIF stops animating as you zoom out — the strip is furniture you aim
/// at, and the animation is the card.
/// Does this card carry sound — and so, does it get a mute button?
///
/// A control that cannot do anything is worse than a missing one: it invites a
/// press and answers the same either way. So the button is left off only where
/// the answer is actually known, and `true` is what an unreadable file gets.
fn has_sound(
    memo: &mut HashMap<String, Option<bool>>,
    assets: &HashMap<String, mbrd_core::mbrd::Asset>,
    item: &Item,
) -> bool {
    // The card's own answer first. Audio and pictures answer from their type
    // alone, and anything imported by this build has it written down.
    if let Some(known) = mbrd_core::media::has_sound(item) {
        return known;
    }
    let Some(hash) = item.asset.as_ref().and_then(mbrd_core::model::ItemAsset::hash) else {
        return true;
    };
    // Once per asset per session, and the miss is cached too — a file this
    // build cannot read must not be re-read every frame. Asked with the
    // borrowed hash first: `entry` wants an owned key even on a hit, which
    // was a `String` per visible video card per frame.
    if let Some(known) = memo.get(hash) {
        return known.unwrap_or(true);
    }
    let sniffed = assets.get(hash).and_then(|asset| mbrd_core::sound::sniff(&asset.bytes));
    memo.insert(hash.to_string(), sniffed);
    sniffed.unwrap_or(true)
}

#[allow(clippy::too_many_arguments)]
fn controls_for(
    media: &mut Media,
    timings: &mut Timings,
    volume_on: Option<&str>,
    item: &Item,
    body: Bounds<Pixels>,
    image: Option<&Arc<RenderImage>>,
    hovered: bool,
    sound: bool,
    hover: Option<transport::Hit>,
    press: Option<transport::Hit>,
) -> Option<Controls> {
    if !mbrd_core::media::is_playable(&item.kind) {
        return None;
    }

    // Whether this card actually moves, which for a picture is a question only
    // its bytes can answer: a `.gif` holding one frame is a photograph, and one
    // holding forty is an animation, and nothing before the decode knows which.
    let moves = image.is_some_and(|image| image.frame_count() > 1);
    let animation = matches!(item.kind, ItemType::Image);
    if animation && !moves {
        return None;
    }

    let flags = mbrd_core::media::playback(item);
    let length = match moves {
        // An animation's length is a property of its frames, which is the one
        // length on the board nobody has to be told — read off the measured
        // clock rather than summed per frame.
        true => image.map(|image| timings.of(&item.id, image).length()),
        false => mbrd_core::media::duration(item).map(Duration::from_secs_f32),
    };

    media.observe(&item.id, length, flags.looping);

    // The autoplay gate. Everything reaching here is already visible — this
    // runs inside the cull — and `Media::play` holds the count at `AT_ONCE`, so
    // what is left to check is that there is something to play and that nobody
    // has pressed this card before. A card somebody paused must not be started
    // again by the next frame, which is what the last clause is for.
    if flags.autoplay && moves && media.get(&item.id).is_none() {
        media.play(&item.id, length, flags.looping);
    }

    let playing = media.is_playing(&item.id);

    // **An animation wears no controls while it is doing its job.** A GIF is a
    // moving picture rather than a player: a strip laid over one is furniture
    // in front of the thing you put on the board. It appears when you point at
    // the card — so it can be stopped — and stays while it is stopped, so that
    // stopping one is not a way of losing the button that starts it again.
    if animation && playing && !hovered {
        return None;
    }

    let card = transport::Box2::of(body);
    let strip = transport::Strip::fit(card, sound)?;

    let at = media.at(&item.id);
    let time = match length {
        Some(length) => format!(
            "{} / {}",
            transport::clock(at.as_secs_f32()),
            transport::clock(length.as_secs_f32())
        ),
        None => transport::clock(at.as_secs_f32()),
    };

    Some(Controls {
        face: match item.kind {
            // The whole of the difference between the two audio cards. See
            // `media::has_sleeve`.
            ItemType::Audio if !mbrd_core::media::has_sleeve(item) => Face::Memo,
            _ => Face::Overlay,
        },
        strip,
        volume: (volume_on == Some(item.id.as_str())).then(|| strip.volume(card)).flatten(),
        playing,
        progress: media.progress(&item.id),
        muted: flags.muted,
        looping: flags.looping,
        loudness: flags.volume,
        length,
        moves,
        time,
        hover,
        press,
    })
}

/// Where a picture of the given shape sits inside a card.
///
/// `cover` fills the card and lets the overflow be cropped by the caller's
/// mask; `contain` fits the whole picture inside and lets the card's own fill
/// show as a letterbox. Both keep the picture's shape — a moodboard that
/// stretched photographs to fit would be worse than useless.
fn fit_into(card: Bounds<Pixels>, aspect: f32, cover: bool) -> Bounds<Pixels> {
    let (cw, ch) = (f(card.size.width), f(card.size.height));
    if !aspect.is_finite() || aspect <= 0.0 || cw <= 0.0 || ch <= 0.0 {
        return card;
    }
    let wider_than_card = aspect > cw / ch;
    // Two cases, and `cover` swaps which one is which: to fill, match the axis
    // where the picture is *relatively short*; to fit, match the other.
    let match_width = if cover { !wider_than_card } else { wider_than_card };
    let (w, h) = if match_width { (cw, cw / aspect) } else { (ch * aspect, ch) };
    Bounds::new(
        gpui::point(card.origin.x + px((cw - w) / 2.0), card.origin.y + px((ch - h) / 2.0)),
        gpui::size(px(w), px(h)),
    )
}

/// Break a label into lines that will roughly fit a card `columns` wide.
///
/// Greedy, by word, with a hard break for a word longer than the whole line —
/// a URL, usually. The last line is elided rather than the rest being dropped
/// silently, because a card that says `photo of the...` is telling the truth
/// about there being more and a card that says `photo of the` is not.
///
/// `room` is in pixels and `adv` says how wide a character is, so this breaks
/// where the words actually run out of card rather than where a count of
/// characters said they would — see `metrics.rs`.
fn wrap(text: &str, room: f32, size: f32, rows: usize, adv: &dyn Advance) -> Vec<SharedString> {
    let space = adv.of(' ', size);
    let mut out: Vec<String> = Vec::new();
    'outer: for paragraph in text.split('\n') {
        let mut line = String::new();
        let mut used = 0.0f32;
        for word in paragraph.split_whitespace() {
            let mut word = word;
            let mut word_wide = adv.width(word, size);
            // A word too long for a line of its own has to be cut, or the
            // greedy loop below would never place it and would spin.
            while word_wide > room {
                if !line.is_empty() {
                    out.push(std::mem::take(&mut line));
                    used = 0.0;
                    if out.len() == rows {
                        break 'outer;
                    }
                }
                let cut = cut_at(word, room, size, adv);
                out.push(word[..cut].to_string());
                if out.len() == rows {
                    break 'outer;
                }
                word = &word[cut..];
                word_wide = adv.width(word, size);
            }
            let would_be = if line.is_empty() { word_wide } else { used + space + word_wide };
            if !line.is_empty() && would_be > room {
                out.push(std::mem::take(&mut line));
                used = 0.0;
                if out.len() == rows {
                    break 'outer;
                }
            }
            if !line.is_empty() {
                line.push(' ');
                used += space;
            }
            line.push_str(word);
            used += word_wide;
        }
        out.push(line);
        if out.len() == rows {
            break 'outer;
        }
    }

    // A trailing empty line is a paragraph break nobody can see; drop it, but
    // never drop the only line, or a card of pure whitespace would draw as one
    // that failed to load.
    while out.len() > 1 && out.last().is_some_and(String::is_empty) {
        out.pop();
    }

    // Did anything not fit? Say so on the last line rather than at the end of
    // the text, which is off the card and therefore not said at all.
    let dropped = text.split_whitespace().count()
        > out.iter().map(|l| l.split_whitespace().count()).sum::<usize>();
    if dropped {
        if let Some(last) = out.last_mut() {
            // Room for the ellipsis itself, which is why this is not simply
            // `room`: the character that says there is more has to fit on the
            // card too, or the thing it is telling you about is the thing it
            // pushed off the edge.
            let allowed = (room - adv.of('\u{2026}', size)).max(0.0);
            while adv.width(last, size) > allowed && !last.is_empty() {
                last.pop();
            }
            last.push('\u{2026}');
        }
    }

    out.into_iter().map(SharedString::from).collect()
}

/// The byte offset to cut `word` at so that what is left of it fits `room` —
/// at least one character, whatever the answer, so a caller cutting a word
/// that does not fit always makes progress.
fn cut_at(word: &str, room: f32, size: f32, adv: &dyn Advance) -> usize {
    let mut used = 0.0;
    let mut cut = 0;
    for (at, c) in word.char_indices() {
        let next = used + adv.of(c, size);
        if next > room && at > 0 {
            return at;
        }
        used = next;
        cut = at + c.len_utf8();
    }
    cut.max(1).min(word.len())
}

/// The zoom, spelled for the corner of the status bar.
///
/// Whole percent where a whole percent is a fine enough step, and finer below
/// that. Rounding the way the bar used to — always to the nearest percent —
/// was right for a range that stopped at ten percent and is wrong for one that
/// does not: every zoom below half a percent prints as `0%`, so the readout
/// reads as broken exactly where somebody is most likely to be checking it.
fn zoom_reading(percent: f32) -> String {
    match percent {
        p if p >= 10.0 => format!("{}", p.round()),
        p if p >= 1.0 => format!("{p:.1}"),
        p => format!("{p:.2}"),
    }
}

/// Digits that hold their width as they change.
///
/// The zoom reading above is the one live number drawn as an element rather
/// than a [`TextRun`] — the transport's own elapsed time reaches
/// [`theme::numeric`] directly on its run, since it already builds one by
/// hand — so this is the one place a div needs the same tabular figures
/// applied through [`gpui::Styled::text_style`] instead.
fn tabular<E: Styled>(mut el: E) -> E {
    el.text_style().get_or_insert_with(Default::default).font_features =
        Some(crate::theme::numeric());
    el
}

/// The meta key that records a card opting *out* of
/// [`Command::DontScaleText`], which is the default.
///
/// Present and true, or absent. Nothing else in the format needs teaching
/// about it: an item's `meta` is carried through a save and a load untouched,
/// so a board written by a build that has never heard of this keeps it.
const SCALE_TEXT: &str = "scaleText";

/// Whether this card's words grow and shrink with the board.
///
/// True unless the card says otherwise. The words on a card are part of it —
/// zooming out is stepping back from the board, and a note whose text held its
/// size while the note itself shrank would be a note wearing somebody else's
/// handwriting. Holding still is the exception, and the exception is what the
/// file records: see `toggle_text_scaling`.
/// The meta key that records a note whose height follows its words.
///
/// Present and true, or absent — same shape as [`SCALE_TEXT`], and for the
/// same reason: a card at the default carries nothing about it, so a board
/// written by a build that has never heard of this reads back unchanged.
const FIT_TEXT: &str = "fitText";

/// The most lines a fitted note is measured over.
///
/// A note holds [`mbrd_core::model::NOTE_MAX`] characters, so at one character
/// a line this is the ceiling that is actually reachable. It exists to keep
/// the measurement finite rather than to clip anything: a note that hit it
/// would be one character wide.
const FIT_ROWS: usize = mbrd_core::model::NOTE_MAX;

/// The shortest a fitted note is allowed to get, in world units.
///
/// A note with nothing written on it still has to be a thing you can see and
/// point at. Two lines' worth, which is the height at which the card reads as
/// a note rather than as a bar.
const FIT_MIN: f32 = 48.0;

/// Whether this note's height follows what is written on it.
fn fits_text(item: &Item) -> bool {
    item.meta.get(FIT_TEXT).and_then(serde_json::Value::as_bool).unwrap_or(false)
}

/// How tall this card would have to be to hold its words, in world units.
///
/// `None` for anything that is not a note set to fit, which is what makes the
/// callers a one-liner.
///
/// Measured at zoom one deliberately. For a card whose text scales — the
/// default — that *is* the answer at every zoom, because the words and the
/// card grow together and the number of lines never changes. For one pinned to
/// a fixed size the true line count does change as the camera moves, and
/// fitting to the current zoom would make a note resize itself when nothing
/// but the camera had happened. So the fit is to the card's own geometry, and
/// the two settings are independent.
///
/// The arithmetic is the painter's, and has to stay that way: [`leading`] of
/// the line's own zoom-independent size, scaled by the line's own
/// [`markdown::Line::scale`], because a heading is taller than a body line
/// and a fit that assumed a uniform grid would cut the last line off every
/// note that starts with one.
fn fitted_height(item: &Item, adv: &dyn Advance) -> Option<f32> {
    if !matches!(item.kind, ItemType::Note | ItemType::Text) || !fits_text(item) {
        return None;
    }
    let room = text_room(item.w, CARD_PAD);
    let text = label_for(item);
    let lines = markdown::lay_out(&text, room, CARD_TEXT, FIT_ROWS, adv);
    let words: f32 = lines
        .iter()
        .map(|line| {
            let size = CARD_TEXT * line.scale;
            size * leading(size)
        })
        .sum();
    Some((words + CARD_PAD * 2.0).max(FIT_MIN))
}

/// Take a fitted note's height back to what its words need, keeping its top
/// edge where it is.
///
/// The top rather than the centre, and that is the whole of why this is a
/// function instead of one line at each call site. An item's `y` is its
/// *centre*, so a note that grew by a line while somebody typed into it would
/// rise half a line up the board — and on a note being typed into, that is the
/// board sliding under the caret once per line. Every other editor grows
/// downwards. So does this.
///
/// A no-op on anything that is not a fitted note, so callers do not have to
/// ask first.
fn refit(item: &mut Item, adv: &dyn Advance) {
    let Some(height) = fitted_height(item, adv) else { return };
    // `y` points up, so the top edge is the *larger* coordinate. See
    // `viewport.rs`, which is the only place that flip happens.
    let top = item.y + item.h / 2.0;
    item.h = height;
    item.y = top - height / 2.0;
}

/// A card's own box: where its border is drawn, turned or not.
///
/// **Not** [`Rect::of_item`], and the difference is the whole point of this
/// existing. That one answers "what area does this card cover", which for a
/// turned card is its bounding box — wider and taller than the card, by up to
/// half its short side. That is the right answer for culling and for a
/// marquee, and the wrong one for a guide: a rule is a claim about an edge,
/// and a rule drawn on the bounding box of a card tilted three degrees floats
/// twenty pixels off the border it is pointing at, with nothing there.
///
/// So alignment reads the card's frame instead. A card has visible borders and
/// that is as far as a guide about it should go.
fn frame(item: &Item) -> Rect {
    Rect::centred(item.x, item.y, item.w, item.h)
}

fn scales_text(item: &Item) -> bool {
    item.meta.get(SCALE_TEXT).and_then(serde_json::Value::as_bool).unwrap_or(true)
}

/// How big this card's words are, and how much air is around them, in screen
/// pixels at this zoom, on a card `height` pixels tall.
///
/// The one place the three answers are worked out, because three callers need
/// them and any disagreement between those three puts the caret on a different
/// row from the text — see [`leading`] for the same argument about the
/// fourth number.
///
/// Padding is the first thing a short card gives up, and that is what the
/// clamp is. Eight pixels top and bottom is nothing on a card you are reading
/// and most of the height of one you are zoomed out from, and air held back at
/// full width while the words it was framing go undrawn is the wrong way
/// round: the air is there to make the words easier to read, so it goes before
/// they do.
fn card_text(item: &Item, zoom: f32, height: f32) -> (f32, f32) {
    let (font, pad) = match scales_text(item) {
        true => (CARD_TEXT * zoom, CARD_PAD * zoom),
        false => (CARD_TEXT, CARD_PAD),
    };
    // The one line this answers for is always the body size — see
    // `place_caret`'s own use of `leading(CARD_TEXT)` for the same reason.
    (font, pad.min(((height - font * leading(CARD_TEXT)) / 2.0).max(0.0)))
}

/// How much room the words have across a card `w` wide with `pad` of air each
/// side — the width the label's wrap, [`markdown::lay_out`], the fit and the
/// editor's own rows all break to.
///
/// One place rather than four, and for the same reason as [`card_text`]: a
/// click is measured back into a character against the very wrap the painter
/// drew, and any two of these disagreeing puts the caret on the wrong row.
///
/// Pixels, now, rather than a count of characters. This used to divide by
/// [`crate::metrics::Estimate`] and hand back a number of columns, which is
/// right for a
/// fixed-width face and wrong for the one the board is actually set in — see
/// `metrics.rs` for what that cost. The wrap does the measuring itself now,
/// and all this has to say is how much room there is.
fn text_room(w: f32, pad: f32) -> f32 {
    (w - pad * 2.0).max(1.0)
}

/// What a card says on it.
///
/// A `gone` item is the interesting case: the bin was emptied on it, so there
/// is nothing left but the name and how big the file used to be. Saying so is
/// better than drawing a blank card that looks broken.
fn label_for(item: &Item) -> String {
    match &item.kind {
        // Verbatim, marks and all: what a note's words *mean* is
        // [`markdown`]'s question, and it wants the marks to answer it. This
        // used to delete `# ` here, which is why a heading was a plain line
        // with its hash quietly missing.
        ItemType::Note | ItemType::Text => item.note_text().unwrap_or(&item.name).to_string(),
        ItemType::Link => item.url().unwrap_or(&item.name).to_string(),
        ItemType::Gone => format!("{} (gone)", item.name),
        _ if item.name.is_empty() => item.kind.as_str().to_string(),
        _ => item.name.clone(),
    }
}

/// The composing keyboard's way in.
///
/// A Japanese, Korean or Chinese keyboard does not hand over finished
/// characters. It hands over a *provisional* run, revises it as more keys are
/// pressed, and only then commits — and it expects to be told where that run
/// is on screen so it can put its candidate window beside it. This is that
/// protocol. The model half, including what a marked run is and how it is
/// replaced, is in `editor.rs`; everything here is translation.
///
/// ## Two counting systems
///
/// Every offset crossing this boundary is a **UTF-16** offset, because the
/// protocol was NSTextInputClient's before it was anybody else's. Every offset
/// on the other side of it is a UTF-8 byte. `editor.rs` owns both conversions,
/// for the same reason it owns every other offset.
///
/// ## Why this does not type everything twice
///
/// On X11, gpui feeds a key press to the installed input handler *only if the
/// app let the press propagate* — see `handle_input` in its `x11/window.rs`.
/// So [`BoardView::on_key_down`] stops propagation on every press the editor
/// took, and the two paths never both write. A composition does not come
/// through that door at all: it arrives on `handle_ime_preedit` and
/// `handle_ime_commit`, which reach the handler whatever the key did.
impl gpui::EntityInputHandler for BoardView {
    fn text_for_range(
        &mut self,
        range_utf16: std::ops::Range<usize>,
        adjusted: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let open = self.editing.as_ref()?;
        let (text, from, to) = open.editor.text_utf16(range_utf16.start, range_utf16.end);
        let text = text.to_string();
        // Only when it is not what was asked for, which is what the protocol
        // reads as "this is the range I could actually give you".
        if from != range_utf16.start || to != range_utf16.end {
            *adjusted = Some(from..to);
        }
        Some(text)
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<gpui::UTF16Selection> {
        let open = self.editing.as_ref()?;
        let (from, to, reversed) = open.editor.selection_utf16();
        Some(gpui::UTF16Selection { range: from..to, reversed })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        let open = self.editing.as_ref()?;
        open.editor.marked_utf16().map(|(from, to)| from..to)
    }

    fn unmark_text(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(open) = &mut self.editing {
            open.editor.unmark();
        }
        cx.notify();
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = &mut self.editing else { return };
        let range = range_utf16.map(|r| (open.editor.utf8_at(r.start), open.editor.utf8_at(r.end)));
        open.editor.replace_text(range, text);
        // The same errand `Reply::Held` runs on an ordinary keystroke: the
        // card behind the caret has to show what was just typed into it.
        self.show_edit();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<std::ops::Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(open) = &mut self.editing else { return };
        let range = range_utf16.map(|r| (open.editor.utf8_at(r.start), open.editor.utf8_at(r.end)));
        // The selection the platform asks for is relative to `new_text`, and
        // `new_text` is not in the editor yet — so it is converted against
        // itself rather than against the editor's own string.
        let selected = new_selected_range_utf16.map(|r| {
            let at = |utf16: usize| -> usize {
                let mut counted = 0;
                for (at, c) in new_text.char_indices() {
                    if counted >= utf16 {
                        return at;
                    }
                    counted += c.len_utf16();
                }
                new_text.len()
            };
            (at(r.start), at(r.end))
        });
        open.editor.replace_marked(range, new_text, selected);
        self.show_edit();
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: std::ops::Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // Where the candidate window goes. The caret's own row, at the card it
        // is on — near enough, and a great deal better than the corner of the
        // screen, which is what `None` here gets you.
        let open = self.editing.as_ref()?;
        let (id, _) = open.on.card()?;
        let item = self.doc.board.item(id)?;
        let vp = self.viewport;
        let centre = vp.to_screen(point(item.x, item.y));
        let (w, h) = ((item.w * vp.zoom).max(1.0), (item.h * vp.zoom).max(1.0));
        let (font_size, pad) = card_text(item, vp.zoom, h);
        let line_height = font_size * leading(CARD_TEXT);

        let at = open.editor.utf8_at(range_utf16.start);
        let rows = open.editor.wrapped(text_room(w, pad), font_size, &self.measure);
        let row = rows.iter().rposition(|&(start, _)| start <= at).unwrap_or(0);
        let left = element_bounds.origin.x + px(centre.x - w / 2.0 + pad);
        let top = element_bounds.origin.y + px(centre.y - h / 2.0 + pad + row as f32 * line_height);
        Some(Bounds::new(gpui::point(left, top), gpui::size(px(1.0), px(line_height))))
    }

    fn character_index_for_point(
        &mut self,
        point: gpui::Point<Pixels>,
        window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let at = self.caret_at(point, window)?;
        Some(self.editing.as_ref()?.editor.utf16_at(at))
    }
}

impl Focusable for BoardView {
    fn focus_handle(&self, _cx: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for BoardView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Everything that is moving, moved on by one frame — and if anything
        // still is, an ask for another frame.
        //
        // **This is the whole of the app's animation scheduling.** There is no
        // timer and no loop: the window redraws when something happens, and a
        // spring in flight counts as something happening until it stops. The
        // moment nothing is moving this stops asking, and a board nobody is
        // touching goes back to costing no frames at all — which is the one
        // way adding motion to this could have made it worse.
        if self.advance() {
            window.request_animation_frame();
        }
        // Not part of `advance`: a line of text coming down in four seconds is
        // a deadline rather than a motion, and it is answered with one timer
        // instead of four seconds of frames.
        self.arm_status(cx);
        // And the board itself, if it has moved since it was last written. See
        // `arm_autosave`: this is the whole of why there is no unsaved-work
        // indicator anywhere in this app.
        self.arm_autosave(cx);

        // Hand back the atlas tiles of anything the cache evicted since the
        // last frame. Here because this is the one place in the frame that has
        // a window, and skipping it does not break anything visible — it just
        // leaks, in the one place a heap profile does not look.
        self.images.sweep(window);
        // And the frames of anything moving, which turn over thirty times a
        // second rather than once a session. See `live.rs`.
        self.live.sweep(window);

        // The face labels are drawn in. Read once, here, because the paint
        // closure runs without a style stack to ask.
        let font = window.text_style().font();

        // Cull, then reduce to what the painter can own. A board may hold
        // twenty thousand items and only the ones on screen are worth a quad.
        // The display's scale factor goes in because the picture cache picks
        // which copy of a photograph to hand over by how many *texels* the GPU
        // is about to read — see `Images::look`. A logical pixel is two of
        // those on a Retina display, and a card that asked in logical pixels
        // would be drawn from a thumbnail at twice the size it was made for.
        self.tally();
        let draws = self.draw_list(window.scale_factor(), cx);
        let (wires, marks) = self.wire_list(window.mouse_position().into());
        // The modifiers as well as the position: `Alt` over a card is about to
        // duplicate rather than move, and a pointer that only said so once the
        // drag had started would be saying it a gesture too late.
        // What the pointer would mean *on the board* — and only where the
        // board is what the pointer is over.
        //
        // The canvas is one element covering the whole window and it claims a
        // cursor through a hitbox that covers the same, so a page drawn on top
        // of it does not take that claim away by being drawn on top: nothing
        // on the settings page inserts a hitbox of its own, and the crosshair
        // the Select tool asks for was leaking straight through it onto rows
        // that are not a canvas at all. A full-page overlay is not a board you
        // are pointing at, so while one is up the board asks for the plain
        // arrow and whatever the page itself wants can be seen.
        let cursor = if self.covered() {
            gpui::CursorStyle::Arrow
        } else {
            self.cursor_at(window.mouse_position(), window.modifiers())
        };
        let board = self.paint_board(draws, wires, marks, font, cursor, cx);

        // What the open page needs from the window, taken before the borrow of
        // the overlay below begins: asking the picture cache for a photograph
        // mutates it, and measuring a character wants the text system. See
        // `opened::Ready`.
        let opened_ready =
            self.opened_id().map(str::to_string).map(|id| self.ready_opened(&id, window, cx));

        let tools = crate::tools::render(self, cx);
        // Borrowed, not cloned. A palette over a big board holds a row —
        // three strings — per card, and cloning the whole thing every frame
        // it was open used to be an allocation storm for a picture that
        // reads it and puts it down. Matching on a shared reference to the
        // field costs nothing to reach for the other two either, so the
        // whole `Overlay` is read this way now rather than only the one
        // variant that used to need it.
        let overlay = match &self.overlay {
            Overlay::None => None,
            Overlay::Menu(menu) => Some(crate::menu::render(menu, self, cx).into_any_element()),
            Overlay::Switcher(switcher) => {
                Some(crate::switcher::render(switcher, self, cx).into_any_element())
            }
            Overlay::Palette(palette) => {
                Some(crate::palette::render(palette, self, cx).into_any_element())
            }
            Overlay::Settings(page) => {
                Some(crate::settings::render(page, self, cx).into_any_element())
            }
            Overlay::Opened(opened) => opened_ready
                .as_ref()
                .map(|ready| crate::opened::render(opened, ready, self, cx).into_any_element()),
        };

        let mut root = div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(self.theme.ground)
            .text_color(self.theme.text)
            // The system's own UI face where the platform has one — macOS and
            // Windows both do — and GPUI's own fallback chain behind it,
            // because on Linux ".SystemUIFont" is a documented TODO that
            // falls back to a face GPUI's own examples ship and nothing else
            // does: a build run on a stock distribution loses this font
            // entirely rather than merely rendering it a little differently.
            // The names below are what a GNOME, an Adwaita or a bare Debian
            // desktop is likeliest to already have on disk, in that order.
            .font_family(BODY_FAMILY)
            // The default is the golden ratio, which is a pleasant number for
            // typesetting a page and not the reason this app's rows are the
            // height they are. Chrome wants to sit close to the text it
            // holds; see `markdown.rs` and [`leading`] for the card's own
            // text, which answers a different question and keeps its own
            // number.
            .line_height(relative(1.2));
        root.text_style().get_or_insert_with(Default::default).font_fallbacks =
            body_font().fallbacks;
        root
            // Nothing where the compositor draws its own. See `titlebar.rs`.
            .child(crate::titlebar::render(self, window, cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .track_focus(&self.focus_handle)
                    .on_key_down(cx.listener(Self::on_key_down))
                    .on_modifiers_changed(cx.listener(Self::on_modifiers))
                    // The pointer is a promise about what a press would do, and
                    // `Alt` changes that promise without the pointer moving.
                    // Nothing else needs the event: it costs a frame, and the
                    // frame is the whole point.
                    .on_modifiers_changed(cx.listener(
                        |_this, _e: &gpui::ModifiersChangedEvent, _window, cx| cx.notify(),
                    ))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
                    .on_mouse_down(MouseButton::Middle, cx.listener(Self::on_mouse_down))
                    .on_mouse_down(MouseButton::Right, cx.listener(Self::on_mouse_down))
                    .on_mouse_move(cx.listener(Self::on_mouse_move))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .on_mouse_up(MouseButton::Middle, cx.listener(Self::on_mouse_up))
                    // A release outside the canvas ends the gesture too,
                    // or a drag off the edge leaves the pipeline stuck
                    // holding a card.
                    .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
                    .on_scroll_wheel(cx.listener(Self::on_scroll))
                    // Files from anywhere else on the machine. `ExternalPaths`
                    // is what the platform hands over; where the pointer was
                    // when it let go is not, so the drop lands in the middle of
                    // the view rather than somewhere invented.
                    .on_drop(cx.listener(|this, paths: &gpui::ExternalPaths, _window, cx| {
                        let at = this.viewport.pan;
                        this.take_files(paths.paths(), at, cx);
                    }))
                    .child(board)
                    .child(tools)
                    .child(self.status_bar())
                    // Above the strip as well as the board: a menu opened near
                    // the bottom of the window flips upward, but one opened on
                    // a short window may still reach it. There is only ever
                    // one of the three to draw — see `Overlay` — so unlike the
                    // fields this replaced, there is no order between them
                    // left to get wrong.
                    .children(overlay)
                    // Last, and above everything: while a board is being read
                    // it is the only thing on screen that is still happening.
                    .children(self.loader()),
            )
            // Last, so the grab strips sit above everything they overlap.
            .children(crate::titlebar::resize_handles(window))
    }
}

/// What a wrap is measured against in the tests below — all six modules of
/// them, which is why this is out here rather than in one of them.
///
/// [`Estimate::columns`] where the assertion is about *where the words broke*,
/// so that "twelve" in a test means twelve characters rather than however wide
/// twelve characters happen to be in whatever face the machine running the
/// test has installed. [`Estimate::average`] — [`guess`] — where the assertion
/// is about a height in pixels, because that is the same half-an-em the old
/// `columns_for` assumed and the numbers therefore mean what they always did.
/// See the same shim in `markdown.rs`.
#[cfg(test)]
fn columns() -> crate::metrics::Estimate {
    crate::metrics::Estimate::columns()
}

#[cfg(test)]
fn guess() -> crate::metrics::Estimate {
    crate::metrics::Estimate::average()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str, wide: usize, rows: usize) -> Vec<String> {
        wrap(text, wide as f32, 1.0, rows, &columns()).into_iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn a_page_covers_the_board_before_its_contents_arrive() {
        // The whole reason `Arrival` exists: there must be no frame in which
        // the board and the page are both half-drawn, because that is a
        // cross-dissolve and it reads as ghosting.
        let half = arrival(0.5);
        assert_eq!(half.ground, 1.0, "the board was still showing through halfway in");
        assert!(half.content < 1.0, "the content had nothing left to do halfway in");

        // And the two ends are the two ends: nothing at all, and everything
        // where it belongs.
        let shut = arrival(0.0);
        assert_eq!((shut.ground, shut.content), (0.0, 0.0));
        assert_eq!(shut.rise, PAGE_RISE, "the content started where it belongs");

        let open = arrival(1.0);
        assert_eq!((open.ground, open.content, open.rise), (1.0, 1.0, 0.0));
    }

    #[test]
    fn a_new_board_never_lands_on_top_of_one_that_is_there() {
        // The whole reason `unused_in` exists. Two untitled boards in a row
        // both answer to `untitled.mbrd`, and a second one that took the name
        // would silently replace the first — which, now that a new board is
        // written to disk the instant it is made, would be this app deleting
        // somebody's work on their behalf.
        let dir = std::env::temp_dir().join(format!("mbrd-unused-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary directory");

        let name = Path::new("untitled.mbrd");
        assert_eq!(unused_in(&dir, name), dir.join("untitled.mbrd"), "an empty directory");

        std::fs::write(dir.join("untitled.mbrd"), b"").expect("writing");
        assert_eq!(unused_in(&dir, name), dir.join("untitled-2.mbrd"));

        std::fs::write(dir.join("untitled-2.mbrd"), b"").expect("writing");
        assert_eq!(unused_in(&dir, name), dir.join("untitled-3.mbrd"));

        // A board with a title of its own keeps it, rather than every new
        // board in the folder being called untitled-something.
        let named = Path::new("Kitchen_ideas.mbrd");
        assert_eq!(unused_in(&dir, named), dir.join("Kitchen_ideas.mbrd"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_label_breaks_between_words_rather_than_inside_them() {
        assert_eq!(lines("the quick brown fox", 10, 4), ["the quick", "brown fox"]);
    }

    #[test]
    fn a_word_too_long_for_a_line_is_cut_rather_than_left_out() {
        // A URL, most of the time. The greedy loop would never place it.
        let out = lines("see https://example.invalid/a/very/long/path here", 12, 6);
        assert!(out.len() > 1);
        assert!(out.iter().all(|l| l.chars().count() <= 12), "{out:?}");
        assert!(out.concat().contains("example"), "{out:?}");
    }

    #[test]
    fn a_paragraph_break_in_a_note_is_kept() {
        assert_eq!(lines("# title\n\nand a body", 20, 6), ["# title", "", "and a body"]);
    }

    #[test]
    fn what_does_not_fit_says_so_rather_than_vanishing() {
        let out = lines("one two three four five six seven eight", 9, 2);
        assert_eq!(out.len(), 2);
        assert!(out[1].ends_with('\u{2026}'), "{out:?}");
    }

    #[test]
    fn a_label_that_fits_is_left_exactly_as_it_is() {
        let out = lines("shelf.jpg", 20, 3);
        assert_eq!(out, ["shelf.jpg"]);
    }

    #[test]
    fn nothing_to_say_is_one_empty_line_rather_than_none() {
        // A card whose name is blank should draw as an empty card, not as one
        // that failed at something.
        assert_eq!(lines("", 20, 3), [""]);
        assert_eq!(lines("   ", 20, 3), [""]);
    }

    #[test]
    fn wrapping_never_runs_away_on_a_card_too_narrow_for_a_letter() {
        // `columns` comes from a division and can arrive as zero-ish. Every
        // branch that consumes a word has to make progress anyway.
        let out = lines("aaaa bbbb", 1, 4);
        assert_eq!(out.len(), 4);
    }

    fn fitted(text: &str, w: f32) -> Item {
        let mut note = Item::new("n", ItemType::Note);
        note.w = w;
        note.h = 180.0;
        note.meta.insert("text".into(), text.into());
        note.meta.insert(FIT_TEXT.into(), serde_json::Value::Bool(true));
        note
    }

    #[test]
    fn a_guide_is_measured_on_a_turned_cards_own_border() {
        // The bug this was reported as: "notes have large gaps around them".
        // `Rect::of_item` is the *area a card covers*, which for a turned card
        // is its bounding box — so a rule claiming to be flush with a note
        // tilted a few degrees floated well outside the border it pointed at,
        // with nothing there.
        let mut note = Item::new("n", ItemType::Note);
        note.w = 220.0;
        note.h = 180.0;
        note.rot = 6.0;

        let covers = Rect::of_item(&note);
        let border = frame(&note);
        assert!(
            covers.width() > border.width(),
            "the premise: a turned box is wider than its card"
        );
        assert_eq!((border.width(), border.height()), (220.0, 180.0));
        // And the gap that was being drawn into was worth more than a hair.
        assert!(covers.width() - border.width() > 10.0);

        // An untouched card is the same either way, which is what keeps this
        // from being a second geometry to keep in step.
        note.rot = 0.0;
        assert_eq!(frame(&note), Rect::of_item(&note));
    }

    #[test]
    fn a_note_set_to_fit_is_as_tall_as_what_is_written_on_it() {
        let short = fitted("one line", 220.0);
        let long = fitted(&"word ".repeat(60), 220.0);
        let (a, b) =
            (fitted_height(&short, &guess()).unwrap(), fitted_height(&long, &guess()).unwrap());
        assert!(b > a, "more words is a taller note: {a} then {b}");
        // And the same words in a narrower card wrap into more lines, so the
        // fit follows the width it is given rather than only the text.
        let narrow = fitted(&"word ".repeat(60), 120.0);
        assert!(fitted_height(&narrow, &guess()).unwrap() > b);
    }

    #[test]
    fn a_fitted_note_with_nothing_on_it_is_still_something_you_can_point_at() {
        assert_eq!(fitted_height(&fitted("", 220.0), &guess()), Some(FIT_MIN));
    }

    #[test]
    fn a_note_nobody_asked_to_fit_is_left_the_size_it_was_given() {
        let mut note = Item::new("n", ItemType::Note);
        note.w = 220.0;
        note.h = 180.0;
        note.meta.insert("text".into(), "one line".into());
        assert_eq!(fitted_height(&note, &guess()), None);
        refit(&mut note, &guess());
        assert_eq!(note.h, 180.0, "the default is a card that stays where you put it");
    }

    #[test]
    fn a_fitted_note_grows_downward_rather_than_out_of_its_middle() {
        // `y` is the *centre* and points up, so the naive version leaves the
        // top edge climbing half a line up the board every time a line is
        // added — which, on the note somebody is typing into, is the board
        // sliding under the caret once per line.
        let mut note = fitted(&"line\n".repeat(12), 220.0);
        note.y = 100.0;
        let top = note.y + note.h / 2.0;
        refit(&mut note, &guess());
        assert!(note.h > 180.0, "the premise: twelve lines want more than this note has");
        assert_eq!(note.y + note.h / 2.0, top, "the top edge stays put");

        // And the same on the way back down, which is the case that makes this
        // a rule rather than a clamp: a note shrinks from the bottom too.
        let mut note = fitted("one line", 220.0);
        note.y = 100.0;
        let top = note.y + note.h / 2.0;
        refit(&mut note, &guess());
        assert!(note.h < 180.0, "the premise: one line wants less than this note has");
        assert_eq!(note.y + note.h / 2.0, top);
    }

    #[test]
    fn typing_into_a_fitted_note_resizes_it_and_typing_into_any_other_does_not() {
        // The path that actually runs sixty times a second while somebody is
        // typing. See `show_edit`, which reaches `write_field` on every press.
        let mut note = fitted("one line", 220.0);
        write_field(&mut note, Field::Note, &"word ".repeat(60), &guess());
        assert!(note.h > 180.0);

        let mut plain = Item::new("p", ItemType::Note);
        plain.w = 220.0;
        plain.h = 180.0;
        write_field(&mut plain, Field::Note, &"word ".repeat(60), &guess());
        assert_eq!(plain.h, 180.0);
    }

    #[test]
    fn a_card_dragged_free_follows_the_pointer_exactly() {
        let to = dropped_at(point(10.0, 20.0), 3.5, -4.25, None);
        assert_eq!((to.x, to.y), (13.5, 15.75));
    }

    #[test]
    fn a_card_dragged_to_the_grid_lands_on_the_nearest_cell() {
        let to = dropped_at(point(0.0, 0.0), 40.0, 20.0, Some(64.0));
        assert_eq!((to.x, to.y), (64.0, 0.0), "40 is nearer 64 than 0, and 20 is nearer 0");
    }

    #[test]
    fn a_snapped_drag_keeps_moving_after_it_has_snapped() {
        // The bug this guards: measure each frame against the last *snapped*
        // position and every small delta rounds away, so the card sticks to the
        // cell it first landed in no matter how far the pointer travels.
        let home = point(0.0, 0.0);
        let step = Some(64.0);
        let mut x = home.x;
        for frame in 1..=30 {
            x = dropped_at(home, frame as f32 * 10.0, 0.0, step).x;
        }
        assert_eq!(x, 320.0, "300 units of pointer is roughly five cells, not none");
    }

    fn card() -> Bounds<Pixels> {
        Bounds::new(gpui::point(px(100.0), px(50.0)), gpui::size(px(200.0), px(100.0)))
    }

    /// The picture's shape, to a hair.
    fn shape(b: Bounds<Pixels>) -> (f32, f32) {
        (f(b.size.width), f(b.size.height))
    }

    #[test]
    fn a_contained_picture_fits_inside_and_keeps_its_shape() {
        // Squarer than the card, so it is the height that binds.
        let out = fit_into(card(), 1.0, false);
        assert_eq!(shape(out), (100.0, 100.0));
        assert!(f(out.origin.x) > 100.0, "it should be centred, not left-aligned");

        // Wider than the card, so it is the width.
        let out = fit_into(card(), 4.0, false);
        assert_eq!(shape(out), (200.0, 50.0));
    }

    #[test]
    fn a_covering_picture_fills_the_card_and_overflows_the_other_way() {
        let out = fit_into(card(), 1.0, true);
        assert_eq!(shape(out), (200.0, 200.0));
        // The overflow is even on both sides, so what is cropped is the edges
        // rather than the bottom.
        assert_eq!(f(out.origin.y), 50.0 - 50.0);

        let out = fit_into(card(), 4.0, true);
        assert_eq!(shape(out), (400.0, 100.0));
    }

    #[test]
    fn a_picture_of_no_shape_at_all_is_drawn_as_the_card() {
        // A decode that produced a zero-width frame should not put a NaN
        // rectangle into the scene.
        for aspect in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let out = fit_into(card(), aspect, true);
            assert_eq!(shape(out), (200.0, 100.0), "aspect {aspect}");
        }
    }
}

#[cfg(test)]
mod rope_tests {
    use super::*;

    fn card(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::centred(x, y, w, h)
    }

    #[test]
    fn a_press_near_an_edge_starts_the_rope_out_of_that_edge() {
        let c = card(0.0, 0.0, 400.0, 100.0);
        assert_eq!(nearest_side(c, point(-190.0, 0.0)), Side::Left);
        assert_eq!(nearest_side(c, point(190.0, 0.0)), Side::Right);
        assert_eq!(nearest_side(c, point(0.0, 45.0)), Side::Top);
        assert_eq!(nearest_side(c, point(0.0, -45.0)), Side::Bottom);
    }

    #[test]
    fn a_wide_card_offers_its_long_sides_for_most_of_itself() {
        // Measured by the distance to each edge rather than by quadrant. On a
        // card four times wider than it is tall, a press in the middle is
        // nearest the top or the bottom — which is where you would aim.
        let c = card(0.0, 0.0, 400.0, 100.0);
        assert_eq!(nearest_side(c, point(-100.0, 20.0)), Side::Top);
        assert_eq!(nearest_side(c, point(100.0, -20.0)), Side::Bottom);
    }

    fn at(x: f32, y: f32) -> gpui::Point<Pixels> {
        gpui::point(px(x), px(y))
    }

    fn length(runs: &[[gpui::Point<Pixels>; 2]]) -> f32 {
        runs.iter()
            .map(|[a, b]| ((f(b.x) - f(a.x)).powi(2) + (f(b.y) - f(a.y)).powi(2)).sqrt())
            .sum()
    }

    #[test]
    fn a_dashed_line_is_mostly_line_and_partly_gap() {
        let line = vec![at(0.0, 0.0), at(100.0, 0.0)];
        let runs = dashed(&line, 6.0, 4.0);
        // Six on, four off, so six tenths of a hundred units is drawn — give
        // or take whichever dash the end lands in the middle of.
        assert!((length(&runs) - 60.0).abs() <= 6.0, "{:?}", length(&runs));
        assert!(runs.len() >= 9);
    }

    #[test]
    fn a_dash_carries_on_round_a_corner() {
        // Measured along the whole line rather than restarting at each vertex.
        // A sampled curve has a vertex every few pixels, so per-segment dashing
        // would produce a solid line and nothing else.
        let bent = vec![at(0.0, 0.0), at(10.0, 0.0), at(20.0, 0.0), at(30.0, 0.0)];
        let straight = vec![at(0.0, 0.0), at(30.0, 0.0)];
        let bent_len = length(&dashed(&bent, 6.0, 4.0));
        let straight_len = length(&dashed(&straight, 6.0, 4.0));
        assert!((bent_len - straight_len).abs() < 0.01, "{bent_len} vs {straight_len}");
    }

    #[test]
    fn a_line_of_no_length_asks_for_no_dashes() {
        assert!(dashed(&[at(5.0, 5.0), at(5.0, 5.0)], 6.0, 4.0).is_empty());
    }

    #[test]
    fn a_stroke_covers_the_line_it_was_built_from() {
        // The ribbon is a filled shape rather than a stroke, so the thing worth
        // asserting is that the shape is around the line: two triangles per
        // segment, and a bounding box a half wider than the line on each side.
        let runs = [[at(0.0, 50.0), at(100.0, 50.0)]];
        let quads = ribbon_quads(&runs, 3.0);
        assert_eq!(quads.len(), 1, "one segment is one quad");
        let ys: Vec<f32> = quads[0].iter().map(|p| f(p.y)).collect();
        assert!(ys.iter().all(|y| (*y - 50.0).abs() <= 3.001), "{ys:?}");
        assert!(ys.iter().any(|y| *y < 50.0) && ys.iter().any(|y| *y > 50.0));
        // Capped at both ends by a half, which is what fills the notch a right
        // angle would otherwise leave at a corner.
        let xs: Vec<f32> = quads[0].iter().map(|p| f(p.x)).collect();
        assert!(xs.iter().any(|x| (*x + 3.0).abs() < 0.01), "{xs:?}");
        assert!(xs.iter().any(|x| (*x - 103.0).abs() < 0.01), "{xs:?}");
        assert!(ribbon(&runs, 3.0).is_some());
    }

    #[test]
    fn a_stroke_of_nothing_is_not_a_draw_call() {
        assert!(ribbon(&[], 2.0).is_none());
        assert!(ribbon(&[[at(4.0, 4.0), at(4.0, 4.0)]], 2.0).is_none());
    }

    #[test]
    fn a_label_is_stored_tidied_and_absent_when_it_is_empty() {
        use mbrd_core::model::{Connection, Item, ItemType};
        let mut board = mbrd_core::Board::default();
        board.items.push(Item::new("a", ItemType::Image));
        board.items.push(Item::new("b", ItemType::Image));
        board.connections.push(Connection {
            a: "a".into(),
            b: "b".into(),
            meta: Default::default(),
        });
        let on = Subject::Rope("a".into(), "b".into());

        write_to(&mut board, &on, "  same    shelf \n ", &guess());
        assert_eq!(board.connections[0].meta.label.as_deref(), Some("same shelf"));

        // Cleared to absent rather than to an empty string: the format writes
        // the key only when there is one, so an emptied label has to take the
        // connection back to being a bare pair.
        write_to(&mut board, &on, "   ", &guess());
        assert_eq!(board.connections[0].meta.label, None);
        assert!(board.connections[0].meta.is_default());
    }
}

/// The group rule, and the two decisions that hang off it.
///
/// All of this is testable without a window because none of it needs one:
/// [`BoardView::pick`] is an associated function over a measurement and a
/// stack of ids, and [`pick_of`] and [`strip`] are free functions. That is
/// deliberate — the whole reason `selects` delegates to `pick` rather than
/// reading `self` is that the rule then has somewhere to be tested.
#[cfg(test)]
mod group_tests {
    use super::*;
    use mbrd_core::guides::Span;

    fn at(id: &str, kind: ItemType, x: f32, y: f32, w: f32, h: f32) -> Item {
        let mut item = Item::new(id, kind);
        item.x = x;
        item.y = y;
        item.w = w;
        item.h = h;
        item
    }

    fn pen(id: &str, x: f32, y: f32, w: f32, h: f32) -> Item {
        at(id, ItemType::Fence, x, y, w, h)
    }

    fn card(id: &str, x: f32, y: f32) -> Item {
        at(id, ItemType::Image, x, y, 40.0, 40.0)
    }

    fn ids(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn pressing_a_card_in_a_group_takes_hold_of_the_group() {
        let items = vec![pen("g", 0.0, 0.0, 400.0, 400.0), card("a", 50.0, 50.0)];
        let fences = Fences::measure(&items);
        assert_eq!(BoardView::pick(&fences, &[], "a"), "g");
    }

    #[test]
    fn a_loose_card_is_only_ever_itself() {
        let items = vec![pen("g", 0.0, 0.0, 100.0, 100.0), card("a", 900.0, 900.0)];
        let fences = Fences::measure(&items);
        assert_eq!(BoardView::pick(&fences, &[], "a"), "a");
    }

    #[test]
    fn the_outermost_group_is_the_one_a_press_means() {
        // Not the innermost. Pressing a card two groups deep means the whole
        // thing, which is what a group being one thing has to mean.
        let items = vec![
            pen("outer", 0.0, 0.0, 800.0, 800.0),
            pen("inner", 0.0, 0.0, 200.0, 200.0),
            card("a", 10.0, 10.0),
        ];
        let fences = Fences::measure(&items);
        assert_eq!(BoardView::pick(&fences, &[], "a"), "outer");
    }

    #[test]
    fn stepping_into_a_group_reaches_the_one_inside_it() {
        let items = vec![
            pen("outer", 0.0, 0.0, 800.0, 800.0),
            pen("inner", 0.0, 0.0, 200.0, 200.0),
            card("a", 10.0, 10.0),
        ];
        let fences = Fences::measure(&items);
        // One step in: the press now reaches the inner group.
        assert_eq!(BoardView::pick(&fences, &ids(&["outer"]), "a"), "inner");
        // Two steps in: it reaches the card itself, and there is nowhere
        // further to go.
        assert_eq!(BoardView::pick(&fences, &ids(&["outer", "inner"]), "a"), "a");
    }

    #[test]
    fn a_group_entered_somewhere_else_does_not_open_this_one() {
        // Standing inside one group must not make a *different* group
        // transparent — otherwise entering anything would gradually dissolve
        // every grouping on the board.
        let items = vec![
            pen("here", 0.0, 0.0, 400.0, 400.0),
            pen("elsewhere", 5_000.0, 0.0, 400.0, 400.0),
            card("a", 50.0, 50.0),
        ];
        let fences = Fences::measure(&items);
        assert_eq!(BoardView::pick(&fences, &ids(&["elsewhere"]), "a"), "here");
    }

    #[test]
    fn a_board_with_no_groups_costs_nothing_to_ask_about() {
        let items = vec![card("a", 0.0, 0.0), card("b", 10.0, 10.0)];
        let fences = Fences::measure(&items);
        assert!(fences.is_empty());
        assert_eq!(BoardView::pick(&fences, &[], "a"), "a");
    }

    #[test]
    fn a_pasted_group_leaves_the_group_in_hand_and_not_its_contents() {
        // The bug this is here to stop is quieter than it looks: selecting the
        // fence *and* the three cards inside it means the next drag takes hold
        // of each card twice, and the next `Delete` bins the group twice over.
        let fresh = vec![
            pen("g", 0.0, 0.0, 400.0, 400.0),
            card("a", 10.0, 10.0),
            card("b", 40.0, 40.0),
            card("c", 70.0, 70.0),
        ];
        assert_eq!(pick_of(&fresh), ids(&["g"]));
    }

    #[test]
    fn a_paste_of_a_group_and_a_loose_card_leaves_both_in_hand() {
        let fresh = vec![
            pen("g", 0.0, 0.0, 400.0, 400.0),
            card("a", 10.0, 10.0),
            card("loose", 9_000.0, 9_000.0),
        ];
        assert_eq!(pick_of(&fresh), ids(&["g", "loose"]));
    }

    #[test]
    fn a_paste_of_nested_groups_leaves_only_the_outermost() {
        let fresh = vec![
            pen("outer", 0.0, 0.0, 800.0, 800.0),
            pen("inner", 0.0, 0.0, 200.0, 200.0),
            card("a", 10.0, 10.0),
        ];
        assert_eq!(pick_of(&fresh), ids(&["outer"]));
    }

    #[test]
    fn a_paste_of_plain_cards_leaves_all_of_them() {
        let fresh = vec![card("a", 0.0, 0.0), card("b", 100.0, 0.0)];
        assert_eq!(pick_of(&fresh), ids(&["a", "b"]));
    }

    #[test]
    fn a_guide_is_one_pixel_across_however_long_it_is() {
        // The bug this is here to stop drew a *sixteen* pixel slab: the
        // overhang was applied on both axes, so a vertical rule — whose two
        // endpoints share an x — came out as wide as its own overhang twice
        // over, in near-opaque colour, laid across somebody's board.
        let vp = Viewport::default();
        let (_, _, wide, tall) = guide_bar(Line::Vertical { x: 0.0, y0: -300.0, y1: 300.0 }, &vp);
        assert_eq!(wide, 1.0, "a vertical rule is a hairline, not a band");
        assert!(tall > 1.0, "and it has the length it was given");

        let (_, _, wide, tall) = guide_bar(Line::Horizontal { y: 0.0, x0: -300.0, x1: 300.0 }, &vp);
        assert_eq!(tall, 1.0, "and so is a horizontal one");
        assert!(wide > 1.0);
    }

    #[test]
    fn a_guide_reaches_past_both_ends_of_what_it_joins() {
        // Only along its own direction. A rule that stopped at the cards would
        // read as one more edge on the card rather than as a rule across both.
        let vp = Viewport::default();
        let line = Line::Vertical { x: 0.0, y0: -100.0, y1: 100.0 };
        let (left, top, _, tall) = guide_bar(line, &vp);
        let (a, b) = (vp.to_screen(point(0.0, -100.0)), vp.to_screen(point(0.0, 100.0)));
        assert_eq!(left, a.x, "and sits exactly on the coordinate it is about");
        assert_eq!(top, a.y.min(b.y) - GUIDE_OVER);
        assert_eq!(tall, (a.y - b.y).abs() + GUIDE_OVER * 2.0);
    }

    #[test]
    fn a_guide_through_a_single_point_is_still_drawable() {
        // A rule between two cards that are on top of each other has no length
        // of its own. It must still come out as something a painter can fill
        // rather than as a zero-sized quad.
        let vp = Viewport::default();
        let (_, _, wide, tall) = guide_bar(Line::Horizontal { y: 5.0, x0: 5.0, x1: 5.0 }, &vp);
        assert!(wide >= 1.0 && tall >= 1.0);
    }

    #[test]
    fn pinning_a_drag_to_an_axis_takes_the_other_ones_guides_with_it() {
        // A rule drawn through an edge the card was not allowed to reach is a
        // rule that lies about what happened.
        let mut found = Snap {
            dx: 3.0,
            dy: -4.0,
            lines: vec![
                Line::Vertical { x: 0.0, y0: 0.0, y1: 10.0 },
                Line::Horizontal { y: 0.0, x0: 0.0, x1: 10.0 },
            ],
            spans: vec![
                Span { horizontal: true, from: 0.0, to: 10.0, across: 0.0 },
                Span { horizontal: false, from: 0.0, to: 10.0, across: 0.0 },
            ],
        };
        // Pinned to the horizontal: the vertical rules are the ones that would
        // have moved it sideways, and they stay.
        strip(&mut found, false);
        assert_eq!((found.dx, found.dy), (3.0, 0.0));
        assert_eq!(found.lines, vec![Line::Vertical { x: 0.0, y0: 0.0, y1: 10.0 }]);
        assert!(found.spans.iter().all(|s| s.horizontal));

        strip(&mut found, true);
        assert_eq!((found.dx, found.dy), (0.0, 0.0));
        assert!(found.lines.is_empty());
        assert!(found.spans.is_empty());
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;

    fn cards(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn what_was_let_go_of_comes_back_while_the_board_has_not_moved() {
        let mut stack = LetGo::default();
        stack.push(cards(&["a", "b"]), None, 7);
        assert!(stack.holding(7));
        let back = stack.take_back(7).expect("the board is where it was");
        assert_eq!(back.cards, cards(&["a", "b"]));
    }

    #[test]
    fn a_board_that_has_changed_since_is_the_end_of_the_stack() {
        // The rule the whole design turns on: a selection restored across an
        // edit would be restored onto a board that is not the one it was made
        // on, so undo goes to the ledger instead and the stack is dropped.
        let mut stack = LetGo::default();
        stack.push(cards(&["a"]), None, 7);
        assert!(!stack.holding(8));
        assert_eq!(stack.take_back(8), None);
        assert_eq!(stack.take_back(7), None, "and it does not come back later either");
    }

    #[test]
    fn letting_go_twice_takes_two_presses_to_walk_back() {
        let mut stack = LetGo::default();
        stack.push(cards(&["a"]), None, 7);
        stack.push(cards(&["b"]), None, 7);
        assert_eq!(stack.take_back(7).map(|h| h.cards), Some(cards(&["b"])));
        assert_eq!(stack.take_back(7).map(|h| h.cards), Some(cards(&["a"])));
        assert_eq!(stack.take_back(7), None);
    }

    #[test]
    fn redo_lets_go_of_it_again() {
        let mut stack = LetGo::default();
        stack.push(cards(&["a"]), None, 7);
        stack.take_back(7).unwrap();
        assert_eq!(stack.again(7).map(|h| h.cards), Some(cards(&["a"])));
        // And it is back on the other side, ready to be taken back again.
        assert!(stack.holding(7));
    }

    #[test]
    fn letting_go_of_something_new_drops_what_was_ahead() {
        // The same rule the ledger keeps: doing something new with the marker
        // back means there is no longer anything in front of it.
        let mut stack = LetGo::default();
        stack.push(cards(&["a"]), None, 7);
        stack.take_back(7).unwrap();
        stack.push(cards(&["b"]), None, 7);
        assert_eq!(stack.again(7), None);
    }

    #[test]
    fn a_rope_is_let_go_of_and_taken_back_like_anything_else() {
        // The two are never both live, so the stack carries whichever it was.
        let mut stack = LetGo::default();
        stack.push(Vec::new(), Some(("a".into(), "b".into())), 7);
        let back = stack.take_back(7).expect("it was only just let go of");
        assert!(back.cards.is_empty());
        assert_eq!(back.rope, Some(("a".into(), "b".into())));
    }

    #[test]
    fn a_board_closed_takes_its_selections_with_it() {
        let mut stack = LetGo::default();
        stack.push(cards(&["a"]), None, 7);
        stack.take_back(7).unwrap();
        stack.forget();
        assert!(!stack.holding(7));
        assert_eq!(stack.again(7), None);
    }
}

#[cfg(test)]
mod hex_tests {
    use super::*;

    #[test]
    fn a_colour_typed_into_a_swatch_becomes_its_colour() {
        let mut swatch = Item::new("s", ItemType::Swatch);
        write_field(&mut swatch, Field::Name, "#3A5F2C", &guess());
        assert_eq!(swatch.meta.get("hex").and_then(|v| v.as_str()), Some("#3a5f2c"));
        assert_eq!(swatch.name, "#3A5F2C", "the name carries the same value, uppercased");
    }

    #[test]
    fn the_short_spelling_is_stored_the_long_way() {
        let mut swatch = Item::new("s", ItemType::Swatch);
        write_field(&mut swatch, Field::Name, "#fa0", &guess());
        assert_eq!(swatch.meta.get("hex").and_then(|v| v.as_str()), Some("#ffaa00"));
    }

    #[test]
    fn something_that_is_not_a_colour_is_just_a_name() {
        let mut swatch = Item::new("s", ItemType::Swatch);
        swatch.meta.insert("hex".into(), serde_json::json!("#123456"));
        write_field(&mut swatch, Field::Name, "warm grey", &guess());
        assert_eq!(swatch.name, "warm grey");
        assert_eq!(
            swatch.meta.get("hex").and_then(|v| v.as_str()),
            Some("#123456"),
            "the colour it had should survive being called something else"
        );
    }

    #[test]
    fn only_a_swatch_gets_this_treatment() {
        // A photograph called `#ff0000.png` is a photograph.
        let mut photo = Item::new("p", ItemType::Image);
        write_field(&mut photo, Field::Name, "#ff0000", &guess());
        assert_eq!(photo.name, "#ff0000");
        assert!(photo.meta.get("hex").is_none());
    }

    #[test]
    fn a_notes_words_go_where_the_format_keeps_them() {
        let mut note = Item::new("n", ItemType::Note);
        write_field(&mut note, Field::Note, "some words", &guess());
        assert_eq!(note.note_text(), Some("some words"));
        assert_eq!(note.name, "", "the name is not the words");
    }
}

#[cfg(test)]
mod zoom_tests {
    use super::*;

    /// A card tall enough that the padding is never the binding constraint.
    const ROOMY: f32 = 400.0;

    #[test]
    fn a_card_scales_its_text_with_the_board_unless_it_is_asked_not_to() {
        let mut note = Item::new("n", ItemType::Note);
        // The default. Nothing in the card's meta says so, which is the point:
        // it is the absence of the key that means the ordinary behaviour.
        assert!(note.meta.get(SCALE_TEXT).is_none());
        assert_eq!(card_text(&note, 3.0, ROOMY), (CARD_TEXT * 3.0, CARD_PAD * 3.0));

        // And the card that was asked to hold still holds still, however far
        // the camera goes either way.
        note.meta.insert(SCALE_TEXT.into(), serde_json::Value::Bool(false));
        assert_eq!(card_text(&note, 1.0, ROOMY), (CARD_TEXT, CARD_PAD));
        assert_eq!(card_text(&note, 8.0, ROOMY), (CARD_TEXT, CARD_PAD), "it followed the camera");
        assert_eq!(card_text(&note, 0.05, ROOMY), (CARD_TEXT, CARD_PAD));
    }

    #[test]
    fn a_card_that_scales_its_text_scales_its_padding_with_it() {
        // Both or neither: text that grew inside padding that did not would
        // walk out of the card, and the reverse would look like the words
        // shrinking into a corner.
        let note = Item::new("n", ItemType::Note);
        assert_eq!(card_text(&note, 3.0, ROOMY), (CARD_TEXT * 3.0, CARD_PAD * 3.0));
        let (font, pad) = card_text(&note, 0.25, ROOMY);
        assert!((font / pad - CARD_TEXT / CARD_PAD).abs() < 1e-6, "they came apart");
    }

    #[test]
    fn a_short_card_gives_up_its_padding_before_it_gives_up_its_words() {
        // The whole of a short card's height goes to the line of text, rather
        // than sixteen pixels of it going to air that pushes the words out.
        let note = Item::new("n", ItemType::Note);
        let line = CARD_TEXT * leading(CARD_TEXT);
        let (_, roomy) = card_text(&note, 1.0, ROOMY);
        assert_eq!(roomy, CARD_PAD, "a tall card should keep all of it");
        let (_, tight) = card_text(&note, 1.0, line + 6.0);
        assert_eq!(tight, 3.0, "it should have given up exactly what did not fit");
        let (_, none) = card_text(&note, 1.0, line);
        assert_eq!(none, 0.0, "with room for the line and nothing else, all of it");
        let (_, floored) = card_text(&note, 1.0, 2.0);
        assert_eq!(floored, 0.0, "padding never goes negative");
    }

    #[test]
    fn a_label_wraps_where_the_words_run_out_of_card_not_where_a_count_does() {
        // The same claim as `editor.rs`'s own version of this test, on the
        // label path: two labels of ten characters each, one of them twice as
        // wide as the other, must not break in the same place.
        use crate::metrics::Ragged;
        let wide = wrap("WWWWW WWWWW", 6.0, 1.0, 9, &Ragged);
        let narrow = wrap("iiiii iiiii", 6.0, 1.0, 9, &Ragged);
        assert_eq!(wide.len(), 2, "ten ems of W do not fit in six");
        assert_eq!(narrow.len(), 1, "two and a bit ems of i do");
    }

    #[test]
    fn a_fitted_note_of_wide_words_is_taller_than_one_of_narrow_words() {
        // And the same thing again where it is most visible: the height a note
        // gives itself. Under a count of characters these two were the same
        // note and got the same height, so one of them was cut off.
        use crate::metrics::Ragged;
        let note = |text: &str| {
            let mut item = Item::new("n", ItemType::Note);
            item.kind = ItemType::Note;
            item.w = 60.0;
            item.meta.insert(FIT_TEXT.into(), serde_json::Value::Bool(true));
            item.meta.insert("text".into(), serde_json::Value::String(text.into()));
            item
        };
        let wide = fitted_height(&note("WWWW WWWW WWWW WWWW"), &Ragged).unwrap();
        let narrow = fitted_height(&note("iiii iiii iiii iiii"), &Ragged).unwrap();
        assert!(wide > narrow, "wide {wide} should need more rows than narrow {narrow}");
    }

    #[test]
    fn scaled_text_wraps_to_the_same_line_at_every_zoom() {
        // The re-flow this setting exists to stop. The room comes from the
        // card measured in screen pixels and a character from the font
        // measured in screen pixels, so if the two scale together the answer
        // cannot depend on the zoom — and the padding clamp is linear in the
        // zoom too, so it does not reintroduce the dependence it looks like it
        // might.
        //
        // Asserted as the *wrap itself* rather than as a column count, now
        // that the wrap is a measurement: what has to hold is that the words
        // break in the same places, whatever the words are.
        let mut note = Item::new("n", ItemType::Note);
        note.w = 300.0;
        note.h = 30.0;
        let words = "the quick brown fox jumps over the lazy dog and keeps going";
        let broke = |zoom: f32| {
            let (font, pad) = card_text(&note, zoom, note.h * zoom);
            wrap(words, text_room(note.w * zoom, pad), font, 99, &guess())
        };
        let at_one = broke(1.0);
        assert!(at_one.len() > 1, "the fixture has to actually wrap for this to say anything");
        for zoom in [0.3, 0.5, 2.0, 7.5, 40.0] {
            assert_eq!(broke(zoom), at_one, "the line re-broke at {zoom}x");
        }
    }

    #[test]
    fn the_zoom_reading_stays_a_number_at_the_bottom_of_the_range() {
        // Every one of these used to print as `0%`.
        assert_eq!(zoom_reading(100.0), "100");
        assert_eq!(zoom_reading(12.4), "12");
        assert_eq!(zoom_reading(2.36), "2.4");
        assert_eq!(zoom_reading(0.4), "0.40");
        assert_eq!(zoom_reading(mbrd_core::viewport::MIN_ZOOM * 100.0), "0.02");
    }
}
