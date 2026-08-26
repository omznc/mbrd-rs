//! The paper catalogue, and the maths that turns a board's `scale` into
//! something that means anything outside the file: a sheet's outline in
//! world units, and a scale bar's nice round length.
//!
//! `BoardSettings::scale` is world units per millimetre — a lens over numbers
//! that were always unitless, per its own doc comment in `model.rs`. Nothing
//! here decodes or draws anything; it only answers "how big, in world units"
//! and "what should the bar beside that answer say".

/// A choice from the catalogue, or none at all — what `Command::Paper` in the
/// app crate carries, and `BoardSettings::paper` stores as the id from
/// [`Self::id`] (`""` for [`Self::NoSheet`]).
///
/// A typed list over the same seven ids `schema::is_paper_id` already
/// whitelists, for exactly the reason `Arrangement` is one over `arrange`'s
/// own strings: a menu built by mapping over [`Self::ALL`] cannot fall behind
/// a sheet the catalogue gains, the way a hand-written submenu could.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaperSize {
    /// No sheet outlined — the value every new board carries.
    NoSheet,
    A3,
    A4,
    A5,
    A6,
    Letter,
    Legal,
    Tabloid,
}

impl PaperSize {
    /// Every value, in menu order — `NoSheet` first because it is what a new
    /// board is already set to, the same reasoning `Arrangement::ALL` gives
    /// for putting `Spiral` first.
    pub const ALL: [Self; 8] = [
        Self::NoSheet,
        Self::A4,
        Self::A3,
        Self::A5,
        Self::A6,
        Self::Letter,
        Self::Legal,
        Self::Tabloid,
    ];

    /// The id `BoardSettings::paper` stores, and [`mm`] reads.
    pub fn id(self) -> &'static str {
        match self {
            Self::NoSheet => "",
            Self::A3 => "a3",
            Self::A4 => "a4",
            Self::A5 => "a5",
            Self::A6 => "a6",
            Self::Letter => "letter",
            Self::Legal => "legal",
            Self::Tabloid => "tabloid",
        }
    }

    /// What the menu row says.
    pub fn label(self) -> &'static str {
        match self {
            Self::NoSheet => "None",
            Self::A3 => "A3",
            Self::A4 => "A4",
            Self::A5 => "A5",
            Self::A6 => "A6",
            Self::Letter => "Letter",
            Self::Legal => "Legal",
            Self::Tabloid => "Tabloid",
        }
    }

    /// An unknown id is `None`, the same way `Arrangement::parse` treats one —
    /// a board carrying an id this build does not know should read as no
    /// sheet rather than fail to open.
    pub fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|p| p.id() == id)
    }
}

/// A sheet's portrait dimensions in millimetres, or `None` for an id the
/// catalogue does not carry. The only ids that reach here are ones
/// `schema::is_paper_id` has already accepted, but a stray one is a sheet with
/// no size rather than a guessed one.
pub fn mm(id: &str) -> Option<(f32, f32)> {
    match id {
        "a3" => Some((297.0, 420.0)),
        "a4" => Some((210.0, 297.0)),
        "a5" => Some((148.0, 210.0)),
        "a6" => Some((105.0, 148.0)),
        "letter" => Some((215.9, 279.4)),
        "legal" => Some((215.9, 355.6)),
        "tabloid" => Some((279.4, 431.8)),
        _ => None,
    }
}

/// A sheet's outline in world units, centred on the origin — the width and
/// height, not the corners, since every caller so far wants a rectangle
/// straddling `(0, 0)` rather than one somewhere else. `None` for no sheet, an
/// unrecognised id, or a `scale` that could not have come from a real board
/// (non-finite or not positive — see `schema::normalize`'s own guard on it).
pub fn outline(id: &str, landscape: bool, scale: f32) -> Option<(f32, f32)> {
    if !(scale.is_finite() && scale > 0.0) {
        return None;
    }
    let (w, h) = mm(id)?;
    let (w, h) = if landscape { (h, w) } else { (w, h) };
    Some((w * scale, h * scale))
}

/// One rung of a 1-2-5 sequence at a given power of ten — the progression
/// every ruler and map scale bar uses, because a person reading it can
/// multiply any of the three by two or five in their head and nothing else
/// on the sequence asks them to.
fn nice_lengths(unit: f32) -> impl Iterator<Item = f32> {
    (-3..12).flat_map(move |n| {
        let base = unit * 10f32.powi(n);
        [base, base * 2.0, base * 5.0]
    })
}

/// A scale bar's world length and label, for a screen bar aimed at
/// `target_px` wide (a caller's own idea of "about this long, no more").
///
/// `scale` is world units per millimetre and `zoom` is screen pixels per
/// world unit — the same two numbers `BoardSettings` and `Viewport` already
/// carry. `units` is `"imperial"` for a sequence of inches, feet and miles;
/// anything else reads as metric millimetres, centimetres and metres, which
/// mirrors `schema::normalize`'s own fold of an unrecognised `units` string
/// down to metric.
///
/// `None` for a `scale`, `zoom` or `target_px` that could not have come from
/// a real board or a real window — a bar with no sensible length is worse
/// than no bar.
pub fn scale_bar(scale: f32, zoom: f32, units: &str, target_px: f32) -> Option<(f32, String)> {
    if !(scale.is_finite() && scale > 0.0) {
        return None;
    }
    if !(zoom.is_finite() && zoom > 0.0) {
        return None;
    }
    if !(target_px.is_finite() && target_px > 0.0) {
        return None;
    }
    let imperial = units == "imperial";
    // World units per real-world base unit: a millimetre for metric, an inch
    // (25.4mm) for imperial.
    let per_base = if imperial { scale * 25.4 } else { scale };

    // The largest nice length whose bar is no wider than the target — a bar
    // that undershoots by a little reads better than one that has to be
    // trimmed back from a round number.
    let mut best: Option<f32> = None;
    for base in nice_lengths(1.0) {
        let px = base * per_base * zoom;
        if px.is_finite() && px <= target_px {
            best = Some(base);
        } else if px.is_finite() {
            break;
        }
    }
    let base = best?;
    Some((base * per_base, label(base, imperial)))
}

fn label(base: f32, imperial: bool) -> String {
    let trim = |v: f32| {
        if (v - v.round()).abs() < 0.001 {
            format!("{}", v.round() as i64)
        } else {
            format!("{v}")
        }
    };
    if imperial {
        if base >= 5280.0 {
            format!("{} mi", trim(base / 5280.0))
        } else if base >= 12.0 {
            format!("{} ft", trim(base / 12.0))
        } else {
            format!("{} in", trim(base))
        }
    } else if base >= 1000.0 {
        format!("{} m", trim(base / 1000.0))
    } else if base >= 10.0 {
        format!("{} cm", trim(base / 10.0))
    } else {
        format!("{} mm", trim(base))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_paper_sizes_id_is_something_the_catalogue_or_schema_recognises() {
        for size in PaperSize::ALL {
            if size == PaperSize::NoSheet {
                assert_eq!(size.id(), "");
            } else {
                assert!(mm(size.id()).is_some(), "{} has no size", size.label());
            }
        }
    }

    #[test]
    fn parsing_a_paper_sizes_own_id_gives_it_back() {
        for size in PaperSize::ALL {
            assert_eq!(PaperSize::parse(size.id()), Some(size));
        }
    }

    #[test]
    fn an_id_no_paper_size_carries_does_not_parse() {
        assert_eq!(PaperSize::parse("a0"), None);
        assert_eq!(PaperSize::parse("nonsense"), None);
    }

    #[test]
    fn every_catalogue_id_gives_a_real_paper_its_own_size() {
        assert_eq!(mm("a4"), Some((210.0, 297.0)));
        assert_eq!(mm("letter"), Some((215.9, 279.4)));
        assert_eq!(mm("tabloid"), Some((279.4, 431.8)));
    }

    #[test]
    fn an_id_the_catalogue_does_not_carry_has_no_size() {
        assert_eq!(mm(""), None);
        assert_eq!(mm("a0"), None);
    }

    #[test]
    fn a4_outlines_at_its_own_size_times_the_boards_scale() {
        let (w, h) = outline("a4", false, 4.0).unwrap();
        assert_eq!(w, 210.0 * 4.0);
        assert_eq!(h, 297.0 * 4.0);
    }

    #[test]
    fn landscape_swaps_the_sheet_rather_than_measuring_a_different_one() {
        let portrait = outline("a4", false, 4.0).unwrap();
        let landscape = outline("a4", true, 4.0).unwrap();
        assert_eq!(landscape, (portrait.1, portrait.0));
    }

    #[test]
    fn no_sheet_and_no_size_both_outline_to_nothing() {
        assert_eq!(outline("", false, 4.0), None);
        assert_eq!(outline("a4", false, 0.0), None);
        assert_eq!(outline("a4", false, -1.0), None);
        assert_eq!(outline("a4", false, f32::NAN), None);
    }

    #[test]
    fn a_scale_bar_is_never_wider_than_what_was_asked_for() {
        // Scanned rather than picked by hand: every zoom in a wide, ordinary
        // range should hand back a bar that fits, because a bar that has to
        // be trimmed after the fact is not what "nice" means here.
        for step in 0..40 {
            let zoom = 0.05 * 1.4f32.powi(step);
            let (world, _) = scale_bar(4.0, zoom, "metric", 120.0).unwrap();
            let px = world * zoom;
            assert!(px <= 120.0 + 0.01, "zoom={zoom} px={px}");
        }
    }

    #[test]
    fn a_metric_bar_reads_in_the_unit_its_size_deserves() {
        // 4 world units per millimetre. At `zoom = 5.0` the largest bar under
        // 120px is 5mm (100px) — the next rung, 10mm, would be 200px.
        let (_, label) = scale_bar(4.0, 5.0, "metric", 120.0).unwrap();
        assert_eq!(label, "5 mm");

        // Zoomed out ten times over, the same board reaches centimetres.
        let (_, label) = scale_bar(4.0, 0.5, "metric", 120.0).unwrap();
        assert_eq!(label, "5 cm");
    }

    #[test]
    fn an_imperial_bar_reads_in_inches_feet_or_miles_as_it_grows() {
        // A `scale` of one world unit per inch, so `zoom` alone decides how
        // many inches fit in the target.
        let inch_scale = 1.0 / 25.4;
        let (_, small) = scale_bar(inch_scale, 10.0, "imperial", 120.0).unwrap();
        assert_eq!(small, "10 in");

        let (_, big) = scale_bar(inch_scale, 2.0, "imperial", 120.0).unwrap();
        assert!(big.ends_with("ft"), "{big}");
    }

    #[test]
    fn an_unusable_scale_zoom_or_target_gives_no_bar() {
        assert_eq!(scale_bar(0.0, 1.0, "metric", 120.0), None);
        assert_eq!(scale_bar(4.0, 0.0, "metric", 120.0), None);
        assert_eq!(scale_bar(4.0, 1.0, "metric", 0.0), None);
        assert_eq!(scale_bar(f32::NAN, 1.0, "metric", 120.0), None);
    }

    #[test]
    fn an_unrecognised_units_string_reads_as_metric() {
        let (_, metric) = scale_bar(4.0, 1.0 / 4.0, "metric", 120.0).unwrap();
        let (_, other) = scale_bar(4.0, 1.0 / 4.0, "nonsense", 120.0).unwrap();
        assert_eq!(metric, other);
    }
}
