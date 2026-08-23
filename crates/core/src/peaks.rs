//! Measuring a recording down to something a card can draw.
//!
//! A three-minute song is eight million samples and a card is three hundred
//! pixels wide, so what a waveform card actually needs is a few hundred
//! numbers. The format has always known this — `waveforms/<hash>.json` is a
//! sidecar with a `res` and that many `peaks` in it, written by
//! [`mbrd::write`](crate::mbrd) and read back by [`mbrd::read`](crate::mbrd) —
//! and until now nothing in this build ever *produced* one. This is the missing
//! half.
//!
//! ## Peak, not average
//!
//! The bar a person recognises is the loudest thing in that slice of time, not
//! the mean of it. Averaging a bucket of samples that swings between -1 and 1
//! gives approximately zero, which would draw a flat line through the middle of
//! a drum track. So each bucket keeps the largest magnitude it saw.
//!
//! ## Why this takes an iterator
//!
//! The caller has just decoded several megabytes of audio and does not want a
//! second copy of it. Taking `impl Iterator<Item = f32>` means the samples can
//! be measured as they come off the decoder and dropped immediately — but an
//! iterator only pays for that if what receives it drops them too. Buffering
//! it all into one `Vec` before resampling would take the promise back: a
//! forty-minute recording is on the order of a hundred million samples, which
//! is most of a gigabyte held just so a few hundred peaks can be read out of
//! it. `measure` instead folds the run down as it goes — see [`CARRIED`] —
//! so the true cost of a recording of any length is a few thousand floats,
//! not one float per sample.

/// How many bars a waveform is measured into.
///
/// Chosen against the card rather than against the audio: an audio card is a
/// few hundred world units wide and a bar wants at least a pixel, so past this
/// the extra numbers are ones no screen can show. It is also what the sidecar
/// costs on disk — five hundred and twelve short decimals is about two
/// kilobytes before deflate, beside megabytes of audio.
pub const RESOLUTION: usize = 512;

/// How many intermediate levels [`measure`] carries at once while it still
/// does not know how long the recording is.
///
/// A few times [`RESOLUTION`] rather than one entry per sample: past this
/// many, [`measure`] folds each neighbouring pair down to the louder of the
/// two — see `fold` — which halves the count and doubles what each slot
/// stands for, and keeps going. That fold is what bounds this module's memory
/// at a few thousand floats no matter whether the recording behind it is
/// four seconds or four hours; a recording shorter than this many samples
/// never folds at all; and is why "measure" as a whole is a real streaming
/// reduction of a run of samples the reader has not been told the length of.
const CARRIED: usize = RESOLUTION * 8;

/// Measure samples into `buckets` peaks, each in `0.0..=1.0`.
///
/// Interleaved channels are fine and want no special handling: the loudest
/// sample in a slice of a stereo file is the loudest thing you would have heard
/// in it, which is the question a waveform answers.
///
/// The count is exact — `buckets` in, `buckets` out — because the sidecar's
/// reader checks `peaks.len() == res` and throws away a file where they
/// disagree. A recording with fewer samples than buckets pads with silence
/// rather than returning a short list.
pub fn measure(samples: impl Iterator<Item = f32>, buckets: usize) -> Vec<f32> {
    if buckets == 0 {
        return Vec::new();
    }

    // Carried at bounded size rather than grown to hold the whole recording:
    // the whole point of taking an iterator is not knowing how long the
    // recording is until it ends, and a run long enough to need folding at
    // all is exactly the run too long to keep whole. `cap` is kept generous
    // against `buckets` too, so a caller asking for an unusually wide
    // waveform still gets a real answer rather than one folded down before
    // there was any need.
    let cap = CARRIED.max(buckets.saturating_mul(2));
    let mut work: Vec<f32> = Vec::with_capacity(cap);
    // How many raw samples the *next* slot pushed onto `work` will stand
    // for, and how far into the current one this pass has got. Doubled
    // every time `work` fills — see `fold` — which is what keeps its length
    // bounded no matter how long the recording runs.
    let mut stride = 1usize;
    let mut into = 0.0f32;
    let mut in_slot = 0usize;

    for sample in samples {
        // A decoder that hands back a NaN — and they do, from a corrupt frame
        // — must not poison the bucket it lands in. `max` propagates the
        // number, but only because the comparison is written this way round.
        let level = if sample.is_finite() { sample.abs().min(1.0) } else { 0.0 };
        into = into.max(level);
        in_slot += 1;
        if in_slot == stride {
            work.push(into);
            (into, in_slot) = (0.0, 0);
            if work.len() == cap {
                fold(&mut work);
                stride *= 2;
            }
        }
    }
    // A slot the recording ended in the middle of still holds real samples —
    // dropping it would throw away up to a stride's worth of the tail on
    // every recording whose length is not an exact multiple of one.
    if in_slot > 0 {
        work.push(into);
    }

    resample(&work, buckets)
}

/// Fold a full carry down to half its length, keeping the louder of each
/// neighbouring pair.
///
/// The other half of what bounds [`measure`]'s memory: called only once
/// `work` has reached [`CARRIED`] (or the wider cap a large `buckets` earns
/// it), so this runs at most a handful of times no matter how long the
/// recording is — each call doubles what a slot represents, so the number of
/// samples it takes to fill `work` again doubles too.
fn fold(work: &mut Vec<f32>) {
    let half = work.len() / 2;
    for i in 0..half {
        work[i] = work[2 * i].max(work[2 * i + 1]);
    }
    work.truncate(half);
}

/// The same, for a caller that already has the samples in hand.
pub fn measure_slice(samples: &[f32], buckets: usize) -> Vec<f32> {
    measure(samples.iter().copied(), buckets)
}

/// Reduce a run of levels to exactly `buckets` peaks.
///
/// Split by proportion rather than by a fixed stride, so that the last bucket
/// is the same width as the first — a stride of `len / buckets` leaves a
/// remainder, and dropping it lops the end off a recording while folding it
/// into the last bucket makes one bar wider than the rest.
fn resample(levels: &[f32], buckets: usize) -> Vec<f32> {
    let mut out = vec![0.0; buckets];
    if levels.is_empty() {
        return out;
    }

    for (i, bucket) in out.iter_mut().enumerate() {
        let from = i * levels.len() / buckets;
        // At least one sample each, for a recording shorter than the bucket
        // count — otherwise `from == to` and every bar is silent.
        let to = (((i + 1) * levels.len()) / buckets).max(from + 1).min(levels.len());
        *bucket = levels[from..to].iter().fold(0.0f32, |peak, &s| peak.max(s));
    }

    out
}

/// Bring a set of peaks up so the loudest bar reaches the top.
///
/// A quiet recording drawn honestly is a flat grey smudge, and the card's job
/// is to show the *shape* of the sound rather than to be a meter. Silence is
/// left alone — scaling nothing up gives nothing, and dividing by it gives
/// worse than nothing.
pub fn normalise(peaks: &mut [f32]) {
    let loudest = peaks.iter().copied().fold(0.0f32, f32::max);
    if loudest <= f32::EPSILON {
        return;
    }
    let gain = 1.0 / loudest;
    for peak in peaks.iter_mut() {
        *peak = (*peak * gain).min(1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_count_is_exactly_what_was_asked_for() {
        // The sidecar's reader throws away a file where `res` and the list
        // disagree, so this is the property that keeps a measurement saveable.
        for len in [0, 1, 7, 511, 512, 513, 100_000] {
            let samples: Vec<f32> = (0..len).map(|i| (i as f32 * 0.01).sin()).collect();
            assert_eq!(measure_slice(&samples, RESOLUTION).len(), RESOLUTION, "for {len}");
        }
    }

    #[test]
    fn a_bucket_keeps_the_loudest_thing_in_it_rather_than_the_average() {
        // A run that swings hard around zero: averaged it is silence, and drawn
        // that way a drum track is a flat line.
        let samples: Vec<f32> = (0..1000).map(|i| if i % 2 == 0 { 0.9 } else { -0.9 }).collect();
        let peaks = measure_slice(&samples, 8);
        for peak in peaks {
            assert!((peak - 0.9).abs() < 0.001, "{peak}");
        }
    }

    #[test]
    fn silence_measures_as_silence() {
        let peaks = measure_slice(&vec![0.0; 4096], 16);
        assert!(peaks.iter().all(|&p| p == 0.0));
        // And normalising it does not divide by nothing.
        let mut peaks = peaks;
        normalise(&mut peaks);
        assert!(peaks.iter().all(|p| p.is_finite()));
    }

    #[test]
    fn the_shape_lands_where_it_happened() {
        // Loud in the middle third, silent either side. The bars should say so.
        let mut samples = vec![0.0f32; 900];
        for s in samples.iter_mut().take(600).skip(300) {
            *s = 1.0;
        }
        let peaks = measure_slice(&samples, 3);
        assert_eq!(peaks[0], 0.0);
        assert_eq!(peaks[1], 1.0);
        assert_eq!(peaks[2], 0.0);
    }

    #[test]
    fn a_run_longer_than_the_carry_folds_rather_than_being_buffered() {
        // Comfortably past `CARRIED`, and built as a plain `map` over a
        // range rather than a `Vec` — this is the test that would be the one
        // to hang or blow its budget if `measure` ever went back to
        // collecting every sample before resampling one. The loud stretch is
        // kept well clear of both the middle third's own boundaries and
        // whatever stride `measure` has folded up to by the time it gets
        // there, so a passing result says the shape survived the folding,
        // not that it got lucky on a boundary.
        let n = CARRIED * 200 + 17;
        let (loud_from, loud_to) = (n * 4 / 10, n * 6 / 10);
        let samples = (0..n).map(|i| if i >= loud_from && i < loud_to { 1.0 } else { 0.0 });
        let peaks = measure(samples, 3);
        assert_eq!(peaks[0], 0.0, "{peaks:?}");
        assert_eq!(peaks[1], 1.0, "{peaks:?}");
        assert_eq!(peaks[2], 0.0, "{peaks:?}");
    }

    #[test]
    fn folding_keeps_the_louder_of_each_neighbouring_pair() {
        let mut work = vec![0.1, 0.9, 0.2, 0.3, 0.8, 0.05];
        fold(&mut work);
        assert_eq!(work, vec![0.9, 0.3, 0.8]);
    }

    #[test]
    fn a_recording_shorter_than_the_bar_count_still_draws() {
        // Four samples into sixteen bars: every bar has to come from somewhere,
        // and none of them may be a slice of nothing.
        let peaks = measure_slice(&[0.25, 0.5, 0.75, 1.0], 16);
        assert_eq!(peaks.len(), 16);
        assert!(peaks.iter().any(|&p| p > 0.0), "it measured as silent");
        assert!(peaks.iter().all(|p| p.is_finite()));
    }

    #[test]
    fn nothing_at_all_is_a_flat_line_rather_than_an_empty_list() {
        assert_eq!(measure_slice(&[], 8), vec![0.0; 8]);
        // And nobody asking for bars gets none, rather than a panic.
        assert!(measure_slice(&[0.5], 0).is_empty());
    }

    #[test]
    fn a_sample_that_is_not_a_number_does_not_poison_the_bar_it_lands_in() {
        // A corrupt frame really does hand back NaNs, and one of them reaching
        // a `max` chain would flatten the whole bar to nothing.
        let samples = vec![0.5, f32::NAN, 0.8, f32::INFINITY, 0.2];
        let peaks = measure_slice(&samples, 1);
        assert_eq!(peaks[0], 0.8);
    }

    #[test]
    fn a_sample_past_full_scale_is_held_to_it() {
        // The sidecar's reader clamps to `0.0..=1.0` on the way back in, so
        // writing something outside it would not survive a round trip.
        let peaks = measure_slice(&[4.0, -9.0], 2);
        assert!(peaks.iter().all(|&p| (0.0..=1.0).contains(&p)), "{peaks:?}");
    }

    #[test]
    fn a_quiet_recording_is_brought_up_to_the_top() {
        let mut peaks = vec![0.01, 0.02, 0.005];
        normalise(&mut peaks);
        assert!((peaks.iter().copied().fold(0.0f32, f32::max) - 1.0).abs() < 0.001);
        // And the shape survived: the loudest is still four times the quietest.
        assert!((peaks[1] / peaks[2] - 4.0).abs() < 0.01, "{peaks:?}");
    }
}
