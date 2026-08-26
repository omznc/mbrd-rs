# Contributing

Notes for anyone building, changing, or shipping mbrd. What the app *is* and
how to drive it is in [`README.md`](README.md); the plan is in
[`ROADMAP.md`](ROADMAP.md).

## Building

```
cargo run -p mbrd                       # a demonstration board
cargo run -p mbrd -- some-board.mbrd    # a real one
```

### Linux needs one system package

*(Only for building, and only for the bare binary. The AppImage on a release
carries its own copies of all of this — see `RELEASING.md`.)*

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
| `core/rope.rs` | the curve a connection draws when nothing is in the way |
| `core/route.rs` | the orthogonal router, for when something is |
| `core/align.rs` | align, distribute, and pushing overlaps apart |
| `core/guides.rs` | what a dragged card lines up with, and the rules drawn to say so |
| `core/snap.rs` | the grid, and `presnap` so turning it off puts things back |
| `core/motion.rs` | springs, flick projection, rubber-banding — pure arithmetic |
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
| `app/palette.rs` | the command list and the search list, and going to what you found |
| `app/camera.rs` | pan and zoom as springs, and the trail a flick is measured from |
| `app/prefs.rs` | what a person chose, as against what a board says — and never in the file |
| `app/dirs.rs` | the four places this app may write, on each of the three platforms |
| `app/save.rs` | the two ways a board crosses the disk — atomically, both ways |
| `app/taps.rs` | a modifier tapped twice, and every reason that is not one |
| `app/fuzzy.rs` | matching what somebody half-remembered against what there is |
| `app/titlebar.rs` | the top bar, on every platform, and the project switcher on it |
| `app/icons.rs` | the pictures, compiled in, and why duotone survives a one-colour draw |
| `app/recent.rs`, `app/theme.rs`, `app/demo.rs` | the rest |

The icons are [Phosphor](https://phosphoricons.com/)'s duotone set, MIT, vendored
one file at a time under `crates/app/assets/icons` with their licence beside
them. Only the ones that are drawn are carried; `app/icons.rs` says why that set
in particular, which is that GPUI keeps an SVG's alpha and throws its colour
away — and Phosphor's two tones are two alphas of one colour rather than two
hues, so they come through a monochrome draw intact.

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
when its centre is inside the fence. `meta.fence` is written on the way out as a
record of that measurement and is never read as the authority for it, which is
what stops a board from arriving with a card that claims to be in a fence it is
nowhere near.

**A decision is stored, and nothing may infer it.** The other half of the same
rule, and the reason the two are written down together: `meta.locked` says an
author nailed a card down, and nothing about where the card sits or what it lies
on may set it or clear it. Notes used to have a second one — `meta.sticky`, which
pinned a note to the card under it — and it is gone: what it bought was a note
that travelled with a photograph, and what it cost was a drag that took hold of
more than what was pressed, for a reason nothing on screen could explain. A
board that still carries `sticky`, `stuckTo` or the older `loose` keeps them,
untouched and unread, the way `meta` keeps every key it does not know.

**An unknown item `type` and an unknown `meta` key must round-trip untouched.**
That is the format's extension point — it is what let `swatch` and then
`sticker` ship without older builds losing those items. The demonstration board
in `app/demo.rs` carries a card of type `hologram` on purpose; if it ever stops
surviving a save, the extension point has broken.

## Tests

```
cargo test                  # not one of them needs a window
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

## Cutting a release

```
git tag v0.0.1 && git push --tags
```

That builds macOS (Apple Silicon and Intel), Linux and Windows in parallel and
opens a **draft** release, which you publish once the matrix is green. On it:
a `.dmg` per Mac architecture, an installer and a portable `.exe` for Windows,
and an AppImage, a `.deb`, an `.rpm` and a tarball for Linux — plus a signed
manifest the app reads to update itself.

Pressing the button on `Release` in the Actions tab instead builds the same set
and attaches them to the run, without cutting a release — which is how to check
the app starts on a platform you do not own. Packaging is also exercised on
every pull request, on all three platforms, so a broken installer script is
found before a tag rather than after one.

The tag has to match the `version` under `[workspace.package]`. CI stops the
run if it does not.

**Publishing the draft is what ships the update.** The app asks
`releases/latest/download/latest.json`, and a draft is not the latest release.

The whole of it is in `RELEASING.md`, including the signing key and what
happens if it is lost, why Windows cannot be cross-compiled from here, why the
macOS artifact is a bundle rather than a binary, and what "unsigned" costs
whoever you send it to. The design behind it is in `SHIPPING.md`.
