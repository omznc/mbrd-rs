//! The three ways a file gets into this app, on the one platform where a file
//! is not a path.
//!
//! Dropping, pasting and picking are all the same shape everywhere else: the
//! platform hands over a path and [`crate::board_view::BoardView::take_files`]
//! reads it. A browser hands over a `File` — bytes and a name, and never a
//! location — so there is nothing for that function to read.
//!
//! What this module does is close that gap in the one place it exists rather
//! than in every place it shows: the bytes are written into the store under a
//! path of our own (see `webfs.rs`, where a path is a key and a file is real
//! enough to be read back), and *the path* is what the app is handed. Above
//! this line the web build takes files exactly the way a desktop does —
//! `import::classify` sniffs the same bytes, the same card lands on the board,
//! and `board_view.rs` has no idea which platform it is on.
//!
//! ## Why the arrivals are a queue
//!
//! A browser only gives up a file's bytes asynchronously, and the listener that
//! receives the drop is not inside gpui's world — it is a DOM callback with no
//! `App` to reach the board through. So a drop lands here, is read, is written,
//! and is left in a queue; `main.rs` drains it on a timer and calls the same
//! method the native drop calls. That is the shape `main.rs` already uses for a
//! board handed over by the Finder, and for the same reason.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use futures::channel::oneshot;
use gpui::{AsyncApp, WeakEntity};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// Where a dropped, pasted or picked file is put.
///
/// Under the home this platform invents rather than beside the boards: these
/// are the *originals* somebody dropped, they are already copied into the
/// `.mbrd` by the time a card exists, and the boards folder is a place a person
/// looks at. A directory of raw drops in it would be litter in the one folder
/// this build asks anybody to care about.
const INBOX: &str = "/home/mbrd/.cache/mbrd/files";

thread_local! {
    /// Files that have arrived and not yet been given to the board.
    static ARRIVED: RefCell<Vec<PathBuf>> = const { RefCell::new(Vec::new()) };

    /// The board, and a way to reach it from outside gpui's own loop.
    ///
    /// Set once the window exists. Weak, so that a page whose board has gone
    /// does not keep it alive; and paired with an `AsyncApp` because a DOM
    /// callback is handed no context at all.
    static BOARD: RefCell<Option<(WeakEntity<crate::board_view::BoardView>, AsyncApp)>> =
        const { RefCell::new(None) };

    /// The listeners, kept alive for the life of the page. A `Closure` that is
    /// dropped is torn down, and a DOM event that reached one afterwards would
    /// throw rather than do nothing.
    static LISTENERS: RefCell<Vec<Closure<dyn FnMut(web_sys::Event)>>> =
        const { RefCell::new(Vec::new()) };
}

/// Start listening for files.
///
/// Called once, from `main.rs`, before the window exists — a drop that arrives
/// during startup is then queued rather than lost.
pub fn install() {
    let Some(window) = web_sys::window() else { return };

    // Both halves are needed and only one of them does anything visible: a
    // `dragover` that is not cancelled means the drop never fires at all, and
    // a `drop` that is not cancelled means the browser navigates away from the
    // app to show the file. gpui's own web backend already cancels both for
    // the canvas; this is the same for the window, so a drop onto any part of
    // the page — including the margins — is caught rather than opening the
    // file in a fresh tab.
    let over = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        event.prevent_default();
    });
    let _ = window.add_event_listener_with_callback("dragover", over.as_ref().unchecked_ref());

    let dropped = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        event.prevent_default();
        let Ok(event) = event.dyn_into::<web_sys::DragEvent>() else { return };
        let Some(transfer) = event.data_transfer() else { return };

        // **Entries, not files, and taken now rather than later.** A folder
        // dropped on a page is not in `DataTransfer::files` at all — that list
        // is files only, so a dropped folder arrives as nothing. The entries
        // are where a folder can be seen and walked, and they can only be
        // asked for while this handler is running: the item list is emptied
        // the moment it returns, so what is collected here is collected
        // synchronously and read afterwards.
        let items = transfer.items();
        let mut entries = Vec::new();
        let mut loose = Vec::new();
        for index in 0..items.length() {
            let Some(item) = items.get(index) else { continue };
            match item.webkit_get_as_entry() {
                Ok(Some(entry)) => entries.push(entry),
                // A browser without the entry API, or an item that is not a
                // file at all. The file itself is still there to be had.
                _ => {
                    if let Ok(Some(file)) = item.get_as_file() {
                        loose.push(file);
                    }
                }
            }
        }

        // Nothing recognisable in the items — some browsers only fill the file
        // list. Falling through to it costs nothing and is the older path.
        if entries.is_empty() && loose.is_empty() {
            accept(files_of(transfer.files()));
            return;
        }

        wasm_bindgen_futures::spawn_local(async move {
            let mut files = loose;
            for entry in entries {
                files.extend(walk(&entry).await);
            }
            land(files).await;
        });
    });
    let _ = window.add_event_listener_with_callback("drop", dropped.as_ref().unchecked_ref());

    // A screenshot pasted straight in. The clipboard hands these over as files
    // with names of its own — `image.png`, usually — which is exactly what the
    // import path wants, so there is nothing special about this arm beyond
    // where the files came from.
    let pasted = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        let Ok(event) = event.dyn_into::<web_sys::ClipboardEvent>() else { return };
        let Some(transfer) = event.clipboard_data() else { return };
        let files = transfer.files();
        // Only when there *are* files. Pasting text is the editor's business
        // and cancelling the event here would take it away from them.
        if files.as_ref().is_some_and(|files| files.length() > 0) {
            event.prevent_default();
            accept(files_of(files));
        }
    });
    let _ = window.add_event_listener_with_callback("paste", pasted.as_ref().unchecked_ref());

    LISTENERS.with(|kept| kept.borrow_mut().extend([over, dropped, pasted]));
}

/// A browser's file list as something that can be held onto.
fn files_of(files: Option<web_sys::FileList>) -> Vec<web_sys::File> {
    let Some(files) = files else { return Vec::new() };
    (0..files.length()).filter_map(|index| files.get(index)).collect()
}

/// Read a set of files into the store and hand them over together.
fn accept(files: Vec<web_sys::File>) {
    if files.is_empty() {
        return;
    }
    wasm_bindgen_futures::spawn_local(land(files));
}

/// Read files into the store, then give the board **all of them at once**.
///
/// The togetherness is the point and it is not tidiness. `take_files` lays a
/// drop out — a row of cards, side by side, in the space it is given — and it
/// can only do that for the files it is handed in one call. Delivered one at a
/// time, every file is its own drop of one, every one is laid out alone, and
/// they land on top of each other in the middle of the view. Reading them all
/// and delivering once is what makes a folder of twenty pictures a row of
/// twenty cards rather than a stack.
async fn land(files: Vec<web_sys::File>) {
    let mut paths = Vec::new();
    for file in files {
        match keep(&file).await {
            Ok(path) => paths.push(path),
            Err(error) => log::warn!("files: {} did not arrive ({error:?})", file.name()),
        }
    }
    if paths.is_empty() {
        return;
    }
    // Sorted, exactly as `import::walk` sorts what it finds in a folder: two
    // drops of the same folder should lay out the same way round.
    paths.sort();
    ARRIVED.with(|arrived| arrived.borrow_mut().extend(paths));
    deliver();
}

/// The files in one dropped entry — the file itself, or what is in the folder.
///
/// One level deep, which is what `import::walk` does with a folder on every
/// other platform: a drop of a folder is a drop of what is *in* it, and a walk
/// of somebody's whole tree is not something a gesture should start.
async fn walk(entry: &web_sys::FileSystemEntry) -> Vec<web_sys::File> {
    if entry.is_file() {
        let Ok(file) = entry.clone().dyn_into::<web_sys::FileSystemFileEntry>() else {
            return Vec::new();
        };
        return one(&file).await.into_iter().collect();
    }

    let Ok(directory) = entry.clone().dyn_into::<web_sys::FileSystemDirectoryEntry>() else {
        return Vec::new();
    };
    let reader = directory.create_reader();
    let mut files = Vec::new();
    // A reader hands back a hundred entries at a time and answers with an
    // empty list when there are no more, so this is the documented way to see
    // all of a folder rather than the first screenful of one.
    loop {
        let batch = batch(&reader).await;
        if batch.is_empty() {
            break;
        }
        for entry in batch {
            // Files only, and no deeper. See the note above.
            if !entry.is_file() {
                continue;
            }
            if let Ok(file) = entry.dyn_into::<web_sys::FileSystemFileEntry>() {
                if let Some(file) = one(&file).await {
                    files.push(file);
                }
            }
        }
    }
    if files.is_empty() {
        log::warn!("files: {} held nothing this build could take", directory.name());
    }
    files
}

/// One entry's `File`, which a browser also only gives up asynchronously.
async fn one(entry: &web_sys::FileSystemFileEntry) -> Option<web_sys::File> {
    let entry = entry.clone();
    let promise = js_sys::Promise::new(&mut |resolve, reject| {
        let ok = Closure::once_into_js(move |file: JsValue| {
            let _ = resolve.call1(&JsValue::NULL, &file);
        });
        let failed = Closure::once_into_js(move |error: JsValue| {
            let _ = reject.call1(&JsValue::NULL, &error);
        });
        entry.file_with_callback_and_error_callback(ok.unchecked_ref(), failed.unchecked_ref());
    });
    JsFuture::from(promise).await.ok().and_then(|file| file.dyn_into().ok())
}

/// One reader's worth of entries.
async fn batch(reader: &web_sys::FileSystemDirectoryReader) -> Vec<web_sys::FileSystemEntry> {
    let reader = reader.clone();
    let promise = js_sys::Promise::new(&mut |resolve, reject| {
        let ok = Closure::once_into_js(move |entries: JsValue| {
            let _ = resolve.call1(&JsValue::NULL, &entries);
        });
        let failed = Closure::once_into_js(move |error: JsValue| {
            let _ = reject.call1(&JsValue::NULL, &error);
        });
        let _ = reader
            .read_entries_with_callback_and_error_callback(ok.unchecked_ref(), failed.unchecked_ref());
    });
    let Ok(entries) = JsFuture::from(promise).await else { return Vec::new() };
    let entries: js_sys::Array = entries.unchecked_into();
    entries.iter().filter_map(|entry| entry.dyn_into().ok()).collect()
}

/// Write one `File` into the store and answer where it went.
async fn keep(file: &web_sys::File) -> Result<PathBuf, JsValue> {
    let buffer = JsFuture::from(file.array_buffer()).await?;
    let bytes = js_sys::Uint8Array::new(&buffer).to_vec();
    let path = free(&file.name());
    crate::store::write(&path, &bytes)
        .map_err(|error| JsValue::from_str(&format!("could not be kept: {error}")))?;
    Ok(path)
}

/// A path in the inbox that nothing is using.
///
/// Two screenshots pasted in a row are both called `image.png`, and the second
/// must not be the first. The same shape as `board_view`'s `unused_in`, and
/// bounded for the same reason.
fn free(name: &str) -> PathBuf {
    let dir = Path::new(INBOX);
    let name = if name.is_empty() { "file" } else { name };
    let taken = dir.join(name);
    if !crate::store::exists(&taken) {
        return taken;
    }
    let path = Path::new(name);
    let stem = path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..1000)
        .map(|n| dir.join(format!("{stem}-{n}{ext}")))
        .find(|candidate| !crate::store::exists(candidate))
        .unwrap_or(taken)
}

/// Everything that has arrived since the last time this was asked.
pub fn take() -> Vec<PathBuf> {
    ARRIVED.with(|arrived| std::mem::take(&mut *arrived.borrow_mut()))
}

/// Tell this module where the board is, once there is one.
pub fn attach(view: WeakEntity<crate::board_view::BoardView>, cx: AsyncApp) {
    BOARD.with(|board| *board.borrow_mut() = Some((view, cx)));
    // Anything dropped on the page while it was still loading. Not inline:
    // this is called from inside gpui's launch closure, where the application
    // is borrowed and [`deliver`]'s spawn would be the one thing it must not
    // do. The first frame takes the queue instead — see `BoardView::render`.
}

/// Give the board whatever has arrived.
///
/// **Through gpui's own executor rather than straight into the board**, and
/// that is the whole substance of this function. A file arrives in a DOM
/// callback, which can land at any moment — including inside a frame, when
/// gpui has the application borrowed and updating a view from underneath it
/// panics. A task spawned here runs when the application is free, which is the
/// same guarantee every other deferred piece of work in this app has.
///
/// The queue is what covers the window before there is a board to spawn
/// against: a file dropped on the page while it is still loading waits there,
/// and `BoardView::render` takes it at the first frame.
pub fn deliver() {
    let waiting = take();
    if waiting.is_empty() {
        return;
    }
    // Cloned out rather than borrowed across the spawn: the task outlives this
    // call and the cell must not still be held when it runs.
    let board = BOARD.with(|board| board.borrow().clone());
    let Some((view, cx)) = board else {
        ARRIVED.with(|arrived| arrived.borrow_mut().extend(waiting));
        return;
    };
    cx.spawn(async move |cx| {
        if view.update(cx, |view, cx| view.take_arrivals(&waiting, cx)).is_err() {
            // The board has gone — a window that closed between the drop and
            // this task. Nothing to do and nobody to tell.
            log::warn!("files: {} arrived with no board to put them on", waiting.len());
        }
    })
    .detach();
}

/// Ask somebody for files, the way the platform does everywhere else.
///
/// The same answer shape as `gpui`'s `prompt_for_paths` — a channel carrying
/// "the picker failed", "cancelled", or the paths — so that the call site in
/// `board_view.rs` is one call site rather than two.
///
/// A file input rather than `showOpenFilePicker`: the modern API is Chromium
/// only, and this one is in every browser and needs no permission prompt. What
/// it costs is the handle — there is no way back to the file somebody chose,
/// so this is an import and never a link. On this platform that is true of
/// every file anyway; see the module note.
pub fn pick_files(multiple: bool) -> oneshot::Receiver<anyhow::Result<Option<Vec<PathBuf>>>> {
    pick(multiple, false)
}

/// Ask somebody for a folder, and take what is directly in it.
///
/// A separate door because a browser will not open one dialog that takes
/// either: an input is `webkitdirectory` or it is not, and the two pick
/// different things. On every other platform this is the same dialog as
/// [`pick_files`] with a folder chosen in it, which is why the command that
/// reaches this one is offered on the web and nowhere else — see
/// `Command::available`.
pub fn pick_folder() -> oneshot::Receiver<anyhow::Result<Option<Vec<PathBuf>>>> {
    pick(true, true)
}

/// The picker both of the above are.
fn pick(multiple: bool, directory: bool) -> oneshot::Receiver<anyhow::Result<Option<Vec<PathBuf>>>> {
    let (send, receive) = oneshot::channel();

    let Some(document) = web_sys::window().and_then(|window| window.document()) else {
        let _ = send.send(Err(anyhow::anyhow!("no page to open a picker in")));
        return receive;
    };

    let input: web_sys::HtmlInputElement = match document
        .create_element("input")
        .ok()
        .and_then(|element| element.dyn_into::<web_sys::HtmlInputElement>().ok())
    {
        Some(input) => input,
        None => {
            let _ = send.send(Err(anyhow::anyhow!("no file picker in this browser")));
            return receive;
        }
    };
    input.set_type("file");
    input.set_multiple(multiple);
    input.set_webkitdirectory(directory);
    // Off-screen rather than `display: none`: a hidden input is not clickable
    // in every browser, and this one is about to be clicked from script.
    let _ = input.style().set_property("position", "fixed");
    let _ = input.style().set_property("left", "-1000px");
    let _ = document.body().map(|body| body.append_child(&input));

    let send = RefCell::new(Some(send));
    let taken = input.clone();
    let holder = document.clone();
    let changed = Closure::<dyn FnMut(web_sys::Event)>::new(move |_event: web_sys::Event| {
        let Some(send) = send.borrow_mut().take() else { return };
        let files = taken.files();
        let count = files.as_ref().map_or(0, |files| files.length());
        let input = taken.clone();
        let holder = holder.clone();
        wasm_bindgen_futures::spawn_local(async move {
            let mut paths = Vec::new();
            if let Some(files) = files {
                for index in 0..count {
                    let Some(file) = files.get(index) else { continue };
                    // A folder input hands over *everything* under what was
                    // chosen, however deep. One level is what `import::walk`
                    // takes from a folder on every other platform, and a
                    // browser says how deep a file was by how many parts its
                    // relative path has.
                    if directory && depth(&file) > 1 {
                        continue;
                    }
                    match keep(&file).await {
                        Ok(path) => paths.push(path),
                        Err(error) => log::warn!("files: {} was not read ({error:?})", file.name()),
                    }
                }
            }
            // The same order a folder is walked in everywhere else.
            paths.sort();
            // The input has done its one job. Left in the page it would collect
            // one of these per picker opened, each holding a closure.
            if let Some(body) = holder.body() {
                let _ = body.remove_child(&input);
            }
            let _ = send.send(Ok((!paths.is_empty()).then_some(paths)));
        });
    });
    let _ = input.add_event_listener_with_callback("change", changed.as_ref().unchecked_ref());

    // A cancelled picker fires `cancel` in current browsers and nothing at all
    // in older ones. Nothing at all is survivable — the channel is dropped with
    // the input and the call site treats that as a cancel — so this is the
    // tidy-up rather than the mechanism.
    LISTENERS.with(|kept| kept.borrow_mut().push(changed));
    input.click();
    receive
}

/// How far under the chosen folder a file was found.
///
/// `webkitRelativePath` is `folder/file.png` for something directly inside and
/// grows a part per level below that. It is not in `web-sys`, so it is read off
/// the object the way JavaScript would.
fn depth(file: &web_sys::File) -> usize {
    js_sys::Reflect::get(file, &JsValue::from_str("webkitRelativePath"))
        .ok()
        .and_then(|value| value.as_string())
        .map(|path| path.matches('/').count())
        .unwrap_or(1)
}

/// Hand a file to the person using the browser.
///
/// The counterpart to [`pick_files`], and the only way a board leaves this
/// build: a page cannot write to somebody's disk, but it can offer them a file,
/// which is what every download is. The board still exists in the store
/// afterwards — this is a copy going out, not a move.
pub fn download(name: &str, bytes: &[u8]) -> anyhow::Result<()> {
    let window = web_sys::window().ok_or_else(|| anyhow::anyhow!("no page"))?;
    let document = window.document().ok_or_else(|| anyhow::anyhow!("no page"))?;

    // Through a `Uint8Array` copy rather than a view into wasm memory: the blob
    // outlives this call, and a view would go stale the moment the heap grew.
    let array = js_sys::Uint8Array::from(bytes);
    let parts = js_sys::Array::of1(&array.buffer());
    let blob = web_sys::Blob::new_with_u8_array_sequence(&parts)
        .map_err(|_| anyhow::anyhow!("could not make a file of that"))?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)
        .map_err(|_| anyhow::anyhow!("could not make a file of that"))?;

    let anchor: web_sys::HtmlAnchorElement = document
        .create_element("a")
        .ok()
        .and_then(|element| element.dyn_into().ok())
        .ok_or_else(|| anyhow::anyhow!("no page"))?;
    anchor.set_href(&url);
    anchor.set_download(name);
    anchor.click();

    // The URL holds the blob alive until it is revoked, and a board is
    // megabytes. The click has already started the download by the time this
    // runs, and the browser keeps its own reference to what it is saving.
    web_sys::Url::revoke_object_url(&url).ok();
    Ok(())
}
