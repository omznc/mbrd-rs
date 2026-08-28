# mbrd — design brief

Everything the app is, everything it draws, and everything a person is expected
to do with it. Written for a designer who has never seen the code.

---

## 1. What it is

**mbrd is an infinite-canvas moodboard, native, written in Rust on
[GPUI](https://gpui.rs) — the same UI framework Zed is built on.** It is a
reimplementation of a browser app of the same name, done natively.

One window. One board open at a time. You pan and zoom around a surface with no
edges, and you drop photographs, videos, audio, notes, colours, links, 3D
models and arbitrary files onto it, join them with ropes, fence them into
groups, tag them, and lay them out.

Three promises shape the whole interface:

1. **There is no save button and no unsaved dot.** Every change is on disk a
   second later. There is no state in which your work exists only in memory.
2. **Everything is undoable, and the undo history survives the file.** Close
   the app, reopen the board, `Ctrl+Z` still walks back through what you did
   last week.
3. **The file travels.** A `.mbrd` is a ZIP with indented JSON, real Markdown
   sidecars you can edit in a text editor, and assets deduped by content hash.
   The format is specified and free to implement.

**Aesthetic target: Zed.** Dense, quiet, keyboard-first, dark by default,
everything reachable by typing. Chrome that gets out of the way of the content.
No decorative surfaces, no gradients, no illustrations.

---

## 2. Constraints the design must respect

These are not preferences — they are how the app is built.

| Constraint | Consequence for design |
|---|---|
| **GPUI, not HTML/CSS.** Flexbox-like layout, no CSS grid, no arbitrary filters, no backdrop-blur | Layouts are flex rows/columns. Effects are: fill, border, radius, three shadow sizes, opacity |
| **The app draws its own titlebar on every platform** | The top 34px is ours. macOS keeps its traffic lights (78px left inset reserved); Windows and client-decorated Linux get our own min/max/close |
| **There can only ever be one full overlay at a time** | Settings, an opened card, the inventory sheet, the palette, the switcher and the context menu are mutually exclusive. No stacking, no modal-over-modal |
| **The board is one painted layer.** Cards carry no event handlers | Card visuals must be paintable primitives: quads, text runs, images. No per-card DOM-like interactivity |
| **Every colour comes from a theme token.** There is no hardcoded colour anywhere in the app | A design that needs a colour needs a *named token*. See §9 |
| **Icons are [Phosphor](https://phosphoricons.com), duotone weight for pictures-of-things, regular weight for shapes** (×, +, ✓, carets, magnifier) | Stay in that set, or say explicitly that the set is changing |
| **A board may hold 20,000 items** | Nothing per-card can be expensive. No thumbnails in list views, no blur, no shadow per card |
| **Reduced motion is a first-class setting** | Every transition needs a defined "instant" fallback that still communicates the same thing |

**Existing measurements** (change them if you have a reason, but know them):
titlebar 34px · status bar 26px · context menu width 216px, rows 26px · palette
and switcher panels 560px wide · icon sizes 12 / 16 / 20px · radii 4 / 6 / 8 /
12px · three shadow sizes.

---

## 3. Window anatomy

```
┌────────────────────────────────────────────────────────────────────────┐
│ [board name ▾] [⌘] [🔍] [⚙]              [update badge]  [─] [□] [×]  │  titlebar 34
├────────────────────────────────────────────────────────────────────────┤
│                                                                        │
│                                                                        │
│                          THE BOARD                                     │
│              (infinite canvas — dot grid, axes,                        │
│               paper outline, cards, ropes, fences)                     │
│                                                                        │
│                                                                        │
│                    ┌──────────────────────────┐                        │
│                    │ ‹  card name   ›   ×     │  tour bar (when on)    │
│                    │    3 of 12               │                        │
│                    └──────────────────────────┘                        │
│  ┌──────────────┐                                                      │
│  │ ▶ ✋ ⎯ 📝  │  tool strip                                            │
├──┴──────────────┴──────────────────────────────────────────────────────┤
│ ⚠ message line …            │ 42 cards │ 8 selected │ 3 in bin │ 100% │  status 26
└────────────────────────────────────────────────────────────────────────┘
```

---

## 4. Surfaces, one by one

### 4.1 Titlebar

Always drawn, on every platform. Three zones:

- **Left group** (after the platform inset): the **board name**, which is also
  the button that opens the board switcher — it wears the app's identity, and
  pressing it is how most people will discover `Ctrl+P`. Then three wordless
  icon buttons: **Commands** (⌘ mark), **Find** (list-magnifier), **Settings**
  (gear). Each has a tooltip naming it *and* its shortcut.
- **Middle**: empty. Dragging it moves the window; double-click maximises.
- **Right**: the **update badge**, then window controls where they are ours.

**Update badge states** (needs design — currently minimal):
`Available "v0.4.0"` → `Downloading 43%` → `Ready — restart to update`. It is a
button at every stage. It must be quiet enough to ignore for a week.

### 4.2 The board

What is drawn behind the cards:

- **Dot grid.** Alpha is computed from zoom so a grid stays a grid when you pull
  back instead of turning into a texture. Coarsens automatically past 40,000
  dots. Toggle `G`.
- **World axes** through the origin. Toggle `X`.
- **Alignment guides** — flash while a drag lines up with a neighbour's edge or
  centre. Low alpha, drawn over somebody's photographs while their hand is down.
- **Paper outline** — an optional sheet (A3–A6, Letter, Legal, Tabloid, or none)
  drawn around the origin, portrait or landscape. This is what makes the board
  mean something printable.
- **Scale bar** (off by default) — a nice round real-world length, metric or
  imperial.
- **Marquee** — a selection rectangle while `Shift`/`Ctrl` + dragging empty space.

### 4.3 Cards

**Fifteen types**, each with its own tint token: `image`, `video`, `audio`,
`note`, `link`, `text`, `model` (3D mesh), `swatch` (a colour), `sticker`,
`fence` (a group frame), `title`, `ghost` (onboarding furniture), `style-tile`,
`gone` (what's left after the bin was emptied), `generic`, plus `other` — a type
written by a build that isn't this one, carried through untouched and drawn as a
question mark.

A card is a **thumbnail of something**. It is a few hundred pixels wide and
there may be twenty thousand of them.

**What a card can wear:**

| Affordance | Behaviour |
|---|---|
| **8 resize grips** | On the outline. 8px in *screen* space at every zoom. Corners win ties |
| **4 rope anchors** | Faint marks *outside* the card, appearing on hover. Drag one to another card to join them. Deliberately further out than the grips so one press can't mean two things |
| **Transport strip** | On video and audio cards, *inside* the bottom edge: play/pause, scrub, loop, mute. Controls drop off in a fixed order as the card gets narrower — never hidden wholesale |
| **Padlock** | Top corner of a locked card. Refuses every drag and handle; layouts route around it |
| **Tag chips** | The words somebody put on it. Max 24 chars each, ~12 per card |
| **Tint** | Notes and stickers carry one of four note-pad colours; `T` cycles |
| **Selection outline** | Its own token, distinct from the card edge |
| **Fade** | A card filtered out by a tag filter is faded, not hidden |

**Notes render real Markdown on the card** — headings, bold, italic, quotes,
rules, links, lists — wrapped and elided to the rows the card has room for.

**Media fit** is per-board with a per-card override: `contain` (the whole
picture with margins) or `cover` (the whole card with crops).

### 4.4 Ropes (connections)

Curves between cards that **route around whatever is in the way** — an elbow is
the exception, a plain curve is the ordinary answer. Nothing re-routes while
anything is moving; the pass runs when the hand comes off.

Four properties, all on the right-click menu: **colour** (Plain / Accent / Warm
/ Leaf / Danger — stored as a *name*, so a theme change recolours existing
boards), **arrows** (none / forward / back / both), **style** (solid / dashed /
dotted), **weight** (fine / normal / bold). Plus a **label** — up to 60
characters, typed onto the middle of the rope.

Toggle all ropes with `W`.

### 4.5 Fences (groups)

A frame around cards. `Ctrl+G` groups the selection, `Ctrl+Shift+G` dissolves.
**Double-clicking a fence steps *inside* it**, so presses reach what's in it;
`Esc` steps back out. That inside/outside state needs a visual — right now it is
barely expressed.

### 4.6 Tool strip (bottom left)

Four tools: **Select** (default), **Pan**, **Connect**, **Note**. Keys `1`–`4`
or `V` / `H` / `C`.

A tool is the one *mode* in an otherwise modeless app, and it earns that by
paying twice: it makes gestures that were hidden behind a modifier visible, and
it makes repeated ones repeatable — drawing nine ropes shouldn't mean nine trips
to a card's edge. **No tool is the only way to do anything**; everything is also
possible from Select.

### 4.7 Status bar (bottom, 26px)

Left: **one message line**, in one of four tones —

- `Done` — something finished and you watched it. **Not drawn at all.**
- `Wrong` — something failed. Warning mark.
- `Told` — something happened out of sight. Info mark.
- `Mode` — where you are rather than what happened. Keyboard mark. **Stands
  until replaced**, and always names the key that leaves the mode.

Right: counts, each omitted entirely when zero — cards, pictures, selected, in
the bin, the standing tag filter (named tags while they fit) — then the zoom
percentage. Separated by hairline rules.

### 4.8 Tour bar

A tour is the board read as an ordered sequence of stops. When one is running, a
floating pill sits above the status bar: `‹` · **stop name** / "3 of 12" · `›` ·
`×`. The ends grey out rather than wrap. The camera flies to each stop.

### 4.9 Context menu (right-click)

Drawn by the app, not the platform, so it is themed like everything else. 216px
wide, 26px rows, rules between sections, submenus.

**It shows only what applies to whatever you pressed** — a rope's menu is about
ropes, a card's about that card, empty paper's about the board — and leaves out
what does not apply. It is fitted inside the window rather than allowed to spill:
too tall becomes scrollable, too wide becomes narrower. A window resize closes it.

Every row is a `Command`, so its label and its keystroke can never drift from
what the keyboard actually does.

### 4.10 The palette (`Shift Shift`, `Ctrl+F`, `#`)

One panel, 560px, four modes:

| Mode | Opened by | Lists | Enter does |
|---|---|---|---|
| **Commands** | `Shift Shift` | Every command in the app, ~90 of them | Runs it |
| **Search** | `Ctrl+F` / `Ctrl+K` | Every card on the board, by name and content | **Flies the camera to it** |
| **Tag** | `#` | Every tag, plus "coin a new one" | Tags/untags the selection |
| **Filter** | menu | Every tag with its count | Adds/removes it from the standing filter |

Fuzzy-matched, with the matched characters highlighted. Rows show an icon for
the card type, and the keystroke on the right for commands.

**Crucial detail:** in the command palette, a command that would achieve nothing
right now is **dimmed, not hidden** — the opposite of the context menu. Dimmed
says "yes, and not right now"; absent says "no such thing". The palette is often
how you find out *whether* a thing exists.

Search results moving the camera is the load-bearing feature on a canvas with no
edges. Being told a card exists is no use if you then have to fly there by hand.

### 4.11 Board switcher (`Ctrl+P`, or the board name)

The same 560px panel shape. Lists boards you've had open (24 remembered), plus
every `.mbrd` sitting next to the open one and in the launch directory.

It is also where boards are **made** and **deleted**. Deleting asks first — it
is the only thing in the app that cannot be undone by doing it again.

### 4.12 Settings page (`Ctrl+,`)

Full-window overlay. Two-level sidebar + a column of rows. Each row is a **name,
a sentence under it, and its control at the far edge**. The sentence is the
point — "a switch called *Axes* tells you nothing at 2am".

A **search field** flattens both levels into one list, matching titles *and*
descriptions.

```
Board          ← travels in the .mbrd, undoable
  Canvas       Grid · Snap to grid · Axes · Connections · Alignment guides · Grid step (32/48/64/96/128)
  Arranging    Card gap (0/4/8/12/16/24/32)
  Media        Media fit (contain / cover)

Application    ← about this computer, never saved into a file you send
  General      Animation
  Appearance   Appearance (System/Light/Dark) · Dark theme ▾ · Light theme ▾ · Themes folder [Reload]
  Updates      Look for new versions · [Check now / Download / Restart to update]
```

**The Board/Application split is the page's spine** and must stay legible: Board
rows go into the file and undo; Application rows are about the person sitting
here and do neither.

Rows carry state that needs design: a setting pinned by an environment variable
says *"Set by MBRD_THEME, which wins at startup"* instead of its description. A
theme name that can't be found says so without pretending the fallback was your
choice.

Controls in use: toggle switch, segmented control, dropdown (wearing the
two-caret mark), button. The theme dropdown opens a **searchable picker**, not a
popup list.

### 4.13 The opened card (double-click, or `O`)

Full window below the titlebar. The board is *gone*, not dimmed.

```
┌ icon  title                                  [Edit] [i] [×] ┐
│ type · 2.4 MB · 1920×1080                                   │
├──────────────────────────────────────────┬──────────────────┤
│   PREVIEW  or  SOURCE                    │   INFO RAIL      │
└──────────────────────────────────────────┴──────────────────┘
```

**The header is identical for every type — that is the point.** A gesture that
opens a note and does nothing to a `.zip` is a gesture nobody trusts. Three
rules:

1. **The gesture works on everything.** A zip opens too, onto what is genuinely
   known: a name, a size, a hash, and a list of what's inside.
2. **Anything that can be shown is shown.** A grey rectangle over bytes we could
   have read is a bug, not a level of detail.
3. **Anything that can be changed is changeable.** The Edit button is *never*
   greyed out. Every card has at least a name.

**Preview kinds:** Document (Markdown, set as a page) · Source (fixed-width,
language-aware) · Sheet (CSV/TSV as a real grid) · Picture (contained) · Vector
(SVG, rasterised) · Video · Audio (waveform) · Colour (a swatch) · Address (a
link) · Archive (a listing) · Nothing (the rail is the whole page).

Some bodies **scroll** (document, source, table, listing); some are **fitted to
the window and never scroll** (picture, colour, poster frame). An image you have
to scroll is an image the page failed to show you.

**The info rail** slides in *beside* the preview, not over it — looking up a
photograph's dimensions shouldn't mean not looking at the photograph. It lists
only facts that are actually known: **no "Duration: —", no empty "Artist"**. A
rail of blanks reads as a form somebody failed to fill in.

Files are named properly: 1,367 extensions across 102 families in 12 categories,
so a `.sldprt` reads "SolidWorks part" rather than "file".

### 4.14 Inventory sheet

Full-window page. **What this board is made of and what it weighs** — because a
`.mbrd` gets heavy and nothing else in the app says why.

Rows are *files* by content hash: name, kind, size, and how many cards use it.
**A row with a card behind it is a button that travels there.** A row with none
is an orphan and is not a button at all. Orphans are reported, never offered for
deletion.

No thumbnails, deliberately — a page that decoded pictures to measure them would
stall at the exact moment the question was asked.

---

## 5. What we expect the user to do

### First five minutes
Open the app → land on a demonstration board with a few cards and a note that
says how to move → pan by dragging, zoom on the wheel → drop a folder of images
onto the window → watch cards land in batches → press `F` to fit everything →
`N` to write a note → `Ctrl+P`, name a new board, start for real.

### The daily loop
Drag images in from a browser or a Finder window. Arrange by hand, or press an
arrangement (Spiral, Free, Grid, Masonry, by Type, by Tag, by Date, Scatter) and
let the engine lay everything out. Nudge with arrows. Align and distribute the
selection. Group related things into a fence. Draw ropes between the things that
explain each other and label them. Tag by theme. Filter down to one tag when the
board gets crowded. Double-click anything to actually look at it.

### The keyboard is the primary interface
The full table (this is also the README's controls section):

| Gesture | Does |
|---|---|
| drag empty space | pan |
| wheel / `Shift`+wheel | zoom to cursor / pan sideways |
| middle-drag | pan from anywhere, even over a card |
| click empty space | deselect (`Ctrl+Z` puts it back) |
| `Shift`/`Ctrl` + drag empty space | marquee-select |
| drag a card | move it and everything selected with it; guides line it up |
| `Shift` mid-drag | pin the move to one axis |
| `Alt` + drag a card | leave a copy behind |
| drag a corner/edge | resize; a picture keeps its proportions |
| `Ctrl` + drag a handle | resize a picture freely |
| `Alt` + drag a handle | crop — the picture fills the card and the card cuts it |
| arrows / `Shift`+arrows | nudge / a whole grid step |
| `0` / `F` | recenter / fit everything |
| `Ctrl+A` / `Esc` / `Del` | select all / clear / move to bin |
| right-click | everything else, next to what it acts on |
| double-click a card, or `O` | open it on the whole window |
| `F2` / `Enter` | type into a card without leaving the board |
| `N` / `K` / `E` | new note / new colour / new fence |
| `T` | next tint, on a note |
| `Ctrl+G` / `Ctrl+Shift+G` | group into a fence / dissolve |
| double-click a fence | step inside; `Esc` steps out |
| hover a card, drag a mark | pull a rope to another card |
| `J` | join the selection with the shortest set of ropes |
| double-click a rope, or `F2` | label it |
| `L` | lock / unlock |
| `W` | show/hide ropes |
| `1`–`4`, `V`/`H`/`C` | select / pan / connect / note tool |
| `Ctrl+C` / `Ctrl+X` / `Ctrl+D` | copy / cut / duplicate |
| `[` / `]` | send to back / bring to front |
| `G` / `X` / `S` | grid / axes / snapping |
| `#` | tag the selection |
| drop a file or folder, `Ctrl+V` | put it on the board |
| `Ctrl+Shift+V` | paste as a link instead of fetching it |
| `Ctrl+Z` / `Ctrl+Shift+Z` | undo / redo |
| `Ctrl+S` | save now, and say so |
| `Ctrl+N` | new board in `~/mbrd` |
| `Ctrl+P` | open another board |
| `Ctrl+F` / `Ctrl+K` | find something on the board and go to it |
| `Shift Shift` | every command there is |
| `Ctrl+,` | settings |
| `Ctrl+U` | check for a new version, then install it |
| `Space` | play/pause the media card under the pointer |

### Things that arrive from outside
Dropping a **folder** of three hundred photographs is normal, and reading them
takes seconds — so cards land **in batches as they arrive**, with progress
somewhere visible and a way to stop. **Pasting a URL** to an MP4 fetches it and
makes it a video card; pasting a URL to a web page makes a link card. A file too
large is *reported*, never silently refused or silently accepted.

---

## 6. Where design is most needed

Ranked. These are the places the app is functionally complete and visually thin.

1. **The welcome / first-run screen — does not exist at all.** See §8.
2. **Import progress.** A folder drop is the app's longest wait and has almost
   no visual language for it.
3. **The update badge and its four states.**
4. **Fence "stepped inside" state.** A real mode with almost no expression.
5. **Empty states.** A brand-new board is a blank grid with nothing to press.
6. **Tag chips on cards, and the standing-filter state.** Currently text.
7. **The opened page's info rail and Edit affordance.**
8. **The inventory sheet's rows** — the one page in the app that is pure data.
9. **Error and warning presentation.** One status line is all there is.
10. **The four tone marks** — Wrong / Told / Mode need to be distinguishable at
    a glance without reading.

**Known-incomplete features** (design around them, or design *for* them):
video and audio decode but do not yet play (poster frames and waveforms only);
rotation round-trips in the file but is not drawn; the scale-calibration mode
(drawing a line along something whose real size you know) is built in the view
but no command reaches it yet; there is no rich text beyond Markdown, no sticker
shapes, and no input-method support for composing keyboards.

---

## 7. Theme tokens — the design system

Every colour in the app is one of these, and a theme file may override any of
them. A design that needs a new colour needs a new **named token** here.

**Board:** `ground` (the paper the board sits on) · `grid` (alpha computed from
zoom) · `axis` · `guide`

**Furniture:** `chrome` (sidebar, menus, panels, tooltips) · `chrome_edge` (their
hairline) · `shadow` (an opacity dial — the three shadow sizes multiply by it)

**Cards:** `card` · `card_edge` · `selected_edge` · `note` `image` `video`
`audio` `link` `fence` (per-type tints, so a board reads as a board and not a
wall of grey) · `notes` (an array of exactly four note-pad colours) ·
`swatch_fallback`

**Words:** `text` (body) · `muted` (labels, counts, status bar, placeholders —
**solid, not `text` at low alpha**, so a call site can dim on purpose) ·
`tertiary` (decorative marks that are *not read as words* — a chevron, an icon
beside a count; allowed under the contrast floor; **never put a word in this**) ·
`quote` · `note_link`

**Accent:** `accent` (as a fill or an edge — 3:1 floor) and `accent_text` (as a
word — 4.5:1 floor). Two tokens because one colour usually can't clear both.

**Connections:** `rope_line` `rope_accent` `rope_warm` `rope_leaf` `rope_danger`
· `anchor`

Themes ship as **families** — a name, an author, and an array of themes, so a
light and a dark meant to be worn as a pair live in one file. Every key is
optional and inherits from the built-in palette for its appearance. Users drop
a `.json` in their themes folder and press Reload.

Two are built in: a warm dark and the same board on paper.

---

## 8. The welcome screen — what to design

**This does not exist yet.** It is a new surface, and here is the spec.

### When it appears
On **first run only** — determined by there being no preferences file in the
config directory. Never again unless reached deliberately (a "Run setup again"
row in Settings → General). It must be **skippable in one press**; every choice
it makes has a working default already.

### Where it sits
The same full-window overlay class as Settings and the opened card — below the
titlebar, board behind it gone rather than dimmed. It is the app, not a dialog
over the app.

### What it asks

**Step 1 — Appearance.** The single most visible choice, so it goes first and it
previews live.
- `System` / `Light` / `Dark` (three values, one question: *what decides?*)
- The **dark theme** and the **light theme** by name, both shown regardless of
  which is currently worn — the pair is chosen once and then followed, so the
  half you aren't looking at has to be reachable while you aren't looking at it.
- A note pointing at the themes folder for people who want their own.
- **Design opportunity:** this is the one place a theme preview is worth real
  space. A miniature board — a few cards, a rope, the grid — recoloured live.

**Step 2 — Where your boards live.**
- A path, defaulting to `~/mbrd`, with a Browse button.
- *Engineering note: `dirs::boards()` currently hardcodes `~/mbrd`. Making this
  choosable needs a new preference key. Flag it in the design.*
- Explain in one sentence what goes there: `Ctrl+N` makes a board here, and this
  is where the switcher looks first.

**Step 3 — How it should behave.** A short list of the settings people actually
have opinions about:
- **Animation** — "Let the interface move. Turn off to land every change
  instantly." (Currently **off** by default, deliberately: a board is a tool
  you're inside all day and every settle is a wait.)
- **Look for new versions** — "Check quietly at startup and say so in the top
  bar." (On by default. Off stops the request being made at all, not just the
  message being shown.)
- **Snap to grid**, **Grid**, **Grid step** — *these are board settings, not app
  settings*. To ask them here they must become **defaults for new boards**,
  which is a new preferences key and a real design decision: the Board /
  Application split is load-bearing elsewhere and mustn't be muddied. Either ask
  them and label them clearly as defaults, or leave them out. **Recommend
  labelling them "Defaults for new boards".**

**Step 4 — Get started.** Three doors, not a "Finish" button:
- **Create a board** (opens a name field, lands you on an empty board)
- **Open a board** (the switcher, or a file picker)
- **Look around the demo board** (what the app currently opens on: a note
  explaining the gestures, two photographs, a video card, an audio card)
- Optionally: **Take the tour** — the tour machinery already exists, so a guided
  walk of the demo board is nearly free.

### Rules it must follow
- **Nothing here is a second implementation of a settings row.** Whatever the
  welcome screen shows, Settings shows the same control with the same
  description. Two copies drift.
- **Every step is skippable and every choice is changeable later**, and the
  screen should say so once, quietly, rather than per-step.
- **No progress bar theatre.** Four steps is a short list, not a wizard. A
  stepper or a single scrolling page are both fine; a five-minute onboarding
  flow is not.
- **It must survive being the very first thing anybody sees on any theme**,
  including a light desktop where the app is about to draw dark.

---

## 9. One-paragraph summary for the designer

A native, keyboard-first infinite moodboard in the Zed idiom: dark by default,
themeable down to the last hairline, dense chrome that gets out of the way, and
everything reachable by typing. Fifteen kinds of card on a canvas with no edges,
joined by routed ropes, fenced into groups, tagged and filtered, laid out by an
arrangement engine, and openable one at a time onto a full-window page that
shows the actual thing. One titlebar, one status bar, one tool strip, and
exactly one overlay at a time. No save button, because there is nothing to save.
What is missing is a first-run experience, a visual language for waiting and for
error, and polish on the three full-window pages — settings, the opened card,
and the inventory sheet.
