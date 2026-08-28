//! Mesh rasters, kept apart from `images.rs`'s cache because a mesh's pixels
//! are not a pure function of its bytes the way a photograph's are.
//!
//! `images.rs` is keyed by content hash and decodes once per hash forever,
//! which is exactly right for a JPEG: the same bytes are the same picture on
//! every card that shares them. A mesh has a camera now
//! (`mbrd_core::media::Orbit`, kept per **item** in `item.meta.orbit`), so two
//! cards pointed at the same `.obj` can be turned to face two different ways
//! — the bytes are shared, the picture is not. So this cache is keyed
//! differently on each side of the one thing that actually is shared:
//!
//! - **`parsed`**, by content hash — the expensive half, turning a few
//!   megabytes of text or binary into a vertex buffer, is exactly as reusable
//!   across cards and across orbits as a JPEG decode is. Parsed once, kept
//!   for as long as any card wants it.
//! - **`resting`**, by item id — the cheap half, rasterising that vertex
//!   buffer from one angle, is redone every time the item's committed orbit
//!   changes, because the answer is different every time.
//!
//! While a drag or a scroll is actively turning a mesh, neither cache is
//! written to every frame — that traffic goes through `crate::live::Live`
//! instead, and only the final, released orbit is worth keeping here. See
//! `BoardView`'s gesture handling for where that line is drawn.
//!
//! No eviction budget, unlike `images.rs`. A board with hundreds of
//! photographs is normal; a board with hundreds of meshes large enough for
//! that to matter is not the case this cache is written for yet, and adding
//! one later costs nothing this shape does not already afford.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::RenderImage;
use image::{Frame, RgbaImage};
use mbrd_core::media::Orbit;
use mbrd_core::mesh::Mesh;

use crate::images::{to_bgra, Decoded, LONGEST_SIDE, THUMB_SIDE};

/// Parsed meshes and their last-rasterised-at-rest pictures.
#[derive(Default)]
pub struct Meshes {
    parsed: HashMap<String, Arc<Mesh>>,
    resting: HashMap<String, Decoded>,
    /// Item ids a background rasterise is already in flight for — the same
    /// "claim before starting" gate `images::Images::begin` uses, so a slow
    /// decode does not get asked for twice while it is still on its way.
    pending: HashSet<String>,
    /// The same gate for *live* frames, plus the one thing a resting decode
    /// never needs: the newest orbit that arrived while one was in flight.
    ///
    /// A resting decode is asked for once per committed change, so a claim is
    /// all it wants. A turning mesh is asked for once per **mouse move**,
    /// which arrives faster than any rasteriser finishes — so a bare claim
    /// would drop every frame but the first, and no claim at all queues an
    /// unbounded pile of them on the background executor and shows you
    /// whichever finishes last. Neither is what a drag should look like.
    ///
    /// So: at most one in flight, and at most one waiting behind it, the
    /// waiting one always overwritten by whatever the pointer did most
    /// recently. Latency is then one rasterise rather than a backlog, and
    /// because there is only ever one in flight per card, a stale frame
    /// cannot land after a newer one and turn the mesh back.
    turning: HashMap<String, Option<Orbit>>,
}

impl Meshes {
    /// A hash's parsed mesh, if this cache has already paid for one.
    pub fn parsed(&self, hash: &str) -> Option<Arc<Mesh>> {
        self.parsed.get(hash).cloned()
    }

    /// Keep a parsed mesh under its content hash, for every card that shares
    /// these bytes and every orbit any of them is ever turned to.
    pub fn cache_parsed(&mut self, hash: &str, mesh: Arc<Mesh>) {
        self.parsed.insert(hash.to_string(), mesh);
    }

    /// An item's picture at its last-committed orbit, if one has been drawn.
    pub fn resting(&self, item_id: &str) -> Option<&Decoded> {
        self.resting.get(item_id)
    }

    /// Replace an item's resting picture — called once a background rasterise
    /// at the item's *committed* orbit lands, never for a frame still being
    /// dragged (see the module doc: those go through `Live` instead).
    pub fn set_resting(&mut self, item_id: &str, decoded: Decoded) {
        self.resting.insert(item_id.to_string(), decoded);
    }

    /// Let go of everything held for a card that left the board.
    pub fn forget(&mut self, item_id: &str) {
        self.resting.remove(item_id);
        self.pending.remove(item_id);
        self.turning.remove(item_id);
    }

    /// Claim an item before starting a background rasterise. `false` means
    /// somebody already has one in flight and this call should not start a
    /// second one.
    pub fn begin(&mut self, item_id: &str) -> bool {
        self.pending.insert(item_id.to_string())
    }

    /// The rasterise this item claimed has landed, one way or another.
    pub fn settle(&mut self, item_id: &str) {
        self.pending.remove(item_id);
    }

    /// Claim an item before starting a *live* rasterise, the turning
    /// counterpart of [`Meshes::begin`]. `false` means one is already on its
    /// way and this orbit belongs in [`Meshes::want_live`] instead.
    pub fn begin_live(&mut self, item_id: &str) -> bool {
        if self.turning.contains_key(item_id) {
            return false;
        }
        self.turning.insert(item_id.to_string(), None);
        true
    }

    /// Hold this orbit as the one to draw next, replacing whatever was
    /// waiting. Does nothing for a card with no live rasterise in flight —
    /// there is nothing to wait behind, and the caller should have started
    /// one.
    pub fn want_live(&mut self, item_id: &str, orbit: Orbit) {
        if let Some(waiting) = self.turning.get_mut(item_id) {
            *waiting = Some(orbit);
        }
    }

    /// Give up on turning this card: release the claim and drop whatever was
    /// waiting behind it.
    ///
    /// For the paths where a live rasterise cannot be started at all — the
    /// card is gone, its bytes are not in the archive, its mesh has not been
    /// parsed yet. Draining the waiting orbit into the same path that just
    /// failed would only fail again; the claim has to come off instead, or
    /// the card is never drawn again for the rest of the session.
    pub fn abandon_live(&mut self, item_id: &str) {
        self.turning.remove(item_id);
    }

    /// A live rasterise has landed. Hands back the orbit that queued up
    /// behind it, if the pointer moved while it was drawing — and keeps the
    /// claim, because the caller is about to start that one. `None` releases
    /// the claim: the drag has caught up, and the next move starts fresh.
    pub fn settle_live(&mut self, item_id: &str) -> Option<Orbit> {
        let waiting = self.turning.get_mut(item_id)?.take();
        if waiting.is_none() {
            self.turning.remove(item_id);
        }
        waiting
    }
}

/// The default camera's distance — `dist` divides into this to become the
/// zoom multiplier `mbrd_core::mesh::rasterize` takes, so the existing
/// default orbit reproduces exactly the framing a mesh always used to have.
fn zoom_of(dist: f32) -> f32 {
    Orbit::default().dist / dist.max(1e-3)
}

/// Parse a mesh's bytes, trying each format this build reads in the same
/// order `images.rs` used to dispatch them in.
pub fn parse(bytes: &[u8]) -> Option<Arc<Mesh>> {
    mbrd_core::mesh::stl(bytes)
        .or_else(|| mbrd_core::mesh::glb(bytes))
        .or_else(|| mbrd_core::mesh::obj(bytes))
        .map(Arc::new)
}

/// A mesh, rasterised at `orbit` into the two-tier shape every picture in
/// `images.rs`'s cache is held in.
///
/// The thin shell around `mbrd_core::mesh`'s pure functions — fits a canvas to
/// the mesh's own silhouette at this orbit, the same way `images::svg` fits
/// one to a document's `viewBox` — moved here from `images.rs` because it now
/// needs an orbit, which `images::decode(bytes)` has no way to be handed.
/// Rasterised at [`mbrd_core::mesh::ANTIALIAS`], both tiers, because this is
/// the picture that is kept: a still nobody is currently dragging is looked
/// at, and a hard silhouette edge shows on one. The turning half is
/// [`rasterize_live`].
pub fn rasterize_tiers(mesh: &Mesh, orbit: Orbit) -> Option<Decoded> {
    // The rotation, paid for once and spent on both tiers — the two are the
    // same camera at two sizes, and it is only the canvas that differs.
    let view = mbrd_core::mesh::view(mesh, orbit.yaw, orbit.pitch)?;
    let (longest, span_w, span_h) = fit(&view)?;
    let zoom = zoom_of(orbit.dist);
    let ss = mbrd_core::mesh::ANTIALIAS;
    let thumb = one(&view, mesh, longest, span_w, span_h, THUMB_SIDE, &orbit, zoom, ss)?;
    let sharp = one(&view, mesh, longest, span_w, span_h, LONGEST_SIDE, &orbit, zoom, ss)?;
    Some(Decoded { thumb, sharp: Some(sharp) })
}

/// The one tier that is actually about to be shown, at one sample per pixel —
/// what a mesh being turned under the pointer is worth.
///
/// [`rasterize_tiers`] draws both tiers because it is filling a cache that
/// will be asked for either. A live frame is thrown away on the next mouse
/// move, so drawing the tier that is not on screen is the whole of the work
/// wasted, and on the board that is the 1024-side one: sixteen times the
/// pixels of the thumbnail beside it. Dropping the supersample takes another
/// four off, and costs an antialiased edge on an object that is moving. The
/// still that lands on release comes back through [`rasterize_tiers`] with
/// both back.
pub fn rasterize_live(mesh: &Mesh, orbit: Orbit, target: u32) -> Option<Arc<RenderImage>> {
    let view = mbrd_core::mesh::view(mesh, orbit.yaw, orbit.pitch)?;
    let (longest, span_w, span_h) = fit(&view)?;
    let zoom = zoom_of(orbit.dist);
    one(&view, mesh, longest, span_w, span_h, target, &orbit, zoom, 1)
}

/// The silhouette a canvas is shaped to, and the longer of its two sides —
/// `None` for a view whose extent is not a number a canvas can be cut from.
fn fit(view: &mbrd_core::mesh::View) -> Option<(f32, f32, f32)> {
    let (span_w, span_h) = view.aspect();
    let longest = span_w.max(span_h);
    (!longest.is_nan() && longest > 0.0).then_some((longest, span_w, span_h))
}

/// One rasterisation of a mesh, on a canvas shaped to its own silhouette and
/// scaled so the longer of the two sides lands on `target`.
#[allow(clippy::too_many_arguments)]
fn one(
    view: &mbrd_core::mesh::View,
    mesh: &Mesh,
    longest: f32,
    span_w: f32,
    span_h: f32,
    target: u32,
    orbit: &Orbit,
    zoom: f32,
    ss: u32,
) -> Option<Arc<RenderImage>> {
    let scale = target as f32 / longest;
    let w = ((span_w * scale).round() as u32).max(1);
    let h = ((span_h * scale).round() as u32).max(1);
    let raster = mbrd_core::mesh::rasterize_view(
        view,
        mesh,
        w,
        h,
        zoom,
        orbit.pan_x,
        orbit.pan_y,
        ss,
    )?;
    let mut rgba = RgbaImage::from_raw(w, h, raster.rgba)?;
    to_bgra(&mut rgba);
    Some(Arc::new(RenderImage::new(vec![Frame::new(rgba)])))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn longest(image: &Arc<RenderImage>) -> i32 {
        image.size(0).width.0.max(image.size(0).height.0)
    }

    fn opaque(image: &Arc<RenderImage>) -> bool {
        image.as_bytes(0).expect("one frame").as_chunks::<4>().0.iter().any(|px| px[3] == 255)
    }

    /// A flat, front-facing quad wide across one axis, so the two screen axes
    /// are told apart the same way `images.rs`'s own picture fixtures are.
    fn stl_bytes() -> Vec<u8> {
        let quad: [[f32; 3]; 4] =
            [[-3.0, -1.0, 0.0], [3.0, -1.0, 0.0], [3.0, 1.0, 0.0], [-3.0, 1.0, 0.0]];
        let mut out = vec![0_u8; 84];
        out[80..84].copy_from_slice(&2_u32.to_le_bytes());
        for tri in [[0, 1, 2], [0, 2, 3]] {
            out.extend_from_slice(&[0_u8; 12]);
            for i in tri {
                for axis in quad[i] {
                    out.extend_from_slice(&axis.to_le_bytes());
                }
            }
            out.extend_from_slice(&[0_u8; 2]);
        }
        out
    }

    /// The same quad as `stl_bytes`, as a Wavefront OBJ face instead of a
    /// binary facet list.
    fn obj_bytes() -> Vec<u8> {
        b"v -3 -1 0\nv 3 -1 0\nv 3 1 0\nv -3 1 0\nf 1 2 3 4\n".to_vec()
    }

    /// The same quad again, as a minimal indexed `.glb` — one bufferView of
    /// positions, one of indices, both read out of a single `BIN` chunk.
    fn glb_bytes() -> Vec<u8> {
        let quad: [[f32; 3]; 4] =
            [[-3.0, -1.0, 0.0], [3.0, -1.0, 0.0], [3.0, 1.0, 0.0], [-3.0, 1.0, 0.0]];
        let mut bin = Vec::new();
        for v in quad {
            for axis in v {
                bin.extend_from_slice(&axis.to_le_bytes());
            }
        }
        let idx_offset = bin.len();
        let indices: [u16; 6] = [0, 1, 2, 0, 2, 3];
        for i in indices {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        let json = format!(
            r#"{{"bufferViews":[
                    {{"buffer":0,"byteOffset":0,"byteLength":48}},
                    {{"buffer":0,"byteOffset":{idx_offset},"byteLength":12}}
                 ],
                 "accessors":[
                    {{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3"}},
                    {{"bufferView":1,"componentType":5123,"count":6,"type":"SCALAR"}}
                 ],
                 "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}},"indices":1}}]}}]}}"#,
        );
        let json = json.as_bytes();
        let total = 12 + 8 + json.len() + 8 + bin.len();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2_u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(json);
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&bin);
        out
    }

    #[test]
    fn a_binary_stl_rasterises_to_both_tiers_with_something_drawn_on_them() {
        let mesh = parse(&stl_bytes()).expect("that is a binary stl");
        let decoded = rasterize_tiers(&mesh, Orbit::default()).expect("a quad has an extent");
        assert_eq!(longest(&decoded.thumb), THUMB_SIDE as i32);
        let sharp = decoded.sharp.expect("a mesh always gets a second tier");
        assert_eq!(longest(&sharp), LONGEST_SIDE as i32);
        assert!(opaque(&decoded.thumb), "the quad's face never made it onto the canvas");
    }

    #[test]
    fn an_obj_rasterises_to_both_tiers_with_something_drawn_on_them() {
        let mesh = parse(&obj_bytes()).expect("that is an obj");
        let decoded = rasterize_tiers(&mesh, Orbit::default()).expect("a quad has an extent");
        assert_eq!(longest(&decoded.thumb), THUMB_SIDE as i32);
        assert!(opaque(&decoded.thumb));
    }

    #[test]
    fn a_glb_rasterises_to_both_tiers_with_something_drawn_on_them() {
        let mesh = parse(&glb_bytes()).expect("that is a glb");
        let decoded = rasterize_tiers(&mesh, Orbit::default()).expect("a quad has an extent");
        assert_eq!(longest(&decoded.thumb), THUMB_SIDE as i32);
        assert!(opaque(&decoded.thumb));
    }

    #[test]
    fn a_live_frame_is_the_one_tier_it_was_asked_for() {
        let mesh = parse(&stl_bytes()).expect("that is a binary stl");
        for target in [THUMB_SIDE, LONGEST_SIDE] {
            let frame =
                rasterize_live(&mesh, Orbit::default(), target).expect("a quad has an extent");
            assert_eq!(longest(&frame), target as i32);
            assert!(opaque(&frame), "the quad's face never made it onto the canvas");
        }
    }

    #[test]
    fn a_live_frame_is_the_size_the_resting_one_will_be() {
        // What the drop to one sample per pixel is not allowed to change. The
        // swap from `live` to `resting` on release has to be a change of
        // sharpness and nothing else — a card that resized on mouse-up would
        // read as the picture flinching at the exact moment you let go.
        let mesh = parse(&stl_bytes()).expect("that is a binary stl");
        let orbit = Orbit { yaw: 200.0, ..Orbit::default() };
        let live = rasterize_live(&mesh, orbit, THUMB_SIDE).unwrap();
        let resting = rasterize_tiers(&mesh, orbit).unwrap();
        assert_eq!(live.size(0), resting.thumb.size(0));
    }

    #[test]
    fn turning_the_orbit_changes_the_resting_picture() {
        let mesh = parse(&stl_bytes()).expect("that is a binary stl");
        let a = rasterize_tiers(&mesh, Orbit::default()).unwrap();
        let b = rasterize_tiers(&mesh, Orbit { yaw: 200.0, ..Orbit::default() }).unwrap();
        assert_ne!(a.thumb.as_bytes(0), b.thumb.as_bytes(0), "a turned mesh should look different");
    }

    #[test]
    fn the_parsed_cache_hands_back_what_it_was_given() {
        let mut meshes = Meshes::default();
        assert!(meshes.parsed("h").is_none());
        let mesh = parse(&stl_bytes()).unwrap();
        meshes.cache_parsed("h", mesh.clone());
        assert!(Arc::ptr_eq(&meshes.parsed("h").unwrap(), &mesh));
    }

    #[test]
    fn only_one_rasterise_may_be_claimed_per_item_at_a_time() {
        let mut meshes = Meshes::default();
        assert!(meshes.begin("card"), "nobody has claimed it yet");
        assert!(!meshes.begin("card"), "a second claim while the first is in flight");
        meshes.settle("card");
        assert!(meshes.begin("card"), "settled, so a new one may be claimed");
    }

    #[test]
    fn a_live_rasterise_is_claimed_the_same_way_a_resting_one_is() {
        let mut meshes = Meshes::default();
        assert!(meshes.begin_live("card"), "nobody is turning it yet");
        assert!(!meshes.begin_live("card"), "a second claim while the first is in flight");
        assert_eq!(meshes.settle_live("card"), None, "the pointer never moved while it drew");
        assert!(meshes.begin_live("card"), "settled, so a new one may be claimed");
    }

    #[test]
    fn only_the_newest_orbit_waits_behind_the_one_being_drawn() {
        // The whole of the coalescing: a drag emits mouse moves faster than
        // anything rasterises them, and every one of those but the last is
        // already out of date by the time a slot opens. Keeping them all is
        // the backlog; keeping the newest is the drag.
        let mut meshes = Meshes::default();
        assert!(meshes.begin_live("card"));

        for yaw in [10.0, 20.0, 30.0] {
            meshes.want_live("card", Orbit { yaw, ..Orbit::default() });
        }
        let next = meshes.settle_live("card").expect("three moves arrived while it drew");
        assert_eq!(next.yaw, 30.0, "the frame drawn next is where the pointer is now");

        // The claim is still held, because the caller is about to draw that
        // one — a second `begin_live` here would be the second in flight.
        assert!(!meshes.begin_live("card"));
        assert_eq!(meshes.settle_live("card"), None, "nothing arrived behind it this time");
        assert!(meshes.begin_live("card"), "and now it is free again");
    }

    #[test]
    fn an_orbit_wanted_by_a_card_nobody_is_drawing_is_not_kept() {
        // `want_live` is only ever "queue behind the one in flight". With
        // none in flight there is nothing to queue behind, and inventing a
        // claim here would strand it: no rasterise is coming to drain it.
        let mut meshes = Meshes::default();
        meshes.want_live("card", Orbit { yaw: 90.0, ..Orbit::default() });
        assert_eq!(meshes.settle_live("card"), None);
        assert!(meshes.begin_live("card"), "the card was never claimed");
    }

    #[test]
    fn forgetting_a_card_drops_its_resting_picture_and_its_claim() {
        let mut meshes = Meshes::default();
        let mesh = parse(&stl_bytes()).unwrap();
        let decoded = rasterize_tiers(&mesh, Orbit::default()).unwrap();
        meshes.set_resting("card", decoded);
        meshes.begin("other");
        meshes.begin_live("card");
        meshes.begin_live("other");
        meshes.forget("card");
        assert!(meshes.resting("card").is_none());
        assert!(meshes.begin_live("card"), "a card that left the board is not still turning");
        assert!(!meshes.begin("other"), "forgetting one card must not touch another's claim");
        assert!(!meshes.begin_live("other"), "nor another's live claim");
    }
}
