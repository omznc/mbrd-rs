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
pub fn rasterize_tiers(mesh: &Mesh, orbit: Orbit) -> Option<Decoded> {
    let (span_w, span_h) = mbrd_core::mesh::aspect(mesh, orbit.yaw, orbit.pitch)?;
    let longest = span_w.max(span_h);
    if longest.is_nan() || longest <= 0.0 {
        return None;
    }
    let zoom = zoom_of(orbit.dist);
    let thumb = one(mesh, longest, span_w, span_h, THUMB_SIDE, &orbit, zoom)?;
    let sharp = one(mesh, longest, span_w, span_h, LONGEST_SIDE, &orbit, zoom)?;
    Some(Decoded { thumb, sharp: Some(sharp) })
}

/// One rasterisation of a mesh, on a canvas shaped to its own silhouette and
/// scaled so the longer of the two sides lands on `target`.
#[allow(clippy::too_many_arguments)]
fn one(
    mesh: &Mesh,
    longest: f32,
    span_w: f32,
    span_h: f32,
    target: u32,
    orbit: &Orbit,
    zoom: f32,
) -> Option<Arc<RenderImage>> {
    let scale = target as f32 / longest;
    let w = ((span_w * scale).round() as u32).max(1);
    let h = ((span_h * scale).round() as u32).max(1);
    let raster = mbrd_core::mesh::rasterize(
        mesh,
        w,
        h,
        orbit.yaw,
        orbit.pitch,
        zoom,
        orbit.pan_x,
        orbit.pan_y,
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
    fn forgetting_a_card_drops_its_resting_picture_and_its_claim() {
        let mut meshes = Meshes::default();
        let mesh = parse(&stl_bytes()).unwrap();
        let decoded = rasterize_tiers(&mesh, Orbit::default()).unwrap();
        meshes.set_resting("card", decoded);
        meshes.begin("other");
        meshes.forget("card");
        assert!(meshes.resting("card").is_none());
        assert!(!meshes.begin("other"), "forgetting one card must not touch another's claim");
    }
}
