//! Turning a board into a filename, and a clock into a timestamp.
//!
//! Both are the file format's business rather than the window's, which is why
//! they live down here: a filename convention that only the UI crate knows is
//! one no test can reach without opening a window.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::Board;

/// Where a board with no file of its own should go.
///
/// The board's title, made safe for a file picker — spaces become underscores,
/// which is the original's `fileNameFor()`. Note that this does **not** rename
/// the board: a title and a filename are different strings, and conflating them
/// is the bug the format's "title repair" exists to undo on old files.
pub fn file_name_for(board: &Board) -> PathBuf {
    let title = board.title.trim();
    let stem: String = if title.is_empty() {
        "untitled".into()
    } else {
        title
            .chars()
            .map(|c| if c.is_whitespace() { '_' } else { c })
            .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
            .collect()
    };
    let stem = if stem.is_empty() { "untitled".into() } else { stem };
    PathBuf::from(format!("{stem}.mbrd"))
}

/// The current time, as a step of the ledger records it.
///
/// Milliseconds since the Unix epoch, which is what the format writes and what
/// a `Date.now()` in the original produces. A clock that has gone backwards
/// gives a negative number rather than a panic: a step with an implausible
/// timestamp is a step with an implausible timestamp, and refusing to record it
/// would lose the edit to protect the date on it.
pub fn now_millis() -> i64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(d) => d.as_millis() as i64,
        Err(err) => -(err.duration().as_millis() as i64),
    }
}

/// The current time, as a manifest wants it.
///
/// Hand-rolled rather than pulling in a date library for one format string.
/// The civil-from-days conversion is Howard Hinnant's, which is exact for every
/// date this will ever see and is not the place to be clever.
pub fn now_iso8601() -> String {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = d.as_secs() as i64;
    let millis = d.subsec_millis();

    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (h, m, s) = (tod / 3600, (tod % 3600) / 60, tod % 60);

    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };

    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}.{millis:03}Z")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_timestamp_is_shaped_the_way_a_manifest_wants_it() {
        let t = now_iso8601();
        assert_eq!(t.len(), 24, "got {t}");
        assert!(t.ends_with('Z'));
        assert_eq!(&t[4..5], "-");
        assert_eq!(&t[10..11], "T");
        // Not before this port was written, and not absurdly after.
        let year: i32 = t[0..4].parse().unwrap();
        assert!((2024..2200).contains(&year), "got {t}");
    }

    #[test]
    fn a_title_becomes_a_filename_without_becoming_the_title() {
        let board = Board { title: "Kitchen ideas".into(), ..Board::default() };
        assert_eq!(file_name_for(&board), PathBuf::from("Kitchen_ideas.mbrd"));
        // The board is untouched — a Save As must not rename what it saves.
        assert_eq!(board.title, "Kitchen ideas");
    }

    #[test]
    fn a_board_with_no_title_still_gets_a_filename() {
        assert_eq!(file_name_for(&Board::default()), PathBuf::from("untitled.mbrd"));
    }
}
