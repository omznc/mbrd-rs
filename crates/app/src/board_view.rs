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
    canvas, div, fill, prelude::*, px, quad, BorderStyle, Bounds, ContentMask, Context,
    FocusHandle, Focusable, Font, FontStyle, FontWeight, Hsla, KeyDownEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, RenderImage, ScrollDelta,
    ScrollWheelEvent, ShapedLine, SharedString, StrikethroughStyle, TextRun, UnderlineStyle,
    Window,
};

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use mbrd_core::geometry::{self, point, Point as WorldPoint, Rect};
use mbrd_core::index::Grid;
use mbrd_core::model::{ConnMeta, Item, ItemAsset, ItemType, TrashEntry, View};
use mbrd_core::rope::{self, Side};
use mbrd_core::state::Pending;
use mbrd_core::viewport::{ViewSize, Viewport, BASE_ZOOM};
use mbrd_core::Document;

use crate::anchor;
use crate::camera::{Camera, Trail};
use crate::command::Command;
use crate::editor::{self, Editor};
use crate::grips::Grip;
use crate::images::{Images, Load};
use crate::import;
use crate::markdown;
use crate::menu::Menu;
use crate::prefs::Prefs;
use crate::switcher::{Reply, Switcher};
use crate::theme::Theme;
use crate::tools::Tool;
use crate::wires::{self, Wire, Wires};
use mbrd_core::align;
use mbrd_core::fence::Fences;
use mbrd_core::stick::{self, Pins};

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

/// How near a rope the pointer has to be, in screen pixels, to press it.
///
/// Generous, because a rope is a line rather than a shape and there is nothing
/// underneath it to hit by accident — the cards are tested first.
const ROPE_REACH: f32 = 7.0;

/// One wheel notch. Small enough that a trackpad's many small deltas do not
/// rocket through the whole zoom range in one flick.
const ZOOM_PER_LINE: f32 = 0.12;

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
/// The card has to be at least this big for a label to be readable at all.
const LOD_LABEL_W: f32 = 40.0;
const LOD_LABEL_H: f32 = 26.0;

/// How big the words on a card are drawn, in screen pixels.
///
/// **This does not depend on the zoom**, and that is the whole point of it
/// being a constant. Text that scales with the camera turns a note into a
/// picture of a note: zoom out and it is an illegible smudge, zoom in and
/// three words fill the window. Text that stays put is a *label* — the board
/// under it grows and shrinks, and what is written on it stays readable the
/// whole way, which is what a map does and what makes a map legible at every
/// scale.
///
/// The cost is deliberate and worth naming: a card zoomed a long way in has a
/// lot of empty space around a small line of text, because the card is a thing
/// on the board and the words on it are not.
const CARD_TEXT: f32 = 13.0;

/// The air between a card's edge and its words, in screen pixels.
///
/// Constant for the same reason [`CARD_TEXT`] is. Padding that scaled while
/// the text did not would make the words appear to shrink into the corner of a
/// card as the camera came in.
const CARD_PAD: f32 = 8.0;

/// The distance from one line of a card's text to the next, as a multiple of
/// the size. Shared by the painter, the wrapper and the caret, because a
/// disagreement between any two of them puts the caret on the wrong row.
const CARD_LEADING: f32 = 1.35;

/// A frame long enough that everything on the clock has already finished.
///
/// What reduced motion runs at. Not infinity: the springs are evaluated with
/// an exponential, and an infinite exponent is a `NaN` camera rather than an
/// arrived one.
const FOREVER: f32 = 10.0;

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

/// Roughly how wide one character is, as a fraction of the font size.
///
/// Used to break a label into lines without shaping it first. It is an estimate
/// and it is allowed to be: the shaped line is clipped to the card afterwards,
/// so being wrong makes a line slightly short or slightly clipped rather than
/// wrong. Measuring properly means shaping every candidate break, which is the
/// cost this whole section exists to avoid.
const AVERAGE_ADVANCE: f32 = 0.5;

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
        start: Vec<(String, WorldPoint)>,
        /// Whether the pointer has actually travelled. A press that never moves
        /// is a click, and must not push an undoable move.
        moved: bool,
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
    /// Sweeping out a selection rectangle over empty space.
    Marquee {
        from: WorldPoint,
        to: WorldPoint,
        additive: bool,
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
}

/// Which of a card's two pieces of text is being typed into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// The label on the card.
    Name,
    /// A sticky note's words.
    Note,
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
}

/// How long something that just happened stays on screen.
const SAY_FOR: Duration = Duration::from_secs(4);

/// How long something that went wrong stays on screen.
const WARN_FOR: Duration = Duration::from_secs(10);

pub struct BoardView {
    pub doc: Document,
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
    prefs: Prefs,
    /// How far in the marks beside each card have faded, by card id.
    ///
    /// A number per card rather than one for the whole board, because hover
    /// and selection both offer marks and the two overlap: pointing at one
    /// card while another is selected has to fade one in without touching the
    /// other. Entries are dropped as they reach zero, so this is empty on the
    /// ordinary board and never grows.
    anchor_fade: HashMap<String, f32>,
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
    focus_handle: FocusHandle,
    /// The file this board came from, where it came from one. `None` means a
    /// save has to invent a name — see `save::default_path`.
    path: Option<PathBuf>,
    /// Where everything is, so that culling and hit-testing do not walk the
    /// whole board. Reached only through [`BoardView::index`], which is what
    /// keeps it from being read while it is out of date.
    grid: Grid,
    /// The board revision `grid` was built from.
    grid_at: u64,
    /// Decoded pictures, keyed by content hash.
    images: Images,
    /// The right-click list, where one is open.
    menu: Option<Menu>,
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
    /// The board switcher, where it is open.
    ///
    /// While this holds a value it takes every key press, which is what makes
    /// it a mode rather than a panel. There is exactly one such mode and this
    /// is it — see `on_key_down`, which routes here before anything else.
    switcher: Option<Switcher>,
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
}

impl BoardView {
    pub fn new(doc: Document, path: Option<PathBuf>, cx: &mut Context<Self>) -> Self {
        let grid = Grid::build(&doc.board.items);
        let grid_at = doc.board.revision();
        let mut view = Self {
            grid,
            grid_at,
            images: Images::default(),
            menu: None,
            switcher: None,
            editing: None,
            tool: Tool::default(),
            rope: None,
            hovering: None,
            wires: Wires::default(),
            drawn: Vec::new(),
            clipboard: Vec::new(),
            doc,
            path,
            viewport: Viewport::default(),
            theme: Theme::default(),
            selection: Vec::new(),
            let_go: LetGo::default(),
            said: None,
            prefs: crate::prefs::load(),
            anchor_fade: HashMap::new(),
            said_timer: false,
            // Rebuilt below, once the saved view has been read: a camera made
            // against the default viewport and then not told about the board's
            // own view would spring from the origin on the first thing that
            // moved it.
            camera: Camera::new(&Viewport::default()),
            pan_trail: Trail::default(),
            gesture: Gesture::None,
            canvas_bounds: Bounds::default(),
            focus_handle: cx.focus_handle(),
        };
        view.restore_saved_view();
        view
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
        self.said = Some(Said { text, until: Some(Instant::now() + SAY_FOR) });
    }

    /// Report something that went wrong. Up for longer.
    fn warn(&mut self, text: String) {
        self.said = Some(Said { text, until: Some(Instant::now() + WARN_FOR) });
    }

    /// Say something that stays true until it is replaced — a mode, not an
    /// event. `None` puts the bar back to saying nothing.
    fn hint(&mut self, text: Option<String>) {
        self.said = text.map(|text| Said { text, until: None });
    }

    /// Stop saying anything.
    fn hush(&mut self) {
        self.said = None;
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
        if matches!(self.gesture, Gesture::None) && self.editing.is_none() {
            let visible = self.viewport.visible();
            for id in self.offering() {
                let Some(item) = self.doc.board.item(&id) else { continue };
                let card = Rect::of_item(item);
                if anchor::too_small(card, &self.viewport) || !card.intersects(&visible) {
                    continue;
                }
                wanted.insert(id);
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
        let Some(Said { until: Some(until), .. }) = &self.said else { return };
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
        self.say(match n {
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
    /// **To the bin, not out of the file.** A binned item keeps its asset and
    /// its place in every connection that names it, because restoring it has to
    /// bring those back. Emptying the bin is the only action in the whole app
    /// that destroys anything, and it is not this one.
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
        let doomed: Vec<String> = std::mem::take(&mut self.selection);
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
        // A selection let go of since the last edit is the newest thing there
        // is to take back, so it goes first. It is not in the ledger and never
        // will be — see [`Held`] — which is why this is a branch here rather
        // than a step in `history.rs`.
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
            None => self.say("nothing to undo".into()),
        }
        cx.notify();
    }

    pub fn redo(&mut self, cx: &mut Context<Self>) {
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
            None => self.say("nothing to redo".into()),
        }
        cx.notify();
    }

    /// Drop from the selection anything the board no longer has.
    fn prune_selection(&mut self) {
        let board = &self.doc.board;
        self.selection.retain(|id| board.item(id).is_some());
    }

    /// Write the board back out.
    ///
    /// The camera is captured first, so that a save records where you are
    /// looking rather than where the file was opened. That is done here rather
    /// than left to the caller for the reason the original gives: two call
    /// sites would each have had to remember, and the one that forgot would
    /// ship files a day out of date.
    pub fn save(&mut self, cx: &mut Context<Self>) {
        // `Ctrl S` reaches here from inside a note, so what is on the card has
        // to be what is written rather than what was there before typing.
        self.stop_editing(true, cx);
        self.capture_view();
        let path =
            self.path.clone().unwrap_or_else(|| mbrd_core::naming::file_name_for(&self.doc.board));
        match crate::save::write(&path, &self.doc) {
            Ok(()) => {
                // Only adopt the path once the write has actually landed. A
                // failed Save As that still moved the target would send the
                // next save somewhere nobody asked for.
                self.path = Some(path.clone());
                crate::recent::remember(&path);
                self.say(format!("saved {}", short_name(&path)));
            }
            // Reported, never swallowed, and left up for longer than a
            // success: a save that silently failed is the one failure mode
            // this app must not have, and a line that had already timed out by
            // the time somebody looked up is barely better than silence.
            Err(err) => self.warn(format!("could not save: {err:#}")),
        }
        cx.notify();
    }

    /// Drop a fresh sticky note in the middle of the view.
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
    pub fn toggle_setting(&mut self, which: Command, cx: &mut Context<Self>) {
        let label = match which {
            Command::ToggleGrid => "Grid",
            Command::ToggleAxes => "Axes",
            Command::ToggleSnap => "Snapping",
            Command::ToggleWeb => "Connections",
            _ => return,
        };
        let now = self.doc.board.edit(label, |board| {
            let settings = &mut board.settings.desktop;
            let flag = match which {
                Command::ToggleGrid => &mut settings.grid,
                Command::ToggleAxes => &mut settings.axes,
                Command::ToggleWeb => &mut settings.web,
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
            let moved =
                self.doc.board.edit(if now { "Snap to grid" } else { "Off the grid" }, |board| {
                    if now {
                        mbrd_core::snap::engage(board, mbrd_core::LayoutMode::Desktop, step)
                    } else {
                        mbrd_core::snap::release(board, mbrd_core::LayoutMode::Desktop)
                    }
                });
            if moved {
                self.say(if now { "snapped to the grid" } else { "put back" }.into());
                cx.notify();
                return;
            }
        }

        self.say(format!("{} {}", label.to_lowercase(), if now { "on" } else { "off" }));
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
        let taken: Vec<Item> =
            self.selection.iter().filter_map(|id| self.doc.board.item(id).cloned()).collect();
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
        let taken: Vec<Item> =
            self.selection.iter().filter_map(|id| self.doc.board.item(id).cloned()).collect();
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
        let mut fresh = Vec::with_capacity(cards.len());
        for (i, mut card) in cards.into_iter().enumerate() {
            card.id = self.fresh_id_from(i);
            card.x += step;
            card.y -= step;
            z += 1.0;
            card.z = z;
            fresh.push(card);
        }
        // Connections are deliberately not copied. A connection names two ids
        // and a copy is a different card, so carrying them across would either
        // wire the copy to the original's neighbours — which nobody asked for —
        // or need a whole remapping pass for something Phase 5 has not built
        // the drawing side of yet.
        let ids: Vec<String> = fresh.iter().map(|c| c.id.clone()).collect();
        let label =
            if ids.len() == 1 { label.to_string() } else { format!("{label} {}", ids.len()) };
        self.doc.board.edit(&label, |board| board.items.extend(fresh));
        self.selection = ids;
        cx.notify();
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
    /// Clear the unstick on any note that was just dropped onto a card.
    ///
    /// `meta.loose` says "the author took this note off its host". Putting it
    /// down on a card is the author saying otherwise, and leaving the flag
    /// behind would make a note that is plainly lying on a photograph refuse
    /// to travel with it, for a reason nothing on screen could explain.
    fn restick(&mut self, moved: &[(String, WorldPoint)], open: &Pending) {
        let landed: Vec<String> = {
            let items = &self.doc.board.items;
            let pins = Pins::measure_ignoring_loose(items);
            moved
                .iter()
                .map(|(id, _)| id)
                .filter(|id| {
                    self.doc.board.item(id).is_some_and(stick::is_loose)
                        && pins.host_of(id).is_some()
                })
                .cloned()
                .collect()
        };
        if landed.is_empty() {
            return;
        }
        // Through the drag's own open step rather than a new one: the note
        // landing where it landed *is* the move, and two entries in the ledger
        // for one gesture would take two presses of undo to put back.
        self.doc.board.during(open, |board| {
            for id in &landed {
                if let Some(item) = board.item_mut(id) {
                    item.meta.remove("loose");
                }
            }
        });
    }

    /// Everything a drag on these cards should actually take hold of.
    ///
    /// Two rules, and both of them are what somebody would expect rather than
    /// what the data structure suggests:
    ///
    /// - **A stuck note hands the gesture to its host.** That is what "pinned"
    ///   means. Dragging the caption off the photograph it is captioning is not
    ///   a thing anybody means to do; unsticking it is how you say you did.
    /// - **A fence brings what is inside it.** A fence that moves and leaves
    ///   its cards behind has not moved a grouping, it has torn one.
    ///
    /// Deliberately worked out once at the press rather than on every frame:
    /// the set has to stay the same for the length of the gesture, or a card
    /// sliding out of a fence mid-drag would stop moving halfway across the
    /// board.
    fn dragging(&self, ids: Vec<String>) -> Vec<String> {
        let items = &self.doc.board.items;
        let pins = Pins::measure(items);
        let fences = Fences::measure(items);

        let mut out: Vec<String> = Vec::with_capacity(ids.len());
        let push = |id: String, out: &mut Vec<String>| {
            if !out.contains(&id) {
                out.push(id);
            }
        };
        for id in ids {
            push(pins.handle(&id).to_string(), &mut out);
        }
        // A separate pass, because what a fence holds is decided by the fences
        // and what a note is stuck to is decided by the pins, and a note stuck
        // to a card inside a fence must not be added twice.
        let mut carried: Vec<String> = Vec::new();
        for id in &out {
            if self.doc.board.item(id).map(|i| &i.kind) == Some(&ItemType::Fence) {
                for held in fences.contents(id, items) {
                    carried.push(held.id.clone());
                }
            }
            for note in pins.stuck_to(id, items) {
                carried.push(note.id.clone());
            }
        }
        for id in carried {
            push(id, &mut out);
        }
        out
    }

    // -----------------------------------------------------------------------
    // Arranging, fencing, unsticking
    // -----------------------------------------------------------------------

    /// Line up, space out, or push apart what is selected.
    ///
    /// One method for all nine, because all nine are the same shape: ask
    /// `core::align` where the cards should go and write the answer through
    /// the door. Nothing here decides anything, which is what keeps the
    /// deciding testable without a window.
    pub fn arrange(&mut self, what: Command, cx: &mut Context<Self>) {
        let picked: Vec<&Item> =
            self.selection.iter().filter_map(|id| self.doc.board.item(id)).collect();
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
            self.say("already there".into());
            cx.notify();
            return;
        }
        let n = moves.len();
        self.doc.board.edit(label, |board| {
            for m in &moves {
                if let Some(item) = board.item_mut(&m.id) {
                    item.x = m.x;
                    item.y = m.y;
                }
            }
        });
        self.say(format!("moved {n}"));
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

    /// Whether anything selected is a note that could be taken off its host.
    pub fn can_unstick(&self) -> bool {
        let pins = Pins::measure(&self.doc.board.items);
        self.selection.iter().any(|id| pins.host_of(id).is_some())
    }

    /// Take a sticky note off the card it is pinned to.
    ///
    /// The one thing about stickiness that is a decision rather than a
    /// measurement, and therefore the one thing stored. It has to be: the usual
    /// reason to unstick a note is to nudge it, so the note is normally still
    /// lying on the card it was unstuck from and no geometry could tell you
    /// otherwise.
    pub fn unstick(&mut self, cx: &mut Context<Self>) {
        let pins = Pins::measure(&self.doc.board.items);
        let loose: Vec<String> =
            self.selection.iter().filter(|id| pins.host_of(id).is_some()).cloned().collect();
        if loose.is_empty() {
            return;
        }
        let n = loose.len();
        self.doc.board.edit("Unstick", |board| {
            for id in &loose {
                if let Some(item) = board.item_mut(id) {
                    item.meta.insert("loose".into(), serde_json::Value::Bool(true));
                    item.meta.remove("stuckTo");
                }
            }
        });
        self.say(match n {
            1 => "unstuck".into(),
            n => format!("unstuck {n}"),
        });
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

    /// The anchor under the pointer, and the card offering it.
    ///
    /// Only the hovered card and the selected ones, which is the same rule the
    /// painter applies: something you cannot see must not be something you can
    /// press.
    fn anchor_at(&self, at: gpui::Point<Pixels>) -> Option<(String, Side)> {
        let local = point(
            f(at.x) - f(self.canvas_bounds.origin.x),
            f(at.y) - f(self.canvas_bounds.origin.y),
        );
        for id in self.offering() {
            let Some(item) = self.doc.board.item(&id) else { continue };
            if let Some(side) = anchor::at(local, Rect::of_item(item), &self.viewport) {
                return Some((id, side));
            }
        }
        None
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

    /// Which cards are currently wearing anchors.
    ///
    /// The one hovered, plus whatever is selected. Hovered first, because a
    /// press lands on the card the pointer is over when the two disagree.
    fn offering(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::with_capacity(self.selection.len() + 1);
        if let Some(id) = &self.hovering {
            out.push(id.clone());
        }
        // The only id that can already be in here is the hovered one, so that
        // is the only one worth checking against. Scanning the whole of `out`
        // instead — which is what this used to do — is a comparison per pair
        // on a list that Ctrl A makes as long as the board.
        for id in &self.selection {
            if Some(id) != self.hovering.as_ref() {
                out.push(id.clone());
            }
        }
        out
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

    pub fn open_board(&mut self, path: &Path, cx: &mut Context<Self>) {
        // Every id in the route cache is about to mean something else, or
        // nothing. A cache that survives a board switch is a cache that draws
        // the old board's lines between the new board's cards.
        self.wires.forget();
        self.drawn.clear();
        self.rope = None;
        // Keep whatever was being typed. The board it belongs to is about to
        // be replaced, so there is no later at which to keep it.
        self.stop_editing(true, cx);
        match crate::save::read(path) {
            Ok(doc) => {
                self.doc = doc;
                self.path = Some(path.to_path_buf());
                self.selection.clear();
                // Ids from the board that was open name nothing on this one.
                self.let_go.forget();
                self.gesture = Gesture::None;
                self.restore_saved_view();
                crate::recent::remember(path);
                self.say(format!("opened {}", short_name(path)));
            }
            // Said, not swallowed, and the board that is open stays open. The
            // failure mode this avoids is losing an hour of work to a typo in
            // somebody else's file name.
            Err(err) => self.warn(format!("could not open: {err:#}")),
        }
        cx.notify();
    }

    pub fn open_switcher(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.switcher = Some(Switcher::open(self.path.as_deref()));
        cx.notify();
    }

    pub fn close_switcher(&mut self) {
        self.switcher = None;
    }

    pub fn close_menu(&mut self) {
        self.menu = None;
    }

    /// The pointer has arrived on a row of the open menu.
    ///
    /// Which settles whether a submenu is open and which one: arriving on a row
    /// that opens onto more opens it, and arriving anywhere else closes what
    /// was open. See [`Menu::reveal`] for why it is arrival rather than
    /// departure that decides.
    pub fn reveal_menu(&mut self, row: usize, cx: &mut Context<Self>) {
        let Some(menu) = &self.menu else { return };
        let entry = menu.entries.get(row).copied();
        let opens = entry.is_some_and(|entry| entry.available(self));
        let room = self.canvas_bounds.size;
        if let Some(menu) = &mut self.menu {
            if menu.reveal(row, room, opens) {
                cx.notify();
            }
        }
    }

    /// How much room there is to draw chrome in, in canvas coordinates.
    ///
    /// Measured at prepaint rather than assumed, which is what lets the menu
    /// fit itself to a window somebody has dragged down to nothing.
    pub fn room(&self) -> gpui::Size<Pixels> {
        self.canvas_bounds.size
    }

    // -----------------------------------------------------------------------
    // Getting things onto the board
    // -----------------------------------------------------------------------

    /// Take files somebody dropped on the window.
    ///
    /// A folder brings what is *directly* in it and nothing deeper. Walking a
    /// tree is not something a drop should start: somebody who drops their home
    /// directory by accident should get a handful of cards and a shrug, not a
    /// board with a hundred thousand items and a frozen window.
    pub fn take_files(&mut self, paths: &[PathBuf], at: WorldPoint, cx: &mut Context<Self>) {
        let mut files: Vec<PathBuf> = Vec::new();
        for path in paths {
            if path.is_dir() {
                files.extend(
                    std::fs::read_dir(path)
                        .into_iter()
                        .flatten()
                        .flatten()
                        .map(|e| e.path())
                        .filter(|p| p.is_file()),
                );
            } else {
                files.push(path.clone());
            }
        }
        files.sort();

        let mut ready = Vec::new();
        let mut refused: Vec<String> = Vec::new();
        for path in &files {
            let name =
                path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let Ok(bytes) = std::fs::read(path) else {
                refused.push(format!("{name} could not be read"));
                continue;
            };
            let file = import::ready(&name, bytes);
            // The ceiling reports; this is the layer that decides, and what it
            // decides is to say so by name rather than to drop it quietly. See
            // the note at the top of `import.rs`.
            if file.is_heavy() {
                refused.push(format!("{name} is {} MB", file.megabytes()));
                continue;
            }
            ready.push(file);
        }

        // What one file was, by name, because a single drop is usually
        // somebody checking whether this app knows what their file is.
        let alone = (ready.len() == 1).then(|| ready[0].described);
        let taken = self.place(ready, at);
        self.say(match (taken, alone, refused.len()) {
            (0, _, 0) => "nothing to add".into(),
            (1, Some(what), 0) => format!("added {what}"),
            (n, _, 0) => format!("added {n}"),
            (0, _, _) => format!("too large: {}", refused.join(", ")),
            (n, _, _) => format!("added {n}; too large: {}", refused.join(", ")),
        });
        cx.notify();
    }

    /// Take whatever is on the clipboard.
    ///
    /// An image becomes a picture, an address becomes a link, and anything else
    /// becomes a note — which is the order somebody would guess, and the reason
    /// [`import::as_url`] is deliberately strict about what an address is.
    pub fn paste(&mut self, cx: &mut Context<Self>) {
        // The app's own cards first. See `paste_cards` for why one key does
        // both and in this order.
        if self.paste_cards(cx) {
            return;
        }
        let Some(item) = cx.read_from_clipboard() else {
            self.say("nothing on the clipboard".into());
            cx.notify();
            return;
        };
        let at = self.viewport.pan;

        let pictures: Vec<import::Ready> = item
            .entries()
            .iter()
            .filter_map(|entry| match entry {
                // No name and no extension, deliberately: a pasted picture has
                // neither, and `import::classify` reads the bytes anyway.
                gpui::ClipboardEntry::Image(image) => {
                    Some(import::ready("pasted", image.bytes.clone()))
                }
                gpui::ClipboardEntry::String(_) => None,
            })
            .collect();
        if !pictures.is_empty() {
            let n = self.place(pictures, at);
            self.say(format!("pasted {n}"));
            cx.notify();
            return;
        }

        let Some(text) = item.text().filter(|t| !t.trim().is_empty()) else {
            self.say("nothing on the clipboard".into());
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

        let mut card = match import::as_url(&text) {
            Some(url) => {
                let mut card = Item::new(id.clone(), ItemType::Link);
                card.name = url.to_string();
                card.w = 300.0;
                card.h = 96.0;
                card.meta.insert("url".into(), serde_json::Value::String(url.to_string()));
                card
            }
            None => {
                let mut card = Item::new(id.clone(), ItemType::Note);
                let words: String = text.trim().chars().take(mbrd_core::model::NOTE_MAX).collect();
                card.name = "note".into();
                card.w = 260.0;
                card.h = 200.0;
                card.meta.insert("text".into(), serde_json::Value::String(words));
                card
            }
        };
        card.x = at.x;
        card.y = at.y;
        card.z = z;
        self.doc.board.edit("Paste", |board| board.items.push(card));
        self.select_only(&id);
        self.say("pasted".into());
        cx.notify();
    }

    /// Put a batch of prepared files on the board, laid out around a point.
    ///
    /// One step for the whole drop rather than one per file: dropping a folder
    /// is one thing somebody did, and undoing it should be one press rather
    /// than forty. The bytes go straight into the archive — assets are not
    /// behind the mutation door, because they are content-addressed and adding
    /// one can only ever be additive. Only the cards go through the door.
    fn place(&mut self, files: Vec<import::Ready>, at: WorldPoint) -> usize {
        if files.is_empty() {
            return 0;
        }
        let count = files.len();
        let mut fresh = self.fresh_id_from(0);
        let mut z = self.top_z();
        let mut cards = Vec::with_capacity(count);

        // A square-ish block, so a folder of twenty photographs arrives as a
        // block you can see rather than a stack you have to unpick.
        let across = (count as f32).sqrt().ceil().max(1.0);
        for (i, file) in files.into_iter().enumerate() {
            let column = i as f32 % across;
            let row = (i as f32 / across).floor();
            let spread = import::ARRIVAL_SIZE * 1.1;
            let spot = point(
                at.x + (column - (across - 1.0) / 2.0) * spread,
                at.y - (row - (across - 1.0) / 2.0) * spread,
            );
            z += 1.0;
            let card = import::card(&file, fresh.clone(), spot, z);
            // Content-addressed, so a photograph already on the board is not
            // stored twice — the second card simply names the same hash.
            self.doc.assets.entry(file.hash.clone()).or_insert(file.asset);
            cards.push(card);
            fresh = self.fresh_id_from(i + 1);
        }

        let ids: Vec<String> = cards.iter().map(|c| c.id.clone()).collect();
        let label = if count == 1 { "Add".to_string() } else { format!("Add {count}") };
        self.doc.board.edit(&label, |board| board.items.extend(cards));
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
            Field::Name => Editor::selecting_all(before.clone(), limit, multiline),
            Field::Note if whole => Editor::selecting_all(before.clone(), limit, multiline),
            Field::Note => Editor::new(before.clone(), limit, multiline),
        };
        self.rope = None;
        self.editing = Some(Editing {
            on: Subject::Card(id.to_string(), field),
            editor,
            before,
            open: self.doc.board.start(),
        });
        self.say(match field {
            Field::Name => "renaming — enter to keep, escape to put it back".into(),
            Field::Note => "editing — escape to put it back, ctrl enter to keep".into(),
        });
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
            open: self.doc.board.start(),
        });
        self.say("labeling — enter to keep, escape to put it back".into());
        cx.notify();
    }

    /// End an edit, keeping what was typed or putting back what was there.
    pub fn stop_editing(&mut self, keep: bool, cx: &mut Context<Self>) {
        let Some(open) = self.editing.take() else { return };
        let text = if keep { open.editor.text().to_string() } else { open.before.clone() };
        let on = open.on.clone();
        self.doc.board.during(&open.open, |board| write_to(board, &on, &text));
        let label = match &open.on {
            Subject::Card(_, Field::Name) => "Rename",
            Subject::Card(_, Field::Note) => "Edit note",
            Subject::Rope(..) => "Label",
        };
        // Records nothing when the text came back the way it went in, which is
        // exactly what a revert should be: not a step that undoes another, but
        // no step at all.
        if self.doc.board.finish(label, open.open) {
            self.say(if keep { "changed".into() } else { "put back".into() });
        } else {
            self.hush();
        }
        cx.notify();
    }

    /// Put the text as it stands onto the card, without ending the edit.
    ///
    /// Through the open gesture, so nothing is recorded — the whole session is
    /// one step, closed by [`Self::stop_editing`]. This runs on every keystroke
    /// and is what makes the card show what is being typed into it.
    fn show_edit(&mut self) {
        let Some(open) = &self.editing else { return };
        let on = open.on.clone();
        let text = open.editor.text().to_string();
        let token = open.open.clone();
        self.doc.board.during(&token, |board| write_to(board, &on, &text));
    }

    /// Move the caret to where somebody clicked.
    ///
    /// The one part of editing that genuinely needs a font: which character a
    /// point is nearest depends on how the text was shaped, so the answer comes
    /// from the same text system that drew it. Everything else about the caret
    /// is in `editor.rs`, without a window.
    fn place_caret(&mut self, at: gpui::Point<Pixels>, extend: bool, window: &mut Window) {
        let Some(open) = &self.editing else { return };
        // A rope's label is not on a card, so there is no card-local geometry
        // to turn a click into a character. It is one short line and the arrow
        // keys reach all of it.
        let Some((id, _)) = open.on.card() else { return };
        let Some(item) = self.doc.board.item(id) else { return };

        let vp = self.viewport;
        let centre = vp.to_screen(point(item.x, item.y));
        let (w, h) = ((item.w * vp.zoom).max(1.0), (item.h * vp.zoom).max(1.0));
        let (font_size, pad) = (CARD_TEXT, CARD_PAD);
        let line_height = font_size * CARD_LEADING;

        // Canvas-local, then card-local, then past the padding.
        let local_x = f(at.x) - f(self.canvas_bounds.origin.x) - (centre.x - w / 2.0) - pad;
        let local_y = f(at.y) - f(self.canvas_bounds.origin.y) - (centre.y - h / 2.0) - pad;

        let lines = open.editor.lines();
        let row = ((local_y / line_height).floor().max(0.0) as usize).min(lines.len() - 1);
        let line = lines[row].to_string();

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

        // Back to an offset in the whole text. `lines` came from splitting on
        // newlines, so a row is the sum of the ones above it plus a separator
        // each — which is the only place this arithmetic lives.
        let start: usize = lines[..row].iter().map(|l| l.len() + 1).sum();
        if let Some(open) = &mut self.editing {
            open.editor.place(start + column, extend);
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
        match self.images.look(&hash) {
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

    fn grip_at(&self, at: gpui::Point<Pixels>) -> Option<(String, Grip, Rect)> {
        let local = point(
            f(at.x) - f(self.canvas_bounds.origin.x),
            f(at.y) - f(self.canvas_bounds.origin.y),
        );
        // Reverse, so the topmost of two overlapping selections wins — the same
        // order the painter draws them in.
        for id in self.selection.iter().rev() {
            let Some(item) = self.doc.board.item(id) else { continue };
            // The untilted box: a turned card's handles are not drawn yet, and
            // offering them where they are not would be worse than not offering
            // them at all.
            if item.rot != 0.0 {
                continue;
            }
            let box_ = Rect::centred(item.x, item.y, item.w, item.h);
            if let Some(grip) = Grip::at(local, box_, &self.viewport) {
                return Some((id.clone(), grip, box_));
            }
        }
        None
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
    fn cursor_at(&mut self, at: gpui::Point<Pixels>) -> gpui::CursorStyle {
        use gpui::CursorStyle;

        // A gesture in flight outranks everything under the pointer, because
        // during one the pointer is not *over* anything — it is holding
        // something, and what it is holding does not change until it is let go.
        match &self.gesture {
            Gesture::Panning { .. } => return CursorStyle::ClosedHand,
            Gesture::Sizing { grip, .. } => return grip.cursor(),
            Gesture::Roping { .. } => return CursorStyle::Crosshair,
            Gesture::Moving { .. } => return CursorStyle::ClosedHand,
            Gesture::Marquee { .. } => return CursorStyle::Crosshair,
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

        if self.hit(world).is_some() {
            // A card is dragged rather than clicked through, and the arrow is
            // what every canvas uses for "this is a thing you can take hold
            // of". A hand here would be a promise to pan.
            return CursorStyle::Arrow;
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
        if self.menu.take().is_some() {
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
            let room = Bounds::new(gpui::point(px(0.0), px(0.0)), self.canvas_bounds.size);
            // Which list, decided before it is placed: a rope's menu is a
            // different height from a card's, and the flip near an edge is
            // measured against whichever one is about to be drawn.
            let entries = crate::command::menu_for(self);
            self.menu = Some(Menu::new(Menu::placed(local, room, entries), entries));
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
                self.place_caret(event.position, event.modifiers.shift, window);
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
            self.gesture =
                Gesture::Sizing { id, grip, start, shape, moved: false, cropping: false, open };
            cx.notify();
            return;
        }

        if let Some((from, side)) = self.anchor_at(event.position) {
            self.gesture = Gesture::Roping { from, side, at: world, over: None };
            cx.notify();
            return;
        }

        // Twice on a card opens it for typing: its words if it has any, its
        // name otherwise. This is the discoverable way in; `F2` and `Enter` are
        // the ones you learn.
        if event.click_count >= 2 {
            if let Some(id) = self.hit(world) {
                self.select_only(&id);
                self.start_editing(&id, cx);
                cx.notify();
                return;
            }
        }

        match self.hit(world) {
            Some(id) => {
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
                let open = self.doc.board.start();
                let start = ids
                    .iter()
                    .filter_map(|id| {
                        self.doc.board.item(id).map(|item| (id.clone(), point(item.x, item.y)))
                    })
                    .collect();
                self.gesture = Gesture::Moving { from: world, start, moved: false, open };
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
                    self.gesture = Gesture::Marquee { from: world, to: world, additive: true };
                } else {
                    self.gesture = Gesture::Panning { from: world, moved: false, clearing: true };
                }
            }
        }
        cx.notify();
    }

    fn on_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let world = self.world_at(event.position);

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
        let Self { doc, gesture, viewport, pan_trail, .. } = self;

        match gesture {
            Gesture::None => {}
            Gesture::Panning { from, moved, .. } => {
                // Move the camera so that the world point grabbed at the press
                // stays under the pointer. Working in world units rather than
                // accumulating screen deltas is what stops the board sliding
                // out from under the cursor during a zoom mid-drag.
                let anchor = *from;
                // Exactly as the move of a card measures it: the camera holds
                // the anchor under the pointer, so a frame where the pointer
                // did not move is a frame where this is zero.
                *moved |= world.x != anchor.x || world.y != anchor.y;
                viewport.pan.x -= world.x - anchor.x;
                viewport.pan.y -= world.y - anchor.y;
                // Where the camera is, and when. Sampled here rather than from
                // the pointer because the pan is what carries on afterwards,
                // so this is already in the units the projection wants — and
                // it stays right through a zoom mid-drag, which a screen-space
                // trail would not.
                pan_trail.push(viewport.pan, Instant::now());
            }
            Gesture::Marquee { to, .. } => *to = world,
            // Handled above, where `self` is still whole.
            Gesture::Roping { .. } => {}
            Gesture::Sizing { id, grip, start, shape, moved, cropping, open } => {
                let to_grid =
                    doc.board.settings.desktop.snap.then_some(doc.board.settings.desktop.grid_step);
                // What the modifiers mean, and the order they are asked in.
                // A picture keeps its shape unless somebody says otherwise;
                // anything else is free unless `Shift` says otherwise. Two
                // defaults, because a photograph and a sticky note want
                // opposite things and only one of them is ever stretched on
                // purpose.
                let crop = alt && shape.is_some();
                let keep = if crop || free {
                    None
                } else {
                    shape.or_else(|| shift.then(|| start.width() / start.height()))
                };
                let box_ = crate::grips::resized(*grip, *start, world, keep, to_grid);
                *moved = true;
                *cropping |= crop;
                let id = id.clone();
                doc.board.during(open, |board| {
                    if let Some(item) = board.item_mut(&id) {
                        let centre = box_.centre();
                        item.x = centre.x;
                        item.y = centre.y;
                        item.w = box_.width();
                        item.h = box_.height();
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
            Gesture::Moving { from, start, moved, open } => {
                let (dx, dy) = (world.x - from.x, world.y - from.y);
                if !*moved && dx == 0.0 && dy == 0.0 {
                    return;
                }
                *moved = true;
                let snap = doc.board.settings.desktop.snap;
                let step = doc.board.settings.desktop.grid_step;
                // Through the open gesture: this writes and records nothing,
                // because the step for the whole drag is closed at the release.
                doc.board.during(open, |board| {
                    for (id, home) in start.iter() {
                        if let Some(item) = board.item_mut(id) {
                            // The free position: where the card would be with no
                            // grid at all. Kept off the card's own x/y so that
                            // the next frame has something unrounded to measure
                            // from, and mirrored into `presnap` so that turning
                            // snapping off can put the card back rather than
                            // leaving it on the lattice.
                            let free = point(home.x + dx, home.y + dy);
                            let to = dropped_at(*home, dx, dy, snap.then_some(step));
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
            }
        }
        cx.notify();
    }

    fn on_mouse_up(&mut self, _event: &MouseUpEvent, _window: &mut Window, cx: &mut Context<Self>) {
        // Every gesture ends in exactly one place. A release that lands outside
        // the canvas is wired to this too, so a drag off the edge cannot leave
        // the pipeline stuck mid-gesture.
        //
        // Taken rather than read, so that ending a gesture owns whatever the
        // gesture was holding — the open step, for a move — and the pipeline is
        // back at rest before any of it is acted on.
        let ended = std::mem::replace(&mut self.gesture, Gesture::None);

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
        // A hand that stopped before letting go gets nothing — see
        // `Trail::velocity`, which is where that rule lives.
        if let Gesture::Panning { .. } = ended {
            let trail = std::mem::take(&mut self.pan_trail);
            if let Some((vx, vy)) = trail.velocity(Instant::now()) {
                self.camera.fling(self.viewport.pan, vx, vy);
            }
            cx.notify();
            return;
        }

        if let Gesture::Moving { start, moved, open, .. } = ended {
            // A press that never travelled is a click. Closing it would be
            // closing an empty gesture, and the ledger would refuse the step
            // anyway — but saying "moved" about it would still be a lie.
            if moved {
                let n = start.len();
                let label = match n {
                    1 => "Move".to_string(),
                    n => format!("Move {n}"),
                };
                // A drop that finds a host clears the unstick. That is the
                // other half of `meta.loose` being a decision: the decision was
                // "not this one", and putting the note down on a card is how
                // you take it back. Written inside the same step as the move,
                // because it is the same gesture.
                self.restick(&start, &open);
                if self.doc.board.finish(&label, open) {
                    self.say(match n {
                        1 => "moved".to_string(),
                        n => format!("moved {n}"),
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
                    self.say("nothing there to join to".into());
                    cx.notify();
                }
            }
            return;
        }

        if let Gesture::Marquee { from, to, additive } = ended {
            let rect =
                Rect::new(from.x.min(to.x), from.y.min(to.y), from.x.max(to.x), from.y.max(to.y));
            if !additive {
                self.selection.clear();
            }
            // Through the index, like every other question about where things
            // are. A sweep over a corner of a large board should cost the
            // corner, not the board.
            let mut swept = Vec::new();
            self.index().in_rect(rect, &mut swept);
            let items = &self.doc.board.items;
            let caught: Vec<String> = swept
                .into_iter()
                .map(|i| &items[i as usize])
                .filter(|i| i.kind.is_content())
                .map(|i| i.id.clone())
                .collect();
            for id in caught {
                if !self.is_selected(&id) {
                    self.selection.push(id);
                }
            }
        }
        cx.notify();
    }

    fn on_scroll(
        &mut self,
        event: &ScrollWheelEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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

        // The one mode. While the switcher is open it takes every press,
        // because it is a text field and a text field that let some of its
        // letters be shortcuts would be a text field you cannot type in.
        if let Some(switcher) = &mut self.switcher {
            let reply = switcher.key(key, mods, event.keystroke.key_char.as_deref());
            match reply {
                Reply::Held => {}
                Reply::Close => self.switcher = None,
                Reply::Open => {
                    let chosen = switcher.chosen();
                    self.switcher = None;
                    if let Some(path) = chosen {
                        self.open_board(&path, cx);
                    }
                }
            }
            cx.notify();
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
                Some(editor::Reply::Held) => {
                    self.show_edit();
                    cx.notify();
                    return;
                }
                Some(editor::Reply::Commit) => {
                    self.stop_editing(true, cx);
                    return;
                }
                Some(editor::Reply::Revert) => {
                    self.stop_editing(false, cx);
                    return;
                }
                // Copy, cut and paste inside text are the clipboard's, not the
                // board's, and they are the only three commands that mean
                // something different in here than out there.
                Some(editor::Reply::Ignored) if mods.secondary => match key {
                    "c" | "x" => {
                        self.copy_text(key == "x", cx);
                        return;
                    }
                    "v" => {
                        self.paste_text(cx);
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

        // Escape puts a tool away before it clears the selection: leaving a
        // mode is the nearest thing to undo about the press.
        if key == "escape" && self.tool != Tool::Select {
            self.choose_tool(Tool::Select, cx);
            return;
        }

        // Escape closes an open menu before it clears the selection, which is
        // the order somebody pressing it expects: the nearest thing first.
        if self.menu.is_some() && key == "escape" {
            self.close_menu();
            cx.notify();
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
            let ids = self.selection.clone();
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
        camera || anchors || self.images.arriving() || self.wires.fading()
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
        // A set rather than the list, for the same reason `draw_list` keeps
        // one: `lit` is asked once per connection, and a walk of the selection
        // inside it makes a board with everything selected cost connections
        // times cards.
        let selection: HashSet<String> = self.selection.iter().cloned().collect();
        // The label being typed, if one is, so the rope shows what is being
        // written on it rather than what it said before the session started.
        let typing = match &self.editing {
            Some(open) => match &open.on {
                Subject::Rope(a, b) => Some((a.clone(), b.clone(), open.editor.text().to_string())),
                Subject::Card(..) => None,
            },
            None => None,
        };

        // The one setting that turns the whole feature off. Checked here rather
        // than at the painter so that a board with the lines switched off costs
        // nothing to route as well as nothing to draw.
        if !self.doc.board.settings.desktop.web {
            self.wires.forget();
            self.drawn.clear();
            return (Vec::new(), Vec::new());
        }

        let Self { doc, grid, wires, .. } = self;
        let items = &doc.board.items;
        let drawn = wires.plan(
            &doc.board,
            visible,
            settled,
            |c| {
                chosen
                    .as_ref()
                    .is_some_and(|(a, b)| (a == &c.a && b == &c.b) || (a == &c.b && b == &c.a))
                    || selection.contains(&c.a)
                    || selection.contains(&c.b)
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
                .filter(|_| vp.zoom > 0.25)
                .map(|text| (SharedString::from(text), on_screen(wire.middle())));

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
    fn draw_list(&mut self, cx: &mut Context<Self>) -> Vec<Draw> {
        let vp = self.viewport;
        let theme = self.theme;
        let board_fit = self.doc.board.media_fit.clone();
        // Cloned out of `self`, because the loop below holds the board
        // immutably while `begin_decode` at the end wants it mutably.
        let editing = self.editing.clone();
        let visible = self.visible_by_depth();

        // The selection as a set, once, rather than a walk of it per card.
        //
        // A linear scan is the right shape for the one or two cards a
        // selection usually holds and the wrong one for the case that hurts:
        // select everything on a full board and drag it, and a scan per
        // visible card against a selection of twenty thousand is the product
        // of the two, every frame. Building the set is one pass over the
        // selection and makes the loop below flat in it.
        let picked: HashSet<&str> = self.selection.iter().map(String::as_str).collect();

        let mut wanted: Vec<String> = Vec::new();
        let mut out = Vec::with_capacity(visible.len());

        for i in visible {
            let item = &self.doc.board.items[i as usize];
            let centre = vp.to_screen(point(item.x, item.y));
            let (w, h) = ((item.w * vp.zoom).max(1.0), (item.h * vp.zoom).max(1.0));
            let body = Bounds::new(
                gpui::point(px(centre.x - w / 2.0), px(centre.y - h / 2.0)),
                gpui::size(px(w), px(h)),
            );
            let smallest = w.min(h);
            let selected = picked.contains(item.id.as_str());

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
                    lines: Vec::new(),
                    font_size: px(1.0),
                    text: theme.text,
                    caret: None,
                    highlight: Vec::new(),
                    grips: false,
                    frame: false,
                });
                continue;
            }

            let plain = smallest < LOD_PLAIN;
            let radius = if plain { px(0.0) } else { px((4.0 * vp.zoom).clamp(1.0, 8.0)) };
            let border = if plain { px(0.0) } else { px(1.0) };

            // The picture, if there is one and it is worth an atlas tile. The
            // asked-for hashes are collected rather than started here, because
            // starting a decode wants `cx` and this loop is holding the board.
            let picture = if smallest >= LOD_PICTURE {
                match picture_hash(item) {
                    Some(hash) => match self.images.look(hash) {
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
                        Load::Waiting | Load::Failed => None,
                    },
                    None => None,
                }
            } else {
                None
            };

            let (font_size, pad) = (CARD_TEXT, CARD_PAD);

            // A card being typed into draws the editor's text rather than its
            // own, caret and all — and draws it over a picture, because
            // renaming a photograph is a thing you have to be able to see.
            let being_edited =
                editing.as_ref().filter(|open| open.on.card().is_some_and(|(id, _)| id == item.id));
            let (lines, caret, highlight) = match being_edited {
                Some(open) => (
                    // Raw, and unstyled. What is being typed is the text
                    // itself, marks and all: a note *is* Markdown, so writing
                    // one means seeing the marks you are writing. Rendering
                    // them away under the caret would also move the caret,
                    // since the characters it counts would stop being the
                    // characters on the screen.
                    open.editor.lines().into_iter().map(markdown::Line::plain).collect(),
                    Some(open.editor.caret_line()),
                    open.editor.highlight(),
                ),
                // The label. Skipped where the card is too small to read, and
                // skipped where a picture already fills the card — a photograph
                // does not need its filename written across it.
                None if picture.is_none() && w > LOD_LABEL_W && h > LOD_LABEL_H => {
                    let inner_w = (w - pad * 2.0).max(1.0);
                    let inner_h = (h - pad * 2.0).max(1.0);
                    let columns =
                        (inner_w / (font_size * AVERAGE_ADVANCE)).floor().max(1.0) as usize;
                    let rows = (inner_h / (font_size * CARD_LEADING)).floor().max(1.0) as usize;
                    let words = label_for(item);
                    let lines = match item.kind {
                        // A note is Markdown, and a card is where it is read.
                        // Everything else is a label — a filename with an
                        // underscore in it is a filename, not an italic.
                        ItemType::Note | ItemType::Text => markdown::lay_out(&words, columns, rows),
                        _ => wrap(&words, columns, rows)
                            .into_iter()
                            .map(markdown::Line::plain)
                            .collect(),
                    };
                    (lines, None, Vec::new())
                }
                None => (Vec::new(), None, Vec::new()),
            };

            // Handles, on what is selected and big enough to put them on. Not
            // on a turned card: nothing draws rotation yet, so a handle would
            // be somewhere the card visibly is not.
            let grips = selected
                && item.rot == 0.0
                && w >= crate::grips::TOO_SMALL
                && h >= crate::grips::TOO_SMALL;

            out.push(Draw {
                body,
                radius,
                // A fence is a wash rather than a block, so the grid and
                // anything behind it still read through.
                fill: if item.kind == ItemType::Fence {
                    theme.colour_of(item).opacity(0.22)
                } else {
                    theme.colour_of(item)
                },
                edge: if selected {
                    theme.selected_edge
                } else if item.kind == ItemType::Fence {
                    theme.fence
                } else {
                    theme.card_edge
                },
                border,
                selected: selected && !plain,
                picture,
                lines,
                font_size: px(font_size),
                text: theme.text,
                caret,
                highlight,
                grips,
                frame: item.kind == ItemType::Fence,
            });
        }

        for hash in wanted {
            self.begin_decode(&hash, cx);
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
        let decoding = cx.background_executor().spawn(async move { crate::images::decode(&bytes) });
        cx.spawn(async move |view, cx| {
            let decoded = decoding.await;
            // `ok()` rather than unwrap: the window can close while a decode is
            // in flight, and a picture nobody is going to look at is not an
            // error worth a panic.
            view.update(cx, |view, cx| {
                view.images.settle(&hash, decoded);
                cx.notify();
            })
            .ok();
        })
        .detach();
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
        let marquee = match &self.gesture {
            Gesture::Marquee { from, to, .. } => Some((*from, *to)),
            _ => None,
        };

        let entity = cx.entity();

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

                for draw in &draws {
                    let body = shift(draw.body, origin);
                    window.paint_quad(quad(
                        body,
                        draw.radius,
                        draw.fill,
                        if draw.frame { draw.border.max(px(2.0)) } else { draw.border },
                        draw.edge,
                        if draw.frame { BorderStyle::Dashed } else { BorderStyle::Solid },
                    ));

                    if let Some((image, at, arrived)) = &draw.picture {
                        let at = shift(*at, origin);
                        // Clipped to the card, because `cover` deliberately
                        // computes a rectangle larger than the card in one
                        // axis and the overflow is the part being cropped.
                        window.with_content_mask(Some(ContentMask { bounds: body }), |window| {
                            // Best effort: an atlas that will not take another
                            // tile should cost this frame a picture, not the
                            // whole frame.
                            let _ =
                                window.paint_image(at, draw.radius.into(), image.clone(), 0, false);
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
                        if *arrived < 1.0 {
                            window.paint_quad(quad(
                                body,
                                draw.radius,
                                draw.fill.opacity(1.0 - *arrived),
                                px(0.0),
                                gpui::transparent_black(),
                                BorderStyle::Solid,
                            ));
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

                    if draw.lines.is_empty() {
                        continue;
                    }
                    let pad = px(CARD_PAD);
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
                            let colour = if line.muted { theme.muted } else { draw.text };
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
                                    color: if span.style.link { theme.accent } else { colour },
                                    // A wash rather than a monospaced face:
                                    // asking for a family this build cannot be
                                    // sure is installed gets the body face back
                                    // and no way to know it happened.
                                    background_color: span
                                        .style
                                        .code
                                        .then(|| theme.muted.opacity(0.16)),
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
                            (shaped, size * CARD_LEADING)
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
                    let size = px(11.0);
                    let shaped = window.text_system().shape_line(text.clone(), size, &[run], None);
                    let pad = px(5.0);
                    let height = size * 1.5;
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
    /// thing they act on, and what is left is this: what board you are on, how
    /// far in, and what just happened.
    fn status_bar(&self) -> impl IntoElement {
        let board = &self.doc.board;
        let title = if board.title.is_empty() { "untitled" } else { &board.title };

        let mut line = format!(
            "{title}  ·  {}%  ·  {} of {}",
            self.viewport.percent().round(),
            self.selection.len(),
            board.items.len(),
        );
        // Mentioned only when there is something in it. A permanently visible
        // empty bin is a readout that is wrong most of the time.
        if !board.trash.is_empty() {
            line.push_str(&format!("  ·  {} in the bin", board.trash.len()));
        }
        // Likewise for the lines, which most boards have none of.
        if !board.connections.is_empty() {
            line.push_str(&format!("  ·  {} connections", board.connections.len()));
        }
        // Likewise: on a board of notes this never appears, rather than reading
        // "0 pictures" forever.
        if self.images.ready_count() > 0 {
            line.push_str(&format!(
                "  ·  {} pictures, {} MB",
                self.images.ready_count(),
                self.images.bytes_held() / (1024 * 1024),
            ));
        }
        if let Some(Said { text: status, .. }) = &self.said {
            line.push_str("  ·  ");
            line.push_str(status);
        }

        div()
            .absolute()
            .bottom_0()
            .left_0()
            .right_0()
            .flex()
            .justify_between()
            .items_center()
            .px(px(12.0))
            .py(px(6.0))
            .bg(self.theme.chrome)
            .border_t_1()
            .border_color(self.theme.chrome_edge)
            .text_size(px(11.0))
            .text_color(self.theme.muted)
            .child(line)
            // What the pointer means, when it means something other than the
            // default — a tool is a mode, and a mode you cannot see is a trap.
            // Falls back to the one hint, because the right button is the only
            // other thing in here somebody has no way of guessing at.
            .child(div().child(self.tool.hint_line().unwrap_or("right-click for more")))
    }
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
fn write_to(board: &mut mbrd_core::Board, on: &Subject, text: &str) {
    match on {
        Subject::Card(id, field) => {
            if let Some(item) = board.item_mut(id) {
                write_field(item, *field, text);
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
fn write_field(item: &mut Item, field: Field, text: &str) {
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
    }
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
    /// The picture, where it goes — which for `cover` is deliberately larger
    /// than the card — and how far it has arrived. See `images::ARRIVING`.
    picture: Option<(Arc<RenderImage>, Bounds<Pixels>, f32)>,
    /// The words, already broken into lines that fit and into runs that are
    /// each set one way. A note's Markdown is read here rather than in the
    /// painter, which only knows how to draw a run.
    lines: Vec<markdown::Line>,
    font_size: Pixels,
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
    /// Whether to draw the four corner dots. The edges resize too, but they
    /// are not drawn — see [`Grip::at`](crate::grips::Grip::at).
    grips: bool,
    /// Whether this is furniture rather than a card — a fence.
    ///
    /// Drawn as a dashed outline over a wash instead of as a solid block. A
    /// fence is a region of the board rather than a thing on it, and one drawn
    /// as an enormous opaque card reads as a card somebody forgot to fill in.
    frame: bool,
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
/// `columns` is an estimate from the font size rather than a measurement; see
/// [`AVERAGE_ADVANCE`]. Everything here counts `char`s rather than bytes, so a
/// line of accented text wraps where it looks like it should.
fn wrap(text: &str, columns: usize, rows: usize) -> Vec<SharedString> {
    let mut out: Vec<String> = Vec::new();
    'outer: for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let mut word = word;
            // A word too long for a line of its own has to be cut, or the
            // greedy loop below would never place it and would spin.
            while word.chars().count() > columns {
                if !line.is_empty() {
                    out.push(std::mem::take(&mut line));
                    if out.len() == rows {
                        break 'outer;
                    }
                }
                let cut = word.char_indices().nth(columns).map(|(i, _)| i).unwrap_or(word.len());
                out.push(word[..cut].to_string());
                if out.len() == rows {
                    break 'outer;
                }
                word = &word[cut..];
            }
            let would_be =
                line.chars().count() + usize::from(!line.is_empty()) + word.chars().count();
            if !line.is_empty() && would_be > columns {
                out.push(std::mem::take(&mut line));
                if out.len() == rows {
                    break 'outer;
                }
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
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
            while last.chars().count() > columns.saturating_sub(1) && !last.is_empty() {
                last.pop();
            }
            last.push('\u{2026}');
        }
    }

    out.into_iter().map(SharedString::from).collect()
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

        // Hand back the atlas tiles of anything the cache evicted since the
        // last frame. Here because this is the one place in the frame that has
        // a window, and skipping it does not break anything visible — it just
        // leaks, in the one place a heap profile does not look.
        self.images.sweep(window);

        // The face labels are drawn in. Read once, here, because the paint
        // closure runs without a style stack to ask.
        let font = window.text_style().font();

        // Cull, then reduce to what the painter can own. A board may hold
        // twenty thousand items and only the ones on screen are worth a quad.
        let draws = self.draw_list(cx);
        let (wires, marks) = self.wire_list(window.mouse_position().into());
        let cursor = self.cursor_at(window.mouse_position());
        let board = self.paint_board(draws, wires, marks, font, cursor, cx);

        let tools = crate::tools::render(self, cx);
        let menu =
            self.menu.clone().map(|open| crate::menu::render(&open, self, cx).into_any_element());
        let switcher = self
            .switcher
            .clone()
            .map(|open| crate::switcher::render(&open, self, cx).into_any_element());

        div()
            .relative()
            .flex()
            .flex_col()
            .size_full()
            .bg(self.theme.ground)
            .text_color(self.theme.text)
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
                    // a short window may still reach it.
                    .children(menu)
                    .children(switcher),
            )
            // Last, so the grab strips sit above everything they overlap.
            .children(crate::titlebar::resize_handles(window))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str, columns: usize, rows: usize) -> Vec<String> {
        wrap(text, columns, rows).into_iter().map(|l| l.to_string()).collect()
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

        write_to(&mut board, &on, "  same    shelf \n ");
        assert_eq!(board.connections[0].meta.label.as_deref(), Some("same shelf"));

        // Cleared to absent rather than to an empty string: the format writes
        // the key only when there is one, so an emptied label has to take the
        // connection back to being a bare pair.
        write_to(&mut board, &on, "   ");
        assert_eq!(board.connections[0].meta.label, None);
        assert!(board.connections[0].meta.is_default());
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
        write_field(&mut swatch, Field::Name, "#3A5F2C");
        assert_eq!(swatch.meta.get("hex").and_then(|v| v.as_str()), Some("#3a5f2c"));
        assert_eq!(swatch.name, "#3A5F2C", "the name carries the same value, uppercased");
    }

    #[test]
    fn the_short_spelling_is_stored_the_long_way() {
        let mut swatch = Item::new("s", ItemType::Swatch);
        write_field(&mut swatch, Field::Name, "#fa0");
        assert_eq!(swatch.meta.get("hex").and_then(|v| v.as_str()), Some("#ffaa00"));
    }

    #[test]
    fn something_that_is_not_a_colour_is_just_a_name() {
        let mut swatch = Item::new("s", ItemType::Swatch);
        swatch.meta.insert("hex".into(), serde_json::json!("#123456"));
        write_field(&mut swatch, Field::Name, "warm grey");
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
        write_field(&mut photo, Field::Name, "#ff0000");
        assert_eq!(photo.name, "#ff0000");
        assert!(photo.meta.get("hex").is_none());
    }

    #[test]
    fn a_notes_words_go_where_the_format_keeps_them() {
        let mut note = Item::new("n", ItemType::Note);
        write_field(&mut note, Field::Note, "some words");
        assert_eq!(note.note_text(), Some("some words"));
        assert_eq!(note.name, "", "the name is not the words");
    }
}
