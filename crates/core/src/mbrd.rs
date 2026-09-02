//! The `.mbrd` file: a ZIP with a different extension.
//!
//! ```text
//! myboard.mbrd
//! ├── mimetype                    the media type, first and uncompressed
//! ├── manifest.json               what this file is
//! ├── board.json                  the board itself
//! ├── assets/<slug>--<hash>.<ext> embedded bytes, deduped by content hash
//! ├── notes/<slug>--<id>.md       one note, as Markdown
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

use crate::model::{Board, ItemAsset, ItemType, NOTE_MAX};
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
    read_watched(reader, |_, _| {})
}

/// [`read`], saying how far it has got.
///
/// `watch` is handed the bytes unpacked so far and the bytes the archive says
/// it holds, once per entry. It exists because **opening is the one thing in
/// this app that can take seconds without anybody having asked for seconds**: a
/// board of photographs is most of a gigabyte to inflate and to hash, and a
/// window that says nothing for that long is indistinguishable from one that
/// has stopped answering.
///
/// The total comes off the central directory, which is parsed before the first
/// entry is touched, so knowing it costs nothing. It is `0` for an archive that
/// declines to say — one written with data descriptors, which this writer never
/// produces — and a caller given a total of nought should show that it is
/// working rather than how far along it is.
pub fn read_watched<R: Read + Seek>(
    reader: R,
    mut watch: impl FnMut(u64, u64),
) -> Result<Document> {
    let mut zip = ZipArchive::new(reader).context("not a readable ZIP archive")?;
    let expected = u64::try_from(zip.decompressed_size().unwrap_or(0)).unwrap_or(u64::MAX);

    let mut board_json: Option<Value> = None;
    let mut manifest_json: Option<Value> = None;
    let mut assets: HashMap<String, Asset> = HashMap::new();
    let mut waveforms: HashMap<String, Waveform> = HashMap::new();
    let mut notes: HashMap<String, String> = HashMap::new();

    let mut total: u64 = 0;
    for i in 0..zip.len() {
        let entry = zip.by_index(i)?;
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
        let declared = entry.size();
        if declared > MAX_ENTRY {
            continue;
        }
        total = total.saturating_add(declared);
        if total > MAX_ARCHIVE {
            bail!("archive is larger than the format allows");
        }

        // **`declared` is a number the archive chose about itself, and the
        // read below is what stops it being taken on trust.** `zip` bounds the
        // *compressed* side of an entry — `.take(compressed_size)` on the way
        // into the decompressor — and nothing bounds the other, so an entry
        // that says a hundred bytes and inflates to a gigabyte would be a
        // gigabyte in this `Vec` before the CRC at the end of the stream had a
        // chance to disagree. The ceiling above would have been checked against
        // the lie rather than against the bytes.
        //
        // So the read is capped at one byte past what was promised, and an
        // entry that overruns is dropped like any other malformed one. That
        // also turns the two ceilings into limits on bytes that actually
        // arrive, which is what they were written to be.
        let mut bytes = Vec::with_capacity(declared.min(1 << 20) as usize);
        let read = entry.take(declared + 1).read_to_end(&mut bytes)? as u64;
        if read != declared {
            continue;
        }
        // After the read rather than before it, so the number describes work
        // that has happened rather than work about to.
        watch(total, expected);

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
    //
    // **Every note is a Markdown file, and the sidecar is that file.** A note
    // whose words fit the head lives in `meta.text` alone; one that has
    // outgrown it keeps the full text as an asset, keyed by its own hash like
    // any other embedded bytes, with `meta.text` stepped down to the derived
    // head it already is for every asset-backed note. This loop is where both
    // halves of that promise are kept against a hand-edited sidecar:
    //
    // - A note with **no asset** that has grown past `NOTE_MAX` is promoted,
    //   not clipped. Its only copy used to be `meta.text` itself, so a person
    //   who opened the `.md` by hand and kept writing had everything past the
    //   512th character silently dropped on the next open — the one case this
    //   format's "legible archive" promise was actually a trap.
    //
    // - A note **with a textual asset** whose sidecar no longer matches it has
    //   been edited by hand, and the edit wins: the sidecar's text becomes a
    //   new asset and the item points at it. Guarded against sidecars a
    //   pre-0.4 build wrote, which carried only the head — a sidecar equal to
    //   the asset's own head is indistinguishable from an untouched one and
    //   must not shear a long note down to its first 512 characters.
    //
    // Every path above and below this one already prefers the asset where
    // there is one (see `opened::words_of` in the app crate).
    for item in &mut board.items {
        if item.kind != ItemType::Note {
            continue;
        }
        let Some(raw) = notes.get(&item.id) else { continue };
        let text = raw.trim_end_matches('\n');
        let rewrite = match item.asset.as_ref().and_then(ItemAsset::hash) {
            Some(hash) => match assets.get(hash) {
                Some(asset) if crate::preview::readable_text(&asset.bytes) => {
                    let whole = String::from_utf8_lossy(&asset.bytes);
                    let whole = whole.trim_end_matches('\n');
                    let head: String = whole.chars().take(NOTE_MAX).collect();
                    (text != whole && text != head.trim_end_matches('\n'))
                        .then(|| (asset.ext.clone(), asset.label.clone()))
                }
                // An asset that is not words — a retyped image card, or bytes
                // the archive lost — is not what the sidecar was derived from,
                // so the sidecar stays what it always was for such a note: the
                // head, and nothing more.
                _ => None,
            },
            None => (text.chars().count() > NOTE_MAX).then(|| ("md".into(), String::new())),
        };
        if let Some((ext, label)) = rewrite {
            let bytes = text.as_bytes().to_vec();
            let hash = hash_bytes(&bytes);
            assets.entry(hash.clone()).or_insert(Asset { bytes, ext, label });
            item.asset = Some(ItemAsset::Embedded { hash, family: None });
        }
        let head: String = text.chars().take(NOTE_MAX).collect();
        item.meta.insert("text".into(), Value::String(head));
        // `rich` flattens to `text` and is authoritative over it, so a hand
        // edit to the Markdown has to drop it or the edit would be shown
        // only until the next render.
        item.meta.remove("rich");
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

    // The one measurement the file records rather than derives. It is stamped
    // here and nowhere else, because here is where the board stops being
    // something being edited and becomes something somebody else will open —
    // possibly straight into a layout where the measurement cannot be taken.
    // See `fence` for why it is written down at all.
    crate::fence::stamp(&mut board.items);

    // **The bin does not go in the file.** It is a within-a-session thing here
    // — see `model::TrashEntry`, which is where the reasoning is written down —
    // and this is the one line that makes that true, because it is the one
    // place the board stops being edited and starts being written.
    //
    // Emptied rather than left out, so that the section is still there and
    // still an array. A `.mbrd` without a `trash` key would be a `.mbrd` an
    // older reader has to guess about, and the answer it should guess is
    // exactly "empty".
    //
    // What this costs is a bin somebody filled in another app, which is gone
    // the first time this one saves. What it buys is that deleting a
    // photograph and saving actually removes the photograph, rather than
    // keeping its bytes indefinitely against a restore that this app has no
    // way to perform.
    board.trash.clear();

    zip.start_file("board.json", deflated)?;
    let filed = state::to_value(&board, doc.board.timeline());
    zip.write_all(serde_json::to_string_pretty(&filed)?.as_bytes())?;

    // Only hashes a live item still names are required, so deleting things and
    // saving actually shrinks the file. A binned one is not among them — the
    // bin is not written at all, and what an undo step still wants is answered
    // below, where a missing one is walked past rather than fatal.
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

    // The note as a file: each note's words, whole, as Markdown. For a note
    // that outgrew its head this is a copy of the asset — the *unabridged*
    // text, not the `meta.text` head, or "the `.md` outranks `board.json`"
    // would hand a person a truncated file and honor their edit to it. A note
    // whose id is not filename-safe simply does not get one: its words are
    // still in `board.json` or under `assets/`, so nothing is lost but the
    // legibility.
    for item in &doc.board.items {
        if item.kind != ItemType::Note {
            continue;
        }
        let Some(head) = item.note_text() else { continue };
        if !filename_safe(&item.id) {
            continue;
        }
        let whole = item
            .asset
            .as_ref()
            .and_then(ItemAsset::hash)
            .and_then(|hash| doc.assets.get(hash))
            .filter(|asset| crate::preview::readable_text(&asset.bytes))
            .map(|asset| String::from_utf8_lossy(&asset.bytes).into_owned());
        let text = whole.as_deref().unwrap_or(head).trim_end_matches('\n');
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

    #[test]
    fn a_read_says_how_far_it_has_got_and_finishes_at_the_whole_archive() {
        // The numbers the opening loader is drawn from. A reading that never
        // reached the total would leave the bar short of the end on every
        // board, which is the one way a progress bar can lie that people
        // notice.
        let doc = doc_with_a_photo_and_a_note();
        let bytes = to_bytes(&doc, "2024-01-01T00:00:00Z").expect("packs");

        let mut seen: Vec<(u64, u64)> = Vec::new();
        let back = read_watched(Cursor::new(bytes), |done, total| seen.push((done, total)))
            .expect("reads");
        assert_eq!(back.board.items.len(), doc.board.items.len());

        assert!(!seen.is_empty(), "a board with entries in it should report on them");
        let total = seen[0].1;
        assert!(total > 0, "this writer stores sizes, so the total is knowable");
        assert!(seen.iter().all(|(_, t)| *t == total), "the total should not move");

        // Never backwards, never past the end, and it arrives.
        assert!(seen.windows(2).all(|w| w[0].0 <= w[1].0), "progress went backwards: {seen:?}");
        assert!(seen.iter().all(|(done, _)| *done <= total), "past the end: {seen:?}");
        assert_eq!(seen.last().map(|(done, _)| *done), Some(total), "{seen:?}");
    }

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

    /// The archive with every `notes/` entry rewritten to `text` — the
    /// hand-edit these tests are about, done the way a person does it: unzip,
    /// write over the file, rezip.
    fn with_sidecars_rewritten(packed: Vec<u8>, text: &str) -> Vec<u8> {
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
                out.write_all(text.as_bytes()).unwrap();
            } else {
                out.write_all(&body).unwrap();
            }
        }
        out.finish().unwrap().into_inner()
    }

    /// The same archive with one entry's declared uncompressed size rewritten.
    ///
    /// That number lives in the central directory, which is where `entry.size()`
    /// reads it from, and rewriting it is the whole of what a zip bomb is: the
    /// claim stops describing what comes out of the decompressor. The compressed
    /// side is left exactly as it was, so the entry still inflates in full.
    fn understating(packed: Vec<u8>, prefix: &str, claim: u32) -> Vec<u8> {
        let mut out = packed;
        let mut at = 0;
        while let Some(found) = out[at..].windows(4).position(|w| w == b"PK\x01\x02") {
            let record = at + found;
            let name_len = u16::from_le_bytes([out[record + 28], out[record + 29]]) as usize;
            let name = &out[record + 46..record + 46 + name_len];
            if name.starts_with(prefix.as_bytes()) {
                out[record + 24..record + 28].copy_from_slice(&claim.to_le_bytes());
                return out;
            }
            at = record + 4;
        }
        panic!("no entry under {prefix} in the archive");
    }

    #[test]
    fn an_entry_that_inflates_past_what_it_declared_is_dropped() {
        // `MAX_ENTRY` and `MAX_ARCHIVE` are checked against `entry.size()`,
        // which is a number the archive writes about itself — and `zip` bounds
        // only the *compressed* side of a read. So an archive that understates
        // an entry used to have it read in full, past both ceilings and into
        // memory, with the CRC only disagreeing once the damage was done.
        let doc = doc_with_a_photo_and_a_note();
        let packed = to_bytes(&doc, "now").unwrap();

        // A megabyte of one repeated byte — a few hundred bytes on disk, and
        // the shape every bomb has. Small here because the assertion is about
        // the guard, not about how far it can be pushed.
        let fat = "z".repeat(1024 * 1024);
        let rezipped = with_sidecars_rewritten(packed, &fat);
        let lying = understating(rezipped, "notes/", 16);

        // The archive still opens: one bad entry is dropped like any other
        // malformed one, rather than taking the board down with it.
        let back = read(Cursor::new(lying)).unwrap();

        // And the sidecar was dropped rather than believed, so the note still
        // says what `board.json` says. Had the read been honoured it would
        // have outranked `board.json` and been promoted to its own asset —
        // see the two tests above, which are that path working.
        let note = back.board.item("p81m4x").unwrap();
        assert_eq!(note.note_text(), Some("# buy the smaller one\n\nthe big one does not fit"));
        assert!(note.asset.is_none(), "a dropped sidecar must not promote anything");
    }

    #[test]
    fn a_hand_edited_note_outranks_board_json() {
        // The whole point of writing the sidecars: unzip, edit, rezip, and the
        // board opens with your words.
        let doc = doc_with_a_photo_and_a_note();
        let packed = to_bytes(&doc, "now").unwrap();
        let rezipped = with_sidecars_rewritten(packed, "# get the big one after all\n");

        let back = read(Cursor::new(rezipped)).unwrap();
        let note = back.board.item("p81m4x").unwrap();
        assert_eq!(note.note_text(), Some("# get the big one after all"));
    }

    #[test]
    fn a_hand_edited_note_that_grows_past_note_max_is_promoted_not_clipped() {
        // Before this fix, a note's only copy of its words was `meta.text`
        // itself, so growing the `.md` sidecar past 512 characters by hand and
        // reopening it silently dropped everything past the 512th character.
        let doc = doc_with_a_photo_and_a_note();
        let packed = to_bytes(&doc, "now").unwrap();

        let long = "x".repeat(NOTE_MAX * 3);
        let rezipped = with_sidecars_rewritten(packed, &long);

        let back = read(Cursor::new(rezipped)).unwrap();
        let note = back.board.item("p81m4x").unwrap();

        // `meta.text` is now the derived head, not the whole thing.
        assert_eq!(note.note_text().unwrap().chars().count(), NOTE_MAX);

        // The full text survived, as the note's own asset.
        let hash = note.asset.as_ref().and_then(ItemAsset::hash).expect("promoted to an asset");
        let asset = back.assets.get(hash).expect("the asset's bytes are in the archive");
        assert_eq!(asset.bytes, long.as_bytes());
        assert_eq!(asset.ext, "md");
    }

    /// A note whose words outgrew the head: the full text as its own asset,
    /// `meta.text` holding only the first `NOTE_MAX` characters.
    fn doc_with_a_grown_note() -> (Document, String) {
        let text = format!("# groceries\n\n{}", "get the big one. ".repeat(64));
        let text = text.trim_end().to_string();
        let bytes = text.as_bytes().to_vec();
        let hash = hash_bytes(&bytes);

        let mut board = Board { title: "Kitchen".into(), ..Board::default() };
        let mut note = Item::new("p81m4x", ItemType::Note);
        note.name = "groceries".into();
        note.meta
            .insert("text".into(), Value::String(text.chars().take(NOTE_MAX).collect::<String>()));
        note.asset = Some(ItemAsset::Embedded { hash: hash.clone(), family: None });
        board.items.push(note);

        let mut assets = HashMap::new();
        assets.insert(hash, Asset { bytes, ext: "md".into(), label: "groceries".into() });

        let doc = Document {
            manifest: Manifest::default(),
            board: BoardState::new(board),
            assets,
            waveforms: HashMap::new(),
        };
        (doc, text)
    }

    #[test]
    fn an_asset_backed_notes_sidecar_carries_the_whole_text_not_the_head() {
        // "Every note is a Markdown file" is this entry. A sidecar that held
        // only the head would hand a person a truncated file, honor their
        // edit to it, and call the result their note.
        let (doc, text) = doc_with_a_grown_note();
        let packed = to_bytes(&doc, "now").unwrap();

        let mut zip = ZipArchive::new(Cursor::new(packed)).unwrap();
        let mut sidecar = None;
        for i in 0..zip.len() {
            let mut e = zip.by_index(i).unwrap();
            if e.name().starts_with("notes/") {
                let mut b = Vec::new();
                e.read_to_end(&mut b).unwrap();
                sidecar = Some(String::from_utf8(b).unwrap());
            }
        }
        assert_eq!(sidecar.as_deref(), Some(format!("{text}\n").as_str()));
    }

    #[test]
    fn a_hand_edit_to_an_asset_backed_notes_sidecar_outranks_the_asset() {
        // The same promise `a_hand_edited_note_outranks_board_json` makes for
        // a bare note, kept for one whose words live in an asset — and kept
        // even when the edit is *shorter* than the head, which is the case no
        // length test can catch. The asset's ext rides along, so a dropped
        // `.md` stays a `.md` through the edit.
        let (doc, _) = doc_with_a_grown_note();
        let packed = to_bytes(&doc, "now").unwrap();
        let rezipped = with_sidecars_rewritten(packed, "actually, the small one\n");

        let back = read(Cursor::new(rezipped)).unwrap();
        let note = back.board.item("p81m4x").unwrap();
        assert_eq!(note.note_text(), Some("actually, the small one"));

        let hash = note.asset.as_ref().and_then(ItemAsset::hash).expect("still asset-backed");
        let asset = back.assets.get(hash).expect("re-pointed at real bytes");
        assert_eq!(asset.bytes, b"actually, the small one");
        assert_eq!(asset.ext, "md");
    }

    #[test]
    fn a_head_only_sidecar_from_an_older_build_leaves_the_asset_alone() {
        // A pre-0.4 build wrote the sidecar from `meta.text` — the head — so
        // every long note it saved has a sidecar that is exactly the asset's
        // first `NOTE_MAX` characters. That file was never edited by anybody,
        // and a reader that took it at its word would shear the note down to
        // its head on the next open: the original clipping bug, rebuilt.
        let (doc, text) = doc_with_a_grown_note();
        let packed = to_bytes(&doc, "now").unwrap();
        let head: String = text.chars().take(NOTE_MAX).collect();
        let rezipped = with_sidecars_rewritten(packed, &format!("{head}\n"));

        let back = read(Cursor::new(rezipped)).unwrap();
        let note = back.board.item("p81m4x").unwrap();

        let hash = note.asset.as_ref().and_then(ItemAsset::hash).expect("still asset-backed");
        let asset = back.assets.get(hash).expect("the bytes survived");
        assert_eq!(asset.bytes, text.as_bytes(), "the whole text, untouched");
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

    #[test]
    fn the_bin_does_not_reach_the_file() {
        // The bin lasts as long as the app is open and no longer. See
        // `model::TrashEntry` for why a section of the format is deliberately
        // dropped on the way out.
        let mut doc = doc_with_a_photo_and_a_note();
        doc.board.edit_at("To the bin", 1, |board| {
            let item = board.items.remove(0);
            board.trash.insert(0, crate::model::TrashEntry { item, at: 1 });
        });
        assert_eq!(doc.board.trash.len(), 1, "it should be in the bin in memory");

        let bytes = to_bytes(&doc, "2026-07-25T10:04:11.882Z").unwrap();
        let back = read(Cursor::new(bytes)).unwrap();

        assert!(back.board.trash.is_empty(), "the bin was written to the file");
        assert_eq!(back.board.items.len(), 1, "the note should be all that is left");
    }

    #[test]
    fn a_binned_picture_is_still_there_for_an_undo_to_find() {
        // The other half of the same decision, and the half that makes it safe:
        // the deleted photograph's bytes leave the *required* set and are
        // carried by the ledger instead — optionally, for as long as a step
        // still names them. Undo is the recovery route now, so it has to work
        // across a save.
        let mut doc = doc_with_a_photo_and_a_note();
        let photo = doc.board.items[0].id.clone();
        doc.board.edit_at("To the bin", 1, |board| {
            let item = board.items.remove(0);
            board.trash.insert(0, crate::model::TrashEntry { item, at: 1 });
        });

        let bytes = to_bytes(&doc, "2026-07-25T10:04:11.882Z").unwrap();
        let mut back = read(Cursor::new(bytes)).unwrap();

        assert_eq!(back.assets.len(), 1, "the ledger still wants the picture's bytes");
        assert_eq!(back.board.undo().as_deref(), Some("To the bin"), "the step did not survive");
        assert!(
            back.board.items.iter().any(|it| it.id == photo),
            "an undo could not bring the picture back",
        );
    }

    #[test]
    fn a_picture_only_the_bin_names_leaves_the_file() {
        // What emptying the bin at the boundary is *for*, isolated from the
        // ledger: this board has no history at all, so the bin is the only
        // thing naming the photograph — and a picture nothing on the board
        // names should not be carried.
        let doc = doc_with_a_photo_and_a_note();
        let mut board: Board = (*doc.board).clone();
        let item = board.items.remove(0);
        board.trash.push(crate::model::TrashEntry { item, at: 1 });
        let doc = Document { board: BoardState::new(board), ..doc };

        let bytes = to_bytes(&doc, "2026-07-25T10:04:11.882Z").unwrap();
        let back = read(Cursor::new(bytes)).unwrap();
        assert!(back.board.trash.is_empty());
        assert!(back.assets.is_empty(), "the deleted photograph is still in the file");
    }
}
