//! The three things macOS expects of an application that this app was not
//! doing.
//!
//! All of them are about the window coming *back*. A board is a document you
//! leave open for days, and on this platform leaving it open means minimising
//! it, hiding it, or closing the window and coming back to the Dock icon
//! tomorrow. Every one of those was a one-way door:
//!
//! - **Nothing answered the Dock icon.** gpui hands the reopen event to a
//!   callback the application registers, and this one registered none — so
//!   closing the window left mbrd running with no window and no way to ask for
//!   another. See [`reopen`].
//! - **Minimising was worse**, because it does not even reach that callback —
//!   miniaturised windows count as visible, so AppKit's *own* default reopen
//!   behaviour is what should have restored it, and gpui's delegate method
//!   accidentally suppresses it. See [`teach_the_dock_icon`].
//! - **And there was no menu bar at all**, so there was no Cmd Q to get out of
//!   either. See [`menus`].
//!
//! The whole file is macOS-only. Nothing here is compiled anywhere else, and
//! `main.rs` calls into it behind a `cfg`.

use gpui::{actions, App, Application, KeyBinding, Menu, MenuItem};

// The objc runtime surgery, in a file of its own.
//
// Split out because it touches no gpui type at all — it is a method being
// replaced on a class by pointer — and because that makes it the one half of
// this module that can be type-checked against a real `aarch64-apple-darwin`
// without a macOS SDK in the room. `gpui` cannot be built for that target from
// Linux; `objc2` can.
mod dock;
pub use dock::teach_the_dock_icon;

// ---------------------------------------------------------------------------
// The menu bar
// ---------------------------------------------------------------------------

actions!(
    mbrd,
    [
        /// Quit mbrd.
        Quit,
        /// Hide mbrd.
        Hide,
        /// Hide every other application.
        HideOthers,
        /// Put the window in the Dock.
        Minimise,
        /// Grow the window to fit the screen, or put it back.
        Zoom,
        /// Close the window.
        CloseWindow,
        /// Open the settings page.
        Settings,
    ]
);

/// Put a menu bar up, and bind the keys that go with it.
///
/// **A bundle with no `NSMainNibFile` and no `set_menus` has an empty menu bar**
/// — not a default one — and every one of the standard key equivalents is a
/// menu item's, not the system's. So mbrd had no Cmd Q, no Cmd W, no Cmd M and
/// no Cmd H: an app that could be minimised into the Dock and then neither
/// restored nor quit, only force-quit. That is the third of the three doors
/// this module closes, and on its own it is the difference between a bug and
/// being stuck.
///
/// ## Only the keys this app does not already use
///
/// mbrd reads its own keyboard off raw key events — see `Command::of` — and a
/// gpui key binding is dispatched *before* those, so anything bound here is
/// taken away from the board. `Cmd Q`, `Cmd H`, `Cmd M` and `Cmd W` were never
/// the board's, which is why they are safe to claim.
///
/// `Cmd ,` is the exception and it is deliberate: the board does answer it, and
/// binding it here means the settings page is opened by this action rather than
/// by `Command::Settings`. The same page opens either way, and the alternative
/// — a Settings item with no shortcut printed beside it — would say the app has
/// no keyboard shortcut for the thing it visibly does.
///
/// There is no Edit menu for the same reason inverted. Cut, copy and paste are
/// the board's own, on keys it handles itself, and claiming them here to print
/// them in a menu would be taking working keys away to advertise them.
pub fn menus(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("cmd-q", Quit, None),
        KeyBinding::new("cmd-h", Hide, None),
        KeyBinding::new("cmd-alt-h", HideOthers, None),
        KeyBinding::new("cmd-m", Minimise, None),
        KeyBinding::new("cmd-w", CloseWindow, None),
        KeyBinding::new("cmd-,", Settings, None),
    ]);

    cx.on_action(|_: &Quit, cx: &mut App| cx.quit());
    cx.on_action(|_: &Hide, cx: &mut App| cx.hide());
    cx.on_action(|_: &HideOthers, cx: &mut App| cx.hide_other_apps());
    cx.on_action(|_: &Minimise, cx: &mut App| {
        on_the_board(cx, |_view, window, _cx| window.minimize_window());
    });
    cx.on_action(|_: &Zoom, cx: &mut App| {
        on_the_board(cx, |_view, window, _cx| window.zoom_window());
    });
    // `flush` and then `remove_window`, which is exactly what the titlebar's
    // own close button does and for the reason it documents: `remove_window`
    // does not go back through the `on_window_should_close` hook `main.rs`
    // registers, so the write that hook exists for has to happen here too.
    // Without it, Cmd W would be the one way left in this app to lose work.
    cx.on_action(|_: &CloseWindow, cx: &mut App| {
        on_the_board(cx, |view, window, cx| {
            view.flush(cx);
            window.remove_window();
        });
    });
    cx.on_action(|_: &Settings, cx: &mut App| {
        on_the_board(cx, |view, window, cx| {
            crate::command::Command::Settings.run(view, window, cx);
        });
    });

    cx.set_menus(vec![
        Menu {
            name: "mbrd".into(),
            items: vec![
                MenuItem::action("Settings…", Settings),
                MenuItem::separator(),
                MenuItem::action("Hide mbrd", Hide),
                MenuItem::action("Hide Others", HideOthers),
                MenuItem::separator(),
                MenuItem::action("Quit mbrd", Quit),
            ],
        },
        Menu {
            name: "Window".into(),
            items: vec![
                MenuItem::action("Minimise", Minimise),
                // No key equivalent, and there is no standard one — Zoom has
                // never had a shortcut on this platform.
                MenuItem::action("Zoom", Zoom),
                MenuItem::separator(),
                MenuItem::action("Close Window", CloseWindow),
            ],
        },
    ]);
}

/// Do something to the one board, if there is one.
///
/// mbrd opens exactly one window — see `main.rs` — so "the first" and "the
/// only" are the same window, and a menu action that fired with none open is
/// one that does nothing rather than one that panics. The same is true of the
/// downcast: a window whose root is not a board is not a window this app made.
fn on_the_board(
    cx: &mut App,
    act: impl FnOnce(
        &mut crate::board_view::BoardView,
        &mut gpui::Window,
        &mut gpui::Context<crate::board_view::BoardView>,
    ),
) {
    let Some(window) = cx.windows().first().copied() else { return };
    let Some(board) = window.downcast::<crate::board_view::BoardView>() else { return };
    let _ = board.update(cx, |view, window, cx| act(view, window, cx));
}

// ---------------------------------------------------------------------------
// The Dock icon
// ---------------------------------------------------------------------------

/// Answer a Dock click that arrives when there is no window left.
///
/// gpui calls this only when the application has no visible windows, which on
/// this platform means the window was *closed* rather than minimised — a
/// miniaturised window still counts as visible, and that case is
/// [`teach_the_dock_icon`]'s.
///
/// Registered on the `Application` rather than inside `run`, because that is
/// where gpui takes it and because the event can arrive before the first window
/// has finished opening.
pub fn reopen(app: &Application, open: impl Fn(&mut App) + 'static) {
    app.on_reopen(move |cx| {
        // Belt and braces: if a window did survive, bringing it forward is the
        // whole of what was wanted and opening a second one would be wrong.
        // mbrd is a one-window application and `Overlay` is why — see its note.
        if let Some(window) = cx.windows().first().copied() {
            let _ = window.update(cx, |_, window, _| window.activate_window());
            cx.activate(true);
            return;
        }
        open(cx);
        cx.activate(true);
    });
}
