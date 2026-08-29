//! Making the Dock icon bring a minimised window back.
//!
//! One function, and what it does is replace one method on gpui's application
//! delegate. Kept apart from the rest of `mac` because it names no gpui type —
//! see the note there on why that matters for being able to check it at all.

use objc2::encode::Encode;
use objc2::runtime::{AnyClass, AnyObject, Bool, Imp, Sel};
use std::sync::atomic::{AtomicPtr, Ordering};

/// What gpui's own implementation looks like, which is the one thing about it
/// this needs to know: the same four arguments, and no return value.
type Gpui = extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, bool);

/// The implementation taken off the class, so ours can still call it.
///
/// A pointer rather than a `OnceLock<Gpui>` because it is written once, before
/// the run loop starts, and read from a method AppKit may call whenever it
/// likes.
static GPUI: AtomicPtr<()> = AtomicPtr::new(std::ptr::null_mut());

/// Make the Dock icon bring a minimised window back.
///
/// ## What is actually wrong
///
/// AppKit asks the delegate `applicationShouldHandleReopen:hasVisibleWindows:`
/// and reads a `BOOL` back. Returning `YES` — or not implementing the method at
/// all — means "do the usual thing", and the usual thing is what deminiaturises
/// a window and brings the application forward. Returning `NO` means the
/// delegate has handled it and AppKit should do nothing.
///
/// gpui implements the method, and implements it as returning **nothing**:
///
/// ```text
/// sel!(applicationShouldHandleReopen:hasVisibleWindows:),
/// should_handle_reopen as extern "C" fn(&mut Object, Sel, id, bool),
/// ```
///
/// A `void` function leaves whatever it likes in the register AppKit then reads
/// a `BOOL` out of. When that byte comes back zero, AppKit takes it as `NO`,
/// does nothing, and the window stays in the Dock — clicking the icon does
/// visibly nothing, and goes on doing nothing, because the value is settled
/// once for the life of the process. That is the "sometimes it will not come
/// back" this fixes.
///
/// It only bites on the *minimised* path, and that is worth being clear about:
/// a window that was closed rather than minimised reaches gpui's own reopen
/// callback, which is `mac::reopen`'s. A miniaturised window counts as visible,
/// so it reaches nothing at all and AppKit's default is the only thing that was
/// ever going to restore it.
///
/// ## What this does about it
///
/// Replaces the method with one of the right shape. gpui's implementation is
/// kept and called first, so the no-window path behaves exactly as it did;
/// then a proper `YES` is returned, which is the part that was missing.
///
/// Safe to call at any point after the binary has loaded — gpui builds the
/// delegate class in a `#[ctor]`, so it exists before `main` does — and safe to
/// call against a gpui that changed: a build that renamed the class or stopped
/// implementing the method leaves this a no-op rather than a crash.
pub fn teach_the_dock_icon() {
    extern "C" fn should_handle_reopen(
        this: *mut AnyObject,
        selector: Sel,
        app: *mut AnyObject,
        has_visible_windows: Bool,
    ) -> Bool {
        let previous = GPUI.load(Ordering::Relaxed);
        if !previous.is_null() {
            // SAFETY: this is the implementation `class_replaceMethod` handed
            // back for this exact selector on this exact class, so its
            // signature is the one `Gpui` names. The `BOOL` is narrowed through
            // `as_bool` deliberately: gpui's function takes a Rust `bool`, and
            // handing it a byte that is neither 0 nor 1 would be undefined
            // where passing the answer to `as_bool` cannot be.
            let previous: Gpui = unsafe { std::mem::transmute(previous) };
            previous(this, selector, app, has_visible_windows.as_bool());
        }
        // The whole point. `YES` is "do what you would have done", and what
        // AppKit would have done is bring the window back.
        Bool::YES
    }

    let Some(class) = AnyClass::get(c"GPUIApplicationDelegate") else { return };

    // Built rather than written out, because `BOOL` is a `signed char` on Intel
    // and a real `bool` on Apple silicon — so the encoding differs by
    // architecture, and `Bool`'s own is the one that is right on both.
    let types = format!("{}@:@{}\0", Bool::ENCODING, Bool::ENCODING);

    // SAFETY: `class` is a registered class, and `should_handle_reopen` has the
    // signature the encoding describes. Replacing a method on a class whose
    // instances all belong to gpui is sound because the replacement calls the
    // implementation it replaced — the only thing it changes is the return
    // value, which is the thing that was wrong.
    let previous = unsafe {
        objc2::ffi::class_replaceMethod(
            (class as *const AnyClass).cast_mut(),
            objc2::sel!(applicationShouldHandleReopen:hasVisibleWindows:),
            std::mem::transmute::<
                extern "C" fn(*mut AnyObject, Sel, *mut AnyObject, Bool) -> Bool,
                Imp,
            >(should_handle_reopen),
            types.as_ptr().cast(),
        )
    };

    // `None` means the class did not implement it, in which case AppKit's
    // default was already in force and there was nothing wrong to fix. Ours is
    // on the class either way, and calling through a null previous is what
    // `should_handle_reopen` already guards against.
    if let Some(previous) = previous {
        GPUI.store(previous as *mut (), Ordering::Relaxed);
    }
}
