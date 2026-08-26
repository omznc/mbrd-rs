//! The one place a colour is written down.
//!
//! Everything that draws asks the theme rather than naming a colour — there is
//! no `rgb(0x…)` anywhere else in this crate — which is what makes a palette
//! something the app can be handed rather than something it was compiled with.
//! [`Theme`] is one such palette; `themes.rs` is the list of them and where
//! they come from, and `THEMES.md` at the root of the repository is the same
//! thing written for somebody about to author one.
//!
//! Two are built in and neither can fail: [`Theme::dark`], which is the warm
//! palette the app has always drawn in, and [`Theme::light`], the same board on
//! paper. Both are the base a theme file inherits from, and both are what the
//! contrast tests at the bottom of this file measure — which is why they are
//! Rust rather than two more `.json`s. Everything else comes through
//! [`overlay`].
//!
//! The original recolours its whole interface from the pictures on the board
//! once there are three of them. That is still not here, and this module is
//! still shaped so that it can be: a palette derived from a board would be one
//! more thing that produces a `Theme`, alongside a file and a built-in.

use std::sync::Arc;

use gpui::{point, px, rgb, rgba, BoxShadow, FontFeatures, Hsla};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// `Copy` because it is a bag of colours that every drawing path wants a
/// private look at, and passing it by reference would tie the painter to the
/// view it came from — which is exactly the borrow a `'static` paint closure
/// cannot hold.
///
/// `Serialize` and `Deserialize` because this struct *is* the theme file
/// format. `gpui::Hsla` already reads and writes itself as `"#rrggbbaa"`, so
/// a field here is a key in a `.json` on somebody's disk without a line of
/// conversion code in between — and, more to the point, without a second
/// list of field names to keep in step with this one. See [`overlay`], which
/// is the whole of the loader that this buys.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Theme {
    /// Behind the canvas — the paper the board sits on.
    pub ground: Hsla,
    /// The dots of the grid.
    pub grid: Hsla,
    /// The world axes through the origin.
    pub axis: Hsla,

    /// The sidebar and any other furniture.
    pub chrome: Hsla,
    pub chrome_edge: Hsla,

    /// A card, and its outline.
    pub card: Hsla,
    pub card_edge: Hsla,
    /// A card that is selected.
    pub selected_edge: Hsla,

    pub text: Hsla,
    /// Labels, counts, the status bar, a placeholder, a tooltip's key —
    /// anything secondary that a person is still expected to *read*.
    ///
    /// Solid, on purpose. This used to be `text` at 47% alpha, which reads
    /// fine on the chrome it was drawn on and turns into something else
    /// entirely the moment a call site multiplies it by another opacity for a
    /// wash: 47% of 16% is 7.5%, not 16%, and the two ended up drawing on top
    /// of each other in more than one place with nobody having asked for the
    /// weaker number. A solid colour is one a caller can dim on purpose and
    /// nothing dims it by accident.
    pub muted: Hsla,
    /// Decorative marks that are not read as words: the titlebar's chevron,
    /// the little icon beside a status-bar count. Quieter than [`Self::muted`]
    /// deliberately — those are marks that repeat what the text beside them
    /// already says, so they are allowed to sit under the contrast floor a
    /// sentence needs. Never put a word in this colour.
    pub tertiary: Hsla,
    /// The accent as a *fill* or an edge: a selected card's outline, the wash
    /// behind a chosen row, the lit segment of a control.
    pub accent: Hsla,
    /// The accent as a *word*.
    ///
    /// A separate field because the two are held to different floors and the
    /// same colour cannot clear both. `accent` is furniture — an outline, a
    /// wash — and furniture needs 3:1; the moment the same colour spells the
    /// name of the chosen segment on the settings page, or the matched letters
    /// in the palette, it is a sentence and needs 4.5:1. The dark palette's
    /// `#b4553a` is 3.5:1 on its own chrome, which is fine for the border it
    /// was chosen for and was quietly failing in the six places that wrote in
    /// it. Same hue, same saturation, moved along lightness until it reads.
    pub accent_text: Hsla,

    /// Per-type card tints, so a board reads as a board at a glance rather than
    /// as a wall of identical grey rectangles.
    pub note: Hsla,
    pub image: Hsla,
    pub video: Hsla,
    pub audio: Hsla,
    pub link: Hsla,
    pub fence: Hsla,

    /// The note pad, `--note-1..4`. Muted, because a note is something to
    /// read.
    ///
    /// On the theme rather than beside it as a `const`, which is where these
    /// lived until themes were a thing somebody could choose. A pad of four
    /// dark tints is not a fact about note cards, it is a fact about a dark
    /// theme: on paper the same four have to be four *pale* washes or every
    /// note on the board turns into a hole punched through it. See
    /// [`Theme::note_tint`].
    pub notes: [Hsla; 4],
    /// What a swatch draws as when its `hex` is missing or is not a colour.
    ///
    /// Grey rather than the plain card, because a grey swatch is still a
    /// swatch and a card-coloured one looks like a card that failed to load.
    pub swatch_fallback: Hsla,

    /// A quote's bar and a rule's line, drawn on a card. Not [`Self::muted`],
    /// even though both used to be the same field: a quote is a *different*
    /// kind of quiet from a secondary label, and a colour tuned for the
    /// chrome behind [`Self::muted`] does not promise anything about sitting
    /// on the note tints underneath this one — which is why this is checked
    /// against the pad's own background rather than borrowed from a field
    /// that was.
    pub quote: Hsla,
    /// A markdown link, drawn on a card. Not [`Self::accent`] — the accent is
    /// what a *selected* card wears, and a link that borrowed it read as a
    /// selection sitting on an unrelated note. Accent's hue family, so a link
    /// still reads as "the same idea, elsewhere" without claiming to be one.
    pub note_link: Hsla,

    /// A `diff`'s added and removed lines. Not [`Self::rope_leaf`] and
    /// [`Self::rope_danger`], even where the values start out equal: those two
    /// are named colours a *connection* may be given, and a theme file that
    /// recoloured a board's connectors should not also recolour what `+` and
    /// `-` mean in a patch.
    pub diff_add: Hsla,
    pub diff_remove: Hsla,

    /// The five colours a connection may be named.
    ///
    /// Five fields rather than an array because the format names them — a
    /// connection is `"leaf"`, never a hex triple — and a name is the thing
    /// that survives a theme changing underneath it. See
    /// [`Theme::rope_for`], which is the only place the name meets a colour.
    pub rope_line: Hsla,
    pub rope_accent: Hsla,
    pub rope_warm: Hsla,
    pub rope_leaf: Hsla,
    pub rope_danger: Hsla,
    /// The marks that appear beside a card you are pointing at. Faint on
    /// purpose: an offer, not a control.
    pub anchor: Hsla,
    /// The rules that appear while a card is being dragged, saying what it has
    /// lined up with. See `core::guides`.
    ///
    /// Deliberately **not** the accent: the accent means "selected" everywhere
    /// else on this board, and a guide is a measurement rather than a thing you
    /// have hold of. Deliberately not a colour of its own either — the palette
    /// is warm and a saturated hue in the middle of it would read as another
    /// card. So it is the text colour, drawn as a hairline, which is only ever
    /// on screen while a hand is down.
    pub guide: Hsla,

    /// What a surface floating over the board casts, at full strength.
    ///
    /// The alpha here is a *dial*, not a shadow: the three sizes below each
    /// carry their own opacity and multiply it by this one, so a theme sets
    /// how heavy its shadows are with a single number instead of restating
    /// three. Black at full strength on the dark palette, which is what the
    /// three sizes were tuned against; a paper theme turns it down, because a
    /// shadow strong enough to lift a panel off `#14150f` reads as dirt on
    /// `#f3f0e6`.
    pub shadow: Hsla,
}

impl Default for Theme {
    /// The dark one, because that is the palette every one of the notes in
    /// this file was written against.
    fn default() -> Self {
        Self::dark()
    }
}

impl Theme {
    /// The warm dark palette the app has always drawn in.
    pub fn dark() -> Self {
        Self {
            ground: rgb(0x14150f).into(),
            grid: rgba(0xe8e2d000).into(),
            axis: rgba(0xe8e2d022).into(),

            chrome: rgb(0x1b1c15).into(),
            chrome_edge: rgba(0xe8e2d016).into(),

            card: rgb(0x26271e).into(),
            card_edge: rgba(0xe8e2d01f).into(),
            selected_edge: rgb(0xb4553a).into(),

            text: rgb(0xe8e2d0).into(),
            // ~7:1 on the chrome (#1b1c15) — comfortably past the 4.5:1 a
            // sentence needs, with room to spare for the tints a card wash
            // draws it over.
            muted: rgb(0xa8a293).into(),
            // ~4.6:1 on the chrome — past the floor, and no further: a mark
            // this quiet is only ever beside the words that already say what
            // it means.
            tertiary: rgb(0x84806f).into(),
            accent: rgb(0xb4553a).into(),
            // ~4.5:1 on the chrome, where every word in it is drawn.
            accent_text: rgb(0xc6694e).into(),

            note: rgb(0x4a422a).into(),
            image: rgb(0x2c3a3d).into(),
            video: rgb(0x3a2c3d).into(),
            audio: rgb(0x2c3d33).into(),
            link: rgb(0x33334a).into(),
            fence: rgba(0xb4553a14).into(),

            // ≥4.5:1 over every card colour and every tint off the pad, not
            // just over the chrome — a quote and a link are drawn on a card,
            // never on the chrome behind it.
            //
            // Both of these were a shade darker until the test at the bottom
            // of this file went and measured the claim: 4.2:1 and 3.3:1 over
            // the lightest note tint, which is the one background the note
            // above named and neither had been checked against.
            quote: rgb(0xbeb9aa).into(),
            note_link: rgb(0xe4ad99).into(),

            diff_add: rgb(0x6f9455).into(),
            // A shade brighter than `rope_danger`: read as a *line*, not
            // glanced at as a chip, and 3.75:1 on this ground was under the
            // 4.5:1 a sentence needs — see the contrast test at the bottom of
            // this file.
            diff_remove: rgb(0xd66666).into(),

            // Bright enough to read against the ground at a glance and dull
            // enough not to compete with the cards they join — a rope is how
            // two things relate, not a third thing on the board.
            rope_line: rgba(0xe8e2d066).into(),
            rope_accent: rgb(0xb4553a).into(),
            rope_warm: rgb(0xc9913f).into(),
            rope_leaf: rgb(0x6f9455).into(),
            rope_danger: rgb(0xbf4a4a).into(),
            anchor: rgba(0xe8e2d059).into(),
            // The same weight as the marks beside a card, and for the same
            // reason that note gives: this is an offer rather than a control.
            // Stronger than the axes it crosses, which sit at `22` and are
            // permanent furniture you have learned to look past — but a guide
            // appears over somebody's photographs while their hand is down, and
            // anything louder than this stops being feedback and starts being
            // a stripe painted across their board.
            guide: rgba(0xe8e2d059).into(),

            // Written as hex like every other colour here, rather than as the
            // `h`/`s`/`l` triples these were before they moved onto the
            // struct. Not a style preference: a theme file carries hex, so a
            // palette that cannot be *said* in hex is one that comes back
            // subtly different from its own round trip through the format —
            // and a built-in that survives being written out and read back is
            // the only thing that makes a built-in a worked example.
            notes: [
                rgb(0x534a27).into(),
                rgb(0x294828).into(),
                rgb(0x2b4250).into(),
                rgb(0x4d2d3d).into(),
            ],
            swatch_fallback: rgb(0x8c8c8c).into(),
            shadow: rgba(0x000000ff).into(),
        }
    }

    /// The same board on paper.
    ///
    /// Not the dark one inverted, which is the usual way of getting a light
    /// theme and the reason most of them are unpleasant: inverting sends the
    /// warm ground to a cold near-white and every card tint to a fluorescent
    /// version of itself. This is the same *hues*, re-chosen at the other end
    /// of the lightness range — a warm paper rather than white, cards a shade
    /// lighter than the paper rather than darker, and an accent taken down far
    /// enough to stay readable as a word on the chrome.
    ///
    /// Every claim the dark palette's notes make about contrast is kept here
    /// and checked by the tests at the bottom of this file, against the light
    /// backgrounds rather than the dark ones — which is the whole reason those
    /// tests exist: a second palette is where a contrast floor stops being
    /// something somebody eyeballed once.
    pub fn light() -> Self {
        Self {
            ground: rgb(0xf3f0e6).into(),
            // The dark theme's grid and axes are the *text* colour at a low
            // alpha; here they are the text colour too, which on paper means
            // ink rather than light.
            grid: rgba(0x2a2b2000).into(),
            axis: rgba(0x2a2b2024).into(),

            chrome: rgb(0xe8e4d6).into(),
            chrome_edge: rgba(0x2a2b201f).into(),

            // Lighter than the ground, not darker. A card is a piece of paper
            // laid *on* the desk, and the shadow underneath is what says so.
            card: rgb(0xfcfaf3).into(),
            card_edge: rgba(0x2a2b2024).into(),
            selected_edge: rgb(0xa8482a).into(),

            text: rgb(0x24251c).into(),
            // ~5.5:1 on the chrome, and past 4.5:1 on every card tint below —
            // the same promise the dark palette's `muted` makes, measured
            // against this palette's backgrounds.
            muted: rgb(0x5c5a4d).into(),
            // Under the floor on purpose, exactly as the dark one is: this is
            // the colour for marks that repeat the words beside them.
            tertiary: rgb(0x807d6d).into(),
            // Darker than the dark theme's accent, which is what it takes for
            // the same burnt orange to still be a readable *word* on paper:
            // #b4553a is 3.5:1 on a paper chrome, and a border that survives
            // that is still a word that does not.
            accent: rgb(0xa8482a).into(),
            // ~4.6:1 on the chrome. Dark enough already, unlike the dark
            // palette's, so the fill and the word are the same colour here.
            accent_text: rgb(0xa8482a).into(),

            note: rgb(0xf0e6c4).into(),
            image: rgb(0xd6e6e9).into(),
            video: rgb(0xe9d9ec).into(),
            audio: rgb(0xd4ead9).into(),
            link: rgb(0xdcdcf0).into(),
            fence: rgba(0xa8482a14).into(),

            quote: rgb(0x5a584a).into(),
            note_link: rgb(0x9c4526).into(),

            diff_add: rgb(0x466b2e).into(),
            diff_remove: rgb(0xa62f2f).into(),

            rope_line: rgba(0x2a2b2066).into(),
            rope_accent: rgb(0xa8482a).into(),
            rope_warm: rgb(0x8a5e10).into(),
            rope_leaf: rgb(0x466b2e).into(),
            rope_danger: rgb(0xa62f2f).into(),
            anchor: rgba(0x2a2b2059).into(),
            guide: rgba(0x2a2b2059).into(),

            // The four off the pad again, at the other end of the range: the
            // same hues, desaturated less because a pale wash needs the
            // saturation to still read as a colour at all.
            notes: [
                rgb(0xeee5c4).into(),
                rgb(0xceeacd).into(),
                rgb(0xd0e2ec).into(),
                rgb(0xeed3e0).into(),
            ],
            swatch_fallback: rgb(0x9e9e9e).into(),
            // Just under half the dark theme's weight. The three sizes below
            // were tuned to hold a panel off a near-black ground; at full
            // strength on paper the same numbers read as smudges.
            shadow: rgba(0x00000073).into(),
        }
    }
}

/// How many tints a note cycles through. The format's number, not this app's.
pub const NOTE_TINT_COUNT: u32 = 4;

/// A theme file's `style` object, laid over a palette that is already whole.
///
/// This is the entire loader, and it is six lines because [`Theme`] is
/// `Serialize` *and* `Deserialize`: the base is written out to a map of hex
/// strings, the file's keys are laid on top of it, and the result is read back
/// as a `Theme`. There is no second list of field names anywhere — adding a
/// colour to the struct adds it to the file format, and no other line of this
/// module has to hear about it.
///
/// **Every key is optional.** A file naming three colours gets those three and
/// the base for the other thirty, which is what makes a theme somebody can
/// actually write by hand: the alternative is a format where a person tweaking
/// an accent has to restate the whole palette and gets a black board when they
/// miss one.
///
/// **Keys this build does not know are ignored**, and deliberately not an
/// error — the same bargain `prefs.rs` and `mbrd-core` make with their own
/// formats. A theme written for a later build that names a colour this one has
/// not got should draw in every colour it *does* share, not refuse.
///
/// A value that is not a colour — `"grue"`, a number, an object — is `None`
/// rather than a palette with a hole in it. Blunt on purpose, twice over: a
/// half-applied theme is a board where four things moved and thirty did not,
/// which is harder to diagnose than one that plainly did not take; and a
/// `None` is something the caller can *count*, which is how a typo in
/// somebody's theme file ends up said out loud on the settings page instead of
/// silently drawing the palette they were trying to change.
pub fn overlay(base: Theme, style: &Map<String, Value>) -> Option<Theme> {
    let Ok(Value::Object(mut out)) = serde_json::to_value(base) else {
        return None;
    };
    for (key, value) in style {
        if out.contains_key(key) {
            out.insert(key.clone(), value.clone());
        }
    }
    serde_json::from_value(Value::Object(out)).ok()
}

/// The corner radii, named rather than picked fresh at every call site.
///
/// Four numbers rather than the six ad-hoc ones they replace, and nested
/// *concentrically* where one shape sits inside another: a row inset six
/// pixels into a panel takes the panel's radius less the inset, not a radius
/// of its own guessed to look about right. A palette row and a switcher row
/// both being [`RADIUS_SM`] inside a [`RADIUS_LG`] panel is that arithmetic
/// done once rather than eyeballed twice.
pub const RADIUS_XS: f32 = 4.0;
pub const RADIUS_SM: f32 = 6.0;
pub const RADIUS_MD: f32 = 8.0;
pub const RADIUS_LG: f32 = 12.0;

/// A shadow, tuned for the ground this app draws on rather than borrowed from
/// Tailwind's default scale.
///
/// Tailwind's shadows are ten-percent black, which is arithmetic that assumes
/// a white page underneath and is close to arithmetically invisible over
/// `#14150f` — every floating surface in this app was being held off the
/// canvas by its one-pixel hairline alone. Three sizes rather than one,
/// because a twenty-six pixel tooltip and a five-hundred-and-sixty pixel
/// modal are not the same weight of thing hanging over the board, and casting
/// them the same shadow said they were.
///
/// Methods on the theme rather than free functions, which is what they were
/// until a second palette existed. The three sizes are still the same three
/// shapes on every theme — that is the point of naming them — but *how heavy*
/// they are is a property of what they fall on, and a paper theme that had no
/// say in it got the near-black arithmetic above drawn straight onto its
/// ground. The size decides the geometry; [`Theme::shadow`] decides the
/// weight.
impl Theme {
    fn cast(&self, y: f32, blur: f32, spread: f32, alpha: f32) -> Vec<BoxShadow> {
        vec![BoxShadow {
            color: Hsla { a: self.shadow.a * alpha, ..self.shadow },
            offset: point(px(0.0), px(y)),
            blur_radius: px(blur),
            spread_radius: px(spread),
        }]
    }

    /// The tooltip's shadow — light, because a tip is gone half a second after
    /// it arrived and does not need to argue for its place above the board.
    pub fn shadow_small(&self) -> Vec<BoxShadow> {
        self.cast(2.0, 8.0, -2.0, 0.45)
    }

    /// The menu's shadow, and the tool strip's — chrome that sits over the
    /// board for as long as somebody is using it, and wants to look like it is
    /// a shade closer to them than the cards underneath.
    pub fn shadow_medium(&self) -> Vec<BoxShadow> {
        self.cast(6.0, 20.0, -4.0, 0.55)
    }

    /// The palette's shadow, the switcher's, the loader's — the surfaces heavy
    /// enough to have put a scrim behind them already, and that want to read as
    /// genuinely lifted off the board rather than merely drawn on top of it.
    pub fn shadow_large(&self) -> Vec<BoxShadow> {
        self.cast(16.0, 48.0, -8.0, 0.60)
    }
}

/// Digits that hold their width as they change.
///
/// Without this every `1` in a reading is narrower than every `0` beside it,
/// so a number that is not otherwise moving still shivers sideways as its
/// digits turn over — the zoom percentage in the status bar and the elapsed
/// time on a card that plays are the app's only two live numbers, and both of
/// them are read at a glance precisely because they hold still.
pub fn numeric() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".to_string(), 1)]))
}

/// A card's tint, where it has one in range.
fn tint_of(item: &mbrd_core::Item) -> Option<u32> {
    let n = item.meta.get("tint")?.as_u64()?;
    (n >= 1).then_some(n as u32)
}

/// `#rrggbb` or `#rgb` to a colour, or `None` for anything else.
///
/// The format is specific that a swatch's hex is held to six digits and that
/// `#rgb` is folded out to the long form. Doing the folding here means a board
/// written by a build that stored the short form still draws, without this one
/// rewriting somebody's file to say so.
pub fn from_hex(text: &str) -> Option<Hsla> {
    let digits = text.strip_prefix('#').unwrap_or(text);
    if !digits.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let long = match digits.len() {
        3 => digits.chars().flat_map(|c| [c, c]).collect::<String>(),
        6 => digits.to_string(),
        _ => return None,
    };
    let value = u32::from_str_radix(&long, 16).ok()?;
    Some(rgb(value).into())
}

impl Theme {
    /// The grid's dots, at an alpha that fades out as the board is zoomed away
    /// from. A grid that stays crisp at 10% stops being a grid and becomes a
    /// texture, and the original solves this the same way.
    pub fn grid_at(&self, zoom: f32) -> Hsla {
        let mut c = self.grid;
        c.a = (0.20 * ((zoom - 0.15) / 0.55).clamp(0.0, 1.0)).max(0.0);
        c
    }

    /// The colour of one particular card.
    ///
    /// The type is the usual answer, but two kinds of card carry their own and
    /// the format is specific about where: a swatch's `meta.hex` *is* the card,
    /// and a note or a sticker's `meta.tint` is which one off the pad it was
    /// torn from. Both are read here rather than at the painter, so there is
    /// one place that knows a card can override its type's colour.
    pub fn colour_of(&self, item: &mbrd_core::Item) -> Hsla {
        use mbrd_core::ItemType as T;
        match item.kind {
            // A swatch with no readable colour falls back to grey rather than
            // to the generic card, because a grey swatch is still a swatch and
            // a card-coloured one looks like a card that failed to load.
            T::Swatch => item
                .meta
                .get("hex")
                .and_then(serde_json::Value::as_str)
                .and_then(from_hex)
                .unwrap_or(self.swatch_fallback),
            T::Note | T::Text | T::Sticker => match tint_of(item) {
                Some(n) => self.note_tint(n),
                None => self.card_for(&item.kind),
            },
            _ => self.card_for(&item.kind),
        }
    }

    /// A mark laid *on* a card, in whichever of black and white can be seen
    /// against it.
    ///
    /// Not [`Self::text`], which is the one colour the theme sets words in and
    /// is right everywhere the background is the theme's own. A card is not:
    /// a swatch is whatever hex somebody typed and a note is one of four tints
    /// that differ between the light and dark palettes, so a badge drawn in
    /// the theme's ink is a badge that vanishes on some of them. The lock is
    /// the one mark this is for — see `Command::ToggleLock`.
    ///
    /// Lightness alone rather than a luminance formula, because the answer is
    /// one of two and the cases where the two disagree are the cases where
    /// both are legible anyway.
    pub fn ink_on(&self, fill: Hsla) -> Hsla {
        if fill.l > 0.55 {
            gpui::hsla(0.0, 0.0, 0.0, 1.0)
        } else {
            gpui::hsla(0.0, 0.0, 1.0, 1.0)
        }
    }

    /// One of the four colours off the note pad, numbered from one.
    ///
    /// The original's `--note-1..4`. Out of range wraps rather than falling
    /// back, so a sticker's 1–8 lands on *some* colour here instead of on the
    /// plain card — this build draws no sticker shapes, and a tinted rectangle
    /// is a better placeholder than an untinted one.
    pub fn note_tint(&self, n: u32) -> Hsla {
        self.notes[(n.max(1) as usize - 1) % self.notes.len()]
    }

    /// The tint for a card of this type.
    pub fn card_for(&self, kind: &mbrd_core::ItemType) -> Hsla {
        use mbrd_core::ItemType as T;
        match kind {
            T::Note | T::Text => self.note,
            T::Image => self.image,
            T::Video => self.video,
            T::Audio => self.audio,
            T::Link => self.link,
            T::Fence => self.fence,
            // Everything else — including a type this build has never heard
            // of — draws as a plain card. That is the format's rule, not a
            // fallback: an unknown type must show up as *something* named.
            _ => self.card,
        }
    }

    /// The colour a connection of this name draws in.
    ///
    /// The one place a [`ConnColor`] becomes something a stroke can take. The
    /// format stores a *name* precisely so that this mapping can change — in
    /// the original, a string that reached a stroke was a string that reached
    /// the CSSOM — and keeping the translation here rather than at the painter
    /// is what leaves the door open to a palette derived from the board.
    pub fn rope_for(&self, colour: mbrd_core::model::ConnColor) -> Hsla {
        use mbrd_core::model::ConnColor as C;
        match colour {
            C::Line => self.rope_line,
            C::Accent => self.rope_accent,
            C::Warm => self.rope_warm,
            C::Leaf => self.rope_leaf,
            C::Danger => self.rope_danger,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mbrd_core::{Item, ItemType};

    /// The relative luminance of a colour, per WCAG.
    ///
    /// Written out rather than borrowed, because the whole point of the two
    /// tests below is to check a claim the doc comments make in prose, and a
    /// claim checked against the same arithmetic that produced it is not
    /// checked at all.
    fn luminance(c: Hsla) -> f32 {
        let rgba = gpui::Rgba::from(c);
        let channel = |v: f32| {
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(rgba.r) + 0.7152 * channel(rgba.g) + 0.0722 * channel(rgba.b)
    }

    /// How far apart two colours are, 1.0 being identical and 21.0 being black
    /// on white.
    fn contrast(fg: Hsla, bg: Hsla) -> f32 {
        let (a, b) = (luminance(fg), luminance(bg));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    /// Everything a *card* is drawn in, which is where a note's own words
    /// land. Not the chrome: nothing on the chrome is written in `quote` or
    /// `note_link`, and folding the two sets together would hold each colour
    /// to backgrounds it never meets.
    fn cards(t: &Theme) -> Vec<Hsla> {
        let mut all = vec![t.card, t.note, t.image, t.video, t.audio, t.link];
        all.extend(t.notes);
        all
    }

    fn swatch(hex: Option<&str>) -> Item {
        let mut item = Item::new("s", ItemType::Swatch);
        if let Some(hex) = hex {
            item.meta.insert("hex".into(), serde_json::json!(hex));
        }
        item
    }

    #[test]
    fn a_swatch_draws_as_the_colour_it_names() {
        let theme = Theme::default();
        assert_eq!(theme.colour_of(&swatch(Some("#ff0000"))), rgb(0xff0000).into());
        // The short form folds out to the long one rather than being refused.
        assert_eq!(theme.colour_of(&swatch(Some("#f00"))), rgb(0xff0000).into());
    }

    #[test]
    fn a_swatch_with_nothing_readable_on_it_is_grey_rather_than_a_card() {
        let theme = Theme::default();
        for bad in [None, Some(""), Some("#12345"), Some("rebeccapurple"), Some("#gggggg")] {
            assert_eq!(theme.colour_of(&swatch(bad)), theme.swatch_fallback, "{bad:?}");
        }
    }

    #[test]
    fn a_note_wears_the_tint_it_was_torn_off_with() {
        let theme = Theme::default();
        let mut note = Item::new("n", ItemType::Note);
        assert_eq!(theme.colour_of(&note), theme.note, "no tint means the plain pad");
        for n in 1..=NOTE_TINT_COUNT {
            note.meta.insert("tint".into(), serde_json::json!(n));
            assert_eq!(theme.colour_of(&note), theme.note_tint(n));
        }
    }

    #[test]
    fn a_tint_from_a_build_with_more_of_them_still_lands_on_a_colour() {
        // A sticker's range is 1 to 8 and this build has four notes' worth, so
        // the number wraps rather than falling back to an untinted card.
        let theme = Theme::default();
        let mut sticker = Item::new("s", ItemType::Sticker);
        sticker.meta.insert("tint".into(), serde_json::json!(7));
        assert_eq!(theme.colour_of(&sticker), theme.note_tint(3));
        // And nonsense falls back rather than panicking on an index.
        sticker.meta.insert("tint".into(), serde_json::json!(0));
        assert_eq!(theme.colour_of(&sticker), theme.card_for(&ItemType::Sticker));
        sticker.meta.insert("tint".into(), serde_json::json!("blue"));
        assert_eq!(theme.colour_of(&sticker), theme.card_for(&ItemType::Sticker));
    }

    #[test]
    fn every_word_written_on_a_card_can_be_read_on_every_card() {
        // The claim the notes beside `quote` and `note_link` make, measured
        // rather than eyeballed. It is measured *here*, in a test, because
        // that note had been wrong about two colours for as long as it had
        // existed — it named the darkest tint and neither colour had ever been
        // checked against the lightest one.
        for (name, theme) in [("dark", Theme::dark()), ("light", Theme::light())] {
            for surface in cards(&theme) {
                for (what, colour) in
                    [("text", theme.text), ("quote", theme.quote), ("link", theme.note_link)]
                {
                    let ratio = contrast(colour, surface);
                    assert!(
                        ratio >= 4.5,
                        "{name}: {what} is {ratio:.2}:1 on {surface:?}, under the 4.5:1 a \
                         sentence needs",
                    );
                }
            }
        }
    }

    #[test]
    fn a_diffs_added_and_removed_lines_can_be_read_on_the_page() {
        // `diff_add` and `diff_remove` are drawn as a whole line of a patch on
        // the page's own ground, not as a chip beside one — so they are held
        // to the same 4.5:1 a sentence needs, not to a decorative bar's looser
        // bar.
        for (name, theme) in [("dark", Theme::dark()), ("light", Theme::light())] {
            for (what, colour) in [("diff_add", theme.diff_add), ("diff_remove", theme.diff_remove)]
            {
                let ratio = contrast(colour, theme.ground);
                assert!(
                    ratio >= 4.5,
                    "{name}: {what} is {ratio:.2}:1 on the ground, under the 4.5:1 a sentence needs",
                );
            }
        }
    }

    #[test]
    fn every_word_written_on_the_chrome_can_be_read_on_the_chrome() {
        // `tertiary` is deliberately absent, and its own note says why: it is
        // for marks that repeat the words beside them, and it is allowed under
        // the floor. Everything here is a word.
        for (name, theme) in [("dark", Theme::dark()), ("light", Theme::light())] {
            for (what, colour) in
                [("text", theme.text), ("muted", theme.muted), ("accent_text", theme.accent_text)]
            {
                let ratio = contrast(colour, theme.chrome);
                assert!(ratio >= 4.5, "{name}: {what} is {ratio:.2}:1 on the chrome");
            }
        }
    }

    #[test]
    fn a_theme_file_that_names_one_colour_gets_the_rest_of_the_palette() {
        // The bargain that makes a theme writable by hand. Somebody changing
        // an accent should not have to restate thirty colours, and missing one
        // should not hand them a black board.
        let style = serde_json::from_str(r##"{ "accent": "#00ff00ff" }"##).unwrap();
        let out = overlay(Theme::dark(), &style).expect("one good colour is a theme");
        assert_eq!(out.accent, rgb(0x00ff00).into());
        assert_eq!(out.ground, Theme::dark().ground, "everything else is the base");
    }

    #[test]
    fn a_theme_file_may_name_a_colour_this_build_has_never_heard_of() {
        // The same promise `prefs.rs` makes about its own file: a theme
        // written for a later build draws in every colour it does share rather
        // than refusing.
        let style =
            serde_json::from_str(r##"{ "accent": "#00ff00ff", "gutter_hover": "#123456" }"##)
                .unwrap();
        let out = overlay(Theme::dark(), &style).expect("an unknown key is not an error");
        assert_eq!(out.accent, rgb(0x00ff00).into());
    }

    #[test]
    fn a_theme_file_that_says_something_that_is_not_a_colour_is_refused_outright() {
        // Blunt on purpose, and *refused* rather than ignored: a `None` here
        // is what `themes.rs` counts, which is what puts "1 theme could not be
        // read" on the settings page instead of quietly handing somebody the
        // palette they were trying to change.
        for bad in [r##"{ "accent": "grue" }"##, r##"{ "accent": 12 }"##, r##"{ "accent": {} }"##] {
            let style = serde_json::from_str(bad).unwrap();
            assert_eq!(overlay(Theme::dark(), &style), None, "{bad}");
        }
    }

    #[test]
    fn a_palette_survives_the_round_trip_through_the_file_format() {
        // The one thing `overlay` assumes and nothing else checks: that a
        // theme written out as hex reads back as itself. If it did not, a file
        // naming *no* keys would still come back a different palette.
        for theme in [Theme::dark(), Theme::light()] {
            let Value::Object(written) = serde_json::to_value(theme).unwrap() else {
                panic!("a theme writes itself as an object")
            };
            assert_eq!(overlay(Theme::light(), &written), Some(theme));
        }
    }
}
