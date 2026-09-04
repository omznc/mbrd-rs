//! What is playing, and where each playhead has got to.
//!
//! One entry per card that has been pressed at least once. Deliberately *not*
//! one per media card on the board — a board with four hundred videos on it
//! should cost four hundred nothings until somebody presses one.
//!
//! ## This holds no decisions
//!
//! Whether a card loops, whether it is muted and how loud it is all live in the
//! board, go through the mutation door, and are undoable. What is here is the
//! other kind of state entirely: where the playhead is, and whether it is
//! moving. Neither survives closing the app, neither belongs in a file somebody
//! is sent, and neither is worth an undo step — nobody has ever wanted to undo
//! a playhead.
//!
//! The board's flags are therefore *copied in* by [`Media::observe`] once a
//! frame rather than read from here, which keeps this module ignorant of the
//! board and keeps the board ignorant of the clock.
//!
//! ## The frame clock
//!
//! [`Media::tick`] is called from `BoardView::advance` and answers the only
//! question that matters to it: is anything still moving? While something is,
//! the window keeps asking for frames; the moment nothing is, a board nobody is
//! touching goes back to costing nothing at all.
//!
//! "Moving" means moving **where somebody can see it**. [`Media::observe`] is
//! called once a frame for every media card the cull kept, and `tick` counts a
//! player only if it was observed this round: a looping animation panned off
//! the edge of the screen must not hold the whole board at the display's
//! refresh rate forever, which is exactly what it did when `tick` answered for
//! every player wherever its card was. The playhead is not paused by being off
//! screen — the next tick after it comes back advances it by however long it
//! was away, the same arithmetic that already absorbs a machine coming out of
//! sleep — so what is given up is only the repaints nobody could see.

use std::collections::HashMap;
use std::time::Duration;
// WASM EXPERIMENT: std's clock panics on wasm32-unknown-unknown; `web-time`
// is the same API over `performance.now()`.
#[cfg(not(target_family = "wasm"))]
use std::time::Instant;
#[cfg(target_family = "wasm")]
use web_time::Instant;

use gpui::RenderImage;

/// How many cards may be playing at once.
///
/// Phase A has nothing behind a player but a playhead, so this costs nothing to
/// enforce and is here from the start because the thing it will bound —
/// a decoder process per video — is not something to discover a limit for after
/// somebody has dropped thirty clips on a board. The least-recently-started
/// player is stopped to make room, and stops *where it is* rather than at the
/// beginning: coming back to a card should carry on rather than start over.
pub const AT_ONCE: usize = 4;

/// One card's playhead.
#[derive(Debug, Clone)]
pub struct Player {
    pub playing: bool,
    /// Where the playhead is.
    pub at: Duration,
    /// How long the whole thing is, once anybody knows. `None` before the
    /// decode has landed, which is normal for the first frame or two.
    pub length: Option<Duration>,
    /// Copied from the board by [`Media::observe`].
    pub looping: bool,
    /// When `at` was last brought level with the clock. Only meaningful while
    /// `playing`, and reset on every resume so that a card paused for an hour
    /// does not leap an hour forward when it starts again.
    since: Instant,
    /// When this player was last started, for choosing which to stop.
    started: Instant,
    /// The last [`Media::round`] this card was seen on screen — stamped by
    /// `observe`, and by `play`/`seek` since a card being pressed is a card
    /// being looked at. `tick` reads it to keep off-screen playback from
    /// asking for frames.
    seen: u64,
    /// Whether a real decoder is telling this playhead where it is.
    ///
    /// Set by [`Media::sync`] and **cleared by every `tick`**, so it is a claim
    /// about the frame that just happened rather than a mode. That is what
    /// makes it self-healing: a card whose pipeline is torn down simply stops
    /// being synced, and the wall clock picks the playhead up again on the very
    /// next frame with no bookkeeping anywhere.
    ///
    /// A GIF is never driven — there is no decoder behind one, only frames and
    /// a measured clock — which is why this defaults to false and why every
    /// test in this module is about the undriven case.
    driven: bool,
}

impl Player {
    /// How far through, `0.0..=1.0`. Zero where the length is not known yet —
    /// a scrubber with nothing to scrub sits at the start.
    pub fn progress(&self) -> f32 {
        match self.length {
            Some(length) if length > Duration::ZERO => {
                (self.at.as_secs_f32() / length.as_secs_f32()).clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    }
}

/// Every playhead on the board.
#[derive(Default)]
pub struct Media {
    players: HashMap<String, Player>,
    /// Which frame this is, as far as visibility is concerned. Bumped once
    /// per [`tick`](Self::tick); a player whose `seen` is behind it was not
    /// observed since the last tick, which means the cull dropped its card.
    round: u64,
}

impl Media {
    /// Move every playhead on to `now`, and say whether anything is still
    /// moving.
    ///
    /// Ending is not the same as looping and both are handled here rather than
    /// at the call sites that start playback: a clip that runs out stops **at
    /// its end**, not back at its beginning, because a card showing its last
    /// frame is telling you it finished and a card showing its first is telling
    /// you nothing happened.
    pub fn tick(&mut self, now: Instant) -> bool {
        let mut moving = false;
        for player in self.players.values_mut() {
            if !player.playing {
                continue;
            }
            // Only a card the cull kept since the last tick counts as moving —
            // see the module doc. The playhead still advances, so a clip that
            // runs out off screen is found finished rather than found frozen.
            let visible = player.seen >= self.round;
            // A decoder owns this playhead, so the clock must not also move it
            // — see `Player::driven`. The stamp is still brought level, so that
            // the frame after a pipeline goes away measures from now rather
            // than from whenever it was last on the clock.
            if std::mem::take(&mut player.driven) {
                player.since = now;
                moving |= visible;
                continue;
            }
            // Saturating, because a clock that went backwards — which happens
            // on a machine coming out of sleep — must not panic here.
            let dt = now.saturating_duration_since(player.since);
            player.since = now;
            player.at += dt;

            match player.length {
                Some(length) if length > Duration::ZERO && player.at >= length => {
                    match player.looping {
                        true => {
                            // Modulo rather than zero, so a long frame does not
                            // lose the overshoot and drift the loop slower.
                            let over = player.at.as_nanos() % length.as_nanos();
                            player.at = Duration::from_nanos(over as u64);
                            moving |= visible;
                        }
                        false => {
                            player.at = length;
                            player.playing = false;
                        }
                    }
                }
                _ => moving |= visible,
            }
        }
        self.round = self.round.wrapping_add(1);
        moving
    }

    /// Put a real decoder's answer in, for a card that has one.
    ///
    /// The pipeline is the truth for anything it is playing: a decoder that
    /// stalls for two hundred milliseconds has a playhead that stalls with it,
    /// and a scrubber running on a wall clock would slide away from the sound
    /// and then snap back. See `pipeline::Beat`, which is what this takes.
    ///
    /// Nothing is created here. A card with no player is a card nobody has
    /// pressed, and a decoder reporting a position for one would be a
    /// scrubber appearing on a card that is not playing.
    pub fn sync(&mut self, id: &str, at: Duration, length: Option<Duration>) {
        let Some(player) = self.players.get_mut(id) else { return };
        player.at = at;
        // `or`, not a replacement: a container that stops reporting its length
        // half way through — which happens on a live stream and on a truncated
        // file — must not take the length off a scrubber that already had one.
        player.length = length.or(player.length);
        player.driven = true;
    }

    /// Bring one player's copy of the board's flags level, and tell it how long
    /// the recording turned out to be.
    ///
    /// Called once a frame for every visible media card. Cheap, and does
    /// nothing at all for a card nobody has pressed.
    pub fn observe(&mut self, id: &str, length: Option<Duration>, looping: bool) {
        if let Some(player) = self.players.get_mut(id) {
            player.looping = looping;
            // Being observed is being on screen: `observe` is called from
            // inside the cull, and this stamp is what `tick` counts as moving.
            player.seen = self.round;
            if let Some(length) = length {
                player.length = Some(length);
                // A length that arrives after the playhead has already run past
                // it — a decode landing late — must not leave a scrubber
                // reading more than full.
                player.at = player.at.min(length);
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<&Player> {
        self.players.get(id)
    }

    pub fn is_playing(&self, id: &str) -> bool {
        self.players.get(id).is_some_and(|p| p.playing)
    }

    /// Every card whose playhead is running, for the frame loop.
    ///
    /// Bounded by [`AT_ONCE`] and so never more than a handful, which is what
    /// makes collecting it once a frame cheaper than handing the whole map out
    /// and letting the caller hold a borrow across the work it does.
    pub fn playing(&self) -> Vec<String> {
        self.players.iter().filter(|(_, p)| p.playing).map(|(id, _)| id.clone()).collect()
    }

    /// Where a card's playhead is. Zero for one that has never played, which is
    /// the same place it would be.
    pub fn at(&self, id: &str) -> Duration {
        self.players.get(id).map_or(Duration::ZERO, |p| p.at)
    }

    pub fn progress(&self, id: &str) -> f32 {
        self.players.get(id).map_or(0.0, Player::progress)
    }

    /// Start a card playing, making room if too many already are.
    ///
    /// A card whose playhead is sitting at the end starts again from the
    /// beginning — the second press of a play button on a finished clip means
    /// "again", not "stay finished".
    pub fn play(&mut self, id: &str, length: Option<Duration>, looping: bool) {
        let now = Instant::now();
        self.make_room(id, now);

        let round = self.round;
        let player = self.players.entry(id.to_string()).or_insert_with(|| Player {
            playing: false,
            at: Duration::ZERO,
            length,
            looping,
            since: now,
            started: now,
            seen: round,
            driven: false,
        });

        player.length = length.or(player.length);
        player.looping = looping;
        if player.length.is_some_and(|l| player.at >= l) {
            player.at = Duration::ZERO;
        }
        player.playing = true;
        player.since = now;
        player.started = now;
        // A card being pressed is a card being looked at.
        player.seen = round;
    }

    pub fn pause(&mut self, id: &str) {
        if let Some(player) = self.players.get_mut(id) {
            player.playing = false;
        }
    }

    /// Put the playhead a fraction of the way through, without starting or
    /// stopping it. Scrubbing a paused card leaves it paused, and scrubbing a
    /// playing one does not interrupt it — which is what every player does.
    pub fn seek(&mut self, id: &str, fraction: f32, length: Option<Duration>) {
        let now = Instant::now();
        let round = self.round;
        let player = self.players.entry(id.to_string()).or_insert_with(|| Player {
            playing: false,
            at: Duration::ZERO,
            length,
            looping: false,
            since: now,
            started: now,
            seen: round,
            driven: false,
        });
        player.length = length.or(player.length);
        player.since = now;
        // Scrubbing, like pressing play, only happens to a card on screen.
        player.seen = round;
        let fraction = if fraction.is_finite() { fraction.clamp(0.0, 1.0) } else { 0.0 };
        player.at = match player.length {
            Some(length) => length.mul_f32(fraction),
            None => Duration::ZERO,
        };
    }

    /// Stop everything except one card.
    ///
    /// **Overlapping audio on a moodboard is noise**, so starting one recording
    /// stops the others. The format's `audioOrder` says the original thought of
    /// a board's audio as a playlist rather than as a wall of sound, and this is
    /// the smallest way to agree with it.
    pub fn pause_others(&mut self, id: &str) {
        for (other, player) in self.players.iter_mut() {
            if other != id {
                player.playing = false;
            }
        }
    }

    /// Forget a card entirely — it was deleted, or the board was closed.
    pub fn forget(&mut self, id: &str) {
        self.players.remove(id);
    }

    /// Stop the oldest player if starting `id` would put us over [`AT_ONCE`].
    fn make_room(&mut self, id: &str, now: Instant) {
        loop {
            let playing: Vec<(&String, Instant)> = self
                .players
                .iter()
                .filter(|(other, p)| p.playing && other.as_str() != id)
                .map(|(other, p)| (other, p.started))
                .collect();
            if playing.len() < AT_ONCE {
                return;
            }
            let Some(oldest) =
                playing.iter().min_by_key(|(_, at)| *at).map(|(id, _)| (*id).clone())
            else {
                return;
            };
            if let Some(player) = self.players.get_mut(&oldest) {
                player.playing = false;
                player.since = now;
            }
        }
    }
}

/// One decoded animation's clock, measured once.
///
/// The delays of a GIF are per frame and not evenly spaced — an animation
/// that pauses on one drawing for a second and flicks through six more in a
/// tenth is extremely normal — so which frame is showing has to be read off
/// the accumulated delays. Doing that accumulation per card per painted frame
/// was three walks of up to four thousand delays, sixty times a second, for
/// numbers that cannot change while the decode lives. Measured once instead,
/// and read back by sum lookup and binary search.
#[derive(Debug, Clone)]
pub struct Timing {
    /// Where each frame's turn ends on the clock, cumulatively. Empty for a
    /// still picture.
    ends: Vec<Duration>,
}

impl Timing {
    pub fn of(image: &RenderImage) -> Self {
        Self::measure(image.frame_count(), |i| Duration::from(image.delay(i)))
    }

    /// The arithmetic behind [`of`](Self::of), over anything that can name
    /// its delays. Separated so it can be tested against a list rather than
    /// against a decoded picture.
    fn measure(count: usize, delay: impl Fn(usize) -> Duration) -> Self {
        let mut acc = Duration::ZERO;
        let ends = (0..count)
            .map(|i| {
                acc += delay(i);
                acc
            })
            .collect();
        Self { ends }
    }

    /// How long one loop takes.
    pub fn length(&self) -> Duration {
        self.ends.last().copied().unwrap_or(Duration::ZERO)
    }

    /// Which frame is showing at `at`: the first whose turn has not ended.
    pub fn frame_at(&self, at: Duration, looping: bool) -> usize {
        let count = self.ends.len();
        if count == 0 {
            return 0;
        }
        let total = self.length();
        if total.is_zero() {
            return 0;
        }

        let t = match looping {
            // `as u64` is safe against the modulo: the remainder is under
            // `total`, and a total past what a `u64` of nanoseconds holds is
            // five hundred years of animation.
            true => Duration::from_nanos((at.as_nanos() % total.as_nanos()) as u64),
            false => at.min(total),
        };

        // The first frame whose end is past `t`. Nothing past it only for a
        // `t` that is exactly `total`, which is where a finished animation
        // sits: its last frame.
        self.ends.partition_point(|end| *end <= t).min(count - 1)
    }
}

/// The [`Timing`] of every animation recently on screen, keyed by card.
///
/// Keyed by the card's id with the decoded picture's identity — its pointer
/// and frame count — held as the check, so a re-decode of the same card
/// measures again rather than reading the old clock. Bounded the blunt way:
/// past [`Self::MOST`] entries the table is dropped whole and remeasured on
/// demand, which costs one walk per animation on screen on the frame after a
/// board that large turns over — nothing next to keeping it forever.
#[derive(Default)]
pub struct Timings {
    known: HashMap<String, (usize, usize, Timing)>,
}

impl Timings {
    const MOST: usize = 512;

    pub fn of(&mut self, id: &str, image: &std::sync::Arc<RenderImage>) -> &Timing {
        let key = std::sync::Arc::as_ptr(image) as *const () as usize;
        let count = image.frame_count();
        if self.known.len() > Self::MOST && !self.known.contains_key(id) {
            self.known.clear();
        }
        let held = self
            .known
            .entry(id.to_string())
            .and_modify(|entry| {
                if entry.0 != key || entry.1 != count {
                    *entry = (key, count, Timing::of(image));
                }
            })
            .or_insert_with(|| (key, count, Timing::of(image)));
        &held.2
    }

    /// Forget a card entirely — it was deleted, or the board was closed.
    pub fn forget(&mut self, id: &str) {
        self.known.remove(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Two durations that are the same to anybody watching.
    ///
    /// `play` reads the wall clock itself, so a test that starts a card and
    /// then ticks it to `now + 2500ms` has spent a few hundred nanoseconds
    /// getting between the two. Asserting exact equality would be asserting
    /// that those nanoseconds do not exist, which is a test that fails on a
    /// slow machine and passes on a fast one. A millisecond is far below what
    /// a frame can show and far above what the scheduler costs.
    fn near(a: Duration, b: Duration) -> bool {
        a.abs_diff(b) < ms(1)
    }

    /// Five frames, a tenth of a second each.
    fn even() -> Vec<Duration> {
        vec![ms(100); 5]
    }

    fn frame_at(delays: &[Duration], at: Duration, looping: bool) -> usize {
        Timing::measure(delays.len(), |i| delays[i]).frame_at(at, looping)
    }

    #[test]
    fn the_frame_showing_is_the_one_whose_turn_it_is() {
        let delays = even();
        assert_eq!(frame_at(&delays, ms(0), true), 0);
        assert_eq!(frame_at(&delays, ms(50), true), 0);
        assert_eq!(frame_at(&delays, ms(100), true), 1);
        assert_eq!(frame_at(&delays, ms(250), true), 2);
        assert_eq!(frame_at(&delays, ms(499), true), 4);
    }

    #[test]
    fn a_loop_comes_back_round() {
        let delays = even();
        assert_eq!(frame_at(&delays, ms(500), true), 0);
        assert_eq!(frame_at(&delays, ms(650), true), 1);
        // And an absurd playhead is still a frame rather than a panic.
        assert_eq!(frame_at(&delays, Duration::from_secs(86_400), true), 0);
    }

    #[test]
    fn an_animation_that_does_not_loop_stops_on_its_last_frame() {
        let delays = even();
        assert_eq!(frame_at(&delays, ms(500), false), 4);
        assert_eq!(frame_at(&delays, ms(9_999), false), 4);
    }

    #[test]
    fn frames_of_different_lengths_each_get_their_own_turn() {
        // The case even spacing would get wrong: one drawing held for a second,
        // then three flicked through.
        let delays = vec![ms(1000), ms(30), ms(30), ms(30)];
        assert_eq!(frame_at(&delays, ms(500), true), 0);
        assert_eq!(frame_at(&delays, ms(1010), true), 1);
        assert_eq!(frame_at(&delays, ms(1040), true), 2);
        assert_eq!(frame_at(&delays, ms(1075), true), 3);
    }

    #[test]
    fn an_animation_with_no_frames_or_no_time_does_not_divide_by_it() {
        assert_eq!(frame_at(&[], ms(10), true), 0);
        assert_eq!(frame_at(&[Duration::ZERO; 4], ms(10), true), 0);
    }

    #[test]
    fn a_playhead_advances_with_the_clock() {
        let mut media = Media::default();
        media.play("a", Some(Duration::from_secs(10)), false);
        let started = Instant::now();

        assert!(media.tick(started + ms(2500)));
        assert!(near(media.at("a"), ms(2500)), "{:?}", media.at("a"));
        assert!((media.progress("a") - 0.25).abs() < 0.01);
    }

    #[test]
    fn a_paused_card_does_not_leap_forward_when_it_starts_again() {
        // The bug this shape exists to prevent: `since` left at the moment the
        // card was paused, so a resume an hour later advances an hour.
        let mut media = Media::default();
        media.play("a", Some(Duration::from_secs(60)), false);
        media.tick(Instant::now() + ms(1000));
        media.pause("a");
        let paused_at = media.at("a");

        media.play("a", Some(Duration::from_secs(60)), false);
        media.tick(Instant::now() + ms(10));
        assert!(
            media.at("a") - paused_at < ms(100),
            "it jumped to {:?} from {paused_at:?}",
            media.at("a")
        );
    }

    #[test]
    fn a_clip_that_runs_out_stops_at_its_end_rather_than_at_its_start() {
        let mut media = Media::default();
        media.play("a", Some(ms(500)), false);
        assert!(!media.tick(Instant::now() + ms(900)), "it should have stopped asking for frames");
        assert!(!media.is_playing("a"));
        assert_eq!(media.at("a"), ms(500), "it went back to the beginning");
        assert_eq!(media.progress("a"), 1.0);
    }

    #[test]
    fn pressing_play_on_a_finished_clip_starts_it_again() {
        let mut media = Media::default();
        media.play("a", Some(ms(500)), false);
        media.tick(Instant::now() + ms(900));
        media.play("a", Some(ms(500)), false);
        assert_eq!(media.at("a"), Duration::ZERO);
        assert!(media.is_playing("a"));
    }

    #[test]
    fn a_looping_clip_keeps_the_overshoot_rather_than_drifting() {
        // Dropping the remainder here makes every loop a frame longer than it
        // should be, which over a minute is visibly slow.
        let mut media = Media::default();
        media.play("a", Some(ms(1000)), true);
        assert!(media.tick(Instant::now() + ms(1750)));
        assert!(near(media.at("a"), ms(750)), "{:?}", media.at("a"));
        assert!(media.is_playing("a"));
    }

    #[test]
    fn only_so_many_things_play_at_once_and_the_oldest_gives_way() {
        let mut media = Media::default();
        for i in 0..AT_ONCE {
            media.play(&format!("card{i}"), Some(Duration::from_secs(60)), true);
            // Distinguishable start times, without sleeping.
            media.players.get_mut(&format!("card{i}")).unwrap().started =
                Instant::now() - Duration::from_secs((AT_ONCE - i) as u64);
        }
        let playing =
            |m: &Media| (0..AT_ONCE).filter(|i| m.is_playing(&format!("card{i}"))).count();
        assert_eq!(playing(&media), AT_ONCE);

        media.play("late", Some(Duration::from_secs(60)), true);
        assert_eq!(playing(&media) + 1, AT_ONCE, "the cap was not held");
        assert!(media.is_playing("late"));
        assert!(!media.is_playing("card0"), "the oldest should have given way");
        assert!(media.is_playing("card3"), "a newer one was stopped instead");
    }

    #[test]
    fn a_card_that_gave_way_carries_on_from_where_it_was() {
        let mut media = Media::default();
        media.play("a", Some(Duration::from_secs(60)), true);
        media.tick(Instant::now() + ms(4000));
        for i in 0..AT_ONCE {
            media.play(&format!("other{i}"), Some(Duration::from_secs(60)), true);
        }
        assert!(!media.is_playing("a"));
        assert!(near(media.at("a"), ms(4000)), "it was rewound rather than paused");
    }

    #[test]
    fn starting_one_recording_stops_the_others() {
        let mut media = Media::default();
        media.play("a", Some(Duration::from_secs(60)), false);
        media.play("b", Some(Duration::from_secs(60)), false);
        media.pause_others("b");
        assert!(media.is_playing("b"));
        assert!(!media.is_playing("a"));
    }

    #[test]
    fn scrubbing_leaves_a_card_however_it_was() {
        let mut media = Media::default();
        let length = Some(Duration::from_secs(10));

        media.seek("paused", 0.5, length);
        assert_eq!(media.at("paused"), Duration::from_secs(5));
        assert!(!media.is_playing("paused"), "scrubbing started it");

        media.play("playing", length, false);
        media.seek("playing", 0.25, length);
        assert!(media.is_playing("playing"), "scrubbing stopped it");
        assert_eq!(media.at("playing"), Duration::from_secs(2) + ms(500));
    }

    #[test]
    fn scrubbing_a_card_of_unknown_length_does_not_invent_a_position() {
        let mut media = Media::default();
        media.seek("a", 0.75, None);
        assert_eq!(media.at("a"), Duration::ZERO);
        // And a nonsense fraction does not reach a `mul_f32`.
        media.seek("a", f32::NAN, Some(Duration::from_secs(10)));
        assert_eq!(media.at("a"), Duration::ZERO);
    }

    #[test]
    fn a_length_that_arrives_late_does_not_leave_the_scrubber_past_the_end() {
        // A decode landing after the card was already pressed: the playhead has
        // been running against no length at all.
        let mut media = Media::default();
        media.play("a", None, true);
        media.tick(Instant::now() + Duration::from_secs(30));
        media.observe("a", Some(Duration::from_secs(2)), true);
        assert!(media.at("a") <= Duration::from_secs(2));
        assert!(media.progress("a") <= 1.0);
    }

    #[test]
    fn nothing_playing_costs_no_frames() {
        let mut media = Media::default();
        assert!(!media.tick(Instant::now()), "an empty board asked for a frame");
        media.play("a", Some(ms(10)), false);
        media.tick(Instant::now() + ms(50));
        assert!(!media.tick(Instant::now() + ms(60)), "a finished board asked for a frame");
    }

    #[test]
    fn a_clock_that_goes_backwards_does_not_panic() {
        // A machine coming out of sleep really does do this.
        let mut media = Media::default();
        media.play("a", Some(Duration::from_secs(10)), true);
        let now = Instant::now();
        media.tick(now + Duration::from_secs(1));
        media.tick(now);
        assert!(media.at("a") <= Duration::from_secs(10));
    }
}
