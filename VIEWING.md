# Opening, previewing and editing

What happens when somebody double-clicks a card, and how the app decides what
to show them.

This is a plan and a specification, in the order the work depends on itself.
`ROADMAP.md` is the port; this is one piece of it, written down separately
because it crosses the format, the dependency list and three crates, and
because several of its six tranches are large enough to span more than one
sitting.

---

## The premise

**A card is a thumbnail of something.** It is a few hundred pixels wide, it is
one of possibly twenty thousand, and it is drawn in one painted layer for
exactly that reason. So there has to be somewhere that shows the *thing*, and a
double-click is the gesture every person who has used a computer already knows
means "show me this".

Three rules follow, and everything below is one of them being kept:

1. **The gesture works on everything.** A double-click that opens a note and
   does nothing to a `.zip` is a gesture nobody trusts. A zip opens too — onto
   what is genuinely known about it, which is a name, a size, a hash and a list
   of what is inside.
2. **Anything that can be shown is shown.** If the bytes can be turned into a
   picture, a page, a waveform or a list, they are. "We have the bytes and drew
   a grey rectangle" is a bug, not a level of detail.
3. **Anything that can be changed is changeable.** Every card has at least a
   name. Most have more. The Edit button is never greyed out; what it opens is
   whatever the card turned out to have.

---

## The shape

One page, one header, one body, one rail.

```
┌ icon  title                                  [Edit] [i] [×] ┐
│ type · 2.4 MB · 1920×1080                                   │
├──────────────────────────────────────────┬──────────────────┤
│                                          │                  │
│   PREVIEW   or   SOURCE                  │   INFO           │
│                                          │   (toggled)      │
│                                          │                  │
└──────────────────────────────────────────┴──────────────────┘
```

The header is identical for every type — that is the point of it. The rail is
one panel with one implementation and one button, and it slides in beside the
preview rather than replacing it: looking up a photograph's dimensions should
not mean not looking at the photograph.

`Escape` closes the page. `Escape` while typing puts the words back, the same
bargain the board makes.

---

## Where the decisions live

Three pure functions in `mbrd-core`, so that "what can this card do" is
arithmetic over an `Item` and its bytes rather than a match arm buried in a
renderer. The crate's layering rule already says this: the answer is testable
without a window, so it is testable without a window.

```rust
core::preview::of(item, asset) -> Preview
core::preview::editable(item, asset) -> Vec<Editable>
core::facts::of(item, doc) -> Vec<Fact>
```

```rust
pub enum Preview {
    Document,                              // Markdown, set as a page
    Source { language: Option<&str> },     // text in a fixed-width face
    Sheet { separator: char },             // CSV and TSV, as a real grid
    Picture,                               // raster, contained
    Video,                                 // a poster today; a player later
    Audio,                                 // a waveform today; a player later
    Colour,                                // a swatch
    Address,                               // a link
    Archive,                               // a listing
    Nothing,                               // the rail is the whole page
}

pub enum Editable {
    Text { limit: usize },
    Hex,                  // a swatch
    Url,                  // a link
    Name,                 // every card, always
}
```

**Done.** `Preview::Vector` exists now, alongside `resvg` — the enum's own rule
held: the variant did not go in until `app/images.rs` had a `render` call to
back it, so it was never a lie the compiler could not catch. An SVG asset
rasterises to both tiers exactly like a raster picture, drawn by the same
`picture()` in `opened.rs` and requested through the same `images::decode`
board cards already asked for by content hash — no separate wiring needed on
either side once `decode()` recognised the bytes. Its XML source stays
editable too (`writable` treats `Vector` like `Source`), so looking at the
picture and reading the markup are both still one Edit button away. See "A
mesh is a rasterisation problem" below for why this reads almost the same
sentence twice — it is the same idea landing in the other content type it
applies to.

`editable` returns a **list**, **principal first**, and that is what makes rule
3 real. Every card yields at least `Name`. A note yields `Text + Name`, a link
`Url + Name`, a swatch exactly `Hex` — its colour and its name are one value in
this format, so it has one field and calling that field "Name" would hide it.
The page's Edit button starts on `[0]`; the rail's rows reach the rest. The
button is never dead.

A **double-click on the shown text** opens the same field the button would, and
only where the button would land on `Text` — the two ask `opened_principal` the
same question, so they cannot disagree. That is the second half of rule 1: a
double-click on the board shows you the card, and a double-click on what it
showed you lets you change it. It is off on a picture, a mesh, a waveform and a
swatch, where there is nothing under the pointer that looks like words, and off
on a PDF or an archive, where what is drawn is extracted rather than the file.

No caret is placed from where that press landed. What was under it was the
rendered markdown, which carries no byte offsets and is about to be replaced by
the source anyway; a long field opens with the caret at its end. Once the source
is up, its own click ladder — one to place, two for the word, three for the
line, four for the lot — has real rows to measure against.

`app/opened.rs` is then only a renderer. It holds no opinion about what a `.mp4`
is.

---

## One text, one door

A note's words and a text file's bytes are the same thing seen twice, and this
page still special-cases the difference — `opened::words_of` prefers the asset
and falls back to `meta.text`, and `BoardView::write_file` is a second path
beside `write_field`. That special-casing turns out to be exactly the right
shape rather than a stopgap: an asset-backed note *should* be read from its
asset and a bare one *should* just be its words, and both already agree on what
`meta.text` means once the migration below closed the one case where they
did not. `core::text::of`/`core::text::write` — one accessor hiding the branch
in a single place instead of three — would be a tidiness win, not a
correctness one, and is left undone on that basis.

### Notes become Markdown files — a format change — **done, the load-bearing half**

**Settled: `notes/<slug>--<id>.md` outranks `board.json` on load, and no
longer clips what it finds.** That second half was the actual bug: a note with
no asset had `meta.text` as its *only* copy of its words, capped at `NOTE_MAX`
(512 characters) everywhere it was written — including by hand, in the
sidecar. Grow a note past that by editing the `.md` directly, the one gesture
the format's "the archive is legible" promise invites, and the next open threw
away everything past the 512th character. Silently: the file that caused it
still had the full text sitting right there.

`mbrd::read` now promotes rather than clips (`mbrd.rs`, in the loop that reads
`notes/` back): a note with no asset whose sidecar is longer than `NOTE_MAX` is
given one — the full text, hashed and stored under `assets/` exactly like a
dropped `.md` file already was — and `meta.text` steps down to being the
derived head every asset-backed note already treats it as. This is the
migration `on load ... this runs once, per note, on the way in` below always
meant; it just turned out to be triggered by growth past the cap rather than by
a separate "has no asset yet" flag, because those are the same event for a note
that starts with none. Covered by
`mbrd::tests::a_hand_edited_note_that_grows_past_note_max_is_promoted_not_clipped`.

A note that came from a dropped file was **never** the bug — see the module
note at the top of `app/opened.rs` — because it already got an asset at import
time. What was missing was the symmetrical case: a note that *started* as pure
`meta.text` and grew past the cap by hand.

**What this does not change.** A note typed inside the app is still bound by
`NOTE_MAX` while it is being typed — the editor's own ceiling, unrelated to the
reader — because that is a UI limit on what one editing session commits, not a
format limit on what the archive may hold. Only hand-editing the sidecar (or,
later, an in-app affordance that writes through the asset path the way
`write_file` already does for a dropped file) grows a note past it. And the
original mbrd still reads `meta.text` and knows nothing about `notes/` as an
authority, so a note promoted this way still shows the original only its head
if the board is later opened there — the interop break this section always
said it was taking on.

---

## The media stack — a real one — **done**

**Settled: decode and play, both.** `ROADMAP.md`'s decision 2 chose poster
frames only; it was revisited, because a moodboard where the video cards are
coloured rectangles and the audio cards are silent is a moodboard missing two
of its four content types. See `ROADMAP.md` Phase 7 for what was built.

**What this section got wrong is worth keeping.** It planned the two halves as
two jobs with two dependency stories — `symphonia` plus `cpal` for audio,
because that is pure Rust and finishable; GStreamer or FFmpeg for video,
because that is not — and sequenced them so the cheap half landed first.

Neither of those turned out to be the shape of the work:

- **They are one job, not two.** A `playbin3` decodes an MP3 as readily as an
  MP4; so does an `AVPlayer`, so does the Media Engine. Doing audio through a
  second stack would have been a second stack for no second capability, and a
  second set of bugs about who owns the playhead. Splitting the halves would
  have *added* work rather than deferred it.
- **The dependency was the platform's, not the tree's.** The premise here was
  that a native decoder means shipping a native decoder. It does not: only
  Linux needed a package, because macOS and Windows have one in the OS.
  `pipeline_mac.rs` and `pipeline_win.rs` are more code than a `cpal` stream
  would have been, and they cost the installers nothing at all.
- **The poster path was not a prerequisite.** It was listed as needed first,
  since "a card is a still until it is pressed". A card that is playing is not
  a still, and one that is not playing already had somewhere to draw. Poster
  extraction on import is still unwritten, and nothing waited for it.

The one thing this section did call correctly is that video is the half that
changes what shipping the app means — it just lands on Linux alone.

---

## What each file type does

Marked by where it stands, not by how it would look in a list.

| Family | Extensions | Preview | Editable | Where it stands |
| --- | --- | --- | --- | --- |
| Markdown | `md` `markdown` `mdown` `mkd` `mdx` | document ⇄ source | text | **done** |
| Plain text | `txt` `text` `rst` `org` `log` … | numbered source | text | **done** |
| Structured text | `json` `toml` `yaml` `xml` `ini` | numbered source | text | **done** |
| Delimited text | `csv` `tsv` | a real table | text | **done** |
| Source code | `rs` `ts` `js` `py` `go` `c` `h` … | numbered source | text | **done** — highlighting is its own job, later |
| Anything else that reads as UTF-8 | — | numbered source | text | **done** — the extension list is a *label*, the bytes are the test |
| Raster images | `png` `jpg` `gif` `webp` `bmp` `tiff` `ico` `tga` `qoi` `exr` `hdr` | contained | name | **done** |
| Images claimed and undecodable | `avif` `heic` `heif` `jxl` | — | name | **done** — reclassified, see below |
| Vector | `svg` | contained, rasterised by `resvg` | text, its XML still edits it | **done** |
| Audio | `mp3` `wav` `flac` `ogg` `m4a` `aac` `opus` | waveform where the archive has one, and it **plays** | name | **done** — the system's decoder, so the list is a label and the machine is the test |
| Video | `mp4` `mov` `webm` `mkv` … | poster where the board has one, and it **plays** | name | **done** — same, drawn in front of the poster |
| PDF | `pdf` | its text, and how many pages | name | **done** — no rasteriser, which stays a native dependency and not the price of entry |
| Fonts | `ttf` `otf` `ttc` | a specimen sheet, set in the face itself | name | **done** |
| Fonts | `woff` `woff2` | — | name | later — a compressed wrapper around the shape above, and unwrapping one is a second dependency |
| Archives | `zip` | the entry listing, with sizes | name | **done** |
| Archives | `tar` `gz` `tgz` | the entry listing | name | **done** |
| Archives | `7z` `rar` `xz` `zst` | listing | name | later, one crate each |
| ZIPs wearing another name | `epub` `docx` `xlsx` `pptx` `sketch` `3mf` `usdz` | the entry listing | name | **done** — the extension check now asks the bytes |
| Design files | `psd` `ai` `sketch` `indd` | the preview they already carry | name | tranche 4 — see *Previews that are already in the file* |
| Camera raw | `cr2` `nef` `arw` `dng` `raf` | the full-size JPEG they already carry | name | tranche 4 — same path |
| 3D, as text | `gltf` `dae` `step` and the ASCII forms of `stl` `ply` | numbered source, and editable | text | **done, without anybody deciding it** — see below |
| 3D, as a shape | `stl` (binary) `obj` `glb` | a shaded picture, dragged to orbit and scrolled to zoom | name | **done** |
| 3D, binary and closed | `fbx` `blend` | rail only | name | not planned |
| Swatch | — | the colour | hex | **done** |
| Link | — | the address, and a button that opens it | url | **done** |
| Anything else | — | rail only | name | **done** |

The row worth pointing at is the sixth. `preview::of` ends its text arm with a
check of the *bytes* — valid UTF-8, and no NUL byte — rather than a lookup in
the extension table. So a `.xyzzy` full of words is shown as words, and the
table above is a list of things that get a **name** on the label rather than a
list of things that can be opened at all.

### The four image types that were a bug

`import.rs` used to classify `avif`, `heic`, `heif` and `jxl` as
`ItemType::Image`. None of them can be decoded by this build:

- **AVIF** — `image` 0.25's `avif` default feature is the *encoder* (`ravif`).
  Decoding needs `avif-native`, which is `dav1d`: a C dependency.
- **HEIC / HEIF** — not supported by `image` at all, and patent-encumbered.
- **JPEG XL** — not supported by `image` at all.

They used to import, fail to decode, and sit on the board as pictures that
could never draw. Two honest ways out: add the decoders, or stop calling them
images so they arrive as named file cards with a rail — which is what every
other format this build cannot open already does. **The second was taken**, in
both `by_extension` and `sniff`, and the description is kept (`"AVIF image"`)
so nothing is lost but the wrong claim. The bytes are still stored, so a build
that grows a decoder can call them images again.

### The 3D row said "rail only" and had been wrong for some time

It claimed every 3D format opens onto its facts and nothing else, blocked on
Phase 11. That was true when it was written and stopped being true the moment
`core::preview::of` grew its bytes test, because **half the 3D
formats in common use are text**. Asked directly, today:

```text
obj            -> Mesh / [Name]                      (done, see below)
stl (ascii)    -> Source { language: None } / [Text { limit: 200000 }, Name]
gltf           -> Source { language: None } / [Text { limit: 200000 }, Name]
dae            -> Source { language: None } / [Text { limit: 200000 }, Name]
step           -> Source { language: None } / [Text { limit: 200000 }, Name]
stl (binary)   -> Mesh / [Name]                      (done, see below)
glb            -> Mesh / [Name]                      (done, see below)
fbx            -> Nothing / [Name]
3mf            -> Nothing / [Name]
usdz           -> Nothing / [Name]
```

Until tranche 5 landed, a `.obj` opened as numbered source and was
**editable**, since the day the text arm landed — a consequence of the rule
that the bytes are the test, which is the rule working correctly, but nobody
had chosen that outcome and nothing tested it. `preview::bytes` now checks
`ext == "obj"` against the file's own shape (a `v` line and an `f` line, the
same test `mbrd_core::mesh::is_obj` uses to decide whether to parse it) before
it ever reaches the text arm, so a real `.obj` rasterises like `stl` and
`glb` do and drops out of `writable()`'s editable set entirely — a `Preview::Mesh`
gets only `[Editable::Name]`. An `.obj`-named file that fails that shape test
(no `v`/`f` lines in its first 200) still falls through to `Source` and stays
editable, which is the correct fallback for a file wearing the extension
without the shape.

Two things the same listing made obvious, both small and both real:

- `language("obj")` was `None`, so the preview was unlabelled. **Fixed** —
  `LANGUAGES` now carries `obj`, `gltf`, `dae` and `stl`, the four text-format
  3D extensions this table lists, so each is set in a fixed-width face with a
  word above it saying what it is. (`obj`'s entry now only fires for the
  fallback case above, since a real `.obj` no longer reaches the `Source` arm
  at all — still live, just narrower than when it was written.)
- `3mf` and `usdz` are **ZIP containers**, and this build has a ZIP reader — but
  `bytes` asked `ext == "zip"` rather than asking the bytes, so both fell
  through to `Nothing`. The same sentence covered `epub`, `docx`, `xlsx`, `pptx`
  and `sketch`: seven formats behind one string comparison. **Fixed** —
  `preview::bytes` now checks the local-file-header and empty-archive magic
  (`PK\x03\x04` / `PK\x05\x06`) directly, so all seven open as a listing
  regardless of what their extension claims. See
  `preview::tests::a_zip_wearing_another_name_is_still_a_listing`.

### Two more places the extension is trusted over the bytes

Worth stating together, because they were the same mistake in three files and
fixing one did not fix the others:

- `import.rs`'s `extension()` returned `"bin"` for any name without a dot, so
  the `("dockerfile", "Dockerfile")` and `("makefile", "Make")` rows in
  `LANGUAGES` could never match. **Fixed** — a dotless name now falls back to
  its own lowercased self (through the same alphanumeric-and-short filter that
  keeps a stray path out of a real extension), so `Dockerfile` is stored and
  looked up under `dockerfile` and the two rows are live. See
  `import::tests::a_dotless_convention_name_is_its_own_extension`.
- `sniff` still has no `PK\x03\x04` arm, and this one is staying that way on
  purpose rather than by oversight: unlike the preview check above, `sniff`
  runs *before* the name is consulted at all, so a blanket "bytes beat the
  name" arm here would reclassify a `model.3mf` or `report.docx` — files whose
  extension is genuinely informative — into a bare `Generic`/`"zip"`, which is
  a regression, not a fix. The practical symptom this bullet used to describe
  — a renamed `.zip` opening onto `Nothing` instead of a listing — is already
  gone: `preview::of` reads the asset's actual bytes for the archive check
  regardless of what `classify()` decided the extension was, so a `photo.zip`
  saved as `photo` (no extension at all) still shows its listing. What is left
  is cosmetic only: such a file's `ItemType`/description read `Generic`/`"file"`
  rather than `Generic`/`"archive"`.

---

## Previews that are already in the file

The cheapest breadth left in this list, and the reason it is worth a section of
its own rather than a row each: **most formats that look expensive are carrying
a picture already.** Photoshop, Illustrator, Sketch, InDesign, camera raw and
EPUB all embed a rasterised preview, because every file browser on every
platform needs one and none of them ship a decoder for the format either.

So the work is not a decoder per format. It is one path that goes looking for
the JPEG or PNG a file already contains, and `image` — already in the tree —
decodes whatever it finds. Six formats, one implementation, no new dependency.

Being honest about which of them can be relied on, because a preview path that
sometimes silently finds nothing is worse than one that was never claimed:

- **Camera raw** — `cr2`, `nef`, `arw`, `dng` are TIFF containers with a
  full-size JPEG inside. Reliable.
- **PSD** — image resource block 1036 is a JPEG thumbnail, and the merged
  composite is there behind it. Reliable.
- **Sketch, EPUB, 3MF** — ZIPs with a `preview.png`, a cover, or a thumbnail
  part. Reliable, and free once the ZIP check above asks the bytes.
- **Office** — `docProps/thumbnail.jpeg` exists only if the writing application
  saved one. Opportunistic; fall through to the entry listing when it is
  missing.
- **Illustrator** — modern `.ai` is PDF-compatible and usually carries a PNG
  stream. Opportunistic.

And what this is **not**: a way out of HEIC. Its embedded thumbnail is HEVC
too, so the four reclassified image types above still need a real decoder and
still wait on one. A trick that works for six formats is worth having; a trick
described as working for seven is a bug report.

---

## A mesh is a rasterisation problem, not a 3D problem

`ROADMAP.md`'s Phase 11 defers 3D on the grounds that GPUI has no 3D. That is
true and it is the wrong constraint, because **a still of a mesh needs no GPU at
all**. Transform the vertices, cull the back faces, z-buffer, flat-shade the
triangles into an `RgbaImage`, hand it over as a `RenderImage`. That is a few
hundred lines of arithmetic against a vertex buffer — which is to say it is
exactly the kind of thing `mbrd-core` is for, testable by rasterising a cube and
asserting which pixels came out.

It also lands somewhere that has been waiting for it. `live.rs` says so in its
own header:

> Nothing produces a live frame yet: the video decoder and **the mesh
> rasteriser** are what fill this, and neither has landed.

So this is filling a slot the architecture already reserved, with the
retire-and-sweep discipline that module exists to enforce already written.

**The rejected alternative**, recorded because it is the obvious one: build each
triangle as a `PathBuilder` path and `window.paint_path` it, the way `wires.rs`
already draws every rope. It works, and it does not scale — that is one
tessellated path per triangle *per frame*, and a mid-size mesh is a hundred
thousand of them against a board whose entire card budget is twenty thousand.
Rasterise once on the background executor instead, exactly as `images::decode`
already does for photographs.

### Which formats, and in what order

The order is by parse cost, not by popularity, because the rasteriser is written
once and each format after the first is only a front-end onto the same vertex
buffer:

1. **`stl` — done.** An 80-byte header, a `u32` count, and 50 bytes per
   triangle. *Only* triangles, so there was no polygon fanning and no index
   table needed to prove the rasteriser with — `mbrd_core::mesh`, tested
   against a cube, with `is_stl` asking the file's own shape (header count ×
   50 bytes has to equal what is actually there) rather than its `solid`
   text, which a binary exporter is free to write regardless. Wired the same
   way `resvg` is: `images::decode` recognises the bytes, fits a canvas to
   the mesh's own silhouette the way it fits one to an SVG's `viewBox`, and
   `Preview::Mesh` carries it onto the open page through the same `picture()`
   every other still goes through.
2. **`obj` — done.** Text, ubiquitous, and needed polygon-to-triangle fanning
   (fan from the first corner) plus negative-index support. Used to open as
   source; now `preview::bytes` gates the extension behind the same shape
   test `mbrd_core::mesh::is_obj` parses against (a `v` line and an `f` line
   in the first 200), so a real `.obj` rasterises and an `.obj`-named
   non-mesh still falls back to source. No `.mtl`, no texture coordinates —
   one file was dragged and one file is what there is.
3. **`glb` — done.** A 12-byte header, a JSON chunk and a binary chunk,
   walked by hand through `serde_json::Value` rather than a schema crate —
   `POSITION` and `indices` accessors only, unindexed primitives treated as
   "every three positions is a triangle", non-`TRIANGLES` primitives skipped,
   multiple meshes concatenated into one vertex buffer with index
   base-offsetting. **The format worth caring about most**, for a reason
   particular to this app: it is self-contained. Geometry *and* textures
   travel in one file, which is the only thing that survives being dragged
   onto a board — though this build reads neither materials nor textures out
   of it yet, so that self-containment is not yet paid off (see below).

`gltf` works when its buffers are inline base64 and not when they reference a
`.bin` nobody dropped. `ply` and `dae` are small additions. `fbx`, `blend`,
`step` and `iges` are not planned: the last two are ASCII and so already open as
source, but parsing NURBS solids is a research project rather than a sitting.

### What it will not do, said before anybody asks

No materials and no textures, from any of the three formats — not even `glb`,
which is the one format here that carries its own. A `.obj` arrives on a board
*without* its `.mtl` and without its texture maps, because one file was
dragged and one file is what there is; a `.glb` could in principle carry its
textures along, but a rasteriser with no GPU has nowhere to put them
regardless, so they are left unread. Flat-shaded is therefore the truthful
answer for the common case, in the same way that calling a HEIC a named file
rather than an image is the truthful answer above.

Two things needed deciding rather than assuming, and both are decided now:

- **What happens past a triangle cap.** Stop, and say so on the rail — not
  decimate. `TRIANGLE_MAX` in `mbrd_core::mesh` is the same shape of decision
  `preview::ENTRIES_MAX` already is, and a silently simplified shape is a
  worse lie than a card that says "200,001 triangles" beside a picture area
  that stayed blank. The triangle count comes from the STL header alone,
  written into `meta.triangles` at import same as a picture's own
  `naturalWidth`, so the rail can say it whether or not the cap let the still
  get drawn.
- **Whether the view turns.** It does now: `mbrd_core::media::Orbit` gives
  every mesh a camera (yaw, pitch, a bounding-sphere-relative distance, and a
  look-at point that can be shifted off the mesh's own centre), clamped and
  persisted per item. A drag turns it, a scroll dollies it, and the same drag
  held with Shift pans it instead — decided once at the press, so letting go
  of Shift mid-drag does not switch a turn into a pan under the pointer.
  Started fixed, as planned, and the draggable view is the second sitting
  that was promised — re-rasterising on the background executor, through a
  cache of its own (`mesh_cache.rs`) rather than `images.rs`'s, because a
  mesh's picture depends on its camera and not only on its bytes. Orthographic
  rather than perspective, on purpose: `dist` scales the same fit-to-canvas
  projection the still already had rather than reprojecting it, which keeps
  the well-tested rasterising pipeline untouched for a difference that is
  easy to miss on a flat-shaded preview anyway. On the board's own small
  thumbnail, where a drag already means "move this card", a right-click
  **Position** (`Command::Position`) hands one card's drag over to its camera
  until the mode is left again; the opened page has no such competition and
  always orbits. Rendered at twice the asked-for resolution and boxed back
  down (`mesh::downsample`) rather than one sample per pixel, so a silhouette
  edge is a soft gradient rather than the hard, aliased step a flat-shaded
  still would otherwise draw at any size worth looking at.

---

## Tranches

**1 — The structure, and everything that needs no new dependency. Done.**
`core/preview.rs` and `core/facts.rs`, with thirty-two tests between them and no
window anywhere near either. The shared info rail behind an `[i]` on every type, and an
Edit button that is never dead. Images contained rather than overflowing.
Numbered source, CSV tables and ZIP listings. Swatch, link and name editing —
`Field` grew a `Url`, and a rail row is how a card's *second* field is reached.
The four broken image types reclassified. `naturalWidth`/`naturalHeight`
written on import, so the rail can say how big a photograph actually is.

Not in it, and deferred on purpose: syntax highlighting, and `core/text.rs` —
see "One text, one door" above for why the accessor turned out to be a
tidiness win rather than one this crate needs to cash in.

**2 — Notes as files, and audio that plays — done.** The load-bearing half of
the format change — `notes/` outranking `board.json`, and a note with no asset
that grows past `NOTE_MAX` being promoted to one instead of clipped — is done;
see "Notes become Markdown files" above. So is playback, though not through
`symphonia` and `cpal`; see the media section. Left: the now-playing bar, which
is a playlist rather than a decoder.

**3 — Video — done.** It arrived with tranche 2 rather than after it, for the
reason written up above: one stack decodes both. Poster extraction on import is
the piece that did not land, and it never blocked anything.

**4 — Breadth.** The largest tranche by count and the smallest by risk: every
format above that needs one pure-Rust dependency or none at all. Roughly in the
order the work gets easier to justify —

- *Free, and done.* Asking the bytes whether they are a ZIP rather than asking
  the name — seven formats behind one comparison. The two dead `LANGUAGES`
  rows (`dockerfile`, `makefile`), and labels for the four text-format 3D
  extensions, including `obj` for the case where a `.obj`-named file fails
  the mesh shape test and falls back to source.
- *One dependency, and done.* `resvg` for SVG — the highest value per unit of
  work in this document, because a logo used to show its XML on a board whose
  entire purpose is looking at things. Now it rasterises to both tiers like
  any other picture, through the same `images::decode` and the same `picture()`
  in `opened.rs`, and its source is still one Edit button away.
- *Free, and done.* `tar` and `tgz` listings, since both crates were already
  compiled in for the updater — now for `mbrd-core` too, since `listing` is
  where a ZIP's central directory was already being walked without unpacking
  anything, and a tar's headers turn out to be exactly as cheap to walk the
  same way. Sniffed the way the ZIP check is: the `ustar` magic at its fixed
  header offset, or the gzip magic in front of it, neither one taken on the
  name's word.
- *Free, and done.* `diff` and `patch` colour their `+`/`-` lines now — one
  `match` in `listed`, `diff_color`, checked against the language `listed` was
  already carrying rather than a second lexer. Two new theme fields,
  `diff_add` and `diff_remove`, and not `rope_leaf`/`rope_danger` under a
  second name: those are what a *connection* may be coloured, and a theme
  file that recoloured a board's connectors was never asking to also recolour
  what `+` and `-` mean in a patch.
- *One dependency, and done.* `lopdf` for a PDF's text and page count. The
  page count is a measurement, written into `meta.pages` at import next to
  `naturalWidth`, so the rail can say it without opening the file — see
  `mbrd_core::facts`. The text is not: `Document::extract_text` walks
  compressed content streams and font encodings to get back to characters,
  which is real decoding and happens where `Preview::Vector`'s decoding
  happens, in `opened.rs`, on the page that asked for it.
- *One dependency, and done.* Fonts: `gpui::TextSystem::add_fonts` takes owned
  bytes at runtime, so a dropped TrueType, OpenType or TTC file is
  **registered and set in**, with `ttf-parser` reading the family name out of
  the `name` table at import — the same measured-not-decoded split as the PDF
  page count, and for the same reason: `facts.rs` can say what a font is
  called without a text system anywhere near it. Registered once per content
  hash, in `opened.rs`, the moment its specimen page is drawn. WOFF and WOFF2
  stay unclaimed — see the format table — the same way AVIF and HEIC do.
- *The embedded-preview path*, which is one implementation for six formats.

**5 — Meshes — done.** The software rasteriser and `stl`, `obj` and `glb`
onto it are all landed — self-contained, no dependency beyond the
`serde_json` already in `core`, and it converts one whole `ItemType` from "a
named card" to "a thing you can see", which is a larger jump than any single
format in tranche 4. `obj` and `glb` are front-ends onto the same vertex
buffer, tested in `mbrd_core::mesh` with no window; see "Which formats, and
in what order" above.

**6 — Editing that is not typing.** The two additions that are worth more than
any format: a CSV edited **as a grid** rather than as its own source, since
`Preview::Sheet` already builds the table and only the caret is missing; and
tags, which `ROADMAP.md`'s Phase 8 still owes and which are the one editable
field every card on the board would have.

Nothing in tranche 1 blocks on anything after it, 2 and 3 are independent of
each other, and 4, 5 and 6 depend on none of them. That ordering is the whole
reason for writing them down apart. **The one real sequencing claim** was
inside tranche 5, now finished: `stl` before `obj` before `glb`, because the
rasteriser was written once against the simplest input and everything after
it was a front-end.
