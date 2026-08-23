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
//! canvas has something on it before the import path does. A board named on
//! the command line that turns out not to open — moved, corrupted, a typo —
//! gets the same demonstration board and a warning in the window, rather than
//! no window at all: see the note in [`main`] on why that failure has to end
//! up somewhere other than a terminal nobody launched from one is watching.

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
mod dirs;
mod editor;
mod fuzzy;
mod grips;
mod icons;
mod images;
mod import;
mod live;
mod markdown;
mod menu;
mod palette;
mod playback;
mod prefs;
mod recent;
mod save;
mod settings;
mod switcher;
mod taps;
mod theme;
mod tip;
mod titlebar;
mod tools;
mod transport;
mod update;
mod wires;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    point, px, size, App, AppContext as _, Application, Bounds, Size, TitlebarOptions,
    WindowBounds, WindowDecorations, WindowOptions,
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

/// How often to look for a board the Finder has handed over. See the note at
/// the call site for why this is a poll and why the interval is not tighter.
const OPENED_EVERY: std::time::Duration = std::time::Duration::from_millis(500);

fn main() {
    let path = std::env::args().nth(1).map(PathBuf::from);

    // The window always opens on the demonstration board, whatever was asked
    // for on the command line. It used to be otherwise: the file named in
    // `argv` was read *before* the window opened, and a board that would not
    // open — moved, corrupted, a typo in the name — printed to a terminal and
    // called `std::process::exit(1)` with no window ever having existed.
    //
    // That is fine from a shell. It is nothing at all from a GUI launcher: a
    // release build asks for `windows_subsystem = "windows"` at the top of
    // this file, so on Windows there is no console to print to either, and
    // double-clicking a corrupt `.mbrd` produces silence — no window, no
    // error, no sign the app even ran. The one thing this must not do is the
    // one thing it did.
    //
    // So `argv`'s path is handed to the window exactly the way a board
    // double-clicked in the Finder already is — see `dropped` below and
    // `BoardView::open_board`, which reports a failed read with `warn()` in
    // the window that is already open rather than refusing to open one. That
    // is what turns a corrupt board into an ordinary, recoverable error
    // instead of a launch that silently did nothing: the demonstration board
    // is what a bad path already falls back to when there was no path at
    // all, so a bad path behaves the same as no path, plus one line saying
    // why.
    let doc = demo::board();

    // What the last update left behind, if there was one. Before the window,
    // because it is two `stat` calls on the ordinary launch and because the
    // previous version is still on disk until somebody takes it away.
    update::sweep();

    let title = if doc.board.title.is_empty() {
        "mbrd".to_string()
    } else {
        format!("{} — mbrd", doc.board.title)
    };

    // The pictures, compiled in. Without this the default asset source
    // answers `None` to everything and every icon in the app draws as nothing
    // — silently, because a `Svg` that cannot load its file still lays out.
    let app = Application::new().with_assets(icons::Icons);

    // A board double-clicked in the Finder arrives here rather than in `argv`:
    // macOS hands the path to a running application as an Apple Event, which
    // is what `CFBundleDocumentTypes` in `packaging/macos/Info.plist` signs
    // this app up for. **The two go together** — declaring the document type
    // without listening would make the Finder offer mbrd for a board and then
    // have mbrd show the demonstration board instead, which is worse than not
    // being offered at all.
    //
    // The hand-off is a cell rather than a direct call because
    // `on_open_urls` is handed no `App`: its signature is `FnMut(Vec<String>)`
    // and there is no context to reach the window through. And the event
    // always arrives *after* the window exists — AppKit delivers
    // `application:openURLs:` once the run loop is going, which is after the
    // closure below has already opened one — so this cannot be read once at
    // startup either. It is left here and collected by the view.
    //
    // Seeded with `argv`'s path rather than `None`, which is what makes the
    // command-line case and the Finder case one path instead of two: both are
    // "a board this window did not open with turned up and wants opening",
    // and both are drained the same way, a few lines down, into the same call
    // to `BoardView::open_board`.
    let dropped: Rc<RefCell<Option<PathBuf>>> = Rc::new(RefCell::new(path));
    {
        let dropped = dropped.clone();
        app.on_open_urls(move |urls| {
            // One board per window, so the first is the one that opens.
            // Taking the last instead would be equally arbitrary and no more
            // useful.
            if let Some(path) = urls.iter().find_map(|url| board_path(url)) {
                *dropped.borrow_mut() = Some(path);
            }
        });
    }

    app.run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.0), px(820.0)), cx);
        let window = cx
            .open_window(
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
                    // which point that module keeps its bar and leaves the three
                    // window buttons to the compositor that claimed them.
                    window_decorations: Some(WindowDecorations::Client),
                    titlebar: Some(TitlebarOptions {
                        title: Some(title.into()),
                        // The same request, spelled the way macOS and Windows
                        // take it: hide the system caption, we are drawing our
                        // own. `titlebar.rs` says why this app insists on that
                        // rather than wearing three different sets of chrome on
                        // three desks.
                        appears_transparent: true,
                        // macOS hides the *bar* and keeps the traffic lights,
                        // which are real buttons it draws over ours — so they
                        // have to be told where our bar is. Centred in its
                        // height, and `titlebar::LEFT_INSET` is the matching
                        // half of this: the room left for them before anything
                        // of ours starts. Ignored everywhere else.
                        traffic_light_position: Some(point(px(12.0), px(11.0))),
                    }),
                    ..Default::default()
                },
                // Always the demo, and always with no path — see the note on
                // `doc` above. Whatever `argv` or the Finder actually meant is
                // opened a few lines down, through `dropped`, once there is a
                // window and a `warn()` for it to fail into.
                |_window, cx| cx.new(|cx| BoardView::new(doc, None, cx)),
            )
            .expect("could not open a window");

        // The board goes to disk before the window goes away.
        //
        // The autosave timer is at most a second from having done this itself
        // — see `BoardView::arm_autosave` — and this is that second. Without
        // it, closing the window straight after typing something is the one
        // way left in this app to lose work, which is exactly what removing
        // the unsaved-work indicator promised could not happen.
        //
        // Registered here rather than in the view because the hook needs a
        // `Window`, and this is where there is one. It covers the compositor's
        // own close — the system titlebar on macOS, a `Super Q`, a right-click
        // on the taskbar; the button *this app* draws calls `flush` itself,
        // because `remove_window` does not come back through here.
        //
        // `flush` answers whether the write can be trusted, and a `false` here
        // turns the close back rather than letting it through — a full disk or
        // a vanished mount is not something to lose a board over in silence.
        // The refusal only happens once: `BoardView::flush` remembers it was
        // already said, so a second attempt at closing is let through, because
        // by then the warning has been read and staying open would just be
        // trapping somebody behind a message they already have.
        let _ = window.update(cx, |_view, window, cx| {
            let view = cx.entity();
            window.on_window_should_close(cx, move |_window, cx| {
                view.update(cx, |view, cx| view.flush(cx))
            });
        });

        let Ok(view) = window.entity(cx) else { return };

        // Whatever `dropped` already holds — `argv`'s path, or a Finder open
        // that AppKit delivered before this closure ran. `open_board` is the
        // one function every way of getting a different board into this
        // window goes through, on the command line exactly as on a drop or a
        // Finder double-click: it reads off the background executor and, if
        // the file will not open, leaves this window exactly as it is and
        // reports the failure with `warn()` instead of taking the window down
        // with it. That is the whole fix for a corrupt board named on the
        // command line — see the note on `doc` above.
        if let Some(path) = dropped.borrow_mut().take() {
            view.update(cx, |view, cx| view.open_board(&path, cx));
        }

        // Collect anything the Finder hands over from now on. A poll, and
        // deliberately: the alternative is parking one of the background
        // executor's threads on a blocking channel read for the life of the
        // app, and that pool is what decodes images. Half a second is well
        // under the time it takes to notice a window has come forward, and a
        // tick that finds nothing does an `Option` check and *does not*
        // repaint — so the ordinary cost of this is a wakeup twice a second
        // and nothing else.
        if cfg!(target_os = "macos") {
            cx.spawn(async move |cx| loop {
                cx.background_executor().timer(OPENED_EVERY).await;
                let Some(path) = dropped.borrow_mut().take() else { continue };
                if view.update(cx, |view, cx| view.open_board(&path, cx)).is_err() {
                    // The window has gone. Nothing left to open boards into.
                    break;
                }
            })
            .detach();
        }

        cx.activate(true);
    });
}

/// The path inside a `file://` URL, if that is what it is.
///
/// `on_open_urls` is given URLs because that is the shape of the Apple Event,
/// and every one this app will ever see is a local file. Anything else — an
/// `http://`, a custom scheme nobody registered — is not a board and is
/// dropped rather than guessed at.
fn board_path(url: &str) -> Option<PathBuf> {
    let rest = url.strip_prefix("file://")?;
    // The host part of a `file://` URL is empty for a local path, leaving the
    // absolute path immediately after it.
    let path = rest.strip_prefix("localhost").unwrap_or(rest);
    if !path.starts_with('/') {
        return None;
    }
    Some(PathBuf::from(percent_decoded(path)))
}

/// Undo the escaping a URL puts on a path.
///
/// A board called `holiday photos.mbrd` arrives as `holiday%20photos.mbrd`,
/// and opening the literal name would fail on exactly the files most likely to
/// be double-clicked. Hand-rolled because this is the only URL this app will
/// ever parse and a percent-decoder is fifteen lines.
fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        // A `%` not followed by two hex digits is a literal `%`, which is a
        // legal character in a file name and would otherwise eat the next two.
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok();
            if let Some(byte) = hex.and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_url_becomes_the_path_inside_it() {
        assert_eq!(board_path("file:///home/a/b.mbrd"), Some(PathBuf::from("/home/a/b.mbrd")));
        assert_eq!(
            board_path("file://localhost/home/a/b.mbrd"),
            Some(PathBuf::from("/home/a/b.mbrd"))
        );
    }

    #[test]
    fn an_escaped_name_is_unescaped() {
        // The names most likely to be double-clicked are the ones with spaces
        // in them, so this is the common case rather than the exotic one.
        assert_eq!(
            board_path("file:///home/a/holiday%20photos.mbrd"),
            Some(PathBuf::from("/home/a/holiday photos.mbrd"))
        );
        assert_eq!(
            board_path("file:///home/a/100%25%20done.mbrd"),
            Some(PathBuf::from("/home/a/100% done.mbrd"))
        );
    }

    #[test]
    fn a_stray_percent_is_left_alone_rather_than_eating_the_next_two() {
        // `%` is a legal character in a file name, and a decoder that assumed
        // otherwise would mangle exactly the names it was meant to rescue.
        assert_eq!(percent_decoded("/a/50%.mbrd"), "/a/50%.mbrd");
        assert_eq!(percent_decoded("/a/%zz.mbrd"), "/a/%zz.mbrd");
        assert_eq!(percent_decoded("/a/%2"), "/a/%2");
    }

    #[test]
    fn anything_that_is_not_a_local_file_is_not_a_board() {
        for url in ["https://example.com/b.mbrd", "mbrd://open", "file://relative", "", "file://"] {
            assert_eq!(board_path(url), None, "{url} should not have been a board");
        }
    }
}
