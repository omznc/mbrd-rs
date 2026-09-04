//! The media stack, on a platform none of the three backends knows about.
//!
//! The fourth file behind `mod pipeline`, and the one nothing ships. `main.rs`
//! picks a real backend for Linux, macOS and Windows; this is what a fifth
//! platform gets — a BSD, a Redox, a target that arrives after this was
//! written. Every method is the no-op the real ones already perform for a card
//! with no reel behind it, which is why it is forty lines rather than a second
//! implementation of anything.
//!
//! ## Why it exists rather than a `cfg` on every call site
//!
//! Each real backend links against something: GStreamer, AVFoundation, the
//! Media Foundation Media Engine. Those are *link-time* dependencies, so a
//! target with none of them cannot compile any of the three, and the
//! alternative to this file is `#[cfg]` scattered through `board_view.rs`
//! around code that is otherwise identical everywhere.
//!
//! The board draws every transport strip either way, because the strips were
//! never the pipeline's — see [`crate::playback`], which holds the playheads
//! and has no decoder in it anywhere. A press on such a platform gets the same
//! answer a broken file gets on a real one: said once, in the card, in words.
//!
//! If somebody wants one of these platforms to play, the work is small and it
//! is not here: most of them have GStreamer, so it is a `cfg` widened in
//! `main.rs` and `Cargo.toml` rather than a new file.

use std::sync::Arc;
use std::time::Duration;

use gpui::{RenderImage, Window};

/// What one frame of the clock says about a card that is playing. The real
/// one's twin — see [`crate::pipeline`] — kept in step by the fact that
/// `board_view` reads every field of it.
#[derive(Debug, Clone, Default)]
pub struct Beat {
    pub at: Duration,
    pub length: Option<Duration>,
    pub ended: bool,
    pub fresh: bool,
    pub trouble: Option<String>,
}

/// What asking for a card's decoder found. The real one's twin — see
/// [`crate::pipeline`], which is where the three answers are argued.
///
/// This backend only ever gives one of them, and that is the point: there is
/// no decoder here and there is never going to be one, so a card is refused
/// rather than left waiting for a file that nothing would open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Opening {
    Ready,
    Waiting,
    Refused,
}

/// Every reel there is, which here is none of them.
///
/// What it does keep is the cards somebody has pressed and not yet been
/// answered about. That is the same shape the real one uses for a file it
/// could not open — a reel that exists only to be reported once and then
/// dropped — which is why `board_view` needs no branch for this platform.
#[derive(Default)]
pub struct Stack {
    owed: std::collections::HashSet<String>,
}

impl Stack {
    pub fn new() -> Self {
        Self::default()
    }

    /// Always refused, so no playhead ever starts. The card is remembered so
    /// that the next frame can say why, once.
    pub fn open(
        &mut self,
        id: &str,
        _hash: &str,
        _ext: &str,
        _bytes: &[u8],
        _video: bool,
    ) -> Opening {
        self.owed.insert(id.to_string());
        Opening::Refused
    }

    pub fn play(&mut self, _id: &str) {}

    pub fn pause(&mut self, _id: &str) {}

    pub fn seek(&mut self, _id: &str, _at: Duration) {}

    pub fn set_loudness(&mut self, _id: &str, _level: f32, _muted: bool) {}

    /// The answer, given once and then forgotten — the same contract
    /// `Reel::told` holds on the real one.
    pub fn poll(&mut self, id: &str, _looping: bool) -> Option<Beat> {
        self.owed.remove(id).then(|| Beat {
            trouble: Some("this build plays no sound or video".into()),
            ..Beat::default()
        })
    }

    pub fn picture(&self, _id: &str) -> Option<Arc<RenderImage>> {
        None
    }

    pub fn open_reels(&self) -> Vec<String> {
        self.owed.iter().cloned().collect()
    }

    pub fn forget(&mut self, id: &str) {
        self.owed.remove(id);
    }

    pub fn forget_all(&mut self) {
        self.owed.clear();
    }

    pub fn trim(&mut self, _keep: usize) {}

    /// Nothing to hand back: there is no picture on this platform, so there is
    /// no atlas tile behind one either. See [`crate::pipeline::Stack::sweep`],
    /// which is the method this one is the shape of.
    pub fn sweep(&mut self, _window: &mut Window) {}
}
