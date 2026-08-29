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
mod fetch;
mod fuzzy;
mod grips;
mod icons;
mod images;
mod import;
mod live;
// The three things macOS expects of an application that this app was not
// doing, all of them about the window coming back — see the module note.
// Nothing in it is compiled anywhere else.
#[cfg(target_os = "macos")]
mod mac;
mod markdown;
mod menu;
mod mesh_cache;
mod metrics;
mod opened;
mod palette;
// Sound and video. Four files behind one name, and the swap is here rather
// than as a `cfg` at every call site: GStreamer on Linux, AVFoundation on
// macOS, the Media Foundation Media Engine on Windows, and a stand-in
// elsewhere that answers "not in this build" to every question.
//
// Three backends rather than one because the one — GStreamer — is a link-time
// dependency, and satisfying it on the other two would cost Windows its
// single-file portable `.exe` and macOS a bundled framework. AVFoundation and
// Media Foundation are already in those operating systems. Each file is the
// same `Stack`, and `board_view.rs` cannot tell which it holds.
#[cfg_attr(target_os = "macos", path = "pipeline_mac.rs")]
#[cfg_attr(target_os = "windows", path = "pipeline_win.rs")]
#[cfg_attr(
    not(any(target_os = "linux", target_os = "macos", target_os = "windows")),
    path = "pipeline_off.rs"
)]
mod pipeline;
mod playback;
mod prefs;
mod recent;
mod save;
mod settings;
mod shrink;
// The half of the media stack that is the same everywhere. Not compiled where
// `pipeline_off.rs` is, which is the one platform with nothing to lay out.
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod spill;
mod stock;
mod switcher;
mod taps;
mod theme;
mod themes;
mod tip;
mod titlebar;
mod tools;
mod transport;
mod update;
mod welcome;
mod wires;

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    point, px, size, App, AppContext as _, Application, Bounds, Entity, Size, TitlebarOptions,
    Window, WindowAppearance, WindowBounds, WindowDecorations, WindowOptions,
};
use mbrd_core::Document;

use board_view::BoardView;
use themes::Appearance;

/// What a window looks like, as this app counts it.
///
/// gpui draws four: each of light and dark has a "vibrant" variant, which is
/// macOS saying the window is translucent over whatever is behind it. That is
/// a fact about the *material*, not about whether the desktop is light or
/// dark, and this app has one opaque ground either way — so the pair folds to
/// one and there are two answers rather than four.
fn appearance(window: &Window) -> Appearance {
    match window.appearance() {
        WindowAppearance::Light | WindowAppearance::VibrantLight => Appearance::Light,
        WindowAppearance::Dark | WindowAppearance::VibrantDark => Appearance::Dark,
    }
}

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
    // Before the platform is built, though it would work at any point after
    // the binary loaded: gpui registers its application delegate in a `#[ctor]`
    // and this replaces one method on it. See `mac::teach_the_dock_icon`, which
    // is the whole of why clicking a minimised mbrd in the Dock did nothing.
    #[cfg(target_os = "macos")]
    mac::teach_the_dock_icon();

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

    // A Dock click that arrives when there is no window left.
    //
    // Registered on the `Application` because that is where gpui takes it, and
    // before `run` because the event can reach an application that has not
    // finished launching. This is the *closed* case; a window that is merely
    // minimised never reaches here — see `mac::teach_the_dock_icon`, which is
    // the other half and is about a bug rather than about a missing handler.
    #[cfg(target_os = "macos")]
    mac::reopen(&app, reopen_window);

    app.run(move |cx: &mut App| {
        // The menu bar, which on this platform is also the entire keyboard: an
        // application with no menu has no Cmd Q. See `mac::menus`.
        #[cfg(target_os = "macos")]
        mac::menus(cx);

        let Some(view) = open_window(cx, doc, title) else { return };

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

        // The first run, if this is one.
        //
        // **After the board, deliberately.** A person who double-clicked a
        // `.mbrd` has said what they wanted this launch to be, and putting
        // four questions over the top of it would be answering a different
        // one — this way the board is already open and loading behind the
        // screen, so closing it lands on the thing they asked for rather than
        // on the demonstration board. It also means the welcome screen is
        // drawn over a real board on the one launch where the miniature
        // preview is being used to choose a theme.
        //
        // Here rather than in `BoardView::new` because it needs the window to
        // exist first: the screen is an `Overlay`, and an overlay opened
        // before there is anything to overlay has nowhere to be.
        view.update(cx, |view, cx| view.welcome_if_new(cx));

        // Collect anything the Finder hands over from now on. A poll, and
        // deliberately: the alternative is parking one of the background
        // executor's threads on a blocking channel read for the life of the
        // app, and that pool is what decodes images. Half a second is well
        // under the time it takes to notice a window has come forward, and a
        // tick that finds nothing does an `Option` check and *does not*
        // repaint — so the ordinary cost of this is a wakeup twice a second
        // and nothing else.
        //
        // **Weakly**, which matters more than it looks. Held strongly this
        // loop could never end: the only thing that breaks it is `update`
        // failing, `update` only fails once the entity is gone, and the loop
        // was the thing keeping it. So a closed window left this timer running
        // for the life of the process with the whole board — every photograph,
        // every clip — still in memory behind it. On this platform closing the
        // window does not end the application, which is exactly the case that
        // made that a leak rather than a technicality.
        if cfg!(target_os = "macos") {
            let watching = view.downgrade();
            cx.spawn(async move |cx| loop {
                cx.background_executor().timer(OPENED_EVERY).await;
                // Checked every tick and not only when something was dropped,
                // because a Dock icon that is never dropped on is the case
                // this timer would otherwise outlive by the life of the
                // process.
                if watching.upgrade().is_none() {
                    break;
                }
                let Some(path) = dropped.borrow_mut().take() else { continue };
                if watching.update(cx, |view, cx| view.open_board(&path, cx)).is_err() {
                    // The window has gone. Nothing left to open boards into.
                    break;
                }
            })
            .detach();
        }

        cx.activate(true);
    });
}

/// Open the window, with everything that needs one hung off it.
///
/// Split out of [`main`] because it is wanted twice: once at launch, and once
/// more when somebody clicks the Dock icon of an mbrd whose window they closed
/// — see `mac::reopen`. Before that second caller existed this was all inline,
/// which is why it reads as a sequence rather than as a constructor.
fn open_window(cx: &mut App, doc: Document, title: String) -> Option<Entity<BoardView>> {
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
            // `doc` in `main`. Whatever `argv` or the Finder actually meant is
            // opened by the caller, once there is a window and a `warn()` for
            // it to fail into.
            |_window, cx| cx.new(|cx| BoardView::new(doc, None, cx)),
        )
        .ok()?;

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
    let _ = window.update(cx, |view, window, cx| {
        // Through the `view` this closure was handed rather than through
        // `cx.entity().update(…)`: the entity is already borrowed for the
        // length of this block, and asking gpui for it a second time is a
        // panic rather than a borrow error. Before the shadowing `let`
        // below, for the same reason.
        view.desktop_appearance(appearance(window), cx);

        let view = cx.entity();
        window.on_window_should_close(cx, move |_window, cx| {
            view.update(cx, |view, cx| view.flush(cx))
        });

        // What the desktop looks like, now and whenever it changes.
        //
        // Here rather than in the view for the same reason the hook above
        // is: it needs a `Window`, and this is where there is one. The
        // seeding matters as much as the observation — on Linux the
        // appearance arrives from the XDG desktop portal over D-Bus, so a
        // window that only listened would sit on the placeholder until the
        // desktop next changed its mind, which on most desks is never.
        //
        // Whether any of this is *acted* on is `BoardView::retheme`'s
        // decision, not this one's: somebody who has pinned the app dark
        // has said the desktop does not get a vote, and this still tracks
        // it so that switching to `System` later is instant.
        let observed = cx.entity();
        window
            .observe_window_appearance(move |window, cx| {
                let looks = appearance(window);
                observed.update(cx, |view, cx| view.desktop_appearance(looks, cx));
            })
            .detach();
    });

    window.entity(cx).ok()
}

/// A window for an application that has none, on the board somebody was last
/// looking at.
///
/// What a Dock click gets after the window was closed. The most recent board
/// rather than the demonstration one, because closing a window is not the same
/// as finishing with the board that was in it — and it goes through
/// `open_board` for the reason every other route does: a board that has since
/// been moved or deleted leaves the window standing with a line saying so,
/// rather than refusing to open one.
///
/// The welcome screen is deliberately not shown here. It is a first-run
/// question, and somebody who has closed a window and clicked the icon has
/// plainly run this before.
#[cfg(target_os = "macos")]
fn reopen_window(cx: &mut App) {
    let Some(view) = open_window(cx, demo::board(), "mbrd".to_string()) else { return };
    if let Some(board) = recent::load().into_iter().next() {
        view.update(cx, |view, cx| view.open_board(&board, cx));
    }
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
