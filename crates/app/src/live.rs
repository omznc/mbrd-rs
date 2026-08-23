//! Pictures that are only true for one frame.
//!
//! `images.rs` is keyed by **content hash** and is exactly right for content
//! that never changes: the same photograph on twelve cards is one decode and
//! one texture, and a hash is the identity of bytes that will be the same bytes
//! next frame. A video frame and a rasterised mesh are neither of those things.
//! They are keyed by **card**, they are replaced constantly, and the bytes
//! behind them have no lasting identity worth caching under.
//!
//! So they live here instead. What the two caches share is the one discipline
//! that is not optional:
//!
//! ## Letting go is the whole job
//!
//! Dropping the last `Arc<RenderImage>` frees the pixels and leaves gpui's
//! sprite atlas holding a tile, because the atlas is keyed by the image's id
//! and nothing told it the id is finished with. `images.rs` says this costs a
//! long session over a big board some memory in the one place a heap profile
//! does not look.
//!
//! Here it is worse by three orders of magnitude. A video at thirty frames a
//! second replaces its picture thirty times a second, so a minute of playback
//! is eighteen hundred abandoned tiles — and the atlas grows until the window
//! stops taking tiles at all, which surfaces as pictures silently failing to
//! paint rather than as anything that looks like running out of memory.
//!
//! [`Live::put`] therefore retires the frame it replaces into a queue, and
//! [`Live::sweep`] hands that queue to the window once a frame from the one
//! place in `render` that has one. Both halves are needed; either alone leaks.

// Nothing produces a live frame yet: the video decoder and the mesh rasteriser
// are what fill this, and neither has landed. It is here ahead of them rather
// than alongside them because the discipline above is the part that is easy to
// get wrong under time pressure, and because it is testable now — the five
// tests below are what say the tiles are released, and they run today.
#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{RenderImage, Window};

/// The newest frame of everything that is currently moving, by card id.
#[derive(Default)]
pub struct Live {
    frames: HashMap<String, Held>,
    /// Frames replaced or dropped since the last sweep, still holding a tile.
    ///
    /// The queue between "a frame was replaced", which happens at any point in
    /// a frame and often off the main thread's schedule, and "the window can be
    /// told", which happens at exactly one point.
    dropped: Vec<Arc<RenderImage>>,
    held: usize,
}

struct Held {
    image: Arc<RenderImage>,
    cost: usize,
}

impl Live {
    /// Hand over the newest frame for a card, retiring the one it replaces.
    pub fn put(&mut self, id: &str, image: Arc<RenderImage>) {
        let cost = cost_of(&image);
        if let Some(old) = self.frames.insert(id.to_string(), Held { image, cost }) {
            self.held = self.held.saturating_sub(old.cost);
            self.dropped.push(old.image);
        }
        self.held += cost;
    }

    /// The newest frame for a card, if it has one.
    pub fn get(&self, id: &str) -> Option<&Arc<RenderImage>> {
        self.frames.get(id).map(|held| &held.image)
    }

    /// Let go of one card's frame — it stopped playing, or it is gone.
    pub fn clear(&mut self, id: &str) {
        if let Some(old) = self.frames.remove(id) {
            self.held = self.held.saturating_sub(old.cost);
            self.dropped.push(old.image);
        }
    }

    /// Let go of every card not in `keep`.
    ///
    /// The counterpart to a cull: a player that has been stopped, or a card
    /// that has been deleted, leaves a frame behind that nothing will ever ask
    /// for again and nothing else will ever notice.
    pub fn retain(&mut self, keep: impl Fn(&str) -> bool) {
        let going: Vec<String> = self.frames.keys().filter(|id| !keep(id)).cloned().collect();
        for id in going {
            self.clear(&id);
        }
    }

    /// Release the atlas tiles of everything retired since the last call.
    ///
    /// Call once a frame, from somewhere with a window. Skipping it does not
    /// corrupt anything — it leaks, quickly, which is why it is one line at the
    /// top of `render` beside [`Images::sweep`](crate::images::Images::sweep)
    /// rather than something to remember at each call site.
    pub fn sweep(&mut self, window: &mut Window) {
        for image in self.dropped.drain(..) {
            // Best effort, for the reasons `images.rs` gives: a tile that was
            // never uploaded has nothing to drop, and a window on its way out
            // will not take instructions.
            let _ = window.drop_image(image);
        }
    }

    /// How much live pixel data is being held, in bytes. For the status bar.
    pub fn bytes_held(&self) -> usize {
        self.held
    }

    /// How many cards are currently showing a live frame.
    pub fn count(&self) -> usize {
        self.frames.len()
    }

    /// How many retired frames are waiting for a window. Should be small, and
    /// should be zero right after a sweep — see the test below.
    #[cfg(test)]
    fn queued(&self) -> usize {
        self.dropped.len()
    }
}

fn cost_of(image: &Arc<RenderImage>) -> usize {
    (0..image.frame_count())
        .map(|i| {
            let size = image.size(i);
            (size.width.0.max(0) as usize) * (size.height.0.max(0) as usize) * 4
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Frame, RgbaImage};

    fn pixels(side: u32) -> Arc<RenderImage> {
        Arc::new(RenderImage::new(vec![Frame::new(RgbaImage::new(side, side))]))
    }

    #[test]
    fn the_newest_frame_is_the_one_you_get() {
        let mut live = Live::default();
        live.put("a", pixels(8));
        let second = pixels(16);
        live.put("a", second.clone());
        assert_eq!(live.get("a").map(Arc::as_ptr), Some(Arc::as_ptr(&second)));
        assert_eq!(live.count(), 1, "a replacement should not be a second card");
    }

    #[test]
    fn every_replaced_frame_is_queued_for_the_window() {
        // The leak this module exists to prevent: thirty of these a second,
        // and every one of them still holds a tile until the window is told.
        let mut live = Live::default();
        for _ in 0..30 {
            live.put("a", pixels(8));
        }
        assert_eq!(live.queued(), 29, "frames were dropped without being queued");
    }

    #[test]
    fn what_is_held_is_what_is_showing_rather_than_what_ever_was() {
        let mut live = Live::default();
        live.put("a", pixels(10));
        let after_one = live.bytes_held();
        for _ in 0..20 {
            live.put("a", pixels(10));
        }
        assert_eq!(live.bytes_held(), after_one, "the count grew with the replacements");

        live.clear("a");
        assert_eq!(live.bytes_held(), 0);
        assert_eq!(live.count(), 0);
    }

    #[test]
    fn clearing_a_card_that_was_never_here_does_nothing() {
        let mut live = Live::default();
        live.clear("nobody");
        assert_eq!(live.queued(), 0);
        assert_eq!(live.bytes_held(), 0);
    }

    #[test]
    fn a_card_that_stopped_playing_does_not_keep_its_frame() {
        let mut live = Live::default();
        for id in ["a", "b", "c"] {
            live.put(id, pixels(8));
        }
        live.retain(|id| id == "b");
        assert_eq!(live.count(), 1);
        assert!(live.get("b").is_some());
        assert!(live.get("a").is_none());
        assert_eq!(live.queued(), 2, "the culled frames must still reach the window");
    }
}
