//! Markdown, as much of it as a sticky note wants.
//!
//! A note is stored as plain text and always has been — `meta.text` in the
//! format, one `.md` file per note in the archive — so this is not a change to
//! what a note *is*. It is the reader that was missing: the words were being
//! drawn exactly as typed, asterisks and all, and the only nod to any of it was
//! a line in the label builder that deleted `# ` so a heading did not read as a
//! hash. This module is that line, done properly.
//!
//! ## What it takes and what it hands back
//!
//! In: the note's text, and how many `columns` and `rows` of body text the card
//! has room for. Out: [`Line`]s that are already wrapped, already elided if
//! there were more of them than fit, and already broken into [`Span`]s that are
//! each set one way. The painter shapes a run per span and never has to know
//! what an asterisk meant.
//!
//! ## Why the wrapping happens in here
//!
//! Because a style crosses a line break. Wrapping first and parsing per line
//! would end a bold at the wrap, and parsing first and wrapping the plain text
//! afterwards would lose which characters the bold was on. So the text is
//! parsed to a style *per character*, folded into lines at that granularity,
//! and only then grouped back into runs — which also makes the fold itself
//! ordinary, since a `Vec` of characters wraps the same way a string does.
//!
//! ## What is deliberately not here
//!
//! No tables, no reference links, no HTML, no nested blockquotes. A note is
//! capped at [`NOTE_MAX`](mbrd_core::model::NOTE_MAX) characters and lives on a
//! card a few hundred pixels wide; the parts of Markdown it can hold are the
//! parts that are worth the ambiguity.

/// How a run of characters is set.
///
/// A set of flags rather than an enum because they genuinely combine: bold
/// inside a link inside a heading is all three at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Style {
    pub bold: bool,
    pub italic: bool,
    /// Backticks. Drawn as a wash behind the characters rather than in another
    /// face, because a face this build cannot be sure is installed is a face
    /// that silently comes out as the body one.
    pub code: bool,
    pub strike: bool,
    /// The visible half of `[text](url)`. The address is dropped: there is
    /// nowhere on a card to put it and nothing yet to do with it.
    pub link: bool,
}

/// A run of characters that are all set the same way.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub text: String,
    pub style: Style,
}

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
                vec![Span { text, style: Style::default() }]
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
/// `columns` and `rows` are in body-sized characters and lines. A heading takes
/// more than one row's worth of height, and is charged for it — see the budget
/// in the loop — so a card does not overflow just because somebody started
/// their note with a `#`.
pub fn lay_out(text: &str, columns: usize, rows: usize) -> Vec<Line> {
    let columns = columns.max(1);
    let mut out: Vec<Line> = Vec::new();
    let mut left = rows as f32;
    let mut fenced = false;
    let mut clipped = false;

    'outer: for raw in text.split('\n') {
        // A fence is a switch, not a line. Everything between two of them is
        // taken as typed — the whole point of asking for it.
        if raw.trim().starts_with("```") {
            fenced = !fenced;
            continue;
        }
        let block = if fenced { verbatim(raw) } else { classify(raw, columns) };

        // The room this block's own lines get, after its bullet or its quote
        // mark and after the width a heading's larger letters cost.
        let room = ((columns as f32 / block.scale).floor() as usize)
            .saturating_sub(block.prefix.chars().count() + block.indent)
            .max(1);

        for (n, run) in fold(&block.body, room).into_iter().enumerate() {
            if left < block.scale {
                clipped = true;
                break 'outer;
            }
            left -= block.scale;

            // The marker on the first line and an indent under it on the rest,
            // so a bullet that wraps hangs rather than starting back at the
            // margin and reading as a second bullet.
            let lead = if n == 0 {
                format!("{}{}", " ".repeat(block.indent), block.prefix)
            } else {
                " ".repeat(block.indent + block.prefix.chars().count())
            };
            let mut spans = Vec::new();
            if !lead.trim().is_empty() || (!lead.is_empty() && n > 0) {
                spans.push(Span { text: lead, style: block.marker });
            }
            spans.extend(group(&run));
            out.push(Line { spans, scale: block.scale, muted: block.muted });
        }
    }

    // Something was left out, so the last line has to say so. A card that ends
    // mid-sentence with no ellipsis is a card that looks complete and is not.
    if clipped {
        if let Some(last) = out.last_mut() {
            if let Some(span) = last.spans.last_mut() {
                span.text.push('\u{2026}');
            } else {
                last.spans.push(Span { text: "\u{2026}".into(), style: Style::default() });
            }
        }
    }
    out
}

/// One line of the source, once it is known what kind of line it is.
struct Block {
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

impl Block {
    fn body(body: Vec<(char, Style)>) -> Block {
        Block {
            prefix: String::new(),
            marker: Style::default(),
            indent: 0,
            scale: 1.0,
            muted: false,
            body,
        }
    }
}

/// Inside a fence: the characters, exactly, set as code.
fn verbatim(raw: &str) -> Block {
    let style = Style { code: true, ..Style::default() };
    Block::body(raw.chars().map(|c| (c, style)).collect())
}

/// What kind of line this is, and what is left of it once the marker is off.
fn classify(raw: &str, columns: usize) -> Block {
    let trimmed = raw.trim_end();
    let body = trimmed.trim_start();
    // Two spaces to a level, which is what every editor's tab key does here.
    let indent = (trimmed.chars().count() - body.chars().count()) / 2 * 2;

    // A rule. Three or more of one mark and nothing else, drawn as the line it
    // stands for rather than as the marks somebody typed.
    if body.chars().count() >= 3 && ['-', '*', '_'].iter().any(|m| body.chars().all(|c| c == *m)) {
        let mut block =
            Block::body(std::iter::repeat_n(('\u{2500}', Style::default()), columns).collect());
        block.muted = true;
        return block;
    }

    // A heading. One to three, because a card is not a document and a fourth
    // level would be body text with a hash in front of it.
    let hashes = body.chars().take_while(|c| *c == '#').count();
    if (1..=3).contains(&hashes) && body.chars().nth(hashes) == Some(' ') {
        let rest: String = body.chars().skip(hashes + 1).collect();
        let mut block = Block::body(bolden(inline(&rest)));
        block.scale = [1.5, 1.25, 1.1][hashes - 1];
        block.indent = indent;
        return block;
    }

    // A quote. The bar is the mark, and the words go quiet with it.
    if let Some(rest) = body.strip_prefix('>') {
        let mut block = Block::body(inline(rest.trim_start()));
        block.prefix = "\u{258f} ".into();
        block.muted = true;
        block.indent = indent;
        return block;
    }

    // A task, before a bullet, because a task is a bullet with a box on it.
    for (mark, box_) in [("- [ ] ", "\u{2610} "), ("- [x] ", "\u{2611} "), ("- [X] ", "\u{2611} ")]
    {
        if let Some(rest) = body.strip_prefix(mark) {
            let mut block = Block::body(inline(rest));
            block.prefix = box_.into();
            block.indent = indent;
            return block;
        }
    }

    // A bullet. The space after the mark is required, which is what keeps
    // `*emphasis*` at the start of a line from becoming a list.
    for mark in ['-', '*', '+'] {
        if let Some(rest) = body.strip_prefix(&format!("{mark} ")) {
            let mut block = Block::body(inline(rest));
            block.prefix = "\u{2022} ".into();
            block.marker = Style { bold: true, ..Style::default() };
            block.indent = indent;
            return block;
        }
    }

    // A numbered item. The number somebody typed, not one counted here: a list
    // that renumbers itself would be a list that argues with the text.
    let digits = body.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && body.chars().skip(digits).take(2).collect::<String>() == ". " {
        let rest: String = body.chars().skip(digits + 2).collect();
        let mut block = Block::body(inline(&rest));
        block.prefix = format!("{}. ", &body[..digits]);
        block.indent = indent;
        return block;
    }

    let mut block = Block::body(inline(body));
    block.indent = indent;
    block
}

/// Everything in a heading, set bold, whatever else it already was.
fn bolden(mut chars: Vec<(char, Style)>) -> Vec<(char, Style)> {
    for (_, style) in &mut chars {
        style.bold = true;
    }
    chars
}

/// The inline pass: markers off, a style on every character that is left.
///
/// A marker only opens where a matching one closes later on the same line, so
/// a lone asterisk in the middle of a sentence stays an asterisk instead of
/// italicising the rest of the note. That is the difference between a reader
/// and a parser: this one is being shown half-finished text constantly, and
/// half-finished text has to look like what it is.
fn inline(text: &str) -> Vec<(char, Style)> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::with_capacity(chars.len());
    let mut style = Style::default();
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];

        // Inside backticks nothing is a marker except the closing backtick.
        if style.code {
            if c == '`' {
                style.code = false;
            } else {
                out.push((c, style));
            }
            i += 1;
            continue;
        }

        // A backslash spends itself on the next character, whatever it is.
        if c == '\\' && i + 1 < chars.len() {
            out.push((chars[i + 1], style));
            i += 2;
            continue;
        }

        let pair = chars.get(i + 1) == Some(&c);
        let two = |mark: char, on: bool, at: usize| -> bool {
            on || closes(&chars, at + 2, &[mark, mark])
        };

        match c {
            '*' | '_' if pair && two(c, style.bold, i) => {
                style.bold = !style.bold;
                i += 2;
            }
            '~' if pair && two('~', style.strike, i) => {
                style.strike = !style.strike;
                i += 2;
            }
            // `_` only between words, so `snake_case` is a name and not an
            // italic. `*` has no such rule because nothing is spelt with one.
            '*' | '_' if !pair && (c == '*' || word_edge(&chars, i)) => {
                if style.italic || closes(&chars, i + 1, &[c]) {
                    style.italic = !style.italic;
                    i += 1;
                } else {
                    out.push((c, style));
                    i += 1;
                }
            }
            '`' if closes(&chars, i + 1, &['`']) => {
                style.code = true;
                i += 1;
            }
            '[' => match link(&chars, i) {
                Some((text, next)) => {
                    let mut linked = style;
                    linked.link = true;
                    out.extend(text.into_iter().map(|c| (c, linked)));
                    i = next;
                }
                None => {
                    out.push((c, style));
                    i += 1;
                }
            },
            _ => {
                out.push((c, style));
                i += 1;
            }
        }
    }
    out
}

/// Whether `mark` appears again from `from` on. What makes an opener an opener.
fn closes(chars: &[char], from: usize, mark: &[char]) -> bool {
    if from >= chars.len() {
        return false;
    }
    chars[from..].windows(mark.len()).any(|w| w == mark)
}

/// Whether an underscore here is between words rather than inside one.
fn word_edge(chars: &[char], at: usize) -> bool {
    let before = at.checked_sub(1).and_then(|i| chars.get(i));
    !before.is_some_and(|c| c.is_alphanumeric())
}

/// `[text](url)` from `at`, as the characters to draw and where to carry on.
fn link(chars: &[char], at: usize) -> Option<(Vec<char>, usize)> {
    let close = (at + 1..chars.len()).find(|i| chars[*i] == ']')?;
    if chars.get(close + 1) != Some(&'(') {
        return None;
    }
    let end = (close + 2..chars.len()).find(|i| chars[*i] == ')')?;
    let text: Vec<char> = chars[at + 1..close].to_vec();
    // `[](url)` has nothing to show, so it is not a link — it is two brackets.
    (!text.is_empty()).then_some((text, end + 1))
}

/// Break styled characters into lines of at most `columns` of them.
///
/// Greedy, by word, with a hard break for a word longer than a whole line — the
/// same rules the plain label wrap uses, and for the same reasons, but carrying
/// a style along with every character.
fn fold(body: &[(char, Style)], columns: usize) -> Vec<Vec<(char, Style)>> {
    // An empty source line is a paragraph break and has to survive as one.
    if body.is_empty() {
        return vec![Vec::new()];
    }

    let mut out: Vec<Vec<(char, Style)>> = Vec::new();
    let mut line: Vec<(char, Style)> = Vec::new();

    for word in body.split(|(c, _)| *c == ' ').filter(|w| !w.is_empty()) {
        let mut word = word;
        // Too long for a line of its own: cut it, or the greedy loop below
        // would never place it and would spin.
        while word.len() > columns {
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            let (head, tail) = word.split_at(columns);
            out.push(head.to_vec());
            word = tail;
        }
        let would_be = if line.is_empty() { word.len() } else { line.len() + 1 + word.len() };
        if would_be > columns && !line.is_empty() {
            out.push(std::mem::take(&mut line));
        }
        if let Some((_, before)) = line.last().copied() {
            // The space between two words is set the way *both* of them are,
            // so a bold phrase stays one run across its spaces while a bold
            // word does not drag its setting along into the gap after it —
            // which for a strikethrough or a link is a visible tail.
            let after = word.first().map(|(_, s)| *s).unwrap_or_default();
            line.push((' ', shared(before, after)));
        }
        line.extend_from_slice(word);
    }
    if !line.is_empty() || out.is_empty() {
        out.push(line);
    }
    out
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
            _ => out.push(Span { text: c.to_string(), style: *style }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The text of every line, which is what a card visibly says.
    fn text(lines: &[Line]) -> Vec<String> {
        lines.iter().map(Line::text).collect()
    }

    /// Every span, as `(text, "bic-l")` — the flags that are on, in order.
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
        let out = lay_out("call board_view once", 40, 8);
        assert_eq!(text(&out), ["call board_view once"]);
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
    fn a_fourth_level_heading_is_not_one() {
        // A card is not a document, and `#### ` would be body text with four
        // hashes in front of it either way.
        let out = lay_out("#### deep", 40, 8);
        assert_eq!(text(&out), ["#### deep"]);
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
    fn what_does_not_fit_says_so() {
        let out = lay_out("one\ntwo\nthree\nfour", 40, 2);
        assert_eq!(out.len(), 2);
        assert!(out[1].text().ends_with('\u{2026}'), "{out:?}");
    }

    #[test]
    fn a_heading_is_charged_for_the_height_it_takes() {
        // Otherwise a card with room for four lines would happily draw four
        // headings and spill them out of the bottom of itself.
        let out = lay_out("# one\n# two\n# three\n# four", 40, 4);
        assert!(out.len() < 4, "{out:?}");
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
