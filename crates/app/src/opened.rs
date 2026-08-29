//! A card, opened.
//!
//! Double-clicking anything on the board brings it up here: the whole window
//! below the titlebar, the board behind it gone rather than dimmed. It is the
//! same kind of surface the settings page is — see `settings.rs`, and `Overlay`
//! in `board_view.rs` for why there can only ever be one of them — and for the
//! same reason: a card is a *thumbnail* of something, and there has to be
//! somewhere that shows the thing.
//!
//! ## This module draws. It does not decide.
//!
//! What a card can be shown as, and what about it can be changed, are questions
//! about an item and its bytes rather than about a window, so they are answered
//! in [`mbrd_core::preview`] and [`mbrd_core::facts`] where they can be tested
//! without one. Everything here is a match on what those two returned. The
//! practical test of the split: this file holds no opinion about what a `.mp4`
//! is, and adding a format means adding a variant there and an arm here.
//!
//! ## One header, one body, one rail
//!
//! ```text
//! ┌ icon  title                                  [Edit] [i] [×] ┐
//! │ type · 2.4 MB · 1920×1080                                   │
//! ├──────────────────────────────────────────┬──────────────────┤
//! │   the preview, or the source             │   the facts      │
//! └──────────────────────────────────────────┴──────────────────┘
//! ```
//!
//! The header is identical for every type, and that is the point of it: a
//! gesture that works on some cards and not others is a gesture nobody trusts,
//! so a `.zip` opens onto the same furniture a photograph does. The rail is
//! **beside** the preview rather than in front of it, because looking up a
//! photograph's dimensions should not mean not looking at the photograph.
//!
//! ## Two ways a body is laid out, and the difference matters
//!
//! A document, a source file, a table and an archive listing are *longer than
//! the window* and scroll. A picture, a colour and a poster frame are **fitted
//! to the window** and do not — an image you have to scroll to see the bottom
//! of is an image the page failed to show you, which is the whole of what
//! "contained" means here. [`scrolls`] is the one place that decision is taken.
//!
//! ## The two texts a text card has
//!
//! A note carries its words in the board — `meta.text`, capped at
//! [`NOTE_MAX`](mbrd_core::model::NOTE_MAX) — which is what makes a note
//! searchable, diffable and small. A card that came from a file also keeps the
//! original bytes as an asset, because 512 characters is a paragraph and a
//! `.md` is usually not.
//!
//! So this page edits whichever of the two the card actually has. A note
//! somebody typed is the card's words and stays under the cap. A card that came
//! from a file is **the file**: the whole of it, edited here and written back
//! as a new asset, with `meta.text` refreshed to the first `NOTE_MAX`
//! characters so the card behind still says what the file starts with. Both go
//! through the ledger, so both undo.

use std::sync::Arc;

use gpui::{
    canvas, div, prelude::*, px, AnyElement, Context, Font, FontFallbacks, FontStyle, FontWeight,
    MouseButton, RenderImage, SharedString, StrikethroughStyle, StyledText, TextRun,
    UnderlineStyle,
};
use mbrd_core::facts::Fact;
use mbrd_core::markdown::{Align, Block, Marker, Run, Style, Table};
use mbrd_core::model::{Item, ItemAsset, ItemType};
use mbrd_core::preview::{Editable, Preview};

use crate::board_view::{BoardView, Field};
use crate::icons::{icon, Icon};
use crate::metrics::Estimate;
use crate::theme::Theme;

/// The page's own text sizes, which are a page's and not a card's.
///
/// Taken from the shape a Markdown preview has converged on everywhere it is
/// done well: body text at full size and full contrast rather than shrunk and
/// dimmed, generous leading, and real air between blocks. The version this
/// replaces on the card is none of those things on purpose — a card is a
/// thumbnail — which is exactly why the two need different numbers.
const BODY: f32 = 15.0;
/// Multiples of the font size. `1.5` for prose, tighter for a fence, where a
/// line is a line of code and the block is read as a shape.
const LEADING: f32 = 1.5;
const CODE_LEADING: f32 = 1.45;
/// The air between two blocks. One number for all of them, so the rhythm of a
/// document is even rather than a set of special cases.
const BLOCK_GAP: f32 = 16.0;
/// How wide the column of text is allowed to get.
///
/// A measure, not a window width: past about eighty characters the eye loses
/// the start of the next line, and a note read on a wide monitor would be one
/// long line after another.
const MEASURE: f32 = 760.0;
/// The air around the column, on both axes.
const PAGE_X: f32 = 48.0;
const PAGE_Y: f32 = 32.0;

/// How wide the information rail is.
///
/// Wide enough for `Content hash` and the hash beside it without the hash
/// wrapping, which is the longest pair [`mbrd_core::facts`] produces and the
/// one that is no use at all if it cannot be compared by eye.
const RAIL: f32 = 300.0;

/// How much larger than body text each heading level is set.
///
/// Six real steps, unlike the card's ramp — a page has the room, and a document
/// with six levels in it is a document where the sixth has to be visibly a
/// level. Everything from H4 down lands at or under the body size and is
/// carried by weight and colour instead, which is what keeps a deep heading
/// from shouting louder than the paragraph it introduces.
const HEADINGS: [f32; 6] = [26.0, 20.0, 17.0, 15.0, 13.5, 12.75];

/// The face a fence, a code span and the editor are set in.
///
/// Named rather than `"monospace"`, which is not a family any platform this
/// build runs on resolves reliably. The chain is the fixed-width faces a
/// developer's machine is likeliest to already have, and GPUI walks it in
/// order — the same bargain the root element makes for the UI face.
const MONO: &str = "JetBrains Mono";
const MONO_FALLBACKS: [&str; 6] =
    ["Fira Code", "Source Code Pro", "DejaVu Sans Mono", "Liberation Mono", "Menlo", "Consolas"];
const MONO_SIZE: f32 = 13.5;

/// Which card is open, and whether the rail is out.
///
/// Everything else on the page is read off the board each frame, the same
/// bargain `settings::Page` makes. The rail is the exception because it is not
/// a fact about the card — it is a thing somebody pressed a button to see.
#[derive(Debug, Clone)]
pub struct Opened {
    pub id: String,
    pub info: bool,
}

impl Opened {
    pub fn open(id: impl Into<String>, info: bool) -> Opened {
        Opened { id: id.into(), info }
    }
}

/// What the page needs from the window, measured before it is drawn.
///
/// Both of these want a `&mut` the render pass does not have while it is
/// holding the overlay — the picture cache is mutated by being asked, and the
/// advance comes from the text system — so they are taken first and handed in.
/// See `BoardView::render`.
pub struct Ready {
    /// The decoded picture, for a card that has one.
    pub picture: Option<Arc<RenderImage>>,
    /// Which frame of it. `0` for a still picture; for a GIF, an APNG or an
    /// animated WebP, wherever the card's playhead has got to — read from the
    /// same clock the card behind uses, so opening one does not restart it.
    pub frame: usize,
    /// The width of one character of [`MONO`] at [`MONO_SIZE`], which is what
    /// turns the editor's byte offsets into places on a screen. See
    /// [`source`].
    pub advance: f32,
}

/// The face a fence and the editor are set in, with its fallbacks attached.
pub fn mono() -> Font {
    let mut font = gpui::font(MONO);
    font.fallbacks =
        Some(FontFallbacks::from_fonts(MONO_FALLBACKS.iter().map(|s| s.to_string()).collect()));
    font
}

pub fn mono_size() -> f32 {
    MONO_SIZE
}

/// How tall one row of the editor is. Shared with `board_view.rs`, which turns
/// a press into a row by dividing by it.
pub fn line_height() -> f32 {
    MONO_SIZE * CODE_LEADING
}

/// The narrowest the editor's rows are allowed to get, in characters. Below
/// this a page is not narrow, it is broken.
const MIN_COLUMNS: f32 = 20.0;

/// How much room the editor's rows have in `width`, and how wide one character
/// of its face is — the pair [`crate::editor::Editor::wrapped`] wants.
///
/// The one place this is worked out, because two places would eventually be
/// two answers and the caret would land on the wrong row — see [`source`].
///
/// The page is set in a fixed-width face **on purpose**, and that is what makes
/// an [`Estimate`] here an exact answer rather than a guess: every character
/// really is `advance` wide, so the thing `metrics.rs` warns about cannot
/// happen. A card is the other case, and measures.
pub fn room_in(width: f32, advance: f32) -> (f32, Estimate) {
    let advance = advance.max(1.0);
    (width.max(MIN_COLUMNS * advance), Estimate::per_em(advance / MONO_SIZE))
}

pub fn render(
    opened: &Opened,
    ready: &Ready,
    view: &BoardView,
    cx: &mut Context<BoardView>,
) -> impl IntoElement {
    let theme = view.theme;
    let arriving = crate::board_view::arrival(view.overlay_presence.value());
    let item = view.doc.board.item(&opened.id);

    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        .flex()
        .flex_col()
        // A page, not a panel: it owns the whole space below the titlebar, so
        // the ground is solid and there is nothing behind to scrim.
        .bg(theme.ground.opacity(arriving.ground))
        .text_color(theme.text)
        // The wheel and both buttons end here — the board underneath still
        // exists, and a press that fell through would land on a card nobody can
        // see.
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        // A mesh's orbit drag, on the whole page rather than on the picture
        // it turns — `on_mouse_move`/`on_mouse_up` only fire while the
        // pointer is actually over the element they are bound to (see
        // gpui's own `Interactivity::on_mouse_move`, gated on
        // `hitbox.is_hovered`), and a real turn of the camera routinely
        // carries the pointer off a picture that is one region among several
        // on the page. The board's own canvas makes the same trade for the
        // same reason: catch the whole gesture on one region big enough that
        // a drag never outruns it, and let `self.gesture` decide whether
        // anything is done with it. `mesh_picture`'s own `on_mouse_down`
        // still starts the drag only when the press lands on the picture.
        .on_mouse_move(cx.listener(BoardView::on_mouse_move))
        .on_mouse_up(MouseButton::Left, cx.listener(BoardView::on_mouse_up))
        // The header and the body together, and not the ground under them: the
        // ground is nailed to the window and only goes opaque, while what is
        // drawn on it fades and rises into place. Moving the outer element
        // instead — which is what this used to do — slid the page's own
        // background off its own window and left a strip of live board showing
        // along the bottom edge of a page that was meant to have replaced it.
        // See `board_view::Arrival`.
        .child(
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .flex()
                .flex_col()
                .opacity(arriving.content)
                .mt(px(arriving.rise))
                .children(item.map(|item| head(item, opened, view, cx)))
                .child(match item {
                    Some(item) => middle(item, opened, ready, view, cx),
                    // The card was deleted from under the page — by an undo, or
                    // by the file being reloaded off disk. Saying so beats a
                    // blank page, and Escape is still the way out.
                    None => missing(theme),
                }),
        )
}

// ---------------------------------------------------------------------------
// What the page is looking at
// ---------------------------------------------------------------------------

/// How this card's contents should be put in front of somebody.
pub fn shown(item: &Item, view: &BoardView) -> Preview {
    mbrd_core::preview::of(item, view.asset_of(item))
}

/// Everything about this card that can be typed into, principal first.
pub fn fields(item: &Item, view: &BoardView) -> Vec<Editable> {
    mbrd_core::preview::editable(item, view.asset_of(item))
}

/// Whether a double-click on the body should open the words for typing.
///
/// Two conditions, and it takes both. The preview has to *be* the words: a
/// double-click on a photograph, a mesh or a waveform is not a request to
/// retype the file behind it, and there is nothing under the pointer there
/// that looks like text. And the words have to be the field the Edit button
/// would open anyway — [`BoardView::opened_principal`] is what decides that,
/// asked here rather than reimplemented, so the gesture and the button can
/// never disagree. A file too long to hold with a caret in it fails that test
/// and so stays readable and untypeable, which is the same answer the button
/// gives.
pub fn typeable(item: &Item, view: &BoardView) -> bool {
    reads(&shown(item, view))
        && matches!(view.opened_principal(&item.id), Some(Editable::Text { .. }))
}

/// Whether a preview of this kind is the card's words, shown as words.
///
/// The half of [`typeable`] that is a fact about the *kind* and so can be
/// asked without a window. Split out for that reason, the same way [`scrolls`]
/// is — a rule the body layout hangs off, written once.
///
/// A PDF is not on this list and neither is an archive: what those show is
/// extracted, and typing into it would be typing into a rendering of a file
/// rather than the file. They are read here and changed elsewhere.
fn reads(what: &Preview) -> bool {
    matches!(what, Preview::Document | Preview::Source { .. } | Preview::Sheet { .. })
}

/// Which hash the page wants decoded, for a card whose preview is a picture of
/// something.
///
/// **Not** `picture_hash` in `board_view.rs`, which answers the same question
/// for a *card* and answers it by type: a PNG that imported as `generic`
/// belongs on this page as a picture and on the board as a named file. Two
/// answers because they are two questions.
pub fn frame_of(id: &str, view: &BoardView) -> Option<String> {
    let item = view.doc.board.item(id)?;
    match shown(item, view) {
        Preview::Picture | Preview::Vector | Preview::Mesh => {
            item.asset.as_ref().and_then(ItemAsset::hash).map(str::to_string)
        }
        // A poster frame, which is the whole of what this build has behind a
        // clip. Nothing here writes one — but a board from
        // the original carries them.
        Preview::Video | Preview::Audio => {
            item.meta.get("cover").and_then(serde_json::Value::as_str).map(str::to_string)
        }
        _ => None,
    }
}

/// The whole text behind a card: the file it came from where there is one, and
/// the card's own words where there is not.
///
/// The module header is what this is implementing. The asset is preferred
/// because it is the *unabridged* copy — `meta.text` was cut to `NOTE_MAX` on
/// the way in — and because a page that showed the first 512 characters of a
/// file and called itself a full view would be the wrong kind of quiet.
pub fn words_of(item: &Item, view: &BoardView) -> String {
    if let Some(text) = file_text(item, view) {
        return text;
    }
    item.note_text().unwrap_or_default().to_string()
}

/// The card's asset, decoded as text, where it has one and it is text.
pub fn file_text(item: &Item, view: &BoardView) -> Option<String> {
    let asset = view.asset_of(item)?;
    // Not every asset on a note is text — a card's type can be changed after
    // the fact — and half-decoded rubbish is worse than the words the board
    // already has.
    mbrd_core::preview::readable_text(&asset.bytes)
        .then(|| String::from_utf8_lossy(&asset.bytes).into_owned())
}

/// Whether a preview of this kind is longer than the window or fitted to it.
///
/// See the module header. The rule is one line and the whole body layout hangs
/// off it, so it is a function rather than a condition written out twice.
fn scrolls(what: &Preview) -> bool {
    matches!(
        what,
        Preview::Document
            | Preview::Source { .. }
            | Preview::Sheet { .. }
            | Preview::Archive
            | Preview::Pdf
            | Preview::Font
    )
}

// ---------------------------------------------------------------------------
// The bar across the top
// ---------------------------------------------------------------------------

fn head(item: &Item, opened: &Opened, view: &BoardView, cx: &mut Context<BoardView>) -> AnyElement {
    let theme = view.theme;
    let id = item.id.clone();
    let typing = view.editing.is_some();

    div()
        .flex_none()
        .w_full()
        .flex()
        .items_center()
        .gap(px(10.0))
        .px(px(24.0))
        .py(px(14.0))
        .border_b_1()
        .border_color(theme.chrome_edge)
        .id(SharedString::from(format!("opened-{id}")))
        .child(icon(Icon::for_kind(&item.kind), crate::icons::ICON_MD, theme.muted))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_size(px(15.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(title(item, view)),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme.muted)
                        .truncate()
                        .child(subtitle(item, view)),
                ),
        )
        // Never grey. Every card has something to type into — that is
        // `preview::editable`'s guarantee, and this button is what it is for.
        .child(button(
            if typing { "Done" } else { "Edit" },
            theme,
            cx.listener(|this, _event, _window, cx| this.toggle_opened_typing(cx)),
        ))
        .child(mark(
            Icon::Told,
            "opened-info",
            opened.info,
            theme,
            cx.listener(|this, _event, _window, cx| this.toggle_opened_info(cx)),
        ))
        .child(mark(
            Icon::Close,
            "opened-close",
            false,
            theme,
            cx.listener(|this, _event, _window, cx| this.close_opened(cx)),
        ))
        .into_any_element()
}

fn button(
    label: &'static str,
    theme: Theme,
    press: impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> AnyElement {
    div()
        .id(label)
        .flex_none()
        .flex()
        .items_center()
        .px(px(10.0))
        .h(px(26.0))
        .rounded(px(crate::theme::RADIUS_SM))
        .border_1()
        .border_color(theme.chrome_edge)
        .text_size(px(12.0))
        .text_color(theme.text)
        .hover(|s| s.bg(theme.accent.opacity(0.10)))
        .active(|s| s.bg(theme.accent.opacity(0.18)))
        .on_mouse_down(MouseButton::Left, press)
        .child(label)
        .into_any_element()
}

/// A wordless square button. `lit` is for the one that is a toggle rather than
/// an action, so the rail being out is visible from the button that put it out.
fn mark(
    which: Icon,
    id: &'static str,
    lit: bool,
    theme: Theme,
    press: impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static,
) -> AnyElement {
    let mut box_ = div()
        .id(id)
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .size(px(26.0))
        .rounded(px(crate::theme::RADIUS_SM))
        .hover(|s| s.bg(theme.accent.opacity(0.10)))
        .active(|s| s.bg(theme.accent.opacity(0.18)))
        .on_mouse_down(MouseButton::Left, press);
    if lit {
        box_ = box_.bg(theme.accent.opacity(0.16));
    }
    box_.child(icon(which, crate::icons::ICON_MD, if lit { theme.accent } else { theme.muted }))
        .into_any_element()
}

/// What the card is called, with a fallback that is never empty.
fn title(item: &Item, view: &BoardView) -> String {
    if !item.name.trim().is_empty() {
        return item.name.clone();
    }
    match shown(item, view) {
        Preview::Document | Preview::Source { .. } => first_line(&words_of(item, view)),
        Preview::Address => item.url().unwrap_or("link").to_string(),
        _ => item.kind.as_str().to_string(),
    }
}

fn first_line(text: &str) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    let line = line.trim_start_matches(['#', '>', '-', '*', ' ']);
    if line.is_empty() {
        "note".into()
    } else {
        line.chars().take(80).collect()
    }
}

/// The line under the title: the first few facts, run together.
///
/// Drawn from the same list the rail holds rather than assembled separately, so
/// the header is a summary of the rail and not a second opinion about the card.
/// Which is also why it stops at three: a subtitle that wrapped would push the
/// body down every time somebody opened a photograph with long tags on it.
fn subtitle(item: &Item, view: &BoardView) -> String {
    mbrd_core::facts::of(item, view.asset_of(item))
        .into_iter()
        .filter(|fact| matches!(fact.name, "Type" | "Size" | "Pixels" | "Length"))
        .map(|fact| fact.value)
        .take(3)
        .collect::<Vec<_>>()
        .join(" · ")
}

// ---------------------------------------------------------------------------
// The body and the rail, side by side
// ---------------------------------------------------------------------------

fn middle(
    item: &Item,
    opened: &Opened,
    ready: &Ready,
    view: &BoardView,
    cx: &mut Context<BoardView>,
) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .w_full()
        .flex()
        .child(body(item, ready, view, cx))
        .children(opened.info.then(|| rail(item, view, cx)))
        .into_any_element()
}

fn body(item: &Item, ready: &Ready, view: &BoardView, cx: &mut Context<BoardView>) -> AnyElement {
    let theme = view.theme;
    let what = shown(item, view);
    // Typing into the words replaces the preview with the source, which is what
    // is actually being typed. Typing into a *short* field does not: a name is
    // edited in the rail, and swapping the whole body out from under somebody
    // renaming a photograph would hide the photograph they are naming it after.
    let typing = view.editing.as_ref().is_some_and(|open| {
        open.on.card().is_some_and(|(id, field)| id == item.id && field == Field::Note)
    });

    // Twice on the shown text swaps it for the source and starts a session, so
    // that the gesture that means "let me change these words" in every program
    // there has ever been means it here too. The card on the board answers a
    // double-click by opening this page — see `BoardView::on_mouse_down` — and
    // this is the second half of that: opening shows, opening again types.
    //
    // No caret is placed from where the press landed, and that is not an
    // oversight. What is under the pointer is the *rendered* markdown — a
    // heading at 20px, a fence in mono, an emphasis span in italic, all of it
    // laid out by gpui and none of it carrying a byte offset back. It is about
    // to be replaced by the source, whose rows are somewhere else entirely. A
    // caret guessed off the old layout would land in the wrong word often
    // enough to be worse than the end of the text, which is where a long field
    // opens. Once the source is up, `source`'s own ladder has real rows to
    // measure and does place the caret exactly.
    let words = !typing && typeable(item, view);

    let inner: AnyElement = match &what {
        _ if typing => source(view, ready, theme, cx),
        Preview::Document => document(&words_of(item, view), theme),
        Preview::Source { language } => listed(&words_of(item, view), *language, theme),
        Preview::Sheet { separator } => sheet(&words_of(item, view), *separator, theme),
        Preview::Archive => inside(item, view, theme),
        Preview::Pdf => pdf(item, view, theme),
        Preview::Font => specimen(item, view, theme, cx),
        // Rasterised by `images::decode` exactly like a raster picture is, so
        // `ready.picture` is already the right thing by the time this draws.
        Preview::Picture | Preview::Vector => picture(ready, theme),
        // A mesh's own picture, with a camera to drag and a wheel to zoom —
        // see `BoardView::begin_mesh_orbit`.
        Preview::Mesh => mesh_picture(item, ready, theme, cx),
        Preview::Video | Preview::Audio => reel(item, ready, view, theme),
        Preview::Colour => swatch(item, theme),
        Preview::Address => address(item, theme, cx),
        Preview::Nothing => nothing(item, theme),
    };

    // A long thing scrolls inside a centred column; a fitted thing takes the
    // room it is given and no more. See the module header.
    if scrolls(&what) || typing {
        return div()
            .id("opened-body")
            .when(words, |d| d.on_mouse_down(MouseButton::Left, opens(cx)))
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .justify_center()
            .overflow_y_scroll()
            // A column rather than the default row, so what is in it stretches
            // to the width instead of shrinking to its own contents — which for
            // the editor is also the width its wrap is measured against.
            .child(
                div()
                    .w_full()
                    .max_w(px(MEASURE + PAGE_X * 2.0))
                    .flex()
                    .flex_col()
                    .px(px(PAGE_X))
                    .py(px(PAGE_Y))
                    .child(inner),
            )
            .into_any_element();
    }
    div()
        .id("opened-body")
        .when(words, |d| d.on_mouse_down(MouseButton::Left, opens(cx)))
        .flex_1()
        .min_w_0()
        .min_h_0()
        .flex()
        .flex_col()
        .p(px(PAGE_Y))
        .child(inner)
        .into_any_element()
}

/// The listener behind `body`'s double-click, shared by its two wrappers.
///
/// The wrapper rather than the text itself, deliberately: a press in the
/// margin beside a paragraph, or below the last line of a short note, is still
/// a press on the page, and a body that only answered on the glyphs would be
/// the kind of target people have to aim at.
fn opens(
    cx: &mut Context<BoardView>,
) -> impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut gpui::App) + 'static {
    // The entity rather than `cx.listener`, which is what the rest of this
    // module reaches for: a listener borrows the context it was made from, and
    // this one outlives the call that made it because two wrappers want it.
    let entity = cx.entity();
    move |event: &gpui::MouseDownEvent, _window: &mut gpui::Window, cx: &mut gpui::App| {
        if event.click_count >= 2 {
            entity.update(cx, |view, cx| view.edit_opened_words(cx));
        }
    }
}

fn missing(theme: Theme) -> AnyElement {
    div()
        .flex_1()
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(theme.muted)
        .text_size(px(13.0))
        .child("this card is no longer on the board")
        .into_any_element()
}

// ---------------------------------------------------------------------------
// The rail
// ---------------------------------------------------------------------------

/// What is known about the card, and what about it can be changed.
///
/// Two sections, in that order deliberately: the fields are why somebody opens
/// the rail on a link, and the facts are why they open it on a zip. Both are
/// always present, because a rail whose sections came and went would be a rail
/// whose rows move under the pointer.
fn rail(item: &Item, view: &BoardView, cx: &mut Context<BoardView>) -> AnyElement {
    let theme = view.theme;
    div()
        .id("opened-rail")
        .flex_none()
        .w(px(RAIL))
        .h_full()
        .overflow_y_scroll()
        .border_l_1()
        .border_color(theme.chrome_edge)
        .bg(theme.chrome.opacity(0.35))
        .flex()
        .flex_col()
        .gap(px(18.0))
        .px(px(18.0))
        .py(px(20.0))
        .child(section(
            "Edit",
            theme,
            fields(item, view).into_iter().map(|what| field(item, what, view, cx)).collect(),
        ))
        .child(section(
            "About",
            theme,
            mbrd_core::facts::of(item, view.asset_of(item))
                .into_iter()
                // Every one but the tags, which are drawn as themselves a
                // little further down: a tag is a thing you can add and take
                // away, and a comma-separated line of them says it is not.
                .filter(|fact| fact.name != "Tags")
                .map(|fact| told(&fact, theme))
                .collect(),
        ))
        .child(section(
            "On the board",
            theme,
            standing(item, view).iter().map(|fact| told(fact, theme)).collect(),
        ))
        .children(tags(item, view, cx))
        .into_any_element()
}

/// What is true of this card *here* rather than of the file behind it.
///
/// A separate section from "About", and separate for a reason that is not
/// tidiness: everything above it is a fact about bytes and would read the same
/// on any board that held them, and everything here would be different on a
/// different board. How many cards share this file, how the picture is fitted
/// into the one you are looking at, what is roped to it. So it cannot live in
/// [`mbrd_core::facts`], which is given an item and its asset and nothing else.
///
/// Empty rows are left out, the same rule the section above follows: a card
/// with no ropes has no "Ropes" line rather than a line reading "none".
fn standing(item: &Item, view: &BoardView) -> Vec<Fact> {
    let board = &view.doc.board;
    let mut out = Vec::new();

    // How many cards are the same file. Worth saying because it is the one
    // fact on this page that changes what deleting the card means: a photograph
    // used twice is still on the board after one of them goes.
    if let Some(ItemAsset::Embedded { hash, .. }) = item.asset.as_ref() {
        let uses = board
            .items
            .iter()
            .filter(|other| {
                matches!(other.asset.as_ref(), Some(ItemAsset::Embedded { hash: h, .. }) if h == hash)
            })
            .count();
        out.push(Fact { name: "Used by", value: plural(uses, "card"), mono: false });
    }

    // Only where there is a picture to fit. The card's own choice where it has
    // made one, and the board's where it has not — which is the same order
    // `draw_list` reads them in, so the rail cannot say "Contain" about a card
    // being drawn cropped.
    if matches!(item.kind, ItemType::Image | ItemType::Video) {
        let fit = item
            .meta
            .get("fit")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&board.media_fit)
            .to_string();
        let mut said = fit.chars();
        let said = match said.next() {
            Some(first) => first.to_uppercase().collect::<String>() + said.as_str(),
            None => fit,
        };
        out.push(Fact { name: "Fit", value: said, mono: false });
    }

    if let Some(said) = rope_tally(&board.connections, &item.id) {
        out.push(Fact { name: "Ropes", value: said, mono: false });
    }

    out
}

/// The ropes touching one card, counted the way somebody looking at it counts.
///
/// By direction, because "2 ropes" and "1 in, 1 out" answer different questions
/// and the second is the one somebody looking at a card in the middle of a
/// diagram is asking. Direction is read *from this card's end*: the same rope
/// is an "out" on one of the two cards it joins and an "in" on the other, which
/// is the whole reason the arrow is drawn. A rope with no arrow is neither, and
/// is counted as itself rather than guessed at.
///
/// `None` where there are none, so the caller leaves the row out entirely
/// instead of printing "Ropes — none", which is a line that says nothing and
/// takes a line to say it.
fn rope_tally(wires: &[mbrd_core::model::Connection], id: &str) -> Option<String> {
    use mbrd_core::model::ConnDir;
    let (mut incoming, mut outgoing, mut plain) = (0usize, 0usize, 0usize);
    for wire in wires {
        // A rope from a card to itself would otherwise be read as leaving only.
        // It counts once at each end, which is what it is.
        for first in [true, false] {
            if (if first { &wire.a } else { &wire.b }) != id {
                continue;
            }
            match (wire.meta.dir, first) {
                (ConnDir::Fwd, true) | (ConnDir::Back, false) => outgoing += 1,
                (ConnDir::Fwd, false) | (ConnDir::Back, true) => incoming += 1,
                (ConnDir::Both, _) => {
                    incoming += 1;
                    outgoing += 1;
                }
                (ConnDir::None, _) => plain += 1,
            }
        }
    }
    let mut said: Vec<String> = Vec::new();
    if incoming > 0 {
        said.push(format!("{incoming} in"));
    }
    if outgoing > 0 {
        said.push(format!("{outgoing} out"));
    }
    if plain > 0 {
        said.push(plural(plain, "rope"));
    }
    match said.is_empty() {
        true => None,
        false => Some(said.join(", ")),
    }
}

/// The card's tags, as tags, with somewhere to add one.
///
/// Nothing at all for a card that cannot carry them — see
/// [`mbrd_core::tags::taggable`] — because an "add a tag" button on a card that
/// will not take one is the exact failure that made fences look broken.
fn tags(item: &Item, view: &BoardView, cx: &mut Context<BoardView>) -> Option<AnyElement> {
    if !mbrd_core::tags::taggable(item) {
        return None;
    }
    let theme = view.theme;
    let worn = mbrd_core::tags::of(item);
    let id = item.id.clone();

    let chip = |text: SharedString, dashed: bool| {
        div()
            .px(px(8.0))
            .py(px(2.0))
            .rounded(px(crate::theme::RADIUS_XS))
            .text_size(px(11.0))
            .border_1()
            .border_color(theme.chrome_edge)
            .when(!dashed, |d| d.bg(theme.chrome).text_color(theme.text))
            .when(dashed, |d| d.border_dashed().text_color(theme.tertiary))
            .child(text)
    };

    Some(
        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(
                div()
                    .text_size(px(10.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.tertiary)
                    .child("Tags"),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(5.0))
                    .children(worn.into_iter().map(|tag| chip(tag.into(), false)))
                    .child(
                        div()
                            .id("opened-add-tag")
                            .cursor_pointer()
                            .hover(|s| s.text_color(theme.text))
                            // Selects the card first. The tag list works on the
                            // selection — see `BoardView::tag_selection` — and
                            // the card you have open is not necessarily the
                            // card that was selected when you opened it.
                            .on_click(cx.listener(move |this, _event, _window, cx| {
                                this.open_tag_list_for(&id, cx);
                            }))
                            .child(chip("+ tag".into(), true)),
                    ),
            )
            .into_any_element(),
    )
}

/// "3 cards", and "1 card" rather than "1 cards".
fn plural(n: usize, word: &str) -> String {
    match n {
        1 => format!("1 {word}"),
        n => format!("{n} {word}s"),
    }
}

fn section(name: &'static str, theme: Theme, rows: Vec<AnyElement>) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .pb(px(6.0))
                .text_size(px(10.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.tertiary)
                .child(name),
        )
        .children(rows)
        .into_any_element()
}

/// One fact: a name, and what it says.
fn told(fact: &Fact, theme: Theme) -> AnyElement {
    div()
        .flex()
        .items_start()
        .gap(px(10.0))
        .py(px(5.0))
        .border_b_1()
        .border_color(theme.chrome_edge.opacity(0.6))
        .child(
            div()
                .flex_none()
                .w(px(88.0))
                .text_size(px(11.0))
                .text_color(theme.muted)
                .child(fact.name),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(if fact.mono { 11.5 } else { 12.5 }))
                .font_family(if fact.mono { MONO } else { ".SystemUIFont" })
                .child(SharedString::from(fact.value.clone())),
        )
        .into_any_element()
}

/// One editable field: what it is called, and either its value or a caret in it.
///
/// A row rather than a form, because only one thing is ever being typed at once
/// — `Editing` is a single session, and that is what makes the whole of it one
/// undo step. Pressing a row starts that session on this field, which is also
/// the only way to reach a card's *second* field: the header button starts the
/// principal one.
fn field(item: &Item, what: Editable, view: &BoardView, cx: &mut Context<BoardView>) -> AnyElement {
    let theme = view.theme;
    let name = match what {
        Editable::Text { .. } => "Text",
        Editable::Hex => "Hex",
        Editable::Url => "Address",
        Editable::Name => "Name",
    };
    let live = view.editing.as_ref().is_some_and(|open| {
        open.on.card().is_some_and(|(id, field)| id == item.id && field == field_of(what))
    });

    let value: AnyElement = match (live, what) {
        // The words are typed in the page, at a size a page can carry — see
        // `body`. The row still lights up, so the rail says where the caret is.
        (true, Editable::Text { .. }) => div()
            .text_size(px(12.0))
            .text_color(theme.accent_text)
            .child("typing, in the page")
            .into_any_element(),
        (true, _) => div()
            .py(px(1.0))
            .child(source(
                view,
                &Ready { picture: None, frame: 0, advance: view.opened_advance() },
                theme,
                cx,
            ))
            .into_any_element(),
        (false, _) => {
            let said = said(item, what, view);
            let empty = said.trim().is_empty();
            div()
                .text_size(px(12.5))
                .text_color(if empty { theme.tertiary } else { theme.text })
                .truncate()
                .child(SharedString::from(if empty { "—".to_string() } else { said }))
                .into_any_element()
        }
    };

    let id = item.id.clone();
    let mut row = div()
        .id(SharedString::from(format!("field-{name}")))
        .flex()
        .items_start()
        .gap(px(10.0))
        .py(px(5.0))
        .border_b_1()
        .border_color(theme.chrome_edge.opacity(0.6))
        .child(
            div()
                .flex_none()
                .w(px(88.0))
                .text_size(px(11.0))
                .text_color(if live { theme.accent } else { theme.muted })
                .child(name),
        )
        .child(div().flex_1().min_w_0().child(value));
    if !live {
        row = row.hover(|s| s.bg(theme.accent.opacity(0.08))).on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _event, _window, cx| {
                this.edit_opened_field(&id, what, cx);
            }),
        );
    }
    row.into_any_element()
}

/// Which of the card's texts a given editable is, for the session that types
/// into it. See `board_view::Field` for why a swatch's colour is its name.
pub fn field_of(what: Editable) -> Field {
    match what {
        Editable::Text { .. } => Field::Note,
        Editable::Hex | Editable::Name => Field::Name,
        Editable::Url => Field::Url,
    }
}

/// What a field says when nobody is typing into it.
fn said(item: &Item, what: Editable, view: &BoardView) -> String {
    match what {
        // One line of it. The rail is three hundred pixels wide and the whole
        // of the text is already on the page beside it.
        Editable::Text { .. } => first_line(&words_of(item, view)),
        Editable::Hex => item
            .meta
            .get("hex")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(&item.name)
            .to_uppercase(),
        Editable::Url => item.url().unwrap_or_default().to_string(),
        Editable::Name => item.name.clone(),
    }
}

// ---------------------------------------------------------------------------
// A document
// ---------------------------------------------------------------------------

/// The note, set as a page.
///
/// One `StyledText` per run of inline content, which is what makes the wrapping
/// right: a paragraph is one shaped string with styled ranges over it, rather
/// than a row of boxes that would wrap at the boundary between two of them and
/// leave a ragged edge in the middle of a sentence.
pub fn document(text: &str, theme: Theme) -> AnyElement {
    let blocks = mbrd_core::markdown::parse(text);
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(BLOCK_GAP))
        .text_size(px(BODY))
        .line_height(px(BODY * LEADING))
        .children(blocks.iter().map(|block| set(block, theme)))
        .into_any_element()
}

fn set(block: &Block, theme: Theme) -> AnyElement {
    match block {
        // The gap is already the space between two blocks. A second one would
        // be a blank paragraph, and a page that grew a hole every time somebody
        // pressed Enter twice would be a page nobody could lay out.
        Block::Gap => div().into_any_element(),
        Block::Paragraph(runs) => paragraph(runs, theme, Style::default(), BODY).into_any_element(),
        Block::Heading { level, runs } => {
            let level = (*level as usize).clamp(1, 6);
            let size = HEADINGS[level - 1];
            let mut head = div()
                .pt(px(if level <= 2 { 8.0 } else { 4.0 }))
                .line_height(px(size * 1.3))
                .child(paragraph(runs, theme, Style { bold: true, ..Style::default() }, size));
            // A rule under the top two levels and nothing under the rest. It
            // separates the *sections* of a document; putting one under every
            // heading turns a page into a stack of boxes.
            if level <= 2 {
                head = head.pb(px(6.0)).border_b_1().border_color(theme.chrome_edge);
            }
            // Below the body size the scale has stopped carrying the level, so
            // colour does: an H5 is body text that is quieter, not louder.
            if size < BODY {
                head = head.text_color(theme.muted);
            }
            head.into_any_element()
        }
        Block::Quote(inner) => div()
            .flex()
            .flex_col()
            .gap(px(BLOCK_GAP * 0.5))
            .pl(px(14.0))
            .border_l_2()
            .border_color(theme.quote)
            .text_color(theme.muted)
            .children(inner.iter().map(|b| set(b, theme)))
            .into_any_element(),
        Block::List { items, .. } => div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .children(items.iter().map(|entry| {
                div()
                    .flex()
                    .items_start()
                    .gap(px(8.0))
                    .child(
                        div()
                            .flex_none()
                            .min_w(px(18.0))
                            .text_color(match entry.marker {
                                Marker::Bullet => theme.muted,
                                _ => theme.tertiary,
                            })
                            .child(bullet(&entry.marker)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(6.0))
                            .children(entry.blocks.iter().map(|b| set(b, theme))),
                    )
                    .into_any_element()
            }))
            .into_any_element(),
        Block::Code { lines, .. } => fence(lines, theme),
        Block::Rule => div().h(px(1.0)).my(px(8.0)).bg(theme.chrome_edge).into_any_element(),
        Block::Table(table) => grid(table, theme),
    }
}

fn bullet(marker: &Marker) -> String {
    match marker {
        Marker::Bullet => "\u{2022}".into(),
        Marker::Number(typed) => format!("{typed}."),
        Marker::Task(false) => "\u{2610}".into(),
        Marker::Task(true) => "\u{2611}".into(),
    }
}

/// One run of inline content, shaped as one string with styled ranges.
fn paragraph(runs: &[Run], theme: Theme, over: Style, size: f32) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .children(runs.iter().map(|run| line(run, theme, over, size)))
        .into_any_element()
}

fn line(run: &Run, theme: Theme, over: Style, size: f32) -> AnyElement {
    if run.is_empty() {
        // An empty run still takes a line's height, or a deliberate break in
        // the middle of a paragraph would close up.
        return div().h(px(size * LEADING)).into_any_element();
    }
    let text: String = run.iter().map(|s| s.text.as_str()).collect();
    let mut runs: Vec<TextRun> = Vec::with_capacity(run.len());
    for span in run {
        let style = Style {
            bold: span.style.bold || over.bold,
            italic: span.style.italic || over.italic,
            code: span.style.code || over.code,
            strike: span.style.strike || over.strike,
            link: span.style.link || over.link,
        };
        runs.push(TextRun {
            len: span.text.len(),
            font: face(style),
            color: if style.link { theme.note_link } else { theme.text },
            // A wash rather than a rounded chip: GPUI paints a text run's
            // background as the run's own box, and rounding it would mean
            // painting per glyph index against a layout this does not hold.
            // The wash is what the card already does, at a size where it
            // reads as a chip anyway.
            background_color: style.code.then(|| theme.chrome.opacity(0.9)),
            underline: style.link.then_some(UnderlineStyle {
                thickness: px(1.0),
                color: Some(theme.note_link.opacity(0.5)),
                wavy: false,
            }),
            strikethrough: style.strike.then_some(StrikethroughStyle {
                thickness: px(1.0),
                color: Some(theme.text.opacity(0.7)),
            }),
        });
    }
    div()
        .text_size(px(size))
        .child(StyledText::new(SharedString::from(text)).with_runs(runs))
        .into_any_element()
}

/// The face one span is set in. Code goes to the fixed-width family; everything
/// else stays on whatever the page inherited, which is the UI face.
fn face(style: Style) -> Font {
    let mut font = if style.code { mono() } else { gpui::font(".SystemUIFont") };
    font.weight = if style.bold { FontWeight::SEMIBOLD } else { FontWeight::NORMAL };
    font.style = if style.italic { FontStyle::Italic } else { FontStyle::Normal };
    font
}

fn fence(lines: &[String], theme: Theme) -> AnyElement {
    let mut block = div()
        .p(px(12.0))
        .rounded(px(crate::theme::RADIUS_SM))
        .bg(theme.chrome.opacity(0.7))
        .border_1()
        .border_color(theme.chrome_edge)
        .font_family(MONO)
        .text_size(px(MONO_SIZE))
        .line_height(px(line_height()))
        .flex()
        .flex_col()
        .children(lines.iter().map(|line| {
            // A blank line inside a fence is still a line of the program.
            div().child(SharedString::from(if line.is_empty() {
                " ".to_string()
            } else {
                line.clone()
            }))
        }));
    block.text_style().get_or_insert_with(Default::default).font_fallbacks =
        Some(FontFallbacks::from_fonts(MONO_FALLBACKS.iter().map(|s| s.to_string()).collect()));
    block.into_any_element()
}

fn grid(table: &Table, theme: Theme) -> AnyElement {
    let width = table.head.len().max(table.rows.iter().map(Vec::len).max().unwrap_or(0));
    let cell = move |run: Option<&Run>, head: bool, column: usize| {
        let align = table.align.get(column).copied().unwrap_or(Align::Ragged);
        let mut box_ = div()
            .flex_1()
            .min_w_0()
            .px(px(10.0))
            .py(px(4.0))
            .border_1()
            .border_color(theme.chrome_edge);
        box_ = match align {
            Align::Right => box_.text_right(),
            Align::Center => box_.text_center(),
            Align::Ragged | Align::Left => box_,
        };
        match run {
            Some(run) => {
                box_.child(line(run, theme, Style { bold: head, ..Style::default() }, BODY))
            }
            None => box_,
        }
    };
    let row = |cells: &[Run], head: bool| {
        div().flex().children((0..width).map(|n| cell(cells.get(n), head, n))).into_any_element()
    };

    div()
        .flex()
        .flex_col()
        .children((!table.head.is_empty()).then(|| row(&table.head, true)))
        .children(table.rows.iter().map(|cells| row(cells, false)))
        .into_any_element()
}

// ---------------------------------------------------------------------------
// The previews that are lists of rows
// ---------------------------------------------------------------------------

/// Whether a `diff` or `patch` line is an addition or a removal, and not one
/// of the two `+++`/`---` file headers every unified diff also starts with a
/// `+` and a `-` on.
fn diff_color(language: Option<&str>, row: &str, theme: &Theme) -> Option<gpui::Hsla> {
    if language != Some("Diff") || row.starts_with("+++") || row.starts_with("---") {
        return None;
    }
    if row.starts_with('+') {
        Some(theme.diff_add)
    } else if row.starts_with('-') {
        Some(theme.diff_remove)
    } else {
        None
    }
}

/// A text file, set the way a text file is read: one line per line, in a
/// fixed-width face, with the lines numbered.
///
/// Nothing is highlighted beyond [`diff_color`] — the first thing anybody
/// needs from a source preview is to be able to say "line 40", and full
/// syntax colour after that.
fn listed(text: &str, language: Option<&'static str>, theme: Theme) -> AnyElement {
    let rows: Vec<&str> = text.lines().collect();
    let width = rows.len().to_string().len();
    let mut block = div()
        .w_full()
        .flex()
        .flex_col()
        .font_family(MONO)
        .text_size(px(MONO_SIZE))
        .line_height(px(line_height()))
        .children(rows.iter().enumerate().map(|(n, row)| {
            let mut line = div().flex_1().min_w_0();
            if let Some(colour) = diff_color(language, row, &theme) {
                line = line.text_color(colour);
            }
            div()
                .flex()
                .gap(px(14.0))
                .child(
                    div()
                        .flex_none()
                        .text_color(theme.tertiary)
                        .child(SharedString::from(format!("{:>width$}", n + 1))),
                )
                .child(line.child(SharedString::from(match row.is_empty() {
                    true => " ".to_string(),
                    false => (*row).to_string(),
                })))
        }));
    block.text_style().get_or_insert_with(Default::default).font_fallbacks =
        Some(FontFallbacks::from_fonts(MONO_FALLBACKS.iter().map(|s| s.to_string()).collect()));

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .children(language.map(|name| stamp(name, theme)))
        .child(block)
        .into_any_element()
}

/// A small label over a preview, saying what it turned out to be.
///
/// A row around the chip rather than the chip itself, because a child of a
/// column stretches to the column's width — and a label the width of the page
/// is not a label.
fn stamp(said: &str, theme: Theme) -> AnyElement {
    div()
        .flex()
        .child(
            div()
                .px(px(7.0))
                .py(px(2.0))
                .rounded(px(crate::theme::RADIUS_SM))
                .bg(theme.chrome.opacity(0.8))
                .text_size(px(10.5))
                .text_color(theme.muted)
                .child(SharedString::from(said.to_string())),
        )
        .into_any_element()
}

/// A CSV, as the table it is.
///
/// The first row is treated as the heading, which is what a spreadsheet
/// exported from anything has and what nothing in the file itself can tell you.
/// Getting it wrong costs one row set in bold; not doing it at all costs the
/// whole reason to draw a table rather than its source.
fn sheet(text: &str, separator: char, theme: Theme) -> AnyElement {
    let rows = mbrd_core::preview::rows(text, separator);
    let Some((head, rest)) = rows.split_first() else {
        return nothing_said("this file has no rows in it", theme);
    };
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    let cell = |value: Option<&String>, head: bool| {
        let mut box_ = div()
            .flex_1()
            .min_w_0()
            .px(px(10.0))
            .py(px(5.0))
            .border_1()
            .border_color(theme.chrome_edge)
            .text_size(px(12.5))
            .truncate();
        if head {
            box_ = box_.font_weight(FontWeight::SEMIBOLD).bg(theme.chrome.opacity(0.6));
        }
        box_.child(SharedString::from(value.cloned().unwrap_or_default()))
    };

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(stamp(
            &format!("{} rows · {width} columns", mbrd_core::facts::count(rows.len())),
            theme,
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .child(div().flex().children((0..width).map(|n| cell(head.get(n), true))))
                .children(
                    rest.iter().map(|row| {
                        div().flex().children((0..width).map(|n| cell(row.get(n), false)))
                    }),
                ),
        )
        .into_any_element()
}

/// What is inside an archive.
fn inside(item: &Item, view: &BoardView, theme: Theme) -> AnyElement {
    let Some(asset) = view.asset_of(item) else {
        return nothing_said("the bytes for this card are missing", theme);
    };
    let entries = mbrd_core::preview::listing(&asset.bytes);
    if entries.is_empty() {
        return nothing_said("this archive would not open", theme);
    }
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(10.0))
        .child(stamp(&format!("{} entries", mbrd_core::facts::count(entries.len())), theme))
        .child(div().flex().flex_col().font_family(MONO).text_size(px(12.0)).children(
            entries.iter().map(|entry| {
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.0))
                    .py(px(3.0))
                    .border_b_1()
                    .border_color(theme.chrome_edge.opacity(0.5))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(if entry.folder { theme.muted } else { theme.text })
                            .child(SharedString::from(entry.path.clone())),
                    )
                    .child(div().flex_none().text_color(theme.tertiary).child(SharedString::from(
                        match entry.folder {
                            true => String::new(),
                            false => mbrd_core::facts::size(entry.size as usize),
                        },
                    )))
            }),
        ))
        .into_any_element()
}

/// A PDF's pages, run together and pulled apart at their own blank lines.
///
/// This is real decoding — walking compressed content streams and each font's
/// own encoding to get back to characters `lopdf::Document::extract_text`
/// already knows how to do — which is why it happens here and not in
/// `mbrd_core::preview`. See [`Preview::Pdf`].
fn pdf_text(bytes: &[u8]) -> Option<String> {
    let doc = lopdf::Document::load_mem(bytes).ok()?;
    let pages: Vec<u32> = doc.get_pages().keys().copied().collect();
    doc.extract_text(&pages).ok()
}

/// A PDF, as the text pulled out of its pages.
fn pdf(item: &Item, view: &BoardView, theme: Theme) -> AnyElement {
    let Some(asset) = view.asset_of(item) else {
        return nothing_said("the bytes for this card are missing", theme);
    };
    let Some(text) = pdf_text(&asset.bytes) else {
        return nothing_said("this PDF would not open", theme);
    };
    let paragraphs: Vec<&str> =
        text.split("\n\n").map(str::trim).filter(|p| !p.is_empty()).collect();
    if paragraphs.is_empty() {
        return nothing_said("this PDF has no text to pull out", theme);
    }
    let pages = item.meta.get("pages").and_then(serde_json::Value::as_u64);

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(BLOCK_GAP))
        .children(
            pages.map(|n| stamp(&format!("{} pages", mbrd_core::facts::count(n as usize)), theme)),
        )
        .text_size(px(BODY))
        .line_height(px(BODY * LEADING))
        .children(
            paragraphs
                .into_iter()
                .map(|p| div().child(SharedString::from(p.to_string())).into_any_element()),
        )
        .into_any_element()
}

/// Hand a font's bytes to the window's text system, once per content hash.
///
/// Registering the same bytes again on every render this page happens to be
/// open for would be work with no different result each time — the hash is
/// the same reason an asset is only ever decoded once per picture, just kept
/// here instead of in `images.rs` because nothing else needs a dropped font's
/// *pixels*, only whichever window asks the platform text system to draw with
/// its name.
fn register_font(hash: &str, bytes: &[u8], cx: &mut Context<BoardView>) {
    static REGISTERED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        std::sync::OnceLock::new();
    let seen = REGISTERED.get_or_init(Default::default);
    if !seen.lock().unwrap().insert(hash.to_string()) {
        return;
    }
    let _ = cx.text_system().add_fonts(vec![std::borrow::Cow::Owned(bytes.to_vec())]);
}

/// A font, as a specimen set in its own face — a headline, then the
/// characters somebody drops a font on a board to check for in the first
/// place.
fn specimen(
    item: &Item,
    view: &BoardView,
    theme: Theme,
    cx: &mut Context<BoardView>,
) -> AnyElement {
    let Some(asset) = view.asset_of(item) else {
        return nothing_said("the bytes for this card are missing", theme);
    };
    let Some(family) = item.meta.get("family").and_then(serde_json::Value::as_str) else {
        return nothing_said("this font's own name table would not open", theme);
    };
    let family = family.to_string();
    match item.asset.as_ref().and_then(ItemAsset::hash) {
        Some(hash) => register_font(hash, &asset.bytes, cx),
        None => _ = cx.text_system().add_fonts(vec![std::borrow::Cow::Owned(asset.bytes.clone())]),
    }

    let row = |text: &'static str, size: f32| {
        div()
            .font_family(family.clone())
            .text_size(px(size))
            .line_height(px(size * LEADING))
            .child(SharedString::from(text))
    };

    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(BLOCK_GAP))
        .child(stamp(&family, theme))
        .child(row("The quick brown fox jumps over the lazy dog", 44.0))
        .child(row("ABCDEFGHIJKLMNOPQRSTUVWXYZ", 20.0))
        .child(row("abcdefghijklmnopqrstuvwxyz", 20.0))
        .child(row("0123456789 .,:;!? — @#&%", 20.0))
        .into_any_element()
}

// ---------------------------------------------------------------------------
// The editor
// ---------------------------------------------------------------------------

/// The words, open for typing.
///
/// Set in the fixed-width face, and that is the decision the rest of this
/// falls out of. A proportional face would mean measuring every prefix of every
/// line to find out where the caret goes; a fixed-width one makes a column a
/// multiplication, so the caret is an element sitting *between* two pieces of
/// text rather than a rectangle painted at a measured `x`. Which is also the
/// right face for the job: what is being typed here is Markdown source, and
/// source is read in columns.
///
/// The wrap is [`Editor::wrapped`](crate::editor::Editor::wrapped), the same one
/// the card uses, so a click and a caret cannot disagree about which row a byte
/// is on.
///
/// The same block draws a name and an address in the rail. It is a session's
/// worth of text with a caret in it either way, and only one session is ever
/// open — so there is only ever one of these on the page, and the bounds it
/// captures are unambiguously the ones a press should be measured against.
fn source(
    view: &BoardView,
    ready: &Ready,
    theme: Theme,
    cx: &mut Context<BoardView>,
) -> AnyElement {
    let Some(editor) = view.editor() else { return div().into_any_element() };
    // How many characters fit, off the width the block was actually given.
    //
    // Measured rather than assumed, and this is the one number that has to be:
    // `board_view.rs` breaks the same width against the same advance to work
    // out which row a press landed on, and two answers a column apart would put
    // the caret on the wrong row of a wrapped line. So both go through
    // [`room_in`]. The first frame has no measurement yet and falls back to
    // the measure the page is capped at, which is the right answer on any
    // window wide enough to reach it.
    let (room, advance) = room_in(view.opened_width().unwrap_or(MEASURE), ready.advance);
    let rows = editor.wrapped(room, MONO_SIZE, &advance);
    let (caret_row, caret_at) = editor.caret_in(&rows);
    let lit = editor.highlight_in(&rows);
    let text = editor.text();

    let entity = cx.entity();
    let mut page = div()
        .id("opened-editor")
        .relative()
        .w_full()
        .flex()
        .flex_col()
        // A press anywhere in the block puts the caret where it landed, and
        // holding Shift takes the selection with it. The row divs are not what
        // is listened to — a press between two of them, or past the end of a
        // short line, has to land somewhere too, and the block is the thing
        // that is always under the pointer.
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|this, event: &gpui::MouseDownEvent, _window, cx| {
                // One press places and arms a drag; two, three and four are the
                // run, the line and the lot — the same ladder the board's own
                // cards answer to, off the platform's own click count.
                match event.click_count {
                    0 | 1 => {
                        this.place_opened_caret(event.position, event.modifiers.shift, cx);
                        this.select_text_drag(true);
                    }
                    clicks => this.select_opened_run_at(event.position, clicks, cx),
                }
            }),
        )
        // Sweeping the selection out. The block rather than the rows, for the
        // same reason the press is on the block: a pointer dragged past the end
        // of a short line is still dragging.
        .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _window, cx| {
            if this.selecting_text() {
                this.place_opened_caret(event.position, true, cx);
            }
        }))
        // And a release ends it wherever it lands. Wired here as well as on the
        // board because the page is over the canvas, so the canvas never sees
        // the button come back up.
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|this, _event: &gpui::MouseUpEvent, _window, _cx| {
                this.select_text_drag(false);
            }),
        )
        // Where the text starts, measured rather than assumed — the page is
        // centred and padded, so nothing outside the layout knows this. Same
        // shape as the board's own bounds capture in `board_view.rs`.
        .child(
            canvas(
                move |bounds, _window, cx| {
                    entity.update(cx, |view, cx| view.opened_text_at(bounds, cx));
                },
                |_, _, _, _| {},
            )
            .absolute()
            .size_full(),
        )
        .font_family(MONO)
        .text_size(px(MONO_SIZE))
        .line_height(px(line_height()))
        .children(rows.iter().enumerate().map(|(n, &(start, end))| {
            let row = &text[start..end];
            let wash: Vec<(usize, usize)> =
                lit.iter().filter(|(r, ..)| *r == n).map(|&(_, from, to)| (from, to)).collect();
            let caret = (n == caret_row).then_some(caret_at);
            // A row with nothing on it still has to be a row's worth of height,
            // or a blank line in the middle of a file would close up under the
            // caret as it passed through.
            div()
                .flex()
                .h(px(line_height()))
                .children(cut(row, &wash, caret).into_iter().map(|chunk| match chunk {
                    Chunk::Caret => caret_mark(theme),
                    Chunk::Text { from, to, selected } => {
                        let mut piece = div().child(SharedString::from(row[from..to].to_string()));
                        if selected {
                            piece = piece.bg(theme.accent.opacity(0.28));
                        }
                        piece.into_any_element()
                    }
                }))
                .into_any_element()
        }));
    page.text_style().get_or_insert_with(Default::default).font_fallbacks =
        Some(FontFallbacks::from_fonts(MONO_FALLBACKS.iter().map(|s| s.to_string()).collect()));
    page.into_any_element()
}

/// One drawn piece of an editor row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Chunk {
    Caret,
    Text { from: usize, to: usize, selected: bool },
}

/// Cut a row into the pieces it is drawn as: stretches of text that are all
/// washed or all not, with the caret standing between two of them.
///
/// Split out from the rendering and tested, because it is the one place in this
/// module that slices a string by an offset that came from somewhere else. The
/// offsets *should* all be on character boundaries — everything in `editor.rs`
/// moves by `char` and says so — but "should" is not what a slice wants, and
/// the failure is a panic on the thread that draws rather than a wrong pixel.
/// So every cut is snapped to a boundary on the way in and the invariant is
/// enforced here rather than assumed everywhere.
fn cut(row: &str, wash: &[(usize, usize)], caret: Option<usize>) -> Vec<Chunk> {
    let snap = |at: usize| {
        let at = at.min(row.len());
        (0..=at).rev().find(|&n| row.is_char_boundary(n)).unwrap_or(0)
    };
    let caret = caret.map(&snap);

    let mut cuts: Vec<usize> = vec![0, row.len()];
    for &(from, to) in wash {
        cuts.push(snap(from));
        cuts.push(snap(to));
    }
    cuts.extend(caret);
    cuts.sort_unstable();
    cuts.dedup();

    let mut out = Vec::with_capacity(cuts.len() + 1);
    for pair in cuts.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        if caret == Some(from) {
            out.push(Chunk::Caret);
        }
        if from == to {
            continue;
        }
        let selected = wash.iter().any(|&(a, b)| snap(a) <= from && to <= snap(b));
        out.push(Chunk::Text { from, to, selected });
    }
    // The caret at the very end of a row — or on a row with nothing on it —
    // has no piece after it to stand in front of, so it is added here.
    if caret.is_some_and(|at| at >= row.len()) {
        out.push(Chunk::Caret);
    }
    out
}

/// The caret: a hairline that takes up no room.
///
/// Half its width in negative margin on each side, so inserting it between two
/// pieces of a row does not push the text after it a pixel to the right — a
/// caret that moved the words as it passed them would be a caret you could
/// watch ruin its own line.
fn caret_mark(theme: Theme) -> AnyElement {
    div().w(px(1.5)).ml(px(-0.75)).mr(px(-0.75)).h_full().bg(theme.accent).into_any_element()
}

// ---------------------------------------------------------------------------
// The previews that are looked at rather than read
// ---------------------------------------------------------------------------

/// A picture, at the size the window has room for and no larger.
///
/// Contained in a box that is exactly the space left over, which is what makes
/// this *contained* rather than merely capped: a portrait photograph on a wide
/// window and a panorama on a tall one both arrive whole. The body around it
/// does not scroll — see the module header — so there is no bottom of the
/// picture to go looking for.
///
/// ## Why this paints rather than using `img()`
///
/// `img()` cannot contain a portrait photograph in a landscape window, and the
/// reason is structural rather than a matter of picking different styles.
/// GPUI's image element sets `aspect_ratio` on its layout node from the
/// picture's own shape, and Taffy's leaf layout treats an aspect ratio as a
/// **floor** on the height — `height = max(height, width / ratio)` — applied
/// after every other constraint, including an explicit `h_full()` and including
/// `max_h`. So a 700×2100 picture in a 1216×666 box was laid out 1216 **×
/// 3651**, and `ObjectFit::Contain` then dutifully fitted the picture to *that*
/// — full width, running off the bottom of the window. The taller the picture,
/// the further off the screen it went. A landscape picture never tripped it,
/// which is why this looked like a bug about vertical images specifically.
///
/// Nothing in the style vocabulary can undo a floor that is applied last, so
/// the fit is arithmetic here instead: one `canvas`, the bounds it is handed,
/// and the same [`gpui::ObjectFit::Contain`] sum against the picture's real
/// size. That is also how the board draws every card — see
/// `BoardView::paint_board` — so this is the page joining what was already the
/// app's one way of putting a picture on screen, rather than a special case.
fn picture(ready: &Ready, theme: Theme) -> AnyElement {
    match &ready.picture {
        Some(image) => {
            let image = image.clone();
            let frame = ready.frame;
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _: (), window, _| {
                            let at = gpui::ObjectFit::Contain.get_bounds(bounds, image.size(frame));
                            // Best effort, like the board's own paint: an atlas
                            // that will not take another tile should cost this
                            // frame a picture rather than the whole frame.
                            let _ = window.paint_image(
                                at,
                                gpui::Corners::default(),
                                image.clone(),
                                frame,
                                false,
                            );
                        },
                    )
                    .size_full(),
                )
                .into_any_element()
        }
        // Still decoding, or these bytes were never a picture. Both are
        // temporary from here — the board's own loader is what settles it —
        // so this says what it is rather than showing a broken frame.
        None => nothing_said("no picture to show", theme),
    }
}

/// A mesh's picture, dragged to orbit and scrolled to zoom.
///
/// Not `picture` — a photograph has no camera and nothing here competes with
/// it for the drag, so a plain image never needs this. A mesh always does:
/// unlike the board's small thumbnail, where the same gesture usually means
/// "move this card" and Position mode is what hands the drag over, the opened
/// page has nothing else for a drag on the picture to mean.
fn mesh_picture(
    item: &Item,
    ready: &Ready,
    theme: Theme,
    cx: &mut Context<BoardView>,
) -> AnyElement {
    match &ready.picture {
        Some(image) => {
            let image = image.clone();
            let frame = ready.frame;
            let id = item.id.clone();
            let id_for_scroll = id.clone();
            div()
                .id(SharedString::from(format!("mesh-{id}")))
                .flex_1()
                .min_h_0()
                .w_full()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, event: &gpui::MouseDownEvent, _window, cx| {
                        view.begin_mesh_orbit(&id, event.position, event.modifiers.shift, cx);
                    }),
                )
                // `on_mouse_move`/`on_mouse_up` are on the page as a whole —
                // see `render`'s own comment on why a drag needs a bigger
                // catch than the picture it turns.
                .on_scroll_wheel(cx.listener(
                    move |view, event: &gpui::ScrollWheelEvent, _window, cx| {
                        view.dolly_mesh(&id_for_scroll, event, cx);
                    },
                ))
                .child(
                    canvas(
                        |_, _, _| {},
                        move |bounds, _: (), window, _| {
                            let at = gpui::ObjectFit::Contain.get_bounds(bounds, image.size(frame));
                            let _ = window.paint_image(
                                at,
                                gpui::Corners::default(),
                                image.clone(),
                                frame,
                                false,
                            );
                        },
                    )
                    .size_full(),
                )
                .into_any_element()
        }
        None => nothing_said("no picture to show", theme),
    }
}

/// A clip or a recording.
///
/// A poster frame where the board carries one, and otherwise the plain truth:
/// there is nothing behind the playhead in this build. Drawn as a panel rather
/// than left blank because the rail beside it has the length, the artist and
/// the size — all of which are real — and a blank middle would read as those
/// being missing too.
fn reel(item: &Item, ready: &Ready, view: &BoardView, theme: Theme) -> AnyElement {
    if ready.picture.is_some() {
        return picture(ready, theme);
    }
    let peaks = item
        .asset
        .as_ref()
        .and_then(ItemAsset::hash)
        .and_then(|hash| view.doc.waveforms.get(hash))
        .map(|waveform| waveform.peaks.clone())
        .unwrap_or_default();

    let said = match item.kind {
        ItemType::Audio => "this build does not play sound yet",
        _ => "this build does not play video yet",
    };
    div()
        .flex_1()
        .min_h_0()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(18.0))
        .rounded(px(crate::theme::RADIUS_MD))
        .bg(theme.chrome.opacity(0.5))
        // The measured peaks, where the archive carries them. Nothing in this
        // build writes that sidecar, so this is for boards that came from one
        // that did — and it is a real preview of a recording, which is more
        // than the line under it can claim.
        .children((!peaks.is_empty()).then(|| {
            div().flex().items_center().h(px(120.0)).gap(px(2.0)).w(px(520.0)).children(
                peaks.iter().take(200).map(|peak| {
                    div()
                        .flex_1()
                        .h(px((peak.clamp(0.0, 1.0) * 110.0).max(2.0)))
                        .rounded(px(1.0))
                        .bg(theme.accent.opacity(0.55))
                }),
            )
        }))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .text_size(px(13.0))
                .text_color(theme.muted)
                .child(icon(Icon::for_kind(&item.kind), crate::icons::ICON_MD, theme.tertiary))
                .child(said),
        )
        .into_any_element()
}

fn swatch(item: &Item, theme: Theme) -> AnyElement {
    let hex = item.meta.get("hex").and_then(serde_json::Value::as_str).unwrap_or(&item.name);
    // Through the theme's own reader rather than a second one here: a swatch
    // with an unreadable hex has a documented fallback and this is not the
    // place to invent a different one.
    let colour = theme.colour_of(item);
    div()
        .flex_1()
        .min_h_0()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .rounded(px(crate::theme::RADIUS_MD))
                .border_1()
                .border_color(theme.chrome_edge)
                .bg(colour),
        )
        .child(
            div()
                .flex_none()
                .font_family(MONO)
                .text_size(px(15.0))
                .child(SharedString::from(hex.to_uppercase())),
        )
        .into_any_element()
}

/// A link's address, which is the one thing a link card is too small to show.
fn address(item: &Item, theme: Theme, cx: &mut Context<BoardView>) -> AnyElement {
    let url = item.url().unwrap_or_default().to_string();
    let open = url.clone();
    let mut box_ = div()
        .flex_1()
        .min_h_0()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(16.0))
        .child(icon(Icon::Link, crate::icons::ICON_LG, theme.tertiary))
        .child(
            div()
                .max_w(px(MEASURE))
                .font_family(MONO)
                .text_size(px(15.0))
                .text_color(if url.is_empty() { theme.tertiary } else { theme.note_link })
                .child(SharedString::from(match url.is_empty() {
                    true => "no address on this card".to_string(),
                    false => url.clone(),
                })),
        );
    if !url.is_empty() {
        box_ = box_.child(
            div()
                .id("opened-open-url")
                .px(px(12.0))
                .h(px(28.0))
                .flex()
                .items_center()
                .rounded(px(crate::theme::RADIUS_SM))
                .border_1()
                .border_color(theme.chrome_edge)
                .text_size(px(12.0))
                .hover(|s| s.bg(theme.accent.opacity(0.10)))
                .active(|s| s.bg(theme.accent.opacity(0.18)))
                // Leaving the app is a thing somebody asks for, so it is a
                // button rather than something the page does on opening.
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |_this, _event, _window, cx| cx.open_url(&open)),
                )
                .child("Open in browser"),
        );
    }
    box_.into_any_element()
}

/// A card there is genuinely nothing to draw for.
///
/// Not an apology. An `.fbx` or a `.3mf` on a board is a thing somebody put
/// there on purpose, and the rail beside this — which opens with the page for
/// exactly these cards, see `BoardView::open_card` — has its name, its size
/// and the hash that identifies it inside the archive, which is the whole of
/// what this build can truthfully say about it.
fn nothing(item: &Item, theme: Theme) -> AnyElement {
    let said = match item.kind {
        ItemType::Gone => "the bytes for this card were emptied out of the bin",
        _ => "nothing here can open this kind of file",
    };
    div()
        .flex_1()
        .min_h_0()
        .w_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(14.0))
        .child(icon(Icon::for_kind(&item.kind), crate::icons::ICON_LG, theme.tertiary))
        .child(div().text_size(px(13.0)).text_color(theme.muted).child(said))
        .into_any_element()
}

/// A panel that says why there is nothing in it.
fn nothing_said(said: &'static str, theme: Theme) -> AnyElement {
    div()
        .flex_1()
        .min_h_0()
        .w_full()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(crate::theme::RADIUS_MD))
        .bg(theme.chrome.opacity(0.5))
        .text_color(theme.muted)
        .text_size(px(13.0))
        .child(said)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The direction is read from *this card's* end, which is the whole point
    /// of the row: the same rope has to say "out" on one card and "in" on the
    /// other, or the arrow drawn between them is decoration.
    #[test]
    fn a_rope_is_an_out_at_one_end_and_an_in_at_the_other() {
        use mbrd_core::model::{ConnDir, ConnMeta, Connection};
        let wire = |a: &str, b: &str, dir| Connection {
            a: a.into(),
            b: b.into(),
            meta: ConnMeta { dir, ..ConnMeta::default() },
        };
        let wires = [wire("one", "two", ConnDir::Fwd)];
        assert_eq!(rope_tally(&wires, "one").as_deref(), Some("1 out"));
        assert_eq!(rope_tally(&wires, "two").as_deref(), Some("1 in"));
        // And a rope pointing the other way says the opposite at each end.
        let back = [wire("one", "two", ConnDir::Back)];
        assert_eq!(rope_tally(&back, "one").as_deref(), Some("1 in"));
        assert_eq!(rope_tally(&back, "two").as_deref(), Some("1 out"));
    }

    /// A rope with no arrow is not a direction anybody chose, so it is counted
    /// as itself rather than folded into one of the two that were.
    #[test]
    fn a_rope_with_no_arrow_is_counted_as_a_rope() {
        use mbrd_core::model::{ConnDir, ConnMeta, Connection};
        let wire = |dir| Connection {
            a: "one".into(),
            b: "two".into(),
            meta: ConnMeta { dir, ..ConnMeta::default() },
        };
        assert_eq!(rope_tally(&[wire(ConnDir::None)], "one").as_deref(), Some("1 rope"));
        assert_eq!(
            rope_tally(&[wire(ConnDir::None), wire(ConnDir::None)], "one").as_deref(),
            Some("2 ropes")
        );
        // Both ways at once is both, which is what the two arrowheads say.
        assert_eq!(rope_tally(&[wire(ConnDir::Both)], "one").as_deref(), Some("1 in, 1 out"));
    }

    /// Nothing rather than a row saying nothing. Every other line on the rail
    /// follows the same rule — see [`standing`].
    #[test]
    fn a_card_with_no_ropes_gets_no_rope_row() {
        use mbrd_core::model::{ConnDir, ConnMeta, Connection};
        let wires = [Connection {
            a: "two".into(),
            b: "three".into(),
            meta: ConnMeta { dir: ConnDir::Fwd, ..ConnMeta::default() },
        }];
        assert_eq!(rope_tally(&wires, "one"), None);
        assert_eq!(rope_tally(&[], "one"), None);
    }

    #[test]
    fn one_of_a_thing_is_not_one_things() {
        assert_eq!(plural(1, "card"), "1 card");
        assert_eq!(plural(2, "card"), "2 cards");
        assert_eq!(plural(0, "rope"), "0 ropes");
    }

    #[test]
    fn a_note_with_no_name_is_titled_by_its_first_words() {
        assert_eq!(first_line("# A heading\nand more"), "A heading");
    }

    #[test]
    fn a_note_with_nothing_in_it_is_still_titled() {
        assert_eq!(first_line(""), "note");
        assert_eq!(first_line("\n\n   \n"), "note");
    }

    #[test]
    fn a_diffs_added_and_removed_lines_are_coloured_and_the_headers_are_not() {
        let theme = Theme::dark();
        assert_eq!(diff_color(Some("Diff"), "+new line", &theme), Some(theme.diff_add));
        assert_eq!(diff_color(Some("Diff"), "-old line", &theme), Some(theme.diff_remove));
        assert_eq!(diff_color(Some("Diff"), "+++ b/file.rs", &theme), None);
        assert_eq!(diff_color(Some("Diff"), "--- a/file.rs", &theme), None);
        assert_eq!(diff_color(Some("Diff"), " context line", &theme), None);
        // A `+` at the front of a Rust file, or any other language, is not a
        // diff's `+` — the colouring is off unless `listed` was told this is one.
        assert_eq!(diff_color(Some("Rust"), "+1", &theme), None);
        assert_eq!(diff_color(None, "+1", &theme), None);
    }

    #[test]
    fn only_the_previews_that_are_words_can_be_typed_into() {
        assert!(reads(&Preview::Document));
        assert!(reads(&Preview::Source { language: Some("Rust") }));
        assert!(reads(&Preview::Sheet { separator: ',' }));
        // Shown, and shown from the bytes — but what is on screen is a
        // rendering, so a double-click on it is not a request to retype it.
        assert!(!reads(&Preview::Pdf));
        assert!(!reads(&Preview::Archive));
        assert!(!reads(&Preview::Font));
        // And nothing that is not text at all.
        assert!(!reads(&Preview::Picture));
        assert!(!reads(&Preview::Mesh));
        assert!(!reads(&Preview::Video));
        assert!(!reads(&Preview::Colour));
        assert!(!reads(&Preview::Address));
        assert!(!reads(&Preview::Nothing));
    }

    #[test]
    fn only_the_long_previews_scroll() {
        // The rule the whole body layout hangs off. A picture that scrolled
        // would be a picture the page failed to contain.
        assert!(scrolls(&Preview::Document));
        assert!(scrolls(&Preview::Source { language: None }));
        assert!(scrolls(&Preview::Sheet { separator: ',' }));
        assert!(scrolls(&Preview::Archive));
        assert!(scrolls(&Preview::Pdf));
        assert!(!scrolls(&Preview::Picture));
        assert!(!scrolls(&Preview::Mesh));
        assert!(!scrolls(&Preview::Colour));
        assert!(!scrolls(&Preview::Video));
        assert!(!scrolls(&Preview::Nothing));
    }

    /// A minimal, real PDF with one page of text, built through `lopdf` rather
    /// than hand-typed — a byte-accurate `xref` table is not something worth
    /// getting right by hand just for a test fixture.
    fn built_pdf() -> Vec<u8> {
        use lopdf::content::{Content, Operation};
        use lopdf::{dictionary, Document, Object, Stream};

        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font",
            "Subtype" => "Type1",
            "BaseFont" => "Courier",
        });
        let resources_id =
            doc.add_object(dictionary! { "Font" => dictionary! { "F1" => font_id } });
        let content = Content {
            operations: vec![
                Operation::new("BT", vec![]),
                Operation::new("Tf", vec!["F1".into(), 48.into()]),
                Operation::new("Td", vec![100.into(), 600.into()]),
                Operation::new("Tj", vec![Object::string_literal("Hello World!")]),
                Operation::new("ET", vec![]),
            ],
        };
        let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
        let page_id = doc.add_object(dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "Contents" => content_id,
        });
        let pages = dictionary! {
            "Type" => "Pages",
            "Kids" => vec![page_id.into()],
            "Count" => 1,
            "Resources" => resources_id,
            "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
        };
        doc.objects.insert(pages_id, Object::Dictionary(pages));
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog_id);

        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn a_pdfs_text_survives_the_round_trip_through_its_own_content_stream() {
        let text = pdf_text(&built_pdf()).expect("a document lopdf itself wrote should open");
        assert!(text.contains("Hello World!"), "got: {text:?}");
    }

    #[test]
    fn every_editable_names_a_field_that_can_be_written() {
        assert_eq!(field_of(Editable::Text { limit: 10 }), Field::Note);
        assert_eq!(field_of(Editable::Name), Field::Name);
        // A swatch's colour is stored as its name. See `write_field`.
        assert_eq!(field_of(Editable::Hex), Field::Name);
        assert_eq!(field_of(Editable::Url), Field::Url);
    }

    /// The text of each piece, and `|` where the caret stands.
    fn drawn(row: &str, wash: &[(usize, usize)], caret: Option<usize>) -> String {
        cut(row, wash, caret)
            .into_iter()
            .map(|chunk| match chunk {
                Chunk::Caret => "|".to_string(),
                Chunk::Text { from, to, selected } => match selected {
                    true => format!("[{}]", &row[from..to]),
                    false => row[from..to].to_string(),
                },
            })
            .collect()
    }

    #[test]
    fn a_row_with_nothing_on_it_still_holds_the_caret() {
        assert_eq!(drawn("", &[], Some(0)), "|");
        assert_eq!(drawn("", &[], None), "");
    }

    #[test]
    fn the_caret_stands_where_it_is_and_only_once() {
        assert_eq!(drawn("abcd", &[], Some(0)), "|abcd");
        assert_eq!(drawn("abcd", &[], Some(2)), "ab|cd");
        assert_eq!(drawn("abcd", &[], Some(4)), "abcd|", "the end of a row is a place too");
    }

    #[test]
    fn a_selection_is_the_middle_of_three_pieces() {
        assert_eq!(drawn("abcdef", &[(2, 4)], None), "ab[cd]ef");
        assert_eq!(drawn("abcdef", &[(0, 6)], None), "[abcdef]");
    }

    #[test]
    fn the_caret_and_the_selection_share_a_row_without_fighting() {
        // The caret sits at one end of what is lit, which is where a drag
        // leaves it, and the wash still has to come out as one piece.
        assert_eq!(drawn("abcdef", &[(2, 4)], Some(4)), "ab[cd]|ef");
        assert_eq!(drawn("abcdef", &[(2, 4)], Some(2)), "ab|[cd]ef");
    }

    #[test]
    fn an_offset_that_is_not_a_character_boundary_does_not_take_the_window_down() {
        // "é" is two bytes. An offset landing between them would panic on the
        // slice; snapped back to the boundary it is merely a caret a character
        // to the left, which nobody dies of.
        let row = "aéb";
        assert_eq!(drawn(row, &[], Some(2)), "a|éb");
        assert_eq!(drawn(row, &[(1, 3)], Some(9)), "a[é]b|", "past the end, and still whole");
    }
}
