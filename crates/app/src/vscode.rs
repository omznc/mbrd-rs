//! A VS Code colour theme, read as a board.
//!
//! This is the theme format now — `assets/themes` is written in it, the files
//! in `dirs::themes()` are written in it, and everything on open-vsx.org is
//! already written in it. One file is one theme:
//!
//! ```json
//! {
//!   "name": "Ember",
//!   "type": "dark",
//!   "colors": { "editor.background": "#141110", "focusBorder": "#e0762f" }
//! }
//! ```
//!
//! ## Why somebody else's format
//!
//! Because the alternative was asking people to write a second one. A theme is
//! not a thing anybody wants to author twice, and there are thousands of these
//! already made, tuned and looked at for years — so the useful question was
//! never "what keys should our themes have", it was "what does a board look
//! like when an editor's palette is put on it".
//!
//! The cost is that VS Code has no key for a card, a note tint, a rope or a
//! guide, and never will: it is an editor, and none of those are things an
//! editor has. So this module is a *translation* rather than a rename. Some of
//! [`Theme`]'s thirty-four colours are lifted straight across from a key that
//! means the same thing — `ground` is `editor.background`, and there is no
//! second opinion to have about that — and the rest are derived from the ones
//! that were, by [`tint`], [`toward`] and [`readable`] below.
//!
//! ## Missing keys are the normal case, not the error case
//!
//! A theme on open-vsx may name six hundred colours or eleven. Both have to
//! land on a board with thirty-four colours on it and none of them missing, so
//! every lookup here is a *chain* — see [`Palette::pick`] — ending in
//! something computed rather than in `None`. There is no such thing as a VS
//! Code theme this module refuses for being too small. `editor.background` and
//! `editor.foreground` are the two it would rather have, and it invents even
//! those from [`Theme::dark`] and [`Theme::light`] when they are absent.
//!
//! For the same reason nothing here reports an unknown key. Under the old
//! format, a key this build had no colour for was worth saying out loud
//! because there were forty of them and a misspelling hid among them; a VS
//! Code theme names hundreds that are simply about an editor, and a settings
//! page listing `editorBracketMatch.border` as unrecognised would be noise
//! with one real complaint buried in it. `themes.rs` says the one thing that
//! is still actionable instead: a file that names *nothing* this build reads.
//!
//! ## The floors are enforced here
//!
//! `theme.rs` holds its two built-in palettes to a contrast floor and tests
//! the claim. That was a promise about colours we chose. It cannot be a
//! promise about colours somebody else chose — a theme built for an editor has
//! never been asked whether its foreground reads on a note tint, because it
//! has no note tints. So the derivation *makes* it true: [`readable`] walks
//! `text`, `quote`, `note_link`, `muted` and `accent_text` away from the
//! background they are drawn on until they clear the floor, and the tints they
//! are measured against are the ones this module just computed. A theme
//! installed from a marketplace lands on a readable board or it does not land.
//!
//! ## What is thrown away
//!
//! `tokenColors` and `semanticTokenColors`, which are the larger half of most
//! theme files. They colour syntax inside an editor; the only code on a board
//! is a fenced block on a note, drawn in one colour. Read but ignored is the
//! honest description — a file that is nothing but `tokenColors` has no
//! opinion about a board and is refused as such.

use std::collections::HashMap;

use gpui::{hsla, hsla_to_rgba, rgb_to_hsla, Hsla};
use serde::Deserialize;
use serde_json::{Map, Value};

use crate::theme::Theme;
use crate::themes::Appearance;

/// A theme file, as far as this app is concerned.
///
/// Four fields out of a format with a dozen. `$schema`, `semanticHighlighting`
/// and the two token maps are all read and dropped — see the module note — and
/// `include` is handled a level up in [`resolve`], because following it needs
/// the other files and this struct only has the one.
#[derive(Debug, Deserialize)]
pub struct File {
    /// The theme's name, as it appears in a list.
    ///
    /// Optional here and required by the time a theme is in the registry: a
    /// theme file inside an extension usually leaves this out, because the
    /// name it is shown under lives in `package.json` beside the path to it.
    /// `themes.rs` and `openvsx.rs` both have a better name than this to fall
    /// back on — a file name and a `contributes.themes[].label` — and neither
    /// of them can supply it here.
    #[serde(default)]
    pub name: Option<String>,
    /// `dark`, `light`, `hc` or `hcLight`. Absent in more files than you would
    /// expect, which is what [`Appearance`]'s fallback below is for.
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    /// Who wrote it. **Not a VS Code key**, and the one thing here that is
    /// ours: the format has no author field because an extension's
    /// `package.json` carries the publisher, and a bare `.json` in somebody's
    /// themes folder has no `package.json`. Unknown keys are ignored by every
    /// reader of this format including VS Code's own, so a theme carrying it
    /// is still a theme anywhere else.
    #[serde(default)]
    pub author: Option<String>,
    /// Where it came from, as a person would say it. **Not a VS Code key**
    /// either, and written by `openvsx.rs` rather than by anybody by hand: a
    /// theme installed from an extension is credited to the extension, and
    /// the extension's display name is nowhere in the theme file. A theme
    /// somebody wrote themselves leaves this out and is credited to its file
    /// name instead. See `themes::Named::family`.
    #[serde(default)]
    pub family: Option<String>,
    /// Another file to start from, relative to this one. Extensions use it to
    /// ship a dark and a light that differ by six colours. Only followed when
    /// there is something to follow it with; see [`resolve`].
    #[serde(default)]
    pub include: Option<String>,
    #[serde(default)]
    pub colors: Map<String, Value>,
}

/// Read one file. JSONC, because that is what these files are.
///
/// The error is a sentence, for the reason `color.rs` gives about its own:
/// nothing matches on it, and it ends up on the settings page under the name
/// of the file it is about.
pub fn parse(text: &str) -> Result<File, String> {
    serde_json::from_str::<File>(&strip(text)).map_err(|e| format!("is not a colour theme: {e}"))
}

/// Lay an `include` chain flat, nearest file last.
///
/// `files` is every theme file in the same extension, keyed by the path each
/// one is at, which is the only form the relative `include` can be resolved
/// against. A cycle, or an include naming a file that is not there, stops the
/// walk rather than failing it — an extension with a broken include still has
/// the colours in the file that was actually asked for, and refusing those to
/// be strict about a link would be refusing the theme somebody chose.
pub fn resolve(path: &str, files: &HashMap<String, String>) -> Result<File, String> {
    let mut chain = Vec::new();
    let mut at = path.to_string();
    // Six, which is five more than any real extension uses. The number is not
    // a limit anybody will reach — it is what stops a file that includes
    // itself from being an infinite loop inside a settings page.
    for _ in 0..6 {
        let Some(text) = files.get(&at) else { break };
        let file = parse(text)?;
        let next = file.include.clone();
        chain.push(file);
        let Some(next) = next else { break };
        at = join(&at, &next);
        if chain.len() > 1 && at == path {
            break;
        }
    }
    let Some(mut out) = chain.pop() else {
        return Err("is not in this extension".into());
    };
    // Everything the chain named, with the file that did the including having
    // the last word — which is the direction VS Code merges, and the only one
    // that makes "a light theme that is the dark one with six colours moved"
    // work at all.
    for outer in chain.into_iter().rev() {
        for (key, value) in outer.colors {
            out.colors.insert(key, value);
        }
        out.name = out.name.or(outer.name);
        out.kind = out.kind.or(outer.kind);
    }
    Ok(out)
}

/// A relative path against the file that named it, the little of it these
/// files use: `./dark.json`, `../themes/dark.json`, `dark.json`.
fn join(from: &str, rel: &str) -> String {
    let mut parts: Vec<&str> = from.split('/').collect();
    parts.pop();
    for step in rel.split('/') {
        match step {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            name => parts.push(name),
        }
    }
    parts.join("/")
}

/// JSONC to JSON: comments out, trailing commas out.
///
/// Needed rather than fastidious. VS Code's own schema allows both, its
/// documentation is written in files that use both, and a large share of what
/// is on open-vsx has a licence header at the top of the theme file. A reader
/// that refused those would refuse them for a reason nobody outside this
/// codebase considers a reason.
///
/// A single pass with one piece of state: whether it is inside a string. That
/// is enough, because the only thing that can hide a `//` is a string literal
/// and the only thing that can hide a quote is a backslash.
pub fn strip(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_string = false;
    while let Some(c) = chars.next() {
        if in_string {
            out.push(c);
            match c {
                '\\' => {
                    if let Some(escaped) = chars.next() {
                        out.push(escaped);
                    }
                }
                '"' => in_string = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => {
                in_string = true;
                out.push(c);
            }
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                let mut last = '\0';
                for c in chars.by_ref() {
                    if last == '*' && c == '/' {
                        break;
                    }
                    last = c;
                }
                // A space, not nothing: `1/**/2` is two tokens and `12` is one.
                out.push(' ');
            }
            _ => out.push(c),
        }
    }
    trailing_commas(&out)
}

/// The other half of JSONC. A comma whose next non-space character closes the
/// thing it is in was never separating two of anything.
fn trailing_commas(text: &str) -> String {
    let bytes: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut at = 0;
    while at < bytes.len() {
        let c = bytes[at];
        if in_string {
            out.push(c);
            if c == '\\' {
                if let Some(&next) = bytes.get(at + 1) {
                    out.push(next);
                    at += 2;
                    continue;
                }
            } else if c == '"' {
                in_string = false;
            }
            at += 1;
            continue;
        }
        if c == '"' {
            in_string = true;
            out.push(c);
            at += 1;
            continue;
        }
        if c == ',' {
            let closes = bytes[at + 1..]
                .iter()
                .find(|c| !c.is_whitespace())
                .is_some_and(|&c| c == '}' || c == ']');
            if closes {
                at += 1;
                continue;
            }
        }
        out.push(c);
        at += 1;
    }
    out
}

// ---------------------------------------------------------------------------
// The palette
// ---------------------------------------------------------------------------

/// A theme's `colors`, with the non-colours already dropped.
///
/// VS Code allows `null` for a key, meaning "unset this one", and a theme that
/// includes another uses it. A `null` that reached [`Palette::pick`] as a
/// value would be a colour that is present and unusable, so it is dropped here
/// where "present" is decided.
struct Palette {
    colors: HashMap<String, Hsla>,
}

impl Palette {
    fn of(colors: &Map<String, Value>) -> Self {
        let colors = colors
            .iter()
            .filter_map(|(key, value)| {
                let text = value.as_str()?;
                let color = crate::color::read(text).ok()?;
                Some((key.clone(), color))
            })
            .collect();
        Self { colors }
    }

    /// The first of these keys the theme has an opinion about.
    ///
    /// A chain rather than a key, everywhere, because the alternative is a
    /// board with a hole in it whenever a theme author did not happen to
    /// colour the one surface this app decided to name it after. The order is
    /// the argument: `chrome` asks about the sidebar first because a sidebar
    /// is what the chrome *is*, and falls back through the other panels a
    /// theme might have coloured instead before giving up and deriving one.
    fn pick(&self, keys: &[&str]) -> Option<Hsla> {
        keys.iter().find_map(|key| self.colors.get(*key).copied())
    }

    /// The same, flattened onto a background.
    ///
    /// Most of what a theme colours in an editor is a translucent wash over
    /// the editor's own background — `editor.selectionBackground` is a good
    /// example, and so is nearly every border. Those are perfectly good
    /// colours to take, and completely wrong to take at their own alpha: a
    /// card drawn at 12% opacity is a hole in the board. So a colour taken for
    /// a *surface* is composited onto the surface behind it first, and comes
    /// out opaque.
    fn solid(&self, keys: &[&str], on: Hsla) -> Option<Hsla> {
        self.pick(keys).map(|c| over(c, on))
    }
}

// ---------------------------------------------------------------------------
// Colour arithmetic
// ---------------------------------------------------------------------------

/// One colour composited onto an opaque one.
fn over(top: Hsla, bottom: Hsla) -> Hsla {
    let (t, b) = (hsla_to_rgba(top), hsla_to_rgba(bottom));
    let a = t.alpha.clamp(0.0, 1.0);
    rgb_to_hsla(gpui::Rgba::new(
        t.red * a + b.red * (1.0 - a),
        t.green * a + b.green * (1.0 - a),
        t.blue * a + b.blue * (1.0 - a),
        1.0,
    ))
}

/// A step from one colour towards another, in straight sRGB and keeping the
/// first one's alpha.
///
/// sRGB rather than a perceptual space on purpose: every value this is used
/// against came out of a hex string somebody chose by eye, and the point is a
/// blend that lands where they would expect it to rather than one that is
/// theoretically better and visibly elsewhere.
fn toward(from: Hsla, to: Hsla, amount: f32) -> Hsla {
    let (f, t) = (hsla_to_rgba(from), hsla_to_rgba(to));
    let amount = amount.clamp(0.0, 1.0);
    let mix = |a: f32, b: f32| a + (b - a) * amount;
    let mut out = rgb_to_hsla(gpui::Rgba::new(
        mix(f.red, t.red),
        mix(f.green, t.green),
        mix(f.blue, t.blue),
        1.0,
    ));
    out.alpha = from.alpha;
    out
}

/// The same colour at a given alpha, which is how every hairline, axis, grid
/// dot and rope in this app is made out of the text colour.
fn faint(color: Hsla, alpha: f32) -> Hsla {
    Hsla { alpha, ..color }
}

/// WCAG relative luminance.
fn luminance(color: Hsla) -> f32 {
    let rgba = hsla_to_rgba(color);
    let channel = |v: f32| {
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(rgba.red) + 0.7152 * channel(rgba.green) + 0.0722 * channel(rgba.blue)
}

/// WCAG contrast ratio, which is what the two floors in `theme.rs` are counted
/// in and therefore what this module has to be able to count in too.
fn contrast(a: Hsla, b: Hsla) -> f32 {
    let (a, b) = (luminance(a), luminance(b));
    let (hi, lo) = if a > b { (a, b) } else { (b, a) };
    (hi + 0.05) / (lo + 0.05)
}

/// A colour moved until it can be read on a background.
///
/// **The one place this module overrules the theme.** A word that cannot be
/// read is not a style choice that happens to be bold — it is a card whose
/// text is not there — and the app has thirty-four colours to make out of a
/// file that was never asked about most of them, so this is the difference
/// between "a marketplace theme works here" and "a marketplace theme works
/// here if you are lucky".
///
/// It moves the *foreground*, never the background, and it moves it away from
/// that background rather than to black or white: a dim blue on a dark ground
/// comes out a bright blue, not a white. Twenty steps of five percent, which
/// is fine enough that a colour that was already close moves imperceptibly and
/// coarse enough that one that was hopeless gets all the way there.
fn readable(color: Hsla, on: Hsla, floor: f32) -> Hsla {
    if contrast(color, on) >= floor {
        return color;
    }
    // Away from the background: towards white if the background is dark,
    // towards black if it is light. Judged on the background rather than on
    // the colour, because the colour is the thing that is failing and its own
    // lightness is not evidence about which direction has room in it.
    let target =
        if luminance(on) < 0.18 { hsla(0.0, 0.0, 1.0, 1.0) } else { hsla(0.0, 0.0, 0.0, 1.0) };
    let mut out = color;
    for step in 1..=20 {
        out = toward(color, target, step as f32 * 0.05);
        if contrast(out, on) >= floor {
            return out;
        }
    }
    out
}

/// A wash of one hue over a surface: how every card tint and note tint is
/// made.
///
/// Hue and saturation from the source, lightness from the *surface* — not a
/// blend of the two colours. A blend is what this was first, and it is wrong
/// in one direction: on a light theme the marketplace's `terminal.ansiYellow`
/// is a dark mustard, and a dark mustard blended into near-white paper at any
/// strength that shows up at all is a stain rather than a tint. Taking only
/// the hue means a yellow note is yellow on both, and pale on the one that is
/// pale.
///
/// `lift` is signed by the appearance: a tint is lighter than its card on a
/// dark theme and darker than it on a light one, which is the same rule the
/// two hand-made palettes in `theme.rs` were built to.
///
/// The saturation is scaled differently for the two, and it is not a fudge:
/// HSL saturation is a fraction of the chroma available at a given lightness,
/// and near white there is very little available. The same `0.4` that is a
/// clear wash on a dark card is invisible on paper. The two numbers below are
/// what put the same *amount of colour* in both, measured against the nine
/// hand-tuned palettes this app shipped before it read anybody else's format.
fn tint(surface: Hsla, hue_from: Hsla, lift: f32) -> Hsla {
    let light = lift < 0.0;
    let (scale, ceiling) = if light { (1.05, 0.70) } else { (0.62, 0.42) };
    let saturation = (hue_from.saturation * scale).clamp(0.10, ceiling);
    let lightness = (surface.lightness + lift).clamp(0.02, 0.98);
    hsla(crate::color::wheel(hue_from), saturation, lightness, 1.0)
}

// ---------------------------------------------------------------------------
// The translation
// ---------------------------------------------------------------------------

/// Which of the two the file says it is.
///
/// `type` when it has one, and the ground's own luminance when it does not —
/// which happens more often than the format's documentation suggests, and is
/// never actually ambiguous: a theme whose editor background is nearly black
/// is a dark theme whatever its file left out. The high-contrast pair map onto
/// the ordinary two, because they differ from them in how loud the borders are
/// rather than in which end of the range they sit at.
pub fn appearance_of(file: &File) -> Appearance {
    match file.kind.as_deref().map(str::to_ascii_lowercase).as_deref() {
        Some("light" | "hclight" | "vs") => Appearance::Light,
        Some("dark" | "hc" | "hcblack" | "vs-dark") => Appearance::Dark,
        _ => {
            let ground = Palette::of(&file.colors).pick(&GROUND);
            match ground {
                Some(ground) if luminance(ground) > 0.18 => Appearance::Light,
                Some(_) => Appearance::Dark,
                None => Appearance::Dark,
            }
        }
    }
}

/// Whether a file has anything to say about a board.
///
/// The one complaint left, and the only one that is still actionable: a JSON
/// file in the themes folder that names none of the keys below is either not a
/// theme at all or is nothing but `tokenColors`, and in both cases the honest
/// thing is to say so rather than to show a row that is the built-in palette
/// wearing somebody's file name.
pub fn says_anything(file: &File) -> bool {
    let palette = Palette::of(&file.colors);
    !palette.colors.is_empty() && palette.pick(READ).is_some()
}

/// Every key this build reads, for [`says_anything`] and for `THEMES.md` to be
/// checked against.
const READ: &[&str] = &[
    "editor.background",
    "editor.foreground",
    "foreground",
    "sideBar.background",
    "activityBar.background",
    "panel.background",
    "editorWidget.background",
    "input.background",
    "dropdown.background",
    "focusBorder",
    "button.background",
    "activityBarBadge.background",
    "progressBar.background",
    "textLink.foreground",
    "descriptionForeground",
    "editorLineNumber.foreground",
    "terminal.ansiYellow",
    "terminal.ansiGreen",
    "terminal.ansiBlue",
    "terminal.ansiRed",
    "terminal.ansiMagenta",
    "terminal.ansiCyan",
];

const GROUND: [&str; 3] = ["editor.background", "editorPane.background", "tab.activeBackground"];

/// A whole board out of an editor's palette.
///
/// The order below is load-bearing: `ground`, `text` and `accent` are settled
/// first because most of the other thirty-one are computed from them, and a
/// surface is settled before anything drawn on it, because [`readable`] needs
/// the background before it can move the foreground onto it.
pub fn to_theme(file: &File, appearance: Appearance) -> Theme {
    let palette = Palette::of(&file.colors);
    let base = appearance.base();
    let light = appearance == Appearance::Light;

    // --- the three everything else is made of ---------------------------

    let ground = palette.solid(&GROUND, base.ground).unwrap_or(base.ground);
    let text = palette
        .pick(&["editor.foreground", "foreground", "editorLineNumber.activeForeground"])
        .map(|c| over(c, ground))
        .unwrap_or(base.text);
    let accent = palette
        .solid(
            &[
                "focusBorder",
                "button.background",
                "activityBarBadge.background",
                "progressBar.background",
                "textLink.foreground",
                "editorCursor.foreground",
            ],
            ground,
        )
        .unwrap_or(base.accent);

    // --- surfaces -------------------------------------------------------

    // The chrome is a *panel*, and every one of these keys is one. Falling
    // back to a step off the ground rather than to the ground itself: a
    // sidebar the same colour as the canvas has no edge, and the hairline this
    // theme may not have set either cannot be relied on to draw one.
    let chrome = palette
        .solid(
            &[
                "sideBar.background",
                "activityBar.background",
                "panel.background",
                "editorWidget.background",
                "menu.background",
            ],
            ground,
        )
        .unwrap_or_else(|| toward(ground, text, if light { 0.06 } else { 0.05 }));

    // A card is a surface that floats *above* the board, which is what every
    // key here is in an editor — a hover, a suggest box, an input. Lifted
    // further off the ground than the chrome is, for the same reason: it is
    // the thing being looked at.
    let card = palette
        .solid(
            &[
                "editorWidget.background",
                "editorHoverWidget.background",
                "input.background",
                "dropdown.background",
                "editorSuggestWidget.background",
            ],
            ground,
        )
        .unwrap_or_else(|| toward(ground, text, if light { 0.03 } else { 0.09 }));

    // A card whose fill came out the same as the board it sits on is a card
    // nobody can see the edge of. Themes do this constantly — a great many set
    // `editorWidget.background` to `editor.background` exactly — so the
    // separation is enforced rather than hoped for.
    let card = match contrast(card, ground) < 1.06 {
        true => toward(ground, text, if light { 0.04 } else { 0.09 }),
        false => card,
    };

    // --- hairlines and the faint furniture -------------------------------
    //
    // All of these are the text colour at an alpha, and every one of them
    // takes a border key first when the theme has one. Alpha is the point: a
    // hairline is a hairline at any lightness, and a border lifted from a
    // theme at full opacity is a stripe.

    let edge_alpha = if light { 0.14 } else { 0.12 };
    let chrome_edge = palette
        .pick(&["sideBar.border", "panel.border", "editorWidget.border", "contrastBorder"])
        .map_or_else(|| faint(text, edge_alpha), |c| faint(c, edge_alpha.max(c.alpha)));
    let card_edge = palette
        .pick(&["editorWidget.border", "input.border", "widget.border", "contrastBorder"])
        .map_or_else(|| faint(text, edge_alpha), |c| faint(c, edge_alpha.max(c.alpha)));

    // The grid's alpha is computed from the zoom and thrown away here — see
    // `THEMES.md` and the grid's own note in `theme.rs` — so only its hue
    // survives, and it is written at zero to say so.
    let grid = faint(text, 0.0);
    let axis = palette
        .pick(&[
            "editorRuler.foreground",
            "editorIndentGuide.activeBackground1",
            "tree.indentGuidesStroke",
        ])
        .map_or_else(|| faint(text, 0.13), |c| faint(c, 0.13));

    // --- words -----------------------------------------------------------
    //
    // Measured against the chrome, which is where the settings page draws
    // them, and then against the cards, which is where a note draws them. The
    // floors are `theme.rs`'s own: 4.5:1 for anything read as a sentence.

    let text = readable(readable(text, chrome, 4.5), card, 4.5);
    let muted = palette
        .pick(&[
            "descriptionForeground",
            "editorLineNumber.activeForeground",
            "input.placeholderForeground",
        ])
        .map(|c| over(c, chrome))
        .unwrap_or_else(|| toward(text, chrome, 0.32));
    let muted = readable(muted, chrome, 4.5);
    // Deliberately allowed under the floor — it is `theme.rs`'s one exemption,
    // for marks that repeat the words beside them — but not allowed to vanish.
    let tertiary = palette
        .pick(&["icon.foreground", "editorLineNumber.foreground", "tab.inactiveForeground"])
        .map(|c| over(c, chrome))
        .unwrap_or_else(|| toward(text, chrome, 0.48));
    let tertiary = readable(tertiary, chrome, 3.0);

    let accent_text = palette
        .pick(&["textLink.foreground", "textLink.activeForeground"])
        .map(|c| over(c, chrome))
        .unwrap_or(accent);
    let accent_text = readable(accent_text, chrome, 4.5);

    // --- the card tints ---------------------------------------------------
    //
    // Six hues from the terminal palette, which is the one part of a VS Code
    // theme that is a *set of named colours* rather than a set of surfaces —
    // exactly what is needed here, and already tuned to sit together. A theme
    // without one falls back to fixed hues carrying the accent's saturation,
    // which is duller than a real ANSI set and still tells six kinds of card
    // apart.

    // The fallback saturation is fixed rather than the accent's, and that is
    // deliberate: an accent is often the loudest colour in a theme, and six
    // tints borrowing its saturation turn a quiet palette into a paint chart.
    // A card tint is a wash. `tint` scales this again per appearance.
    let ansi = |key: &str, hue: f32| -> Hsla {
        palette.pick(&[key]).unwrap_or_else(|| hsla(hue, 0.42, accent.lightness, 1.0))
    };
    let yellow = ansi("terminal.ansiYellow", 45.0 / 360.0);
    let green = ansi("terminal.ansiGreen", 125.0 / 360.0);
    let blue = ansi("terminal.ansiBlue", 225.0 / 360.0);
    let magenta = ansi("terminal.ansiMagenta", 310.0 / 360.0);
    let cyan = ansi("terminal.ansiCyan", 190.0 / 360.0);
    let red = ansi("terminal.ansiRed", 2.0 / 360.0);

    let type_lift = if light { -0.10 } else { 0.065 };
    let note = tint(card, yellow, type_lift);
    let image = tint(card, cyan, type_lift);
    let video = tint(card, magenta, type_lift);
    let audio = tint(card, green, type_lift);
    let link = tint(card, blue, type_lift);
    let fence = faint(accent, if light { 0.08 } else { 0.09 });

    // The pad, a shade further off the card than the type tints are: a note
    // somebody tinted on purpose should be more obviously tinted than one that
    // is simply a note.
    let pad_lift = if light { -0.125 } else { 0.085 };
    let notes = [
        tint(card, yellow, pad_lift),
        tint(card, green, pad_lift),
        tint(card, blue, pad_lift),
        tint(card, magenta, pad_lift),
    ];

    // --- words drawn on a card -------------------------------------------
    //
    // Held to the floor against *every* surface a note can be, the pad
    // included, which is the exact check `theme.rs` says was quietly failing
    // before there was a second palette to compare against. There is no
    // reason to expect a marketplace theme to pass it by luck.

    let surfaces = [card, note, image, video, audio, link, notes[0], notes[1], notes[2], notes[3]];
    let worst = |color: Hsla, floor: f32| -> Hsla {
        surfaces.iter().fold(color, |color, surface| readable(color, *surface, floor))
    };

    let text = worst(text, 4.5);
    let quote = worst(toward(text, card, 0.22), 4.5);
    let note_link = worst(
        palette
            .pick(&["textLink.foreground", "textLink.activeForeground"])
            .map(|c| over(c, card))
            .unwrap_or(accent),
        4.5,
    );

    // --- ropes, diffs and the rest ---------------------------------------

    Theme {
        ground,
        grid,
        axis,
        chrome,
        chrome_edge,
        card,
        card_edge,
        // A selection outline is furniture and needs 3:1, not the 4.5:1 a word
        // needs — and it is drawn on the board, not on the chrome.
        selected_edge: readable(accent, ground, 3.0),
        text,
        muted,
        tertiary,
        accent: readable(accent, chrome, 3.0),
        accent_text,
        note,
        image,
        video,
        audio,
        link,
        fence,
        notes,
        // Grey at the theme's own end of the range rather than a fixed one:
        // `#8c8c8c` is a swatch on a dark board and a smudge on paper.
        swatch_fallback: hsla(0.0, 0.0, if light { 0.62 } else { 0.55 }, 1.0),
        quote,
        note_link,
        diff_add: readable(
            palette
                .pick(&["gitDecoration.addedResourceForeground", "charts.green"])
                .unwrap_or(green),
            card,
            4.5,
        ),
        diff_remove: readable(
            palette.pick(&["gitDecoration.deletedResourceForeground", "charts.red"]).unwrap_or(red),
            card,
            4.5,
        ),
        // A rope is drawn on the board, between cards, and is a line rather
        // than a word: bright enough to follow, dim enough not to become a
        // third thing on the board. The five names, not five hexes — see
        // `Theme::rope_for`.
        rope_line: faint(text, if light { 0.45 } else { 0.4 }),
        rope_accent: readable(accent, ground, 3.0),
        rope_warm: readable(yellow, ground, 3.0),
        rope_leaf: readable(green, ground, 3.0),
        rope_danger: readable(red, ground, 3.0),
        anchor: faint(text, 0.35),
        guide: faint(text, 0.35),
        // `widget.shadow` is a real key and a real shadow, and its alpha is
        // the dial `THEMES.md` describes. A theme without one gets the
        // built-in's, which is full strength on a dark board and under half on
        // paper.
        shadow: palette.pick(&["widget.shadow", "scrollbar.shadow"]).unwrap_or(base.shadow),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme(json: &str) -> Theme {
        let file = parse(json).expect("parses");
        to_theme(&file, appearance_of(&file))
    }

    #[test]
    fn a_comment_and_a_trailing_comma_are_not_a_broken_file() {
        // Both are legal in the format VS Code documents, and both appear in
        // the licence header a great many marketplace themes open with.
        let file = parse(
            r##"{
                // Ember, by somebody.
                "name": "Ember", /* dark */
                "type": "dark",
                "colors": { "editor.background": "#141110", },
            }"##,
        )
        .expect("parses");
        assert_eq!(file.name.as_deref(), Some("Ember"));
        assert_eq!(file.colors.len(), 1);
    }

    #[test]
    fn a_slash_inside_a_string_is_not_a_comment() {
        let file = parse(r##"{"name": "http://x//y", "colors": {}}"##).expect("parses");
        assert_eq!(file.name.as_deref(), Some("http://x//y"));
    }

    #[test]
    fn a_theme_that_names_one_colour_is_still_a_whole_board() {
        // The point of the format: nobody has to name thirty-four things.
        let t = theme(r##"{"type":"dark","colors":{"editor.background":"#101010"}}"##);
        assert_eq!(crate::color::write(t.ground), "#101010ff");
        // And the twelve most load-bearing of the rest are not the ground.
        for other in [t.chrome, t.card, t.text, t.note, t.image, t.link] {
            assert_ne!(crate::color::write(other), crate::color::write(t.ground));
        }
    }

    #[test]
    fn the_appearance_is_read_off_the_ground_when_the_file_does_not_say() {
        assert_eq!(
            appearance_of(&parse(r##"{"colors":{"editor.background":"#101010"}}"##).unwrap()),
            Appearance::Dark
        );
        assert_eq!(
            appearance_of(&parse(r##"{"colors":{"editor.background":"#f4f1ea"}}"##).unwrap()),
            Appearance::Light
        );
    }

    #[test]
    fn a_card_is_never_the_same_colour_as_the_board_it_sits_on() {
        // The commonest thing a real theme does: one background for both.
        let t = theme(
            r##"{"type":"dark","colors":{
                "editor.background":"#1e1e1e",
                "editorWidget.background":"#1e1e1e"
            }}"##,
        );
        assert!(contrast(t.card, t.ground) > 1.05, "a card with no edge is not a card");
    }

    #[test]
    fn a_word_is_readable_on_every_surface_a_word_is_drawn_on() {
        // A deliberately hostile theme: a foreground three percent off its own
        // background, which is a thing an editor theme can get away with for a
        // colour it only ever uses on one surface.
        let t = theme(
            r##"{"type":"dark","colors":{
                "editor.background":"#1a1a1a",
                "editor.foreground":"#222222",
                "textLink.foreground":"#1d1d1d",
                "sideBar.background":"#161616"
            }}"##,
        );
        let surfaces =
            [t.card, t.chrome, t.note, t.image, t.video, t.audio, t.link, t.notes[0], t.notes[3]];
        for surface in surfaces {
            assert!(
                contrast(t.text, surface) >= 4.5,
                "text is {:.1}:1 on {}",
                contrast(t.text, surface),
                crate::color::write(surface)
            );
        }
        for surface in [t.card, t.note, t.notes[0], t.notes[3]] {
            assert!(contrast(t.quote, surface) >= 4.4, "quote too quiet");
            assert!(contrast(t.note_link, surface) >= 4.4, "a link nobody can see is not a link");
        }
        assert!(contrast(t.muted, t.chrome) >= 4.5);
        assert!(contrast(t.accent_text, t.chrome) >= 4.5);
    }

    #[test]
    fn a_light_theme_tints_downwards_and_a_dark_one_upwards() {
        // The rule both hand-made palettes were built to: a note is a wash on
        // its card, and a wash on paper is darker than the paper.
        let dark = theme(r##"{"type":"dark","colors":{"editor.background":"#141110"}}"##);
        let light = theme(r##"{"type":"light","colors":{"editor.background":"#f6efe7"}}"##);
        assert!(dark.notes[0].lightness > dark.card.lightness);
        assert!(light.notes[0].lightness < light.card.lightness);
    }

    #[test]
    fn the_terminal_colours_are_what_a_rope_is_named_after() {
        let t = theme(
            r##"{"type":"dark","colors":{
                "editor.background":"#141110",
                "terminal.ansiGreen":"#7f9e4d",
                "terminal.ansiRed":"#d64b46"
            }}"##,
        );
        assert_eq!(crate::color::write(t.rope_leaf), "#7f9e4dff");
        assert_eq!(crate::color::write(t.rope_danger), "#d64b46ff");
    }

    #[test]
    fn a_translucent_surface_lands_opaque() {
        // `editorWidget.background` at 20% over a dark editor is a *dark grey
        // panel*, not a card you can see the board through.
        let t = theme(
            r##"{"type":"dark","colors":{
                "editor.background":"#101010",
                "editorWidget.background":"#ffffff33"
            }}"##,
        );
        assert_eq!(t.card.alpha, 1.0);
    }

    #[test]
    fn a_file_with_nothing_but_syntax_in_it_is_not_a_theme() {
        let only_tokens = parse(r##"{"name":"x","tokenColors":[{"scope":"comment"}]}"##).unwrap();
        assert!(!says_anything(&only_tokens));
        let real = parse(r##"{"colors":{"editor.background":"#101010"}}"##).unwrap();
        assert!(says_anything(&real));
    }

    #[test]
    fn an_include_is_the_base_and_the_file_that_named_it_has_the_last_word() {
        let files = HashMap::from([
            (
                "themes/base.json".to_string(),
                r##"{"type":"dark","colors":{"editor.background":"#101010","focusBorder":"#ff0000"}}"##
                    .to_string(),
            ),
            (
                "themes/warm.json".to_string(),
                r##"{"include":"./base.json","name":"Warm","colors":{"focusBorder":"#00ff00"}}"##
                    .to_string(),
            ),
        ]);
        let file = resolve("themes/warm.json", &files).expect("resolves");
        assert_eq!(file.name.as_deref(), Some("Warm"));
        assert_eq!(file.kind.as_deref(), Some("dark"));
        let palette = Palette::of(&file.colors);
        assert_eq!(crate::color::write(palette.pick(&["editor.background"]).unwrap()), "#101010ff");
        assert_eq!(crate::color::write(palette.pick(&["focusBorder"]).unwrap()), "#00ff00ff");
    }

    #[test]
    fn a_file_that_includes_itself_is_not_an_infinite_loop() {
        let files = HashMap::from([(
            "a.json".to_string(),
            r##"{"include":"a.json","colors":{"editor.background":"#101010"}}"##.to_string(),
        )]);
        assert!(resolve("a.json", &files).is_ok());
    }

    #[test]
    fn a_null_colour_is_absent_rather_than_black() {
        // VS Code's way of saying "unset the one my include set".
        let t = theme(
            r##"{"type":"dark","colors":{"editor.background":"#141110","focusBorder":null}}"##,
        );
        assert_ne!(crate::color::write(t.accent), "#000000ff");
    }
}
