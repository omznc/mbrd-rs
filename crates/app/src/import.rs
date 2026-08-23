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
//! What is deliberately *not* here: the original's catalogue of some thirteen
//! hundred formats, which is generated rather than written. What is below is
//! the families that matter for the four card types this build can draw,
//! plus enough of a long tail that an unrecognised file still arrives as a
//! named card rather than as nothing. See the roadmap's Phase 3.

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

    Ready {
        kind,
        described,
        hash,
        asset: Asset { bytes, ext: ext.to_string(), label: stem(name) },
        natural,
        name: name.to_string(),
        text,
        sound,
    }
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

/// A text file's words, held to what a note may carry.
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
    Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .filter(|e| !e.is_empty() && e.chars().all(|c| c.is_ascii_alphanumeric()) && e.len() <= 12)
        .unwrap_or_else(|| "bin".into())
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
        _ if brand == Some(b"avif") => (ItemType::Image, "AVIF image", "avif"),
        _ if matches!(brand, Some(b"heic") | Some(b"heix") | Some(b"mif1")) => {
            (ItemType::Image, "HEIF image", "heic")
        }

        _ if matches!(brand, Some(b"qt  ")) => (ItemType::Video, "QuickTime video", "mov"),
        _ if matches!(brand, Some(b"M4A ") | Some(b"M4B ")) => {
            (ItemType::Audio, "AAC audio", "m4a")
        }
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

        _ if starts(b"glTF") => (ItemType::Model, "glTF model", "glb"),
        _ if starts(b"%PDF-") => (ItemType::Generic, "PDF document", "pdf"),
        _ => return None,
    })
}

/// What the name claims, for bytes that gave nothing away.
fn by_extension(ext: &str) -> (ItemType, &'static str) {
    match ext {
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "avif" | "heic"
        | "heif" | "qoi" | "ico" | "tga" | "exr" | "hdr" | "jxl" => (ItemType::Image, "image"),
        // Vector, so it never has magic bytes worth trusting — an SVG is XML
        // and may start with a comment, a declaration or the tag itself.
        "svg" => (ItemType::Image, "SVG image"),

        "mp4" | "m4v" | "mov" | "webm" | "mkv" | "avi" | "wmv" | "flv" | "mpg" | "mpeg" | "ogv"
        | "3gp" | "mts" => (ItemType::Video, "video"),

        "mp3" | "wav" | "flac" | "ogg" | "oga" | "opus" | "m4a" | "aac" | "aiff" | "aif"
        | "wma" | "alac" | "ape" => (ItemType::Audio, "audio"),

        "glb" | "gltf" | "obj" | "stl" | "fbx" | "dae" | "3mf" | "ply" | "usdz" | "blend"
        | "step" | "stp" | "iges" | "igs" | "sldprt" | "sldasm" => (ItemType::Model, "3D / CAD"),

        "md" | "markdown" | "txt" | "text" | "rst" | "org" => (ItemType::Note, "text"),

        "ttf" | "otf" | "woff" | "woff2" => (ItemType::Generic, "font"),
        "pdf" => (ItemType::Generic, "PDF document"),
        "zip" | "tar" | "gz" | "7z" | "rar" | "xz" | "zst" => (ItemType::Generic, "archive"),

        // Not a failure. An unknown file arrives as a named card holding its own
        // bytes, which is what lets a board be a place you put things before you
        // know what to do with them.
        _ => (ItemType::Generic, "file"),
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
        assert_eq!(described, "3D / CAD");
        assert_eq!(ext, "sldprt");
    }

    #[test]
    fn a_file_nobody_wrote_a_rule_for_still_arrives() {
        let (kind, described, ext) = classify("notes.xyzzy", b"whatever this is");
        assert_eq!(kind, ItemType::Generic);
        assert_eq!(described, "file");
        assert_eq!(ext, "xyzzy");
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

        let mut avif = vec![0, 0, 0, 0x18];
        avif.extend_from_slice(b"ftypavif");
        avif.extend_from_slice(b"\0\0\0\0");
        assert_eq!(classify("photo", &avif).0, ItemType::Image);

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
