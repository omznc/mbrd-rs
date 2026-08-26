//! Markdown on a card: as much of it as a note has room for.
//!
//! The reading is not done here. [`mbrd_core::markdown::parse`] turns a note
//! into blocks with every marker already off and every character already
//! carrying its style, and this module answers the *other* question — what
//! those blocks look like on a rectangle a few hundred pixels wide, with a
//! fixed number of body lines to spend.
//!
//! ## What it takes and what it hands back
//!
//! In: the note's text, how wide the card is in pixels, and how many `rows` of
//! body text it has room for — plus a [`crate::metrics::Advance`], which is
//! what says how wide a character is and therefore where a line breaks. Out:
//! [`Line`]s that are already wrapped, already elided if
//! there were more of them than fit, and already broken into [`Span`]s that are
//! each set one way. The painter shapes a run per span and never has to know
//! what an asterisk meant.
//!
//! ## Why the wrapping happens in here
//!
//! Because a style crosses a line break. Wrapping first and parsing per line
//! would end a bold at the wrap, and parsing first and wrapping the plain text
//! afterwards would lose which characters the bold was on. So a block's spans
//! are spread back out to a style *per character*, folded into lines at that
//! granularity, and only then grouped into runs again — which also makes the
//! fold itself ordinary, since a `Vec` of characters wraps the same way a
//! string does.
//!
//! ## A card is a level of detail, not a document
//!
//! Everything the parser can produce arrives here, including the things a card
//! plainly cannot show properly — a table, a heading six deep, a list four
//! levels in. None of them are refused, because a card that silently dropped
//! part of a note would be lying about what the note says. They are *flattened*
//! instead: a table becomes its rows with the columns divided, a deep heading
//! becomes bold body text, nesting becomes indentation. Open the card and the
//! full window sets the same blocks properly. See `opened.rs`.

use crate::metrics::Advance;
use mbrd_core::markdown::{Block, Marker, Run, Table};

pub use mbrd_core::markdown::{Span, Style};

/// How much larger than body text each heading level is set.
///
/// Wider at the top than the ramp this replaces, whose bottom two steps — 1.25
/// and 1.1 — sat close enough together that an H3 read as body text with a hash
/// quietly in front of it rather than as its own level. Flat from the fourth
/// down: a card is not a document, and the difference between an H4 and an H5
/// on something this size is a difference nobody can see. They stay bold, which
/// is what makes them still read as headings.
const HEADINGS: [f32; 6] = [1.6, 1.3, 1.12, 1.0, 1.0, 1.0];

/// The bar in front of a quote, and the space after it.
const QUOTE: &str = "\u{258f} ";

/// What divides two cells of a table on a card. A table is drawn as its rows
/// here — there is no room to align columns on a note — so the divider
/// is what is left of the grid.
const CELL: &str = " \u{2502} ";

/// One line of a note, as it will be drawn.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub spans: Vec<Span>,
    /// A multiplier on the body font size. Headings are larger; everything
    /// else is `1.0`, and the painter multiplies rather than deciding.
    pub scale: f32,
    /// Quotes and rules, which are said quietly.
    pub muted: bool,
}

impl Line {
    /// One line, set the ordinary way. What a card that is not a note gets.
    pub fn plain(text: impl Into<String>) -> Line {
        let text = text.into();
        Line {
            spans: if text.is_empty() {
                Vec::new()
            } else {
                vec![Span::new(text, Style::default())]
            },
            scale: 1.0,
            muted: false,
        }
    }

    /// The characters, with nothing said about how they are set.
    ///
    /// The painter shapes this and measures against it, so the byte offsets in
    /// a span and the byte offsets in here are the same offsets — which is what
    /// lets a caret and a highlight keep working over styled text.
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

/// The whole job: text in, drawable lines out.
///
/// `width` is in pixels and `size` is the body font size in pixels; `rows` is
/// still a count of body-sized lines, because that is a budget rather than a
/// measurement. A heading takes more than one row's worth of height, and is
/// charged for it — see the budget in the loop — so a card does not overflow
/// just because somebody started their note with a `#`.
///
/// `adv` is what makes the wrap a measurement instead of a division; see
/// [`crate::metrics`] for why it is a parameter and not a constant.
pub fn lay_out(text: &str, width: f32, size: f32, rows: usize, adv: &dyn Advance) -> Vec<Line> {
    let width = width.max(1.0);
    let mut pieces = Vec::new();
    flatten(&mbrd_core::markdown::parse(text), &Where::default(), width, size, adv, &mut pieces);

    let mut out: Vec<Line> = Vec::new();
    let mut left = rows as f32;
    let mut clipped = false;

    'outer: for piece in &pieces {
        // A heading's letters are larger, so its own characters are measured
        // at its own size — but its marker and its indent are body-sized, and
        // are measured at `size`.
        let em = size * piece.scale;
        // The room this piece's own lines get, after its bullet or its quote
        // mark. Never nothing: a card narrower than its own marker still has
        // to put a character somewhere, and the painter clips the overflow.
        let head = format!("{}{}", " ".repeat(piece.indent), piece.prefix);
        let room = (width - adv.width(&head, size)).max(em);
        let hang = hang(piece, size, adv);

        for (n, run) in fold(&piece.body, room, em, adv).into_iter().enumerate() {
            // `!out.is_empty()`, so the first line is always drawn however
            // little room there is. Without it a note that opens with a
            // heading — which costs more than one row — came back *empty* the
            // moment the card was down to a single row, while the same note
            // written without the `#` still showed its first line. A card
            // going blank is not a level of detail, it is a card that has lost
            // its contents, and the painter clips what overflows anyway.
            if left < piece.scale && !out.is_empty() {
                clipped = true;
                break 'outer;
            }
            left -= piece.scale;

            // The marker on the first line and an indent under it on the rest,
            // so a bullet that wraps hangs rather than starting back at the
            // margin and reading as a second bullet.
            let lead = if n == 0 { head.clone() } else { hang.clone() };
            let mut spans = Vec::new();
            if !lead.trim().is_empty() || (!lead.is_empty() && n > 0) {
                spans.push(Span::new(lead, piece.marker));
            }
            spans.extend(group(&run));
            out.push(Line { spans, scale: piece.scale, muted: piece.muted });
        }
    }

    // Something was left out, so the last line has to say so. A card that ends
    // mid-sentence with no ellipsis is a card that looks complete and is not.
    if clipped {
        if let Some(last) = out.last_mut() {
            match last.spans.last_mut() {
                Some(span) => span.text.push('\u{2026}'),
                None => last.spans.push(Span::new("\u{2026}", Style::default())),
            }
        }
    }
    // An empty note is one empty line rather than none, so that a card being
    // typed into has somewhere to put its caret.
    if out.is_empty() {
        out.push(Line::plain(""));
    }
    out
}

/// The spaces a wrapped line hangs by: as near the marker's own width as a run
/// of spaces can get.
///
/// Counted rather than assumed. In a proportional face `- ` and two spaces are
/// not the same width, and a bullet whose second line started a few pixels off
/// the first reads as a second bullet rather than as the same one continuing.
fn hang(piece: &Piece, size: f32, adv: &dyn Advance) -> String {
    let space = adv.of(' ', size);
    let marker = adv.width(&piece.prefix, size);
    let wide = if space > 0.0 { (marker / space).round() as usize } else { 0 };
    " ".repeat(piece.indent + wide)
}

// ---------------------------------------------------------------------------
// Flattening
// ---------------------------------------------------------------------------

/// One line's worth of block, once it is known what kind of block it came from.
#[derive(Debug)]
struct Piece {
    /// The bullet, the number, the quote bar. Drawn, and not part of the text.
    prefix: String,
    /// How the marker itself is set, which is not how the words are.
    marker: Style,
    /// Nesting, in body-sized characters.
    indent: usize,
    scale: f32,
    muted: bool,
    body: Vec<(char, Style)>,
}

impl Piece {
    fn new(body: Vec<(char, Style)>, at: &Where) -> Piece {
        Piece {
            prefix: at.prefix.clone(),
            marker: at.marker,
            indent: at.depth * 2,
            scale: 1.0,
            muted: at.muted,
            body,
        }
    }
}

/// Where in the nesting a block is being flattened, which is everything a
/// block needs to know about its ancestors.
///
/// Carried down rather than fixed up afterwards, because a bullet inside a
/// quote wants `▎ • ` — one prefix built on the way in — rather than two
/// prefixes fought over on the way out.
#[derive(Debug, Default, Clone)]
struct Where {
    prefix: String,
    marker: Style,
    depth: usize,
    muted: bool,
}

impl Where {
    /// A level deeper, with whatever marker this level puts in front.
    fn under(&self, prefix: &str, marker: Style) -> Where {
        Where {
            prefix: format!("{}{}", self.prefix, prefix),
            marker,
            depth: self.depth,
            muted: self.muted,
        }
    }

    /// The same place, with nothing more to put in front — what a list item's
    /// second and later blocks get, so a paragraph under a bullet does not
    /// grow a second bullet.
    fn again(&self) -> Where {
        Where { prefix: " ".repeat(self.prefix.chars().count()), ..self.clone() }
    }
}

fn flatten(
    blocks: &[Block],
    at: &Where,
    width: f32,
    size: f32,
    adv: &dyn Advance,
    out: &mut Vec<Piece>,
) {
    for block in blocks {
        match block {
            Block::Gap => out.push(Piece::new(Vec::new(), at)),
            Block::Paragraph(runs) => {
                for run in runs {
                    out.push(Piece::new(spread(run, Style::default()), at));
                }
            }
            Block::Heading { level, runs } => {
                // Bold throughout, whatever the words were already set as: a
                // heading on a card has to read as one at a glance, and the
                // scale alone does not carry the bottom half of the ramp.
                let scale = HEADINGS[(*level as usize).clamp(1, 6) - 1];
                for run in runs {
                    let mut piece =
                        Piece::new(spread(run, Style { bold: true, ..Style::default() }), at);
                    piece.scale = scale;
                    out.push(piece);
                }
            }
            Block::Quote(inner) => {
                let mut under = at.under(QUOTE, at.marker);
                under.muted = true;
                flatten(inner, &under, width, size, adv, out);
            }
            // `ordered` says nothing a card can use: the marker on each entry
            // already carries the number that was typed, and a bullet and a
            // number are drawn the same way here.
            Block::List { items, .. } => {
                for entry in items {
                    let (prefix, marker) = match &entry.marker {
                        Marker::Bullet => {
                            ("\u{2022} ".to_string(), Style { bold: true, ..Style::default() })
                        }
                        Marker::Number(typed) => (format!("{typed}. "), at.marker),
                        Marker::Task(false) => ("\u{2610} ".to_string(), at.marker),
                        Marker::Task(true) => ("\u{2611} ".to_string(), at.marker),
                    };
                    let first = at.under(&prefix, marker);
                    // Only the item's *first* block wears the marker. Everything
                    // else under the same bullet — a second paragraph, a nested
                    // list — is set at the width of the marker instead, which is
                    // what indents it under the words rather than under the dot.
                    // That is also the whole of the nesting: a list four levels
                    // in is four markers' worth of spaces, arrived at by the
                    // same rule each time rather than by counting depth twice.
                    let rest = first.again();
                    for (n, block) in entry.blocks.iter().enumerate() {
                        flatten(
                            std::slice::from_ref(block),
                            if n == 0 { &first } else { &rest },
                            width,
                            size,
                            adv,
                            out,
                        );
                    }
                }
            }
            Block::Code { lines, .. } => {
                let style = Style { code: true, ..Style::default() };
                for line in lines {
                    out.push(Piece::new(line.chars().map(|c| (c, style)).collect(), at));
                }
            }
            Block::Rule => {
                // Drawn as the line it stands for rather than as the marks
                // somebody typed.
                // As many box-drawing characters as fit the room left over
                // once the quote bar or the nesting has had its share.
                let lead = format!("{}{}", " ".repeat(at.depth * 2), at.prefix);
                let unit = adv.of('\u{2500}', size);
                let room = (width - adv.width(&lead, size)).max(0.0);
                let count = if unit > 0.0 { (room / unit).floor() as usize } else { 0 };
                let mut piece = Piece::new(
                    std::iter::repeat_n(('\u{2500}', Style::default()), count.max(1)).collect(),
                    at,
                );
                piece.muted = true;
                out.push(piece);
            }
            Block::Table(table) => flatten_table(table, at, out),
        }
    }
}

/// A table, as the rows it is made of.
///
/// No column alignment, because a card has no room to hold columns apart and
/// padding them to a common width would spend most of a narrow card on spaces.
/// The header row keeps its weight, which is what still makes it read as one.
fn flatten_table(table: &Table, at: &Where, out: &mut Vec<Piece>) {
    let mut row = |cells: &[Run], bold: bool| {
        let mut body: Vec<(char, Style)> = Vec::new();
        for (n, cell) in cells.iter().enumerate() {
            if n > 0 {
                body.extend(CELL.chars().map(|c| (c, Style::default())));
            }
            body.extend(spread(cell, Style { bold, ..Style::default() }));
        }
        out.push(Piece::new(body, at));
    };
    if !table.head.is_empty() {
        row(&table.head, true);
    }
    for cells in &table.rows {
        row(cells, false);
    }
}

/// A run's spans back out to one style per character, with `over` folded in.
///
/// `over` is what the *block* imposes on everything in it — bold, for a
/// heading — and it is an or rather than a replacement, so a link inside a
/// heading is still a link.
fn spread(run: &Run, over: Style) -> Vec<(char, Style)> {
    let mut out = Vec::new();
    for span in run {
        let style = Style {
            bold: span.style.bold || over.bold,
            italic: span.style.italic || over.italic,
            code: span.style.code || over.code,
            strike: span.style.strike || over.strike,
            link: span.style.link || over.link,
        };
        out.extend(span.text.chars().map(|c| (c, style)));
    }
    out
}

// ---------------------------------------------------------------------------
// Wrapping
// ---------------------------------------------------------------------------

/// Break styled characters into lines of at most `columns` of them.
///
/// Greedy, by word, with a hard break for a word longer than a whole line — the
/// same rules the plain label wrap uses, and for the same reasons, but carrying
/// a style along with every character.
fn fold(body: &[(char, Style)], room: f32, em: f32, adv: &dyn Advance) -> Vec<Vec<(char, Style)>> {
    // An empty block is a paragraph break and has to survive as one.
    if body.is_empty() {
        return vec![Vec::new()];
    }

    let wide = |run: &[(char, Style)]| -> f32 { run.iter().map(|(c, _)| adv.of(*c, em)).sum() };
    let space = adv.of(' ', em);

    let mut out: Vec<Vec<(char, Style)>> = Vec::new();
    let mut line: Vec<(char, Style)> = Vec::new();
    let mut so_far = 0.0f32;

    for word in body.split(|(c, _)| *c == ' ').filter(|w| !w.is_empty()) {
        let mut word = word;
        let mut word_wide = wide(word);
        // Too long for a line of its own: cut it, or the greedy loop below
        // would never place it and would spin.
        while word_wide > room {
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
                so_far = 0.0;
            }
            let cut = fits(word, room, em, adv);
            out.push(word[..cut].to_vec());
            word = &word[cut..];
            word_wide = wide(word);
        }
        let would_be = if line.is_empty() { word_wide } else { so_far + space + word_wide };
        if would_be > room && !line.is_empty() {
            out.push(std::mem::take(&mut line));
            so_far = 0.0;
        }
        if let Some((_, before)) = line.last().copied() {
            // The space between two words is set the way *both* of them are,
            // so a bold phrase stays one run across its spaces while a bold
            // word does not drag its setting along into the gap after it —
            // which for a strikethrough or a link is a visible tail.
            let after = word.first().map(|(_, s)| *s).unwrap_or_default();
            line.push((' ', shared(before, after)));
            so_far += space;
        }
        line.extend_from_slice(word);
        so_far += word_wide;
    }
    if !line.is_empty() || out.is_empty() {
        out.push(line);
    }
    out
}

/// How many characters of `run` fit in `room` — at least one, whatever the
/// answer, because the caller is cutting a word that does not fit and a cut of
/// nothing would leave it doing that forever.
fn fits(run: &[(char, Style)], room: f32, em: f32, adv: &dyn Advance) -> usize {
    let mut wide = 0.0;
    let mut count = 0;
    for (c, _) in run {
        let next = wide + adv.of(*c, em);
        if next > room && count > 0 {
            break;
        }
        wide = next;
        count += 1;
    }
    count.max(1).min(run.len().saturating_sub(1)).max(1)
}

/// What two neighbours agree about, for the space between them.
fn shared(a: Style, b: Style) -> Style {
    Style {
        bold: a.bold && b.bold,
        italic: a.italic && b.italic,
        code: a.code && b.code,
        strike: a.strike && b.strike,
        link: a.link && b.link,
    }
}

/// Styled characters back into runs, one per stretch that is set the same way.
fn group(run: &[(char, Style)]) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    for (c, style) in run {
        match out.last_mut() {
            Some(span) if span.style == *style => span.text.push(*c),
            _ => out.push(Span::new(c.to_string(), *style)),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::Estimate;

    /// [`lay_out`] measured in characters rather than pixels.
    ///
    /// Every assertion below is about *where the words broke*, and a test that
    /// had to know how wide an `m` is in whatever face the machine running it
    /// happens to have would be a test about that machine. [`Estimate::columns`]
    /// makes one character one unit wide, so `columns` here means columns.
    fn lay_out(text: &str, columns: usize, rows: usize) -> Vec<Line> {
        super::lay_out(text, columns as f32, 1.0, rows, &Estimate::columns())
    }

    /// The text of every line, which is what a card visibly says.
    fn text(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::text).collect()
    }

    /// Every span, as `(text, "bics-l")` — the flags that are on, in order.
    fn spans(line: &Line) -> Vec<(String, String)> {
        line.spans
            .iter()
            .map(|s| {
                let mut on = String::new();
                for (flag, letter) in [
                    (s.style.bold, 'b'),
                    (s.style.italic, 'i'),
                    (s.style.code, 'c'),
                    (s.style.strike, 's'),
                    (s.style.link, 'l'),
                ] {
                    if flag {
                        on.push(letter);
                    }
                }
                (s.text.clone(), on)
            })
            .collect()
    }

    #[test]
    fn the_marks_come_off_and_the_setting_stays_on() {
        let out = lay_out("a **bold** word", 40, 8);
        assert_eq!(text(&out), ["a bold word"], "the asterisks are not words");
        assert_eq!(
            spans(&out[0]),
            [
                ("a ".to_string(), String::new()),
                ("bold".into(), "b".into()),
                (" word".into(), String::new())
            ]
        );
    }

    #[test]
    fn emphasis_code_and_a_line_through_all_arrive() {
        let out = lay_out("*this* `that` ~~gone~~", 40, 8);
        assert_eq!(text(&out), ["this that gone"]);
        let on: Vec<String> = spans(&out[0]).into_iter().map(|(_, f)| f).collect();
        assert!(on.contains(&"i".to_string()), "{on:?}");
        assert!(on.contains(&"c".to_string()), "{on:?}");
        assert!(on.contains(&"s".to_string()), "{on:?}");
    }

    #[test]
    fn a_marker_with_nothing_to_close_it_is_a_character() {
        // Half-typed text is most of what this reader is ever shown, and it
        // has to look like half-typed text rather than reformatting the note.
        let out = lay_out("2 * 3 is six", 40, 8);
        assert_eq!(text(&out), ["2 * 3 is six"]);
        assert_eq!(spans(&out[0]), [("2 * 3 is six".to_string(), String::new())]);
    }

    #[test]
    fn an_underscore_inside_a_word_is_part_of_the_word() {
        let out = lay_out("call board_view_now once", 40, 8);
        assert_eq!(text(&out), ["call board_view_now once"]);
        assert_eq!(spans(&out[0]).len(), 1, "no italics in a snake_case name");
    }

    #[test]
    fn a_heading_loses_its_hashes_and_gains_a_size() {
        let out = lay_out("# Title\nand words", 40, 8);
        assert_eq!(text(&out), ["Title", "and words"]);
        assert!(out[0].scale > 1.0);
        assert_eq!(out[1].scale, 1.0);
        assert!(out[0].spans.iter().all(|s| s.style.bold), "a heading is bold throughout");
    }

    #[test]
    fn a_note_that_opens_with_a_heading_still_says_something_in_one_row() {
        // The card is down to a single body row, which is less than a heading
        // costs. It used to come back empty — the whole note gone, on a card
        // that was still plainly big enough to read a word off.
        let out = lay_out("# Title\nand words", 40, 1);
        assert_eq!(text(&out), ["Title…"], "a heading-first note went blank");
    }

    #[test]
    fn a_note_clipped_after_its_first_line_still_says_it_was_clipped() {
        let out = lay_out("# Title\nand words\nand more", 40, 2);
        assert_eq!(text(&out), ["Title…"]);
    }

    #[test]
    fn a_deep_heading_is_bold_rather_than_hashes() {
        // The line reader this replaces stopped at three levels and showed
        // `#### deep` with its hashes on, which is the one thing a reader must
        // never do: print the syntax at somebody.
        let out = lay_out("#### deep", 40, 8);
        assert_eq!(text(&out), ["deep"]);
        assert_eq!(out[0].scale, 1.0, "a card has no fourth size");
        assert!(out[0].spans.iter().all(|s| s.style.bold));
    }

    #[test]
    fn a_bullet_becomes_a_bullet() {
        let out = lay_out("- one\n- two", 40, 8);
        assert_eq!(text(&out), ["\u{2022} one", "\u{2022} two"]);
    }

    #[test]
    fn a_wrapped_bullet_hangs_under_itself() {
        let out = lay_out("- one two three four", 12, 8);
        assert_eq!(text(&out), ["\u{2022} one two", "  three four"]);
    }

    #[test]
    fn a_list_inside_a_list_is_indented_under_it() {
        let out = lay_out("- one\n  - inner\n- two", 40, 8);
        assert_eq!(text(&out), ["\u{2022} one", "  \u{2022} inner", "\u{2022} two"]);
    }

    #[test]
    fn a_numbered_list_keeps_the_numbers_that_were_typed() {
        let out = lay_out("3. third\n4. fourth", 40, 8);
        assert_eq!(text(&out), ["3. third", "4. fourth"]);
    }

    #[test]
    fn a_task_shows_whether_it_is_done() {
        let out = lay_out("- [ ] open\n- [x] shut", 40, 8);
        assert_eq!(text(&out), ["\u{2610} open", "\u{2611} shut"]);
    }

    #[test]
    fn a_quote_is_marked_and_said_quietly() {
        let out = lay_out("> borrowed", 40, 8);
        assert_eq!(text(&out), ["\u{258f} borrowed"]);
        assert!(out[0].muted);
    }

    #[test]
    fn a_rule_is_drawn_as_a_rule() {
        let out = lay_out("---", 6, 8);
        assert_eq!(text(&out), ["\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}"]);
        assert!(out[0].muted);
    }

    #[test]
    fn a_link_shows_its_words_and_not_its_address() {
        let out = lay_out("see [the docs](https://example.invalid/x)", 40, 8);
        assert_eq!(text(&out), ["see the docs"]);
        assert!(out[0].spans.iter().any(|s| s.style.link && s.text == "the docs"));
    }

    #[test]
    fn a_bracket_that_is_not_a_link_is_left_alone() {
        let out = lay_out("a [note] here", 40, 8);
        assert_eq!(text(&out), ["a [note] here"]);
    }

    #[test]
    fn a_table_is_flattened_to_its_rows() {
        // A card cannot hold columns apart, so it says the same thing in the
        // shape it does have room for rather than dropping the table.
        let out = lay_out("| a | b |\n|---|---|\n| 1 | 2 |", 40, 8);
        assert_eq!(text(&out), ["a \u{2502} b", "1 \u{2502} 2"]);
        assert!(out[0].spans.iter().any(|s| s.style.bold), "the header keeps its weight");
    }

    #[test]
    fn a_style_survives_the_line_it_is_wrapped_across() {
        // The reason the parse happens per character and the fold happens
        // after it: a bold that spans a wrap is bold on both lines.
        let out = lay_out("**one two three four**", 10, 8);
        assert_eq!(text(&out), ["one two", "three four"]);
        assert!(out.iter().all(|l| l.spans.iter().all(|s| s.style.bold)), "{out:?}");
    }

    #[test]
    fn a_fence_is_taken_exactly_as_typed() {
        let out = lay_out("```\nlet **x** = 1;\n```", 40, 8);
        assert_eq!(text(&out), ["let **x** = 1;"]);
        assert!(out[0].spans.iter().all(|s| s.style.code));
    }

    #[test]
    fn a_paragraph_break_is_kept() {
        let out = lay_out("one\n\ntwo", 40, 8);
        assert_eq!(text(&out), ["one", "", "two"]);
    }

    #[test]
    fn a_line_break_is_a_line_break() {
        // CommonMark would run these together. A note does not: see the
        // core module's header.
        let out = lay_out("one\ntwo", 40, 8);
        assert_eq!(text(&out), ["one", "two"]);
    }

    #[test]
    fn what_does_not_fit_says_so() {
        let out = lay_out("one\ntwo\nthree\nfour", 40, 2);
        assert_eq!(out.len(), 2);
        assert!(out[1].text().ends_with('\u{2026}'), "{out:?}");
    }

    #[test]
    fn a_heading_is_charged_for_the_height_it_takes() {
        // Otherwise a card with room for four lines would happily draw four
        // headings and spill them out of the bottom of itself.
        let out = lay_out("# one\n\n# two\n\n# three\n\n# four", 40, 4);
        assert!(out.len() < 7, "{out:?}");
    }

    #[test]
    fn a_word_too_long_for_the_card_is_cut_rather_than_left_out() {
        let out = lay_out("https://example.invalid/a/very/long/path", 12, 6);
        assert!(out.len() > 1);
        assert!(out.iter().all(|l| l.text().chars().count() <= 12), "{out:?}");
    }

    #[test]
    fn nothing_at_all_is_one_empty_line_rather_than_none() {
        assert_eq!(text(&lay_out("", 20, 4)), [""]);
    }

    #[test]
    fn plain_text_is_a_line_of_plain_text() {
        let line = Line::plain("as typed");
        assert_eq!(line.text(), "as typed");
        assert_eq!(line.scale, 1.0);
        assert_eq!(spans(&line), [("as typed".to_string(), String::new())]);
    }
}
