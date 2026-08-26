//! Markdown, parsed once for everything that shows it.
//!
//! A note is stored as plain text and always has been — `meta.text` in the
//! format, one `.md` file per note in the archive — so this is not a change to
//! what a note *is*. It is the reader, and it lives down here rather than
//! beside the painter for the reason everything else in this crate does: the
//! answer to "what does this text mean" is arithmetic over a string, and
//! written here it can be tested by asserting blocks rather than by looking at
//! a window.
//!
//! ## One parser, two pictures
//!
//! The same note is drawn in two very different places — a card a few hundred
//! pixels wide, and the whole window when somebody opens it — and those want
//! different *layouts*: one elides, scales and wraps to a column count; the
//! other sets a document. What they must never differ about is what the text
//! **says**. A card that read `2 * 3` as an italic while the open window read
//! it as a multiplication would be two programs disagreeing about one file.
//!
//! So this module answers only the first question. [`parse`] hands back a tree
//! of [`Block`]s with every marker already off and every character already
//! carrying its [`Style`]; the card layout in `markdown.rs` in the app crate
//! folds that into lines, and the open window sets it as a page. Neither owns
//! a parser.
//!
//! ## Why `pulldown-cmark` rather than the reader that was here
//!
//! What was here before was a hand-rolled line reader — good enough for a note,
//! and it had to be, because a card cannot show a table anyway. The moment a
//! note opens full-window that ceiling stops being free: nested
//! lists, tables, setext headings and reference links are all things a page has
//! room for and a hand-rolled reader gets subtly wrong. `pulldown-cmark` is the
//! CommonMark implementation the rest of the Rust world uses, it hands back a
//! source range with every event — which is what makes the blank-line rule
//! below possible at all — and it is a pull parser, so this costs an
//! allocation per block rather than a syntax tree per note.
//!
//! ## Two places this deliberately is not CommonMark
//!
//! 1. **A single newline is a line break.** CommonMark folds `one\ntwo` into
//!    one paragraph reading `one two`. That is right for a document written in
//!    a text editor and wrong for a note, where the newlines somebody
//!    typed are the shape they meant. So a soft break ends a run here, exactly
//!    as a hard break does — the same bargain every comment box on the web has
//!    settled on.
//! 2. **A blank line survives as one.** CommonMark discards it: block
//!    separation is structure, not whitespace. But a note is often written as
//!    stanzas with air between them, and a reader that closed the gaps would be
//!    reformatting somebody's note in front of them. [`Block::Gap`] is that
//!    blank line, recovered from the source ranges either side of it.
//!
//! Everything else is CommonMark, plus tables, strikethrough and task lists.
//! Not HTML: a note is not a web page, and raw tags are shown as the characters
//! they are rather than interpreted.

use std::ops::Range;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};

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
    /// The visible half of `[text](url)`. See [`Span::href`] for the other.
    pub link: bool,
}

/// A run of characters that are all set the same way.
#[derive(Debug, Clone, PartialEq)]
pub struct Span {
    pub text: String,
    pub style: Style,
    /// Where a link goes, on the spans that are one.
    ///
    /// The card drops this — there is nowhere on a card to put an address and
    /// nothing it could do with one — but the open window has room for both, so
    /// the parser keeps it rather than making the window parse again to find
    /// it.
    pub href: Option<String>,
}

impl Span {
    pub fn new(text: impl Into<String>, style: Style) -> Span {
        Span { text: text.into(), style, href: None }
    }
}

/// One visual line of inline content: a stretch of spans with no break in it.
///
/// A run rather than a line, because it is not yet a line — the card wraps it
/// to a column count and the window wraps it to a width, and either turns one
/// of these into several.
pub type Run = Vec<Span>;

/// What the marker on a list item is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Marker {
    Bullet,
    /// The number **as it was typed**, not one counted here. A list that
    /// renumbers itself is a list that argues with the text in front of it, and
    /// somebody who wrote `1.` five times meant something by it.
    Number(String),
    /// `- [ ]` and `- [x]`.
    Task(bool),
}

/// One item of a list, and everything under it — which may be another list.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub marker: Marker,
    pub blocks: Vec<Block>,
}

/// How a table column is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    /// No colon in the divider row. Left, in practice, but said separately
    /// because a renderer may want to set a column of numbers its own way.
    Ragged,
    Left,
    Center,
    Right,
}

/// A table, once the pipes are off.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    pub align: Vec<Align>,
    pub head: Vec<Run>,
    pub rows: Vec<Vec<Run>>,
}

/// A block of a note, with its markers off.
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// A blank line somebody left in the source. See the module header.
    Gap,
    Paragraph(Vec<Run>),
    /// `level` is 1 to 6, as written. What each level *looks* like is the
    /// renderer's business — a card has room for about three of them and a page
    /// for all six.
    Heading {
        level: u8,
        runs: Vec<Run>,
    },
    Quote(Vec<Block>),
    List {
        ordered: bool,
        items: Vec<Entry>,
    },
    /// A fence. `language` is the word after the backticks, where there was
    /// one; nothing here highlights with it yet, and it is kept because the
    /// alternative is parsing the note twice when something does.
    Code {
        language: Option<String>,
        lines: Vec<String>,
    },
    Rule,
    Table(Table),
}

/// Text in, blocks out. The whole of the public surface.
pub fn parse(text: &str) -> Vec<Block> {
    let mut options = Options::empty();
    options.insert(Options::ENABLE_TABLES);
    options.insert(Options::ENABLE_STRIKETHROUGH);
    options.insert(Options::ENABLE_TASKLISTS);

    let mut builder = Builder {
        src: text,
        stack: vec![Frame::Body(Body::default())],
        style: Style::default(),
        href: None,
    };
    for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
        builder.take(event, range);
    }
    builder.finish()
}

// ---------------------------------------------------------------------------
// Building
// ---------------------------------------------------------------------------

/// Blocks accumulating inside one container — the note itself, a quote, or a
/// list item — and where the last of them ended.
#[derive(Debug, Default)]
struct Body {
    blocks: Vec<Block>,
    /// The byte the previous block ended at, so the next one can ask what was
    /// between them. `None` until there has been a previous one, which is what
    /// keeps a note that opens with a blank line from opening with a gap.
    last: Option<usize>,
}

/// What is currently open. A stack, because Markdown nests.
#[derive(Debug)]
enum Frame {
    Body(Body),
    List {
        ordered: bool,
        items: Vec<Entry>,
    },
    Item {
        marker: Marker,
    },
    /// `loose` is whether the source actually had a `Paragraph` tag around
    /// this. A tight list item does not — its words arrive as bare text — so
    /// one is opened for them, and the next block-level event has to know to
    /// close it again. See [`Builder::flush`].
    Para {
        runs: Vec<Run>,
        loose: bool,
    },
    Heading {
        level: u8,
        runs: Vec<Run>,
    },
    Code {
        language: Option<String>,
        text: String,
    },
    Table {
        align: Vec<Align>,
        head: Vec<Run>,
        rows: Vec<Vec<Run>>,
        row: Vec<Run>,
        heading: bool,
    },
    Cell(Run),
}

struct Builder<'a> {
    src: &'a str,
    stack: Vec<Frame>,
    /// How the characters arriving right now are set. One field rather than a
    /// second stack: `Strong` and `Emphasis` do not nest inside themselves in
    /// CommonMark, so a flag that is turned on and off is the whole of it.
    style: Style,
    /// The address of the link being written, where one is open.
    href: Option<String>,
}

impl Builder<'_> {
    fn finish(mut self) -> Vec<Block> {
        // Everything but the root should already have been closed by its own
        // `End`; the parser closes an unterminated container at the end of the
        // document, so this is a formality rather than error handling.
        let end = self.src.len();
        while self.stack.len() > 1 {
            self.close(end..end);
        }
        match self.stack.pop() {
            Some(Frame::Body(body)) => body.blocks,
            _ => Vec::new(),
        }
    }

    fn take(&mut self, event: Event<'_>, range: Range<usize>) {
        match event {
            Event::Start(tag) => self.open(tag, range),
            Event::End(tag) => self.end(tag, range),
            Event::Text(text) => self.text(&text),
            // Inline code is a leaf rather than a pair, so its style is set on
            // the characters directly instead of through the stack.
            Event::Code(text) => {
                let style = Style { code: true, ..self.style };
                self.write(&text, style);
            }
            // Not interpreted. A note is not a web page — see the module
            // header — so the tags stay the characters somebody typed.
            Event::Html(text) | Event::InlineHtml(text) => self.text(&text),
            // Both end a run. The difference between them is only whether
            // somebody asked for it, and this reader gives it to them either
            // way.
            Event::SoftBreak | Event::HardBreak => self.wrap(),
            Event::Rule => {
                self.flush(range.start);
                self.gap(&range);
                self.place(Block::Rule, range);
            }
            Event::TaskListMarker(done) => {
                if let Some(Frame::Item { marker }) =
                    self.stack.iter_mut().rev().find(|f| matches!(f, Frame::Item { .. }))
                {
                    *marker = Marker::Task(done);
                }
            }
            // Neither is enabled, so neither can arrive; taken as their own
            // characters rather than dropped, on the principle the rest of this
            // module runs on.
            Event::FootnoteReference(text) | Event::InlineMath(text) | Event::DisplayMath(text) => {
                self.text(&text)
            }
        }
    }

    fn open(&mut self, tag: Tag<'_>, range: Range<usize>) {
        match tag {
            Tag::Paragraph => {
                self.flush(range.start);
                self.gap(&range);
                self.stack.push(Frame::Para { runs: vec![Vec::new()], loose: true });
            }
            Tag::Heading { level, .. } => {
                self.flush(range.start);
                self.gap(&range);
                self.stack.push(Frame::Heading { level: level as u8, runs: vec![Vec::new()] });
            }
            // The kind is GitHub's `[!NOTE]` callout syntax, which nothing here
            // draws differently yet. Taken as an ordinary quote rather than
            // refused, so the words still arrive.
            Tag::BlockQuote(_) => {
                self.flush(range.start);
                self.gap(&range);
                self.stack.push(Frame::Body(Body::default()));
            }
            Tag::CodeBlock(kind) => {
                self.flush(range.start);
                self.gap(&range);
                let language = match kind {
                    CodeBlockKind::Fenced(word) if !word.trim().is_empty() => {
                        Some(word.trim().to_string())
                    }
                    _ => None,
                };
                self.stack.push(Frame::Code { language, text: String::new() });
            }
            Tag::List(start) => {
                self.flush(range.start);
                self.gap(&range);
                self.stack.push(Frame::List { ordered: start.is_some(), items: Vec::new() });
            }
            Tag::Item => {
                self.flush(range.start);
                let ordered = matches!(self.stack.last(), Some(Frame::List { ordered: true, .. }));
                // The digits the item actually starts with, so a `3.` stays a
                // three. [`Marker::Number`] says why.
                let marker = if ordered {
                    let typed: String =
                        self.src[range.start..].chars().take_while(char::is_ascii_digit).collect();
                    Marker::Number(if typed.is_empty() { "1".into() } else { typed })
                } else {
                    Marker::Bullet
                };
                self.stack.push(Frame::Item { marker });
                self.stack.push(Frame::Body(Body::default()));
            }
            Tag::Table(align) => {
                self.flush(range.start);
                self.gap(&range);
                self.stack.push(Frame::Table {
                    align: align.into_iter().map(read_align).collect(),
                    head: Vec::new(),
                    rows: Vec::new(),
                    row: Vec::new(),
                    heading: false,
                });
            }
            Tag::TableHead => {
                if let Some(Frame::Table { heading, .. }) = self.stack.last_mut() {
                    *heading = true;
                }
            }
            Tag::TableRow => {}
            Tag::TableCell => self.stack.push(Frame::Cell(Vec::new())),
            Tag::Emphasis => self.style.italic = true,
            Tag::Strong => self.style.bold = true,
            Tag::Strikethrough => self.style.strike = true,
            Tag::Link { dest_url, .. } => {
                self.style.link = true;
                self.href = Some(dest_url.to_string());
            }
            // An image's alt text is what there is to show, and it arrives as
            // ordinary text between here and the close. A picture inside a note
            // is a thing this app has cards for.
            Tag::Image { .. } => {}
            Tag::FootnoteDefinition(_)
            | Tag::HtmlBlock
            | Tag::MetadataBlock(_)
            | Tag::DefinitionList
            | Tag::DefinitionListTitle
            | Tag::DefinitionListDefinition
            | Tag::Superscript
            | Tag::Subscript => {}
        }
    }

    fn end(&mut self, tag: TagEnd, range: Range<usize>) {
        match tag {
            TagEnd::Emphasis => self.style.italic = false,
            TagEnd::Strong => self.style.bold = false,
            TagEnd::Strikethrough => self.style.strike = false,
            TagEnd::Link => {
                self.style.link = false;
                self.href = None;
            }
            TagEnd::Image => {}
            TagEnd::TableHead | TagEnd::TableRow => self.row(),
            TagEnd::TableCell => {
                if let Some(Frame::Cell(cell)) = self.stack.pop() {
                    if let Some(Frame::Table { row, .. }) = self.stack.last_mut() {
                        row.push(cell);
                    }
                }
            }
            // One `Start` pushed two frames — the item and its contents — so
            // one `End` takes both back off.
            TagEnd::Item => {
                self.flush(range.end);
                let blocks = match self.stack.pop() {
                    Some(Frame::Body(body)) => body.blocks,
                    _ => Vec::new(),
                };
                if let Some(Frame::Item { marker }) = self.stack.pop() {
                    if let Some(Frame::List { items, .. }) = self.stack.last_mut() {
                        items.push(Entry { marker, blocks });
                    }
                }
            }
            TagEnd::FootnoteDefinition
            | TagEnd::HtmlBlock
            | TagEnd::MetadataBlock(_)
            | TagEnd::DefinitionList
            | TagEnd::DefinitionListTitle
            | TagEnd::DefinitionListDefinition
            | TagEnd::Superscript
            | TagEnd::Subscript => {}
            _ => {
                self.flush(range.end);
                self.close(range);
            }
        }
    }

    /// Pop the innermost container and put what it built into its parent.
    fn close(&mut self, range: Range<usize>) {
        let Some(frame) = self.stack.pop() else { return };
        match frame {
            Frame::Para { runs, .. } => self.place(Block::Paragraph(trimmed(runs)), range),
            Frame::Heading { level, runs } => {
                self.place(Block::Heading { level, runs: trimmed(runs) }, range)
            }
            Frame::Code { language, text } => {
                // A fence's text ends with the newline before the closing
                // backticks, and keeping it would put a blank line at the
                // bottom of every code block in the note.
                let body = text.strip_suffix('\n').unwrap_or(&text);
                let lines = body.split('\n').map(str::to_string).collect();
                self.place(Block::Code { language, lines }, range)
            }
            Frame::List { ordered, items } => self.place(Block::List { ordered, items }, range),
            Frame::Table { align, head, rows, .. } => {
                self.place(Block::Table(Table { align, head, rows }), range)
            }
            // A body reached here is a quote's: an item's is taken by
            // `TagEnd::Item` above, which is the only other thing that opens
            // one.
            Frame::Body(body) => self.place(Block::Quote(body.blocks), range),
            Frame::Item { .. } | Frame::Cell(_) => {}
        }
    }

    /// Close the paragraph that was opened for a tight list item's bare words.
    ///
    /// Called before every block-level event, which is the moment such a
    /// paragraph is over. A paragraph the source actually wrote is left alone —
    /// it has its own `End` coming.
    fn flush(&mut self, at: usize) {
        if matches!(self.stack.last(), Some(Frame::Para { loose: false, .. })) {
            self.close(at..at);
        }
    }

    /// Finish a table row, into the head or the body as it happens to be.
    fn row(&mut self) {
        if let Some(Frame::Table { head, rows, row, heading, .. }) = self.stack.last_mut() {
            let cells = std::mem::take(row);
            if *heading {
                *head = cells;
                *heading = false;
            } else {
                rows.push(cells);
            }
        }
    }

    /// Put a finished block into the container holding it.
    fn place(&mut self, block: Block, range: Range<usize>) {
        let Some(Frame::Body(body)) = self.stack.last_mut() else { return };
        body.last = Some(range.end);
        body.blocks.push(block);
    }

    /// A [`Block::Gap`] where the source had a blank line before this block.
    ///
    /// Asked *before* the block is opened, because that is when its start is
    /// known.
    ///
    /// Read backwards off the block's own start rather than forwards from where
    /// the previous block's range ended, which sounds like the same question
    /// and is not: a block's range takes in the newline that ended it, and a
    /// list's takes in the blank line after it as well, so the text "between"
    /// two blocks is a different amount of the same whitespace depending on
    /// what the first one was. The run of whitespace in front of a block is the
    /// same string however it got there.
    fn gap(&mut self, range: &Range<usize>) {
        let Some(Frame::Body(body)) = self.stack.last() else { return };
        // `None` is the first block of its container, which cannot have a blank
        // line before it that means anything.
        if body.last.is_none() || range.start > self.src.len() {
            return;
        }
        // Two newlines is one blank line. More than one blank line is still one
        // gap: a note is not a place to hold a page apart.
        let before = self.src[..range.start].chars().rev();
        let blank = before.take_while(|c| c.is_whitespace()).filter(|c| *c == '\n').count();
        if blank >= 2 {
            if let Some(Frame::Body(body)) = self.stack.last_mut() {
                body.blocks.push(Block::Gap);
            }
        }
    }

    // -----------------------------------------------------------------------
    // Inline
    // -----------------------------------------------------------------------

    /// Text, set however the enclosing tags say.
    fn text(&mut self, text: &str) {
        // A fence takes its characters exactly, which is the whole point of
        // asking for one — and it arrives line by line, newlines included.
        if let Some(Frame::Code { text: held, .. }) = self.stack.last_mut() {
            held.push_str(text);
            return;
        }
        let style = self.style;
        self.write(text, style);
    }

    /// Add characters to whatever run is open, opening one where none is.
    fn write(&mut self, text: &str, style: Style) {
        let href = self.href.clone();
        let run = self.run();
        match run.last_mut() {
            Some(span) if span.style == style && span.href == href => span.text.push_str(text),
            _ => run.push(Span { text: text.to_string(), style, href }),
        }
    }

    /// End the run and start another. A break, of either kind.
    fn wrap(&mut self) {
        match self.stack.last_mut() {
            Some(Frame::Para { runs, .. }) | Some(Frame::Heading { runs, .. }) => {
                runs.push(Vec::new())
            }
            // A break inside a table cell is a space: a cell is one line by
            // construction, and a renderer that honoured the break would be
            // drawing outside its own row.
            Some(Frame::Cell(_)) => self.write(" ", Style::default()),
            _ => {}
        }
    }

    /// The run characters are going into, opening a paragraph if the words
    /// arrived bare — which is how a tight list item's words arrive.
    fn run(&mut self) -> &mut Run {
        if !matches!(
            self.stack.last(),
            Some(Frame::Para { .. } | Frame::Heading { .. } | Frame::Cell(_))
        ) {
            self.stack.push(Frame::Para { runs: vec![Vec::new()], loose: false });
        }
        match self.stack.last_mut() {
            Some(Frame::Para { runs, .. }) | Some(Frame::Heading { runs, .. }) => {
                if runs.is_empty() {
                    runs.push(Vec::new());
                }
                runs.last_mut().expect("just pushed one")
            }
            Some(Frame::Cell(run)) => run,
            _ => unreachable!("a paragraph was just opened for this"),
        }
    }
}

/// Drop a trailing empty run, which a paragraph ending in a break leaves
/// behind and which would draw as a blank line nobody typed.
fn trimmed(mut runs: Vec<Run>) -> Vec<Run> {
    while runs.len() > 1 && runs.last().is_some_and(Vec::is_empty) {
        runs.pop();
    }
    runs
}

fn read_align(align: Alignment) -> Align {
    match align {
        Alignment::None => Align::Ragged,
        Alignment::Left => Align::Left,
        Alignment::Center => Align::Center,
        Alignment::Right => Align::Right,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The plain text of a run, which is what it visibly says.
    fn said(run: &Run) -> String {
        run.iter().map(|s| s.text.as_str()).collect()
    }

    /// Every run of a paragraph block, flattened, for the common assertion.
    fn lines(blocks: &[Block]) -> Vec<String> {
        blocks
            .iter()
            .flat_map(|b| match b {
                Block::Paragraph(runs) | Block::Heading { runs, .. } => {
                    runs.iter().map(said).collect()
                }
                Block::Gap => vec![String::new()],
                _ => Vec::new(),
            })
            .collect()
    }

    #[test]
    fn the_marks_come_off_and_the_setting_stays_on() {
        let out = parse("a **bold** word");
        let Block::Paragraph(runs) = &out[0] else { panic!("{out:?}") };
        assert_eq!(said(&runs[0]), "a bold word", "the asterisks are not words");
        assert!(runs[0].iter().any(|s| s.style.bold && s.text == "bold"));
    }

    #[test]
    fn a_marker_with_nothing_to_close_it_is_a_character() {
        // Half-typed text is most of what this reader is ever shown, and it has
        // to look like half-typed text rather than reformatting the note.
        let out = parse("2 * 3 is six");
        assert_eq!(lines(&out), ["2 * 3 is six"]);
        let Block::Paragraph(runs) = &out[0] else { panic!() };
        assert_eq!(runs[0].len(), 1, "one plain span, no emphasis: {runs:?}");
    }

    #[test]
    fn an_underscore_inside_a_word_is_part_of_the_word() {
        let out = parse("call snake_case_name once");
        assert_eq!(lines(&out), ["call snake_case_name once"]);
        let Block::Paragraph(runs) = &out[0] else { panic!() };
        assert_eq!(runs[0].len(), 1, "no italics in a snake_case name: {runs:?}");
    }

    #[test]
    fn one_newline_is_a_line_break_and_not_a_space() {
        // CommonMark would fold these into one paragraph reading "one two".
        // See the module header for why a note does not.
        let out = parse("one\ntwo");
        assert_eq!(lines(&out), ["one", "two"]);
    }

    #[test]
    fn a_blank_line_survives_as_a_gap() {
        let out = parse("one\n\ntwo");
        assert_eq!(lines(&out), ["one", "", "two"]);
        assert!(matches!(out[1], Block::Gap), "{out:?}");
    }

    #[test]
    fn two_blank_lines_are_still_one_gap() {
        let out = parse("one\n\n\n\ntwo");
        assert_eq!(out.iter().filter(|b| matches!(b, Block::Gap)).count(), 1, "{out:?}");
    }

    #[test]
    fn a_heading_keeps_the_level_it_was_written_at() {
        let out = parse("# Title\nand words");
        // The newline rule above makes these one paragraph's worth of source,
        // but a heading ends at its own line either way.
        assert!(matches!(out[0], Block::Heading { level: 1, .. }), "{out:?}");
        assert_eq!(lines(&out), ["Title", "and words"]);
    }

    #[test]
    fn every_heading_level_arrives_including_the_deep_ones() {
        // The reader this replaces stopped at three and showed `#### deep` with
        // its hashes on. A page has room for six.
        let out = parse("#### deep");
        assert!(matches!(out[0], Block::Heading { level: 4, .. }), "{out:?}");
        assert_eq!(lines(&out), ["deep"]);
    }

    #[test]
    fn a_bullet_list_is_a_list() {
        let out = parse("- one\n- two");
        let Block::List { ordered: false, items } = &out[0] else { panic!("{out:?}") };
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].marker, Marker::Bullet);
        assert_eq!(lines(&items[0].blocks), ["one"]);
        assert_eq!(lines(&items[1].blocks), ["two"]);
    }

    #[test]
    fn a_numbered_list_keeps_the_numbers_that_were_typed() {
        let out = parse("3. third\n4. fourth");
        let Block::List { ordered: true, items } = &out[0] else { panic!("{out:?}") };
        assert_eq!(items[0].marker, Marker::Number("3".into()));
        assert_eq!(items[1].marker, Marker::Number("4".into()));
    }

    #[test]
    fn a_task_shows_whether_it_is_done() {
        let out = parse("- [ ] open\n- [x] shut");
        let Block::List { items, .. } = &out[0] else { panic!("{out:?}") };
        assert_eq!(items[0].marker, Marker::Task(false));
        assert_eq!(items[1].marker, Marker::Task(true));
        assert_eq!(lines(&items[0].blocks), ["open"]);
    }

    #[test]
    fn a_list_inside_a_list_is_a_list_inside_a_list() {
        // The whole reason for a tree rather than a line reader: the old one
        // counted leading spaces and hoped.
        let out = parse("- one\n  - inner\n- two");
        let Block::List { items, .. } = &out[0] else { panic!("{out:?}") };
        assert_eq!(items.len(), 2, "{items:?}");
        assert!(
            items[0].blocks.iter().any(|b| matches!(b, Block::List { .. })),
            "{:?}",
            items[0].blocks
        );
    }

    #[test]
    fn a_quote_holds_blocks_rather_than_a_line() {
        let out = parse("> borrowed\n> words");
        let Block::Quote(inner) = &out[0] else { panic!("{out:?}") };
        assert_eq!(lines(inner), ["borrowed", "words"]);
    }

    #[test]
    fn a_rule_is_a_rule() {
        assert_eq!(parse("---"), [Block::Rule]);
    }

    #[test]
    fn a_fence_is_taken_exactly_as_typed_and_keeps_its_language() {
        let out = parse("```rust\nlet **x** = 1;\n```");
        let Block::Code { language, lines } = &out[0] else { panic!("{out:?}") };
        assert_eq!(language.as_deref(), Some("rust"));
        assert_eq!(lines, &["let **x** = 1;".to_string()], "the asterisks are characters here");
    }

    #[test]
    fn a_link_keeps_its_words_and_its_address() {
        let out = parse("see [the docs](https://example.invalid/x)");
        let Block::Paragraph(runs) = &out[0] else { panic!("{out:?}") };
        assert_eq!(said(&runs[0]), "see the docs");
        let link = runs[0].iter().find(|s| s.style.link).expect("{runs:?}");
        assert_eq!(link.text, "the docs");
        assert_eq!(link.href.as_deref(), Some("https://example.invalid/x"));
    }

    #[test]
    fn a_bracket_that_is_not_a_link_is_left_alone() {
        assert_eq!(lines(&parse("a [note] here")), ["a [note] here"]);
    }

    #[test]
    fn a_table_arrives_with_its_pipes_off() {
        let out = parse("| a | b |\n|---|--:|\n| 1 | 2 |");
        let Block::Table(table) = &out[0] else { panic!("{out:?}") };
        assert_eq!(table.align, [Align::Ragged, Align::Right]);
        assert_eq!(table.head.iter().map(said).collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.rows[0].iter().map(said).collect::<Vec<_>>(), ["1", "2"]);
    }

    #[test]
    fn html_is_shown_rather_than_obeyed() {
        let out = parse("a <b>tag</b> here");
        assert_eq!(lines(&out), ["a <b>tag</b> here"]);
    }

    #[test]
    fn nothing_at_all_is_nothing_at_all() {
        assert_eq!(parse(""), []);
    }

    #[test]
    fn a_note_that_is_only_whitespace_does_not_panic() {
        parse("   \n\n\t\n");
    }
}
