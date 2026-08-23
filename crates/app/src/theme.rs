//! The one place a colour is written down.
//!
//! The original recolours its whole interface from the pictures on the board
//! once there are three of them, with four ready-made palettes underneath. None
//! of that is here yet, and this module is shaped so that it can be: everything
//! that draws asks the theme rather than naming a colour, so the day a palette
//! is derived from the board there is one struct to fill in instead of a grep.

use gpui::{rgb, rgba, Hsla};

/// `Copy` because it is a bag of colours that every drawing path wants a
/// private look at, and passing it by reference would tie the painter to the
/// view it came from — which is exactly the borrow a `'static` paint closure
/// cannot hold.
#[derive(Debug, Clone, Copy, PartialEq)]
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
    /// Labels, counts, the status bar — anything secondary.
    pub muted: Hsla,
    pub accent: Hsla,

    /// Per-type card tints, so a board reads as a board at a glance rather than
    /// as a wall of identical grey rectangles.
    pub note: Hsla,
    pub image: Hsla,
    pub video: Hsla,
    pub audio: Hsla,
    pub link: Hsla,
    pub fence: Hsla,

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
}

impl Default for Theme {
    fn default() -> Self {
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
            muted: rgba(0xe8e2d077).into(),
            accent: rgb(0xb4553a).into(),

            note: rgb(0x4a422a).into(),
            image: rgb(0x2c3a3d).into(),
            video: rgb(0x3a2c3d).into(),
            audio: rgb(0x2c3d33).into(),
            link: rgb(0x33334a).into(),
            fence: rgba(0xb4553a14).into(),

            // Bright enough to read against the ground at a glance and dull
            // enough not to compete with the cards they join — a rope is how
            // two things relate, not a third thing on the board.
            rope_line: rgba(0xe8e2d066).into(),
            rope_accent: rgb(0xb4553a).into(),
            rope_warm: rgb(0xc9913f).into(),
            rope_leaf: rgb(0x6f9455).into(),
            rope_danger: rgb(0xbf4a4a).into(),
            anchor: rgba(0xe8e2d059).into(),
        }
    }
}

/// The note pad, `--note-1..4`. Muted, because a note is something to read.
const NOTE_TINTS: [Hsla; 4] = [
    Hsla { h: 0.13, s: 0.36, l: 0.24, a: 1.0 },
    Hsla { h: 0.33, s: 0.28, l: 0.22, a: 1.0 },
    Hsla { h: 0.56, s: 0.30, l: 0.24, a: 1.0 },
    Hsla { h: 0.92, s: 0.26, l: 0.24, a: 1.0 },
];

/// How many tints a note cycles through. The format's number, not this app's.
pub const NOTE_TINT_COUNT: u32 = 4;

/// What a swatch draws as when its `hex` is missing or is not a colour.
const SWATCH_FALLBACK: Hsla = Hsla { h: 0.0, s: 0.0, l: 0.55, a: 1.0 };

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
                .unwrap_or(SWATCH_FALLBACK),
            T::Note | T::Text | T::Sticker => match tint_of(item) {
                Some(n) => self.note_tint(n),
                None => self.card_for(&item.kind),
            },
            _ => self.card_for(&item.kind),
        }
    }

    /// One of the four colours off the note pad, numbered from one.
    ///
    /// The original's `--note-1..4`. Out of range wraps rather than falling
    /// back, so a sticker's 1–8 lands on *some* colour here instead of on the
    /// plain card — this build draws no sticker shapes, and a tinted rectangle
    /// is a better placeholder than an untinted one.
    pub fn note_tint(&self, n: u32) -> Hsla {
        NOTE_TINTS[(n.max(1) as usize - 1) % NOTE_TINTS.len()]
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
            assert_eq!(theme.colour_of(&swatch(bad)), SWATCH_FALLBACK, "{bad:?}");
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
}
