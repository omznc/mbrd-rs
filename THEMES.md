# Themes

mbrd wears **VS Code colour themes**. Not a format that looks like one — the
same one. A `.json` out of any editor extension is a theme here, and a theme
written here opens in that editor.

There are two ways to get one.

## Install one from open-vsx.org

*Settings → Application → Appearance → **More themes** → Browse* opens a search
over [open-vsx.org](https://open-vsx.org), the Eclipse Foundation's open
extension registry. Type, press Enter, arrow to the one you want, press Enter
again. The extension is downloaded, every theme in it is written into your
themes folder, and the two dropdowns above have the new names in them straight
away.

The theme picker — the list that opens when you press either dropdown — has the
same search behind the **+** beside its search field, or behind `Ctrl N`. It
opens over the list rather than in place of it: Escape puts the search away and
leaves you in the list, with whatever you installed now in it.

Desktop only. A browser tab has neither a folder to keep a theme in nor
permission to fetch one.

## Write one, or drop one in

Press **Open folder** on the same row, put a `.json` in it, and press
**Reload**. The folder is printed on that row, and is:

| | |
|---|---|
| Linux | `$XDG_CONFIG_HOME/mbrd/themes` (usually `~/.config/mbrd/themes`) |
| macOS | `~/Library/Application Support/mbrd/themes` |
| Windows | `%APPDATA%\mbrd\themes` |

---

## The shape of a file

One file is one theme.

```json
{
  "name": "Ink",
  "type": "dark",
  "colors": {
    "editor.background": "#0e1014",
    "editor.foreground": "#e6e9ef",
    "focusBorder": "#5a8de0"
  }
}
```

That is a complete theme. Everything else — the cards, the note tints, the
ropes, the hairlines — is worked out from those three.

- **`name`** is what the settings page calls it. Leave it out and the file name
  is used instead.
- **`type`** is `"dark"`, `"light"`, `"hc"` or `"hcLight"`. It decides which of
  the two slots on the settings page your theme can be chosen into. Leave it out
  and mbrd reads the brightness of the background instead, which is right nearly
  always and is why so many themes on the registry get away without it.
- **`colors`** is the VS Code colour map. Every key in it is optional.
- **`include`** is followed inside an extension, which is how a light theme that
  is its dark twin with six colours moved is written. A theme installed from
  open-vsx is flattened on the way in, so the file in your folder never needs it.

Colours are `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`. Comments and trailing
commas are allowed, because the format allows them and most themes use them.

`tokenColors` and `semanticTokenColors` are read past. mbrd has no syntax
highlighting to apply them to. Every other key it does not use is ignored in
silence — a VS Code theme names hundreds of them.

The one thing that is refused is a file that names **no** surface, word or hue
key from the tables below. Borders and shadows do not count: a theme that is
only hairlines is not a theme. A file like that is either not a colour theme at
all or is nothing but syntax highlighting, and loading it would show the
built-in palette wearing your file's name. The settings page says which file,
and why.

### Two keys that are not VS Code's

Both optional, both ignored by every other reader of the format.

| key | what it is |
|---|---|
| `author` | who wrote it. Shown beside the name in the picker |
| `family` | where it came from. Written by the open-vsx install so a theme can be credited to the extension it came out of; a theme you wrote yourself is credited to its file name |

---

## What mbrd reads

In fallback chains: the first key your theme sets wins, and if none of them is
set the last column says what happens instead.

### The board

| mbrd draws | from | if absent |
|---|---|---|
| the board itself | `editor.background`, `editorPane.background`, `tab.activeBackground` | the built-in ground |
| body text | `editor.foreground`, `foreground`, `editorLineNumber.activeForeground` | the built-in text |
| the accent | `focusBorder`, `button.background`, `activityBarBadge.background`, `progressBar.background`, `textLink.foreground`, `editorCursor.foreground` | the built-in accent |

### Surfaces

| mbrd draws | from | if absent |
|---|---|---|
| the chrome — sidebar, menus, panels | `sideBar.background`, `activityBar.background`, `panel.background`, `editorWidget.background`, `menu.background` | a step off the board towards the text |
| a card | `editorWidget.background`, `editorHoverWidget.background`, `input.background`, `dropdown.background`, `editorSuggestWidget.background` | a step off the board |

A card that comes out the same colour as the board is pushed off it anyway. A
great many themes set `editorWidget.background` to `editor.background` exactly,
and a card nobody can see the edge of is not a card.

### Hairlines and faint furniture

Every one of these is taken at a low alpha, because a hairline lifted at full
opacity is a stripe.

| mbrd draws | from | if absent |
|---|---|---|
| the chrome hairline | `sideBar.border`, `panel.border`, `editorWidget.border`, `contrastBorder` | the text colour, faint |
| the card outline | `editorWidget.border`, `input.border`, `widget.border`, `contrastBorder` | the text colour, faint |
| the world axes | `editorRuler.foreground`, `editorIndentGuide.activeBackground1`, `tree.indentGuidesStroke` | the text colour, faint |
| what a floating surface casts | `widget.shadow`, `scrollbar.shadow` | the built-in shadow |

The grid takes its hue from the text and **not** its alpha: that is computed
from the zoom, so a grid stays a grid when you pull back instead of turning
into a texture.

`widget.shadow`'s alpha is a dial. The three shadow sizes carry their own
opacity and multiply it by this one, so `#000000ff` is full strength and
`#00000073` is a little under half. Turn it down for a light theme.

### Words

| mbrd draws | from | if absent |
|---|---|---|
| labels, counts, the status bar | `descriptionForeground`, `editorLineNumber.activeForeground`, `input.placeholderForeground` | the text, moved towards the chrome |
| chevrons and icons beside a count | `icon.foreground`, `editorLineNumber.foreground`, `tab.inactiveForeground` | the text, moved further |
| the accent as a word | `textLink.foreground`, `textLink.activeForeground` | the accent |
| a link inside a note | `textLink.foreground`, `textLink.activeForeground` | the accent |

A code fence inside a note is the accent at a low alpha. A swatch whose hex is
missing or unreadable draws grey, at this theme's own end of the range: a grey
swatch is still a swatch, and `#8c8c8c` is a swatch on a dark board and a smudge
on paper.

### The six hues

| mbrd draws | from |
|---|---|
| a note card, and note pad 1 | `terminal.ansiYellow` |
| an image card | `terminal.ansiCyan` |
| a video card | `terminal.ansiMagenta`, and note pad 4 |
| an audio card | `terminal.ansiGreen`, and note pad 2 |
| a link card | `terminal.ansiBlue`, and note pad 3 |
| a red rope, and a removed line in a diff | `terminal.ansiRed` |

The terminal palette is the one part of a VS Code theme that is a *set of named
colours* rather than a set of surfaces, which is exactly what six card tints and
a four-slot note pad need — and it is already tuned to sit together. Each hue is
washed onto the card colour rather than used at full strength: a tinted card is
a card, not a swatch.

A theme with no terminal palette gets six fixed hues at a fixed, low saturation.
That is duller than a real ANSI set and still tells six kinds of card apart. The
saturation is fixed rather than borrowed from the accent on purpose — an accent
is often the loudest colour in a theme, and six tints borrowing it turn a quiet
palette into a paint chart.

### Diffs

| mbrd draws | from | if absent |
|---|---|---|
| an added line | `gitDecoration.addedResourceForeground`, `charts.green` | the green above |
| a removed line | `gitDecoration.deletedResourceForeground`, `charts.red` | the red above |

### Ropes, anchors and guides

A connection is stored as a **name** — Line, Accent, Warm, Leaf, Danger — and
never as a hex triple, which is what lets a theme change underneath a board that
already exists. Line is the text colour at low alpha; Accent, Warm, Leaf and
Danger are the accent, the yellow, the green and the red. The faint marks beside
a card you point at, and the rules that flash while a drag lines up with a
neighbour, are the text colour at low alpha.

---

## Contrast is enforced, not assumed

This is the part that is not in the format, and it is why a theme picked at
random off a registry lands on a readable board.

- **Anything read as a sentence clears 4.5:1** against every surface it is drawn
  on. Body text, quotes and note links are checked against the card *and* all
  four note pad tints *and* all five card type tints, not merely against the
  chrome — a quote is drawn on a card, never on the chrome behind it.
- **Marks and furniture clear 3:1** against what they sit on: the accent, the
  selection outline, the four rope colours.
- Chevrons and the icons beside a count are the one exemption. They repeat the
  words next to them, so they are held to 3:1 and no higher.

A colour that misses its floor is moved away from its background in small steps
until it clears, towards white on a dark surface and towards black on a light
one. Your hue survives; your unreadable lightness does not.

Nothing about this is optional and nothing about it is a warning. It happens on
load, to built-in themes and downloaded ones alike.

---

## Overriding a built-in

Name a theme the same as one that ships and yours wins. That is the only way to
correct a built-in without waiting for a release. A light and a dark may share a
name — that is the usual way to name a pair — because a theme is identified by
its name *and* its appearance.

## If a theme goes missing

The choice is stored as a **name**, not a palette, so it survives you editing the
file it came from. If you delete that file, mbrd falls back to the built-in and
the settings page says so, keeping the name written down. Put the file back and
your theme returns.

## From the environment

Two variables, for the case where you need the app to be a particular brightness
*before* you can comfortably look at it to change it. Both win over whatever is
saved, so a toggle that disagrees with one says so on its row.

```
MBRD_APPEARANCE=system|light|dark
MBRD_THEME=<name>
```
