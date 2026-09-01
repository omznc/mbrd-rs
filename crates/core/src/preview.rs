//! What a card can be *shown* as, and what about it can be *changed*.
//!
//! Both questions are asked by the window that opens when somebody
//! double-clicks a card (`opened.rs` in the UI crate), and neither of them is a
//! question about drawing. "A `.rs` file is source, set in a fixed-width face"
//! and "a swatch has exactly one editable field and it is the colour" are facts
//! about an item and its bytes; they can be decided, and tested, with no window
//! anywhere near them. So they are decided here.
//!
//! ## The two rules this module exists to keep
//!
//! **Anything that can be shown is shown.** If the bytes can be turned into a
//! picture, a page, a table or a listing, [`of`] says so. A grey rectangle over
//! bytes this build could have read is a bug, not a level of detail — which is
//! why the text arm ends with a check of the bytes themselves rather than a
//! list of extensions: a file called `.notes` that happens to be UTF-8 is text,
//! and there is no reason to pretend otherwise.
//!
//! **Anything that can be changed is changeable.** [`editable`] never returns
//! an empty list. Every card has a name, most have more, and the list comes
//! back **principal first** — the thing somebody pressing Edit meant, which for
//! a note is its words, for a link its address, and for a photograph the only
//! thing a photograph has.
//!
//! ## What is deliberately not here
//!
//! No decoding. This module reads an extension, sniffs for UTF-8, and walks a
//! ZIP central directory; it does not open an image, a video or a font. The
//! variants it returns for those say *what to try*, and the crate that owns the
//! decoder is the one that finds out whether it worked. That keeps this fast
//! enough to call on every frame, which is what it is called on.

use crate::mbrd::Asset;
use crate::model::{Item, ItemType};

/// The longest text this build will open for typing.
///
/// A real limit rather than a shrug: an editor is a `String` with a caret in it
/// and every keystroke re-wraps the whole of it, so a field that is quick at
/// ten thousand characters is not quick at ten million. Two hundred thousand is
/// a long README and a short novel. Past it a file still opens for *reading* —
/// the preview never has a limit — and only the Edit button goes grey.
pub const TEXT_MAX: usize = 200_000;

/// How many rows of a spreadsheet are worth building elements for.
///
/// A table is read at the top; nobody scrolls to row forty thousand of a CSV in
/// a moodboard, and a laid-out grid that large is a frame nobody gets back.
pub const ROWS_MAX: usize = 2_000;

/// And how many names out of an archive.
pub const ENTRIES_MAX: usize = 4_000;

/// How a card's contents should be put in front of somebody.
///
/// Not a file type — several file types land on the same variant, and the same
/// file type lands on different ones depending on what the card claims to be.
/// It is the answer to "what does the page draw", and nothing else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Preview {
    /// Markdown, set as a page. See [`crate::markdown`].
    Document,
    /// Text, set in a fixed-width face, one line per line. `language` is what
    /// to call it — and, eventually, what to highlight it as.
    Source { language: Option<&'static str> },
    /// Delimited text, set as a real grid rather than as its own source.
    Sheet { separator: char },
    /// Raster bytes, drawn to fit.
    Picture,
    /// Vector bytes, rasterised and drawn to fit — the same as [`Self::Picture`]
    /// on screen, and a distinct variant anyway, because the two are decoded by
    /// different code in the crate that draws (`resvg` against the one, `image`
    /// against the other) and a shared variant would hide which.
    Vector,
    /// A moving picture, once there is something behind the playhead.
    Video,
    /// A recording, drawn as its waveform.
    Audio,
    /// A colour, large.
    Colour,
    /// A web address, which is the one thing a link card is too small to show.
    Address,
    /// A ZIP, drawn as the list of what is inside it. See [`listing`].
    Archive,
    /// A PDF, shown as the text pulled out of it. Unlike [`Self::Source`], what
    /// is on screen is not what is in the file: getting from one to the other
    /// means walking compressed content streams and font encodings, which is
    /// decoding, not sniffing — so it happens in the crate that draws, the same
    /// division `Vector` keeps with `resvg`. Read-only for the same reason: the
    /// text is derived, and there is nowhere honest to write an edit back to.
    Pdf,
    /// A font, shown as a specimen set in the face it drops onto the board —
    /// which means the bytes have to be handed to a live text system before
    /// anything can be drawn, so, like [`Self::Pdf`] and [`Self::Vector`],
    /// this is a promise about *what to try*, not a decoded result.
    Font,
    /// A mesh, rasterised to a flat-shaded still — see [`crate::mesh`]. A
    /// promise about what to try, the same way [`Self::Vector`] is: the bytes
    /// are a binary STL, an OBJ or a GLB, not yet a picture, and the crate
    /// that draws finds out whether the triangle count was one this build
    /// will spend a background-executor slot rasterising.
    Mesh,
    /// Nothing this build can draw. **Not a failure**: the page still opens,
    /// onto the facts, which for an FBX or a `.heic` is genuinely all that is
    /// known. See the module header of `opened.rs`.
    Nothing,
}

/// One thing about a card that can be typed into.
///
/// The list [`editable`] returns is ordered principal-first, and the page's one
/// Edit button starts on `[0]`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Editable {
    /// The words: a typed note's own text, or the whole of the file behind a
    /// card that came from one. `limit` is which of those it is.
    Text { limit: usize },
    /// A swatch's colour. Written as the card's *name*, because in this format
    /// a swatch's name and its `meta.hex` are the same value — see `write_field`
    /// in `board_view.rs`. Named separately here so the page can label the row
    /// what it actually is.
    Hex,
    /// A link's address.
    Url,
    /// The label on the card. Every card has one, so this is always last and
    /// always present.
    Name,
}

/// What to draw for this card.
pub fn of(item: &Item, asset: Option<&Asset>) -> Preview {
    let lowered = asset.map(|asset| asset.ext.to_ascii_lowercase());
    let ext = lowered.as_deref().unwrap_or("");

    match item.kind {
        // The three that are their own content and never have bytes.
        ItemType::Swatch => Preview::Colour,
        ItemType::Link => Preview::Address,
        // Furniture. A title, a fence and a style tile are things the board is
        // drawn *around*; there is no "inside" to open onto. `Gone` is an item
        // whose bytes were emptied out of the bin and can never come back.
        ItemType::Title | ItemType::Ghost | ItemType::Fence => Preview::Nothing,
        ItemType::StyleTile | ItemType::Gone => Preview::Nothing,

        ItemType::Video => Preview::Video,
        ItemType::Audio => Preview::Audio,

        ItemType::Image | ItemType::Sticker => match asset {
            None => Preview::Nothing,
            // Rasterised by `resvg`, in the crate that draws: this module
            // says what to try and never opens a decoder itself.
            // The four below are a different case entirely: real pictures,
            // correctly named, that no decoder in this tree can open.
            Some(_) if ext == "svg" => Preview::Vector,
            Some(_) if UNREADABLE.contains(&ext) => Preview::Nothing,
            Some(_) => Preview::Picture,
        },

        // A note with no file behind it is its own words, and its words are
        // Markdown. A note that came from a file is whatever that file is —
        // dropping a `.csv` on the board should not make it a paragraph.
        ItemType::Note | ItemType::Text => match asset {
            None => Preview::Document,
            Some(asset) => bytes(ext, asset).unwrap_or(Preview::Document),
        },

        // `Generic` is where every unrecognised file lands, and `Other` is a
        // type some other build wrote. Both are worth looking inside.
        ItemType::Model | ItemType::Generic | ItemType::Other(_) => match asset {
            None => Preview::Nothing,
            Some(asset) => bytes(ext, asset).unwrap_or(Preview::Nothing),
        },
    }
}

/// What the bytes themselves turn out to be, for a card whose type did not
/// already settle it. `None` means nothing here could read them.
fn bytes(ext: &str, asset: &Asset) -> Option<Preview> {
    if is_zip(&asset.bytes) || is_gzip(&asset.bytes) || is_tar(&asset.bytes) {
        return Some(Preview::Archive);
    }
    if is_pdf(&asset.bytes) {
        return Some(Preview::Pdf);
    }
    if is_font(&asset.bytes) {
        return Some(Preview::Font);
    }
    if crate::mesh::is_stl(&asset.bytes) || crate::mesh::is_glb(&asset.bytes) {
        return Some(Preview::Mesh);
    }
    // Gated on the extension, unlike the checks above: an OBJ has no magic
    // bytes of its own to trust, only a shape a `.csv` or a `.txt` could
    // share by accident — see `mesh::is_obj`'s own doc for why it wants the
    // name checked first.
    if ext == "obj" && crate::mesh::is_obj(&asset.bytes) {
        return Some(Preview::Mesh);
    }
    if let Some(separator) = separator(ext) {
        return Some(Preview::Sheet { separator });
    }
    // A picture on a card that does not claim to be one. It happens: a card's
    // type can be changed after the fact, and an import that fell back to
    // `Generic` on a name it did not know is still holding a PNG.
    if READABLE.contains(&ext) {
        return Some(Preview::Picture);
    }
    if !readable_text(&asset.bytes) {
        return None;
    }
    Some(match DOCUMENT.contains(&ext) {
        true => Preview::Document,
        false => Preview::Source { language: language(ext) },
    })
}

/// Everything about this card that can be typed into, principal first.
///
/// Never empty. The page's Edit button is never grey for want of something to
/// change — only for a file too long to hold in an editor, which is the page's
/// own decision and not this one.
pub fn editable(item: &Item, asset: Option<&Asset>) -> Vec<Editable> {
    // A swatch is the one card with exactly one field, and it is not `Name`
    // even though that is where it is stored: typing a colour into a swatch is
    // the whole colour picker this build has, and calling that row "Name" would
    // hide it.
    if item.kind == ItemType::Swatch {
        return vec![Editable::Hex];
    }

    let mut out = Vec::with_capacity(3);
    // Anything that reads as words can be typed as words — which is a wider
    // claim than "is a note". A `.toml` on the board is editable here, and the
    // limit says whether the edit lands in `meta.text` or in the archive.
    if let Some(limit) = writable(item, asset) {
        out.push(Editable::Text { limit });
    }
    if item.kind == ItemType::Link {
        out.push(Editable::Url);
    }
    out.push(Editable::Name);
    out
}

/// The cap on this card's text, where its text can be typed into at all.
///
/// One limit, [`TEXT_MAX`], and it is the editor's rather than the board's: it
/// says what a `String` with a caret in it can be asked to hold, not what
/// `meta.text` may carry. A note used to be held to
/// [`NOTE_MAX`](crate::model::NOTE_MAX) here because
/// the board's copy was its *only* copy; now every note is a Markdown file
/// underneath — its words move to an asset the moment they outgrow the head —
/// so the cap on `meta.text` is the committer's business, not the typist's.
fn writable(item: &Item, asset: Option<&Asset>) -> Option<usize> {
    match of(item, asset) {
        // A vector is drawn as a picture but is still, underneath, the XML that
        // makes one — and that stays true and stays editable even once it can
        // also be looked at rather than read.
        Preview::Document | Preview::Source { .. } | Preview::Sheet { .. } | Preview::Vector => {
            Some(TEXT_MAX)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// What an extension claims
// ---------------------------------------------------------------------------

/// Set as a page rather than as its own source.
const DOCUMENT: &[&str] = &["md", "markdown", "mdown", "mkd", "mdx"];

/// Raster formats the `image` crate in this tree can actually decode.
const READABLE: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tif", "tiff", "ico", "tga", "qoi", "exr", "hdr",
    "dds", "pnm", "ppm", "pgm", "pbm", "ff",
];

/// Raster formats that are real pictures and that nothing in this tree can
/// open. AVIF needs `dav1d`, a C dependency; HEIC and JPEG XL are not supported
/// by the `image` crate at all. A card holding one of these opens onto its
/// facts rather than onto a frame that will never arrive.
pub const UNREADABLE: &[&str] = &["avif", "heic", "heif", "jxl"];

/// What to call a fixed-width preview, by extension.
///
/// A label, not a lexer. Nothing highlights yet, and when something does this
/// is the string it will look up — which is why it is a table rather than a
/// match arm returning `&str` from thin air.
const LANGUAGES: &[(&str, &str)] = &[
    ("rs", "Rust"),
    ("ts", "TypeScript"),
    ("tsx", "TypeScript"),
    ("js", "JavaScript"),
    ("mjs", "JavaScript"),
    ("cjs", "JavaScript"),
    ("jsx", "JavaScript"),
    ("py", "Python"),
    ("go", "Go"),
    ("c", "C"),
    ("h", "C"),
    ("cpp", "C++"),
    ("cxx", "C++"),
    ("cc", "C++"),
    ("hpp", "C++"),
    ("cs", "C#"),
    ("java", "Java"),
    ("kt", "Kotlin"),
    ("kts", "Kotlin"),
    ("swift", "Swift"),
    ("rb", "Ruby"),
    ("php", "PHP"),
    ("pl", "Perl"),
    ("sh", "Shell"),
    ("bash", "Shell"),
    ("zsh", "Shell"),
    ("fish", "Shell"),
    ("ps1", "PowerShell"),
    ("lua", "Lua"),
    ("sql", "SQL"),
    ("jl", "Julia"),
    ("hs", "Haskell"),
    ("ml", "OCaml"),
    ("ex", "Elixir"),
    ("exs", "Elixir"),
    ("erl", "Erlang"),
    ("clj", "Clojure"),
    ("scala", "Scala"),
    ("dart", "Dart"),
    ("zig", "Zig"),
    ("nim", "Nim"),
    ("el", "Emacs Lisp"),
    ("vim", "Vim script"),
    ("html", "HTML"),
    ("htm", "HTML"),
    ("css", "CSS"),
    ("scss", "Sass"),
    ("sass", "Sass"),
    ("less", "Less"),
    ("json", "JSON"),
    ("jsonc", "JSON"),
    ("toml", "TOML"),
    ("lock", "TOML"),
    ("yaml", "YAML"),
    ("yml", "YAML"),
    ("xml", "XML"),
    ("svg", "SVG"),
    ("ini", "INI"),
    ("cfg", "INI"),
    ("conf", "INI"),
    ("diff", "Diff"),
    ("patch", "Diff"),
    ("tf", "Terraform"),
    ("proto", "Protobuf"),
    ("graphql", "GraphQL"),
    ("gradle", "Gradle"),
    ("dockerfile", "Dockerfile"),
    ("makefile", "Make"),
    ("mk", "Make"),
    ("tex", "TeX"),
    ("bib", "BibTeX"),
    // Text-format 3D: this build has no rasteriser for these, so the text arm
    // shows them as source — unlabelled otherwise, since none of the other
    // entries above are a 3D format's own extension.
    ("obj", "Wavefront OBJ"),
    ("gltf", "glTF"),
    ("dae", "COLLADA"),
    ("stl", "STL"),
];

/// What a fixed-width preview of this extension should be called, where there
/// is a name worth putting on it.
pub fn language(ext: &str) -> Option<&'static str> {
    let ext = ext.to_ascii_lowercase();
    LANGUAGES.iter().find(|(name, _)| *name == ext).map(|(_, label)| *label)
}

/// Which character separates the columns of this extension, if any does.
pub fn separator(ext: &str) -> Option<char> {
    match ext {
        "csv" => Some(','),
        "tsv" | "tab" => Some('\t'),
        _ => None,
    }
}

/// Whether these bytes are text somebody could read.
///
/// Two conditions, and the second is the one that matters. Valid UTF-8 is not
/// enough on its own — a great many binary files are accidentally valid UTF-8
/// for their whole length, and a `.wasm` set in a fixed-width face is a page of
/// noise. A NUL byte is what almost every binary format has and almost no text
/// file does, so it is the tiebreak, and it is the same one `file(1)` and every
/// diff tool ever written have used.
pub fn readable_text(bytes: &[u8]) -> bool {
    !bytes.contains(&0) && std::str::from_utf8(bytes).is_ok()
}

// ---------------------------------------------------------------------------
// Delimited text
// ---------------------------------------------------------------------------

/// Cut delimited text into rows and cells.
///
/// RFC 4180 as far as it goes: a cell may be quoted, a quoted cell may contain
/// the separator and newlines, and `""` inside one is a literal quote. What it
/// deliberately does not do is fail — a malformed CSV is still shown, as
/// whatever it parsed to, because the alternative is an error message where a
/// person expected to see their spreadsheet.
///
/// Capped at [`ROWS_MAX`]. A caller that cares that it was cut short can
/// compare the length.
pub fn rows(text: &str, separator: char) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut row: Vec<String> = Vec::new();
    let mut cell = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();

    while let Some(c) = chars.next() {
        if quoted {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    cell.push('"');
                }
                '"' => quoted = false,
                _ => cell.push(c),
            }
            continue;
        }
        match c {
            // Only at the *start* of a cell, so a stray quote in the middle of
            // an unquoted word stays a character rather than swallowing the
            // rest of the file.
            '"' if cell.is_empty() => quoted = true,
            _ if c == separator => row.push(std::mem::take(&mut cell)),
            '\r' => {}
            '\n' => {
                row.push(std::mem::take(&mut cell));
                out.push(std::mem::take(&mut row));
                if out.len() >= ROWS_MAX {
                    return out;
                }
            }
            _ => cell.push(c),
        }
    }
    // A file that ends without a newline still ends with a row; one that ends
    // *with* a newline does not gain an empty one.
    if !cell.is_empty() || !row.is_empty() {
        row.push(cell);
        out.push(row);
    }
    out
}

// ---------------------------------------------------------------------------
// Archives
// ---------------------------------------------------------------------------

/// Whether these bytes are a ZIP container, asked of the bytes rather than of
/// the name. `docx`, `xlsx`, `pptx`, `epub`, `sketch`, `3mf` and `usdz` are all
/// ZIPs wearing another name, and a `.zip` renamed by a download manager is
/// still one once its extension no longer says so — so this is what a card
/// is checked against, not `ext == "zip"`.
fn is_zip(bytes: &[u8]) -> bool {
    bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06")
}

/// Whether these bytes are gzip-compressed, regardless of what `.gz` or
/// `.tgz` the name claims. Unpacked in [`listing`] on the assumption a
/// gzipped board asset is a gzipped tarball — the one shape this build reads.
fn is_gzip(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x1f, 0x8b])
}

/// Whether these bytes are a POSIX (`ustar`) tar, sniffed rather than trusted
/// from the name for the same reason [`is_zip`] is: the magic sits at a fixed
/// offset in every header a POSIX tar writer produces, so there is a real
/// check here rather than a guess from `.tar`.
fn is_tar(bytes: &[u8]) -> bool {
    bytes.len() > 262 && &bytes[257..262] == b"ustar"
}

/// Whether these bytes are a PDF. The header is `%PDF-` at the very front in
/// every real-world PDF — a scanner may pad the file before it, which is why
/// this looks in the first kilobyte rather than only at offset zero.
fn is_pdf(bytes: &[u8]) -> bool {
    let window = &bytes[..bytes.len().min(1024)];
    window.windows(5).any(|w| w == b"%PDF-")
}

/// Whether these bytes are a raw SFNT font — TrueType, OpenType or a TTC
/// collection. Not WOFF or WOFF2: both are a compressed *wrapper* around this
/// shape rather than this shape itself, and unwrapping one is a second
/// dependency, not the one this build already carries for the name table. A
/// `.woff` still arrives as a named card rather than a lie about what could
/// be drawn from it — see the module header.
fn is_font(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0x00, 0x01, 0x00, 0x00])
        || bytes.starts_with(b"OTTO")
        || bytes.starts_with(b"true")
        || bytes.starts_with(b"ttcf")
}

/// One name out of an archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    /// Uncompressed, in bytes. `0` for a directory.
    pub size: u64,
    pub folder: bool,
}

/// What is inside a ZIP, a tar or a gzipped tar, without unpacking any of it.
///
/// For a ZIP this is the central directory only, which is why it is cheap
/// enough to call on a hundred-megabyte archive: no entry is decompressed and
/// none is read past its header. A tar carries no such index — every header
/// has to be walked to find the next one — so a gzipped tar this large is
/// genuinely read, not just peeked at. An archive that will not open at all,
/// in any of the three shapes, comes back empty rather than as an error — the
/// page above has a perfectly good thing to say about a file it cannot read,
/// which is its name, its size and its hash.
pub fn listing(bytes: &[u8]) -> Vec<Entry> {
    if let Ok(mut zip) = zip::ZipArchive::new(std::io::Cursor::new(bytes)) {
        let mut out = Vec::new();
        for n in 0..zip.len().min(ENTRIES_MAX) {
            let Ok(entry) = zip.by_index(n) else { continue };
            out.push(Entry {
                path: entry.name().to_string(),
                size: entry.size(),
                folder: entry.is_dir(),
            });
        }
        return out;
    }
    if is_gzip(bytes) {
        return tar_listing(flate2::read::GzDecoder::new(bytes));
    }
    tar_listing(bytes)
}

/// The shared walk behind both tar branches of [`listing`]: a plain tar and a
/// gzip-wrapped one differ only in which reader sits underneath.
fn tar_listing(reader: impl std::io::Read) -> Vec<Entry> {
    let mut tar = tar::Archive::new(reader);
    let Ok(entries) = tar.entries() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.take(ENTRIES_MAX) {
        let Ok(entry) = entry else { break };
        let Ok(path) = entry.path() else { continue };
        out.push(Entry {
            path: path.to_string_lossy().into_owned(),
            size: entry.size(),
            folder: entry.header().entry_type().is_dir(),
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(ext: &str, bytes: &[u8]) -> Asset {
        Asset { bytes: bytes.to_vec(), ext: ext.into(), label: "file".into() }
    }

    fn with(kind: ItemType, ext: &str, bytes: &[u8]) -> (Item, Asset) {
        (Item::new("a", kind), asset(ext, bytes))
    }

    #[test]
    fn a_typed_note_is_a_document_and_a_dropped_markdown_file_is_too() {
        let bare = Item::new("a", ItemType::Note);
        assert_eq!(of(&bare, None), Preview::Document);

        let (item, file) = with(ItemType::Note, "md", b"# hello");
        assert_eq!(of(&item, Some(&file)), Preview::Document);
    }

    #[test]
    fn a_note_that_came_from_a_spreadsheet_is_a_spreadsheet() {
        // Dropping a `.csv` on the board classifies it as text, which must not
        // then mean it is read as a paragraph.
        let (item, file) = with(ItemType::Note, "csv", b"a,b\n1,2");
        assert_eq!(of(&item, Some(&file)), Preview::Sheet { separator: ',' });
    }

    #[test]
    fn source_keeps_the_name_of_its_language() {
        let (item, file) = with(ItemType::Generic, "rs", b"fn main() {}");
        assert_eq!(of(&item, Some(&file)), Preview::Source { language: Some("Rust") });
    }

    #[test]
    fn a_file_nobody_has_a_name_for_is_still_shown_if_it_reads_as_text() {
        // The whole point of the byte check: an extension nothing knows, over
        // bytes anybody could read.
        let (item, file) = with(ItemType::Generic, "xyzzy", b"plain words");
        assert_eq!(of(&item, Some(&file)), Preview::Source { language: None });
    }

    #[test]
    fn a_binary_that_happens_to_be_valid_utf8_is_not_offered_as_text() {
        let (item, file) = with(ItemType::Generic, "bin", b"MZ\0\0\x90rubbish");
        assert_eq!(of(&item, Some(&file)), Preview::Nothing);
    }

    #[test]
    fn a_picture_this_build_cannot_open_says_so_instead_of_showing_a_frame() {
        for ext in UNREADABLE {
            let (item, file) = with(ItemType::Image, ext, b"\0\0\0 whatever");
            assert_eq!(of(&item, Some(&file)), Preview::Nothing, "{ext}");
        }
        let (item, file) = with(ItemType::Image, "png", b"\x89PNG");
        assert_eq!(of(&item, Some(&file)), Preview::Picture);
    }

    #[test]
    fn an_svg_is_a_vector_and_still_holds_its_source_to_edit() {
        let (item, file) = with(ItemType::Image, "svg", b"<svg/>");
        assert_eq!(of(&item, Some(&file)), Preview::Vector);
        assert_eq!(editable(&item, Some(&file))[0], Editable::Text { limit: TEXT_MAX });
    }

    #[test]
    fn a_zip_is_a_listing_and_furniture_is_nothing() {
        let (item, file) = with(ItemType::Generic, "zip", b"PK\x03\x04");
        assert_eq!(of(&item, Some(&file)), Preview::Archive);
        assert_eq!(of(&Item::new("a", ItemType::Fence), None), Preview::Nothing);
        assert_eq!(of(&Item::new("a", ItemType::Title), None), Preview::Nothing);
    }

    #[test]
    fn a_zip_wearing_another_name_is_still_a_listing() {
        // `docx`, `xlsx`, `pptx`, `epub`, `sketch`, `3mf` and `usdz` are all
        // ZIP containers; the check must ask the bytes, not the extension.
        for ext in ["docx", "xlsx", "pptx", "epub", "sketch", "3mf", "usdz"] {
            let (item, file) = with(ItemType::Generic, ext, b"PK\x03\x04rest of a real zip");
            assert_eq!(of(&item, Some(&file)), Preview::Archive, "{ext}");
        }
        // And a genuinely empty ZIP, whose local file header is skipped in
        // favour of going straight to the end-of-central-directory record.
        let (item, file) =
            with(ItemType::Generic, "epub", b"PK\x05\x06\0\0\0\0\0\0\0\0\0\0\0\0\0\0");
        assert_eq!(of(&item, Some(&file)), Preview::Archive);
    }

    #[test]
    fn a_pdf_is_shown_by_its_own_text_and_not_editable() {
        let (item, file) = with(ItemType::Generic, "pdf", b"%PDF-1.4\nrest of a real pdf");
        assert_eq!(of(&item, Some(&file)), Preview::Pdf);
        assert_eq!(
            editable(&item, Some(&file)),
            vec![Editable::Name],
            "no bytes to write text back to"
        );
    }

    #[test]
    fn a_raw_sfnt_font_is_a_specimen_and_a_wrapped_one_is_not_yet() {
        for magic in [[0x00, 0x01, 0x00, 0x00], *b"OTTO", *b"true", *b"ttcf"] {
            let (item, file) = with(ItemType::Generic, "ttf", &magic);
            assert_eq!(of(&item, Some(&file)), Preview::Font, "{magic:?}");
        }
        // WOFF and WOFF2 wrap the same shape under compression this build does
        // not undo, so they fall through to whatever the bytes otherwise read
        // as — here, nothing at all.
        let (item, file) = with(ItemType::Generic, "woff", b"wOFF\0\x01\0\0rest");
        assert_eq!(of(&item, Some(&file)), Preview::Nothing);
    }

    #[test]
    fn a_binary_stl_is_a_mesh_and_not_editable() {
        let mut bytes = vec![0_u8; 84 + 50];
        bytes[80..84].copy_from_slice(&1_u32.to_le_bytes());
        let (item, file) = with(ItemType::Model, "stl", &bytes);
        assert_eq!(of(&item, Some(&file)), Preview::Mesh);
        assert_eq!(editable(&item, Some(&file)), vec![Editable::Name]);
    }

    #[test]
    fn a_glb_is_a_mesh_regardless_of_extension() {
        let json = br#"{"bufferViews":[],"accessors":[],"meshes":[]}"#;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"glTF");
        bytes.extend_from_slice(&2_u32.to_le_bytes());
        bytes.extend_from_slice(&(12 + 8 + json.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&(json.len() as u32).to_le_bytes());
        bytes.extend_from_slice(b"JSON");
        bytes.extend_from_slice(json);
        let (item, file) = with(ItemType::Model, "glb", &bytes);
        assert_eq!(of(&item, Some(&file)), Preview::Mesh);
    }

    #[test]
    fn an_obj_is_a_mesh_only_when_the_extension_says_so_too() {
        let (item, file) = with(ItemType::Model, "obj", b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n");
        assert_eq!(of(&item, Some(&file)), Preview::Mesh);

        // The same bytes under a different name are not sniffed into being
        // one — see `mesh::is_obj`'s own doc for why this check alone is not
        // trusted the way `is_stl` and `is_glb` are.
        let (item, file) = with(ItemType::Generic, "md", b"v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n");
        assert_eq!(of(&item, Some(&file)), Preview::Document);
    }

    #[test]
    fn every_card_has_something_to_change() {
        // The rule this module exists to keep. Nothing comes back empty.
        for kind in [
            ItemType::Image,
            ItemType::Video,
            ItemType::Audio,
            ItemType::Note,
            ItemType::Link,
            ItemType::Text,
            ItemType::Model,
            ItemType::Title,
            ItemType::Ghost,
            ItemType::Swatch,
            ItemType::Sticker,
            ItemType::Fence,
            ItemType::StyleTile,
            ItemType::Gone,
            ItemType::Generic,
            ItemType::Other("something-else".into()),
        ] {
            let item = Item::new("a", kind.clone());
            assert!(!editable(&item, None).is_empty(), "{kind:?} has nothing to type into");
        }
    }

    #[test]
    fn the_principal_field_comes_first() {
        let note = Item::new("a", ItemType::Note);
        assert_eq!(editable(&note, None)[0], Editable::Text { limit: TEXT_MAX });

        let link = Item::new("a", ItemType::Link);
        assert_eq!(editable(&link, None), vec![Editable::Url, Editable::Name]);

        let picture = Item::new("a", ItemType::Image);
        assert_eq!(editable(&picture, None), vec![Editable::Name]);
    }

    #[test]
    fn a_swatch_has_one_field_and_it_is_the_colour() {
        let swatch = Item::new("a", ItemType::Swatch);
        assert_eq!(editable(&swatch, None), vec![Editable::Hex]);
    }

    #[test]
    fn a_card_that_came_from_a_file_is_held_to_the_files_limit() {
        let (item, file) = with(ItemType::Note, "md", b"# hello");
        assert_eq!(editable(&item, Some(&file))[0], Editable::Text { limit: TEXT_MAX });
    }

    #[test]
    fn an_archive_is_shown_but_not_typed_into() {
        let (item, file) = with(ItemType::Generic, "zip", b"PK\x03\x04");
        assert_eq!(editable(&item, Some(&file)), vec![Editable::Name]);
    }

    #[test]
    fn a_row_of_a_spreadsheet_is_its_cells() {
        assert_eq!(rows("a,b,c", ','), vec![vec!["a", "b", "c"]]);
        assert_eq!(rows("a\tb", '\t'), vec![vec!["a", "b"]]);
    }

    #[test]
    fn a_quoted_cell_may_hold_the_separator_and_a_newline() {
        let out = rows("one,\"two, and\nmore\",three", ',');
        assert_eq!(out, vec![vec!["one", "two, and\nmore", "three"]]);
    }

    #[test]
    fn two_quotes_inside_a_quoted_cell_are_one_quote() {
        assert_eq!(rows("\"she said \"\"no\"\"\"", ','), vec![vec!["she said \"no\""]]);
    }

    #[test]
    fn a_trailing_newline_does_not_add_an_empty_row() {
        assert_eq!(rows("a,b\nc,d\n", ','), vec![vec!["a", "b"], vec!["c", "d"]]);
        assert_eq!(rows("a,b\nc,d", ','), vec![vec!["a", "b"], vec!["c", "d"]]);
    }

    #[test]
    fn an_empty_cell_is_still_a_cell() {
        assert_eq!(rows("a,,c", ','), vec![vec!["a", "", "c"]]);
        assert_eq!(rows(",", ','), vec![vec!["", ""]]);
    }

    #[test]
    fn windows_line_endings_do_not_end_up_in_the_cells() {
        assert_eq!(rows("a,b\r\nc,d\r\n", ','), vec![vec!["a", "b"], vec!["c", "d"]]);
    }

    #[test]
    fn an_archive_that_will_not_open_is_empty_rather_than_an_error() {
        assert!(listing(b"not a zip at all").is_empty());
    }

    #[test]
    fn an_archive_gives_up_its_names_without_being_unpacked() {
        use std::io::Write;
        let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
        zip.add_directory("pictures/", options).unwrap();
        zip.start_file("pictures/one.png", options).unwrap();
        zip.write_all(b"0123456789").unwrap();
        let packed = zip.finish().unwrap().into_inner();

        let out = listing(&packed);
        assert_eq!(out.len(), 2);
        assert!(out[0].folder, "a directory entry is marked as one");
        assert_eq!(out[1].path, "pictures/one.png");
        assert_eq!(out[1].size, 10, "the size is the uncompressed one");
    }

    fn packed_tar() -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        let mut dir = tar::Header::new_gnu();
        dir.set_entry_type(tar::EntryType::Directory);
        dir.set_size(0);
        dir.set_cksum();
        builder.append_data(&mut dir, "pictures/", std::io::empty()).unwrap();

        let mut file = tar::Header::new_gnu();
        file.set_size(10);
        file.set_cksum();
        builder.append_data(&mut file, "pictures/one.png", b"0123456789".as_slice()).unwrap();
        builder.into_inner().unwrap()
    }

    #[test]
    fn a_tar_gives_up_its_names_the_same_way_a_zip_does() {
        let packed = packed_tar();
        assert!(is_tar(&packed));

        let out = listing(&packed);
        assert_eq!(out.len(), 2);
        assert!(out[0].folder, "a directory entry is marked as one");
        assert_eq!(out[1].path, "pictures/one.png");
        assert_eq!(out[1].size, 10);
    }

    #[test]
    fn a_gzipped_tar_is_read_through_the_gzip_first() {
        use std::io::Write;
        let packed = packed_tar();
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&packed).unwrap();
        let gzipped = gz.finish().unwrap();

        assert!(is_gzip(&gzipped));
        let out = listing(&gzipped);
        assert_eq!(out.len(), 2);
        assert_eq!(out[1].path, "pictures/one.png");
        assert_eq!(out[1].size, 10);
    }

    #[test]
    fn a_tar_or_a_gzipped_tar_previews_as_an_archive() {
        let (item, tarred) = with(ItemType::Generic, "tar", &packed_tar());
        assert_eq!(of(&item, Some(&tarred)), Preview::Archive);

        use std::io::Write;
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        gz.write_all(&packed_tar()).unwrap();
        let (item, gzipped) = with(ItemType::Generic, "tgz", &gz.finish().unwrap());
        assert_eq!(of(&item, Some(&gzipped)), Preview::Archive);
    }

    #[test]
    fn a_language_is_looked_up_whatever_the_case_of_the_name() {
        assert_eq!(language("RS"), Some("Rust"));
        assert_eq!(language("nothing-in-particular"), None);
    }
}
