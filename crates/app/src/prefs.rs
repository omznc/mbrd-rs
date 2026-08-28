//! What somebody set, as opposed to what the app noticed.
//!
//! A small JSON file in the **config** directory `dirs.rs` names, which is the
//! whole distinction between this module and `recent.rs`: that one is *state* —
//! a list the app kept without being asked — and this is a choice a person
//! made. A backup of somebody's settings should carry this and not that.
//!
//! It is also deliberately not part of the board. A `.mbrd` travels: it is sent
//! to other people, and `settings.desktop` inside one is about the *board* —
//! whether it has a grid, how big its cells are. How much a particular person's
//! particular eyes want the camera to move is not a fact about a moodboard, and
//! writing it into one would mean handing it to everybody the file is sent to.
//!
//! Reading is best-effort and silent, like `recent.rs`: a config that cannot be
//! read is the defaults, because the alternative is an app that will not start
//! over a stray comma.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::dirs;
use crate::themes::{Appearance, DEFAULT_DARK, DEFAULT_LIGHT};

/// Which appearance the app wears, and whether it is being told by the
/// desktop.
///
/// Three values rather than a bool with a separate "follow the system" switch,
/// because the three are one question — *what decides?* — and splitting it in
/// two produces a pair of controls where one of them is sometimes meaningless.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Whatever the desktop says, and changing when it changes.
    System,
    Light,
    Dark,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::System => "System",
            Self::Light => "Light",
            Self::Dark => "Dark",
        }
    }

    /// Which palette to wear, given what the desktop currently says.
    ///
    /// The `system` argument is only consulted for [`Mode::System`], which is
    /// the point of passing it in rather than asking the window here: this
    /// function is then pure, and the one place that has to know how to ask a
    /// window what it looks like is the one place that has a window.
    pub fn appearance(self, system: Appearance) -> Appearance {
        match self {
            Self::System => system,
            Self::Light => Appearance::Light,
            Self::Dark => Appearance::Dark,
        }
    }

    fn parse(word: &str) -> Option<Self> {
        match word {
            "system" => Some(Self::System),
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            _ => None,
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Light => "light",
            Self::Dark => "dark",
        }
    }
}

/// Which preference something in the environment might be pinning.
///
/// This used to be a `bool` naming one of two settings, which worked for
/// exactly as long as there were two. A name per setting rather than a
/// position, so that adding a third is adding an arm rather than remembering
/// which way round `true` meant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Setting {
    Motion,
    Update,
    Theme,
    Appearance,
}

/// What a board this app *makes* starts out as.
///
/// The awkward corner of this module, and worth naming rather than hiding.
/// Everything else here is an Application preference in the sense the settings
/// page means: it is about the person sitting at this computer, it never goes
/// into a `.mbrd`, and undo has no opinion about it. These two are Board
/// settings — they live in the file, they travel to whoever it is sent to, and
/// undo can take them back.
///
/// So this is deliberately **not** a way of setting them. It is a way of
/// setting what a *new* board is born with, which is a fact about this
/// computer's habits rather than about any board: changing it leaves every
/// board that already exists exactly as it was, and the welcome screen and the
/// settings page both have to say so in those words. The moment this starts
/// reaching into `self.doc` it has become a second implementation of the
/// Canvas section, which is the one thing the settings page's own note forbids.
///
/// Only the two the welcome screen asks about. The rest of `BoardSettings` is
/// left at its own defaults, because a preference nobody is offered is a
/// preference nobody can have got wrong.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NewBoard {
    pub snap: bool,
    pub grid_step: f32,
}

impl Default for NewBoard {
    /// The board format's own defaults, restated rather than invented: a
    /// person who never opens the welcome screen must get exactly the board
    /// they got before this struct existed.
    fn default() -> Self {
        let born = mbrd_core::BoardSettings::default();
        Self { snap: born.snap, grid_step: born.grid_step }
    }
}

impl NewBoard {
    /// Stamp these onto a board that has just been made.
    pub fn apply(self, settings: &mut mbrd_core::BoardSettings) {
        settings.snap = self.snap;
        settings.grid_step = self.grid_step;
    }
}

/// What somebody has chosen.
///
/// Not `Copy` any more, which it was until it carried two theme *names*. The
/// alternative was interning them or holding a pair of indices into a registry
/// that is rebuilt when somebody edits a file, and a `String` in a struct read
/// a handful of times per frame is not worth either.
#[derive(Debug, Clone, PartialEq)]
pub struct Prefs {
    /// Whether the interface is allowed to move.
    ///
    /// Off means the camera arrives rather than travelling, the marks beside a
    /// card are simply there or not, and a picture appears when it has decoded.
    /// **It does not mean less feedback** — every one of those things still
    /// happens and still says the same thing, it just happens at once. That is
    /// the distinction the reduced-motion setting on every platform draws, and
    /// getting it wrong by also removing the feedback is the usual way of
    /// making the accessible path the worse one.
    ///
    /// **Off by default.** It was on, on the usual reasoning that motion is
    /// what tells you a camera travelled rather than teleported, and the
    /// default was changed because that reasoning is about the *first* time
    /// somebody sees a transition rather than the ten-thousandth. A board is a
    /// tool somebody is inside all day, and every settle is a wait between
    /// having decided something and being able to act on it. The switch is
    /// still here, and on is still a good answer — it is just no longer the
    /// one nobody chose.
    pub motion: bool,

    /// Whether to find out that a new version exists.
    ///
    /// On by default, and off is a real answer rather than a delay: it stops
    /// the request being made at all, not just the message being shown. An app
    /// that keeps asking after being told not to has not been told not to.
    ///
    /// This is only about *looking*. Whether anything can then be installed is
    /// a property of how the app was installed rather than of what anybody
    /// chose — see `update/eligible.rs`.
    pub update: bool,

    /// Whether the app is light, dark, or doing as the desktop does.
    ///
    /// **Dark by default, not `System`.** `System` is the answer this ought to
    /// have, and it is not the default for a reason that is specific rather
    /// than cautious: on Linux gpui reads the appearance from the XDG desktop
    /// portal, a desktop that expresses *no* preference is reported as
    /// `Light`, and so is every window before the portal has answered at all.
    /// This app has been dark since it existed. Defaulting to `System` would
    /// mean a share of people opening an app they had been using for months
    /// and finding it white, having changed nothing — which is a worse
    /// first-run than the one it fixes. The switch is right there, and
    /// choosing it is a decision somebody makes once.
    pub mode: Mode,

    /// The theme worn when the app is dark, by name.
    ///
    /// A name rather than a palette, and that is the whole design: a name is
    /// what survives the file it came from being edited under it, and it is
    /// also the only thing that can still be written down when the theme is
    /// somebody else's file that this build has never seen. What happens when
    /// the name points at nothing is `themes::Registry::resolve`'s problem,
    /// and it falls back rather than blanking.
    pub theme: String,
    /// The theme worn when the app is light.
    ///
    /// A second field rather than one that is rewritten as the mode changes,
    /// because the pair is the point: somebody who follows their desktop has
    /// chosen *two* themes, and an app that remembered only the current one
    /// would forget the other every sunset.
    pub theme_light: String,

    /// Where a board this app makes is put, if somebody has said.
    ///
    /// `None` rather than the default path spelled out, and the difference is
    /// not pedantry: a person who has never been asked has *not chosen*
    /// `~/mbrd`, they have declined to have an opinion — so if the platform's
    /// answer to "where is home" ever changes underneath them, `None` follows
    /// it and a written-down path does not. It also means the settings file of
    /// somebody who took the default carries no absolute path at all, which is
    /// what makes it survivable to copy between machines.
    ///
    /// Read through [`Prefs::boards`] rather than directly, which is where the
    /// fallback lives.
    pub boards_dir: Option<PathBuf>,

    /// Whether the first-run screen has been through.
    ///
    /// State by the letter of `dirs.rs`'s split — something the app noticed
    /// rather than something anybody chose — and it lives here anyway, on
    /// purpose. The welcome screen's entire job is to collect the rest of this
    /// struct, and putting the flag that decides whether it runs in a
    /// *different file* would mean somebody restoring their settings from a
    /// backup gets all of their answers back and is asked all of the questions
    /// again. The flag belongs with the answers it is about.
    pub welcomed: bool,

    /// What a board this app makes is born with. See [`NewBoard`], whose note
    /// is about why these are here and not on the settings page's Canvas
    /// section.
    pub new_board: NewBoard,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            motion: false,
            update: true,
            mode: Mode::Dark,
            theme: DEFAULT_DARK.into(),
            theme_light: DEFAULT_LIGHT.into(),
            boards_dir: None,
            welcomed: false,
            new_board: NewBoard::default(),
        }
    }
}

impl Prefs {
    /// Whether the environment is overriding a setting, and which.
    ///
    /// A toggle that writes a choice the next launch will ignore is a toggle
    /// that lies, so whatever offers one has to be able to say "this is being
    /// forced elsewhere". Answers the variable's name, which is the only useful
    /// thing to tell somebody: it is what they have to go and unset.
    pub fn forced(what: Setting) -> Option<&'static str> {
        let name = match what {
            Setting::Motion => "MBRD_MOTION",
            Setting::Update => "MBRD_NO_UPDATE",
            Setting::Theme => "MBRD_THEME",
            Setting::Appearance => "MBRD_APPEARANCE",
        };
        std::env::var_os(name).is_some().then_some(name)
    }

    /// Which theme name this appearance wears.
    pub fn theme_for(&self, appearance: Appearance) -> &str {
        match appearance {
            Appearance::Light => &self.theme_light,
            Appearance::Dark => &self.theme,
        }
    }

    /// Choose the theme for one appearance, leaving the other alone.
    pub fn set_theme(&mut self, appearance: Appearance, name: impl Into<String>) {
        match appearance {
            Appearance::Light => self.theme_light = name.into(),
            Appearance::Dark => self.theme = name.into(),
        }
    }

    /// Where a board this app makes goes.
    ///
    /// The chosen one, or the platform's — see [`Prefs::boards_dir`] for why
    /// the unchosen case is a `None` that falls through here rather than the
    /// default path written into the file. `None` from this means there is no
    /// home directory to put anything in, which is [`dirs::boards`]'s answer
    /// and not a new failure.
    pub fn boards(&self) -> Option<PathBuf> {
        match &self.boards_dir {
            Some(chosen) => Some(chosen.clone()),
            None => dirs::boards(),
        }
    }

    /// Remember where boards go, or go back to following the platform.
    ///
    /// A path equal to the platform's answer is stored as `None` rather than
    /// as itself: somebody who browses to `~/mbrd` and picks it has chosen the
    /// default, and writing it down as an absolute path would quietly opt them
    /// out of ever tracking their home directory again.
    ///
    /// No caller yet. The control that will have one is the welcome screen's
    /// "where do boards go" step, and this is deliberately written *before* it
    /// rather than inside it: the rule above is about the stored value, not
    /// about the dialog, and a settings page that later grows the same control
    /// has to reach the same answer through the same door.
    #[allow(dead_code)]
    pub fn set_boards(&mut self, dir: Option<&Path>) {
        self.boards_dir = match dir {
            Some(dir) if Some(dir) != dirs::boards().as_deref() => Some(dir.to_path_buf()),
            _ => None,
        };
    }
}

/// Where it lives.
fn store() -> Option<PathBuf> {
    Some(dirs::config()?.join("settings.json"))
}

/// What somebody has chosen, or the defaults.
///
/// The environment variable wins, and exists because a setting somebody needs
/// in order to look at the screen without feeling ill should not require them
/// to look at the screen first. `MBRD_MOTION=0` in a launcher, a shell profile
/// or a desktop entry is a way in that needs nothing from this app.
pub fn load() -> Prefs {
    let mut prefs = Prefs::default();

    if let Some(path) = store() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            if let Ok(value) = serde_json::from_str::<Value>(&text) {
                if let Some(motion) = value.get("motion").and_then(Value::as_bool) {
                    prefs.motion = motion;
                }
                if let Some(update) = value.get("update").and_then(Value::as_bool) {
                    prefs.update = update;
                }
                // A mode this build does not recognise leaves the default
                // standing rather than being coerced to one of the three —
                // the same rule every other key here follows, and the reason
                // a settings file from a later build is survivable.
                if let Some(mode) = value.get("mode").and_then(Value::as_str).and_then(Mode::parse)
                {
                    prefs.mode = mode;
                }
                // Not checked against the registry here, deliberately. This
                // module knows nothing about which themes exist — it reads a
                // name somebody wrote down, and whether that name still points
                // at anything is a question for whoever has the registry. A
                // theme file that is missing this morning may be back this
                // afternoon, and a `load` that "corrected" the name would have
                // thrown the choice away in between.
                if let Some(name) = value.get("theme").and_then(Value::as_str) {
                    prefs.theme = name.to_string();
                }
                if let Some(name) = value.get("theme_light").and_then(Value::as_str) {
                    prefs.theme_light = name.to_string();
                }
                // An empty string is not a directory. It is what a field
                // somebody cleared by hand leaves behind, and treating it as a
                // path would put every new board at the filesystem root — the
                // same rule `dirs::from_env` applies to an exported-but-empty
                // variable, for the same reason.
                if let Some(dir) =
                    value.get("boards").and_then(Value::as_str).filter(|d| !d.is_empty())
                {
                    prefs.boards_dir = Some(PathBuf::from(dir));
                }
                if let Some(seen) = value.get("welcomed").and_then(Value::as_bool) {
                    prefs.welcomed = seen;
                }
                if let Some(snap) = value.get("newBoardSnap").and_then(Value::as_bool) {
                    prefs.new_board.snap = snap;
                }
                // Clamped to the same range the board format clamps its own
                // grid step to — see `schema.rs`. A settings file carrying
                // zero would otherwise mint boards whose snapping divides by
                // it.
                if let Some(step) = value.get("newBoardGridStep").and_then(Value::as_f64) {
                    prefs.new_board.grid_step = (step as f32).clamp(1.0, 4096.0);
                }
            }
        }
    }

    if let Some(set) = std::env::var_os("MBRD_MOTION") {
        prefs.motion = !off(&set);
    }

    // `MBRD_NO_UPDATE` rather than `MBRD_UPDATE`, because the thing anybody
    // wants from an environment variable here is to switch it *off* — in a
    // launcher, a desktop entry, or a build somebody is redistributing. Being
    // set at all is enough; its value is not read.
    if std::env::var_os("MBRD_NO_UPDATE").is_some() {
        prefs.update = false;
    }

    // The same argument `MBRD_MOTION` makes, for the same people: somebody who
    // cannot look at a bright screen should not have to look at one in order
    // to find the switch that stops it. Set in a launcher, a shell profile or
    // a desktop entry, and it needs nothing from this app.
    if let Some(word) =
        std::env::var("MBRD_APPEARANCE").ok().and_then(|w| Mode::parse(&w.to_lowercase()))
    {
        prefs.mode = word;
    }

    // A *name*, and it lands in both slots because the variable is one string
    // and there are two of them. A name that only exists as a dark theme
    // simply falls back to the light base when the app is light — which is
    // `Registry::resolve`'s ordinary behaviour rather than a special case.
    if let Ok(name) = std::env::var("MBRD_THEME") {
        if !name.is_empty() {
            prefs.theme = name.clone();
            prefs.theme_light = name;
        }
    }

    prefs
}

/// Write what somebody chose.
///
/// Best-effort and silent, like [`load`] and like `recent.rs`: a settings file
/// that cannot be written is a setting that does not persist, which is worth
/// less than an app that refuses to toggle it.
///
/// **Unknown keys are carried through.** The file is read back, the keys this
/// build knows are replaced, and everything else is left exactly as it
/// arrived — the same bargain `mbrd-core` makes with the board format, and for
/// the same reason: a settings file written by a newer build should survive
/// being opened by an older one rather than being quietly trimmed to whatever
/// this binary happens to understand.
///
/// Note what this cannot do. `MBRD_MOTION`, `MBRD_NO_UPDATE`, `MBRD_THEME` and
/// `MBRD_APPEARANCE` all win at load,
/// so toggling a setting that an environment variable is forcing will write the
/// choice and then appear not to have taken effect on the next run. That is the
/// right precedence — a variable in a launcher or a desktop entry is a
/// deliberate override by whoever set it up — but it does mean the two can
/// disagree, which is why [`Prefs::forced`] exists to say so.
pub fn save(prefs: &Prefs) {
    let Some(path) = store() else { return };

    // Read first, so that keys this build does not know about survive.
    let mut out = match std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
    {
        Some(Value::Object(map)) => map,
        _ => Map::new(),
    };
    out.insert("motion".into(), Value::Bool(prefs.motion));
    out.insert("update".into(), Value::Bool(prefs.update));
    out.insert("mode".into(), Value::String(prefs.mode.word().into()));
    out.insert("theme".into(), Value::String(prefs.theme.clone()));
    out.insert("theme_light".into(), Value::String(prefs.theme_light.clone()));
    out.insert("welcomed".into(), Value::Bool(prefs.welcomed));
    out.insert("newBoardSnap".into(), Value::Bool(prefs.new_board.snap));
    out.insert("newBoardGridStep".into(), Value::from(f64::from(prefs.new_board.grid_step)));
    // Removed rather than written as `null` or as the default path when
    // nobody has chosen one. See `Prefs::boards_dir`: the absence of the key
    // is what "I have no opinion, follow the platform" is spelled as, and a
    // key holding the platform's current answer would freeze it.
    match &prefs.boards_dir {
        Some(dir) => {
            out.insert("boards".into(), Value::String(dir.to_string_lossy().into_owned()));
        }
        None => {
            out.remove("boards");
        }
    }

    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Indented, with a newline at the end. It was one long line until the
    // settings page grew a button that opens this file in a text editor, at
    // which point "what the machine can read" stopped being the only
    // requirement — and it was never really the only one, because the way
    // anybody has ever changed a setting that has no switch is by opening this
    // file and typing in it.
    //
    // `to_string_pretty` cannot fail on a value that came out of a `Map`, but
    // the fallback is *the same settings on one line* rather than an `unwrap`
    // or an empty object: this whole function is best-effort by design, and the
    // one thing it must never do is turn "could not be formatted" into "your
    // settings are gone".
    let value = Value::Object(out);
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    let _ = std::fs::write(&path, text + "\n");
}

/// Whether an environment variable is saying no.
fn off(value: &std::ffi::OsStr) -> bool {
    matches!(value.to_string_lossy().as_ref(), "0" | "off" | "false" | "no")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_interface_holds_still_until_somebody_asks_it_not_to() {
        assert!(!Prefs::default().motion);
    }

    #[test]
    fn looking_for_updates_is_on_until_somebody_says_otherwise() {
        assert!(Prefs::default().update);
    }

    #[test]
    fn the_ways_of_saying_no_are_the_ones_people_write() {
        for no in ["0", "off", "false", "no"] {
            assert!(off(std::ffi::OsStr::new(no)), "{no} should have counted as off");
        }
        for yes in ["1", "on", "true", "yes", ""] {
            assert!(!off(std::ffi::OsStr::new(yes)), "{yes} should not have counted as off");
        }
    }

    #[test]
    fn saving_keeps_keys_this_build_does_not_know_about() {
        // The same bargain the board format makes: a settings file written by
        // a newer build must survive being opened by an older one rather than
        // being trimmed to whatever this binary understands.
        //
        // Exercised against the merge directly rather than the filesystem,
        // because `save` writes to the real config directory and a test that
        // did that would clobber whatever the person running it had chosen.
        let existing = serde_json::json!({ "motion": true, "theme": "midnight" });
        let mut out = match existing {
            Value::Object(map) => map,
            _ => unreachable!(),
        };
        out.insert("motion".into(), Value::Bool(false));
        out.insert("update".into(), Value::Bool(false));

        assert_eq!(out.get("theme").and_then(Value::as_str), Some("midnight"));
        assert_eq!(out.get("motion").and_then(Value::as_bool), Some(false));
        assert_eq!(out.get("update").and_then(Value::as_bool), Some(false));
    }

    #[test]
    fn the_file_is_written_for_somebody_to_read() {
        // The settings page has a button that opens this file in a text
        // editor, so "what the machine can read" was never the only
        // requirement — and the way anybody has ever changed a setting with no
        // switch is by opening it and typing.
        //
        // Exercised against the formatting rather than the filesystem, for the
        // reason the test below gives: `save` writes to the real config
        // directory and a test that did that would clobber whatever the person
        // running it had chosen.
        let mut out = Map::new();
        out.insert("motion".into(), Value::Bool(false));
        out.insert("theme".into(), Value::String("Ink".into()));
        let value = Value::Object(out);
        let text = serde_json::to_string_pretty(&value).expect("a map is always writable");

        assert!(text.contains("\n"), "it is not one long line");
        assert!(text.contains("  \"motion\""), "the keys are indented");
        // And it still reads back as the same settings, which is the only
        // thing prettiness is not allowed to cost.
        assert_eq!(serde_json::from_str::<Value>(&text).unwrap(), value);
    }

    #[test]
    fn an_environment_variable_that_is_not_set_forces_nothing() {
        // `forced` is what stops a toggle lying about having taken effect. In
        // a test environment none of the variables are set, so all four answer
        // None — and the point of the test is that it asks about *all* of
        // them. It used to be the only thing that made a preference added
        // without a `forced` arm show up; now that the argument is an enum,
        // the compiler catches that and this checks the arms are right.
        for what in [Setting::Motion, Setting::Update, Setting::Theme, Setting::Appearance] {
            assert_eq!(Prefs::forced(what), None, "{what:?}");
        }
    }

    #[test]
    fn the_app_is_dark_until_somebody_asks_it_to_follow_the_desktop() {
        // Not `System`, and the field's own note says why at length: on Linux
        // a desktop with no stated preference reads as *light*, and so does
        // every window before the portal has answered. Defaulting to `System`
        // would turn an app somebody has been using for months white on the
        // strength of a question their desktop never answered.
        assert_eq!(Prefs::default().mode, Mode::Dark);
    }

    #[test]
    fn following_the_desktop_is_the_only_mode_that_listens_to_it() {
        // The other two are answers, not preferences with an override.
        for system in [Appearance::Light, Appearance::Dark] {
            assert_eq!(Mode::System.appearance(system), system);
            assert_eq!(Mode::Light.appearance(system), Appearance::Light);
            assert_eq!(Mode::Dark.appearance(system), Appearance::Dark);
        }
    }

    #[test]
    fn the_two_themes_are_remembered_apart() {
        // The reason there are two fields rather than one that is rewritten as
        // the mode changes: somebody who follows their desktop has chosen a
        // pair, and an app that kept only the current one would forget the
        // other every sunset.
        let mut prefs = Prefs::default();
        prefs.set_theme(Appearance::Dark, "Ink");
        assert_eq!(prefs.theme_for(Appearance::Dark), "Ink");
        assert_eq!(prefs.theme_for(Appearance::Light), DEFAULT_LIGHT, "the other is untouched");
    }

    #[test]
    fn a_mode_this_build_does_not_recognise_leaves_the_default_standing() {
        // The same promise the rest of this file makes about a settings file
        // written by a later build: a word that means nothing here is ignored,
        // not coerced into one of the three.
        assert_eq!(Mode::parse("dusk"), None);
        assert_eq!(Mode::parse("Dark"), None, "the file's spelling is lowercase");
        for word in ["system", "light", "dark"] {
            assert_eq!(Mode::parse(word).map(Mode::word), Some(word));
        }
    }

    #[test]
    fn the_file_is_config_rather_than_state() {
        // The one thing about this module worth a test: `recent.rs` deliberately
        // writes under the *state* directory, and putting the two in the same
        // place would be the mistake that note exists to prevent.
        //
        // Asked of `dirs` rather than of the spelling of the path, because the
        // spelling is per-platform and on macOS the two are legitimately the
        // same directory — see `dirs.rs`. What has to hold everywhere is that
        // this module asks the config question and `recent.rs` asks the state
        // one, so that the split exists wherever the platform offers one.
        let path = store().expect("there is a home directory in a test environment");
        let config = dirs::config().expect("there is a home directory in a test environment");
        assert!(path.starts_with(&config), "it landed at {}", path.display());
        assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("settings.json"));
    }
}
