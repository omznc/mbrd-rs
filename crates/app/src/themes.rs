//! Every palette this app can wear, and where they come from.
//!
//! `theme.rs` is one palette — the struct that everything which draws asks
//! instead of naming a colour. This is the *list* of them: two compiled into
//! the binary, however many more are sitting in `dirs::themes()`, and the one
//! function that turns a name somebody chose into a [`Theme`] somebody can see.
//!
//! ## The shape of a theme file
//!
//! A *family* with an array of themes in it, which is the shape Zed's themes
//! use and is worth copying for one concrete reason rather than for
//! resemblance: a light and a dark that are meant to be worn as a pair are
//! authored together and shipped as one file, and a format with one theme per
//! file makes that pair into two files with a naming convention between them.
//!
//! ```json
//! {
//!   "name": "Ink",
//!   "author": "somebody",
//!   "themes": [
//!     { "name": "Ink", "appearance": "dark", "style": { "accent": "#5a8de0" } }
//!   ]
//! }
//! ```
//!
//! `style` is [`Theme`]'s own field names against `#rrggbb` or `#rrggbbaa`
//! strings, and **every key is optional** — see [`theme::overlay`], which is
//! the whole of the merge. `appearance` is what a theme inherits the rest of
//! its palette from: a `dark` theme starts at [`Theme::dark`] and a `light`
//! one at [`Theme::light`]. That is what lets `sepia.json` in this crate's
//! assets name twenty colours instead of thirty-four and still be a complete
//! theme.
//!
//! ## Why the built-ins go through the same door
//!
//! Two of them do not: [`Theme::dark`] and [`Theme::light`] are Rust, because
//! they are the palettes everything else is *measured against* and the app has
//! to be able to draw itself when every file on disk has failed. Every other
//! built-in is a `.json` in `assets/themes`, read by the same parser that
//! reads somebody's own — which is the only arrangement that keeps the
//! published format honest. A format whose only real user is an external one
//! is a format that breaks quietly.
//!
//! ## Reading is best-effort and silent
//!
//! Like `prefs.rs` and `recent.rs`. A theme file that is not JSON, or that
//! names a colour as `"grue"`, is skipped and counted rather than thrown — the
//! alternative is an app that will not start because of a file somebody was
//! halfway through editing. The count is not thrown away either: the settings
//! page says how many were unreadable, because a theme that silently does not
//! appear in a list is indistinguishable from one that was never saved.

use std::path::PathBuf;

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::dirs;
use crate::theme::{self, Theme};

/// Which of the two palettes a theme is a variation on, and which of the two
/// slots in the settings page it fills.
///
/// Not a bool. It is stored in a file somebody wrote by hand, where `"dark"`
/// is a word and `true` is a riddle, and it is shown in a list where the two
/// are headings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    Light,
    Dark,
}

impl Appearance {
    pub fn label(self) -> &'static str {
        match self {
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    /// The palette a theme of this appearance starts from before its own file
    /// is laid over the top.
    pub fn base(self) -> Theme {
        match self {
            Self::Light => Theme::light(),
            Self::Dark => Theme::dark(),
        }
    }
}

/// A theme file: several themes under one name, by one person.
#[derive(Debug, Deserialize)]
struct Family {
    #[serde(default)]
    name: String,
    #[serde(default)]
    author: String,
    #[serde(default)]
    themes: Vec<Entry>,
}

/// One theme inside a family.
#[derive(Debug, Deserialize)]
struct Entry {
    name: String,
    appearance: Appearance,
    #[serde(default)]
    style: Map<String, Value>,
}

/// A theme that is ready to be worn, with everything the settings page needs
/// to show it in a list.
///
/// The palette is resolved *once, at load*, rather than every time somebody
/// arrows past it in a picker. A theme is a few dozen colours and merging one
/// is cheap, but the picker previews live as the highlight moves — see
/// `BoardView::preview_theme` — and doing the merge inside a paint is the kind
/// of thing that is free until somebody has forty themes.
#[derive(Debug, Clone)]
pub struct Named {
    pub name: String,
    /// Which file it came from, as a person would say it: the family name, or
    /// empty for the two that are compiled in.
    pub family: String,
    pub author: String,
    pub appearance: Appearance,
    pub theme: Theme,
}

/// The name of the theme worn when nobody has chosen one, per appearance.
pub const DEFAULT_DARK: &str = "Ash";
pub const DEFAULT_LIGHT: &str = "Paper";

/// The built-in `.json` families, in the binary.
///
/// The same trick `icons.rs` uses on the SVGs, and for the same reason: a
/// built-in that had to be found on disk is a built-in that can be missing.
const BUILT_IN: &[(&str, &str)] = &[
    ("ink.json", include_str!("../assets/themes/ink.json")),
    ("sepia.json", include_str!("../assets/themes/sepia.json")),
    ("moss.json", include_str!("../assets/themes/moss.json")),
    ("plum.json", include_str!("../assets/themes/plum.json")),
    ("slate.json", include_str!("../assets/themes/slate.json")),
];

/// Every theme this run can offer.
#[derive(Debug, Clone)]
pub struct Registry {
    themes: Vec<Named>,
    /// Files in the themes directory that could not be read, by name.
    ///
    /// Kept rather than logged. Nothing in this app has a console somebody is
    /// looking at, so a warning printed to stderr is a warning nobody receives
    /// — the settings page says this out loud instead.
    pub unreadable: Vec<String>,
}

impl Default for Registry {
    /// The two that cannot fail. Used before [`load`](Self::load) has run and
    /// as the answer when there is no home directory to read from.
    fn default() -> Self {
        Self {
            themes: vec![
                Named {
                    name: DEFAULT_DARK.into(),
                    family: String::new(),
                    author: "mbrd".into(),
                    appearance: Appearance::Dark,
                    theme: Theme::dark(),
                },
                Named {
                    name: DEFAULT_LIGHT.into(),
                    family: String::new(),
                    author: "mbrd".into(),
                    appearance: Appearance::Light,
                    theme: Theme::light(),
                },
            ],
            unreadable: Vec::new(),
        }
    }
}

impl Registry {
    /// Everything: the two compiled-in palettes, the built-in families, and
    /// whatever is in `dirs::themes()`.
    ///
    /// In that order, and the order is the precedence — see
    /// [`add`](Self::add). Somebody who writes a `Ash.json` gets their `Ash`,
    /// not this one's, which is the only way of correcting a built-in that
    /// does not involve waiting for a release.
    pub fn load() -> Self {
        Self::load_from(dirs::themes().as_deref())
    }

    /// The same, from a directory somebody names.
    ///
    /// Split out solely so that the half of this module which touches a
    /// filesystem can be tested against a directory a test made, rather than
    /// against whatever the person running the tests happens to have written.
    /// `load` is the only caller outside them.
    fn load_from(dir: Option<&std::path::Path>) -> Self {
        let mut registry = Self::default();
        for (file, text) in BUILT_IN {
            registry.read(file, text);
        }

        let Some(dir) = dir else { return registry };
        let Ok(entries) = std::fs::read_dir(dir) else { return registry };
        // Sorted, because `read_dir` is in whatever order the filesystem feels
        // like and a settings list that reshuffles itself between launches is
        // one nobody can learn the shape of.
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")))
            .collect();
        files.sort();

        for path in files {
            let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into());
            match std::fs::read_to_string(&path) {
                Ok(text) => registry.read(&name, &text),
                Err(_) => registry.unreadable.push(name),
            }
        }
        registry
    }

    /// One file's worth, however many themes that is.
    fn read(&mut self, file: &str, text: &str) {
        let Ok(family) = serde_json::from_str::<Family>(text) else {
            self.unreadable.push(file.to_string());
            return;
        };
        if family.themes.is_empty() {
            self.unreadable.push(file.to_string());
            return;
        }
        for entry in family.themes {
            // A theme whose style will not merge is counted with the files
            // that would not parse, and for the same reason: from where
            // somebody is sitting, a theme that does not appear in the list
            // and a theme that appears wearing somebody else's colours are
            // both "it did not work", and only one of them says so.
            let Some(theme) = theme::overlay(entry.appearance.base(), &entry.style) else {
                self.unreadable.push(file.to_string());
                continue;
            };
            self.add(Named {
                theme,
                name: entry.name,
                family: family.name.clone(),
                author: family.author.clone(),
                appearance: entry.appearance,
            });
        }
    }

    /// Add a theme, or replace one of the same name and appearance.
    ///
    /// Replace rather than refuse, because [`load`](Self::load) reads in
    /// precedence order and the last word should be the person sitting here.
    /// Keyed on the name *and* the appearance, so a family may legitimately
    /// ship a light and a dark both called "Ink" — which is the usual way of
    /// naming a pair and would otherwise mean one of them silently ate the
    /// other.
    fn add(&mut self, theme: Named) {
        match self
            .themes
            .iter_mut()
            .find(|t| t.name == theme.name && t.appearance == theme.appearance)
        {
            Some(existing) => *existing = theme,
            None => self.themes.push(theme),
        }
    }

    /// Every theme of one appearance, in the order a list should show them.
    pub fn of(&self, appearance: Appearance) -> Vec<&Named> {
        let mut found: Vec<&Named> =
            self.themes.iter().filter(|t| t.appearance == appearance).collect();
        // The built-in first, then everything else alphabetically. The default
        // is pinned to the top rather than sorted into the middle because it
        // is the one row somebody navigates to on purpose — it is where they
        // go back to.
        found.sort_by_key(|t| (!t.family.is_empty(), t.name.to_lowercase()));
        found
    }

    /// The palette behind a name, or the built-in default for that
    /// appearance.
    ///
    /// The fallback is the whole reason this is a lookup rather than a stored
    /// palette. A theme is chosen by *name*, and a name is what survives the
    /// file it came from being edited — but also what is left pointing at
    /// nothing when somebody deletes that file between two launches. Falling
    /// back to the base is what stops that being a blank app with no way back
    /// to the settings page.
    pub fn resolve(&self, name: &str, appearance: Appearance) -> Theme {
        self.themes
            .iter()
            .find(|t| t.name == name && t.appearance == appearance)
            .map_or_else(|| appearance.base(), |t| t.theme)
    }

    /// Whether a name is one this registry knows, which is what the settings
    /// page needs in order to say that a chosen theme has gone missing rather
    /// than quietly showing the default as though it had been chosen.
    pub fn knows(&self, name: &str, appearance: Appearance) -> bool {
        self.themes.iter().any(|t| t.name == name && t.appearance == appearance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same two floors `theme.rs` holds its own palettes to.
    fn readable(t: &Theme) -> Result<(), String> {
        fn luminance(c: gpui::Hsla) -> f32 {
            let rgba = gpui::Rgba::from(c);
            let ch = |v: f32| {
                if v <= 0.03928 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * ch(rgba.r) + 0.7152 * ch(rgba.g) + 0.0722 * ch(rgba.b)
        }
        let contrast = |fg, bg| {
            let (a, b) = (luminance(fg), luminance(bg));
            (a.max(b) + 0.05) / (a.min(b) + 0.05)
        };
        let mut cards = vec![t.card, t.note, t.image, t.video, t.audio, t.link];
        cards.extend(t.notes);
        for surface in cards {
            for (what, colour) in [("text", t.text), ("quote", t.quote), ("link", t.note_link)] {
                let ratio = contrast(colour, surface);
                if ratio < 4.5 {
                    return Err(format!("{what} is {ratio:.2}:1 on a card"));
                }
            }
        }
        for (what, colour) in [("text", t.text), ("muted", t.muted), ("accent_text", t.accent_text)]
        {
            let ratio = contrast(colour, t.chrome);
            if ratio < 4.5 {
                return Err(format!("{what} is {ratio:.2}:1 on the chrome"));
            }
        }
        Ok(())
    }

    /// The registry as it is without touching the disk — the two compiled-in
    /// palettes plus the built-in families. Every test here uses this rather
    /// than [`Registry::load`], which reads the real themes directory and
    /// would pass or fail depending on what the person running it has written.
    fn built_in() -> Registry {
        let mut registry = Registry::default();
        for (file, text) in BUILT_IN {
            registry.read(file, text);
        }
        registry
    }

    #[test]
    fn every_theme_this_binary_ships_can_be_read_on() {
        // The reason the built-in families are files rather than Rust: this
        // test is the only thing standing between a pretty palette and one
        // where the quotes on a note have vanished into the card behind them.
        let registry = built_in();
        assert!(registry.unreadable.is_empty(), "shipped a theme that will not parse");
        for theme in &registry.themes {
            if let Err(problem) = readable(&theme.theme) {
                panic!("the built-in theme {:?}: {problem}", theme.name);
            }
        }
    }

    #[test]
    fn a_theme_may_name_only_what_it_changes() {
        // `sepia.json` deliberately says nothing about the grid, the axes, the
        // ropes or the shadows, and is a whole theme anyway. If inheritance
        // ever broke, this is what would catch it — the alternative symptom is
        // a theme that looks *nearly* right, which nobody reports.
        let registry = built_in();
        let sepia = registry.resolve("Sepia", Appearance::Light);
        assert_eq!(sepia.guide, Theme::light().guide, "an unnamed colour comes from the base");
        assert_ne!(sepia.ground, Theme::light().ground, "a named one does not");
    }

    #[test]
    fn a_dark_theme_and_a_light_one_may_share_a_name() {
        // The usual way of naming a pair, and the reason `add` is keyed on
        // both halves: keyed on the name alone, one of these would replace the
        // other and a family would silently ship one theme.
        let mut registry = Registry::default();
        registry.read(
            "pair.json",
            r##"{ "name": "Pair", "themes": [
                { "name": "Pair", "appearance": "dark",  "style": { "accent": "#111111" } },
                { "name": "Pair", "appearance": "light", "style": { "accent": "#222222" } }
            ] }"##,
        );
        assert!(registry.unreadable.is_empty());
        assert_eq!(registry.resolve("Pair", Appearance::Dark).accent, gpui::rgb(0x111111).into());
        assert_eq!(registry.resolve("Pair", Appearance::Light).accent, gpui::rgb(0x222222).into());
    }

    #[test]
    fn somebody_elses_theme_wins_over_a_built_in_of_the_same_name() {
        // The only way of correcting a built-in that does not involve waiting
        // for a release.
        let mut registry = built_in();
        registry.read(
            "mine.json",
            r##"{ "name": "Mine", "themes": [
                { "name": "Ink", "appearance": "dark", "style": { "accent": "#00ff00" } }
            ] }"##,
        );
        assert_eq!(registry.resolve("Ink", Appearance::Dark).accent, gpui::rgb(0x00ff00).into());
        assert_eq!(registry.of(Appearance::Dark).iter().filter(|t| t.name == "Ink").count(), 1);
    }

    #[test]
    fn a_theme_that_has_gone_missing_falls_back_rather_than_blanking_the_app() {
        // What happens on the second launch after somebody deletes a theme
        // file. The name in `settings.json` still points at it, and the answer
        // has to be a palette rather than nothing — the alternative is an app
        // with no visible way back to the settings page that would fix it.
        let registry = built_in();
        assert!(!registry.knows("Gone", Appearance::Dark));
        assert_eq!(registry.resolve("Gone", Appearance::Dark), Theme::dark());
        assert_eq!(registry.resolve("Gone", Appearance::Light), Theme::light());
    }

    #[test]
    fn a_file_that_is_not_a_theme_is_counted_rather_than_thrown() {
        // A file somebody is halfway through editing must not stop the app
        // starting, and must not vanish without trace either.
        let mut registry = Registry::default();
        for (file, bad) in [
            ("truncated.json", "{ \"name\": \"Half\", "),
            ("empty.json", r#"{ "name": "Nothing", "themes": [] }"#),
            ("wrong-shape.json", r#"{ "themes": [{ "name": "X" }] }"#),
            (
                "not-a-colour.json",
                r##"{ "themes": [{ "name": "X", "appearance": "dark",
                    "style": { "accent": "burnt sienna" } }] }"##,
            ),
        ] {
            let before = registry.unreadable.len();
            registry.read(file, bad);
            assert_eq!(registry.unreadable.len(), before + 1, "{file} should have been counted");
        }
        // And the two that cannot fail are still there to draw with.
        assert_eq!(registry.of(Appearance::Dark).len(), 1);
    }

    #[test]
    fn a_theme_file_on_disk_joins_the_list() {
        // The one half of this module that touches a filesystem, against a
        // directory the test made rather than the one the person running it
        // keeps their own themes in.
        let dir = std::env::temp_dir().join(format!("mbrd-themes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a temp directory");
        std::fs::write(
            dir.join("mine.json"),
            r##"{ "name": "Mine", "author": "me", "themes": [
                { "name": "Neon", "appearance": "dark", "style": { "accent": "#00ff88" } }
            ] }"##,
        )
        .unwrap();
        // Not JSON, and not a `.json` — one has to be counted and the other
        // has to be passed over without being counted, because a `README.txt`
        // sitting in the folder is not a theme that failed.
        std::fs::write(dir.join("broken.json"), "{ oh no").unwrap();
        std::fs::write(dir.join("notes.txt"), "not a theme and not claiming to be").unwrap();

        let registry = Registry::load_from(Some(&dir));
        assert_eq!(registry.resolve("Neon", Appearance::Dark).accent, gpui::rgb(0x00ff88).into());
        assert_eq!(registry.unreadable, vec!["broken.json".to_string()]);
        // And the built-ins are still all there beside it.
        assert!(registry.knows(DEFAULT_DARK, Appearance::Dark));
        assert!(registry.knows("Ink", Appearance::Dark));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nowhere_to_read_from_is_still_a_working_app() {
        // No home directory, or a themes folder that does not exist. Both
        // have to be the built-ins rather than nothing.
        let registry = Registry::load_from(None);
        assert!(registry.knows(DEFAULT_DARK, Appearance::Dark));
        assert!(registry.knows(DEFAULT_LIGHT, Appearance::Light));
        assert!(registry.unreadable.is_empty());

        let missing = std::env::temp_dir().join("mbrd-themes-that-are-not-there");
        assert!(Registry::load_from(Some(&missing)).knows(DEFAULT_DARK, Appearance::Dark));
    }

    #[test]
    fn the_default_is_the_top_of_its_own_list() {
        // Where somebody goes back to, so it is not sorted into the middle.
        let registry = built_in();
        assert_eq!(
            registry.of(Appearance::Dark).first().map(|t| t.name.as_str()),
            Some(DEFAULT_DARK)
        );
        assert_eq!(
            registry.of(Appearance::Light).first().map(|t| t.name.as_str()),
            Some(DEFAULT_LIGHT)
        );
    }
}
