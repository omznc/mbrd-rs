//! How wide text is.
//!
//! The wrap used to be a division. Every character was assumed to be half the
//! font size wide — `AVERAGE_ADVANCE` in `board_view.rs`, now [`Estimate`] —
//! and a card `w` pixels across therefore held `w / (size * 0.5)` of them,
//! whatever they were. That is right for a monospaced face and wrong for every
//! other one: `WWWW` and `iiii` are four characters each and nothing like the
//! same width, so a line of wide letters ran off the card and a line of narrow
//! ones broke well short of the edge, with the padding left unfilled.
//!
//! So the question a wrap asks is not "how many characters fit" but "how wide
//! is this one", and that is the only thing in here.
//!
//! ## Why a trait and not a function
//!
//! Two callers cannot answer it the same way. The painter has a window and a
//! resolved face and should measure; [`fitted_height`](crate::board_view) runs
//! from inside a `during` closure on every keystroke, with no window anywhere
//! near it, and the unit tests under `markdown.rs` and `editor.rs` have no
//! font at all — a test that asserted where a line broke would otherwise be
//! asserting which fonts the machine running it happens to have installed.
//!
//! [`Estimate`] is what those get, and it is deliberately the *old* arithmetic
//! rather than a worse one: the same answer this file was written to stop
//! shipping, kept where it is the only answer available and named so it cannot
//! be mistaken for a measurement.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{px, Font, FontId, TextSystem};
use std::sync::Arc;

/// How wide a character is.
///
/// `&self` rather than `&mut self`, with the cache behind a [`RefCell`], so a
/// measurer can be handed to a closure that has already borrowed half the
/// board — which is exactly what `write_to` does on every keystroke.
pub trait Advance {
    /// The width of `c` in pixels, set at `size`.
    fn of(&self, c: char, size: f32) -> f32;

    /// The width of a whole run, which is the sum and is worth not writing
    /// out at all six call sites.
    fn width(&self, text: &str, size: f32) -> f32 {
        text.chars().map(|c| self.of(c, size)).sum()
    }
}

/// Every character the same width.
///
/// What there is before a face has been resolved, and what the tests measure
/// against. See the module doc for why this still exists.
#[derive(Debug, Clone, Copy)]
pub struct Estimate {
    /// Width of one character as a fraction of the font size.
    per_em: f32,
}

impl Estimate {
    /// What a proportional face averages out at, near enough. The number this
    /// whole module exists to stop being the *only* answer.
    pub const AVERAGE: f32 = 0.5;

    pub fn average() -> Self {
        Self { per_em: Self::AVERAGE }
    }

    /// One character to one unit of width.
    ///
    /// For tests, so that "forty" in a test means forty characters and the
    /// assertion is about where the words broke rather than about arithmetic.
    #[cfg(test)]
    pub fn columns() -> Self {
        Self { per_em: 1.0 }
    }

    /// A width somebody has already measured, as a fraction of the font size.
    ///
    /// For a *monospaced* face, where this is not an estimate at all: every
    /// character really is the same width, so one `shape_line` of a single
    /// character answers for all of them. That is what the open page does —
    /// see `opened.rs`, which is set in a fixed-width face on purpose.
    pub fn per_em(per_em: f32) -> Self {
        Self { per_em: per_em.max(0.0) }
    }
}

impl Advance for Estimate {
    fn of(&self, _c: char, size: f32) -> f32 {
        size * self.per_em
    }
}

/// The size a character is measured at, once, before being scaled to whatever
/// size it is actually being set in.
///
/// Advances are linear in the font size — they come out of the face's own
/// units-per-em — so one measurement serves every zoom, and the cache is keyed
/// on the character alone rather than on the pair. Large enough that the
/// rounding in the platform's own answer is a rounding error here too.
const REFERENCE: f32 = 64.0;

/// A real face, asked once per character and then remembered.
pub struct Face {
    text_system: Arc<TextSystem>,
    font: FontId,
    /// Advance at [`REFERENCE`] pixels, per character seen so far.
    seen: RefCell<HashMap<char, f32>>,
    /// What to say about a character the platform will not measure.
    fallback: Estimate,
}

impl Face {
    pub fn new(text_system: Arc<TextSystem>, font: &Font) -> Self {
        let id = text_system.resolve_font(font);
        Self {
            text_system,
            font: id,
            seen: RefCell::new(HashMap::new()),
            fallback: Estimate::average(),
        }
    }
}

impl Advance for Face {
    fn of(&self, c: char, size: f32) -> f32 {
        // A newline has no width and is not something a face is asked about.
        // It reaches here through a note's own text; measuring it would be a
        // platform error per line rather than an answer.
        if c == '\n' || c == '\r' {
            return 0.0;
        }
        if let Some(&at_reference) = self.seen.borrow().get(&c) {
            return at_reference * size / REFERENCE;
        }
        let at_reference = self
            .text_system
            .advance(self.font, px(REFERENCE), c)
            .map(|size| f32::from(size.width))
            // A character this face has no glyph for and no fallback for. The
            // estimate is a worse answer than a measurement and a better one
            // than zero, which would stack the rest of the line on top of it.
            .unwrap_or_else(|_| self.fallback.of(c, REFERENCE));
        // Some faces report nothing for a control character, and a zero here
        // would let an unbounded run of them fit on one line.
        let at_reference = if at_reference.is_finite() && at_reference > 0.0 {
            at_reference
        } else {
            self.fallback.of(c, REFERENCE)
        };
        self.seen.borrow_mut().insert(c, at_reference);
        at_reference * size / REFERENCE
    }
}

/// A [`Face`] that can be held rather than borrowed.
///
/// The board needs one of these inside `during` and `edit` closures, which
/// have already taken a mutable borrow of the document — so what goes in has
/// to be owned, and cheap enough to clone once per keystroke. The [`Rc`] is
/// because a face's whole value is the cache it accumulates, and a copy per
/// keystroke would be a cache that never got warm.
#[derive(Clone)]
pub struct Measure(Rc<Face>);

impl Measure {
    pub fn new(text_system: Arc<TextSystem>, font: &Font) -> Self {
        Measure(Rc::new(Face::new(text_system, font)))
    }
}

impl Advance for Measure {
    fn of(&self, c: char, size: f32) -> f32 {
        self.0.of(c, size)
    }
}

/// A face that is deliberately uneven: `W` an em wide, `i` a fifth of one,
/// everything else half.
///
/// The fixture the wrap tests need, because the whole claim being made is that
/// a row of `W`s breaks sooner than a row of `i`s — which is exactly what
/// [`Estimate`] cannot tell you and what shipping it as the only answer got
/// wrong. Not a real face: a real one would make these tests assertions about
/// which fonts the machine running them has.
#[cfg(test)]
pub struct Ragged;

#[cfg(test)]
impl Advance for Ragged {
    fn of(&self, c: char, size: f32) -> f32 {
        size * match c {
            'W' => 1.0,
            'i' => 0.2,
            _ => 0.5,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_estimate_is_linear_in_the_size() {
        let e = Estimate::average();
        assert_eq!(e.of('x', 10.0), 5.0);
        assert_eq!(e.of('W', 10.0), 5.0, "it cannot tell them apart, which is the point");
        assert_eq!(e.of('x', 20.0), 10.0);
    }

    #[test]
    fn a_column_estimate_makes_one_character_one_unit() {
        let e = Estimate::columns();
        assert_eq!(e.width("hello", 1.0), 5.0);
    }

    #[test]
    fn a_run_is_the_sum_of_its_characters() {
        let e = Estimate::average();
        assert_eq!(e.width("abc", 10.0), 15.0);
        assert_eq!(e.width("", 10.0), 0.0);
    }
}
