//! A filesystem for a build that has no filesystem.
//!
//! The web is the one platform this app runs on where `std::fs` compiles and
//! then fails at every call, and where there is no home directory for
//! `dirs.rs` to find. Both are the same problem seen from two ends: a browser
//! tab is not given a disk. This module is what stands in for one.
//!
//! ## The shape, and why it is this shape
//!
//! Two halves. In front, a map from path to bytes that lives in memory and
//! answers every read and write *synchronously*. Behind it, IndexedDB, which
//! every change is written through to and which the map is filled from once at
//! startup — see [`hydrate`], which `main.rs` awaits before it opens a window.
//!
//! The synchronous front is not a convenience, it is the requirement. Every
//! caller — `prefs.rs`, `recent.rs`, `themes.rs`, `save.rs` — reads and writes
//! inline on the thread that draws, because on the three native platforms that
//! is one syscall against a page cache. The browser offers no synchronous
//! storage worth having: `localStorage` is synchronous but holds about five
//! megabytes for the whole origin, which is one photograph; OPFS and IndexedDB
//! hold gigabytes but only asynchronously. Putting the map in front is what
//! lets this build keep real boards *and* keep every call site in the app
//! exactly as it is on a desktop.
//!
//! What that costs is a window — the width of a microtask — where a write has
//! happened in memory and not yet in the database. A tab closed inside it
//! loses that write. It is the same bargain `save.rs` already makes with the
//! autosave timer on a desktop, and it is why the write-through is queued
//! immediately rather than debounced: there is nothing to gain by waiting.
//!
//! ## Paths stay paths
//!
//! A caller still says `/home/mbrd/mbrd/board.mbrd` — what `dirs.rs` answers on
//! this platform. The path's own string is the key, a directory is a prefix,
//! and nothing above `store.rs` has to know the difference. So the switcher
//! still lists a folder, a board still has a location that can be named, and
//! the same code runs here as on a desktop.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::{Error, ErrorKind, Result};
use std::path::{Path, PathBuf};

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// The database, and the one store in it.
const DB: &str = "mbrd";
const STORE: &str = "files";

/// The version this build asks the browser for.
///
/// One, and it stays one until the *shape* of a record changes. A bump runs
/// `onupgradeneeded` against whatever is already on somebody's machine, which
/// is the only chance there is to migrate it.
const VERSION: u32 = 1;

/// What is at a path.
///
/// Directories are kept rather than implied, because `create_dir_all` followed
/// by a listing — which is what a first run does with the boards folder — has
/// to find something there.
#[derive(Clone)]
enum Node {
    File(Vec<u8>),
    Dir,
}

thread_local! {
    /// The store, in memory. One tab, one thread, one map.
    static FILES: RefCell<HashMap<PathBuf, Node>> = RefCell::new(HashMap::new());

    /// The database, once it is open. `None` until [`hydrate`] has run, and
    /// `None` forever in a tab that is not allowed to keep data — a browser in
    /// private mode, or one told to block site storage. The app still works
    /// there; it just forgets everything when the tab closes, which is the
    /// same behaviour as a read-only disk and is already handled by every
    /// caller.
    static DATABASE: RefCell<Option<web_sys::IdbDatabase>> = const { RefCell::new(None) };
}

/// Read everything the browser is keeping for this origin into memory.
///
/// Awaited by `main.rs` before the window opens, so that the first `prefs.rs`
/// read finds what the last session wrote. A failure here is not fatal and is
/// not reported to anybody: it means this tab starts empty, which is exactly
/// what a first run looks like.
pub async fn hydrate() {
    match load().await {
        Ok(()) => log::info!("store: opened"),
        Err(error) => log::warn!("store: starting empty ({error:?})"),
    }
}

/// The body of [`hydrate`], with the failures still attached.
async fn load() -> std::result::Result<(), JsValue> {
    let window = web_sys::window().ok_or_else(|| JsValue::from_str("no window"))?;
    let factory = window
        .indexed_db()?
        .ok_or_else(|| JsValue::from_str("this browser is keeping no data for this site"))?;

    let open = factory.open_with_u32(DB, VERSION)?;

    // The store is made here or never: `onupgradeneeded` is the only moment a
    // browser allows the schema to change, and it fires before the open
    // succeeds.
    let upgrade = Closure::<dyn FnMut(web_sys::Event)>::new(move |event: web_sys::Event| {
        let Some(request) = event.target().and_then(|t| t.dyn_into::<web_sys::IdbOpenDbRequest>().ok())
        else {
            return;
        };
        if let Ok(db) = request.result().and_then(|db| db.dyn_into::<web_sys::IdbDatabase>()) {
            if !db.object_store_names().contains(STORE) {
                let _ = db.create_object_store(STORE);
            }
        }
    });
    open.set_onupgradeneeded(Some(upgrade.as_ref().unchecked_ref()));

    let db: web_sys::IdbDatabase = settled(open.clone().unchecked_into()).await?.unchecked_into();
    drop(upgrade);

    // Everything at once. A board is tens of megabytes at the outside and the
    // alternative — a read per file, on demand, asynchronously — is the thing
    // this module exists to avoid: it would put an `await` inside every call
    // site in the app. See the module note.
    let transaction = db.transaction_with_str(STORE)?;
    let store = transaction.object_store(STORE)?;
    let keys: js_sys::Array = settled(store.get_all_keys()?).await?.unchecked_into();
    let values: js_sys::Array = settled(store.get_all()?).await?.unchecked_into();

    FILES.with(|files| {
        let mut files = files.borrow_mut();
        for (key, value) in keys.iter().zip(values.iter()) {
            let Some(path) = key.as_string() else { continue };
            let node = if value.is_null() || value.is_undefined() {
                Node::Dir
            } else if let Ok(bytes) = value.clone().dyn_into::<js_sys::Uint8Array>() {
                Node::File(bytes.to_vec())
            } else {
                Node::Dir
            };
            files.insert(PathBuf::from(path), node);
        }
    });

    DATABASE.with(|slot| *slot.borrow_mut() = Some(db));
    Ok(())
}

/// An IndexedDB request as a future.
///
/// IndexedDB predates promises and answers with events, so this is the adapter:
/// a promise whose two halves are hung off `onsuccess` and `onerror`, awaited
/// the ordinary way.
async fn settled(request: web_sys::IdbRequest) -> std::result::Result<JsValue, JsValue> {
    let promise = js_sys::Promise::new(&mut |resolve, reject| {
        let done = request.clone();
        let ok = Closure::once_into_js(move |_event: web_sys::Event| {
            let value = done.result().unwrap_or(JsValue::NULL);
            let _ = resolve.call1(&JsValue::NULL, &value);
        });
        let err = Closure::once_into_js(move |_event: web_sys::Event| {
            let _ = reject.call1(&JsValue::NULL, &JsValue::from_str("the browser refused"));
        });
        request.set_onsuccess(Some(ok.unchecked_ref()));
        request.set_onerror(Some(err.unchecked_ref()));
    });
    JsFuture::from(promise).await
}

/// Write one change through to the database, without waiting for it.
///
/// `None` deletes. Failures are logged and dropped: the change is already in
/// memory and the app has already been told the write succeeded, so there is
/// nothing here for a caller to do about it. The one that matters — the disk
/// being full — is reported by the browser as a rejected transaction, and it
/// is the reason this logs rather than staying silent.
fn persist(path: &Path, node: Option<&Node>) {
    let key = JsValue::from_str(&path.to_string_lossy());
    let value = match node {
        // A `Uint8Array` copy rather than a view: the view would point into
        // this module's memory, and the transaction outlives the call.
        Some(Node::File(bytes)) => js_sys::Uint8Array::from(bytes.as_slice()).into(),
        Some(Node::Dir) => JsValue::NULL,
        None => JsValue::UNDEFINED,
    };

    let queued = DATABASE.with(|slot| -> std::result::Result<(), JsValue> {
        let borrow = slot.borrow();
        let Some(db) = borrow.as_ref() else { return Ok(()) };
        let transaction = db.transaction_with_str_and_mode(STORE, web_sys::IdbTransactionMode::Readwrite)?;
        let store = transaction.object_store(STORE)?;
        if node.is_none() {
            store.delete(&key)?;
        } else {
            store.put_with_key(&value, &key)?;
        }
        Ok(())
    });
    if let Err(error) = queued {
        log::warn!("store: {} was not kept ({error:?})", path.display());
    }
}

/// Read a file whole.
pub fn read(path: &Path) -> Result<Vec<u8>> {
    FILES.with(|files| match files.borrow().get(path) {
        Some(Node::File(bytes)) => Ok(bytes.clone()),
        Some(Node::Dir) => Err(Error::new(ErrorKind::IsADirectory, "that is a folder")),
        None => Err(Error::new(ErrorKind::NotFound, "no such file")),
    })
}

/// Read a file whole, as text.
pub fn read_to_string(path: &Path) -> Result<String> {
    String::from_utf8(read(path)?)
        .map_err(|_| Error::new(ErrorKind::InvalidData, "that file is not text"))
}

/// Write a file whole, replacing whatever was there.
///
/// The parent directories come into existence with it, which `std::fs::write`
/// does not do. That is deliberate and it is not laxness: on a desktop the
/// callers that write into `dirs::config()` call `create_dir_all` first
/// because a missing directory is a real error there. Here a directory is a
/// prefix of a key, so a write that had to be preceded by one would only be a
/// way to get the same file with an error in between.
pub fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        create_dir_all(parent)?;
    }
    let node = Node::File(bytes.to_vec());
    persist(path, Some(&node));
    FILES.with(|files| files.borrow_mut().insert(path.to_path_buf(), node));
    Ok(())
}

/// Make a directory, and every directory above it.
pub fn create_dir_all(path: &Path) -> Result<()> {
    let mut here = PathBuf::new();
    for part in path.components() {
        here.push(part);
        let fresh = FILES.with(|files| {
            let mut files = files.borrow_mut();
            // Only where there is nothing already: over a file this would
            // delete it, and over a directory it is a write nobody asked for.
            if files.contains_key(&here) {
                return false;
            }
            files.insert(here.clone(), Node::Dir);
            true
        });
        if fresh {
            persist(&here, Some(&Node::Dir));
        }
    }
    Ok(())
}

/// Remove a file.
pub fn remove_file(path: &Path) -> Result<()> {
    let gone = FILES.with(|files| files.borrow_mut().remove(path));
    match gone {
        Some(Node::File(_)) => {
            persist(path, None);
            Ok(())
        }
        // Put it back: this is `remove_file`, and a directory is not a file.
        Some(node @ Node::Dir) => {
            FILES.with(|files| files.borrow_mut().insert(path.to_path_buf(), node));
            Err(Error::new(ErrorKind::IsADirectory, "that is a folder"))
        }
        None => Err(Error::new(ErrorKind::NotFound, "no such file")),
    }
}

/// Move a file, replacing anything at the destination.
pub fn rename(from: &Path, to: &Path) -> Result<()> {
    let bytes = read(from)?;
    write(to, &bytes)?;
    remove_file(from)
}

/// Whether anything is stored at a path.
pub fn exists(path: &Path) -> bool {
    FILES.with(|files| files.borrow().contains_key(path))
}

/// Whether a path is a directory — marked as one, or with anything under it.
pub fn is_dir(path: &Path) -> bool {
    FILES.with(|files| {
        let files = files.borrow();
        match files.get(path) {
            Some(Node::Dir) => true,
            Some(Node::File(_)) => false,
            None => files.keys().any(|key| key.parent() == Some(path)),
        }
    })
}

/// Whether a path is a file.
pub fn is_file(path: &Path) -> bool {
    FILES.with(|files| matches!(files.borrow().get(path), Some(Node::File(_))))
}

/// What is directly inside a directory.
///
/// Paths rather than entries, which is the shape every caller in this app
/// actually wants — see `store::read_dir_paths`, which is the same shape
/// natively so that the two platforms share one call site.
pub fn read_dir_paths(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = FILES.with(|files| {
        files
            .borrow()
            .keys()
            .filter(|path| path.parent() == Some(dir))
            .cloned()
            .collect::<Vec<PathBuf>>()
    });
    // A map hands its keys back in whatever order it likes. Every caller sorts
    // or shows these, so sort here and have one answer rather than a different
    // one per run.
    out.sort();
    Ok(out)
}
