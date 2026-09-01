//! Getting somebody else's files onto the board.
//!
//! A drop or a paste arrives as bytes and a name, and has to come out the other
//! side as a card that points at an asset. Four things happen in between, and
//! each is here because doing it at the call site would mean doing it twice —
//! once for the drop and once for the paste — and the two would diverge.
//!
//! 1. **Classify.** What kind of card is this? The name is a hint and the bytes
//!    are the truth, so the bytes are asked first: a `.jpg` that is really a
//!    PNG is common enough, and a file with no extension at all is normal on a
//!    clipboard. Only when the bytes say nothing does the extension get a say.
//! 2. **Hash.** SHA-256 over the contents, which is the format's own identity
//!    for an asset. Two cards holding the same photograph name the same hash
//!    and the archive carries it once — so dropping the same folder twice costs
//!    two cards and no extra bytes.
//! 3. **Measure.** A picture is decoded far enough to learn its shape, so the
//!    card arrives the shape of what is on it rather than as a default
//!    rectangle somebody has to fix by hand.
//! 4. **Report.** A file too large to be reasonable is *reported*, never
//!    silently refused and never silently accepted. This module does not know
//!    whether somebody meant to put a two-gigabyte video on a moodboard; the
//!    layer that can say so is the one holding the pointer.
//!
//! The two halves of "what is this", and they are deliberately different
//! sizes. **What kind of card it becomes** is the short hand-written table in
//! [`kind_of`] below — short because every entry is a promise that this build
//! can draw the thing, and a card type given out generously is a frame on the
//! board that stays empty forever. **What it is called** is
//! [`mbrd_core::formats`], thirteen hundred extensions generated from the
//! original's own catalogue, because a name costs nothing to be generous with:
//! a `.sldprt` reading "SolidWorks part" is strictly better than "file" even
//! though neither will ever open.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use mbrd_core::mbrd::Asset;
use mbrd_core::model::{Item, ItemAsset, ItemType, NOTE_MAX};

/// Past this, one file is worth asking about. Not a limit — see the module note.
///
/// Chosen against what a `.mbrd` is *for*: it is a document somebody sends to
/// somebody else, and a single asset larger than this makes it something that
/// has to be shared another way. A video usually is that, which is why the
/// format carries posters.
pub const WORTH_ASKING: usize = 128 * 1024 * 1024;

/// The biggest a new card is on the board, in world units, along its long side.
pub const ARRIVAL_SIZE: f32 = 420.0;

/// A file, understood well enough to become a card.
pub struct Ready {
    pub kind: ItemType,
    /// What the type is called, for a status line: `PNG image`, `MPEG audio`.
    pub described: &'static str,
    pub hash: String,
    pub asset: Asset,
    /// The picture's shape, where it is a picture and could be read.
    pub natural: Option<(u32, u32)>,
    pub name: String,
    /// A note's words, where the file is text rather than bytes.
    pub text: Option<String>,
    /// Whether the file carries a sound track, where that could be read.
    ///
    /// `None` means nobody managed to look, which the card reads as "assume
    /// there is sound" — see [`mbrd_core::sound`].
    pub sound: Option<bool>,
    /// A PDF's page count, where the file could be parsed.
    pub pages: Option<u64>,
    /// A font's own family name, read out of its `name` table.
    pub family: Option<String>,
    /// A binary STL's triangle count, read off its header alone.
    pub triangles: Option<u32>,
}

impl Ready {
    /// Whether this one is large enough that somebody should be told.
    pub fn is_heavy(&self) -> bool {
        self.asset.bytes.len() > WORTH_ASKING
    }

    /// How big, in megabytes, for saying so.
    pub fn megabytes(&self) -> usize {
        self.asset.bytes.len() / (1024 * 1024)
    }
}

/// Work out what a file is and get it ready to be a card.
///
/// Never fails: a file this build cannot place becomes a generic card with its
/// bytes attached, which is the same promise the format makes about a type it
/// has never heard of. Refusing here would mean a folder drop silently losing
/// the files nobody wrote a rule for.
pub fn ready(name: &str, bytes: Vec<u8>) -> Ready {
    let (kind, described, ext) = classify(name, &bytes);
    let hash = format!("{:x}", Sha256::digest(&bytes));

    let natural = matches!(kind, ItemType::Image).then(|| measure(&bytes)).flatten();

    // A note carries its words in the board rather than in an asset, because
    // that is what makes a note searchable, editable and diffable. The bytes
    // are still kept — see `Ready::asset` — so nothing is lost if this build's
    // idea of "text" turns out to be wrong about a particular file.
    let text = matches!(kind, ItemType::Note).then(|| words(&bytes)).flatten();

    // Asked here rather than at the first play, because it is the difference
    // between a video card wearing a mute button and not, and a card should not
    // grow a control the first time it is pressed. Audio answers for itself and
    // a still picture is silent by construction, so only video needs looking
    // into — and that is a walk of the track list, not a decode.
    let sound = matches!(kind, ItemType::Video).then(|| mbrd_core::sound::sniff(&bytes)).flatten();

    // The page count, not the pages: a page count is a fact the information
    // rail can say without opening the file at every frame it is drawn on
    // (see `mbrd_core::facts`), while the text on the page is decoded from
    // scratch each time the card is opened, in `opened.rs`, the same split
    // `naturalWidth` keeps with the pixels of a picture.
    let pages = (ext == "pdf").then(|| page_count(&bytes)).flatten();

    // Only the raw SFNT shapes here — `font_family` cannot open a WOFF any
    // more than `Preview::Font` claims to, see `preview::is_font`.
    let family =
        matches!(ext.as_str(), "ttf" | "otf" | "ttc").then(|| font_family(&bytes)).flatten();

    // A binary STL's count is four bytes read, not a mesh parsed — the same
    // "measured, not decoded" split `pages` and `family` keep above. An OBJ or
    // a GLB have no such shortcut, so their count costs the same parse
    // `images::decode` will do again at the first frame that wants to draw
    // one; both still happen once here rather than on every frame the rail is
    // redrawn. A `.stl` that turned out to be the ASCII variant, or an `.obj`
    // or `.glb` that will not parse, is still a `Model` card, just one with no
    // count and no rasterised still yet. See `mbrd_core::mesh`.
    let triangles = match ext.as_str() {
        "stl" if mbrd_core::mesh::is_stl(&bytes) => mbrd_core::mesh::triangle_count(&bytes),
        "obj" if mbrd_core::mesh::is_obj(&bytes) => {
            mbrd_core::mesh::obj(&bytes).map(|m| m.triangles.len() as u32)
        }
        "glb" if mbrd_core::mesh::is_glb(&bytes) => {
            mbrd_core::mesh::glb(&bytes).map(|m| m.triangles.len() as u32)
        }
        _ => None,
    };

    Ready {
        kind,
        described,
        hash,
        asset: Asset { bytes, ext: ext.to_string(), label: stem(name) },
        natural,
        name: name.to_string(),
        text,
        sound,
        pages,
        family,
        triangles,
    }
}

/// How many pages a PDF has, without extracting anything out of them.
fn page_count(bytes: &[u8]) -> Option<u64> {
    Some(lopdf::Document::load_mem(bytes).ok()?.get_pages().len() as u64)
}

/// A font's own name for itself, preferring the typographic family — the one
/// meant for display — over the compatibility name some fonts still carry
/// from the sixteen-style-per-family era.
fn font_family(bytes: &[u8]) -> Option<String> {
    let face = ttf_parser::Face::parse(bytes, 0).ok()?;
    let mut family = None;
    let mut typographic = None;
    for name in face.names() {
        if name.name_id == ttf_parser::name_id::FAMILY && family.is_none() {
            family = name.to_string();
        }
        if name.name_id == ttf_parser::name_id::TYPOGRAPHIC_FAMILY && typographic.is_none() {
            typographic = name.to_string();
        }
    }
    typographic.or(family)
}

/// Everything a drop points at, as a list of files to read.
///
/// A folder brings what is *directly* in it and nothing deeper. Walking a tree
/// is not something a drop should start: somebody who drops their home
/// directory by accident should get a handful of cards and a shrug, not a board
/// with a hundred thousand items on it.
///
/// Sorted, so that the block a folder arrives as is in the order the folder is
/// in rather than in whatever order the filesystem happened to answer. That
/// also makes the layout below reproducible, which is what lets the same drop
/// twice look the same twice.
///
/// Off the drawing thread, please — see [`crate::board_view::BoardView::take_files`].
/// A directory listing is a syscall per entry and the directory may be on a
/// network mount.
pub fn walk(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = Vec::new();
    for path in paths {
        if path.is_dir() {
            files.extend(
                std::fs::read_dir(path)
                    .into_iter()
                    .flatten()
                    .flatten()
                    .map(|e| e.path())
                    .filter(|p| p.is_file()),
            );
        } else {
            files.push(path.clone());
        }
    }
    files.sort();
    files
}

/// How many cards wide a block of `count` of them is laid out to be.
///
/// Square-ish, so a folder of twenty photographs arrives as a block you can see
/// rather than a stack you have to unpick.
pub fn across(count: usize) -> f32 {
    (count as f32).sqrt().ceil().max(1.0)
}

/// Where the `nth` card of a block goes, given the block's centre and width.
///
/// Split out from the placing so that a drop still arriving can work out where
/// each card goes as it turns up, without knowing what the ones behind it are.
/// That is the whole of what makes a folder land card by card rather than all
/// at once — see [`crate::board_view::BoardView::take_files`].
pub fn spot(at: mbrd_core::geometry::Point, across: f32, nth: usize) -> mbrd_core::geometry::Point {
    let column = nth as f32 % across;
    let row = (nth as f32 / across).floor();
    let spread = ARRIVAL_SIZE * 1.1;
    mbrd_core::geometry::point(
        at.x + (column - (across - 1.0) / 2.0) * spread,
        at.y - (row - (across - 1.0) / 2.0) * spread,
    )
}

/// Build the card, centred where it was dropped.
pub fn card(ready: &Ready, id: String, at: mbrd_core::geometry::Point, z: f32) -> Item {
    let mut item = Item::new(id, ready.kind.clone());
    item.name = ready.name.clone();
    item.x = at.x;
    item.y = at.y;
    item.z = z;

    let (w, h) = shape_for(ready);
    item.w = w;
    item.h = h;

    // A note's words live in the board. The asset is still attached, so the
    // original file survives a round trip even though nothing reads it.
    if let Some(text) = &ready.text {
        item.meta.insert("text".into(), serde_json::Value::String(text.clone()));
    }
    // A measurement, not a decision: it says what the file is, so it is written
    // only where it was actually read and re-derivable if it ever is not.
    if let Some(sound) = ready.sound {
        mbrd_core::media::set_has_sound(&mut item, sound);
    }
    // The picture's own size, under the names the web platform uses for it —
    // this format came from a browser and a reader that has seen a DOM already
    // knows what `naturalWidth` means. Written because it is otherwise thrown
    // away the moment the card has been shaped by it, and because the size a
    // card was *dropped* at says nothing about the size of the file: an
    // information rail that could not say how big a photograph is would be a
    // rail missing the first thing anybody asks. See `mbrd_core::facts`.
    if let Some((w, h)) = ready.natural {
        item.meta.insert("naturalWidth".into(), serde_json::Value::from(w));
        item.meta.insert("naturalHeight".into(), serde_json::Value::from(h));
    }
    if let Some(pages) = ready.pages {
        item.meta.insert("pages".into(), serde_json::Value::from(pages));
    }
    if let Some(family) = &ready.family {
        item.meta.insert("family".into(), serde_json::Value::from(family.clone()));
    }
    if let Some(triangles) = ready.triangles {
        item.meta.insert("triangles".into(), serde_json::Value::from(triangles));
    }

    item.asset = Some(ItemAsset::Embedded { hash: ready.hash.clone(), family: None });
    item
}

/// A card the size of what is on it, held to something a board can hold.
fn shape_for(ready: &Ready) -> (f32, f32) {
    let Some((w, h)) = ready.natural else {
        return match ready.kind {
            ItemType::Note => (260.0, 200.0),
            ItemType::Audio => (320.0, 96.0),
            ItemType::Video => (400.0, 225.0),
            _ => (300.0, 220.0),
        };
    };
    let (w, h) = (w.max(1) as f32, h.max(1) as f32);
    let scale = ARRIVAL_SIZE / w.max(h);
    (mbrd_core::geometry::clamp_size(w * scale), mbrd_core::geometry::clamp_size(h * scale))
}

/// A picture's shape, without keeping the pixels.
///
/// The header alone, which for every format here is a few dozen bytes off the
/// front — a full decode of a folder of photographs would be a folder of
/// photographs decoded twice, once for this and once for the cache.
fn measure(bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// A text file's words, held to the head the board carries in `meta.text`.
///
/// Not a limit on the note: the file's whole text rides along as the card's
/// asset, and everything that shows or edits the words prefers it — see
/// `opened::words_of`. This is only the derived, searchable first paragraph.
fn words(bytes: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut out = text.trim().to_string();
    if out.chars().count() > NOTE_MAX {
        let cut = out.char_indices().nth(NOTE_MAX).map(|(i, _)| i)?;
        out.truncate(cut);
    }
    Some(out)
}

/// The file name without its extension, for an archive entry to be named after.
fn stem(name: &str) -> String {
    Path::new(name)
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| name.to_string())
}

/// The extension, lowercased, or `bin` for a file that has none.
fn extension(name: &str) -> String {
    fn valid(s: &str) -> bool {
        !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric()) && s.len() <= 12
    }
    let path = Path::new(name);
    if let Some(e) = path.extension() {
        let e = e.to_string_lossy().to_lowercase();
        if valid(&e) {
            return e;
        }
    }
    // A dotless name can still be a recognised convention — `Dockerfile`,
    // `Makefile` — rather than nothing, and the same filter that keeps a
    // stray path out of an extension keeps a stray sentence out of this
    // fallback: `no-extension-at-all` has a hyphen and stays `bin`.
    if let Some(base) = path.file_name() {
        let base = base.to_string_lossy().to_lowercase();
        if valid(&base) {
            return base;
        }
    }
    "bin".into()
}

/// What kind of thing this is: the card type, a name for it, and the extension
/// the archive should keep it under.
///
/// The bytes get the first word. An extension is a claim somebody's file
/// manager made and is wrong often enough to matter — a `.jpeg` that is a PNG,
/// a screenshot pasted with no name at all — while the first few bytes of every
/// format here are the format saying what it is.
pub fn classify(name: &str, bytes: &[u8]) -> (ItemType, &'static str, String) {
    if let Some((kind, described, ext)) = sniff(bytes) {
        return (kind, described, ext.to_string());
    }
    let ext = extension(name);
    let (kind, described) = by_extension(&ext);
    (kind, described, ext)
}

/// What the bytes themselves say, where they say anything.
fn sniff(bytes: &[u8]) -> Option<(ItemType, &'static str, &'static str)> {
    let starts = |magic: &[u8]| bytes.len() >= magic.len() && &bytes[..magic.len()] == magic;
    // The ISO base media brand, four bytes in after the box length. It covers
    // MP4, MOV, HEIC and AVIF, which are the same container with different
    // claims about what is inside.
    let brand = (bytes.len() >= 12 && &bytes[4..8] == b"ftyp").then(|| &bytes[8..12]);
    // RIFF and EBML both put the real answer past a header.
    let riff = (bytes.len() >= 12 && starts(b"RIFF")).then(|| &bytes[8..12]);

    Some(match () {
        _ if starts(b"\x89PNG\r\n\x1a\n") => (ItemType::Image, "PNG image", "png"),
        _ if starts(b"\xff\xd8\xff") => (ItemType::Image, "JPEG image", "jpg"),
        _ if starts(b"GIF87a") || starts(b"GIF89a") => (ItemType::Image, "GIF image", "gif"),
        _ if riff == Some(b"WEBP") => (ItemType::Image, "WebP image", "webp"),
        _ if starts(b"BM") => (ItemType::Image, "bitmap image", "bmp"),
        _ if starts(b"II*\0") || starts(b"MM\0*") => (ItemType::Image, "TIFF image", "tiff"),
        _ if starts(b"qoif") => (ItemType::Image, "QOI image", "qoi"),
        // Named, but not `Image` — see `by_extension` for why these four are the
        // one family this build classifies away from what they truthfully are.
        _ if brand == Some(b"avif") => (ItemType::Generic, "AVIF image", "avif"),
        _ if matches!(brand, Some(b"heic") | Some(b"heix") | Some(b"mif1")) => {
            (ItemType::Generic, "HEIF image", "heic")
        }

        _ if matches!(brand, Some(b"qt  ")) => (ItemType::Video, "QuickTime video", "mov"),
        _ if matches!(brand, Some(b"M4A ") | Some(b"M4B ")) => {
            (ItemType::Audio, "AAC audio", "m4a")
        }
        // A Canon RAW is an ISO container too, and without this arm it would
        // fall through to the catch-all below and land as a *video* — the one
        // way the bytes-first rule can be worse than the name, since the name
        // says `.cr3` plainly. Named rather than drawn, for the reason
        // `kind_of` gives: nothing here decodes it.
        _ if brand == Some(b"crx ") => (ItemType::Generic, "Canon RAW (CR3)", "cr3"),
        // Every other ISO brand is some flavour of MP4. Checked last of the
        // `ftyp` arms for exactly that reason.
        _ if brand.is_some() => (ItemType::Video, "MPEG-4 video", "mp4"),
        _ if starts(b"\x1a\x45\xdf\xa3") => (ItemType::Video, "Matroska video", "webm"),
        _ if riff == Some(b"AVI ") => (ItemType::Video, "AVI video", "avi"),

        _ if riff == Some(b"WAVE") => (ItemType::Audio, "WAV audio", "wav"),
        _ if starts(b"fLaC") => (ItemType::Audio, "FLAC audio", "flac"),
        _ if starts(b"OggS") => (ItemType::Audio, "Ogg audio", "ogg"),
        _ if starts(b"ID3") => (ItemType::Audio, "MPEG audio", "mp3"),
        // A bare MPEG frame, for a file with no tag on the front.
        _ if bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0 => {
            (ItemType::Audio, "MPEG audio", "mp3")
        }

        _ if mbrd_core::mesh::is_glb(bytes) => (ItemType::Model, "glTF model", "glb"),
        _ if mbrd_core::mesh::is_stl(bytes) => (ItemType::Model, "STL mesh", "stl"),
        _ if starts(b"%PDF-") => (ItemType::Generic, "PDF document", "pdf"),

        _ if starts(b"OTTO") => (ItemType::Generic, "OpenType font", "otf"),
        _ if starts(&[0x00, 0x01, 0x00, 0x00]) || starts(b"true") => {
            (ItemType::Generic, "TrueType font", "ttf")
        }
        _ if starts(b"ttcf") => (ItemType::Generic, "TrueType collection", "ttc"),
        _ if starts(b"wOFF") => (ItemType::Generic, "WOFF font", "woff"),
        _ if starts(b"wOF2") => (ItemType::Generic, "WOFF2 font", "woff2"),
        _ => return None,
    })
}

/// What the name claims, for bytes that gave nothing away.
///
/// Two questions with two different answers behind them — see the module note.
/// The kind comes off the hand-written table; the words come off the generated
/// catalogue, and fall back to the family it is filed under and then to the
/// one word [`kind_of`] can always give. So an extension nobody has ever heard
/// of still lands as a named card, which is what the last arm of the table is
/// for.
fn by_extension(ext: &str) -> (ItemType, &'static str) {
    let kind = kind_of(ext);
    let described = mbrd_core::formats::name(ext)
        .or_else(|| mbrd_core::formats::family(ext).map(|f| f.label))
        .unwrap_or_else(|| kind_of_word(&kind));
    (kind, described)
}

/// The one word that is true of every card of a kind, for a file the catalogue
/// has never heard of.
fn kind_of_word(kind: &ItemType) -> &'static str {
    match kind {
        ItemType::Image => "image",
        ItemType::Video => "video",
        ItemType::Audio => "audio",
        ItemType::Model => "3D / CAD",
        ItemType::Note => "text",
        // Not a failure. An unknown file arrives as a named card holding its
        // own bytes, which is what lets a board be a place you put things
        // before you know what to do with them.
        _ => "file",
    }
}

/// What kind of card this extension makes.
///
/// **Kept short on purpose**, and every arm is a claim that something in this
/// tree can draw it. See the module note for why the catalogue is not allowed
/// to answer this: it knows a `.cr3` is a photograph, and nothing here decodes
/// Canon RAW.
fn kind_of(ext: &str) -> ItemType {
    match ext {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "qoi" | "ico"
        // Vector, so it never has magic bytes worth trusting — an SVG is XML
        // and may start with a comment, a declaration or the tag itself.
        | "tga" | "exr" | "hdr" | "svg" => ItemType::Image,

        "mp4" | "m4v" | "mov" | "webm" | "mkv" | "avi" | "wmv" | "flv" | "mpg" | "mpeg" | "ogv"
        | "3gp" | "mts" => ItemType::Video,

        "mp3" | "wav" | "flac" | "ogg" | "oga" | "opus" | "m4a" | "aac" | "aiff" | "aif"
        | "wma" | "alac" | "ape" => ItemType::Audio,

        "glb" | "gltf" | "obj" | "stl" | "fbx" | "dae" | "3mf" | "ply" | "usdz" | "blend"
        | "step" | "stp" | "iges" | "igs" | "sldprt" | "sldasm" => ItemType::Model,

        "md" | "markdown" | "txt" | "text" | "rst" | "org" => ItemType::Note,

        // Everything else is a named card holding its own bytes, which is what
        // lets a board be a place you put things before you know what to do
        // with them. Fonts, PDFs and archives all land here and all have a
        // page of their own to open on — see `opened.rs`.
        //
        // So do the real pictures nothing in this tree can decode: AVIF wants
        // `dav1d`, and HEIC and JPEG XL the `image` crate does not do at all.
        // Calling them images would put a card on the board that can never
        // draw — a permanently empty frame, which is worse than the named file
        // card every other unopenable format already gets. Reclassified rather
        // than dropped: the bytes are kept, so a build that grows a decoder can
        // call them images again. See `preview::UNREADABLE`.
        _ => ItemType::Generic,
    }
}

/// Whether some text is a web address, for deciding what a paste becomes.
///
/// Deliberately narrow. Anything vaguer turns a pasted sentence containing the
/// word `http` into a link card, and the wrong card type is more annoying than
/// the missing one — a note can be retyped, a link nobody meant cannot be
/// followed.
pub fn as_url(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    let looks_like = (trimmed.starts_with("https://") || trimmed.starts_with("http://"))
        && !trimmed.contains(char::is_whitespace)
        && trimmed.len() > 10;
    looks_like.then_some(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbaImage::new(w, h);
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img).write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn the_bytes_are_believed_over_the_name() {
        // A PNG somebody's camera app called `.jpg`. This happens constantly.
        let (kind, described, ext) = classify("holiday.jpg", &png(4, 4));
        assert_eq!(kind, ItemType::Image);
        assert_eq!(described, "PNG image");
        assert_eq!(ext, "png", "the archive should keep it under what it is");
    }

    #[test]
    fn a_file_with_no_name_at_all_is_still_placed() {
        // A screenshot off the clipboard arrives exactly like this.
        let (kind, _, ext) = classify("", &png(4, 4));
        assert_eq!(kind, ItemType::Image);
        assert_eq!(ext, "png");
    }

    #[test]
    fn the_name_is_believed_when_the_bytes_say_nothing() {
        let (kind, described, ext) = classify("model.sldprt", b"\0\0\0\0nothing in particular");
        assert_eq!(kind, ItemType::Model);
        // Off the catalogue, which is the whole reason it is here: the hand
        // table only knows this is a mesh-ish thing, and "3D / CAD" was all
        // a card could say about it before.
        assert_eq!(described, "SolidWorks part");
        assert_eq!(ext, "sldprt");
    }

    /// The catalogue names what the hand table has no opinion about, and the
    /// hand table still decides what kind of card it is.
    #[test]
    fn the_catalogue_names_what_the_hand_table_cannot_draw() {
        let (kind, described, _) = classify("layers.psd", b"8BPS\0\x01");
        assert_eq!(kind, ItemType::Generic, "nothing here decodes a PSD");
        assert_eq!(described, "Photoshop document");

        // A photograph by every measure except the one that matters: no
        // decoder, so no image card. See `kind_of`.
        let (kind, described, _) = classify("dsc_0001.cr3", b"\0\0\0\x18ftypcrx ");
        assert_eq!(kind, ItemType::Generic);
        assert_eq!(described, "Canon RAW (CR3)");
    }

    #[test]
    fn a_file_nobody_wrote_a_rule_for_still_arrives() {
        let (kind, described, ext) = classify("notes.xyzzy", b"whatever this is");
        assert_eq!(kind, ItemType::Generic);
        // Thirteen hundred extensions and this is not one of them, so the card
        // falls all the way back to the word every card can wear.
        assert_eq!(described, "file");
        assert_eq!(ext, "xyzzy");
    }

    /// A PDF with `n` empty pages and nothing in them — page count only reads
    /// the page tree, not a single content stream, so that is all this builds.
    fn built_pdf(n: usize) -> Vec<u8> {
        use lopdf::{dictionary, Document, Object};

        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let kids: Vec<Object> = (0..n)
            .map(|_| doc.add_object(dictionary! { "Type" => "Page", "Parent" => pages_id }).into())
            .collect();
        let tree = dictionary! { "Type" => "Pages", "Kids" => kids, "Count" => n as i64 };
        doc.objects.insert(pages_id, Object::Dictionary(tree));
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog_id);

        let mut bytes = Vec::new();
        doc.save_to(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn a_pdfs_page_count_is_read_at_import() {
        let file = ready("report.pdf", built_pdf(3));
        assert_eq!(file.pages, Some(3));
    }

    #[test]
    fn a_file_that_is_not_really_a_pdf_gets_no_page_count() {
        let file = ready("report.pdf", b"not actually a pdf".to_vec());
        assert_eq!(file.pages, None);
    }

    #[test]
    fn a_true_type_or_open_type_font_says_its_own_family_name() {
        // Built by hand-writing a valid `name` table is its own small project,
        // so this reads one of whatever real fonts the machine running the
        // test already has, and checks `font_family` against `fontdb`'s own
        // reading of the same file — two independent parsers of the same
        // bytes, agreeing.
        let mut db = resvg::usvg::fontdb::Database::new();
        db.load_system_fonts();
        let agree = db.faces().find_map(|face| {
            let resvg::usvg::fontdb::Source::File(path) = &face.source else { return None };
            let bytes = std::fs::read(path).ok()?;
            let ours = font_family(&bytes)?;
            Some((ours, face.families.first()?.0.clone()))
        });
        let (ours, fontdbs) = agree.expect("a dev machine has at least one file-backed TTF or OTF");
        assert_eq!(ours, fontdbs);
    }

    #[test]
    fn a_font_that_will_not_parse_has_no_family_name() {
        assert_eq!(font_family(b"not a font at all"), None);
    }

    fn binary_stl(triangles: u32) -> Vec<u8> {
        let mut out = vec![0_u8; 84];
        out[80..84].copy_from_slice(&triangles.to_le_bytes());
        out.resize(84 + triangles as usize * 50, 0);
        out
    }

    #[test]
    fn a_binary_stls_triangle_count_is_read_at_import() {
        let file = ready("part.stl", binary_stl(6));
        assert_eq!(file.triangles, Some(6));
        assert_eq!(file.kind, ItemType::Model);
    }

    #[test]
    fn an_ascii_stl_gets_no_triangle_count_yet() {
        let file = ready("part.stl", b"solid part\nendsolid part\n".to_vec());
        assert_eq!(file.triangles, None);
    }

    #[test]
    fn an_objs_triangle_count_is_read_at_import() {
        let obj = b"v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nf 1 2 3 4\n".to_vec();
        let file = ready("part.obj", obj);
        assert_eq!(file.triangles, Some(2), "a quad fans into two triangles");
        assert_eq!(file.kind, ItemType::Model);
    }

    #[test]
    fn a_file_named_obj_that_is_not_one_gets_no_triangle_count() {
        let file = ready("part.obj", b"this is not a wavefront file at all\n".to_vec());
        assert_eq!(file.triangles, None);
    }

    #[test]
    fn a_glbs_triangle_count_is_read_at_import() {
        let json = br#"{"bufferViews":[],"accessors":[],"meshes":[]}"#;
        let mut glb = Vec::new();
        glb.extend_from_slice(b"glTF");
        glb.extend_from_slice(&2_u32.to_le_bytes());
        glb.extend_from_slice(&(12 + 8 + json.len() as u32).to_le_bytes());
        glb.extend_from_slice(&(json.len() as u32).to_le_bytes());
        glb.extend_from_slice(b"JSON");
        glb.extend_from_slice(json);
        // No meshes at all, so no triangles — the count is still `None`
        // rather than the import failing, the same as the ASCII STL above.
        let file = ready("model.glb", glb);
        assert_eq!(file.triangles, None);
        assert_eq!(file.kind, ItemType::Model, "the magic alone is enough to classify it");
    }

    #[test]
    fn a_dotless_convention_name_is_its_own_extension() {
        // `LANGUAGES` in `preview.rs` carries `dockerfile` and `makefile` rows
        // precisely so these are not stranded as `bin`.
        let (_, _, ext) = classify("Dockerfile", b"FROM rust:latest\n");
        assert_eq!(ext, "dockerfile");
        let (_, _, ext) = classify("Makefile", b"all:\n\techo hi\n");
        assert_eq!(ext, "makefile");
    }

    #[test]
    fn an_extension_that_is_not_one_does_not_become_an_archive_entry() {
        // `ext` ends up in a path inside the ZIP, so a name like
        // `photo.../../etc` must not become the extension.
        let (_, _, ext) = classify("photo.tar.gz.a-very-long-extension", b"?");
        assert_eq!(ext, "bin");
        let (_, _, ext) = classify("no-extension-at-all", b"?");
        assert_eq!(ext, "bin");
    }

    #[test]
    fn the_containers_that_hold_more_than_one_thing_are_told_apart() {
        let mut mp4 = vec![0, 0, 0, 0x18];
        mp4.extend_from_slice(b"ftypisom");
        mp4.extend_from_slice(b"\0\0\x02\0");
        assert_eq!(classify("clip", &mp4).0, ItemType::Video);

        let mut m4a = vec![0, 0, 0, 0x18];
        m4a.extend_from_slice(b"ftypM4A ");
        m4a.extend_from_slice(b"\0\0\0\0");
        assert_eq!(classify("track", &m4a).0, ItemType::Audio);

        // Told apart from an MP4, and still not classified as an image: nothing
        // in this tree can decode AVIF, and a card that can never draw is worse
        // than a named file card. The description is what keeps the knowledge.
        let mut avif = vec![0, 0, 0, 0x18];
        avif.extend_from_slice(b"ftypavif");
        avif.extend_from_slice(b"\0\0\0\0");
        let (kind, described, ext) = classify("photo", &avif);
        assert_eq!(kind, ItemType::Generic);
        assert_eq!(described, "AVIF image");
        assert_eq!(ext, "avif");

        let mut wav = b"RIFF".to_vec();
        wav.extend_from_slice(&[0, 0, 0, 0]);
        wav.extend_from_slice(b"WAVE");
        assert_eq!(classify("sound", &wav).0, ItemType::Audio);

        let mut webp = b"RIFF".to_vec();
        webp.extend_from_slice(&[0, 0, 0, 0]);
        webp.extend_from_slice(b"WEBP");
        assert_eq!(classify("picture", &webp).0, ItemType::Image);
    }

    #[test]
    fn the_same_file_twice_is_the_same_asset_twice() {
        let a = ready("one.png", png(8, 8));
        let b = ready("another-name.png", png(8, 8));
        assert_eq!(a.hash, b.hash, "identical bytes should be one asset");

        let c = ready("different.png", png(9, 8));
        assert_ne!(a.hash, c.hash);
    }

    #[test]
    fn a_card_arrives_the_shape_of_the_picture_on_it() {
        let ready = ready("wide.png", png(1600, 400));
        let item = card(&ready, "x".into(), mbrd_core::geometry::point(0.0, 0.0), 1.0);
        assert!((item.w / item.h - 4.0).abs() < 0.01, "{}x{}", item.w, item.h);
        assert!((item.w - ARRIVAL_SIZE).abs() < 0.01, "{}", item.w);
        assert_eq!(item.kind, ItemType::Image);
        assert_eq!(item.asset.as_ref().and_then(ItemAsset::hash), Some(ready.hash.as_str()));
    }

    #[test]
    fn a_picture_of_absurd_proportions_still_makes_a_card_a_board_can_hold() {
        let ready = ready("sliver.png", png(4000, 2));
        let item = card(&ready, "x".into(), mbrd_core::geometry::point(0.0, 0.0), 1.0);
        assert!(item.h >= mbrd_core::model::MIN_SIZE, "height {}", item.h);
        assert!(item.w <= mbrd_core::model::MAX_SIZE);
    }

    #[test]
    fn a_text_file_becomes_a_note_with_the_words_in_it() {
        let ready = ready("thoughts.md", b"# a heading\n\nand a thought".to_vec());
        assert_eq!(ready.kind, ItemType::Note);
        let item = card(&ready, "x".into(), mbrd_core::geometry::point(0.0, 0.0), 1.0);
        assert_eq!(item.note_text(), Some("# a heading\n\nand a thought"));
    }

    #[test]
    fn a_text_file_longer_than_a_note_is_cut_rather_than_refused() {
        let long = "x".repeat(NOTE_MAX * 3);
        let ready = ready("long.txt", long.into_bytes());
        assert_eq!(ready.text.as_ref().map(|t| t.chars().count()), Some(NOTE_MAX));
    }

    #[test]
    fn a_text_file_that_is_not_text_does_not_become_a_note_full_of_rubbish() {
        let ready = ready("claims-to-be.txt", vec![0xff, 0xfe, 0x00, 0x01]);
        assert!(ready.text.is_none());
    }

    #[test]
    fn only_something_that_is_plainly_an_address_is_treated_as_one() {
        assert_eq!(as_url("  https://example.invalid/a  "), Some("https://example.invalid/a"));
        assert_eq!(as_url("http://example.invalid"), Some("http://example.invalid"));
        assert!(as_url("see https://example.invalid for more").is_none());
        assert!(as_url("https://").is_none());
        assert!(as_url("just a thought").is_none());
    }

    /// A directory this test owns, named after the test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mbrd-import-test-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temporary directory");
        dir
    }

    #[test]
    fn a_folder_brings_what_is_in_it_and_not_what_is_under_it() {
        let dir = scratch("one-level");
        std::fs::write(dir.join("b.png"), b"?").unwrap();
        std::fs::write(dir.join("a.png"), b"?").unwrap();
        std::fs::create_dir(dir.join("deeper")).unwrap();
        std::fs::write(dir.join("deeper/c.png"), b"?").unwrap();

        let found = walk(std::slice::from_ref(&dir));
        // Sorted, so the block a folder arrives as is in the folder's order
        // rather than in whatever order the filesystem answered in.
        assert_eq!(found, vec![dir.join("a.png"), dir.join("b.png")]);
    }

    #[test]
    fn a_file_dropped_by_itself_is_taken_as_it_is() {
        let dir = scratch("bare-file");
        let file = dir.join("only.png");
        std::fs::write(&file, b"?").unwrap();
        assert_eq!(walk(std::slice::from_ref(&file)), vec![file]);
        // A path that is neither is nobody's problem here: it becomes a file
        // that cannot be read, which is a line in the status bar, not a panic.
        assert_eq!(walk(&[dir.join("gone.png")]), vec![dir.join("gone.png")]);
    }

    #[test]
    fn one_card_lands_on_the_point_it_was_dropped_at() {
        let at = mbrd_core::geometry::point(120.0, -40.0);
        let spot = spot(at, across(1), 0);
        assert_eq!((spot.x, spot.y), (at.x, at.y));
    }

    #[test]
    fn a_block_fills_across_before_it_fills_down() {
        // Nine cards is three across, so the fourth starts the second row —
        // left of the third and below the first.
        let at = mbrd_core::geometry::point(0.0, 0.0);
        let across = across(9);
        assert_eq!(across, 3.0);
        let (first, third, fourth) =
            (spot(at, across, 0), spot(at, across, 2), spot(at, across, 3));
        assert!(third.x > first.x && (third.y - first.y).abs() < 0.01);
        // World y points up, so the row below is the lower number.
        assert!((fourth.x - first.x).abs() < 0.01 && fourth.y < first.y);
    }

    #[test]
    fn a_block_is_centred_on_where_it_was_dropped() {
        let at = mbrd_core::geometry::point(500.0, 250.0);
        let across = across(4);
        let spots: Vec<_> = (0..4).map(|n| spot(at, across, n)).collect();
        let mid_x = spots.iter().map(|s| s.x).sum::<f32>() / 4.0;
        let mid_y = spots.iter().map(|s| s.y).sum::<f32>() / 4.0;
        assert!((mid_x - at.x).abs() < 0.01, "{mid_x}");
        assert!((mid_y - at.y).abs() < 0.01, "{mid_y}");
    }

    #[test]
    fn a_large_file_is_reported_rather_than_judged() {
        let small = ready("small.png", png(4, 4));
        assert!(!small.is_heavy());
        let big = ready("big.bin", vec![0; WORTH_ASKING + 1]);
        assert!(big.is_heavy());
        assert_eq!(big.megabytes(), WORTH_ASKING / (1024 * 1024));
        // And it is still ready. Deciding is somebody else's job.
        assert_eq!(big.kind, ItemType::Generic);
    }
}
