# Roadmap

What it would take to make `mbrd-rs` a real port of mbrd, in the order the work
actually depends on itself.

This is a plan, not a promise. The phases are ordered by dependency rather than
by appeal — the first two are unglamorous and everything else is much more
expensive if they are skipped.

---

## Done

Pan, zoom-to-cursor, marquee select, drag to move, arrow nudge, delete,
new note, save. Client-side titlebar with window controls. The `.mbrd` format in
both directions. **Phase 1: the mutation door and undo** — see below. 71 tests.
See `README.md`.

**Getting it onto other people's machines** — installers for all three
platforms, an icon, desktop and Finder integration, and an updater that runs
inside the app. Planned in `SHIPPING.md`, operated from `RELEASING.md`. It is
not a phase in the list below because it is not about what the app *does*; it
is the difference between a program and something somebody can have.

---

## Out of scope

Cut deliberately, not forgotten. Where the file format has a field for one of
these, **the field still round-trips** — dropping data on load would break the
format's promise even though we do nothing with it.

| cut | why |
| --- | --- |
| Custom fonts (drop a font file on the board) | A moodboard tool that is also a font manager. `settings.fonts` still round-trips. |
| Palettes, dynamic colour extraction, the pigment lab, appearance `vars`, style tiles | Recolouring the whole interface from the board's photographs is a lovely trick and not what anyone opens the app for. `settings.appearance` still round-trips. |
| Whimsy (the path-style axis) | A setting for how curvy the connection lines are. |
| Sound cues | An entire synthesis engine for interface feedback. |
| Quality tiers (the three-step performance setting) | It exists because the browser could not be relied on. Native should just be fast; if it is not, that is a bug to fix rather than a setting to offer. |
| Service worker, offline shell, QR / phone sharing, mobile web | Meaningless natively. |
| The Mobile profile: the column packer, per-layout geometry, switching between profiles | No phone runs this build, so there is no device to switch it for. `layouts.mobile`, `settings.mobile`, `mobile_header` and `mobile_columns` still round-trip. |

**Kept but late:** stickers and swatches. Decorative, but they are items a person
places, and they cost almost nothing once note editing exists.

---

## Three decisions to make before Phase 1

These change the shape of everything downstream, so they are worth settling
first rather than discovering.

### 1. Undo: port the timeline, or design fresh?

The original's history is a **step ledger** — each step holds the state on both
sides of what one action touched, keyed by item id, which makes it reversible in
either direction without knowing what the action meant. It survives a reload,
every step is a point the board can be taken back to, and three commands carry a
re-runnable rule (`align`, `distribute`, `arrange`) so editing the past ripples
forward.

**Recommendation: port the ledger's *format* faithfully, and keep the engine
simple to start.** A `.mbrd` that loses its history on a round trip through this
build is a data-loss bug even if we never draw a timeline. So: parse `timeline`,
carry it, write it back; implement plain linear undo/redo over the same delta
shape; leave scrubbing and step-editing for later. That keeps the format honest
without committing to the whole feature on day one.

**Settled, and done that way.** `op` is carried verbatim rather than dropped —
the original re-derives it in a session and does not write it, but a build that
loses a newer build's rule on a save is losing work it could have kept.

### 2. Video: a media stack, or poster frames only?

There is no `<video>` here. Real playback means GStreamer or FFmpeg — a large
native dependency, a decode thread, and frame delivery into a GPU texture every
16ms.

**Recommendation: poster frames only until Phase 7.** A video card shows the
still cut from its first frame (which the format already stores under
`meta.cover`) and double-click opens it in the system player. That is 90% of the
value of video on a moodboard for 5% of the work. Revisit when the rest is real.

### 3. 3D models: do them, or cut them?

Eleven mesh formats, hand-written synchronous readers, per-parse triangle
ceilings — the readers port cleanly and are good, testable, `mbrd-core`-shaped
work. The problem is the other end: gpui draws quads, text, paths and images.
It has **no 3D**. Rendering a mesh means a custom render pass against blade or a
software rasteriser into a texture.

**Recommendation: defer to last, and be willing to cut.** If it stays, do the
parsers early (they are pure and testable) and the rendering last.

**Settled, and the recommendation above is overtaken: it stays, as a software
rasteriser.** Of the two options named, the second turns out not to be the
consolation prize. A *still* of a mesh needs no render pass, no blade and no GPU
— transform, cull, z-buffer, flat-shade into an `RgbaImage`, hand it over as a
`RenderImage`. That is arithmetic against a vertex buffer, so it is
`mbrd-core`-shaped work of exactly the kind this decision already called good,
and it is testable by rasterising a cube and asserting pixels. `live.rs` was
built to hold the result and names the mesh rasteriser in its own header.

Two things this decision got wrong, both recorded in `VIEWING.md`: "no 3D" was
read as "no mesh preview", and the eleven formats are not eleven readers —
`stl`, `obj` and `glb` cover the ground, and half the others are text and have
been opening as editable source since `preview::of` grew its bytes test. See
`VIEWING.md`, tranche 5.

---

## Phase 1 — The mutation door and undo — **done**

**The single most important phase, and the one it is most tempting to skip.**

Right now `board_view.rs` mutates `doc.board` directly. Every feature after this
adds more places that do. Retrofitting undo across thirty scattered mutation
sites is the classic way this goes wrong — so the door comes before the things
that walk through it.

- ~~`core/state.rs`: one API through which every write to a `Board` passes.~~
  `BoardState` owns the board privately, derefs to `&Board` for reads, and lends
  out `&mut Board` only inside a closure it is watching. `Document.board` is one
  of these, so there is no `&mut Board` anywhere above `core`.
- ~~`core/history.rs`: the step ledger.~~ A delta applies only the fields it
  changed, which `merge_text` is the single implementation of.
- ~~Coalescing.~~ `run_key` — the ids that changed and which of *their* fields.
- ~~Undo/redo, and the marker (`at`).~~
- ~~`timeline` read and written.~~ With the fingerprint, so a ledger that
  describes another board is marked stale rather than believed.
- ~~Rewire `board_view.rs`.~~ A drag is one step: the gesture pipeline opens one
  at mouse-down and closes it at mouse-up.

**Three things that were not on this list and turned out to belong to it:**

- **A step must not cost the board.** The obvious ledger writes the board to
  text on both sides of every change; at 20,000 items that was 139ms per
  keystroke and a 1.09s undo. `BoardState` keeps a second copy of the board and
  compares it *structurally*, turning into text only what differs — 2.4ms and
  2.0ms, both proportional to the change rather than to the board.
- **`layouts.desktop` and the item fields are the same geometry twice**, and
  nothing was keeping them level, so every save wrote a file whose own numbers
  contradicted each other. The door levels them: the file's layout wins on open
  (which is what the format says), the items win thereafter.
- **A geometry list is keyed by id**, so `serialize` now writes it in the item
  list's order. Without that, a card removed and restored by an undo landed at
  the end of the layout and the same board produced different bytes.

**Left for later, deliberately:** scrubbing to an arbitrary step, naming one,
editing one so the change ripples forward, and trimming a ledger that has grown
large. All of them are additions to `history.rs`; none of them changes the
format, which is the point of having done the format first.

---

## Phase 2 — Rendering that scales, and assets that draw — **done**

Today each item is a `div`. That is right for fifty cards and wrong for the
twenty thousand the format allows.

- ~~Move item drawing into one painted layer.~~ Quads, pictures and shaped text,
  all inside the one `canvas()`. Cards were already event-free — that is this
  module's premise — so the per-`div` layout, style resolution and hit-test
  region were being paid for nothing.
- ~~Level-of-detail thresholds.~~ Three tiers: dust (one flat quad), plain (no
  rounding, no border, no label), and full. A label is the expensive one, so it
  is also the first to go.
- ~~**Spatial index** in `core`.~~ `core/index.rs`, a uniform grid hash, behind
  both culling and hit-testing. On a full board a screenful costs **2.0µs**
  against the scan's **97µs**, and a press **0.1µs** — measured in
  `tests/scale.rs`, which fails if the gap closes.
- ~~Asset pipeline.~~ Decode on `cx.background_executor()`, into an LRU keyed by
  content hash. Painted with `window.paint_image` rather than through an
  `ImageSource`, which keeps the whole board in the one painted layer.
- ~~Images render. Video and audio cards render their `meta.cover` poster.~~

**Three things that were not on this list and turned out to belong to it:**

- **The cache had to be told to let go.** Dropping the last `Arc<RenderImage>`
  frees the pixels and leaves the sprite atlas holding a tile — a leak in the
  one place a heap profile does not look. `Images::sweep` hands the tiles back,
  and it runs at the top of `render` because that is where a `&mut Window` is.
- **The index is only valid for the list it was built from**, so `revision`
  comes from one counter for the whole process rather than starting at zero per
  board. Two boards each at their own step one would otherwise collide, and
  closing one file to open another is the ordinary way to do that.
- **`Rect::of_item` ignored rotation.** A turned card reaches past its own
  width, so it was being culled early, cropped out of a fit, and missed by a
  marquee that plainly crossed it. The box is now the tilted one; `geometry::hit`
  still decides, because the box is a superset.

**Not done, and honestly so:** a card is still drawn square. GPUI's quads do not
rotate, so drawing rotation wants a transform layer — the geometry knows about
it everywhere else.

---

## Phase 3 — Getting things onto the board — **mostly done**

- ~~Drag-and-drop of files and folders; paste from the clipboard.~~ A folder
  brings what is directly in it and goes no deeper: somebody who drops their
  home directory by accident should get a shrug, not a frozen window. A paste is
  a picture, an address, or some words, in that order — and an address that
  points at a file is *followed*: a pasted MP4, GIF or `.obj` is fetched and
  becomes that card, because a link to a video is a video somebody meant to put
  on their board. `Ctrl Shift V` is how to say you meant the link. See
  `fetch.rs` for the bounds on it. **A file picker via the XDG portal is still
  to do.**
- ~~Classify by extension and magic bytes into an `ItemType`.~~ The bytes get the
  first word — a `.jpg` that is really a PNG is common, and a pasted screenshot
  has no name at all. **The catalogue is a hand-written subset**, not the
  original's generated ~1,350 formats: the families that matter for the four
  card types this build draws, plus enough of a tail that a `.sldprt` reads
  "3D / CAD" and anything else arrives as a named card rather than as nothing.
  Generating the full table is still worth doing.
- ~~Content-hash dedup on the way in.~~ SHA-256, the format's own identity, so
  dropping the same folder twice costs cards and no bytes.
- ~~Size ceilings, and the **consent** model.~~ `import.rs` reports
  (`Ready::is_heavy`); `board_view.rs` decides, and what it decides is to name
  the file and its size rather than to drop it quietly.

**Left:** the portal, and the generated catalogue.

---

## Phase 4 — Editing what is on it — **mostly done**

- ~~Resize grips: corners, edges, `Shift` to keep proportions.~~ `app/grips.rs`.
  The test happens in screen pixels and the answer is in world units, so a
  handle is the same size to aim at whatever the zoom. **Four dots, not eight**:
  an edge is taken along its whole run rather than at a handle in the middle of
  it, which is both less furniture and less to aim at. And *keeping the shape*
  is a ratio rather than a flag, because the shape worth keeping is the
  picture's own — so a photograph resizes proportionally by **default**,
  `Ctrl` resizes it freely, and `Alt` crops it by framing it `cover`.
- **Rotate — not done, and blocked rather than skipped.** GPUI carries a
  transform on monochrome SVG sprites and on nothing else: not quads, not
  images, not glyphs. So drawing a turned card means a `paint_path` body and a
  render target per picture, which is a piece of work in its own right. Offering
  a rotate gesture before then would mean a card that turns and does not look
  turned. Handles are hidden on a card that already has a `rot`, rather than
  drawn where the card visibly is not.
- ~~Rename, `F2`.~~ And Enter, and a double-click.
- ~~**Note editing.**~~ `app/editor.rs` — the model only: a string, a caret, a
  selection and the rules, all testable without a window. The two things that
  genuinely need a font — which character was clicked, and where to put the
  caret — are in `board_view.rs`, where the text system is. **Markdown is read
  and drawn** — `app/markdown.rs`, text in and styled lines out, no window
  needed to test it — but that is a *reader*, not the stored rich model: a note
  is still one string in `meta.text`, which is what keeps it a real `.md` file
  in the archive. A note being typed into shows its marks, because the caret
  counts characters and they have to be the characters on the screen. **The
  stored rich model is still ahead**, and so is input-method support: a composing keyboard
  types nothing rather than typing something wrong, which wants
  `EntityInputHandler` built once for the whole app.
- ~~Swatches~~; **stickers are half**. A swatch is complete — `meta.hex`, the
  name carrying the same value, and typing a colour into it *is* the colour
  picker. A sticker's `shape` names a symbol in the original's SVG catalogue,
  which this build does not carry, so one draws as a tinted card and its fields
  round-trip untouched.
- ~~Copy, cut, paste, duplicate, and the internal clipboard.~~ `Ctrl V` tries
  the app's own cards first and falls through to the system clipboard, so one
  key does both in the order somebody would guess.

**One thing that was not on this list and turned out to belong to it:** a
clipboard that is *two* clipboards. A card is not text — it has a size, a type
and possibly several megabytes of photograph — so copying one puts the cards on
the app's own clipboard and their *names* on the system's, and pasting looks at
the app's first.

---

## Phase 5 — Structure between things — **done**

- ~~**Fences.**~~ `core/fence.rs`. Membership is *measured*, not stored: a card
  is in a fence when its **centre** is inside the fence's rectangle, which is
  what makes a card half over an edge belong somewhere definite rather than to
  whichever list it got written into first. Nesting is smallest-containing-wins,
  and a fence's parent must be of **strictly greater area** — that is what makes
  the containment chain a strict order, and what makes `chain()` terminate
  rather than walk a cycle of two same-sized fences each holding the other.
  `meta.fence` is written on the way out as a *record* of the measurement and is
  never read as the authority for it.
- ~~**Connections drawn.**~~ Two pieces, because they answer different
  questions. `core/rope.rs` draws the ordinary line — a Bézier leaving each card
  by its facing side, which is what a connection between two cards with nothing
  between them should look like. `core/route.rs` is the orthogonal A\* router,
  and it runs **only when the curve actually meets something**: the lattice is
  built from the obstacles' own edges pushed out by a clearance rather than from
  a fixed grid, because world space is infinite and float. Cost is distance plus
  a turn penalty, and the search state is `(node, arrival-axis)` so a turn is
  charged where it is made. There is **no failure case** — a concession ladder
  walks full clearance → a third → none → drop the obstacles covering an end →
  a two-bend elbow — so the worst outcome is a line that goes *behind* something
  rather than no line at all. Arrowheads, the four styles, five colours, three
  weights and labels are all in.
- ~~Sticky notes pinned to a host~~ — **removed, and not coming back.** It
  shipped twice and was wrong both times. First as a measurement, where any note
  lying on a card was pinned to it and `meta.loose` was the opt-out: what
  everybody hit was two things that merely overlapped refusing to move apart,
  for a reason nothing on screen could explain. Then as a decision, `meta.sticky`
  off by default and set from the note's own menu, which fixed the surprise and
  left a feature nobody reached for — a menu row, a keystroke, a drop preview,
  a rule in every layout pass and a special case in every gesture that moves
  something, to buy a caption that travels with a photograph. A fence already
  does that, is visible, and is asked for the same way every other grouping is.
  `core/stick.rs` is gone; the keys it wrote ride through `meta` untouched.
- ~~Locking~~ — `meta.locked`, and the shape the sticky flag should have had:
  a decision, stored, that nothing measured may set. A locked card cannot be
  dragged, resized, nudged or binned, no layout deals it a slot — it goes in as
  an obstacle instead, the way the title card does — and it wears a padlock in
  its top corner so that being unable to move it is never a mystery. Still
  selectable, because unlocking is a thing you do to it.
- ~~z-order~~ — done in Phase 4.
- ~~Align, distribute, and overlap separation.~~ `core/align.rs`, all measured on
  the *tilted* box so a turned card aligns by what it covers. Distribute equalises
  the **gaps** and holds the two ends. Separation is relaxation with a short-axis
  push. Each returns only what actually moved, so aligning an already-aligned row
  records no step to undo.
- ~~Snap-to-grid, with `presnap`.~~ `core/snap.rs`. Three rules: a card already
  on the lattice gets no memo, a memo is written once however many times you
  toggle, and releasing clears it.

**Three things that were not on this list and turned out to belong to it:**

**Nothing is routed while anything is moving.** A route is kept exactly as long
as both of its ends are where they were — `app/wires.rs` caches on the two end
rectangles, so a card that is not an end moving costs nothing, and a drag asks
the router nothing at all and draws curves. Forty routes over sixty cards settle
in **2.16ms**, which is the measurement `tests/scale.rs` holds honest.

**GPUI has no stroke.** `paint_path` fills, so a line has to be *built* as the
region it covers — and `line_to` fans triangles from the path's first point,
which is only correct for a star-shaped polygon. So the ribbon is pushed as
triangles directly, each segment extended by half a width at both ends, which is
what fills the notch a right angle would otherwise leave at a corner. Dashes are
measured in **screen pixels** so a dashed line stays dashed at every zoom, and
by arclength along the whole polyline so a dash carries on round a corner.

**A card offers you the rope.** Hovering a card puts four faint marks just
outside its edges (`app/anchor.rs`, kept out of the gesture pipeline for the
reason `grips.rs` is); dragging one draws a rope that snaps to the facing side of
whatever card it lands on. That wanted a tool strip (`app/tools.rs` — select,
pan, connect, note) and a **second menu**: a rope is not a card, so `command.rs`
now carries three lists — one card, one rope, one for a multiple selection — and
the rope's rows tick to show the colour, style, weight and arrows it already has.

---

## Phase 6 — Arrangements — **done**

- The seven arrangements, each a pure `(items, opts) -> Vec<Point>` in `core`.
  Spiral, grid, masonry, by type, by date, scattered, free. Whole board or just
  the selection. (An eighth, by tag, joined them the same way — `Arrangement`
  is a closed enum with an `ALL` menu-mapping tested against the `Layout`
  menu's own entry count, so nothing here can drift out of sync with what the
  engine actually offers.)

**Note:** the Mobile profile — the column packer, per-layout geometry, and
switching between profiles — is out of scope; see the table above. `settings`
and `layouts` still carry a `mobile` side so a file that has one is not
corrupted on save, but nothing here reads it as a second layout to show.

---

## Phase 7 — Media

- Audio: decode, play, and draw the waveform from the `waveforms/` sidecar
  (already parsed and written). The playlist, `audioOrder`, the now-playing bar.
- Video: see decision 2. If it goes ahead, GStreamer.

---

## Phase 8 — Finding your way around

- ~~Search (`Ctrl`+`K`) across names, note text, link URLs and tags, and jump to a
  result.~~ **Done**, except tags — there are none yet. `Ctrl`+`F` and `Ctrl`+`K`
  both open it, as does tapping `Ctrl` twice; a result is selected *and*
  travelled to, because being told where a card is would be no use on a canvas
  you then have to cross by hand. See `app/palette.rs`.
- The command palette (`Shift` `Shift`) — `command.rs`'s whole table as a list
  you type at, which is the only way to reach the third of it that has no key.
  Done alongside the search, since they are one mechanism with two lists.
- Tags, and filtering the board by them.
- The tour: a saved route, walked with a camera move per stop.
- The viewer: double-click to zoom to an item, and a full-screen look at one.
- The bin as a place on the canvas you can drag things back out of. **Dropped.**
  A bin earns its keep by being somewhere you can take things back *out* of, and
  nothing here ever built that — so what the format's `trash` section actually
  did in this app was keep every deleted photograph's bytes in the file forever,
  against a restore that was never coming. The bin now lives in memory for the
  length of a session and is emptied at the file boundary; undo is the route
  back, and it survives a reopen because the ledger does. See
  `core/model.rs`'s `TrashEntry`. If this comes back it comes back as the
  draggable place described here, not as a number in the corner.
- The inventory sheet — what this board is made of, and what it weighs.

---

## Phase 9 — Real-world size ◐

Scale calibration against a sheet of paper, the paper outlines, the scale bar,
the HUD, metric and imperial. Small, self-contained, and pleasant.

The maths and the outline are done: `mbrd_core::paper` carries a seven-sheet
catalogue (`a3` through `tabloid`, the ids `schema::is_paper_id` already
whitelisted), turns a sheet plus `BoardSettings::scale` into a world-space
rectangle, and picks a scale bar's nice round length off a 1-2-5 sequence in
either metric or imperial. `paint_board` outlines the sheet around the origin
whenever `paper` names one, in the same stroke-only style as the axes.

The bar is painted too, now that `hud` has something to mean: `ToggleHud` is a
command exactly the way `ToggleGrid` and `ToggleWeb` are — a key-less row on
the View menu and in the palette, since the letters worth spending are gone —
and `status_bar` draws a segment beside the zoom reading, its own width the
bar's length in screen pixels, whenever it is on.

And now a way to set them, not through `settings.rs` but the way `Arrange`
already answers the same question for the eight layouts: `Command::Paper`
carries a `PaperSize` mapped over `mbrd_core::paper::PaperSize::ALL`, a
`Paper` submenu on `View` names the sheet it is closed on, and
`ToggleLandscape` and `ToggleUnits` sit beside it as plain switches — the
`bool` and the two-spelling string that a submenu of one choice each would
have been a worse way to show. All three go through `BoardView::set_paper` /
`toggle_setting` / `toggle_units`, the same one-door `board.edit` every other
board setting already goes through.

Left, and the only piece of Phase 9 still open: the calibration gesture
itself — draw a line against the paper, say what it measures, and `scale`
follows. That one wants a real window to get right and is the riskiest piece
left in the whole phase.

---

## Phase 10 — Making boards smaller

Recompress images; re-encode audio to Opus. Says what it will do before it does
it, and is undoable. Depends on Phase 1 for the undo half.

---

## Phase 11 — 3D — **done**

See decision 3, and its settlement: a software rasteriser rather than a render
pass, which takes this out of "willing to cut" and into ordinary work. The plan
lived in `VIEWING.md` as tranche 5 — `stl` first because it is only triangles,
then `obj` and `glb` as two more front-ends onto the same vertex buffer, all
three now wired into `images::decode` the same way `resvg` is for an SVG:

- **`stl`** — a binary STL's facets, read straight off their fixed 50-byte
  layout with no vertex shared between two triangles, because the format
  shares none either.
- **`obj`** — a face fanned from its first corner regardless of how many
  corners it names, and a negative index counted back from whichever vertex
  was most recently read. No `.mtl`, no texture coordinates: one file was
  dropped, and one file is what there is, the same note the module header
  gives for every format here.
- **`glb`** — the `JSON` chunk's `meshes`/`primitives`/`accessors` walked by
  hand (`serde_json::Value`, no schema crate — the whole of what is read is
  four fields deep) to find each primitive's `POSITION` and, where it has one,
  its `indices`; every mesh in the file concatenates into one vertex buffer,
  offset so a second mesh's triangles still point at its own vertices. No
  materials and no textures read even though this is the one format that
  carries its own — a rasteriser with no GPU has nowhere to put them
  regardless.

All three are tested in `mbrd-core` with no window anywhere near them: `stl`
and the rasteriser itself against a cube, `obj` against a fanned quad and a
face naming a corner that does not exist yet, `glb` against a hand-built
container exercising both the indexed and unindexed paths and two meshes
sharing one buffer.

The still it opened with is not the still it kept: `mbrd_core::media::Orbit`
gives every mesh card a camera — yaw, pitch and a bounding-sphere-relative
distance, clamped and persisted per item — and a drag orbits it, a scroll
dollies it, both in the opened page always and on the board's own small
thumbnail behind a right-click "Position" toggle (`Command::Position`) that
hands a card's drag over from moving it to turning it. Still orthographic,
on purpose — see the mesh-camera plan's own note on that trade-off — so the
rasteriser's projection and z-buffer are untouched; only what feeds them a
rotation changed.

Still last in this list, and no longer last because it is doubtful.

---

## Ordering, in one line

**1 → 2 → 3** is the spine and is not reorderable: undo before mutations,
rendering before content, content before everything. **1, 2 and most of 3 are
done**, so is most of **4**, and **5** is done — structure between things, which
is what a moodboard is *for* once the things are on it. So the next thing is
**6**, the seven arrangements, and **7–11** are breadth. What is left of 3 and
4 — the XDG portal, the generated format
catalogue, rich notes, sticker shapes — can be picked up any time; none of it
blocks anything. **Rotation drawing is the one genuine dependency**: it is
blocked on GPUI's scene primitives, and it blocks the rotate gesture.

## Where the work lands

Roughly, and worth watching — the split is what keeps the format testable.

| phase | `mbrd-core` (no window, fully testable) | `mbrd` (needs a window) |
| --- | --- | --- |
| 1 ✓ | the door, the ledger, the timeline format | rewiring the view |
| 2 ✓ | spatial index | painted layer, decode cache |
| 3 ◐ | classify, ceilings *(catalogue partial)* | drop targets *(portal to do)* |
| 4 ◐ | *(the stored rich-note model, still ahead)* | grips ✓, the text input ✓, Markdown ✓ |
| 5 ✓ | fence measurement, the router, align/distribute, locking, snap | the ribbon, the anchors, the rope menu |
| 6 ✓ | all eight arrangements | — |
| 9 ◐ | scale and paper maths ✓ | the outline ✓, the status bar's scale segment ✓, the Paper/Landscape/Units menu ✓, the calibration gesture *(still ahead)* |
| 11 ✓ | the rasteriser, `stl`, `obj`, `glb` | the decode dispatch, the picture pipeline |

If a phase turns out to be mostly right-hand column, that is usually a sign
something pure is hiding in it that has not been pulled out yet.
