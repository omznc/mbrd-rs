//! Matching what somebody half-remembered against a list of what there is.
//!
//! One matcher, three lists. The board switcher had this to itself until the
//! palettes wanted the same behaviour, and three copies of a scoring function
//! is the drift `command.rs` opens by warning about: they would agree on the
//! day they were written and disagree by the time anybody noticed, and the
//! symptom — "typing the same letters finds different things in two places" —
//! is one nobody would think to report as a bug.
//!
//! Subsequence rather than substring, because the whole point of typing into a
//! list is that you can be approximate: `algn` should find "Align left" and
//! `ktch` should find `kitchen.mbrd`. What stops that being useless is the
//! scoring — letters in a row and letters that start a word are worth more, so
//! the thing you were actually aiming at sorts above the thing that merely
//! contains the same letters in the same order.

/// Points for a subsequence match, or `None` if the letters are not all there.
///
/// Both arguments are expected lowercase. Case folding is the caller's, because
/// a caller that matches several fields against one query would otherwise fold
/// the query once per field.
pub fn subsequence(needle: &str, haystack: &str) -> Option<i32> {
    let mut points = 0;
    let mut run = 0;
    let mut chars = haystack.char_indices().peekable();

    for want in needle.chars() {
        loop {
            let (i, got) = chars.next()?;
            if got == want {
                points += 1;
                // A letter following the one before it, or starting a word, is
                // what somebody was actually aiming at.
                let starts_word = i == 0
                    || haystack[..i].chars().next_back().is_some_and(|c| !c.is_alphanumeric());
                points += run * 2 + i32::from(starts_word) * 3;
                run += 1;
                break;
            }
            run = 0;
        }
    }
    // Shorter answers, all else equal: an exact name should beat one that
    // merely contains it.
    Some(points - (haystack.chars().count() as i32) / 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters_that_are_not_all_there_are_not_a_match() {
        assert!(subsequence("xyz", "kitchen").is_none());
        // Order matters: the letters are all present but not in that sequence.
        assert!(subsequence("nek", "kitchen").is_none());
    }

    #[test]
    fn letters_in_a_row_beat_letters_scattered_about() {
        let together = subsequence("kitch", "kitchen").unwrap();
        let apart = subsequence("kitch", "k-i-t-c-h").unwrap();
        assert!(together > apart, "{together} should beat {apart}");
    }

    #[test]
    fn a_word_start_is_worth_more_than_a_letter_in_the_middle() {
        let starting = subsequence("l", "left").unwrap();
        let buried = subsequence("l", "align").unwrap();
        assert!(starting > buried, "{starting} should beat {buried}");
    }

    /// The matching is greedy: it takes the first letter that will do rather
    /// than looking for the one that would score best.
    ///
    /// Stated as a test because it is a real limitation and not an obvious
    /// one — `l` against "align left" matches the `l` in "align", so the
    /// word-start bonus is not earned even though a better match was two words
    /// along. Behaviour inherited from the board switcher and kept deliberately
    /// rather than fixed: backtracking to the best assignment is a different
    /// and much slower algorithm, and on the lists this matches against — a few
    /// dozen command labels, a board's worth of short names — the greedy
    /// answer and the best answer are almost always the same one.
    #[test]
    fn matching_takes_the_first_letter_that_will_do() {
        let greedy = subsequence("l", "align left").unwrap();
        let if_it_looked_ahead = subsequence("l", "left").unwrap();
        assert!(greedy < if_it_looked_ahead);
    }

    #[test]
    fn an_empty_query_matches_anything_at_all() {
        // The palettes never ask — an empty query lists everything rather than
        // scoring it — but a matcher that refused would make that a special
        // case at every call site instead of here.
        assert!(subsequence("", "anything").is_some());
    }
}
