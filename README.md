# mbrd-rs

A native port of [mbrd](../mbrd) — a moodboard on a canvas that goes on
forever — onto [GPUI](https://gpui.rs/), the GPU-accelerated Rust UI framework
from the people who make Zed.

The board model and the `.mbrd` file format are done and tested. Every change
goes through one door and can be taken back, and that history survives a save.
The canvas draws itself in one painted layer with a spatial index behind it, so
it holds the twenty thousand cards the format allows; pictures decode off the
main thread and render; files can be dropped or pasted onto the board; cards can
be typed into, resized, stacked, copied and coloured. Cards can be joined with
ropes that go round whatever is in the way, penned into fences, pinned to each
other, lined up and spaced out. What is left is mostly *arrangements* and the
things that need a decoder. See **What is not here yet**.

## Run it

```
cargo run -p mbrd                       # a demonstration board
cargo run -p mbrd -- some-board.mbrd    # a real one
```

### Linux needs one system package

GPUI links `libxkbcommon-x11` on Linux **whether or not you enable its X11
backend** — the dependency is not feature-gated upstream — so the development
package has to be present even for a Wayland-only build:

```
sudo dnf install libxkbcommon-x11-devel     # Fedora
sudo apt install libxkbcommon-x11-dev       # Debian / Ubuntu
```

Without it the compile succeeds and the *link* fails with
`unable to find library -lxkbcommon-x11`, which is a confusing place to meet
the problem for the first time.

Also wanted: a Vulkan driver, and `fontconfig` and `freetype` development
packages, all of which a desktop install normally already has.

## Builds for other people

```
git tag v0.0.1 && git push --tags
```

That builds macOS (Apple Silicon and Intel), Linux and Windows in parallel and
opens a **draft** release with all four on it, which you publish once the
matrix is green. Pressing the button on `Release` in the Actions tab instead
builds the same four and attaches them to the run, without cutting a release —
which is how to check the app starts on a platform you do not own.

The tag has to match the `version` under `[workspace.package]`. CI stops the
run if it does not.

The whole of it is in `RELEASING.md`, including why Windows cannot be
cross-compiled from here, why the macOS artifact is a bundle rather than a
binary, and what "unsigned" costs whoever you send it to.

## Controls

| | |
|---|---|
| drag empty space | pan around |
| click empty space | let go of the selection; `Ctrl`+`Z` puts it back |
| middle-drag | pan from anywhere, even over a card |
| wheel | zoom to the cursor |
| `Shift` + wheel | pan sideways |
| `Shift` or `Ctrl` + drag empty space | select several cards |
| drag a card | move it, and anything selected with it |
| arrows | nudge; `Shift` moves a whole grid step |
| `0` / `F` | recenter / fit everything on screen |
| `Ctrl`+`A` / `Esc` / `Del` | select all / clear / to the bin |
| right-click | everything else, next to what it acts on |
| right-click bare paper | lets go of the selection, and offers the board's own list |
| a right-click row marked ▸ | opens beside it, rather than down the same list |
| double-click a card, or `F2` | type into it |
| drag a corner or an edge | resize; a picture keeps its proportions |
| `Ctrl` + drag a handle | resize a picture freely; `Shift` holds any card's shape |
| `Alt` + drag a handle | crop: the picture fills the card and the card cuts it |
| `N` / `K` / `E` | new sticky note / new color / new fence |
| `T` | next tint, on a note |
| hover a card, drag a mark | pull a rope to another card |
| `J` | join what is selected, with the shortest set of ropes |
| double-click a rope, or `F2` | label it; right-click it for color, arrows, style |
| `U` | unstick a note from the card it is on |
| `W` | show or hide the ropes |
| `1`–`4`, or `V` / `H` / `C` | select / pan / connect / note |
| `Ctrl`+`C` / `Ctrl`+`X` / `Ctrl`+`D` | copy / cut / duplicate |
| `[` / `]` | send to back / bring to front |
| `G` / `X` / `S` | grid / axes / snapping |
| drop a file or folder | put it on the board |
| `Ctrl`+`V` | paste a picture, an address, or some words |
| `Ctrl`+`Z` / `Ctrl`+`Shift`+`Z` | undo / redo |
| `Ctrl`+`S` | save |
| `Ctrl`+`P` | open another board |

## Layout

Two crates, and the split is load-bearing rather than tidy:

```
crates/core/   mbrd-core — the board and the file format. No UI, no GPU.
crates/app/    mbrd      — the window.
```

`mbrd-core` has no dependency on GPUI, which is what makes the format testable:
feeding the reader a broken board and asserting what comes back needs no window,
no event loop and no graphics driver, so those tests run in milliseconds and run
anywhere. The original enforces the same layering with a test; here Cargo
enforces it, because the cycle would not build.

```
geometry, history <- model <- {schema, index, viewport, naming} <- state <- mbrd  |  app
```

| file | what it holds |
| --- | --- |
| `core/model.rs` | `Board`, `Item`, settings, connections — what the app writes |
| `core/state.rs` | **the one door every write passes through**, and undo |
| `core/history.rs` | the step ledger, as data: diff, merge, fold, fingerprint |
| `core/schema.rs` | `board.json` in both directions — what a reader will *accept* |
| `core/mbrd.rs` | the ZIP: manifest, assets, note sidecars, waveforms |
| `core/viewport.rs` | the camera, and the only place world y is flipped to screen y |
| `core/geometry.rs` | rectangles, hit-testing, bounds |
| `core/index.rs` | the spatial index: which cards are near a place |
| `core/naming.rs` | a title to a filename, a clock to a timestamp |
| `core/fence.rs` | which fence a card is in — **measured**, never stored |
| `core/stick.rs` | which card a note is pinned to, and what `loose` means |
| `core/rope.rs` | the curve a connection draws when nothing is in the way |
| `core/route.rs` | the orthogonal router, for when something is |
| `core/align.rs` | align, distribute, and pushing overlaps apart |
| `core/snap.rs` | the grid, and `presnap` so turning it off puts things back |
| `app/board_view.rs` | the canvas, **the one gesture pipeline**, and the painted layer |
| `app/command.rs` | **every action, named once** — the keyboard and the menu read it |
| `app/editor.rs` | a text field, because GPUI ships none — caret, selection, words |
| `app/grips.rs` | which handle the pointer is on, and what dragging it does |
| `app/anchor.rs` | the four marks beside a card, and where a rope starts |
| `app/wires.rs` | the route cache, and the rule that nothing routes while anything moves |
| `app/tools.rs` | what the pointer means right now, and the strip that says so |
| `app/images.rs` | decoding off the main thread, and the cache that must not leak |
| `app/markdown.rs` | a note's marks, read into lines and runs a painter can draw |
| `app/import.rs` | a dropped file to a card: classify, hash, measure, report |
| `app/menu.rs`, `app/switcher.rs` | the right-click list, and jumping between boards |
| `app/titlebar.rs` | window furniture, for compositors that decline to draw it |
| `app/recent.rs`, `app/theme.rs`, `app/save.rs`, `app/demo.rs` | the rest |

## Six things that are easy to break silently

**`x` and `y` are an item's centre, and `y` points up.** A card at `y: 100` sits
*above* the origin. The flip to screen coordinates happens in the four
conversions in `viewport.rs` and nowhere else. Getting this wrong does not
crash; it mirrors the board.

**`schema::normalize` cannot fail.** It returns a `Board`, never a `Result`, and
degrades one field at a time. There is deliberately no error type in that module
to be tempted by, because the alternative is a load that gives up half-way and
leaves a board that is neither the old one nor the new one.

**Nothing outside `core/state.rs` may hold a `&mut Board`.** `BoardState` owns
one and lends it out only inside a closure it is watching, which is what records
a step for every mutation without any call site having to remember to. It derefs
to `&Board`, so *reading* one is exactly as easy as reading a board; there is no
`DerefMut`. The day something adds a second way to write to a board is the day
undo starts quietly missing edits.

**The spatial index is only valid for the list it was built from.** `Grid`
stores positions in a slice, so a stale one does not answer slightly wrong — it
answers about cards that have moved. Every reader goes through
`BoardView::index`, which checks `BoardState::revision` first; that revision is
drawn from one counter for the whole process, so "the same number" means "the
same board, unchanged" rather than merely "some board, unchanged".

**Membership is measured, and `meta` only records it.** A card is in a fence
when its centre is inside the fence; a note is stuck to a card when enough of it
lies over one. `meta.fence` and `meta.stuckTo` are written on the way out as a
record of that measurement and are never read as the authority for it, which is
what stops a board from arriving with a card that claims to be in a fence it is
nowhere near. The one exception is deliberate: `meta.loose` is a *decision*
rather than a measurement, because a note you have just unstuck is normally
still lying on the card you unstuck it from, and without the flag it would pin
itself straight back.

**An unknown item `type` and an unknown `meta` key must round-trip untouched.**
That is the format's extension point — it is what let `swatch` and then
`sticker` ship without older builds losing those items. The demonstration board
in `app/demo.rs` carries a card of type `hologram` on purpose; if it ever stops
surviving a save, the extension point has broken.

## The file format

A `.mbrd` is a ZIP with a different extension, and it is
[specified](../mbrd/research/docs/mbrd-format.md) and explicitly free to
implement. This crate is a second implementation of it, which is the most useful
thing a port like this can be.

```
myboard.mbrd
├── mimetype                    the media type, first and stored, at offset 38
├── manifest.json               what this file is
├── board.json                  the board itself
├── assets/<slug>--<hash>.<ext> embedded bytes, deduped by content hash
├── notes/<slug>--<id>.md       one sticky note, as Markdown
└── waveforms/<hash>.json       one audio file's measured readings
```

The archive is meant to be *legible*, not merely parseable — so the JSON is
indented, the notes are real Markdown you can edit in place, and `file(1)` can
name the format without knowing anything about it:

```
$ file Kitchen.mbrd
Kitchen.mbrd: Zip data (MIME type "application/vnd.mbrd+zip"?)
```

Editing `notes/*.md` inside the archive and rezipping really does change the
board — the sidecar outranks `board.json`, which is the whole point of writing
it, and there is a test for it.

## Tests

```
cargo test                  # 321 tests; not one of them needs a window
```

They are mostly about the format refusing to lose somebody's work: rubbish in
still gives a board back, a binned card never collides with a live one, a
connection to a binned card survives, a tampered asset drops without taking the
board with it, and packing a board whose bytes are missing **fails** rather than
writing a file with a hole in it while reporting success.

Some are about a change being reversible: a step applies only the fields it
changed, a run of nudges is one entry, a board walked six steps back and six
forward is byte-identical at every stop, and a history that does not describe the
board it arrived with says so rather than being believed.

The newest are about scale and about what arrives from outside. `core/index.rs`
is checked against a brute-force scan over a thousand boards' worth of windows,
and `tests/scale.rs` holds the measurements honest: on a full board of twenty
thousand cards, a screenful costs **2.0µs** where the scan it replaced cost
**97µs**, and a press costs **0.1µs**. On the import side, the bytes are believed
over the file name, a `.jpg` that is really a PNG lands as a PNG, the same
photograph twice is one asset, and a file too large is reported by name rather
than dropped quietly.

The router is tested as arithmetic, which is the only way it *can* be tested:
every line it returns is checked for right angles, for clearing the boxes it was
asked to clear, and — at the bottom of the concession ladder, where a route has
to go behind something — for still arriving. `tests/scale.rs` holds the frame
budget honest as well as the index: forty routes over sixty cards settle in
**2.16ms**, and nothing is routed at all while anything is moving.

The text field is tested the way the format is — as data, with no window. A
caret never lands inside a character, backspace over an `é` takes the whole
letter, a limit counts characters rather than bytes so a note in Greek holds as
much as one in English, up and down keep the column they started from, and
`Ctrl S` inside a note saves the board rather than typing an `s`.

## What is not here yet

See [`ROADMAP.md`](ROADMAP.md) for the plan, what has been cut and why, and the
three decisions worth settling first. The short version, in the order it
matters:

- **No input method.** A composing keyboard — Japanese, Korean, Chinese — types
  nothing rather than typing something wrong. That wants GPUI's
  `EntityInputHandler`, built once for the whole app.
- **Notes are Markdown, and no more than that.** Headings, bullets, numbered
  and task lists, quotes, rules, `**bold**`, `*italic*`, `` `code` ``,
  `~~struck~~` and `[links](url)` are read and drawn; a note being typed into
  shows its marks, because a note *is* the text. What is still missing is the
  original's stored rich model — alignment, the highlighter wash, and text that
  keeps its formatting when it leaves the card.
- **No sticker shapes.** The format's `meta.shape` names a symbol in the
  original's SVG catalogue, which this build does not carry, so a sticker draws
  as a tinted card. Its `shape` and `tint` round-trip untouched.
- **No file picker.** Drag-and-drop and paste work; opening a board you have
  never opened before still means the command line or a `.mbrd` sitting next to
  one you have. The XDG portal is Phase 3.
- **Video and audio do not play**, and a dropped video arrives without a poster
  — extracting one needs a decoder this build does not link. A video or audio
  card *does* draw the `meta.cover` a board already carries.
- **No arrangements, mobile layout, palettes or sound.** The seven
  arrangements — spiral, grid, masonry, by type, by date, scattered, free — and
  the Mobile layout profile are Phase 6, and are the largest thing left.
- **Rotation is not drawn**, so nothing offers a way to set it. It round-trips
  through the format and is honoured by hit-testing, culling and framing, but
  GPUI carries a transform only on monochrome SVG sprites — not on quads, images
  or glyphs — so drawing it means a card body as a `paint_path` and a render
  target per picture. Handles are hidden on a turned card rather than drawn
  somewhere the card visibly is not.
- **Undo is linear.** The ledger is parsed, kept and written back in full —
  including the `op` rules this build does not run — but only its two ends are
  reachable. Scrubbing to an arbitrary step, naming one, and editing one so the
  change ripples forward are all still ahead.
