//! mbrd, natively.
//!
//! A port of the browser moodboard at `~/dev/mbrd` onto GPUI. The board model
//! and the `.mbrd` format live in `mbrd-core`, which knows nothing about a
//! window; this crate is the part that draws.
//!
//! ```text
//! mbrd [board.mbrd]
//! ```
//!
//! Opening nothing gives you a demonstration board, which exists so that the
//! canvas has something on it before the import path does.

// On Windows a console-subsystem binary opens a console window behind the app
// every time somebody double-clicks it. Ask for the GUI subsystem in release
// builds; keep the console in debug, where the `eprintln!` below is the only
// thing that reports a board that would not open. Inert everywhere but Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod anchor;
mod board_view;
mod camera;
mod command;
mod demo;
mod editor;
mod grips;
mod images;
mod import;
mod markdown;
mod menu;
mod prefs;
mod recent;
mod save;
mod switcher;
mod theme;
mod titlebar;
mod tools;
mod wires;

use std::path::PathBuf;

use gpui::{
    px, size, App, AppContext as _, Application, Bounds, Size, TitlebarOptions, WindowBounds,
    WindowDecorations, WindowOptions,
};

use board_view::BoardView;

/// The smallest the window is allowed to get.
///
/// Not a taste: below this the window's own chrome starts colliding with
/// itself. The titlebar is 34 tall and the status line sits under it, the tool
/// strip is around 350 wide floating over the top left, and the right-click
/// menu is 216 wide and fits itself to whatever is left — see `menu.rs`, which
/// scrolls rather than spilling because it cannot leave the window. A floor
/// here is what keeps all of that a layout rather than a pile.
///
/// Handed to the compositor rather than enforced by us: on Wayland it becomes
/// `xdg_toplevel.set_min_size` and on X11 a `WM_SIZE_HINTS`, so the drag stops
/// at the edge instead of the window snapping back after it.
const MIN_SIZE: Size<gpui::Pixels> = Size { width: px(640.0), height: px(420.0) };

fn main() {
    let path = std::env::args().nth(1).map(PathBuf::from);

    // Read the file *before* the window opens. A board that cannot be opened
    // should say so on a terminal and exit, rather than flashing a window that
    // then shows an error — and at this point there is no window to show one in.
    let doc = match &path {
        Some(p) => match save::read(p) {
            Ok(doc) => doc,
            Err(err) => {
                eprintln!("mbrd: could not open {}: {err:#}", p.display());
                std::process::exit(1);
            }
        },
        None => demo::board(),
    };

    // Remembered here rather than in the switcher, so that a board opened from
    // the command line is in the list next time even if the switcher was never
    // used. That is the ordinary way somebody's first board gets into it.
    if let Some(p) = &path {
        recent::remember(p);
    }

    let title = if doc.board.title.is_empty() {
        "mbrd".to_string()
    } else {
        format!("{} — mbrd", doc.board.title)
    };

    Application::new().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.0), px(820.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(MIN_SIZE),
                // Ask for client-side decorations rather than leaving it to
                // the default, which is `Server`.
                //
                // The reason is that `Server` is not a question the compositor
                // reliably answers. gpui sets the state to `Server` optimistically
                // and only corrects it if an `xdg-decoration` Configure event
                // arrives saying otherwise — so a compositor that implements the
                // protocol and a compositor that has never heard of it are
                // *indistinguishable* from inside the app. Both report `Server`.
                //
                // GNOME's mutter is the second kind, and has been for its whole
                // life: it does not implement `xdg-decoration` at all, on the
                // position that clients should draw their own. So asking for
                // `Server` there means asking for a titlebar nobody draws, and
                // the window arrives with no way to move, resize or close it.
                //
                // Asking for `Client` is answerable: we draw the titlebar in
                // `titlebar.rs`, and a compositor that insists on decorating
                // anyway sends a Configure that flips us back to `Server`, at
                // which point that module draws nothing and defers to it.
                window_decorations: Some(WindowDecorations::Client),
                titlebar: Some(TitlebarOptions { title: Some(title.into()), ..Default::default() }),
                ..Default::default()
            },
            |_window, cx| cx.new(|cx| BoardView::new(doc, path, cx)),
        )
        .expect("could not open a window");
        cx.activate(true);
    });
}
