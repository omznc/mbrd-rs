//! Every palette this app can wear, and where they come from.
//!
//! `theme.rs` is one palette — the struct that everything which draws asks
//! instead of naming a colour. This is the *list* of them: two compiled into
//! the binary, sixteen more shipped as files beside them, however many are
//! sitting in `dirs::themes()`, and the one function that turns a name
//! somebody chose into a [`Theme`] somebody can see.
//!
//! ## The shape of a theme file
//!
//! A VS Code colour theme, which `vscode.rs` reads and translates. One file is
//! one theme:
//!
//! ```json
//! {
//!   "name": "Ember",
//!   "type": "dark",
//!   "colors": { "editor.background": "#141110", "focusBorder": "#e0762f" }
//! }
//! ```
//!
//! This module used to define a format of its own — a family with a light and
//! a dark inside it, keyed by this app's own field names. It was a good format
//! and nobody was ever going to write one. Somebody else's format comes with
//! thousands of themes already written in it, a marketplace to get them from
//! (see `openvsx.rs`) and editors that will preview one while it is being
//! authored, and none of those are things a format of our own could be given.
//! What it costs is that a board has colours an editor does not — see the
//! translation in `vscode.rs`, which is where that whole argument lives.
//!
//! **Every key is optional**, and a theme naming one colour is a whole theme:
//! everything not named is computed from what was. There is no inheritance
//! from a base palette any more, and no partial theme — a file either says
//! something about a board or it says nothing at all.
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
//! Like `prefs.rs` and `recent.rs`. A theme file that is not JSON, or that has
//! nothing in it about a board, is skipped rather than thrown — the
//! alternative is an app that will not start because of a file somebody was
//! halfway through editing. What went wrong is not thrown away either: the
//! settings page names the file and says what it was, because a theme that
//! silently does not appear in a list is indistinguishable from one that was
//! never saved.
//!
//! **An unrecognised key is not one of those complaints.** It used to be, and
//! it had to be, because the format had forty keys and a misspelling hid among
//! them in the same silence a key from a later build did. A VS Code theme
//! names hundreds of colours that are about an editor and nothing else, so the
//! only thing left worth saying is when a file names *none* this build reads.

use std::path::PathBuf;

use crate::dirs;
use crate::theme::Theme;
use crate::vscode;

/// Which of the two palettes a theme is a variation on, and which of the two
/// slots in the settings page it fills.
///
/// Not a bool. It is stored in a file somebody wrote by hand, where `"dark"`
/// is a word and `true` is a riddle, and it is shown in a list where the two
/// are headings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

    /// The palette worn when everything else has failed: no theme of this
    /// appearance was found, or a file could not be read at all.
    ///
    /// Not an inheritance root any more. Under the old format a theme named
    /// three colours and took thirty from here; a VS Code theme names none of
    /// this app's colours and every one of them is computed from what it did
    /// name, so this is a floor rather than a base — see `vscode::to_theme`,
    /// which still asks for it when a file says nothing about a surface at
    /// all.
    pub fn base(self) -> Theme {
        match self {
            Self::Light => Theme::light(),
            Self::Dark => Theme::dark(),
        }
    }

    /// The name of the theme worn when nobody has chosen one.
    pub fn default_theme(self) -> &'static str {
        match self {
            Self::Light => DEFAULT_LIGHT,
            Self::Dark => DEFAULT_DARK,
        }
    }
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
    /// Where it came from, as a person would say it: the file's own name for
    /// one in the themes folder, the extension's for one installed from
    /// open-vsx, and **empty for anything this binary ships** — which is what
    /// the settings page turns into the words "Built in".
    pub family: String,
    pub author: String,
    pub appearance: Appearance,
    pub theme: Theme,
}

/// The name of the theme worn when nobody has chosen one, per appearance.
pub const DEFAULT_DARK: &str = "Ash";
pub const DEFAULT_LIGHT: &str = "Paper";

/// The built-in themes, in the binary.
///
/// The same trick `icons.rs` uses on the SVGs, and for the same reason: a
/// built-in that had to be found on disk is a built-in that can be missing.
///
/// Two files where there used to be one, because the format has one theme per
/// file and a light and a dark are two themes. That is the one thing the old
/// format did better — a pair meant to be worn together was authored together
/// — and it is worth the trade: `ember-dark.json` and `ember-light.json` are
/// the same pair with the naming convention on the outside instead of the
/// inside, and either of them opens in an editor that understands it.
const BUILT_IN: &[(&str, &str)] = &[
    ("ink-dark.json", include_str!("../assets/themes/ink-dark.json")),
    ("sepia-light.json", include_str!("../assets/themes/sepia-light.json")),
    ("moss-dark.json", include_str!("../assets/themes/moss-dark.json")),
    ("moss-light.json", include_str!("../assets/themes/moss-light.json")),
    ("plum-dark.json", include_str!("../assets/themes/plum-dark.json")),
    ("plum-light.json", include_str!("../assets/themes/plum-light.json")),
    ("slate-dark.json", include_str!("../assets/themes/slate-dark.json")),
    ("slate-light.json", include_str!("../assets/themes/slate-light.json")),
    // Four with a hue each, and a pair apiece. The five above are variations
    // on the two base palettes and read as one another in a list — which is
    // most of why the dark list and the light list looked like the same list
    // before the rows grew swatches. These are meant to be told apart at a
    // glance: a warm orange, a cool teal, a blue, and a near-monochrome with a
    // single red in it.
    ("ember-dark.json", include_str!("../assets/themes/ember-dark.json")),
    ("ember-light.json", include_str!("../assets/themes/ember-light.json")),
    ("tide-dark.json", include_str!("../assets/themes/tide-dark.json")),
    ("tide-light.json", include_str!("../assets/themes/tide-light.json")),
    ("cobalt-dark.json", include_str!("../assets/themes/cobalt-dark.json")),
    ("cobalt-light.json", include_str!("../assets/themes/cobalt-light.json")),
    ("vellum-dark.json", include_str!("../assets/themes/vellum-dark.json")),
    ("vellum-light.json", include_str!("../assets/themes/vellum-light.json")),
];

/// Something wrong with a file in the themes directory, in the words the
/// settings page shows.
///
/// A sentence rather than an error type, because nothing matches on this. It
/// is read once, by whoever is working out why the theme they just wrote is
/// not in the list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Complaint {
    /// The file it is about, as it is named on disk. **The whole point.** This
    /// used to be a count, and a count cannot be acted on: "one file there
    /// could not be read" is the same sentence whether somebody has one theme
    /// or forty.
    pub file: String,
    pub why: String,
}

/// A file name without its extension, which is what a theme with no name of
/// its own is called and what a folder theme is credited to.
fn stem(file: &str) -> String {
    file.rsplit_once('.').map_or(file, |(before, _)| before).to_string()
}

/// Every theme this run can offer.
#[derive(Debug, Clone)]
pub struct Registry {
    themes: Vec<Named>,
    /// What is wrong in the themes directory, and which file it is wrong in.
    ///
    /// Kept rather than logged. Nothing in this app has a console somebody is
    /// looking at, so a warning printed to stderr is a warning nobody receives
    /// — the settings page says this out loud instead.
    ///
    /// **Only files from the folder.** The built-in families go through the
    /// same reader, and a complaint about one of those is a bug in this build
    /// rather than something the person sitting here can fix — it would send
    /// them looking for `ink.json` in a directory it has never been in. A test
    /// stands in for the report instead; see `every_built_in_family_is_clean`.
    pub complaints: Vec<Complaint>,
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
            complaints: Vec::new(),
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
            registry.read(file, text, "", false);
        }

        let Some(dir) = dir else { return registry };
        // Sorted, because a directory lists in whatever order the filesystem
        // feels like and a settings list that reshuffles itself between
        // launches is one nobody can learn the shape of. `read_dir_paths`
        // sorts on both platforms, which is where that now happens.
        let Ok(paths) = crate::store::read_dir_paths(dir) else { return registry };
        let files: Vec<PathBuf> = paths
            .into_iter()
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("json")))
            .collect();

        for path in files {
            let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into());
            match crate::store::read_to_string(&path) {
                Ok(text) => {
                    let family = stem(&name);
                    registry.read(&name, &text, &family, true);
                }
                Err(_) => registry.complain(true, &name, "could not be opened".into()),
            }
        }
        registry
    }

    /// Note something wrong with a file, once.
    ///
    /// Deduplicated on the pair, because a family with the same mistake in both
    /// of its themes is one mistake — the old count pushed per *entry* and then
    /// reported per *file*, so a two-theme family with one bad half announced
    /// that two files could not be read when there was one.
    fn complain(&mut self, mine: bool, file: &str, why: String) {
        if !mine {
            return;
        }
        let complaint = Complaint { file: file.to_string(), why };
        if !self.complaints.contains(&complaint) {
            self.complaints.push(complaint);
        }
    }

    /// One file's worth: one theme.
    ///
    /// `from_folder` says whether anything wrong with it is worth telling
    /// somebody about — see [`Registry::complaints`].
    ///
    /// `family` is where it came from, in the words the settings page shows,
    /// and empty for anything this binary ships. The caller supplies it
    /// because only the caller knows: a file in the themes folder is named by
    /// its own file name, and one installed from open-vsx is named by the
    /// extension it came out of, which is nowhere in the file.
    fn read(&mut self, file: &str, text: &str, family: &str, from_folder: bool) {
        let parsed = match vscode::parse(text) {
            Ok(parsed) => parsed,
            Err(why) => {
                self.complain(from_folder, file, why);
                return;
            }
        };
        // The one complaint left, and the only one still worth making. A theme
        // may name eleven colours or six hundred and both are fine; a file
        // that names none this build reads is either not a theme or is nothing
        // but syntax highlighting, and showing it as a row would be showing
        // the built-in palette under somebody else's file name.
        if !vscode::says_anything(&parsed) {
            self.complain(from_folder, file, "names no colour this build reads".into());
            return;
        }
        let appearance = vscode::appearance_of(&parsed);
        // The file's own name, then the file's. A theme file inside an
        // extension usually has no `name` at all — it is carried in the
        // `package.json` beside it — and a row with no name in it is a row
        // nobody can choose.
        let name = match parsed.name.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
            Some(name) => name.to_string(),
            None => stem(file),
        };
        // The file's own credit first, which only a theme installed from
        // open-vsx has: an extension's display name is nowhere in a theme
        // file, so `openvsx.rs` writes it in. Everything else is credited to
        // what the caller knows.
        let family = match parsed.family.as_deref().map(str::trim).filter(|f| !f.is_empty()) {
            Some(own) => own,
            None => family,
        };
        self.add(Named {
            theme: vscode::to_theme(&parsed, appearance),
            name,
            family: family.to_string(),
            author: parsed.author.clone().unwrap_or_default(),
            appearance,
        });
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
        // The default first, then the rest of the built-ins, then everything
        // from the folder, each group alphabetically. The default is pinned
        // rather than sorted into the middle because it is the one row
        // somebody navigates to on purpose — it is where they go back to, and
        // under the old format it only sat at the top because "Ash" and
        // "Paper" happened to sort there.
        let default = appearance.default_theme();
        found.sort_by_key(|t| (t.name != default, !t.family.is_empty(), t.name.to_lowercase()));
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
            let rgba = gpui::hsla_to_rgba(c);
            let ch = |v: f32| {
                if v <= 0.03928 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * ch(rgba.red) + 0.7152 * ch(rgba.green) + 0.0722 * ch(rgba.blue)
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
        // A tint is also a mark — the empty board's two doors, the stock
        // panel's column — and `Theme::legible` is what makes one. 3:1,
        // because none of those is a word. See `theme.rs`'s own test.
        for (what, tint) in
            [("note", t.note), ("image", t.image), ("video", t.video), ("link", t.link)]
        {
            let ratio = contrast(t.legible(tint, t.muted), t.chrome);
            if ratio < 3.0 {
                return Err(format!("{what} as a mark is {ratio:.2}:1 on the chrome"));
            }
        }
        Ok(())
    }

    /// The registry as it is without touching the disk — the two compiled-in
    /// palettes plus the built-in files. Every test here uses this rather
    /// than [`Registry::load`], which reads the real themes directory and
    /// would pass or fail depending on what the person running it has written.
    fn built_in() -> Registry {
        let mut registry = Registry::default();
        for (file, text) in BUILT_IN {
            registry.read(file, text, "", true);
        }
        registry
    }

    #[test]
    fn every_theme_this_binary_ships_can_be_read_on() {
        // The reason the built-ins are files rather than Rust: this test is
        // the only thing standing between a pretty palette and one where the
        // quotes on a note have vanished into the card behind them.
        //
        // It matters more now than it did, because these files no longer name
        // the colours it checks. `quote` and the four note tints are computed
        // by `vscode::to_theme` out of the handful of keys the file *does*
        // name, so this is a test of the translation as much as of the
        // palettes — and the translation has to hold for a theme nobody here
        // wrote, which is the whole point of taking the format.
        let registry = built_in();
        assert!(
            registry.complaints.is_empty(),
            "shipped a theme that will not parse: {:?}",
            registry.complaints
        );
        for theme in &registry.themes {
            if let Err(problem) = readable(&theme.theme) {
                panic!("the built-in theme {:?}: {problem}", theme.name);
            }
        }
    }

    #[test]
    fn every_pair_this_binary_ships_is_still_a_pair() {
        // One file per theme means a light and a dark that belong together are
        // held together by their names alone. Seven of the nine are pairs, and
        // a rename that quietly halved one would show up here rather than in a
        // settings page with one Ember in it.
        let registry = built_in();
        for name in ["Moss", "Plum", "Slate", "Ember", "Tide", "Cobalt", "Vellum"] {
            assert!(registry.knows(name, Appearance::Dark), "{name} has no dark half");
            assert!(registry.knows(name, Appearance::Light), "{name} has no light half");
        }
    }

    #[test]
    fn a_theme_may_name_only_what_it_changes() {
        // A file naming one colour is a whole theme. Under the old format that
        // worked by inheriting thirty-odd from a base palette; now everything
        // is computed from what was named, which is a stronger promise and an
        // easier one to break — a hole in `vscode::to_theme` is a colour that
        // silently comes out as the built-in's.
        let mut registry = Registry::default();
        registry.read(
            "one.json",
            r##"{ "name": "One", "type": "dark", "colors": { "editor.background": "#101010" } }"##,
            "one",
            true,
        );
        assert!(registry.complaints.is_empty(), "{:?}", registry.complaints);
        let one = registry.resolve("One", Appearance::Dark);
        assert_eq!(one.ground, crate::color::rgb(0x101010));
        // And the colours it never mentioned are its own rather than the
        // fallback's, because they were worked out from the one it did.
        assert_ne!(one.card, Theme::dark().card);
        assert_ne!(one.notes[0], Theme::dark().notes[0]);
    }

    #[test]
    fn a_dark_theme_and_a_light_one_may_share_a_name() {
        // The usual way of naming a pair, and the reason `add` is keyed on
        // both halves: keyed on the name alone, one of these would replace the
        // other and a pair would silently ship one theme.
        let mut registry = Registry::default();
        registry.read(
            "pair-dark.json",
            r##"{ "name": "Pair", "type": "dark", "colors": { "focusBorder": "#118811" } }"##,
            "pair-dark",
            true,
        );
        registry.read(
            "pair-light.json",
            r##"{ "name": "Pair", "type": "light", "colors": { "focusBorder": "#118811" } }"##,
            "pair-light",
            true,
        );
        assert!(registry.complaints.is_empty(), "{:?}", registry.complaints);
        assert!(registry.knows("Pair", Appearance::Dark));
        assert!(registry.knows("Pair", Appearance::Light));
        assert_ne!(
            registry.resolve("Pair", Appearance::Dark).ground,
            registry.resolve("Pair", Appearance::Light).ground
        );
    }

    #[test]
    fn somebody_elses_theme_wins_over_a_built_in_of_the_same_name() {
        // The only way of correcting a built-in that does not involve waiting
        // for a release.
        let mut registry = built_in();
        registry.read(
            "mine.json",
            r##"{ "name": "Ink", "type": "dark", "colors": { "focusBorder": "#00ff00" } }"##,
            "mine",
            true,
        );
        assert_eq!(
            crate::color::write(registry.resolve("Ink", Appearance::Dark).rope_accent),
            "#00ff00ff"
        );
        assert_eq!(registry.of(Appearance::Dark).iter().filter(|t| t.name == "Ink").count(), 1);
    }

    #[test]
    fn a_theme_with_no_name_of_its_own_is_called_after_its_file() {
        // Which is the normal shape of a theme file inside an extension: the
        // name it is shown under lives in the `package.json` beside it, and a
        // row with no name in it is a row nobody can choose.
        let mut registry = Registry::default();
        registry.read(
            "Solarized Dark.json",
            r##"{ "type": "dark", "colors": { "editor.background": "#002b36" } }"##,
            "solarized",
            true,
        );
        assert!(registry.knows("Solarized Dark", Appearance::Dark));
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
            ("empty.json", r##"{ "name": "Nothing" }"##),
            // A theme file that is nothing but syntax highlighting has no
            // opinion about a board, and a row for it would be the built-in
            // palette wearing somebody else's name.
            ("syntax-only.json", r##"{ "name": "Tokens", "tokenColors": [] }"##),
        ] {
            let before = registry.complaints.len();
            registry.read(file, bad, "x", true);
            assert_eq!(registry.complaints.len(), before + 1, "{file} should have been named");
            assert_eq!(registry.complaints.last().unwrap().file, file);
        }
        // And the two that cannot fail are still there to draw with.
        assert_eq!(registry.of(Appearance::Dark).len(), 1);
    }

    #[test]
    fn a_key_this_build_has_no_colour_for_is_no_longer_worth_a_word() {
        // The complaint this replaces was right for a format with forty keys,
        // where a misspelling hid in the same silence as a key from a later
        // build. A VS Code theme names hundreds of colours about an editor,
        // and naming every one of them on a settings page would be noise with
        // the one real complaint buried in it.
        let mut registry = Registry::default();
        registry.read(
            "mine.json",
            r##"{ "name": "Editor", "type": "dark", "colors": {
                "editor.background": "#101010",
                "editorBracketMatch.border": "#00ff88",
                "notebook.cellBorderColor": "#00ff88"
            } }"##,
            "mine",
            true,
        );
        assert!(registry.complaints.is_empty(), "{:?}", registry.complaints);
        assert_eq!(
            registry.resolve("Editor", Appearance::Dark).ground,
            crate::color::rgb(0x101010)
        );
    }

    #[test]
    fn one_mistake_is_said_once() {
        // `complain` deduplicates on the pair, which is what stops a folder
        // read twice — or a file named in two complaints for the same reason —
        // from saying the same sentence twice.
        let mut registry = Registry::default();
        for _ in 0..2 {
            registry.read("broken.json", "{ oh no", "broken", true);
        }
        assert_eq!(registry.complaints.len(), 1, "{:?}", registry.complaints);
    }

    #[test]
    fn a_built_in_never_complains_at_somebody_about_their_own_folder() {
        // The built-ins go through the same reader. A complaint about one of
        // those would send somebody looking for `ink-dark.json` in a directory
        // it has never been in — it is a bug in this build, and
        // `every_theme_this_binary_ships_can_be_read_on` is what stands in for
        // the report.
        let mut registry = Registry::default();
        registry.read("built-in.json", "{ not json at all", "", false);
        assert!(registry.complaints.is_empty());
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
            r##"{ "name": "Neon", "type": "dark", "author": "me",
                  "colors": { "focusBorder": "#00ff88" } }"##,
        )
        .unwrap();
        // Not JSON, and not a `.json` — one has to be counted and the other
        // has to be passed over without being counted, because a `README.txt`
        // sitting in the folder is not a theme that failed.
        std::fs::write(dir.join("broken.json"), "{ oh no").unwrap();
        std::fs::write(dir.join("notes.txt"), "not a theme and not claiming to be").unwrap();

        let registry = Registry::load_from(Some(&dir));
        assert_eq!(
            crate::color::write(registry.resolve("Neon", Appearance::Dark).rope_accent),
            "#00ff88ff"
        );
        assert_eq!(registry.complaints.len(), 1, "{:?}", registry.complaints);
        assert_eq!(registry.complaints[0].file, "broken.json");
        // Credited to the file it came from, which is what the settings page
        // shows beside its name — a built-in says "Built in" instead.
        let neon = registry.of(Appearance::Dark).into_iter().find(|t| t.name == "Neon").unwrap();
        assert_eq!(neon.family, "mine");
        assert_eq!(neon.author, "me");
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
        assert!(registry.complaints.is_empty());

        let missing = std::env::temp_dir().join("mbrd-themes-that-are-not-there");
        assert!(Registry::load_from(Some(&missing)).knows(DEFAULT_DARK, Appearance::Dark));
    }

    #[test]
    fn the_default_is_the_top_of_its_own_list() {
        // Where somebody goes back to, so it is not sorted into the middle.
        // Pinned by name now rather than by being the only entry with no file
        // behind it: every shipped theme is credited the same way, and "Paper"
        // does not sort above "Moss".
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
