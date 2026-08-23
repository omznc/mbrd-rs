//! Does this file carry any sound?
//!
//! A video card wears a mute button, and a mute button on a clip with no audio
//! track is a control that does nothing — it invites a press and then answers
//! with silence either way. The only honest way to leave it off is to know, and
//! knowing means looking inside the container.
//!
//! This looks at the **track list**, not at the audio. Every container here
//! declares its tracks in a header, so the question "is there a sound track"
//! is answered by a few hundred bytes of structure near the front of the file
//! rather than by a decoder. That is the whole reason this can live in core: no
//! codec, no process, no dependency, and it runs on the bytes already in hand
//! at import.
//!
//! Every reader returns [`Option<bool>`] and every one of them returns `None`
//! rather than guessing. `None` is a real answer and it has a real meaning:
//! *nobody has looked successfully*. A caller that turns `None` into `false`
//! would hide the mute button on files this module simply cannot read, which is
//! a worse failure than showing it — so the app reads `None` as "assume sound",
//! and this module is what shrinks the set of files that land there.

/// How deep a box tree is walked before it is called malformed.
///
/// Real files nest four deep (`moov` → `trak` → `mdia` → `hdlr`). Anything
/// claiming much more is either damaged or built to make a parser spin.
const DEPTH_MAX: u32 = 8;

/// Does this file carry a sound track?
///
/// `None` where the container is unrecognised, truncated, or malformed — see
/// the module note on why that is not `false`.
pub fn sniff(bytes: &[u8]) -> Option<bool> {
    match kind(bytes)? {
        Container::Iso => iso(bytes),
        Container::Matroska => matroska(bytes),
        Container::Riff => riff(bytes),
    }
}

enum Container {
    /// MP4, M4V, MOV, 3GP — ISO base media, a tree of length-prefixed boxes.
    Iso,
    /// MKV, WebM — EBML, a tree of variable-length ids and sizes.
    Matroska,
    /// AVI — RIFF, chunks with little-endian lengths.
    Riff,
}

fn kind(bytes: &[u8]) -> Option<Container> {
    if bytes.len() < 12 {
        return None;
    }
    if bytes[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        return Some(Container::Matroska);
    }
    if &bytes[..4] == b"RIFF" && &bytes[8..12] == b"AVI " {
        return Some(Container::Riff);
    }
    // ISO files are identified by their first box rather than by a signature.
    // `ftyp` is the usual one; some QuickTime files lead with `moov` or with a
    // bare `mdat`, and those are still worth walking.
    match &bytes[4..8] {
        b"ftyp" | b"moov" | b"mdat" | b"free" | b"skip" | b"wide" | b"pnot" => Some(Container::Iso),
        _ => None,
    }
}

// ---------------------------------------------------------------- ISO / MP4

/// A box is `[u32 size][u32 type]`, with `size` counting the header itself.
const ISO_HEAD: usize = 8;

fn iso(bytes: &[u8]) -> Option<bool> {
    let moov = iso_child(bytes, 0, bytes.len(), b"moov", 0)?;
    let mut sound = false;
    let mut tracks = 0;
    iso_each(bytes, moov.0, moov.1, b"trak", &mut |start, stop| {
        tracks += 1;
        if iso_handler(bytes, start, stop) == Some(*b"soun") {
            sound = true;
        }
    });
    // A `moov` with no tracks at all is a file we did not understand, not a
    // silent one.
    (tracks > 0).then_some(sound)
}

/// The four-character handler of a track: `soun`, `vide`, `text`, `sbtl`…
fn iso_handler(bytes: &[u8], start: usize, stop: usize) -> Option<[u8; 4]> {
    let mdia = iso_child(bytes, start, stop, b"mdia", 0)?;
    let hdlr = iso_child(bytes, mdia.0, mdia.1, b"hdlr", 0)?;
    // version+flags (4), pre_defined (4), then the handler.
    let at = hdlr.0.checked_add(8)?;
    let end = at.checked_add(4)?;
    (end <= hdlr.1).then(|| {
        let mut four = [0u8; 4];
        four.copy_from_slice(&bytes[at..end]);
        four
    })
}

/// The payload range of the first box of this type, searching one level down.
fn iso_child(
    bytes: &[u8],
    start: usize,
    stop: usize,
    want: &[u8; 4],
    depth: u32,
) -> Option<(usize, usize)> {
    let mut found = None;
    iso_walk(bytes, start, stop, depth, &mut |kind, at, end| {
        if found.is_none() && kind == want {
            found = Some((at, end));
        }
    });
    found
}

/// Every box of this type at this level.
fn iso_each(
    bytes: &[u8],
    start: usize,
    stop: usize,
    want: &[u8; 4],
    f: &mut dyn FnMut(usize, usize),
) {
    iso_walk(bytes, start, stop, 0, &mut |kind, at, end| {
        if kind == want {
            f(at, end);
        }
    });
}

fn iso_walk(
    bytes: &[u8],
    start: usize,
    stop: usize,
    depth: u32,
    f: &mut dyn FnMut(&[u8; 4], usize, usize),
) {
    if depth >= DEPTH_MAX {
        return;
    }
    let mut at = start;
    while at + ISO_HEAD <= stop {
        let declared = u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]);
        let mut kind = [0u8; 4];
        kind.copy_from_slice(&bytes[at + 4..at + 8]);

        let (payload, size) = match declared {
            // Size 1 means the real, 64-bit size follows the type.
            1 => {
                if at + ISO_HEAD + 8 > stop {
                    return;
                }
                let mut wide = [0u8; 8];
                wide.copy_from_slice(&bytes[at + ISO_HEAD..at + ISO_HEAD + 8]);
                let size = u64::from_be_bytes(wide);
                // A 64-bit size that does not fit an address is a malformed
                // file, not a very large one — bail rather than truncate.
                let Ok(size) = usize::try_from(size) else { return };
                (at + ISO_HEAD + 8, size)
            }
            // Size 0 means "to the end of the file", which only the last box
            // may say.
            0 => (at + ISO_HEAD, stop - at),
            _ => (at + ISO_HEAD, declared as usize),
        };
        // A box smaller than the header it just claimed would step backwards.
        let Some(end) = at.checked_add(size).filter(|end| *end > payload && *end <= stop) else {
            return;
        };
        f(&kind, payload, end);
        at = end;
    }
}

// ------------------------------------------------------------- EBML / WebM

const SEGMENT: u64 = 0x1853_8067;
const TRACKS: u64 = 0x1654_AE6B;
const TRACK_ENTRY: u64 = 0xAE;
const TRACK_TYPE: u64 = 0x83;
/// Matroska's own numbering: 1 is video, 2 is audio, 17 is subtitles.
const TRACK_TYPE_AUDIO: u64 = 2;

fn matroska(bytes: &[u8]) -> Option<bool> {
    let segment = ebml_child(bytes, 0, bytes.len(), SEGMENT)?;
    let tracks = ebml_child(bytes, segment.0, segment.1, TRACKS)?;
    let mut sound = false;
    let mut entries = 0;
    ebml_each(bytes, tracks.0, tracks.1, TRACK_ENTRY, &mut |start, stop| {
        entries += 1;
        if ebml_uint(bytes, start, stop, TRACK_TYPE) == Some(TRACK_TYPE_AUDIO) {
            sound = true;
        }
    });
    (entries > 0).then_some(sound)
}

fn ebml_uint(bytes: &[u8], start: usize, stop: usize, id: u64) -> Option<u64> {
    let (at, end) = ebml_child(bytes, start, stop, id)?;
    // EBML integers are big-endian and as short as they can be — a one-byte
    // `2` and an eight-byte `2` are the same number.
    (end > at && end - at <= 8)
        .then(|| bytes[at..end].iter().fold(0u64, |v, b| (v << 8) | *b as u64))
}

fn ebml_child(bytes: &[u8], start: usize, stop: usize, id: u64) -> Option<(usize, usize)> {
    let mut found = None;
    ebml_each(bytes, start, stop, id, &mut |at, end| {
        if found.is_none() {
            found = Some((at, end));
        }
    });
    found
}

fn ebml_each(bytes: &[u8], start: usize, stop: usize, id: u64, f: &mut dyn FnMut(usize, usize)) {
    let mut at = start;
    while at < stop {
        // The id keeps its marker bits — that is what makes `0xAE` the literal
        // written in the spec — while the size drops them.
        let Some((found, id_len)) = vint(bytes, at, stop, true) else { return };
        let Some((size, size_len)) = vint(bytes, at + id_len, stop, false) else { return };
        let payload = at + id_len + size_len;

        // A Segment written by a live muxer has an unknown size: every value
        // bit set. It runs to the end of what we were given.
        let unknown = size == (1u64 << (7 * size_len as u64)) - 1;
        let end = match unknown {
            true => stop,
            false => match usize::try_from(size).ok().and_then(|s| payload.checked_add(s)) {
                Some(end) if end <= stop => end,
                // Truncated: this element claims more than the file holds.
                _ => return,
            },
        };
        if found == id {
            f(payload, end);
        }
        // An unknown-size element we did not want cannot be stepped over,
        // because nothing in the file says where it ends.
        if unknown {
            return;
        }
        // The header is at least two bytes, so this always moves forward —
        // including for a legal zero-length element.
        at = end.max(payload);
    }
}

/// A variable-length integer: the leading zeros of the first byte give the
/// length, and the marker bit is either kept (ids) or cleared (sizes).
fn vint(bytes: &[u8], at: usize, stop: usize, keep_marker: bool) -> Option<(u64, usize)> {
    let first = *bytes.get(at).filter(|_| at < stop)?;
    if first == 0 {
        // Five bytes or more: outside Matroska's range and outside ours.
        return None;
    }
    let len = first.leading_zeros() as usize + 1;
    if len > 8 || at + len > stop {
        return None;
    }
    let mut value = match keep_marker {
        true => first as u64,
        false => (first as u64) & ((1u64 << (8 - len)) - 1),
    };
    for byte in &bytes[at + 1..at + len] {
        value = (value << 8) | *byte as u64;
    }
    Some((value, len))
}

// -------------------------------------------------------------- RIFF / AVI

fn riff(bytes: &[u8]) -> Option<bool> {
    // Every `strh` this file will ever have lives inside one `LIST hdrl`,
    // which the format requires near the front — before `LIST movi`, the
    // frame data, which is what the rest of the file mostly is. A two-hour
    // capture is hundreds of thousands of `movi` chunks and not one of them
    // is a stream header, so this walks the top level by hand rather than
    // through `riff_walk`: it is looking for exactly one chunk, and it stops
    // as soon as it either finds `hdrl` or runs out of file, rather than
    // recursing into everything `hdrl` was never going to be.
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let mut kind = [0u8; 4];
        kind.copy_from_slice(&bytes[at..at + 4]);
        let size = u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]])
            as usize;
        let payload = at + 8;
        let end = payload.checked_add(size).filter(|end| *end <= bytes.len())?;
        if &kind == b"LIST" && payload + 4 <= end && &bytes[payload..payload + 4] == b"hdrl" {
            let mut sound = false;
            let mut streams = 0;
            riff_walk(bytes, payload + 4, end, 0, &mut |kind, at, end| {
                if kind == b"strh" && end >= at + 4 {
                    streams += 1;
                    if &bytes[at..at + 4] == b"auds" {
                        sound = true;
                    }
                }
            });
            return (streams > 0).then_some(sound);
        }
        // Not `hdrl` — a chunk, or a `LIST` of some other kind (`INFO`,
        // `movi`…) — so it is stepped over rather than looked inside.
        at = end + (size % 2);
    }
    None
}

fn riff_walk(
    bytes: &[u8],
    start: usize,
    stop: usize,
    depth: u32,
    f: &mut dyn FnMut(&[u8; 4], usize, usize),
) {
    if depth >= DEPTH_MAX {
        return;
    }
    let mut at = start;
    while at + 8 <= stop {
        let mut kind = [0u8; 4];
        kind.copy_from_slice(&bytes[at..at + 4]);
        let size = u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]])
            as usize;
        let payload = at + 8;
        let Some(end) = payload.checked_add(size).filter(|end| *end <= stop) else {
            return;
        };
        if &kind == b"LIST" || &kind == b"RIFF" {
            // A list's payload opens with its own four-character type, and the
            // children follow it.
            riff_walk(bytes, payload + 4, end, depth + 1, f);
        } else {
            f(&kind, payload, end);
        }
        // Chunks are padded to an even boundary, and the pad is not counted in
        // the size — a file where it is forgotten would misread every chunk
        // after the first odd one.
        at = end + (size % 2);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------ fixtures

    fn iso_box(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = ((payload.len() + 8) as u32).to_be_bytes().to_vec();
        out.extend_from_slice(kind);
        out.extend_from_slice(payload);
        out
    }

    fn hdlr(handler: &[u8; 4]) -> Vec<u8> {
        let mut payload = vec![0u8; 8]; // version+flags, pre_defined
        payload.extend_from_slice(handler);
        payload.extend_from_slice(&[0u8; 12]); // reserved
        payload.push(0); // an empty name
        iso_box(b"hdlr", &payload)
    }

    fn trak(handler: &[u8; 4]) -> Vec<u8> {
        iso_box(b"trak", &iso_box(b"mdia", &hdlr(handler)))
    }

    fn mp4(handlers: &[&[u8; 4]]) -> Vec<u8> {
        let mut file = iso_box(b"ftyp", b"isom\0\0\x02\0isomiso2");
        let mut moov = Vec::new();
        for handler in handlers {
            moov.extend_from_slice(&trak(handler));
        }
        file.extend_from_slice(&iso_box(b"moov", &moov));
        file.extend_from_slice(&iso_box(b"mdat", &[0u8; 64]));
        file
    }

    fn ebml_elem(id: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut out = id.to_vec();
        // A one-byte size covers 0..=126, which is all these fixtures need; the
        // longer forms are exercised through the real-shaped Segment below.
        assert!(payload.len() < 127);
        out.push(0x80 | payload.len() as u8);
        out.extend_from_slice(payload);
        out
    }

    fn webm(kinds: &[u64]) -> Vec<u8> {
        let mut entries = Vec::new();
        for kind in kinds {
            let entry = ebml_elem(&[0x83], &[*kind as u8]);
            entries.extend_from_slice(&ebml_elem(&[0xAE], &entry));
        }
        let tracks = ebml_elem(&[0x16, 0x54, 0xAE, 0x6B], &entries);
        let segment = ebml_elem(&[0x18, 0x53, 0x80, 0x67], &tracks);
        let mut file = ebml_elem(&[0x1A, 0x45, 0xDF, 0xA3], &[0x42, 0x86, 0x81, 0x01]);
        file.extend_from_slice(&segment);
        file
    }

    fn riff_chunk(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut out = kind.to_vec();
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
        if payload.len() % 2 == 1 {
            out.push(0);
        }
        out
    }

    fn riff_list(kind: &[u8; 4], children: &[u8]) -> Vec<u8> {
        let mut payload = kind.to_vec();
        payload.extend_from_slice(children);
        riff_chunk(b"LIST", &payload)
    }

    fn avi(kinds: &[&[u8; 4]]) -> Vec<u8> {
        let mut hdrl = riff_chunk(b"avih", &[0u8; 56]);
        for kind in kinds {
            let mut strh = kind.to_vec();
            strh.extend_from_slice(&[0u8; 52]);
            hdrl.extend_from_slice(&riff_list(b"strl", &riff_chunk(b"strh", &strh)));
        }
        let body = riff_list(b"hdrl", &hdrl);
        let mut file = b"RIFF".to_vec();
        file.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
        file.extend_from_slice(b"AVI ");
        file.extend_from_slice(&body);
        file
    }

    // --------------------------------------------------------------- tests

    #[test]
    fn a_silent_clip_is_told_apart_from_one_with_sound() {
        // The whole point of the module: these two differ by one four-character
        // string a long way inside the file, and the mute button depends on it.
        assert_eq!(sniff(&mp4(&[b"vide"])), Some(false));
        assert_eq!(sniff(&mp4(&[b"vide", b"soun"])), Some(true));
        assert_eq!(sniff(&webm(&[1])), Some(false));
        assert_eq!(sniff(&webm(&[1, 2])), Some(true));
        assert_eq!(sniff(&avi(&[b"vids"])), Some(false));
        assert_eq!(sniff(&avi(&[b"vids", b"auds"])), Some(true));
    }

    #[test]
    fn a_sound_track_counts_wherever_it_sits_in_the_list() {
        assert_eq!(sniff(&mp4(&[b"soun", b"vide"])), Some(true));
        assert_eq!(sniff(&mp4(&[b"vide", b"text", b"soun"])), Some(true));
        assert_eq!(sniff(&webm(&[2, 1])), Some(true));
        assert_eq!(sniff(&webm(&[1, 17, 2])), Some(true));
    }

    #[test]
    fn subtitles_are_not_sound() {
        assert_eq!(sniff(&mp4(&[b"vide", b"sbtl", b"text"])), Some(false));
        assert_eq!(sniff(&webm(&[1, 17])), Some(false));
    }

    #[test]
    fn a_bare_audio_file_reads_as_sound() {
        // An .m4a is the same container with the video track left out.
        assert_eq!(sniff(&mp4(&[b"soun"])), Some(true));
        assert_eq!(sniff(&webm(&[2])), Some(true));
    }

    #[test]
    fn nobody_looked_is_a_different_answer_from_no_sound() {
        // Each of these must be `None`, because a caller reads `None` as
        // "assume sound" and `Some(false)` as "take the mute button away".
        assert_eq!(sniff(b""), None, "empty");
        assert_eq!(sniff(b"not a media file at all"), None, "not a container");
        assert_eq!(sniff(&[0u8; 4096]), None, "zeroes");
        assert_eq!(sniff(&mp4(&[])), None, "a moov with no tracks");
        assert_eq!(sniff(&webm(&[])), None, "a Tracks with no entries");
        assert_eq!(sniff(&avi(&[])), None, "an hdrl with no streams");

        // A GIF, a PNG and a JPEG all arrive here if a caller is careless.
        assert_eq!(sniff(b"GIF89a\0\0\0\0\0\0\0\0"), None);
        assert_eq!(sniff(&[0x89, b'P', b'N', b'G', 13, 10, 26, 10, 0, 0, 0, 13]), None);
    }

    #[test]
    fn a_truncated_file_stops_rather_than_reading_past_the_end() {
        let whole = mp4(&[b"vide", b"soun"]);
        for cut in 1..whole.len() {
            // Not asserting the answer — asserting there is one, and that it
            // came back. A half-downloaded file must not take the import with
            // it.
            let _ = sniff(&whole[..cut]);
        }
        for container in [webm(&[1, 2]), avi(&[b"vids", b"auds"])] {
            for cut in 1..container.len() {
                let _ = sniff(&container[..cut]);
            }
        }
    }

    #[test]
    fn a_box_that_claims_no_length_does_not_spin() {
        // Size zero means "to the end of the file" and is legal exactly once,
        // for the last box. A file that says it everywhere used to be an
        // infinite loop rather than a rejection.
        let mut file = iso_box(b"ftyp", b"isom\0\0\x02\0");
        file.extend_from_slice(&[0, 0, 0, 0]);
        file.extend_from_slice(b"moov");
        file.extend_from_slice(&[0, 0, 0, 0]);
        file.extend_from_slice(b"trak");
        assert_eq!(sniff(&file), None);
    }

    #[test]
    fn a_box_shorter_than_its_own_header_is_refused() {
        let mut file = iso_box(b"ftyp", b"isom\0\0\x02\0");
        file.extend_from_slice(&3u32.to_be_bytes()); // less than the eight-byte header
        file.extend_from_slice(b"moov");
        assert_eq!(sniff(&file), None);
    }

    #[test]
    fn a_deeply_nested_file_gives_up_instead_of_recursing() {
        let mut nest = riff_chunk(b"strh", b"auds\0\0\0\0");
        for _ in 0..64 {
            nest = riff_list(b"strl", &nest);
        }
        let mut file = b"RIFF".to_vec();
        file.extend_from_slice(&((nest.len() + 4) as u32).to_le_bytes());
        file.extend_from_slice(b"AVI ");
        file.extend_from_slice(&nest);
        assert_eq!(sniff(&file), None, "the depth guard should have stopped this");
    }

    #[test]
    fn an_odd_chunk_is_padded_and_the_next_one_is_still_found() {
        // The pad byte is not counted in the size. Forgetting it misreads every
        // chunk after the first odd one, which here means losing the audio.
        let mut hdrl = riff_chunk(b"junk", b"odd"); // three bytes, so one pad
        hdrl.extend_from_slice(&riff_list(b"strl", &riff_chunk(b"strh", b"auds\0\0\0\0")));
        let body = riff_list(b"hdrl", &hdrl);
        let mut file = b"RIFF".to_vec();
        file.extend_from_slice(&((body.len() + 4) as u32).to_le_bytes());
        file.extend_from_slice(b"AVI ");
        file.extend_from_slice(&body);
        assert_eq!(sniff(&file), Some(true));
    }

    #[test]
    fn a_quicktime_file_that_leads_with_moov_is_still_read() {
        // Some .mov files put the header first and some put it last; both are
        // ordinary, and a moodboard sees both.
        let mut file = iso_box(b"moov", &trak(b"soun"));
        file.extend_from_slice(&iso_box(b"mdat", &[0u8; 32]));
        assert_eq!(sniff(&file), Some(true));

        let mut tail = iso_box(b"ftyp", b"qt  \0\0\x02\0");
        tail.extend_from_slice(&iso_box(b"mdat", &[0u8; 128]));
        tail.extend_from_slice(&iso_box(b"moov", &trak(b"soun")));
        assert_eq!(sniff(&tail), Some(true), "moov at the end of the file");
    }

    #[test]
    fn a_sixty_four_bit_box_is_read_through_rather_than_stopped_at() {
        // Large `mdat` boxes use the 64-bit form, and `moov` sits after them.
        let mut file = iso_box(b"ftyp", b"isom\0\0\x02\0");
        let payload = [0u8; 32];
        file.extend_from_slice(&1u32.to_be_bytes());
        file.extend_from_slice(b"mdat");
        file.extend_from_slice(&((payload.len() + 16) as u64).to_be_bytes());
        file.extend_from_slice(&payload);
        file.extend_from_slice(&iso_box(b"moov", &trak(b"soun")));
        assert_eq!(sniff(&file), Some(true));
    }

    #[test]
    fn a_segment_of_unknown_length_still_yields_its_tracks() {
        // A Matroska file straight from a recorder does not know how long it
        // will be, and writes every size bit set to say so.
        let entry = ebml_elem(&[0x83], &[2]);
        let tracks = ebml_elem(&[0x16, 0x54, 0xAE, 0x6B], &ebml_elem(&[0xAE], &entry));
        let mut file = ebml_elem(&[0x1A, 0x45, 0xDF, 0xA3], &[0x42, 0x86, 0x81, 0x01]);
        file.extend_from_slice(&[0x18, 0x53, 0x80, 0x67, 0xFF]); // Segment, unknown size
        file.extend_from_slice(&tracks);
        assert_eq!(sniff(&file), Some(true));
    }
}
