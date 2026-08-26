//! What a media card is *doing*: whether it plays, how loud, and — for a mesh —
//! where the camera looking at it is standing.
//!
//! All of it lives in the item's [`meta`](crate::model::ItemMeta), which is a
//! map with no schema and unknown keys that round-trip untouched. That is what
//! makes this module possible at all: a board written here opens in a build
//! that has never heard of `volume` and comes back with the volume still on it.
//!
//! ## These are decisions, not measurements
//!
//! The README draws this line and it decides how the setters below behave.
//! `meta.fence` is a *measurement* — the geometry can re-derive it, so a stale
//! one is harmless and an absent one costs nothing. A playback flag is the
//! other kind: nothing on the board can tell you that somebody turned the sound
//! on, and there is no way to re-derive it.
//!
//! So a flag that has been set is written **explicitly, including when it
//! happens to equal this build's default**, and an absent key means "nobody has
//! said" rather than "off". Omitting defaults on the way out — which is what
//! [`ConnMeta`](crate::model::ConnMeta) does, and the right thing there — would
//! mean a future build that changed a default silently changed somebody's
//! board.
//!
//! ## Nothing here is trusted at rest
//!
//! Every read clamps, and every read has a fallback. These values arrive from a
//! file somebody else wrote: a volume of `1e9`, a `loop` that is the string
//! `"yes"`, a pitch of `NaN`. None of those may reach a mixer, a decoder or a
//! matrix, and the place to stop them is on the way in rather than at each of
//! the call sites that would otherwise have to remember.

use serde_json::Value;

use crate::model::{Item, ItemType};

/// The longest a tag read off a file may be.
///
/// An `artist` is a label on a card, not a document — and these strings come
/// out of ID3 frames, which have no length limit and occasionally contain an
/// entire discography's worth of credits.
pub const TAG_MAX: usize = 120;

/// How far a mesh's camera may be pushed in or pulled out, in bounding-sphere
/// radii. Scale-free on purpose: a mesh measured in millimetres and the same
/// mesh measured in metres want the same numbers here.
pub const DIST_MIN: f32 = 1.05;
pub const DIST_MAX: f32 = 12.0;

/// How far up or down the camera may be turned, in degrees.
///
/// Short of the pole rather than at it: at exactly ninety the view direction
/// and the world's up axis are parallel and the look-at basis is undefined, so
/// the picture flips over. One degree of clearance is invisible and is the
/// whole of the fix.
pub const PITCH_LIMIT: f32 = 89.0;

/// How far the look-at point may be shifted off the mesh's own centre, in
/// fractions of its silhouette span at the current turn. Scale-free the same
/// way `DIST_MIN`/`DIST_MAX` are: `1.0` is "off the edge of the mesh's own
/// extent," which is already generous room to look at a corner up close, and
/// past a couple of multiples of that a pan has nothing left to show.
pub const PAN_LIMIT: f32 = 1.5;

/// Whether a card of this type is something that can play.
///
/// `Image` is in the list, which surprises people: an animated GIF is an image
/// by every other measure this codebase applies — it is classified as one, it
/// decodes through the picture path, it carries its bytes as one — and it is
/// also a thing with a playhead and a loop. Whether a *particular* image
/// animates is a question about its bytes, and this is a question about its
/// type; the caller that has the decoded frames is the one that can tell.
pub fn is_playable(kind: &ItemType) -> bool {
    matches!(kind, ItemType::Video | ItemType::Audio | ItemType::Image)
}

/// The four playback flags, resolved.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Playback {
    pub autoplay: bool,
    /// `loop` in the file. Not here, because it is a keyword.
    pub looping: bool,
    pub muted: bool,
    /// `0.0` to `1.0`.
    pub volume: f32,
}

impl Playback {
    /// What a card of this type does before anybody has said otherwise.
    ///
    /// The two that matter are on the same line of reasoning. **A board that
    /// makes noise when you open it is a board you close**, so video arrives
    /// muted; and a clip on a moodboard is a thing you look at rather than a
    /// thing you watch to the end, so it arrives looping. Audio is the
    /// opposite of both: it is *only* sound, so muting it by default would
    /// leave a card that does nothing at all, and a voice memo that repeated
    /// itself forever would be a fault rather than a feature.
    ///
    /// **An animation autoplays and a video does not**, which is the one
    /// asymmetry here worth defending. A GIF is *authored* as a loop — it has
    /// no sound, no length worth reading and nothing to decide about; one that
    /// sits still until pressed is a broken GIF. A video is a thing somebody
    /// chooses to watch, it may be twenty minutes long, and playing it because
    /// it drifted on screen is a decision the app does not get to make. The
    /// app's own gate can still refuse an autoplay this says yes to.
    pub fn default_for(kind: &ItemType) -> Self {
        let sound = matches!(kind, ItemType::Audio);
        let drawn = matches!(kind, ItemType::Image);
        Self { autoplay: drawn, looping: !sound, muted: !sound, volume: 1.0 }
    }
}

/// Whether this card has a soundtrack at all.
///
/// `None` means nobody has looked yet, which is not the same as "no" — a
/// control that hides itself because the answer has not arrived would flicker
/// into existence the moment it did. Callers should treat `None` as "probably",
/// and the import that measures the file writes the answer down.
///
/// An animated picture is the one case that needs no measuring: no image format
/// carries a soundtrack, so a GIF is silent by construction rather than by
/// observation.
pub fn has_sound(item: &Item) -> Option<bool> {
    match item.kind {
        ItemType::Image => Some(false),
        ItemType::Audio => Some(true),
        _ => flag(item, "sound"),
    }
}

pub fn set_has_sound(item: &mut Item, sound: bool) {
    item.meta.insert("sound".into(), Value::Bool(sound));
}

/// Whether this card should be playing.
///
/// The same key as [`Playback::autoplay`], and deliberately: **the play button
/// writes it.** Pressing pause on a card is not a fact about this session, it
/// is somebody saying "not this one" — and a board where three clips were
/// stopped and one was left running should open that way tomorrow. The playhead
/// is not stored with it, because "where I had got to" and "whether this plays"
/// are different claims and only the second is about the board.
pub fn wants_to_play(item: &Item) -> bool {
    playback(item).autoplay
}

/// Record whether this card should be playing.
pub fn set_wants_to_play(item: &mut Item, playing: bool) {
    set_autoplay(item, playing);
}

/// The playback flags of one card.
pub fn playback(item: &Item) -> Playback {
    let fallback = Playback::default_for(&item.kind);
    Playback {
        autoplay: flag(item, "autoplay").unwrap_or(fallback.autoplay),
        looping: flag(item, "loop").unwrap_or(fallback.looping),
        muted: flag(item, "muted").unwrap_or(fallback.muted),
        volume: number(item, "volume")
            .filter(|v| v.is_finite())
            .map(|v| v.clamp(0.0, 1.0))
            .unwrap_or(fallback.volume),
    }
}

pub fn set_autoplay(item: &mut Item, on: bool) {
    item.meta.insert("autoplay".into(), Value::Bool(on));
}

pub fn set_looping(item: &mut Item, on: bool) {
    item.meta.insert("loop".into(), Value::Bool(on));
}

pub fn set_muted(item: &mut Item, on: bool) {
    item.meta.insert("muted".into(), Value::Bool(on));
}

/// Set the volume, held to the range a mixer will take.
pub fn set_volume(item: &mut Item, volume: f32) {
    let held = if volume.is_finite() { volume.clamp(0.0, 1.0) } else { 1.0 };
    item.meta.insert("volume".into(), json_number(held as f64));
}

/// How long the recording is, in seconds, where the file said.
///
/// Written at import so that a card can draw a timeline — and say how long the
/// thing on it is — without decoding several megabytes to find out. Absent is
/// normal: it means nobody has measured this one yet, not that it is empty.
pub fn duration(item: &Item) -> Option<f32> {
    number(item, "duration").filter(|d| d.is_finite() && *d > 0.0)
}

pub fn set_duration(item: &mut Item, seconds: f32) {
    if seconds.is_finite() && seconds > 0.0 {
        item.meta.insert("duration".into(), json_number(seconds as f64));
    }
}

/// A tag read off the file, where there was one.
pub fn tag<'a>(item: &'a Item, key: &str) -> Option<&'a str> {
    item.meta.get(key).and_then(Value::as_str).filter(|s| !s.is_empty())
}

pub fn artist(item: &Item) -> Option<&str> {
    tag(item, "artist")
}

pub fn album(item: &Item) -> Option<&str> {
    tag(item, "album")
}

/// Record a tag, tidied and held to [`TAG_MAX`]. An empty one removes the key.
pub fn set_tag(item: &mut Item, key: &str, value: &str) {
    let cleaned = crate::schema::collapse_space(value, TAG_MAX);
    match cleaned.is_empty() {
        true => {
            item.meta.remove(key);
        }
        false => {
            item.meta.insert(key.into(), Value::String(cleaned));
        }
    }
}

/// Whether this card carries enough about itself to be worth showing as more
/// than a bare player.
///
/// The whole of the difference between the two audio cards: a voice memo is a
/// waveform and a play button, and a record is a sleeve with words on it. What
/// decides is what the *file* carried — cover art, or a name for who made it —
/// because that is the difference a person hears in the question "what is this
/// recording", and it is not something to ask them to set by hand.
pub fn has_sleeve(item: &Item) -> bool {
    item.meta.get("cover").and_then(Value::as_str).is_some_and(|h| !h.is_empty())
        || artist(item).is_some()
        || album(item).is_some()
}

/// Where the camera looking at a mesh is standing.
///
/// Angles in degrees; `dist` in bounding-sphere radii, so the same numbers frame
/// a teapot and a terrain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Orbit {
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    /// The look-at point's shift off the mesh's own centre, in fractions of
    /// its silhouette span — see [`PAN_LIMIT`]. `(0.0, 0.0)` is dead centre,
    /// which is what every mesh card had before panning existed.
    pub pan_x: f32,
    pub pan_y: f32,
}

impl Default for Orbit {
    /// Slightly round the side and slightly above, which is the angle that
    /// tells you the most about an unfamiliar shape — straight on, a box and a
    /// cylinder are the same rectangle.
    fn default() -> Self {
        Self { yaw: 30.0, pitch: 20.0, dist: 2.6, pan_x: 0.0, pan_y: 0.0 }
    }
}

impl Orbit {
    /// Turned by some number of degrees, held inside the limits.
    pub fn turned(self, dyaw: f32, dpitch: f32) -> Self {
        Self {
            yaw: wrap_degrees(self.yaw + dyaw),
            pitch: (self.pitch + dpitch).clamp(-PITCH_LIMIT, PITCH_LIMIT),
            ..self
        }
    }

    /// Pushed in or pulled out. `factor` below one moves closer.
    pub fn dollied(self, factor: f32) -> Self {
        let dist = match factor.is_finite() && factor > 0.0 {
            true => (self.dist * factor).clamp(DIST_MIN, DIST_MAX),
            false => self.dist,
        };
        Self { dist, ..self }
    }

    /// Shifted by some fraction of the mesh's own span, held inside
    /// [`PAN_LIMIT`]. Non-finite input leaves the pan where it was, the same
    /// guard `dollied` holds a bad factor to.
    pub fn panned(self, dx: f32, dy: f32) -> Self {
        let shift = |v: f32, d: f32| {
            if d.is_finite() {
                (v + d).clamp(-PAN_LIMIT, PAN_LIMIT)
            } else {
                v
            }
        };
        Self { pan_x: shift(self.pan_x, dx), pan_y: shift(self.pan_y, dy), ..self }
    }
}

/// A mesh card's camera, or the default one.
pub fn orbit(item: &Item) -> Orbit {
    let fallback = Orbit::default();
    let Some(src) = item.meta.get("orbit").and_then(Value::as_object) else {
        return fallback;
    };
    let read = |key: &str, or: f32| {
        src.get(key)
            .and_then(Value::as_f64)
            .map(|v| v as f32)
            .filter(|v| v.is_finite())
            .unwrap_or(or)
    };
    Orbit {
        yaw: wrap_degrees(read("yaw", fallback.yaw)),
        pitch: read("pitch", fallback.pitch).clamp(-PITCH_LIMIT, PITCH_LIMIT),
        dist: read("dist", fallback.dist).clamp(DIST_MIN, DIST_MAX),
        pan_x: read("pan_x", fallback.pan_x).clamp(-PAN_LIMIT, PAN_LIMIT),
        pan_y: read("pan_y", fallback.pan_y).clamp(-PAN_LIMIT, PAN_LIMIT),
    }
}

pub fn set_orbit(item: &mut Item, orbit: Orbit) {
    let mut out = serde_json::Map::new();
    out.insert("yaw".into(), json_number(round_to(orbit.yaw, 100.0) as f64));
    out.insert("pitch".into(), json_number(round_to(orbit.pitch, 100.0) as f64));
    out.insert("dist".into(), json_number(round_to(orbit.dist, 1000.0) as f64));
    out.insert("pan_x".into(), json_number(round_to(orbit.pan_x, 1000.0) as f64));
    out.insert("pan_y".into(), json_number(round_to(orbit.pan_y, 1000.0) as f64));
    item.meta.insert("orbit".into(), Value::Object(out));
}

// ---------------------------------------------------------------------------
// Reading things that may not be what they claim
// ---------------------------------------------------------------------------

/// A flag, read as truthiness rather than as a strict boolean.
///
/// `BoardSettings` says its flags "should be read as truthiness tests", and the
/// same applies here for the same reason: these keys are shared with a build
/// that writes JSON out of a browser, where `1` and `"true"` are both things a
/// boolean can arrive as. Refusing them would turn a working board into a
/// silently muted one.
fn flag(item: &Item, key: &str) -> Option<bool> {
    match item.meta.get(key)? {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => Some(n.as_f64().is_some_and(|f| f != 0.0)),
        Value::String(s) => match s.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Some(true),
            "false" | "0" | "no" | "off" | "" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn number(item: &Item, key: &str) -> Option<f32> {
    item.meta.get(key).and_then(Value::as_f64).map(|v| v as f32)
}

/// A JSON number that is never `null`.
///
/// `serde_json::Number::from_f64` refuses infinities and NaN, and the `json!`
/// macro turns that refusal into `null` — which would put a `null` where a
/// reader expects a number. Every caller here has already held its value to a
/// finite range, so the fallback is unreachable; it is here so that the day one
/// of them stops doing that, the file still parses.
fn json_number(v: f64) -> Value {
    serde_json::Number::from_f64(v).map(Value::Number).unwrap_or_else(|| Value::from(0))
}

/// Degrees brought back into `-180..=180`.
fn wrap_degrees(deg: f32) -> f32 {
    if !deg.is_finite() {
        return 0.0;
    }
    let mut out = (deg + 180.0) % 360.0;
    if out < 0.0 {
        out += 360.0;
    }
    out - 180.0
}

/// Rounded, so that turning a mesh does not write fifteen digits per frame into
/// a file somebody is going to read.
fn round_to(v: f32, places: f32) -> f32 {
    (v * places).round() / places
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn item(kind: ItemType) -> Item {
        Item::new("a", kind)
    }

    #[test]
    fn video_arrives_silent_and_looping_and_audio_the_other_way_round() {
        let video = playback(&item(ItemType::Video));
        assert!(video.muted, "a board that makes noise when you open it");
        assert!(video.looping);

        let audio = playback(&item(ItemType::Audio));
        assert!(!audio.muted, "a muted audio card does nothing at all");
        assert!(!audio.looping);
    }

    #[test]
    fn an_animation_plays_itself_and_a_video_waits_to_be_asked() {
        assert!(playback(&item(ItemType::Image)).autoplay, "a GIF that sits still is broken");
        for kind in [ItemType::Video, ItemType::Audio] {
            assert!(!playback(&item(kind.clone())).autoplay, "{kind:?} started on its own");
        }
    }

    #[test]
    fn a_flag_that_has_been_set_is_written_even_when_it_matches_the_default() {
        // The whole of the module note: absent means "nobody has said", so a
        // decision that happens to agree with us still has to be recorded.
        let mut it = item(ItemType::Video);
        assert!(playback(&it).looping, "the default for a video");
        set_looping(&mut it, true);
        assert_eq!(it.meta.get("loop"), Some(&json!(true)), "the decision was dropped");
    }

    #[test]
    fn a_flag_is_read_as_truthiness() {
        // What a board written out of a browser can put here.
        for (written, expected) in
            [(json!(1), true), (json!(0), false), (json!("true"), true), (json!("off"), false)]
        {
            let mut it = item(ItemType::Video);
            it.meta.insert("muted".into(), written.clone());
            assert_eq!(playback(&it).muted, expected, "for {written}");
        }
    }

    #[test]
    fn a_flag_that_is_nonsense_falls_back_rather_than_deciding() {
        let mut it = item(ItemType::Video);
        it.meta.insert("muted".into(), json!({ "not": "a flag" }));
        assert!(playback(&it).muted, "it should have fallen back to the default");
    }

    #[test]
    fn a_volume_from_a_file_never_reaches_a_mixer_unheld() {
        for written in [json!(9999.0), json!(-3), json!("loud")] {
            let mut it = item(ItemType::Audio);
            it.meta.insert("volume".into(), written.clone());
            let v = playback(&it).volume;
            assert!((0.0..=1.0).contains(&v), "{written} gave {v}");
        }
    }

    #[test]
    fn a_volume_that_is_not_a_number_at_all_is_not_written() {
        let mut it = item(ItemType::Audio);
        set_volume(&mut it, f32::NAN);
        assert_eq!(playback(&it).volume, 1.0);
        assert!(it.meta["volume"].is_number(), "a null landed where a number goes");
    }

    #[test]
    fn a_duration_is_absent_rather_than_zero_when_nobody_has_measured_it() {
        let it = item(ItemType::Audio);
        assert_eq!(duration(&it), None);

        let mut it = item(ItemType::Audio);
        set_duration(&mut it, 12.5);
        assert_eq!(duration(&it), Some(12.5));

        // A file claiming a negative or absurd length is not a length.
        let mut it = item(ItemType::Audio);
        it.meta.insert("duration".into(), json!(-4));
        assert_eq!(duration(&it), None);
    }

    #[test]
    fn a_tag_is_tidied_and_bounded() {
        let mut it = item(ItemType::Audio);
        set_tag(&mut it, "artist", "  the   band  ");
        assert_eq!(artist(&it), Some("the band"));

        set_tag(&mut it, "album", &"x".repeat(TAG_MAX * 3));
        assert_eq!(album(&it).map(str::len), Some(TAG_MAX));

        // An empty tag is an absent one, not an empty string on the card.
        set_tag(&mut it, "artist", "   ");
        assert_eq!(artist(&it), None);
    }

    #[test]
    fn what_the_file_carried_decides_which_audio_card_this_is() {
        let bare = item(ItemType::Audio);
        assert!(!has_sleeve(&bare), "a voice memo");

        let mut tagged = item(ItemType::Audio);
        set_tag(&mut tagged, "artist", "somebody");
        assert!(has_sleeve(&tagged));

        let mut covered = item(ItemType::Audio);
        covered.meta.insert("cover".into(), json!("a".repeat(64)));
        assert!(has_sleeve(&covered));

        // An empty cover hash is not a cover.
        let mut empty = item(ItemType::Audio);
        empty.meta.insert("cover".into(), json!(""));
        assert!(!has_sleeve(&empty));
    }

    #[test]
    fn a_picture_is_known_to_be_silent_without_anybody_measuring_it() {
        // No image format carries a soundtrack, so this is the one answer that
        // needs no file read — and it is what keeps a mute button off a GIF.
        assert_eq!(has_sound(&item(ItemType::Image)), Some(false));
        assert_eq!(has_sound(&item(ItemType::Audio)), Some(true));
    }

    #[test]
    fn a_video_nobody_has_measured_is_a_maybe_rather_than_a_no() {
        // `None` and `Some(false)` mean different things to a control that
        // hides itself: guessing "no" here makes the mute button appear late.
        assert_eq!(has_sound(&item(ItemType::Video)), None);

        let mut silent = item(ItemType::Video);
        set_has_sound(&mut silent, false);
        assert_eq!(has_sound(&silent), Some(false));

        let mut loud = item(ItemType::Video);
        set_has_sound(&mut loud, true);
        assert_eq!(has_sound(&loud), Some(true));
    }

    #[test]
    fn whether_a_card_plays_survives_being_written_down() {
        // The whole of "pause three and restart the app".
        let mut it = item(ItemType::Video);
        assert!(!wants_to_play(&it), "a video should not start on its own");
        set_wants_to_play(&mut it, true);
        assert!(wants_to_play(&it));
        set_wants_to_play(&mut it, false);
        assert!(!wants_to_play(&it), "the pause was not recorded");
        assert_eq!(it.meta.get("autoplay"), Some(&json!(false)));
    }

    #[test]
    fn a_camera_never_reaches_the_pole() {
        let turned = Orbit::default().turned(0.0, 400.0);
        assert!(turned.pitch <= PITCH_LIMIT);
        let turned = Orbit::default().turned(0.0, -400.0);
        assert!(turned.pitch >= -PITCH_LIMIT);
    }

    #[test]
    fn turning_all_the_way_round_comes_back_to_where_it_started() {
        let once = Orbit::default().turned(360.0, 0.0);
        assert!((once.yaw - Orbit::default().yaw).abs() < 0.001, "{}", once.yaw);
        // And it stays in a range a reader can trust rather than growing.
        let far = Orbit::default().turned(3600.0, 0.0);
        assert!((-180.0..=180.0).contains(&far.yaw), "{}", far.yaw);
    }

    #[test]
    fn a_camera_cannot_be_pushed_inside_the_model_or_lost_behind_it() {
        assert_eq!(Orbit::default().dollied(0.0001).dist, DIST_MIN);
        assert_eq!(Orbit::default().dollied(1000.0).dist, DIST_MAX);
        // A nonsense factor leaves it where it was rather than moving it
        // somewhere unrepresentable.
        assert_eq!(Orbit::default().dollied(f32::NAN).dist, Orbit::default().dist);
    }

    #[test]
    fn a_camera_survives_a_round_trip_through_meta() {
        let mut it = item(ItemType::Model);
        let sent = Orbit::default().turned(-95.0, 12.0).dollied(0.6).panned(0.4, -0.2);
        set_orbit(&mut it, sent);
        let back = orbit(&it);
        assert!((back.yaw - sent.yaw).abs() < 0.01, "{back:?} != {sent:?}");
        assert!((back.pitch - sent.pitch).abs() < 0.01, "{back:?} != {sent:?}");
        assert!((back.dist - sent.dist).abs() < 0.01, "{back:?} != {sent:?}");
        assert!((back.pan_x - sent.pan_x).abs() < 0.01, "{back:?} != {sent:?}");
        assert!((back.pan_y - sent.pan_y).abs() < 0.01, "{back:?} != {sent:?}");
    }

    #[test]
    fn a_camera_cannot_be_panned_off_into_the_distance() {
        let far = Orbit::default().panned(400.0, -400.0);
        assert_eq!(far.pan_x, PAN_LIMIT);
        assert_eq!(far.pan_y, -PAN_LIMIT);
        // A nonsense shift leaves it where it was, same as `dollied`'s guard.
        let unmoved = Orbit::default().panned(f32::NAN, f32::NAN);
        assert_eq!(unmoved.pan_x, Orbit::default().pan_x);
        assert_eq!(unmoved.pan_y, Orbit::default().pan_y);
    }

    #[test]
    fn a_camera_out_of_a_broken_file_is_still_a_camera() {
        let mut it = item(ItemType::Model);
        it.meta.insert("orbit".into(), json!({ "yaw": "sideways", "pitch": 800, "dist": 0 }));
        let back = orbit(&it);
        assert_eq!(back.yaw, Orbit::default().yaw, "a bad axis should fall back");
        assert!(back.pitch <= PITCH_LIMIT);
        assert!(back.dist >= DIST_MIN);

        // And an `orbit` that is not an object at all does not panic.
        it.meta.insert("orbit".into(), json!("over there"));
        assert_eq!(orbit(&it), Orbit::default());
    }
}
