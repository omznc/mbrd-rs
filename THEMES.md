# Writing a theme

Every colour mbrd draws with comes from one place, and this is how to replace
it.

Drop a `.json` in your themes folder, press **Reload** on
*Settings → Application → Appearance*, and it appears in the list. The folder
is printed on that same page, and is:

| | |
|---|---|
| Linux | `$XDG_CONFIG_HOME/mbrd/themes` (usually `~/.config/mbrd/themes`) |
| macOS | `~/Library/Application Support/mbrd/themes` |
| Windows | `%APPDATA%\mbrd\themes` |

---

## The shape of a file

One file is a **family**: a name, an author, and however many themes under it.
A light and a dark meant to be worn as a pair belong in one file, which is the
whole reason the array is there.

```json
{
  "name": "Ink",
  "author": "you",
  "themes": [
    {
      "name": "Ink",
      "appearance": "dark",
      "style": {
        "ground": "#0e1014",
        "accent": "#5a8de0",
        "accent_text": "#7fa9ec"
      }
    }
  ]
}
```

`appearance` is `"dark"` or `"light"`. It decides two things: which of the two
slots on the settings page your theme can be chosen into, and — more
importantly — **which built-in palette it inherits from**.

**Every key in `style` is optional.** Anything you leave out comes from the
built-in palette for that appearance. The three keys above are a complete
theme; the other thirty-odd fill themselves in. This is deliberate: a format
where changing an accent means restating the whole palette is one where you
get a black board the first time you miss a key.

Colours are `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`. Alpha matters for a
handful of them — the grid, the axes, the hairlines — and is noted below.

Keys mbrd does not recognise are ignored rather than refused, so a theme
written for a later build still draws in every colour it shares with this one.
**They are named on the settings page anyway**, because a key you misspelled is
ignored in exactly the same silence as one this build predates, and the two are
only tellable apart by being told. A theme with a typo in it still loads.

A *value* that is not a colour is a different matter: it fails the whole theme,
which then shows up on the settings page under the file it was in. Failing
loudly beats a palette with four things moved and thirty not.

---

## Every colour

### The board itself

| key | what it is |
|---|---|
| `ground` | behind the canvas — the paper the board sits on |
| `grid` | the dots of the grid. **Alpha is ignored**: it is computed from the zoom, so a grid stays a grid when you pull back instead of turning into a texture |
| `axis` | the world axes through the origin. Low alpha |
| `guide` | the rules that flash while a drag lines up with a neighbour. Low alpha — this appears over somebody's photographs while their hand is down |

### Furniture

| key | what it is |
|---|---|
| `chrome` | the sidebar, menus, panels, tooltips |
| `chrome_edge` | their hairline. Low alpha |
| `shadow` | what a floating surface casts. **The alpha is a dial**: the three shadow sizes carry their own opacity and multiply it by this one, so `#000000ff` is full strength and `#00000073` is a little under half. Turn it down for a light theme |

### Cards

| key | what it is |
|---|---|
| `card` | a plain card, and the fallback for any card type this build has never heard of |
| `card_edge` | its outline. Low alpha |
| `selected_edge` | the outline of a selected card |
| `note`, `image`, `video`, `audio`, `link`, `fence` | per-type card tints, so a board reads as a board rather than a wall of identical grey rectangles |
| `notes` | **an array of exactly four** — the note pad, `--note-1..4`. A note or a sticker carries which one it was torn from |
| `swatch_fallback` | what a swatch draws as when its hex is missing or unreadable. Grey, not card-coloured: a grey swatch is still a swatch |

### Words

| key | what it is |
|---|---|
| `text` | body text, everywhere |
| `muted` | labels, counts, the status bar, placeholders — anything secondary that is still **read**. Solid, not `text` at low alpha, so a call site can dim it on purpose without something else dimming it by accident |
| `tertiary` | decorative marks that are *not* read as words: a chevron, the icon beside a count. Allowed under the contrast floor. Never put a word in this |
| `quote` | a quote's bar and a rule's line, drawn **on a card** |
| `note_link` | a markdown link, drawn **on a card** |

### The accent

| key | what it is |
|---|---|
| `accent` | the accent as a *fill or an edge* — a selection outline, the wash behind a chosen row, a lit segment |
| `accent_text` | the accent as a *word*. A separate key because the two are held to different floors and one colour usually cannot clear both: an outline needs 3:1 and a sentence needs 4.5:1. Set them the same if your accent is dark enough |

### Connections

`rope_line`, `rope_accent`, `rope_warm`, `rope_leaf`, `rope_danger` — the five
colours a connection may be named. The format stores the *name*, never a hex
triple, which is exactly what lets a theme change underneath an existing
board. `anchor` is the faint marks that appear beside a card you point at.

---

## Making one that is actually readable

The built-in palettes are held to two floors, and both are checked by tests
rather than by eye:

- **`text`, `quote` and `note_link` clear 4.5:1 against every card colour**,
  including all four note tints — not just against the chrome. A quote is
  drawn on a card, never on the chrome behind it, and this is the exact check
  the shipped dark theme was quietly failing before there was a second palette
  to compare it to.
- **`text`, `muted` and `accent_text` clear 4.5:1 against `chrome`.**

`tertiary` is deliberately exempt: it is for marks that repeat the words
beside them.

Nothing enforces this on *your* theme — it is your app — but the numbers are
what the two built-ins were tuned to, and a theme that ignores them is one
where the quotes on a note disappear into the card at some point you will not
be looking.

## Overriding a built-in

Name a theme the same as one that ships and yours wins. That is the only way
to correct a built-in without waiting for a release. A light and a dark may
share a name — that is the usual way to name a pair — because a theme is
identified by its name *and* its appearance.

## If a theme goes missing

The choice is stored as a **name**, not a palette, so it survives you editing
the file it came from. If you delete that file, mbrd falls back to the
built-in and the settings page says so, keeping the name written down — put
the file back and your theme returns.

## From the environment

Two variables, for the case where you need the app to be a particular
brightness *before* you can comfortably look at it to change it. Both win over
whatever is saved, so a toggle that disagrees with one says so on its row.

```
MBRD_APPEARANCE=system|light|dark
MBRD_THEME=<name>
```
