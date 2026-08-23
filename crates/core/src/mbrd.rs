//! The `.mbrd` file: a ZIP with a different extension.
//!
//! ```text
//! myboard.mbrd
//! ├── mimetype                    the media type, first and uncompressed
//! ├── manifest.json               what this file is
//! ├── board.json                  the board itself
//! ├── assets/<slug>--<hash>.<ext> embedded bytes, deduped by content hash
//! ├── notes/<slug>--<id>.md       one sticky note, as Markdown
//! └── waveforms/<hash>.json       one audio file's measured readings
//! ```
//!
//! The format makes three promises, and everything awkward in this module is
//! one of them being kept:
//!
//! 1. **A board is one file.** Assets are embedded, never linked.
//! 2. **The archive is legible.** Notes are real Markdown, the JSON is
//!    indented, waveforms are readable numbers. Not "parseable given the
//!    source" — legible. If mbrd vanished, `unzip` would still get your work back.
//! 3. **It opens with no network and no account.**
//!
//! Anything that breaks one of those is a format change, not an optimisation.
//!
//! The full specification lives in the original repository at
//! `research/docs/mbrd-format.md`, and it is explicitly free to implement.

use std::collections::HashMap;
use std::io::{Cursor, Read, Seek, Write};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::model::{Board, ItemType, NOTE_MAX};
use crate::schema;
use crate::state::{self, BoardState};

/// The media type. A file is recognised by its extension, by this type, or by
/// the `mimetype` entry.
pub const MIME_TYPE: &str = "application/vnd.mbrd+zip";

/// The format version this build writes.
pub const FORMAT_VERSION: u64 = 1;

/// What this build calls itself in a manifest's `app` field.
pub const APP_ID: &str = concat!("mbrd-rs v", env!("CARGO_PKG_VERSION"));

/// A board past this is not a board. There is no ZIP64 in this format.
const MAX_ARCHIVE: u64 = 4 * 1024 * 1024 * 1024;
/// A single entry ceiling, so a zip bomb cannot be inflated into memory.
const MAX_ENTRY: u64 = 512 * 1024 * 1024;

/// One embedded file, keyed elsewhere by the SHA-256 of exactly these bytes.
#[derive(Debug, Clone)]
pub struct Asset {
    pub bytes: Vec<u8>,
    /// `[a-z0-9]{1,12}`, taken from the original filename and used only to
    /// rebuild a media type on the way back in — ZIP entries carry no content
    /// type of their own.
    pub ext: String,
    /// What the card called it, kept only to rebuild a readable entry name on
    /// the way out. Never an identity: the hash is the identity.
    pub label: String,
}

/// An audio file's measured readings, so a card can draw its bars without
/// decoding several megabytes again.
#[derive(Debug, Clone)]
pub struct Waveform {
    pub peaks: Vec<f32>,
}

/// What a `.mbrd` says about itself.
#[derive(Debug, Clone)]
pub struct Manifest {
    pub version: u64,
    pub app: String,
    pub created: String,
    pub modified: String,
    pub title: String,
    /// Features a reader **must** understand to open this file at all.
    pub requires: Vec<String>,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            version: FORMAT_VERSION,
            app: APP_ID.to_string(),
            created: String::new(),
            modified: String::new(),
            title: String::new(),
            requires: Vec::new(),
        }
    }
}

/// A board and everything it carries.
#[derive(Debug, Clone, Default)]
pub struct Document {
    pub manifest: Manifest,
    /// The board, its history, and the only way to change either.
    ///
    /// A `BoardState` reads exactly like the `Board` it wraps — `doc.board.items`
    /// still says what it always said — and cannot be written to except through
    /// [`BoardState::edit`]. That is the whole of what keeps undo from having to
    /// be retrofitted across every place that moves a card.
    pub board: BoardState,
    /// Keyed by content hash.
    pub assets: HashMap<String, Asset>,
    /// Keyed by the hash of the *audio*, not the id of the card — a waveform is
    /// a property of a recording, so the same clip twice is measured once.
    pub waveforms: HashMap<String, Waveform>,
}

// ---------------------------------------------------------------------------
// Reading
// ---------------------------------------------------------------------------

/// Open a `.mbrd`.
///
/// This is the one function in the crate that is allowed to refuse, and it does
/// so in exactly three cases: the bytes are not a ZIP, the manifest says this is
/// not an mbrd, or `requires` names a feature this build has never heard of.
/// **Everything else degrades** — that division is the format's, not a
/// convenience, and the reasoning is worth keeping in view when adding a check
/// here. A reader that refuses a board it could mostly have opened loses work
/// that a slightly lossy open would have kept.
pub fn read<R: Read + Seek>(reader: R) -> Result<Document> {
    let mut zip = ZipArchive::new(reader).context("not a readable ZIP archive")?;

    let mut board_json: Option<Value> = None;
    let mut manifest_json: Option<Value> = None;
    let mut assets: HashMap<String, Asset> = HashMap::new();
    let mut waveforms: HashMap<String, Waveform> = HashMap::new();
    let mut notes: HashMap<String, String> = HashMap::new();

    let mut total: u64 = 0;
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i)?;
        if entry.is_dir() {
            continue;
        }
        // Every entry name is validated before it is used. Nothing in here ever
        // becomes a path on disk, but a `..` or an absolute name is still a
        // signal the archive was built to be unpacked somewhere it should not
        // be, and refusing it costs nothing.
        let name = match safe_name(entry.name()) {
            Some(n) => n,
            None => continue,
        };
        if entry.size() > MAX_ENTRY {
            continue;
        }
        total = total.saturating_add(entry.size());
        if total > MAX_ARCHIVE {
            bail!("archive is larger than the format allows");
        }

        let mut bytes = Vec::with_capacity(entry.size().min(1 << 20) as usize);
        entry.read_to_end(&mut bytes)?;

        if name == "board.json" {
            board_json = serde_json::from_slice(&bytes).ok();
        } else if name == "manifest.json" {
            manifest_json = serde_json::from_slice(&bytes).ok();
        } else if name == "mimetype" {
            // Deliberately not checked. `manifest.json`'s `format` is the answer
            // to "what is this", it is stricter, and a second source for the
            // same fact would disagree the first time somebody unzipped a
            // board, edited a note, and rezipped it with the entries reordered.
        } else if let Some(rest) = name.strip_prefix("assets/") {
            if let Some((hash, ext)) = split_asset_name(rest) {
                if verify(&hash, &bytes) {
                    assets.insert(hash, Asset { bytes, ext, label: String::new() });
                }
            }
        } else if let Some(rest) = name.strip_prefix("notes/") {
            if let Some(id) = note_id(rest) {
                if let Ok(text) = String::from_utf8(bytes) {
                    notes.insert(id, text);
                }
            }
        } else if let Some(rest) = name.strip_prefix("waveforms/") {
            if let Some(hash) = rest.strip_suffix(".json").filter(|h| schema::is_hash(h)) {
                if let Some(w) = parse_waveform(&bytes) {
                    waveforms.insert(hash.to_string(), w);
                }
            }
        }
    }

    let manifest = read_manifest(manifest_json.as_ref())?;

    let board_json = board_json.ok_or_else(|| anyhow!("no readable board.json in the archive"))?;
    let mut board = schema::normalize(&board_json);

    // **The `.md` outranks `board.json`.** Edit one of those files by hand,
    // rezip, and the board opens with your edit. That is the entire point of
    // writing them, and it is the one place in this reader where a sidecar wins
    // over the record it was derived from.
    for item in &mut board.items {
        if item.kind != ItemType::Note {
            continue;
        }
        if let Some(text) = notes.get(&item.id) {
            let text: String = text.trim_end_matches('\n').chars().take(NOTE_MAX).collect();
            item.meta.insert("text".into(), Value::String(text));
            // `rich` flattens to `text` and is authoritative over it, so a hand
            // edit to the Markdown has to drop it or the edit would be shown
            // only until the next render.
            item.meta.remove("rich");
        }
    }

    // Where the title is missing from the board, the manifest supplies one.
    if board.title.is_empty() && !manifest.title.is_empty() {
        board.title = manifest.title.clone();
    }

    // The ledger is adopted against the document *as the file carried it*,
    // which is why the parsed JSON is handed over whole rather than just its
    // `timeline` key. See `schema::doc_fingerprint`.
    let board = BoardState::opened(board, &board_json);

    Ok(Document { manifest, board, assets, waveforms })
}

fn read_manifest(data: Option<&Value>) -> Result<Manifest> {
    let Some(src) = data else {
        // No manifest at all is not fatal. It is the one required entry this
        // reader can reconstruct entirely from defaults, and refusing here
        // would lose a board over a missing courtesy.
        return Ok(Manifest::default());
    };

    // A file whose manifest says it is something else is refused.
    match src.get("format").and_then(Value::as_str) {
        Some("mbrd") | None => {}
        Some(other) => bail!("this file says it is a {other}, not an mbrd"),
    }

    let requires: Vec<String> = src
        .get("requires")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
        .unwrap_or_default();

    // **The one rule in this format that asks a reader to fail rather than
    // degrade.** `version` cannot do this job: no reader anywhere refuses a
    // higher version, so a build meeting a field it has never heard of drops
    // it, writes it back out missing, and reports the save as a success.
    // `requires` is the only way to say "this is newer than you *and you will
    // lose something*", which is the only version of that sentence anybody can
    // act on. It costs nothing for years and then saves somebody's work once.
    if let Some(unknown) = requires.iter().find(|r| !understands(r)) {
        bail!("this board needs a feature this build does not have: {unknown}");
    }

    Ok(Manifest {
        version: src.get("version").and_then(Value::as_u64).unwrap_or(FORMAT_VERSION),
        app: src.get("app").and_then(Value::as_str).unwrap_or_default().to_string(),
        created: src.get("created").and_then(Value::as_str).unwrap_or_default().to_string(),
        modified: src.get("modified").and_then(Value::as_str).unwrap_or_default().to_string(),
        title: src.get("title").and_then(Value::as_str).unwrap_or_default().to_string(),
        requires,
    })
}

/// The features this build claims to understand.
///
/// Empty, and meant to stay empty for a long time: everything in the format
/// today degrades honestly. A `requires` that cried wolf would be the first
/// thing an implementer hard-coded past.
fn understands(_feature: &str) -> bool {
    false
}

// ---------------------------------------------------------------------------
// Writing
// ---------------------------------------------------------------------------

/// Pack a board into a `.mbrd`.
///
/// Fails rather than writing a hole: **a referenced hash with no bytes fails
/// the export**. It used to warn and carry on in the original, which produced a
/// file with a missing photograph while telling the user their work was safe.
pub fn write<W: Write + Seek>(writer: W, doc: &Document, now: &str) -> Result<()> {
    let mut zip = ZipWriter::new(writer);

    // First entry in the archive, **stored rather than deflated, and with no
    // extra field** — which puts the media type at a fixed offset where a tool
    // that has never heard of this format can find it. A local file header is
    // 30 bytes plus the name, so `application/vnd.mbrd+zip` begins at offset
    // 38: the number a file(1) magic rule wants. ODF and EPUB do the same.
    zip.start_file(
        "mimetype",
        SimpleFileOptions::default()
            .compression_method(CompressionMethod::Stored)
            .large_file(false),
    )?;
    zip.write_all(MIME_TYPE.as_bytes())?;

    let deflated = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);

    let created = if doc.manifest.created.is_empty() { now } else { &doc.manifest.created };
    let manifest = json!({
        "format": "mbrd",
        "version": FORMAT_VERSION,
        "app": APP_ID,
        // Preserved across re-saves, so a board keeps its birthday.
        "created": created,
        "modified": now,
        "title": doc.board.title,
        "requires": Vec::<String>::new(),
    });
    zip.start_file("manifest.json", deflated)?;
    // Indented, not minified. Promise 2.
    zip.write_all(serde_json::to_string_pretty(&manifest)?.as_bytes())?;

    let mut board: Board = (*doc.board).clone();

    // A waveform written as a sidecar is **removed** from `board.json`. Storing
    // it twice would be the same bytes for nothing and two places to disagree.
    for item in &mut board.items {
        let hash = item.asset.as_ref().and_then(crate::model::ItemAsset::hash);
        if hash.map(|h| doc.waveforms.contains_key(h)).unwrap_or(false) {
            item.meta.remove("peaks");
        }
    }

    // The two measurements the file records rather than derives. Both are
    // stamped here and nowhere else, because here is where the board stops
    // being something being edited and becomes something somebody else will
    // open — possibly straight into a layout where the measurement cannot be
    // taken. See `fence` and `stick` for why each is written down at all.
    crate::fence::stamp(&mut board.items);
    crate::stick::stamp(&mut board.items);

    zip.start_file("board.json", deflated)?;
    let filed = state::to_value(&board, doc.board.timeline());
    zip.write_all(serde_json::to_string_pretty(&filed)?.as_bytes())?;

    // Only hashes still referenced by a live item or a binned one are written,
    // so deleting things and saving actually shrinks the file.
    for hash in doc.board.required_hashes() {
        let asset = doc
            .assets
            .get(&hash)
            .ok_or_else(|| anyhow!("the board names an asset that is not here: {hash}"))?;
        zip.start_file(asset_name(asset, &hash), deflated)?;
        zip.write_all(&asset.bytes)?;
    }

    // And the fourth class of reference: bytes only a step of the ledger still
    // wants. Written where they are here, **walked past where they are not** —
    // that is the one difference from the three above, and it is deliberate. A
    // step can name bytes something else legitimately discarded, and refusing to
    // write somebody's board over an entry in its history would be the wrong way
    // round. What it costs is a scrub that comes back to a card with no picture,
    // which is a hole in the past rather than a hole in the board.
    for hash in doc.board.optional_hashes() {
        let Some(asset) = doc.assets.get(&hash) else { continue };
        zip.start_file(asset_name(asset, &hash), deflated)?;
        zip.write_all(&asset.bytes)?;
    }

    // The convenience copies. A note whose id is not filename-safe simply does
    // not get one: its text is still in `board.json`, so nothing is lost but
    // the convenience.
    for item in &doc.board.items {
        if item.kind != ItemType::Note {
            continue;
        }
        let Some(text) = item.note_text() else { continue };
        if !filename_safe(&item.id) {
            continue;
        }
        let slug = slugify(text.lines().next().unwrap_or_default());
        zip.start_file(format!("notes/{slug}--{}.md", item.id), deflated)?;
        zip.write_all(text.as_bytes())?;
        zip.write_all(b"\n")?;
    }

    for (hash, wave) in &doc.waveforms {
        if !schema::is_hash(hash) {
            continue;
        }
        zip.start_file(format!("waveforms/{hash}.json"), deflated)?;
        zip.write_all(format_waveform(wave).as_bytes())?;
    }

    zip.finish()?;
    Ok(())
}

/// Pack a board into memory. The common case, since a `.mbrd` is one file.
pub fn to_bytes(doc: &Document, now: &str) -> Result<Vec<u8>> {
    let mut buf = Cursor::new(Vec::new());
    write(&mut buf, doc, now)?;
    Ok(buf.into_inner())
}

/// The SHA-256 of some bytes, as 64 lowercase hex characters.
pub fn hash_bytes(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().iter().fold(String::with_capacity(64), |mut s, b| {
        use std::fmt::Write as _;
        let _ = write!(s, "{b:02x}");
        s
    })
}

fn verify(hash: &str, bytes: &[u8]) -> bool {
    hash_bytes(bytes) == hash
}

// ---------------------------------------------------------------------------
// Entry names
// ---------------------------------------------------------------------------

/// Reject anything that would escape the archive if it were ever unpacked.
fn safe_name(name: &str) -> Option<String> {
    if name.is_empty()
        || name.len() > 512
        || name.starts_with('/')
        || name.contains('\\')
        || name.contains('\0')
        || name.split('/').any(|part| part == ".." || part == ".")
    {
        return None;
    }
    Some(name.to_string())
}

/// `<slug>--<hash>.<ext>`, or the bare `<hash>.<ext>`.
///
/// **The slug is for you and the hash is for the reader.** A reader must accept
/// both forms and must not derive anything from the slug — not the type, not
/// the name, not the order. Requiring it would mean requiring every
/// implementation to reproduce one app's slug function byte for byte.
fn split_asset_name(rest: &str) -> Option<(String, String)> {
    let (stem, ext) = rest.rsplit_once('.')?;
    if ext.is_empty()
        || ext.len() > 12
        || !ext.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
    {
        return None;
    }
    // Split on the **last** `--`. The slug has its own runs of punctuation
    // collapsed to a single dash, so two in a row appear nowhere inside it.
    let hash = match stem.rsplit_once("--") {
        Some((_slug, hash)) => hash,
        None => stem,
    };
    if !schema::is_hash(hash) {
        return None;
    }
    Some((hash.to_string(), ext.to_string()))
}

fn asset_name(asset: &Asset, hash: &str) -> String {
    let slug = slugify(&asset.label);
    let ext = if asset.ext.is_empty() { "bin" } else { &asset.ext };
    if slug.is_empty() {
        format!("assets/{hash}.{ext}")
    } else {
        format!("assets/{slug}--{hash}.{ext}")
    }
}

/// `<slug>--<id>.md`, where `<id>` is what matches the file back to its note.
fn note_id(rest: &str) -> Option<String> {
    let stem = rest.strip_suffix(".md")?;
    let id = match stem.rsplit_once("--") {
        Some((_slug, id)) => id,
        None => stem,
    };
    if id.is_empty() || !filename_safe(id) {
        return None;
    }
    Some(id.to_string())
}

fn filename_safe(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// Lowercased, non-alphanumerics collapsed to a single dash, 48 characters.
fn slugify(text: &str) -> String {
    let mut out = String::with_capacity(48);
    let mut pending_dash = false;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.extend(c.to_lowercase());
            if out.chars().count() >= 48 {
                break;
            }
        } else {
            pending_dash = true;
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Waveforms
// ---------------------------------------------------------------------------

/// `res` must equal `peaks.length`. A file where they disagree has been
/// truncated and is **ignored, not fatal** — the card falls back to measuring
/// the audio again, which is slower and always right.
fn parse_waveform(bytes: &[u8]) -> Option<Waveform> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let res = value.get("res")?.as_u64()? as usize;
    let peaks: Vec<f32> = value
        .get("peaks")?
        .as_array()?
        .iter()
        .filter_map(Value::as_f64)
        .map(|f| f.clamp(0.0, 1.0) as f32)
        .collect();
    if peaks.len() != res {
        return None;
    }
    Some(Waveform { peaks })
}

/// Sixteen numbers to a line, so the file is something a person can read.
///
/// JSON rather than packed binary, and the size argument does not survive
/// contact with deflate: after compression the two are within a few hundred
/// bytes, which beside the megabytes of audio they were measured from is not
/// worth making the archive unreadable for.
fn format_waveform(wave: &Waveform) -> String {
    let mut out = format!("{{\n  \"res\": {},\n  \"peaks\": [\n", wave.peaks.len());
    for (i, chunk) in wave.peaks.chunks(16).enumerate() {
        if i > 0 {
            out.push_str(",\n");
        }
        out.push_str("    ");
        let row: Vec<String> = chunk
            .iter()
            .map(|p| format!("{:.3}", p).trim_end_matches('0').trim_end_matches('.').to_string())
            .map(|s| if s.is_empty() || s == "-0" { "0".into() } else { s })
            .collect();
        out.push_str(&row.join(", "));
    }
    out.push_str("\n  ]\n}\n");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Item, ItemAsset};

    fn doc_with_a_photo_and_a_note() -> Document {
        let bytes = b"not really a jpeg".to_vec();
        let hash = hash_bytes(&bytes);

        let mut board = Board { title: "Kitchen".into(), ..Board::default() };

        let mut photo = Item::new("k3f9a2", ItemType::Image);
        photo.name = "kitchen-window.jpg".into();
        photo.x = 120.0;
        photo.y = -40.0;
        photo.asset = Some(ItemAsset::Embedded { hash: hash.clone(), family: None });
        board.items.push(photo);

        let mut note = Item::new("p81m4x", ItemType::Note);
        note.name = "buy the smaller one".into();
        note.meta.insert(
            "text".into(),
            Value::String("# buy the smaller one\n\nthe big one does not fit".into()),
        );
        board.items.push(note);

        let mut assets = HashMap::new();
        assets.insert(hash, Asset { bytes, ext: "jpg".into(), label: "kitchen window".into() });

        Document {
            manifest: Manifest::default(),
            board: BoardState::new(board),
            assets,
            waveforms: HashMap::new(),
        }
    }

    #[test]
    fn a_board_survives_a_pack_and_an_unpack() {
        let doc = doc_with_a_photo_and_a_note();
        let bytes = to_bytes(&doc, "2026-07-25T10:04:11.882Z").unwrap();
        let back = read(Cursor::new(bytes)).unwrap();

        assert_eq!(back.board.title, "Kitchen");
        assert_eq!(back.board.items.len(), 2);
        assert_eq!(back.board.items[0].name, "kitchen-window.jpg");
        assert_eq!(back.board.items[0].x, 120.0);
        assert_eq!(back.assets.len(), 1);
        assert_eq!(back.assets.values().next().unwrap().bytes, b"not really a jpeg".to_vec());
    }

    #[test]
    fn the_mimetype_lands_at_offset_thirty_eight() {
        // The number a file(1) magic rule wants. A local file header is 30
        // bytes plus the 8-byte name, and the entry must carry no extra field.
        let bytes = to_bytes(&doc_with_a_photo_and_a_note(), "now").unwrap();
        assert_eq!(&bytes[30..38], b"mimetype");
        assert_eq!(&bytes[38..38 + MIME_TYPE.len()], MIME_TYPE.as_bytes());
    }

    #[test]
    fn a_hand_edited_note_outranks_board_json() {
        // The whole point of writing the sidecars: unzip, edit, rezip, and the
        // board opens with your words.
        let doc = doc_with_a_photo_and_a_note();
        let packed = to_bytes(&doc, "now").unwrap();

        let mut zip = ZipArchive::new(Cursor::new(packed)).unwrap();
        let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
        for i in 0..zip.len() {
            let mut e = zip.by_index(i).unwrap();
            let name = e.name().to_string();
            let mut b = Vec::new();
            e.read_to_end(&mut b).unwrap();
            entries.push((name, b));
        }

        let mut out = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, body) in entries {
            out.start_file(&name, SimpleFileOptions::default()).unwrap();
            if name.starts_with("notes/") {
                out.write_all(b"# get the big one after all\n").unwrap();
            } else {
                out.write_all(&body).unwrap();
            }
        }
        let rezipped = out.finish().unwrap().into_inner();

        let back = read(Cursor::new(rezipped)).unwrap();
        let note = back.board.item("p81m4x").unwrap();
        assert_eq!(note.note_text(), Some("# get the big one after all"));
    }

    #[test]
    fn an_asset_whose_bytes_do_not_match_its_hash_is_dropped() {
        let doc = doc_with_a_photo_and_a_note();
        let packed = to_bytes(&doc, "now").unwrap();

        let mut zip = ZipArchive::new(Cursor::new(packed)).unwrap();
        let mut out = ZipWriter::new(Cursor::new(Vec::new()));
        for i in 0..zip.len() {
            let mut e = zip.by_index(i).unwrap();
            let name = e.name().to_string();
            let mut b = Vec::new();
            e.read_to_end(&mut b).unwrap();
            out.start_file(&name, SimpleFileOptions::default()).unwrap();
            if name.starts_with("assets/") {
                out.write_all(b"tampered").unwrap();
            } else {
                out.write_all(&b).unwrap();
            }
        }
        let tampered = out.finish().unwrap().into_inner();

        let back = read(Cursor::new(tampered)).unwrap();
        // The board still opens. The card is there; only its bytes are not.
        assert_eq!(back.board.items.len(), 2);
        assert!(back.assets.is_empty());
    }

    #[test]
    fn packing_a_board_that_names_missing_bytes_fails_rather_than_writing_a_hole() {
        let mut doc = doc_with_a_photo_and_a_note();
        doc.assets.clear();
        assert!(to_bytes(&doc, "now").is_err());
    }

    #[test]
    fn a_manifest_requiring_an_unknown_feature_is_refused() {
        let mut doc = doc_with_a_photo_and_a_note();
        let packed = to_bytes(&doc, "now").unwrap();
        doc.assets.clear();

        let mut zip = ZipArchive::new(Cursor::new(packed)).unwrap();
        let mut out = ZipWriter::new(Cursor::new(Vec::new()));
        for i in 0..zip.len() {
            let mut e = zip.by_index(i).unwrap();
            let name = e.name().to_string();
            let mut b = Vec::new();
            e.read_to_end(&mut b).unwrap();
            out.start_file(&name, SimpleFileOptions::default()).unwrap();
            if name == "manifest.json" {
                out.write_all(br#"{"format":"mbrd","version":2,"requires":["holograms"]}"#)
                    .unwrap();
            } else {
                out.write_all(&b).unwrap();
            }
        }
        let packed = out.finish().unwrap().into_inner();
        let err = read(Cursor::new(packed)).unwrap_err().to_string();
        assert!(err.contains("holograms"), "got: {err}");
    }

    #[test]
    fn a_bare_hash_asset_name_is_as_legal_as_a_decorated_one() {
        let hash = "a".repeat(64);
        assert_eq!(split_asset_name(&format!("{hash}.mp4")), Some((hash.clone(), "mp4".into())));
        assert_eq!(
            split_asset_name(&format!("kitchen-window--{hash}.jpg")),
            Some((hash.clone(), "jpg".into()))
        );
        assert_eq!(split_asset_name("kitchen-window--notahash.jpg"), None);
    }

    #[test]
    fn entry_names_that_would_escape_the_archive_are_refused() {
        assert!(safe_name("assets/photo.jpg").is_some());
        assert!(safe_name("../../etc/passwd").is_none());
        assert!(safe_name("/etc/passwd").is_none());
        assert!(safe_name("assets/../../x").is_none());
        assert!(safe_name("assets\\x.jpg").is_none());
    }

    #[test]
    fn a_truncated_waveform_is_ignored_rather_than_fatal() {
        assert!(parse_waveform(br#"{"res":4,"peaks":[0.1,0.2]}"#).is_none());
        let ok = parse_waveform(br#"{"res":2,"peaks":[0.1,0.2]}"#).unwrap();
        assert_eq!(ok.peaks.len(), 2);
    }

    #[test]
    fn a_waveform_round_trips_and_reads_sixteen_to_a_line() {
        let wave = Waveform { peaks: (0..40).map(|i| i as f32 / 40.0).collect() };
        let text = format_waveform(&wave);
        assert_eq!(text.lines().filter(|l| l.starts_with("    ")).count(), 3);
        let back = parse_waveform(text.as_bytes()).unwrap();
        assert_eq!(back.peaks.len(), 40);
    }

    #[test]
    fn slugs_collapse_punctuation_so_two_dashes_never_appear_inside_one() {
        assert_eq!(slugify("Kitchen Window!! (final)"), "kitchen-window-final");
        assert_eq!(slugify("  --  "), "");
        assert!(!slugify("a -- b").contains("--"));
    }
}
