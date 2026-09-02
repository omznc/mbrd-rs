//! The first run.
//!
//! Four questions, asked once, on the surface every other page in this app
//! uses — see `settings.rs` and `stock.rs` for the shape and `Overlay` in
//! `board_view.rs` for why there can only ever be one of them up at a time.
//!
//! ## What it is allowed to ask
//!
//! Only things that are true before there is a board. Appearance, where boards
//! should live, whether the interface moves and whether it looks for updates —
//! every one of them is an *Application* preference in the sense the settings
//! page's own note means: about the person sitting here, kept in their config
//! directory, never written into a `.mbrd` and never undoable.
//!
//! The one apparent exception is the pair under "Defaults for new boards",
//! and it is worth being exact about why it is not one. Snapping and the grid
//! step *are* Board settings. This page does not set them: it sets what a
//! board this computer makes is **born** with, which is a habit of this
//! installation rather than a property of any board. Nothing here reaches into
//! the board that happens to be open, the heading says so in words, and
//! `prefs::NewBoard` carries the same note next to the fields themselves. The
//! moment that stops being true this page has become a second Canvas section,
//! which is the one thing `settings.rs` forbids.
//!
//! ## Nothing here is a second implementation
//!
//! Every control on this page is the settings page's own — `switch_at`,
//! `segmented`, `dropdown` and `picker_panel` are all imported from
//! `settings.rs` rather than restated, and the two rows that are commands run
//! the same `Command` the menus and the palette run. A welcome screen whose
//! toggles were copies would be a welcome screen that drifts, and it would
//! drift on the one screen where nobody has learnt what the app usually does
//! yet.
//!
//! ## And it is skippable in one press
//!
//! Escape at any point, from any page, and the answers already given are
//! kept — every control writes through to `prefs::save` the moment it is
//! pressed, exactly as it would on the settings page. There is no *Finish*
//! that commits and therefore no way to lose four answers by leaving before
//! it. The last page has no Next for the same reason: it is not a summary to
//! be confirmed, it is four doors out.

use gpui::{
    div, prelude::*, px, AnyElement, Context, FontWeight, Modifiers, MouseButton, SharedString,
};

use crate::board_view::BoardView;
use crate::color::Tint;
use crate::command::Command;
use crate::editor::{self, Editor};
use crate::icons::{icon, Icon, ICON_LG, ICON_MD, ICON_SM};
use crate::prefs::Mode;
use crate::settings::{self, Picker};
use crate::theme::Theme;
use crate::themes::Appearance;

/// The longest a boards folder may be typed to.
///
/// Well past any real path and short of the length at which a single-line
/// field stops being one. Paths have no portable maximum worth quoting here —
/// this is a field's limit, not a filesystem's.
const PATH_MAX: usize = 512;

/// How wide the live preview is drawn, and how tall.
///
/// One pair of numbers rather than two, because the cards inside the preview
/// are placed as fractions of it — see [`chip`] — and a card that thinks the
/// panel is 352 wide while the panel is 300 wide sits somewhere the ropes
/// behind it do not go.
const PREVIEW_W: f32 = 352.0;
const PREVIEW_H: f32 = 186.0;

/// One of the four pages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Appearance,
    Boards,
    Behaviour,
    Started,
}

impl Step {
    pub const ALL: [Self; 4] = [Self::Appearance, Self::Boards, Self::Behaviour, Self::Started];

    /// The word in the rail, which is deliberately shorter than the heading
    /// over the page it leads to: a rail is read sideways at a glance and a
    /// heading is read.
    fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance",
            Self::Boards => "Boards folder",
            Self::Behaviour => "Behaviour",
            Self::Started => "Get started",
        }
    }

    fn at(self) -> usize {
        Self::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
}

/// The open screen.
///
/// Holds no answers. Every control reads its state off the prefs each frame
/// and writes through on press — the same bargain `settings::Page` makes, and
/// for the sharper version of the same reason: a page that collected answers
/// into itself and applied them at the end would be a page that loses them
/// when somebody presses Escape, which is the one thing it promises not to do.
#[derive(Debug, Clone)]
pub struct Welcome {
    pub step: Step,
    /// The boards folder, as typed. Seeded from the prefs when the screen
    /// opens and written back as it changes — see [`Welcome::folder_text`].
    pub folder: Editor,
    /// Whether the folder field is wearing the keyboard. Only ever true on
    /// [`Step::Boards`], which is the only page with anything to type into.
    pub focused: bool,
    pub picking: Option<Picker>,
}

impl Welcome {
    /// Open on the first page, with the folder field showing wherever boards
    /// currently go.
    ///
    /// The field starts filled rather than empty with a placeholder, because
    /// the default is an answer and not the absence of one — an empty field
    /// would read as a question nobody has answered when in fact `~/mbrd` is
    /// what will happen if it is left alone.
    pub fn open(boards: Option<&std::path::Path>) -> Self {
        let shown = boards.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        Self {
            step: Step::Appearance,
            folder: Editor::new(shown, PATH_MAX, false),
            focused: false,
            picking: None,
        }
    }

    /// What the folder field says, trimmed, or `None` for a field somebody has
    /// emptied.
    ///
    /// Emptying it is a real answer and means "follow the platform" — the same
    /// thing `Prefs::boards_dir` spells as `None`, and the reason this returns
    /// an `Option` rather than defaulting the blank back to the current path.
    pub fn folder_text(&self) -> Option<std::path::PathBuf> {
        let typed = self.folder.text().trim();
        (!typed.is_empty()).then(|| std::path::PathBuf::from(typed))
    }

    /// Move through the pages, stopping at both ends.
    ///
    /// Clamped rather than wrapping. Wrapping a four-step rail means Next on
    /// the last page silently returns to the first, which reads as the screen
    /// having restarted itself.
    pub fn go(&mut self, by: isize) {
        let at = (self.step.at() as isize + by).clamp(0, Step::ALL.len() as isize - 1);
        self.step = Step::ALL[at as usize];
        // Leaving the folder page puts the field down. Carrying `focused`
        // onto a page with no field would leave a caret drawn on the page
        // somebody moved to.
        if self.step != Step::Boards {
            self.focused = false;
        }
    }

    pub fn show(&mut self, step: Step) {
        self.step = step;
        if step != Step::Boards {
            self.focused = false;
        }
    }

    /// One key press.
    ///
    /// `names` is the theme list a picker is choosing from, empty except while
    /// one is open — passed in for the reason `settings::Page::key` documents:
    /// the registry lives on the view and this is a plain struct.
    pub fn key(
        &mut self,
        key: &str,
        mods: Modifiers,
        text: Option<&str>,
        names: &[String],
    ) -> settings::Reply {
        if self.picking.is_some() {
            return settings::picker_key(&mut self.picking, key, mods, text, names);
        }

        // Out, from anywhere, keeping every answer already given. See the
        // module note on why there is nothing to lose by leaving.
        if key == "escape" {
            return settings::Reply::Close;
        }

        // The one page with a text field takes the keys first, so that typing
        // a path containing an `n` does not walk the rail.
        if self.step == Step::Boards {
            if key == "enter" {
                self.go(1);
                return settings::Reply::Held;
            }
            let reply = self.folder.key(key, editor::Mods::from(mods), text);
            if reply != editor::Reply::Ignored {
                self.focused = true;
                return settings::Reply::Folder;
            }
            if mods.secondary() && key == "v" {
                return settings::Reply::Paste;
            }
        }

        match key {
            // Enter is Next everywhere but the last page, where `go` clamps
            // and it does nothing — which is right: the last page's answer is
            // which of the four doors, and there is no default among them.
            "enter" | "right" | "tab" => self.go(1),
            "left" => self.go(-1),
            _ => {}
        }
        settings::Reply::Held
    }

    /// Paste into whichever field is currently taking keys.
    pub fn insert(&mut self, text: &str) {
        match &mut self.picking {
            Some(picker) => picker.query.insert(text),
            None => self.folder.insert(text),
        }
    }
}

// ---------------------------------------------------------------------------
// The page
// ---------------------------------------------------------------------------

pub fn render(screen: &Welcome, view: &BoardView, cx: &mut Context<BoardView>) -> impl IntoElement {
    let theme = view.theme;
    let arriving = crate::board_view::arrival(view.overlay_presence.value());
    let last = screen.step == Step::Started;

    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .bottom_0()
        .flex()
        .justify_center()
        // A page rather than a panel, so the ground is solid and there is
        // nothing behind to scrim. Same as the settings page.
        .bg(theme.ground.opacity(arriving.ground))
        .text_color(theme.text)
        .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .child(
            div()
                .w_full()
                .max_w(px(880.0))
                .h_full()
                .flex()
                .flex_col()
                .gap(px(24.0))
                .pt(px(34.0))
                .pb(px(26.0))
                .px(px(24.0))
                .opacity(arriving.content)
                .mt(px(arriving.rise))
                .child(heading(last, theme, cx))
                .child(rail(screen, view, cx))
                .child(
                    div()
                        .id("welcome-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .child(body(screen, view, cx)),
                ),
        )
        .when_some(screen.picking.as_ref(), |d, picker| {
            d.child(settings::picker_panel(picker, view, cx))
        })
}

/// The title, and the way out.
///
/// The heading changes on the last page and the way out changes with it:
/// "Skip setup" is honest while there are questions left and a lie once there
/// are none, at which point the same button is simply Close. Both are the same
/// key, which is what the chip says out loud.
fn heading(last: bool, theme: Theme, cx: &mut Context<BoardView>) -> AnyElement {
    let (title, blurb, word) = match last {
        false => (
            "Set up mbrd",
            "Four short questions. Everything here can be changed later in Settings.",
            "Skip setup",
        ),
        true => ("You’re set up", "All of it is in Settings if you change your mind.", "Close"),
    };
    div()
        .flex_none()
        .flex()
        .items_end()
        .justify_between()
        .gap(px(16.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(7.0))
                .min_w_0()
                .child(div().text_size(px(25.0)).font_weight(FontWeight::SEMIBOLD).child(title))
                .child(div().text_size(px(13.0)).text_color(theme.muted).child(blurb)),
        )
        .child(
            div()
                .id("welcome-close")
                .flex_none()
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(11.0))
                .py(px(5.0))
                .rounded(px(crate::theme::RADIUS_SM))
                .border_1()
                .border_color(theme.chrome_edge)
                .text_size(px(12.0))
                .text_color(theme.muted)
                .hover(|s| s.bg(theme.accent.opacity(0.10)).text_color(theme.text))
                .active(|s| s.bg(theme.accent.opacity(0.18)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _event, _window, cx| {
                        this.close_welcome(cx);
                        cx.stop_propagation();
                    }),
                )
                .child(word)
                .child(div().text_size(px(10.5)).text_color(theme.tertiary).child("Esc")),
        )
        .into_any_element()
}

/// The four steps, and the two buttons that walk them.
///
/// Every step is pressable, not just the next one. A rail that only went
/// forwards would make "what did I say to the first question" a thing you
/// cannot check without finishing — and the answers are all live, so there is
/// nothing to invalidate by going back.
fn rail(screen: &Welcome, view: &BoardView, cx: &mut Context<BoardView>) -> AnyElement {
    let theme = view.theme;
    let at = screen.step.at();
    let mut row = div().flex_none().flex().items_center().gap(px(8.0));

    for (i, step) in Step::ALL.into_iter().enumerate() {
        if i > 0 {
            row = row.child(div().w(px(16.0)).h(px(1.0)).bg(theme.chrome_edge));
        }
        let here = i == at;
        let done = i < at;
        row = row.child(
            div()
                .id(SharedString::from(format!("welcome-step-{i}")))
                .flex()
                .items_center()
                .gap(px(7.0))
                .px(px(10.0))
                .py(px(5.0))
                .rounded(px(crate::theme::RADIUS_SM))
                .when(here, |d| d.bg(theme.accent.opacity(0.12)))
                .when(!here, |d| d.hover(|s| s.bg(theme.chrome)))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _event, _window, cx| {
                        this.show_welcome_step(step, cx);
                        cx.stop_propagation();
                    }),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_none()
                        .size(px(18.0))
                        .rounded_full()
                        .when(here, |d| d.bg(theme.accent))
                        .when(!here, |d| d.border_1().border_color(theme.chrome_edge))
                        .text_size(px(10.0))
                        // A step already answered wears a tick rather than its
                        // number: the number is only useful while it is still
                        // telling you how far there is to go.
                        .when(done, |d| d.child(icon(Icon::Check, 9.0, theme.accent_text)))
                        .when(!done, |d| {
                            d.text_color(if here { theme.ground } else { theme.muted })
                                .child(format!("{}", i + 1))
                        }),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .text_color(if here { theme.text } else { theme.muted })
                        .child(step.label()),
                ),
        );
    }

    row.child(div().flex_1())
        .child(walk("welcome-back", "Back", at > 0, -1, theme, cx))
        // No Next on the last page. There is nothing after it, and a button
        // that did nothing would be the screen pretending to have a fifth
        // question.
        .when(at + 1 < Step::ALL.len(), |d| {
            d.child(walk("welcome-next", "Next", true, 1, theme, cx))
        })
        .into_any_element()
}

/// Back or Next. Accented forwards, outlined back — the two are not equals,
/// and drawing them as a pair of identical buttons would make the way on as
/// hard to find as the way back.
fn walk(
    id: &'static str,
    word: &'static str,
    live: bool,
    by: isize,
    theme: Theme,
    cx: &mut Context<BoardView>,
) -> AnyElement {
    let forward = by > 0;
    div()
        .id(id)
        .flex_none()
        .px(px(if forward { 15.0 } else { 13.0 }))
        .py(px(5.0))
        .rounded(px(crate::theme::RADIUS_SM))
        .text_size(px(12.5))
        .when(forward, |d| {
            d.bg(theme.accent).text_color(theme.ground).font_weight(FontWeight::MEDIUM)
        })
        .when(!forward && live, |d| {
            d.border_1().border_color(theme.chrome_edge).text_color(theme.text)
        })
        // A Back with nowhere to go is drawn as the shape of a button that is
        // not one, rather than removed: the row would otherwise reflow under
        // the pointer the moment somebody reached the second page.
        .when(!live, |d| d.text_color(theme.tertiary))
        .when(live, |d| {
            d.hover(|s| s.opacity(0.88)).active(|s| s.opacity(0.75)).on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, _event, _window, cx| {
                    this.walk_welcome(by, cx);
                    cx.stop_propagation();
                }),
            )
        })
        .child(word)
        .into_any_element()
}

fn body(screen: &Welcome, view: &BoardView, cx: &mut Context<BoardView>) -> AnyElement {
    match screen.step {
        Step::Appearance => appearance(view, cx),
        Step::Boards => boards(screen, view, cx),
        Step::Behaviour => behaviour(view, cx),
        Step::Started => started(view, cx),
    }
}

/// The heading over one page's contents, and the sentence under it.
fn asked(title: &'static str, blurb: &'static str, theme: Theme) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(4.0))
        .child(div().text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).child(title))
        .child(div().text_size(px(12.5)).text_color(theme.muted).child(blurb))
        .into_any_element()
}

/// The small capital label over a group inside a page.
fn over(words: &'static str, theme: Theme) -> AnyElement {
    div()
        .text_size(px(10.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.tertiary)
        .child(words.to_uppercase())
        .into_any_element()
}

// ---------------------------------------------------------------------------
// 1 · Appearance
// ---------------------------------------------------------------------------

fn appearance(view: &BoardView, cx: &mut Context<BoardView>) -> AnyElement {
    let theme = view.theme;
    let modes = [Mode::System, Mode::Light, Mode::Dark];
    let labels: Vec<String> = modes.iter().map(|m| m.label().to_string()).collect();
    let worn = view.appearance();

    let mut choices = div().flex_1().min_w_0().flex().flex_col().gap(px(14.0)).child(
        div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .child(over("What decides", theme))
            // Wrapped in a row so the control is as wide as its three words.
            // A `flex_col` stretches its children across, and a segmented
            // control stretched to the column's width reads as a toolbar
            // rather than as one setting with three answers — the container
            // is the whole of what says the options belong together.
            .child(div().flex().child(settings::segmented(
                "welcome-mode",
                &labels,
                modes.iter().position(|&m| m == view.prefs.mode),
                |this, at, cx| this.set_mode([Mode::System, Mode::Light, Mode::Dark][at], cx),
                view,
                cx,
            ))),
    );

    // Both slots, always, and in the order the app is most likely wearing —
    // the same argument the settings page makes for showing the pair rather
    // than only the live one: choosing a pair once is the whole point, and the
    // half you are not looking at has to be reachable while you are not
    // looking at it.
    let mut pair = div().flex().flex_col().gap(px(8.0));
    for appearance in [Appearance::Dark, Appearance::Light] {
        let name = view.prefs.theme_for(appearance).to_string();
        let known = view.themes.knows(&name, appearance);
        let id = match appearance {
            Appearance::Dark => "welcome-theme-dark",
            Appearance::Light => "welcome-theme-light",
        };
        pair = pair.child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(16.0))
                .child(div().text_size(px(12.5)).child(format!("{} theme", appearance.label())))
                .child(settings::dropdown(
                    id,
                    appearance,
                    &name,
                    known,
                    worn == appearance,
                    theme,
                    cx,
                    |this, appearance, cx| this.pick_welcome_theme(appearance, cx),
                )),
        );
    }
    choices = choices.child(pair);

    // Where a theme comes from, said once here so that the answer to "these
    // two are all there is?" is on the screen that asks the question.
    if let Some(path) = crate::dirs::themes() {
        choices = choices.child(
            div()
                .text_size(px(11.5))
                .text_color(theme.tertiary)
                .line_height(gpui::relative(1.5))
                .child(format!("Drop a theme family into {} and it appears here.", path.display())),
        );
    }

    div()
        .flex()
        .flex_col()
        .gap(px(16.0))
        .child(asked(
            "Appearance",
            "Pick the pair once. mbrd follows whichever half your desktop is wearing.",
            theme,
        ))
        .child(
            div().flex().gap(px(22.0)).child(choices).child(
                div()
                    // Its natural width, but it yields before it is cut
                    // off: the preview is the argument for the choice to
                    // its left, and half a preview makes it badly.
                    .flex_shrink_1()
                    .w(px(PREVIEW_W))
                    .min_w(px(216.0))
                    .flex()
                    .flex_col()
                    .gap(px(7.0))
                    .child(over("Live preview", theme))
                    .child(miniature(theme))
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(theme.tertiary)
                            .child("Recolours as you choose. This is the board, not a sample."),
                    ),
            ),
        )
        .into_any_element()
}

/// A board, small.
///
/// Every colour in it is a token off the live [`Theme`], which is the whole
/// point: it is not a picture of a board, it is the same arithmetic the board
/// itself is drawn with at a size that fits beside the controls. Choosing a
/// theme two inches to the left recolours this in the same frame, so the
/// question "what will that look like" is answered before it is asked.
///
/// The dots and the two connections are painted rather than laid out. A
/// hundred-odd dots as `div`s would each cost an insert into gpui's bounds
/// tree — see the note over the board's own grid, which is about exactly this
/// — and a curve is not a rectangle at all.
fn miniature(theme: Theme) -> AnyElement {
    const W: f32 = PREVIEW_W;
    const H: f32 = PREVIEW_H;
    /// Where each card sits, as `[x, y, w, h]`, and which token fills it.
    const STEP: f32 = 14.0;

    div()
        .relative()
        .h(px(H))
        .rounded(px(crate::theme::RADIUS_MD))
        .border_1()
        .border_color(theme.chrome_edge)
        .bg(theme.ground)
        .overflow_hidden()
        .child(
            gpui::canvas(
                |_, _, _| {},
                move |bounds, _: (), window, _| {
                    let x0 = f32::from(bounds.origin.x);
                    let y0 = f32::from(bounds.origin.y);
                    let w = f32::from(bounds.size.width);
                    let h = f32::from(bounds.size.height);

                    // The grid, in one layer, for the reason the board's own
                    // grid is: a field of dots does not overlap itself, so the
                    // bounds tree can be asked once instead of per dot.
                    if theme.grid.alpha > 0.001 || true {
                        // The board computes the grid's alpha from the zoom
                        // rather than reading the token's — see `Theme::grid`.
                        // At this size there is one honest answer, and it is
                        // the one the board lands on when a board is at rest.
                        let dot = theme.axis;
                        window.paint_layer(bounds, |window| {
                            let mut y = y0 + STEP;
                            while y < y0 + h {
                                let mut x = x0 + STEP;
                                while x < x0 + w {
                                    window.paint_quad(gpui::fill(
                                        gpui::Bounds {
                                            origin: gpui::point(px(x), px(y)),
                                            size: gpui::size(px(1.0), px(1.0)),
                                        },
                                        dot,
                                    ));
                                    x += STEP;
                                }
                                y += STEP;
                            }
                        });
                    }

                    // Two connections, because one is a line and two are a
                    // vocabulary: the accented one and a plain dashed-looking
                    // run at half weight say that a rope has a colour and a
                    // weight without needing a legend.
                    let p = |x: f32, y: f32| gpui::point(px(x0 + x), px(y0 + y));
                    let scale_x = w / W;
                    let scale_y = h / H;
                    let q = |x: f32, y: f32| p(x * scale_x, y * scale_y);
                    if let Some(path) = crate::board_view::ribbon(
                        &[[q(130.0, 61.0), q(163.0, 68.0)], [q(163.0, 68.0), q(196.0, 75.0)]],
                        0.8,
                    ) {
                        window.paint_path(path, theme.rope_accent);
                    }
                    if let Some(path) =
                        crate::board_view::ribbon(&[[q(103.0, 96.0), q(103.0, 124.0)]], 0.7)
                    {
                        window.paint_path(path, theme.rope_line);
                    }
                },
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
        // Four cards, one of each thing a board is mostly made of: a picture,
        // a note, a note in one of the four tints, and a colour.
        .child(chip(26.0, 26.0, 104.0, 70.0, theme.card, theme.card_edge))
        .child(
            chip(196.0, 44.0, 96.0, 62.0, theme.card, theme.card_edge)
                .p(px(8.0))
                .flex()
                .flex_col()
                .gap(px(5.0))
                .child(line(0.70, 5.0, theme.muted))
                .child(line(1.00, 4.0, theme.card_edge))
                .child(line(0.86, 4.0, theme.card_edge))
                .child(line(0.60, 4.0, theme.card_edge)),
        )
        .child(chip(64.0, 124.0, 78.0, 44.0, theme.notes[0], theme.card_edge))
        .child(chip(214.0, 126.0, 44.0, 44.0, theme.rope_leaf, theme.rope_leaf))
        .into_any_element()
}

/// One card in the miniature.
///
/// Placed as a fraction of the panel rather than at a pixel, so the whole
/// thing scales with whatever width the column ends up with. The alternative
/// is a miniature that is correct at exactly one window size and clipped at
/// every narrower one — and the canvas painting the dots and the ropes behind
/// these already works in fractions, so this is what keeps the two layers
/// agreeing.
fn chip(x: f32, y: f32, w: f32, h: f32, fill: gpui::Hsla, edge: gpui::Hsla) -> gpui::Div {
    div()
        .absolute()
        .left(gpui::relative(x / PREVIEW_W))
        .top(gpui::relative(y / PREVIEW_H))
        .w(gpui::relative(w / PREVIEW_W))
        .h(gpui::relative(h / PREVIEW_H))
        .rounded(px(crate::theme::RADIUS_SM))
        .bg(fill)
        .border_1()
        .border_color(edge)
}

/// A line of writing on the miniature's note, as the block it reads as at this
/// size. Words would be unreadable and a lie about the font.
fn line(fraction: f32, height: f32, colour: gpui::Hsla) -> gpui::Div {
    div().w(gpui::relative(fraction)).h(px(height)).rounded(px(2.0)).bg(colour)
}

// ---------------------------------------------------------------------------
// 2 · Where boards live
// ---------------------------------------------------------------------------

fn boards(screen: &Welcome, view: &BoardView, cx: &mut Context<BoardView>) -> AnyElement {
    let theme = view.theme;
    let default = crate::dirs::boards();
    let typed = screen.folder_text();
    // Whether what is in the field is the platform's own answer. Worth saying,
    // because "no opinion" and "the same path, written down" behave
    // differently on a machine whose home directory moves — see
    // `Prefs::set_boards`.
    let is_default = typed.is_none() || typed.as_deref() == default.as_deref();

    div()
        .flex()
        .flex_col()
        .gap(px(12.0))
        .child(asked(
            "Where your boards live",
            "Ctrl N makes a board here, and the switcher looks here first.",
            theme,
        ))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .child(
                    div()
                        .id("welcome-folder")
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(8.0))
                        .px(px(10.0))
                        .py(px(6.0))
                        .rounded(px(crate::theme::RADIUS_SM))
                        .bg(theme.chrome)
                        .border_1()
                        .border_color(if screen.focused { theme.accent } else { theme.chrome_edge })
                        .cursor_text()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _event, _window, cx| {
                                this.focus_welcome_folder(cx);
                                cx.stop_propagation();
                            }),
                        )
                        .child(icon(Icon::Folder, ICON_MD, theme.accent))
                        .child(div().flex_1().min_w_0().text_size(px(12.0)).child(
                            crate::palette::query_line(
                                &screen.folder,
                                "Follow this computer’s usual place",
                                12.0,
                                screen.focused,
                                &theme,
                            ),
                        )),
                )
                .child(settings::button(
                    "welcome-browse",
                    "Browse…",
                    true,
                    theme,
                    cx,
                    |this, cx| this.browse_for_boards(cx),
                )),
        )
        // What will actually happen, in the words of the path it will happen
        // in. A field is a promise and this is the receipt for it.
        .child(
            div()
                .text_size(px(11.5))
                .text_color(theme.tertiary)
                .line_height(gpui::relative(1.5))
                .child(match (&typed, &default) {
                    (None, Some(fallback)) => format!(
                        "Left blank, so boards go wherever this computer keeps them — {} today.",
                        fallback.display()
                    ),
                    (None, None) => {
                        "This computer has no home directory, so there is nowhere to put a new \
                         board until you name a folder."
                            .to_string()
                    }
                    (Some(dir), _) if is_default => {
                        format!("{} — this computer's usual place.", dir.display())
                    }
                    (Some(dir), _) => format!(
                        "New boards go in {}. It is made the first time one is saved.",
                        dir.display()
                    ),
                }),
        )
        // Boards that already exist do not move. The one thing somebody might
        // reasonably fear from this field, answered without being asked.
        .child(
            div()
                .text_size(px(11.5))
                .text_color(theme.muted)
                .child("Boards you already have stay where they are."),
        )
        .into_any_element()
}

// ---------------------------------------------------------------------------
// 3 · How it behaves
// ---------------------------------------------------------------------------

fn behaviour(view: &BoardView, cx: &mut Context<BoardView>) -> AnyElement {
    let theme = view.theme;
    let motion_note = crate::prefs::Prefs::forced(crate::prefs::Setting::Motion)
        .map(|var| format!("Set by {var}, which wins at startup."));
    let update_note = crate::prefs::Prefs::forced(crate::prefs::Setting::Update)
        .map(|var| format!("Set by {var}, which wins at startup."));

    let steps: Vec<String> =
        settings::GRID_STEPS.iter().map(|s| format!("{}", *s as i32)).collect();
    let chosen =
        settings::GRID_STEPS.iter().position(|s| (*s - view.prefs.new_board.grid_step).abs() < 0.5);

    div()
        .flex()
        .flex_col()
        .child(asked(
            "How it should behave",
            "The same rows as Settings, with the same words. Nothing here is a second \
             implementation.",
            theme,
        ))
        .child(div().h(px(4.0)))
        .child(row(
            Command::ToggleMotion.label(),
            motion_note.unwrap_or_else(|| {
                "Let the interface move. Turn off to land every change instantly.".into()
            }),
            command_switch(Command::ToggleMotion, view, cx),
            theme,
        ))
        .child(row(
            Command::ToggleUpdateChecks.label(),
            update_note.unwrap_or_else(|| {
                "Check quietly at startup and say so in the top bar when one exists.".into()
            }),
            command_switch(Command::ToggleUpdateChecks, view, cx),
            theme,
        ))
        // The heading is load-bearing rather than decorative: it is the whole
        // of what stops the two rows under it reading as a change to the board
        // that is open. See the module note, and `prefs::NewBoard`.
        .child(div().h(px(18.0)))
        .child(asked(
            "Defaults for new boards",
            "Board settings, so these are the starting point for boards you make from now on, \
             not a change to any you already have.",
            theme,
        ))
        .child(div().h(px(4.0)))
        .child(row(
            "Snap to grid",
            "New boards start with cards landing on the grid step.",
            settings::switch_at(
                "welcome-new-snap",
                view.prefs.new_board.snap,
                view,
                cx,
                |this, _window, cx| this.set_new_board_snap(!this.prefs.new_board.snap, cx),
            ),
            theme,
        ))
        .child(row(
            "Grid step",
            "How far a nudge with Shift travels, and what snapping snaps to.",
            settings::segmented(
                "welcome-new-step",
                &steps,
                chosen,
                |this, at, cx| this.set_new_board_step(settings::GRID_STEPS[at], cx),
                view,
                cx,
            ),
            theme,
        ))
        .into_any_element()
}

/// A switch that runs a [`Command`], so this page and the settings page cannot
/// disagree about what the row does or what state it is in.
fn command_switch(command: Command, view: &BoardView, cx: &mut Context<BoardView>) -> AnyElement {
    let on = command.ticked(view) == Some(true);
    settings::switch_at(command.label(), on, view, cx, move |this, window, cx| {
        command.run(this, window, cx);
        command.ticked(this) == Some(true)
    })
}

/// One setting: a name, the sentence under it, and its control at the edge.
///
/// The same shape `settings::Spec::into_row` draws, restated here only because
/// that one carries a `Section` this page has no business inventing one of.
/// The measurements are the settings page's, so a person who has seen this
/// screen recognises that one.
fn row(
    title: impl Into<SharedString>,
    about: impl Into<SharedString>,
    control: AnyElement,
    theme: Theme,
) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(24.0))
        .py(px(11.0))
        .border_t_1()
        .border_color(theme.chrome_edge.opacity(0.6))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .min_w_0()
                .child(div().text_size(px(13.0)).child(title.into()))
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(theme.muted)
                        .line_height(gpui::relative(1.4))
                        .child(about.into()),
                ),
        )
        .child(div().flex_none().child(control))
        .into_any_element()
}

// ---------------------------------------------------------------------------
// 4 · Four doors
// ---------------------------------------------------------------------------

fn started(view: &BoardView, cx: &mut Context<BoardView>) -> AnyElement {
    let theme = view.theme;
    div()
        .flex()
        .flex_col()
        .gap(px(14.0))
        .child(div().text_size(px(15.0)).font_weight(FontWeight::SEMIBOLD).child("Get started"))
        .child(
            div()
                .flex()
                .gap(px(12.0))
                .child(door(
                    "welcome-door-new",
                    Icon::NewBoard,
                    "Create a board",
                    "Name it, and land on empty paper.",
                    true,
                    theme,
                    cx,
                    |this, window, cx| {
                        this.close_welcome(cx);
                        Command::NewBoard.run(this, window, cx);
                    },
                ))
                .child(door(
                    "welcome-door-open",
                    Icon::Folder,
                    "Open a board",
                    "The switcher, or a file anywhere on disk.",
                    false,
                    theme,
                    cx,
                    |this, window, cx| {
                        this.close_welcome(cx);
                        this.open_switcher(window, cx);
                    },
                ))
                .child(door(
                    "welcome-door-demo",
                    Icon::Explore,
                    "Look around the demo",
                    "A few cards and a note on how to move.",
                    false,
                    theme,
                    cx,
                    |this, _window, cx| this.close_welcome(cx),
                ))
                .child(door(
                    "welcome-door-tour",
                    Icon::Tour,
                    "Take the tour",
                    "Every stop on the demonstration board.",
                    false,
                    theme,
                    cx,
                    |this, window, cx| {
                        this.close_welcome(cx);
                        Command::Tour.run(this, window, cx);
                    },
                )),
        )
        .into_any_element()
}

/// One way out of this screen.
///
/// The first is accented because it is the one most people want and a row of
/// four identical cards is a row with no answer in it. The other three are
/// outlined — offers rather than defaults.
#[allow(clippy::too_many_arguments)]
fn door(
    id: &'static str,
    mark: Icon,
    title: &'static str,
    blurb: &'static str,
    first: bool,
    theme: Theme,
    cx: &mut Context<BoardView>,
    press: fn(&mut BoardView, &mut gpui::Window, &mut Context<BoardView>),
) -> AnyElement {
    div()
        .id(id)
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(8.0))
        .p(px(16.0))
        .rounded(px(crate::theme::RADIUS_MD))
        .border_1()
        .border_color(if first { theme.accent } else { theme.chrome_edge })
        .bg(if first { theme.accent.opacity(0.10) } else { theme.chrome })
        .hover(|s| s.bg(theme.accent.opacity(if first { 0.18 } else { 0.08 })))
        .active(|s| s.bg(theme.accent.opacity(0.26)))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |this, _event, window, cx| {
                press(this, window, cx);
                cx.stop_propagation();
            }),
        )
        .child(icon(mark, ICON_LG, if first { theme.accent_text } else { theme.muted }))
        .child(div().text_size(px(13.0)).font_weight(FontWeight::SEMIBOLD).child(title))
        .child(
            div()
                .text_size(px(11.5))
                .text_color(theme.muted)
                .line_height(gpui::relative(1.45))
                .child(blurb),
        )
        .into_any_element()
}

/// The mark beside a hint, at the one size hints use.
#[allow(dead_code)]
fn hint_mark(mark: Icon, theme: Theme) -> gpui::Svg {
    icon(mark, ICON_SM, theme.tertiary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Welcome {
        Welcome::open(Some(std::path::Path::new("/home/somebody/mbrd")))
    }

    #[test]
    fn the_rail_stops_at_both_ends_rather_than_wrapping() {
        // Wrapping would make Next on the last page look like the screen had
        // restarted itself.
        let mut w = screen();
        w.go(-1);
        assert_eq!(w.step, Step::Appearance);
        for _ in 0..10 {
            w.go(1);
        }
        assert_eq!(w.step, Step::Started);
    }

    #[test]
    fn escape_leaves_from_every_page() {
        // The promise the module note makes: there is no page you can get
        // stuck on and no answer that is lost by leaving.
        for step in Step::ALL {
            let mut w = screen();
            w.show(step);
            assert_eq!(
                w.key("escape", Modifiers::default(), None, &[]),
                settings::Reply::Close,
                "{step:?}"
            );
        }
    }

    #[test]
    fn typing_a_path_does_not_walk_the_rail() {
        // The bug this exists to prevent: `n` is a letter in half the paths
        // anybody would type, and a rail that took it would move the page out
        // from under the field mid-word.
        let mut w = screen();
        w.show(Step::Boards);
        w.folder = Editor::new("", PATH_MAX, false);
        for letter in ["n", "o", "t", "e", "s"] {
            w.key(letter, Modifiers::default(), Some(letter), &[]);
        }
        assert_eq!(w.step, Step::Boards, "the field kept the keys");
        assert_eq!(w.folder.text(), "notes");
    }

    #[test]
    fn an_emptied_folder_field_means_follow_the_platform() {
        // Not "the path that was there before". Emptying it is an answer, and
        // it is the same answer `Prefs::boards_dir` spells as `None`.
        let mut w = screen();
        assert!(w.folder_text().is_some());
        w.folder = Editor::new("   ", PATH_MAX, false);
        assert_eq!(w.folder_text(), None);
    }

    #[test]
    fn leaving_the_folder_page_puts_the_field_down() {
        // A caret left drawn on a page with no field in it.
        let mut w = screen();
        w.show(Step::Boards);
        w.focused = true;
        w.go(1);
        assert!(!w.focused);
    }

    #[test]
    fn every_step_is_reachable_and_knows_where_it_is() {
        for (i, step) in Step::ALL.into_iter().enumerate() {
            assert_eq!(step.at(), i, "{step:?}");
            assert!(!step.label().is_empty());
        }
    }
}
