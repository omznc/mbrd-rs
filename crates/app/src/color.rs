//! The four things a colour used to do by itself.
//!
//! `gpui::Hsla` is `palette::Hsla` now — the fork handed its colour type to a
//! colour library rather than keeping its own — and palette is a better piece
//! of arithmetic than the twenty lines it replaced. But it is a *library's*
//! type, so the three conveniences gpui had bolted onto its own struct went
//! with it, and one of them was load-bearing for a file on somebody's disk.
//! This module is those, and only those: a hex literal that lands as an
//! [`Hsla`], a way to dim one, where it sits on the wheel, and the `#rrggbbaa`
//! reading and writing that `THEMES.md` promises.
//!
//! Nothing here is a colour. `theme.rs` is still the one place a colour is
//! written down; this is the grammar that file is written in.

use gpui::{hsla_to_rgba, rgb_to_hsla, Hsla, Rgba};
use serde::{Deserialize, Deserializer, Serializer};

/// A hex literal as a colour, the way `rgb(0x14150f)` always read here.
///
/// gpui's own `rgb` stops at [`Rgba`] and there is no longer a `From` between
/// the two, so every call site in `theme.rs` would otherwise end in the same
/// conversion. It ends here instead.
pub fn rgb(hex: u32) -> Hsla {
    rgb_to_hsla(gpui::rgb(hex))
}

/// The same, with the alpha byte read rather than assumed — `0xe8e2d016` is a
/// hairline, not a cream.
pub fn rgba(hex: u32) -> Hsla {
    rgb_to_hsla(gpui::rgba(hex))
}

/// Where a colour sits on the wheel, as the fraction of a turn that
/// [`gpui::hsla`] takes.
///
/// palette counts hue in degrees and wraps it, which is the right unit for a
/// colour library and the wrong one for the only caller here: `theme.rs`
/// spaces generated note tints evenly around the circle, and evenly spaced in
/// `0.0..1.0` is arithmetic a reader can check.
pub fn wheel(color: Hsla) -> f32 {
    color.hue.into_positive_degrees() / 360.0
}

/// Dimming, which every drawing path in this crate does and none of them
/// should have to spell out.
pub trait Tint {
    /// This colour at `factor` of its own alpha.
    ///
    /// *Of its own* — the same multiply gpui did, not an assignment. It is
    /// what lets [`crate::theme::Theme::shadow`] be a dial that the three
    /// shadow sizes multiply their own weight by, and what makes
    /// `accent.opacity(0.35)` a wash of the accent rather than a flat 35%
    /// whatever the theme said.
    fn opacity(self, factor: f32) -> Self;
}

impl Tint for Hsla {
    fn opacity(self, factor: f32) -> Self {
        Hsla { alpha: self.alpha * factor.clamp(0.0, 1.0), ..self }
    }
}

/// `#rrggbbaa` in and out, for a [`Hsla`] field of a theme.
///
/// Applied per field in [`crate::theme::Theme`] rather than inherited from the
/// type, which is the one real cost of the move: palette writes a colour as an
/// object of three numbers, and a theme file is hex strings — it says so in
/// `THEMES.md`, and there are files on disk that were written against that
/// promise. So the format lives here, where it can be tested, instead of being
/// a property of whichever colour crate the framework happens to use.
///
/// Reading takes `#rgb`, `#rgba`, `#rrggbb` and `#rrggbbaa`, and writing
/// always produces the long form — both exactly as gpui's own `Rgba` did, so
/// no theme that loaded before this module existed loads differently after it.
pub mod hex {
    use super::*;

    pub fn serialize<S: Serializer>(color: &Hsla, out: S) -> Result<S::Ok, S::Error> {
        out.serialize_str(&write(*color))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(input: D) -> Result<Hsla, D::Error> {
        let raw = String::deserialize(input)?;
        read(&raw).map_err(serde::de::Error::custom)
    }
}

/// The same, for the pad of four note tints.
///
/// A separate module because `serde(with = …)` names the field's whole type,
/// and `[Hsla; 4]` is not `Hsla`.
pub mod hex_pad {
    use super::*;

    pub fn serialize<S: Serializer>(pad: &[Hsla; 4], out: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = out.serialize_seq(Some(pad.len()))?;
        for color in pad {
            seq.serialize_element(&write(*color))?;
        }
        seq.end()
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(input: D) -> Result<[Hsla; 4], D::Error> {
        let raw = <[String; 4]>::deserialize(input)?;
        let mut pad = [gpui::black(); 4];
        for (slot, text) in pad.iter_mut().zip(&raw) {
            *slot = read(text).map_err(serde::de::Error::custom)?;
        }
        Ok(pad)
    }
}

/// A colour as `#rrggbbaa`.
///
/// Always eight digits, even at full alpha: a theme round-tripped through the
/// settings page and back to disk should not quietly change shape depending on
/// whether one of its colours happened to be opaque.
fn write(color: Hsla) -> String {
    let color = hsla_to_rgba(color);
    let byte = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    let (r, g, b, a) = (byte(color.red), byte(color.green), byte(color.blue), byte(color.alpha));
    format!("#{r:02x}{g:02x}{b:02x}{a:02x}")
}

/// A colour from `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`.
///
/// The error is a sentence rather than a type because nothing matches on it:
/// [`crate::theme::overlay`] turns any failure into `None` — a colour that
/// does not parse is the whole theme declining rather than a palette with a
/// hole in it — and what a person sees is `themes.rs` counting that refusal
/// on the settings page. The sentence is for whoever is reading a serde error
/// while working out why.
fn read(value: &str) -> Result<Hsla, String> {
    const EXPECTED: &str = "expected #rgb, #rgba, #rrggbb or #rrggbbaa";

    let Some(("", digits)) = value.trim().split_once('#') else {
        return Err(format!("invalid colour '{value}': {EXPECTED}"));
    };
    // Byte indices, which is only safe because the length check below has
    // already rejected anything a hex digit is not.
    if !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(format!("invalid colour '{value}': {EXPECTED}"));
    }

    let pair = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).unwrap_or(0);
    // `0xf` means `0xff`, the way the short form does everywhere else.
    let single = |at: usize| {
        let v = u8::from_str_radix(&digits[at..at + 1], 16).unwrap_or(0);
        (v << 4) | v
    };

    let (r, g, b, a) = match digits.len() {
        3 => (single(0), single(1), single(2), 0xff),
        4 => (single(0), single(1), single(2), single(3)),
        6 => (pair(0), pair(2), pair(4), 0xff),
        8 => (pair(0), pair(2), pair(4), pair(6)),
        _ => return Err(format!("invalid colour '{value}': {EXPECTED}")),
    };

    let channel = |v: u8| v as f32 / 255.0;
    Ok(rgb_to_hsla(Rgba::new(channel(r), channel(g), channel(b), channel(a))))
}

#[cfg(test)]
mod tests {
    use gpui::hsla;

    use super::*;

    /// A pair of colours are the same to the eye and to a file: comparing
    /// floats out of two different conversions is comparing rounding, so the
    /// hex both write is the comparison that means anything.
    fn same(a: Hsla, b: Hsla) -> bool {
        write(a) == write(b)
    }

    #[test]
    fn a_hex_literal_survives_the_trip_to_a_string_and_back() {
        let ground = rgb(0x14150f);
        assert_eq!(write(ground), "#14150fff");
        assert!(same(read("#14150f").unwrap(), ground));
    }

    #[test]
    fn the_alpha_byte_is_read_rather_than_assumed() {
        let hairline = rgba(0xe8e2d016);
        assert_eq!(write(hairline), "#e8e2d016");
        assert!(same(read("#e8e2d016").unwrap(), hairline));
    }

    #[test]
    fn the_short_form_doubles_each_digit() {
        assert!(same(read("#abc").unwrap(), read("#aabbccff").unwrap()));
        assert!(same(read("#abcd").unwrap(), read("#aabbccdd").unwrap()));
    }

    #[test]
    fn an_opaque_colour_still_writes_its_alpha() {
        assert_eq!(write(read("#abc").unwrap()), "#aabbccff");
    }

    #[test]
    fn what_is_not_a_colour_is_an_error_and_not_a_guess() {
        for not in ["grue", "", "#", "#ab", "#abcde", "#12345g", "14150f", "#14150f0"] {
            assert!(read(not).is_err(), "{not:?} parsed as a colour");
        }
    }

    #[test]
    fn whitespace_around_a_colour_is_forgiven() {
        assert!(same(read("  #14150f  ").unwrap(), rgb(0x14150f)));
    }

    #[test]
    fn opacity_is_a_fraction_of_the_alpha_a_colour_already_had() {
        let half = rgba(0x00000080);
        assert_eq!(rgb(0x000000).opacity(0.5).alpha, 0.5);
        // A dial on a dial: the shadow weight multiplies, it does not replace.
        assert!((half.opacity(0.5).alpha - half.alpha * 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn a_factor_outside_the_dial_is_clamped_rather_than_wrapped() {
        assert_eq!(rgb(0x000000).opacity(4.0).alpha, 1.0);
        assert_eq!(rgb(0x000000).opacity(-1.0).alpha, 0.0);
    }

    #[test]
    fn the_wheel_reads_back_the_fraction_it_was_given() {
        for turn in [0.0, 0.25, 0.5, 0.75] {
            let color = hsla(turn, 0.5, 0.5, 1.0);
            assert!((wheel(color) - turn).abs() < 1e-4, "{turn} came back as {}", wheel(color));
        }
    }

    #[test]
    fn a_pad_of_four_is_a_list_of_four_strings() {
        let pad = [rgb(0x2a2b20), rgb(0x2b2a24), rgb(0x252b26), rgb(0x2a2528)];
        let written = serde_json::to_value(PadHolder { notes: pad }).unwrap();
        assert_eq!(written["notes"][0], "#2a2b20ff");
        let back: PadHolder = serde_json::from_value(written).unwrap();
        assert!(back.notes.iter().zip(&pad).all(|(a, b)| same(*a, *b)));
    }

    #[derive(serde::Serialize, serde::Deserialize)]
    struct PadHolder {
        #[serde(with = "super::hex_pad")]
        notes: [Hsla; 4],
    }
}
