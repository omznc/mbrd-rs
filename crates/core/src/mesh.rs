//! A mesh, rasterised to a still picture with no GPU anywhere near it.
//!
//! The port's original call was to defer 3D on the grounds that GPUI has no 3D.
//! That is true and it is the wrong constraint: a still of a mesh needs no GPU at
//! all. Transform the vertices, cull the back faces, z-buffer, flat-shade the
//! triangles into a flat RGBA buffer — a few hundred lines of arithmetic
//! against a vertex buffer, which is to say exactly the kind of thing this
//! crate is for, testable by rasterising a cube and asserting which pixels
//! came out.
//!
//! ## What this will not do, said before anybody asks
//!
//! No materials, no textures, no per-vertex normals read off the file even
//! where one carries them: a face's own geometry gives a flat-shaded normal
//! for free, and a `.obj` dragged onto a board arrives without its `.mtl` and
//! without its texture maps regardless — one file was dropped, and one file
//! is what there is.
//!
//! It does turn, now: [`rasterize`] takes a `yaw`/`pitch`/`zoom`/`pan_x`/
//! `pan_y`, and `mbrd_core::media::Orbit` is where the app keeps one per mesh
//! card, persisted through `item.meta` the same way volume and loop are. The
//! camera is still a straight-on orthographic fit rather than a true
//! perspective one — `zoom` scales the existing fit-to-canvas margin and
//! `pan_x`/`pan_y` shift the look-at point within it, rather than moving a
//! camera through space — which keeps this rasteriser exactly as tested as
//! it always was; a real camera distance, with foreshortening, is a further
//! change to the projection and the z-buffer's sign convention that this
//! file does not make.
//!
//! It is also antialiased: [`rasterize`] renders at twice the asked-for
//! resolution and boxes the result back down (see [`downsample`]), because a
//! single sample per pixel draws every silhouette edge as a hard, aliased
//! step.

/// A triangle mesh: positions, and the triangles they make. Nothing else —
/// see the module header for why not.
#[derive(Debug, Clone, PartialEq)]
pub struct Mesh {
    pub vertices: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

/// Past this many triangles, [`rasterize`] declines rather than spending a
/// background-executor slot on a render that will not finish before the next
/// one is due. Chosen the way [`crate::preview::ENTRIES_MAX`] was: high
/// enough that nothing anybody drags onto a moodboard is turned away, low
/// enough that the cost of one still is bounded regardless of what shows up.
pub const TRIANGLE_MAX: usize = 200_000;

/// A flat RGBA8 buffer, straight alpha, one frame. The same shape
/// `images::straighten` already hands to `to_bgra` for a rasterised SVG, kept
/// dependency-free here because this crate has no reason to know what
/// `image` or `gpui` are — see the crate's own layering note in `lib.rs`.
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

// ---------------------------------------------------------------------------
// STL
// ---------------------------------------------------------------------------

/// Whether these bytes are a binary STL.
///
/// Not asked of the header text: an exporter is free to write `solid ...` as
/// the first line of a *binary* file as a courtesy to anyone who opens it in
/// an editor, so a "starts with `solid`" check would call plenty of binary
/// files ASCII and plenty of ASCII files binary depending on what somebody
/// felt like typing. What is reliable is the shape: a binary STL is exactly
/// an 84-byte header followed by the triangle count it declares, times 50
/// bytes each, and nothing a person writes by hand produces that byte count
/// by accident.
pub fn is_stl(bytes: &[u8]) -> bool {
    let Some(count) = triangle_count(bytes) else { return false };
    // In `u64`, not in `usize`, and that is not tidiness. `usize` is 32 bits on
    // wasm, where a declared count of four billion times fifty does not fit —
    // and this is asked of *every* file that is imported, because `classify`
    // sniffs for a mesh before it looks at a name. Four arbitrary bytes at
    // offset 80 are all it takes: in a debug build the multiplication panics
    // and takes the whole page down mid-import, and in a release build it
    // wraps and can call a text file an STL. Sixty-four bits cannot overflow
    // on either kind of machine, and the comparison needs no word size of its
    // own.
    bytes.len() as u64 == 84 + count as u64 * 50
}

/// The triangle count alone, off the four bytes that carry it — cheap enough
/// to read at import without parsing a single vertex. The same split
/// `facts::pages` keeps with a PDF's page count: written once, so the rail
/// can say it without opening the file. See `Ready::triangles` in the UI
/// crate's `import.rs`.
pub fn triangle_count(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(80..84)?.try_into().ok()?))
}

/// A binary STL's triangles, each with its own three vertices — STL shares
/// none of them, so this does not try to weld matching corners back together.
pub fn stl(bytes: &[u8]) -> Option<Mesh> {
    if !is_stl(bytes) {
        return None;
    }
    let count = triangle_count(bytes)? as usize;
    let mut vertices = Vec::with_capacity(count * 3);
    let mut triangles = Vec::with_capacity(count);
    for n in 0..count {
        // 50 bytes per facet: a normal this build recomputes from the
        // geometry instead of trusting (12 bytes), three vertices (36
        // bytes), and a 2-byte attribute count nothing here reads.
        let at = 84 + n * 50 + 12;
        let mut corners = [[0.0_f32; 3]; 3];
        for (c, corner) in corners.iter_mut().enumerate() {
            for (a, axis) in corner.iter_mut().enumerate() {
                let start = at + c * 12 + a * 4;
                *axis = f32::from_le_bytes(bytes.get(start..start + 4)?.try_into().ok()?);
            }
        }
        let base = vertices.len() as u32;
        vertices.extend_from_slice(&corners);
        triangles.push([base, base + 1, base + 2]);
    }
    Some(Mesh { vertices, triangles })
}

// ---------------------------------------------------------------------------
// Wavefront OBJ
// ---------------------------------------------------------------------------

/// Whether these bytes read like a Wavefront OBJ — cheap enough to run at
/// preview time, because it parses no vertex: a text mesh has no magic bytes
/// to check the way a binary one does, so this looks for the one thing every
/// OBJ has and almost nothing else does, a line that is just `v` followed by
/// numbers and, somewhere among the first couple of hundred lines, one that
/// is just `f` followed by more of them.
///
/// A caller reaching for this is expected to have the extension in hand
/// already and to have checked it says `obj` first — unlike [`is_stl`] and
/// [`is_glb`], there is no byte pattern here that could not, in principle, be
/// the opening of some other text file, and the ids `.obj` and `.stl` and
/// `.glb` do not carry are the ones `schema::is_paper_id`-style whitelisting
/// exists for elsewhere. See `preview::bytes`, which does check it.
pub fn is_obj(bytes: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(bytes) else { return false };
    // No line cap: a real mesh lists every vertex before its first face, and a
    // "base mesh" is exactly the shape that has thousands of them — a 200-line
    // window saw only vertices and called it text. The whole buffer is
    // already walked once above to check it is UTF-8, so a second linear pass
    // over the same bytes costs nothing this check doesn't already pay for.
    let (mut has_v, mut has_f) = (false, false);
    for line in text.lines() {
        match line.split_whitespace().next() {
            Some("v") => has_v = true,
            Some("f") => has_f = true,
            _ => {}
        }
        if has_v && has_f {
            return true;
        }
    }
    false
}

/// An OBJ's triangles, fanned from whatever polygons its `f` lines describe —
/// a quad or a pentagon becomes two or three triangles sharing its first
/// corner, which is a face's own choice of diagonal and not always the one a
/// modelling tool would have picked, but it is a choice made honestly out of
/// the vertices that are actually there rather than a guess at a better one.
///
/// Every face index takes only the vertex component (`v`, `v/vt`, `v/vt/vn`
/// and `v//vn` all read the same) — no materials, no texture coordinates,
/// [`stl`]'s "arrives without its `.mtl`" note holds here as much as
/// anywhere. A negative index is OBJ's own way of counting back from
/// whichever vertex was most recently read, and is honoured for the same
/// reason `-1` in a Python slice is: some exporters only ever write it that
/// way.
pub fn obj(bytes: &[u8]) -> Option<Mesh> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut vertices: Vec<[f32; 3]> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();

    for line in text.lines() {
        let mut tokens = line.split_whitespace();
        match tokens.next() {
            Some("v") => {
                let mut nums = tokens.filter_map(|t| t.parse::<f32>().ok());
                let (Some(x), Some(y), Some(z)) = (nums.next(), nums.next(), nums.next()) else {
                    continue;
                };
                vertices.push([x, y, z]);
            }
            Some("f") => {
                let seen = vertices.len() as i64;
                let corners: Vec<u32> = tokens
                    .filter_map(|token| {
                        let i: i64 = token.split('/').next()?.parse().ok()?;
                        let index = if i > 0 { i - 1 } else { seen + i };
                        (0..seen).contains(&index).then_some(index as u32)
                    })
                    .collect();
                // Fanned from the first corner, so a triangle contributes one
                // triangle and every larger polygon contributes one fewer
                // triangle than it has corners.
                for window in 1..corners.len().saturating_sub(1) {
                    triangles.push([corners[0], corners[window], corners[window + 1]]);
                }
            }
            _ => {}
        }
    }
    (!triangles.is_empty()).then_some(Mesh { vertices, triangles })
}

// ---------------------------------------------------------------------------
// glTF binary (.glb)
// ---------------------------------------------------------------------------

/// Whether these bytes are a binary glTF container — the 12-byte header every
/// `.glb` opens with: the literal `glTF`, then a version, then the length of
/// the whole file. Unlike [`is_obj`], this is a real magic number, so this one
/// needs nothing else checked before it is trusted, the same as [`is_stl`].
pub fn is_glb(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[0..4] == b"glTF"
}

/// One embedded chunk of a `.glb`: the four-byte type that names it (`JSON`
/// or `BIN\0`) and the bytes themselves, with the length and padding already
/// taken off.
fn glb_chunks(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let total = u32::from_le_bytes(bytes.get(8..12)?.try_into().ok()?) as usize;
    let end = total.min(bytes.len());
    let (mut at, mut json, mut bin) = (12usize, None, None);
    while at + 8 <= end {
        let len = u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?) as usize;
        let kind = bytes.get(at + 4..at + 8)?;
        let data_start = at + 8;
        let data_end = data_start.checked_add(len)?;
        if data_end > end {
            break;
        }
        match kind {
            b"JSON" => json = Some(&bytes[data_start..data_end]),
            b"BIN\0" => bin = Some(&bytes[data_start..data_end]),
            _ => {}
        }
        at = data_end;
    }
    Some((json?, bin.unwrap_or(&[])))
}

/// The `VEC3` accessor's own values, read straight out of the `BIN` chunk —
/// every `POSITION` a glTF primitive can have, since the format allows no
/// other component type or shape for it.
fn glb_positions(
    accessors: &[serde_json::Value],
    views: &[serde_json::Value],
    bin: &[u8],
    accessor: usize,
) -> Option<Vec<[f32; 3]>> {
    let acc = accessors.get(accessor)?;
    if acc.get("componentType")?.as_u64()? != 5126 || acc.get("type")?.as_str()? != "VEC3" {
        return None;
    }
    let (start, stride, count) = glb_layout(acc, views, 12)?;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let at = start + i * stride;
        let x = f32::from_le_bytes(bin.get(at..at + 4)?.try_into().ok()?);
        let y = f32::from_le_bytes(bin.get(at + 4..at + 8)?.try_into().ok()?);
        let z = f32::from_le_bytes(bin.get(at + 8..at + 12)?.try_into().ok()?);
        out.push([x, y, z]);
    }
    Some(out)
}

/// A `SCALAR` accessor's values widened to `u32`, whatever they were stored
/// as — a glTF index buffer is one of three integer widths, chosen for size
/// rather than for any difference in meaning.
fn glb_indices(
    accessors: &[serde_json::Value],
    views: &[serde_json::Value],
    bin: &[u8],
    accessor: usize,
) -> Option<Vec<u32>> {
    let acc = accessors.get(accessor)?;
    if acc.get("type")?.as_str()? != "SCALAR" {
        return None;
    }
    let component = acc.get("componentType")?.as_u64()?;
    let width = match component {
        5121 | 5120 => 1, // UNSIGNED_BYTE, BYTE
        5123 | 5122 => 2, // UNSIGNED_SHORT, SHORT
        5125 => 4,        // UNSIGNED_INT
        _ => return None,
    };
    let (start, stride, count) = glb_layout(acc, views, width)?;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let at = start + i * stride;
        out.push(match width {
            1 => *bin.get(at)? as u32,
            2 => u16::from_le_bytes(bin.get(at..at + 2)?.try_into().ok()?) as u32,
            _ => u32::from_le_bytes(bin.get(at..at + 4)?.try_into().ok()?),
        });
    }
    Some(out)
}

/// Where an accessor's values start in the `BIN` chunk, how far apart each
/// one is, and how many there are — the arithmetic [`glb_positions`] and
/// [`glb_indices`] both need once they know their element's own width.
/// Only buffer `0` is ever read: the single `BIN` chunk embedded in a `.glb`
/// is that buffer by construction, and an accessor pointing anywhere else is
/// one this build has no bytes for.
fn glb_layout(
    accessor: &serde_json::Value,
    views: &[serde_json::Value],
    width: usize,
) -> Option<(usize, usize, usize)> {
    let count = accessor.get("count")?.as_u64()? as usize;
    let acc_offset = accessor.get("byteOffset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let view = views.get(accessor.get("bufferView")?.as_u64()? as usize)?;
    if view.get("buffer").and_then(|v| v.as_u64()).unwrap_or(0) != 0 {
        return None;
    }
    let view_offset = view.get("byteOffset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
    let stride = view.get("byteStride").and_then(|v| v.as_u64()).map_or(width, |s| s as usize);
    Some((view_offset + acc_offset, stride, count))
}

/// Every triangle-mode primitive of every mesh in a `.glb`, concatenated into
/// one [`Mesh`] — glTF's own way of grouping several draw calls under one
/// name carries nothing this rasteriser draws differently, so there is
/// nothing to lose by flattening it. A primitive with no `indices` is drawn
/// the way glTF says an unindexed one is: every three positions in a row are
/// a triangle. A primitive this build cannot read — a `mode` other than
/// triangles, a `POSITION` in a shape or type it does not expect — is
/// skipped rather than failing the whole file, the same as a bad `f` line in
/// [`obj`].
pub fn glb(bytes: &[u8]) -> Option<Mesh> {
    if !is_glb(bytes) {
        return None;
    }
    let (json, bin) = glb_chunks(bytes)?;
    let root: serde_json::Value = serde_json::from_slice(json).ok()?;
    let accessors = root.get("accessors")?.as_array()?;
    let views = root.get("bufferViews")?.as_array()?;
    let meshes = root.get("meshes").and_then(|v| v.as_array())?;

    let mut vertices: Vec<[f32; 3]> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    for mesh in meshes {
        let Some(primitives) = mesh.get("primitives").and_then(|v| v.as_array()) else { continue };
        for prim in primitives {
            // 4 is TRIANGLES, and the default when `mode` is left off.
            if prim.get("mode").and_then(|v| v.as_u64()).unwrap_or(4) != 4 {
                continue;
            }
            let Some(position) = prim.get("attributes").and_then(|a| a.get("POSITION")) else {
                continue;
            };
            let Some(position) = position.as_u64() else { continue };
            let Some(positions) = glb_positions(accessors, views, bin, position as usize) else {
                continue;
            };
            let base = vertices.len() as u32;
            vertices.extend_from_slice(&positions);

            match prim.get("indices").and_then(|v| v.as_u64()) {
                Some(accessor) => {
                    let Some(index) = glb_indices(accessors, views, bin, accessor as usize) else {
                        continue;
                    };
                    triangles.extend(
                        index
                            .as_chunks::<3>()
                            .0
                            .iter()
                            .map(|t| [base + t[0], base + t[1], base + t[2]]),
                    );
                }
                None => {
                    let mut i = 0u32;
                    while (i as usize) + 3 <= positions.len() {
                        triangles.push([base + i, base + i + 1, base + i + 2]);
                        i += 3;
                    }
                }
            }
        }
    }
    (!triangles.is_empty()).then_some(Mesh { vertices, triangles })
}

// ---------------------------------------------------------------------------
// The rasteriser
// ---------------------------------------------------------------------------

/// Not unit length, and it does not need to be: the shading below clamps its
/// result to `0.0..=1.0` regardless, and a light a few percent too strong or
/// weak is invisible next to the flat-shading it already is.
const LIGHT: [f32; 3] = [0.4, 0.6, 0.7];

/// Yaw, then pitch, in degrees, around a mesh centred on the origin.
///
/// A camera rather than a fixed angle now — see `mbrd_core::media::Orbit` for
/// where the app keeps one per mesh card. `yaw`/`pitch` are plain `f32`s
/// rather than `Orbit` itself so this crate does not have to depend on
/// `media`'s item-shaped code to draw a triangle; the app destructures an
/// `Orbit` at the call site instead.
fn rotate(v: [f32; 3], yaw: f32, pitch: f32) -> [f32; 3] {
    let (sy, cy) = yaw.to_radians().sin_cos();
    let (sp, cp) = pitch.to_radians().sin_cos();
    let [x, y, z] = v;
    let (x1, z1) = (x * cy + z * sy, z * cy - x * sy);
    let (y2, z2) = (y * cp - z1 * sp, y * sp + z1 * cp);
    [x1, y2, z2]
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = dot(v, v).sqrt();
    if len < 1e-9 {
        [0.0, 0.0, 0.0]
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

/// Twice the signed area of the triangle `(ax,ay) (bx,by) (cx,cy)` — the one
/// building block barycentric rasterisation is made of: called once for the
/// whole triangle and twice more per candidate pixel, and its sign is what
/// tells inside from outside regardless of which way the triangle winds.
fn edge(ax: f32, ay: f32, bx: f32, by: f32, cx: f32, cy: f32) -> f32 {
    (bx - ax) * (cy - ay) - (by - ay) * (cx - ax)
}

/// A mesh's vertices rotated into a camera's view, and the box that bounds
/// them on screen — the one pass over the vertex buffer that both [`aspect`]
/// and [`rasterize_view`] need.
///
/// Handed back as a value rather than recomputed because drawing a mesh asks
/// for the same rotation three times over otherwise: once to fit a canvas to
/// the silhouette, and once more inside each tier that is then drawn on it.
/// At a couple of hundred thousand vertices that is two whole passes and two
/// allocations spent to arrive back at a `Vec` that was already in hand —
/// which is affordable once for a still, and is not affordable per frame of a
/// drag that is turning the camera.
pub struct View {
    /// Every vertex of the mesh, rotated, under the same indices the mesh's
    /// own triangles already carry — so a triangle finds its corners here
    /// exactly as it would in `mesh.vertices`.
    rotated: Vec<[f32; 3]>,
    min: [f32; 2],
    max: [f32; 2],
}

impl View {
    /// How wide and how tall the silhouette is from here, in the mesh's own
    /// units. [`aspect`] is this and nothing else.
    pub fn aspect(&self) -> (f32, f32) {
        (self.max[0] - self.min[0], self.max[1] - self.min[1])
    }
}

/// A mesh rotated to `yaw`/`pitch` degrees, ready to be measured or drawn.
///
/// `None` for exactly the meshes [`rasterize`] declines — no triangles, more
/// than [`TRIANGLE_MAX`] of them, or one so flat every vertex projects to the
/// same point.
pub fn view(mesh: &Mesh, yaw: f32, pitch: f32) -> Option<View> {
    if mesh.triangles.is_empty() || mesh.triangles.len() > TRIANGLE_MAX {
        return None;
    }
    let rotated: Vec<[f32; 3]> = mesh.vertices.iter().map(|&v| rotate(v, yaw, pitch)).collect();
    let mut min = [f32::MAX, f32::MAX];
    let mut max = [f32::MIN, f32::MIN];
    for v in &rotated {
        min[0] = min[0].min(v[0]);
        min[1] = min[1].min(v[1]);
        max[0] = max[0].max(v[0]);
        max[1] = max[1].max(v[1]);
    }
    if max[0] - min[0] <= 1e-6 && max[1] - min[1] <= 1e-6 {
        return None;
    }
    Some(View { rotated, min, max })
}

/// How wide and how tall this mesh's silhouette is at this `yaw`/`pitch`, in
/// the mesh's own units — what the UI crate fits a canvas to before calling
/// [`rasterize`], the same way an SVG's own `viewBox` decides a canvas shape
/// before `resvg::render` is asked to fill it. `None` for exactly the meshes
/// [`rasterize`] would also decline.
///
/// A caller that is about to *draw* as well as measure wants [`view`] and to
/// keep what it hands back: this throws the rotated vertices away, and they
/// are the expensive half.
pub fn aspect(mesh: &Mesh, yaw: f32, pitch: f32) -> Option<(f32, f32)> {
    Some(view(mesh, yaw, pitch)?.aspect())
}

/// A mesh, flat-shaded onto a `width` × `height` canvas, viewed from `yaw`/
/// `pitch` degrees, pushed in or out by `zoom`, and recentred by `pan_x`/
/// `pan_y` — a multiplier on the fit, not a camera distance: below `1.0`
/// leaves more margin around the silhouette, above `1.0` fills more of the
/// canvas with it. `None` for a mesh with nothing to draw — no triangles,
/// more than [`TRIANGLE_MAX`] of them, or one so flat every vertex projects
/// to the same point — the same "say nothing rather than lie" choice
/// `crate::preview` makes for a picture that will not open.
///
/// Rendered at [`ANTIALIAS`] times the asked-for resolution and boxed back
/// down — see [`downsample`] — because a single sample per pixel draws a
/// silhouette edge as a hard, aliased step, which reads as "pixelated" the
/// moment a mesh card is any size worth looking at.
///
/// Rotates the mesh itself, so a caller drawing the same camera more than
/// once — two tiers of one picture, say — wants [`view`] and
/// [`rasterize_view`], which let that pass be paid for once.
#[allow(clippy::too_many_arguments)]
pub fn rasterize(
    mesh: &Mesh,
    width: u32,
    height: u32,
    yaw: f32,
    pitch: f32,
    zoom: f32,
    pan_x: f32,
    pan_y: f32,
) -> Option<Raster> {
    let view = view(mesh, yaw, pitch)?;
    rasterize_view(&view, mesh, width, height, zoom, pan_x, pan_y, ANTIALIAS)
}

/// How many samples per pixel on each axis [`rasterize`] renders at. See
/// [`rasterize_view`], which takes it as an argument instead.
pub const ANTIALIAS: u32 = 2;

/// [`rasterize`] against a [`View`] that has already been paid for, at a
/// chosen supersampling factor.
///
/// `ss` is what [`rasterize`] fixes at [`ANTIALIAS`]: the canvas is drawn
/// `ss` times larger on each axis and boxed back down, so `2` costs four
/// times the pixels of `1` and buys a soft silhouette edge instead of a hard
/// step. A mesh being turned under the pointer is worth `1` — an edge on a
/// moving object is not read, and the still that lands when the drag ends is
/// drawn at [`ANTIALIAS`] regardless. `0` is taken as `1`.
///
/// `None` if `view` was not built from `mesh`, which would otherwise index a
/// vertex that is not there: the same "say nothing rather than lie" answer
/// the rest of this module gives, rather than a panic on a mismatch a caller
/// holding two meshes can make.
#[allow(clippy::too_many_arguments)]
pub fn rasterize_view(
    view: &View,
    mesh: &Mesh,
    width: u32,
    height: u32,
    zoom: f32,
    pan_x: f32,
    pan_y: f32,
    ss: u32,
) -> Option<Raster> {
    if width == 0 || height == 0 || view.rotated.len() != mesh.vertices.len() {
        return None;
    }
    let ss = ss.max(1);
    let hi = raw_rasterize(view, mesh, width * ss, height * ss, zoom, pan_x, pan_y);
    // At one sample per pixel the box filter is the identity — every output
    // pixel has exactly one input, opaque where it was drawn on and clear
    // where it was not — so skip a full pass over the buffer to arrive at the
    // buffer.
    if ss == 1 {
        return Some(hi);
    }
    Some(downsample(&hi, width, height, ss))
}

/// [`rasterize_view`]'s own math, at whatever resolution it is asked to run
/// at — split out so the antialiasing wrapper can render this larger on each
/// axis without duplicating the projection or the raster loop.
fn raw_rasterize(
    view: &View,
    mesh: &Mesh,
    width: u32,
    height: u32,
    zoom: f32,
    pan_x: f32,
    pan_y: f32,
) -> Raster {
    let (rotated, min, max) = (&view.rotated, view.min, view.max);
    let (span_x, span_y) = (max[0] - min[0], max[1] - min[1]);

    // Contained rather than stretched, the same rule a picture is held to on
    // the open page: the whole silhouette fits inside the canvas, with a
    // margin, at whichever axis is tighter. `zoom` is clamped rather than
    // trusted — it arrives off a scroll gesture and a stray `NaN`/`inf` here
    // would turn every pixel of the canvas into the same triangle. `pan_x`/
    // `pan_y` get the same treatment, a shift of the look-at point rather
    // than trusted raw input.
    const MARGIN: f32 = 0.86;
    let zoom = if zoom.is_finite() { zoom.clamp(0.05, 20.0) } else { 1.0 };
    let sanitize_pan = |v: f32| if v.is_finite() { v.clamp(-3.0, 3.0) } else { 0.0 };
    let (pan_x, pan_y) = (sanitize_pan(pan_x), sanitize_pan(pan_y));
    let scale =
        zoom * MARGIN * (width as f32 / span_x.max(1e-6)).min(height as f32 / span_y.max(1e-6));
    let (cx, cy) =
        ((min[0] + max[0]) / 2.0 + pan_x * span_x, (min[1] + max[1]) / 2.0 + pan_y * span_y);
    let to_screen = |v: [f32; 3]| -> (f32, f32) {
        // Screen Y grows downward; the mesh's own Y grows upward.
        ((v[0] - cx) * scale + width as f32 / 2.0, height as f32 / 2.0 - (v[1] - cy) * scale)
    };

    let mut depth = vec![f32::NEG_INFINITY; (width * height) as usize];
    let mut rgba = vec![0_u8; (width * height) as usize * 4];

    for tri in &mesh.triangles {
        let corners = tri.map(|i| rotated[i as usize]);
        let [a, b, c] = corners;
        let normal = normalize(cross(sub(b, a), sub(c, a)));
        // The camera looks toward the mesh from `+Z` in rotated space; a
        // triangle facing away from it is a back face, and drawing it would
        // paint the inside of the mesh over whatever is genuinely visible.
        if normal[2] <= 0.0 {
            continue;
        }
        let shade = (0.32 + 0.68 * dot(normal, LIGHT).max(0.0)).clamp(0.0, 1.0);

        let (ax, ay) = to_screen(a);
        let (bx, by) = to_screen(b);
        let (ccx, ccy) = to_screen(c);
        let area = edge(ax, ay, bx, by, ccx, ccy);
        if area.abs() < 1e-6 {
            continue;
        }

        let min_x = ax.min(bx).min(ccx).floor().clamp(0.0, width as f32 - 1.0) as u32;
        let max_x = ax.max(bx).max(ccx).ceil().clamp(0.0, width as f32 - 1.0) as u32;
        let min_y = ay.min(by).min(ccy).floor().clamp(0.0, height as f32 - 1.0) as u32;
        let max_y = ay.max(by).max(ccy).ceil().clamp(0.0, height as f32 - 1.0) as u32;
        if min_x > max_x || min_y > max_y {
            continue;
        }

        for py in min_y..=max_y {
            for px in min_x..=max_x {
                let (fx, fy) = (px as f32 + 0.5, py as f32 + 0.5);
                let w0 = edge(bx, by, ccx, ccy, fx, fy) / area;
                let w1 = edge(ccx, ccy, ax, ay, fx, fy) / area;
                let w2 = edge(ax, ay, bx, by, fx, fy) / area;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let z = w0 * a[2] + w1 * b[2] + w2 * c[2];
                let at = (py * width + px) as usize;
                if z <= depth[at] {
                    continue;
                }
                depth[at] = z;
                let base = 205.0 * shade;
                let px4 = at * 4;
                rgba[px4] = base as u8;
                rgba[px4 + 1] = (base * 0.97) as u8;
                rgba[px4 + 2] = (base * 0.90) as u8;
                rgba[px4 + 3] = 255;
            }
        }
    }

    Raster { width, height, rgba }
}

/// A `ss`×-higher-resolution raster, boxed down to `width` × `height` — a
/// premultiplied-alpha-correct average, so a silhouette edge comes out as a
/// soft gradient of partial coverage rather than the hard on/off step
/// [`raw_rasterize`]'s single sample per pixel draws. Only covered
/// (`alpha > 0`) samples contribute their colour to the average; an output
/// pixel's own alpha is just how many of its samples were covered, which is
/// exactly what "this pixel is half on the mesh" means.
fn downsample(hi: &Raster, width: u32, height: u32, ss: u32) -> Raster {
    let mut rgba = vec![0_u8; (width * height) as usize * 4];
    let total = ss * ss;
    for y in 0..height {
        for x in 0..width {
            let (mut r, mut g, mut b, mut covered) = (0_u32, 0_u32, 0_u32, 0_u32);
            for dy in 0..ss {
                for dx in 0..ss {
                    let at = (((y * ss + dy) * hi.width + (x * ss + dx)) * 4) as usize;
                    if hi.rgba[at + 3] == 0 {
                        continue;
                    }
                    r += hi.rgba[at] as u32;
                    g += hi.rgba[at + 1] as u32;
                    b += hi.rgba[at + 2] as u32;
                    covered += 1;
                }
            }
            if covered == 0 {
                continue;
            }
            let out = ((y * width + x) * 4) as usize;
            rgba[out] = (r / covered) as u8;
            rgba[out + 1] = (g / covered) as u8;
            rgba[out + 2] = (b / covered) as u8;
            rgba[out + 3] = (covered * 255 / total) as u8;
        }
    }
    Raster { width, height, rgba }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unit cube centred on the origin — the fixture the rasteriser's plan
    /// names as the one to prove the rasteriser with. Twelve triangles, two
    /// per face, wound so each one's own cross product already points
    /// outward.
    fn cube() -> Mesh {
        let vertices = vec![
            [-1.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
            [1.0, 1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
            [1.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0],
        ];
        let faces: [[u32; 4]; 6] = [
            [0, 3, 2, 1], // -Z
            [4, 5, 6, 7], // +Z
            [0, 1, 5, 4], // -Y
            [3, 7, 6, 2], // +Y
            [0, 4, 7, 3], // -X
            [1, 2, 6, 5], // +X
        ];
        let mut triangles = Vec::new();
        for [a, b, c, d] in faces {
            triangles.push([a, b, c]);
            triangles.push([a, c, d]);
        }
        Mesh { vertices, triangles }
    }

    fn binary_stl(mesh: &Mesh) -> Vec<u8> {
        let mut out = vec![0_u8; 84];
        out[80..84].copy_from_slice(&(mesh.triangles.len() as u32).to_le_bytes());
        for tri in &mesh.triangles {
            out.extend_from_slice(&[0_u8; 12]); // a normal nothing here reads
            for &i in tri {
                for axis in mesh.vertices[i as usize] {
                    out.extend_from_slice(&axis.to_le_bytes());
                }
            }
            out.extend_from_slice(&[0_u8; 2]); // the attribute count
        }
        out
    }

    #[test]
    fn a_binary_stl_is_told_from_an_ascii_one_by_its_shape_not_its_header() {
        let bytes = binary_stl(&cube());
        assert!(is_stl(&bytes));
        assert_eq!(triangle_count(&bytes), Some(12));

        // A courteous binary exporter's `solid` prefix does not fool the
        // check, because this one is not looking at the header text.
        let mut disguised = bytes.clone();
        disguised[0..5].copy_from_slice(b"solid");
        assert!(is_stl(&disguised));

        // An ASCII STL, or anything else, does not have this shape by
        // accident.
        assert!(!is_stl(b"solid cube\nfacet normal 0 0 0\nendfacet\nendsolid\n"));
        assert!(!is_stl(b"too short"));
    }

    #[test]
    fn a_file_that_declares_four_billion_triangles_is_answered_rather_than_believed() {
        // Every import asks this question of every file, so the four bytes at
        // offset 80 are whatever somebody's prose happened to put there. On a
        // 32-bit target — which the web build is — multiplying such a count by
        // fifty in `usize` overflows: it panicked mid-drop and took the page
        // with it. The answer is `false`, and getting an answer at all is the
        // point of the test.
        let mut text =
            b"# a note long enough to reach offset eighty, which most notes are".to_vec();
        text.resize(200, b'.');
        text[80..84].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(!is_stl(&text));
        assert_eq!(triangle_count(&text), Some(u32::MAX));
    }

    #[test]
    fn a_binary_stls_triangles_round_trip_through_their_own_bytes() {
        let mesh = cube();
        let bytes = binary_stl(&mesh);
        let parsed = stl(&bytes).expect("a well-formed binary STL");
        assert_eq!(parsed.triangles.len(), 12);
        assert_eq!(parsed.vertices.len(), 36, "STL shares no vertices between triangles");
        // The first facet's first corner is the cube's own first vertex.
        assert_eq!(parsed.vertices[0], mesh.vertices[0]);
    }

    /// The angle the fixed view used before the camera turned — kept as a
    /// fixture rather than a public constant, since nothing in the app needs
    /// this exact angle any more.
    const DEFAULT: (f32, f32, f32) = (-35.0, -25.0, 1.0);

    #[test]
    fn a_cube_rasterises_to_a_lit_silhouette_on_an_empty_ground() {
        let (yaw, pitch, zoom) = DEFAULT;
        let raster = rasterize(&cube(), 64, 64, yaw, pitch, zoom, 0.0, 0.0)
            .expect("a cube has triangles to draw");
        assert_eq!(raster.rgba.len(), 64 * 64 * 4);

        let alpha_at = |x: u32, y: u32| raster.rgba[((y * 64 + x) * 4 + 3) as usize];
        assert_eq!(alpha_at(0, 0), 0, "a corner of the canvas is background, not cube");
        assert_eq!(alpha_at(32, 32), 255, "the centre of the canvas is the cube");

        // Two faces catch the fixed light differently, which is the whole
        // point of shading being per-triangle rather than a single flat tint
        // over the entire silhouette.
        let lit: std::collections::HashSet<u8> = (0..64)
            .flat_map(|y| (0..64).map(move |x| (x, y)))
            .filter(|&(x, y)| alpha_at(x, y) == 255)
            .map(|(x, y)| raster.rgba[((y * 64 + x) * 4) as usize])
            .collect();
        assert!(lit.len() > 1, "every lit pixel came out the same shade: {lit:?}");
    }

    #[test]
    fn past_the_triangle_cap_a_mesh_is_not_rasterised() {
        let mut mesh = cube();
        mesh.triangles = vec![[0, 1, 2]; TRIANGLE_MAX + 1];
        let (yaw, pitch, zoom) = DEFAULT;
        assert!(rasterize(&mesh, 64, 64, yaw, pitch, zoom, 0.0, 0.0).is_none());
    }

    #[test]
    fn a_mesh_with_no_triangles_is_not_rasterised() {
        let (yaw, pitch, zoom) = DEFAULT;
        assert!(rasterize(
            &Mesh { vertices: vec![], triangles: vec![] },
            64,
            64,
            yaw,
            pitch,
            zoom,
            0.0,
            0.0
        )
        .is_none());
    }

    #[test]
    fn turning_the_camera_changes_what_is_drawn() {
        let (_, pitch, zoom) = DEFAULT;
        // Not a multiple of 90 degrees either side: a cube is symmetric under
        // a quarter turn, and that symmetry — not the camera parameter being
        // ignored — is what would make two such views identical.
        let a = rasterize(&cube(), 64, 64, 0.0, pitch, zoom, 0.0, 0.0)
            .expect("a cube has triangles to draw");
        let b = rasterize(&cube(), 64, 64, 40.0, pitch, zoom, 0.0, 0.0)
            .expect("a cube has triangles to draw");
        assert_ne!(a.rgba, b.rgba, "a turn of a cube looks different");
    }

    #[test]
    fn zooming_in_grows_the_silhouette_and_zooming_out_shrinks_it() {
        let (yaw, pitch, _) = DEFAULT;
        let opaque_pixels =
            |raster: &Raster| raster.rgba.as_chunks::<4>().0.iter().filter(|p| p[3] == 255).count();

        let far = rasterize(&cube(), 64, 64, yaw, pitch, 0.4, 0.0, 0.0).unwrap();
        let normal = rasterize(&cube(), 64, 64, yaw, pitch, 1.0, 0.0, 0.0).unwrap();
        let near = rasterize(&cube(), 64, 64, yaw, pitch, 2.0, 0.0, 0.0).unwrap();

        assert!(opaque_pixels(&far) < opaque_pixels(&normal), "zoomed out should draw smaller");
        assert!(opaque_pixels(&normal) < opaque_pixels(&near), "zoomed in should draw bigger");
    }

    #[test]
    fn a_camera_at_the_orbit_limits_still_rasterises_without_garbage() {
        // `media::PITCH_LIMIT`/`DIST_MIN`/`DIST_MAX` translate here to pitch
        // near +/-90 and zoom far from 1.0 — the edges of what the app will
        // ever actually pass in, and the one place this file can check it
        // without depending on `media` to get the numbers.
        for (yaw, pitch, zoom) in [(0.0, 89.0, 12.0), (0.0, -89.0, 0.05), (359.0, 0.0, 20.0)] {
            let raster =
                rasterize(&cube(), 32, 32, yaw, pitch, zoom, 0.0, 0.0).expect("still a cube");
            assert!(
                raster.rgba.iter().any(|&b| b != 0),
                "the limits should not rasterise to nothing"
            );
        }
        // A non-finite zoom is not trusted either — see `rasterize`'s own clamp.
        assert!(rasterize(&cube(), 32, 32, 0.0, 0.0, f32::NAN, 0.0, 0.0).is_some());
    }

    #[test]
    fn aspect_and_rasterize_agree_at_every_camera() {
        for (yaw, pitch, zoom) in [(-35.0, -25.0, 1.0), (10.0, 80.0, 3.0), (200.0, -40.0, 0.2)] {
            let (span_w, span_h) = aspect(&cube(), yaw, pitch).expect("a cube has an extent");
            assert!(span_w > 0.0 && span_h > 0.0);
            assert!(rasterize(&cube(), 64, 64, yaw, pitch, zoom, 0.0, 0.0).is_some());
        }
    }

    #[test]
    fn panning_shifts_the_silhouette_without_losing_it() {
        let (yaw, pitch, zoom) = DEFAULT;
        let centred = rasterize(&cube(), 64, 64, yaw, pitch, zoom, 0.0, 0.0).unwrap();
        let panned = rasterize(&cube(), 64, 64, yaw, pitch, zoom, 0.6, -0.4).unwrap();
        assert_ne!(centred.rgba, panned.rgba, "a pan should move the silhouette");
        assert!(
            panned.rgba.iter().any(|&b| b != 0),
            "a modest pan should not push the cube off-canvas"
        );

        // Not trusted any more than zoom is: garbage input leaves the frame
        // centred rather than producing a canvas of the same triangle.
        let garbage =
            rasterize(&cube(), 64, 64, yaw, pitch, zoom, f32::NAN, f32::INFINITY).unwrap();
        assert_eq!(garbage.rgba, centred.rgba);
    }

    #[test]
    fn a_silhouette_edge_is_softened_rather_than_a_hard_step() {
        // A single-sample rasteriser draws every covered pixel at alpha 255
        // and every uncovered one at 0 — nothing in between. Antialiasing's
        // whole job is to put something in between at the boundary.
        let (yaw, pitch, zoom) = DEFAULT;
        let raster = rasterize(&cube(), 64, 64, yaw, pitch, zoom, 0.0, 0.0).unwrap();
        let alphas: std::collections::HashSet<u8> =
            raster.rgba.as_chunks::<4>().0.iter().map(|p| p[3]).collect();
        assert!(
            alphas.iter().any(|&a| a != 0 && a != 255),
            "no partially-covered edge pixel was found: {alphas:?}"
        );
    }

    #[test]
    fn the_shared_view_path_draws_exactly_what_the_all_in_one_path_draws() {
        // The whole safety net for rotating the vertices once and spending
        // that on two tiers: taking the pass out from under `rasterize` is
        // only allowed to be faster, never to be different. Byte-for-byte,
        // because "close enough" on a rasteriser is how a silhouette quietly
        // moves half a pixel.
        for (yaw, pitch, zoom) in [(-35.0, -25.0, 1.0), (10.0, 80.0, 3.0), (200.0, -40.0, 0.2)] {
            for (pan_x, pan_y) in [(0.0, 0.0), (0.6, -0.4)] {
                let mesh = cube();
                let whole = rasterize(&mesh, 64, 48, yaw, pitch, zoom, pan_x, pan_y).unwrap();
                let view = view(&mesh, yaw, pitch).expect("a cube has an extent");
                let shared =
                    rasterize_view(&view, &mesh, 64, 48, zoom, pan_x, pan_y, ANTIALIAS).unwrap();
                assert_eq!(
                    whole.rgba, shared.rgba,
                    "the two paths disagree at {yaw}/{pitch}/{zoom} panned {pan_x}/{pan_y}"
                );
                assert_eq!((shared.width, shared.height), (64, 48));
            }
        }
    }

    #[test]
    fn one_sample_per_pixel_draws_the_same_cube_with_a_hard_edge() {
        // What a mesh under the pointer is drawn at. It still has to be the
        // cube — same canvas, same silhouette in the middle — and the only
        // thing it gives up is the partial coverage at the boundary that
        // `a_silhouette_edge_is_softened_rather_than_a_hard_step` asserts is
        // there at `ANTIALIAS`.
        let (yaw, pitch, zoom) = DEFAULT;
        let mesh = cube();
        let view = view(&mesh, yaw, pitch).expect("a cube has an extent");
        let single = rasterize_view(&view, &mesh, 64, 64, zoom, 0.0, 0.0, 1).unwrap();
        assert_eq!((single.width, single.height), (64, 64));
        assert_eq!(single.rgba.len(), 64 * 64 * 4);

        let alphas: std::collections::HashSet<u8> =
            single.rgba.as_chunks::<4>().0.iter().map(|p| p[3]).collect();
        assert_eq!(
            alphas,
            std::collections::HashSet::from([0, 255]),
            "one sample per pixel is covered or it is not: {alphas:?}"
        );
        let alpha_at = |x: u32, y: u32| single.rgba[((y * 64 + x) * 4 + 3) as usize];
        assert_eq!(alpha_at(32, 32), 255, "the centre of the canvas is still the cube");
        assert_eq!(alpha_at(0, 0), 0, "a corner of the canvas is still background");

        // Zero is not a resolution anybody meant, and it must not be a
        // divide-by-nothing either.
        assert!(rasterize_view(&view, &mesh, 64, 64, zoom, 0.0, 0.0, 0).is_some());
    }

    #[test]
    fn a_view_of_one_mesh_will_not_be_drawn_onto_another() {
        // `View` carries the rotated vertices and the triangles are read off
        // the mesh, so a caller holding two meshes could index a vertex that
        // is not there. A cube and its own STL round-trip are the same shape
        // at 8 vertices and at 36 — exactly the mismatch that would panic.
        let (yaw, pitch, zoom) = DEFAULT;
        let welded = cube();
        let unwelded = stl(&binary_stl(&welded)).expect("a well-formed binary STL");
        assert_ne!(welded.vertices.len(), unwelded.vertices.len());

        let view = view(&welded, yaw, pitch).expect("a cube has an extent");
        assert!(rasterize_view(&view, &unwelded, 64, 64, zoom, 0.0, 0.0, ANTIALIAS).is_none());
        assert!(rasterize_view(&view, &welded, 64, 64, zoom, 0.0, 0.0, ANTIALIAS).is_some());
    }

    /// A bumpy heightfield of roughly `(side - 1)² × 2` triangles — enough
    /// geometry to time against, built rather than loaded so no fixture file
    /// has to exist. Bumpy rather than flat on purpose: a plane's triangles
    /// are all wound the same way and all shaded the same, which is neither
    /// what a real mesh costs to cull nor what it costs to shade.
    fn heightfield(side: u32) -> Mesh {
        let n = side.max(2);
        let mut vertices = Vec::with_capacity((n * n) as usize);
        for y in 0..n {
            for x in 0..n {
                let fx = x as f32 / (n - 1) as f32 * 2.0 - 1.0;
                let fy = y as f32 / (n - 1) as f32 * 2.0 - 1.0;
                vertices.push([fx, fy, (fx * 3.0).sin() * (fy * 3.0).cos() * 0.35]);
            }
        }
        let mut triangles = Vec::with_capacity(((n - 1) * (n - 1) * 2) as usize);
        for y in 0..n - 1 {
            for x in 0..n - 1 {
                let a = y * n + x;
                triangles.push([a, a + 1, a + n]);
                triangles.push([a + 1, a + n + 1, a + n]);
            }
        }
        Mesh { vertices, triangles }
    }

    #[test]
    #[ignore = "a measurement rather than an assertion: cargo test -p mbrd-core --release -- --ignored --nocapture"]
    fn what_one_frame_of_a_turning_mesh_costs() {
        let (yaw, pitch, zoom) = DEFAULT;
        const REPS: u32 = 5;

        // An ordinary mesh, and one at the cap — the second is what decides
        // whether this rasteriser is ever worth replacing with a GPU, since
        // that is the size where a CPU one has the most to lose.
        for side in [160, 317] {
            let mesh = heightfield(side);

            // What one mouse move used to cost: a measuring pass that threw
            // its rotation away, then both tiers — each rotating the mesh
            // again, each supersampled — of which the board showed one.
            let before = std::time::Instant::now();
            for _ in 0..REPS {
                aspect(&mesh, yaw, pitch).unwrap();
                rasterize(&mesh, 256, 256, yaw, pitch, zoom, 0.0, 0.0).unwrap();
                rasterize(&mesh, 1024, 1024, yaw, pitch, zoom, 0.0, 0.0).unwrap();
            }
            let before = before.elapsed() / REPS;

            // What it costs now: one rotation, the one tier that is on
            // screen, one sample per pixel.
            let after = std::time::Instant::now();
            for _ in 0..REPS {
                let view = view(&mesh, yaw, pitch).unwrap();
                rasterize_view(&view, &mesh, 256, 256, zoom, 0.0, 0.0, 1).unwrap();
            }
            let after = after.elapsed() / REPS;

            println!(
                "{} triangles, per frame: {before:?} before, {after:?} after — {:.1}x",
                mesh.triangles.len(),
                before.as_secs_f64() / after.as_secs_f64().max(f64::EPSILON),
            );
        }
    }

    #[test]
    fn a_meshs_silhouette_is_measured_before_anything_is_drawn_on_it() {
        let (yaw, pitch, _) = DEFAULT;
        let (w, h) = aspect(&cube(), yaw, pitch).expect("a cube has an extent");
        assert!(w > 0.0 && h > 0.0);
        // A cube looks the same from every axis, so this particular
        // three-quarter view — neither a straight-on face nor a corner-first
        // diagonal — puts its silhouette within shouting distance of square,
        // not stretched three-to-one along either axis.
        assert!((0.5..2.0).contains(&(w / h)), "w={w} h={h}");
    }

    // -----------------------------------------------------------------------
    // Wavefront OBJ
    // -----------------------------------------------------------------------

    const SQUARE_OBJ: &str = "\
# a unit square, one quad face
v 0 0 0
v 1 0 0
v 1 1 0
v 0 1 0
f 1 2 3 4
";

    #[test]
    fn is_obj_recognises_a_real_obj_and_rejects_plain_text() {
        assert!(is_obj(SQUARE_OBJ.as_bytes()));
        assert!(!is_obj(b"just a note about version 2, filed under f minor"));
        assert!(!is_obj(&[0xff, 0xd8, 0xff, 0xe0]), "binary bytes are not even UTF-8 here");
    }

    #[test]
    fn a_high_poly_obj_is_still_recognised_when_its_faces_start_past_line_two_hundred() {
        let mut text = String::new();
        for i in 0..500 {
            text.push_str(&format!("v 0 0 {i}\n"));
        }
        text.push_str("f 1 2 3\n");
        assert!(
            is_obj(text.as_bytes()),
            "500 vertices before the first face is a normal mesh, not text"
        );
    }

    #[test]
    fn an_obj_fans_a_quad_into_two_triangles_sharing_its_first_corner() {
        let mesh = obj(SQUARE_OBJ.as_bytes()).expect("four vertices and one face");
        assert_eq!(mesh.vertices.len(), 4);
        assert_eq!(mesh.triangles, vec![[0, 1, 2], [0, 2, 3]]);
    }

    #[test]
    fn an_obj_honours_negative_indices_counting_back_from_the_newest_vertex() {
        let text = "v 0 0 0\nv 1 0 0\nv 1 1 0\nf -3 -2 -1\n";
        let mesh = obj(text.as_bytes()).expect("three vertices and one face");
        assert_eq!(mesh.triangles, vec![[0, 1, 2]]);
    }

    #[test]
    fn a_face_corner_outside_the_vertices_read_so_far_is_dropped_not_panicked_on() {
        // `f` names a fourth corner before a fourth vertex exists — the kind
        // of file a half-written export or a hand edit produces. Three good
        // corners are still a triangle rather than nothing.
        let text = "v 0 0 0\nv 1 0 0\nv 1 1 0\nf 1 2 3 4\n";
        let mesh = obj(text.as_bytes()).expect("three good corners are still a face");
        assert_eq!(mesh.vertices.len(), 3);
        assert_eq!(mesh.triangles, vec![[0, 1, 2]]);
    }

    #[test]
    fn an_obj_with_no_face_worth_drawing_is_no_mesh_at_all() {
        assert!(obj(b"v 0 0 0\nv 1 0 0\nv 1 1 0\n").is_none(), "vertices with no face");
        assert!(obj(b"# nothing here but a comment\n").is_none());
    }

    // -----------------------------------------------------------------------
    // glTF binary (.glb)
    // -----------------------------------------------------------------------

    fn glb_bytes(json: &str, bin: &[u8]) -> Vec<u8> {
        let json = json.as_bytes();
        let total = 12 + 8 + json.len() + 8 + bin.len();
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(json.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(json);
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(bin);
        out
    }

    fn positions_bin(verts: &[[f32; 3]]) -> Vec<u8> {
        let mut bin = Vec::new();
        for v in verts {
            for axis in v {
                bin.extend_from_slice(&axis.to_le_bytes());
            }
        }
        bin
    }

    #[test]
    fn is_glb_recognises_the_magic_and_rejects_anything_else() {
        let bytes = glb_bytes(r#"{"bufferViews":[],"accessors":[],"meshes":[]}"#, &[]);
        assert!(is_glb(&bytes));
        assert!(!is_glb(b"glT"), "too short for even the header");
        assert!(!is_glb(SQUARE_OBJ.as_bytes()));
    }

    #[test]
    fn a_glb_with_no_indices_treats_every_three_positions_as_a_triangle() {
        let verts = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let bin = positions_bin(&verts);
        let json = format!(
            r#"{{"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":{}}}],
                 "accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}}],
                 "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}}}}]}}]}}"#,
            bin.len(),
        );
        let bytes = glb_bytes(&json, &bin);
        let mesh = glb(&bytes).expect("a well-formed unindexed glb");
        assert_eq!(mesh.vertices, verts);
        assert_eq!(mesh.triangles, vec![[0, 1, 2]]);
    }

    #[test]
    fn a_glbs_indexed_primitive_reuses_vertices_across_triangles() {
        let verts = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
        let mut bin = positions_bin(&verts);
        let pos_len = bin.len();
        let idx_offset = bin.len();
        let indices: [u16; 6] = [0, 1, 2, 0, 2, 3];
        for i in indices {
            bin.extend_from_slice(&i.to_le_bytes());
        }
        let json = format!(
            r#"{{"bufferViews":[
                    {{"buffer":0,"byteOffset":0,"byteLength":{pos_len}}},
                    {{"buffer":0,"byteOffset":{idx_offset},"byteLength":12}}
                 ],
                 "accessors":[
                    {{"bufferView":0,"componentType":5126,"count":4,"type":"VEC3"}},
                    {{"bufferView":1,"componentType":5123,"count":6,"type":"SCALAR"}}
                 ],
                 "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}},"indices":1}}]}}]}}"#,
        );
        let bytes = glb_bytes(&json, &bin);
        let mesh = glb(&bytes).expect("a well-formed indexed glb");
        assert_eq!(mesh.vertices, verts);
        assert_eq!(mesh.triangles, vec![[0, 1, 2], [0, 2, 3]]);
    }

    #[test]
    fn two_meshes_in_one_glb_share_one_vertex_buffer_offset_to_match() {
        let verts = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [2.0, 0.0, 0.0]];
        let bin = positions_bin(&verts);
        let json = r#"{"bufferViews":[
                    {"buffer":0,"byteOffset":0,"byteLength":36},
                    {"buffer":0,"byteOffset":36,"byteLength":36}
                 ],
                 "accessors":[
                    {"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"},
                    {"bufferView":1,"componentType":5126,"count":1,"type":"VEC3"}
                 ],
                 "meshes":[
                    {"primitives":[{"attributes":{"POSITION":0}}]},
                    {"primitives":[{"attributes":{"POSITION":1}}]}
                 ]}"#;
        // The second mesh has only one vertex — not a triangle by itself —
        // so what proves the offset is real is that the first mesh still
        // rasterises correctly with the second one's vertex sitting after it
        // in the concatenated buffer.
        let bytes = glb_bytes(json, &bin);
        let mesh = glb(&bytes).expect("the first mesh alone is a triangle");
        assert_eq!(mesh.vertices.len(), 4);
        assert_eq!(mesh.triangles, vec![[0, 1, 2]]);
        assert_eq!(mesh.vertices[3], verts[3], "the second mesh's vertex still landed");
    }

    #[test]
    fn a_primitive_whose_mode_is_not_triangles_is_skipped() {
        let verts = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]];
        let bin = positions_bin(&verts);
        let json = format!(
            r#"{{"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":{}}}],
                 "accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}}],
                 "meshes":[{{"primitives":[{{"mode":0,"attributes":{{"POSITION":0}}}}]}}]}}"#,
            bin.len(),
        );
        let bytes = glb_bytes(&json, &bin);
        assert!(glb(&bytes).is_none(), "POINTS is not a shape this rasteriser draws");
    }

    #[test]
    fn a_glb_missing_its_bin_chunk_or_its_json_is_not_a_mesh() {
        assert!(glb(b"glTF\x02\0\0\0\x0c\0\0\0").is_none(), "a header with no chunks after it");
        assert!(!is_glb(b"PK\x03\x04mbrd0000"), "a different magic number, of the same length");
    }
}
