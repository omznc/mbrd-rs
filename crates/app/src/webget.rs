//! The way out of the browser: which desktop build this machine wants, and
//! where it is.
//!
//! The web build is the whole app — the same board, the same file, the same
//! keys — with three things it cannot have: a folder on your disk that boards
//! live in, a link it may follow, and an updater. So the bar carries one chip
//! that leads to the version without those, and this is what stands behind it.
//!
//! ## Guessing, and then saying what was guessed
//!
//! A browser will say what operating system it is on and will *not* say which
//! processor, which matters exactly once: a Mac. An `aarch64` build will not
//! start on an Intel Mac, so being wrong there is somebody downloading fifteen
//! megabytes of nothing.
//!
//! Two answers, in order:
//!
//! 1. **The renderer's own name**, out of WebGL. On Apple silicon every
//!    browser reports something with "Apple" in it — "Apple GPU" in Safari,
//!    "ANGLE (Apple, Apple M2, …)" in Chrome — and on an Intel Mac it names
//!    Intel or AMD instead. This is the only signal in a browser that
//!    distinguishes the two, which is why a whole GPU context is stood up for
//!    one string.
//! 2. **Apple silicon**, where that says nothing — Firefox hides it under
//!    `privacy.resistFingerprinting`. It is the newer machine, it is the one
//!    that runs both, and the tooltip names the file so a person on the other
//!    one can see that it is not theirs before they press.
//!
//! Nothing here is ever the last word: every state of the chip can reach the
//! releases page, where all six files are listed under their own names.
//!
//! ## One request, and no switch on it
//!
//! The names of release files carry their version, so the address of a build
//! cannot be worked out from here — this page is deployed on every push and
//! may be newer than the newest release. So the newest release is asked for,
//! once a session, from the API rather than from the release itself, because
//! only the API answers a browser at all: a release asset sends no
//! `Access-Control-Allow-Origin` and the fetch of one dies in the page.
//!
//! The desktop's updater is behind `Prefs::update`, and this is not, because
//! there is nothing left for that switch to protect. **The page itself is
//! served by GitHub** — every byte of the app came from `github.io` a second
//! before this runs — so asking GitHub which release is newest tells it
//! precisely nothing it was not already told by the request that loaded this
//! file. The switch exists on the desktop because there the app is on your
//! disk and the request is the only thing that leaves; here it would be a
//! control over the second of two requests to the same server.

use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;

/// The repository, for the button that says where this came from.
pub const SOURCE: &str = "https://github.com/omznc/mbrd-rs";

/// Every build there has ever been, listed under its own name. Where the chip
/// goes when there is nothing better to point at, and where its tooltip sends
/// anybody whose machine was guessed wrong.
pub const RELEASES: &str = "https://github.com/omznc/mbrd-rs/releases";

/// What the newest release is. The API rather than
/// `releases/latest/download/latest.json`, which is the file the desktop
/// updater reads: that one is a release asset, and release assets are served
/// without the header that would let a page read them.
const LATEST: &str = "https://api.github.com/repos/omznc/mbrd-rs/releases/latest";

/// A desktop this page can hand somebody a build for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Desk {
    /// `apple` is the processor: an Apple silicon build will not start on an
    /// Intel Mac. See the module note on how the two are told apart.
    Mac {
        apple: bool,
    },
    Windows,
    Linux,
}

impl Desk {
    /// What to call it in the chip.
    pub fn label(self) -> &'static str {
        match self {
            Self::Mac { .. } => "macOS",
            Self::Windows => "Windows",
            Self::Linux => "Linux",
        }
    }

    /// Whether a release file is the one this desk wants.
    ///
    /// By the tail of the name rather than by the whole of it, because the
    /// version sits in the middle of every one of them:
    ///
    /// ```text
    /// mbrd_0.5.1_aarch64.app.tar.gz     mbrd_0.5.1_x64.exe
    /// mbrd_0.5.1_x64.app.tar.gz         mbrd_0.5.1_x86_64-linux.tar.gz
    /// ```
    ///
    /// The two Linux packages — `.deb` and `.rpm` — are deliberately not
    /// matched. A browser cannot tell which of them a machine wants, and the
    /// tarball is the one answer that is right on every distribution. Both are
    /// named in the tooltip, on the page that has them.
    fn wants(self, name: &str) -> bool {
        match self {
            Self::Mac { apple: true } => name.ends_with("aarch64.app.tar.gz"),
            Self::Mac { apple: false } => name.ends_with("x64.app.tar.gz"),
            Self::Windows => name.ends_with(".exe"),
            Self::Linux => name.ends_with("x86_64-linux.tar.gz"),
        }
    }

    /// The one thing this build is not, said where somebody can act on it.
    pub fn caveat(self) -> &'static str {
        match self {
            Self::Mac { apple: true } => "Apple silicon — Intel is on the releases page",
            Self::Mac { apple: false } => "Intel — Apple silicon is on the releases page",
            Self::Windows => "portable, one file, no installer",
            Self::Linux => "a tarball — .deb and .rpm are on the releases page",
        }
    }
}

/// A build to offer, once GitHub has said which one exists.
#[derive(Debug, Clone)]
pub struct Build {
    pub desk: Desk,
    /// The file's own name, so the tooltip can say what will arrive.
    pub name: String,
    pub url: String,
    /// For the tooltip. A number worth showing: these are between six and
    /// fifteen megabytes and somebody on a phone should know before pressing.
    pub bytes: u64,
    /// The release's own version, with no `v` on the front.
    pub version: String,
}

/// What the chip in the titlebar has to say. One walk, like the update badge
/// beside it on the desktop — see `BoardView::update_badge`, which is the same
/// idea and the same corner of the same bar.
#[derive(Debug, Clone, Default)]
pub enum Getting {
    /// Not asked yet. Lasts one frame on a page that is allowed to ask, and
    /// forever on one that is not — the first render is what starts it.
    #[default]
    Cold,
    /// Asking. Drawn exactly as `Cold` is: this takes a moment on a good
    /// connection and a chip that flickered through a third state on every
    /// launch would be one nobody reads.
    Asking,
    /// There is a file for this machine and one press gets it.
    Found(Build),
    /// No file to name — an unknown desktop, a phone, a request that failed,
    /// or somebody who has turned looking off. The releases page instead,
    /// which is a worse answer and still a real one.
    Page,
}

/// What this browser is running on, or `None` where there is nothing to offer.
///
/// A phone and a tablet are `None` rather than wrong: there is no mbrd for
/// either, and a chip that offered a Mac build to somebody on Android would be
/// the one piece of chrome in the app that lies. iPadOS reports itself as a
/// Mac and is caught by the touch points, which is the standard way and still
/// a guess — a Mac with a touch screen does not exist, so it holds.
pub fn desk() -> Option<Desk> {
    let navigator = web_sys::window()?.navigator();
    let agent = navigator.user_agent().ok()?;

    if agent.contains("Android") || agent.contains("iPhone") || agent.contains("iPod") {
        return None;
    }
    if agent.contains("Windows") {
        return Some(Desk::Windows);
    }
    if agent.contains("Mac") {
        // An iPad says "Macintosh" and has a touch screen; no Mac does.
        if navigator.max_touch_points() > 1 {
            return None;
        }
        return Some(Desk::Mac { apple: apple_silicon().unwrap_or(true) });
    }
    // "X11" as well as "Linux", because that is what a BSD says, and the
    // tarball is the closest thing to an answer either has.
    if agent.contains("Linux") || agent.contains("X11") {
        return Some(Desk::Linux);
    }
    None
}

/// Whether the GPU behind this page is Apple's own.
///
/// `None` where the browser will not say, which is a real answer and not a
/// failure: Firefox hides the renderer's name by default. See the module note
/// for what is done with each.
///
/// The context is thrown away in the same breath it is made. It is a real GPU
/// context and it is stood up once per session for one string.
fn apple_silicon() -> Option<bool> {
    use web_sys::WebGlRenderingContext as Gl;

    /// `UNMASKED_RENDERER_WEBGL`, which is only a number until the extension
    /// that defines it has been asked for.
    const UNMASKED_RENDERER: u32 = 0x9246;

    let canvas: web_sys::HtmlCanvasElement =
        web_sys::window()?.document()?.create_element("canvas").ok()?.dyn_into().ok()?;
    let gl: Gl = canvas.get_context("webgl").ok()??.dyn_into().ok()?;
    gl.get_extension("WEBGL_debug_renderer_info").ok()??;
    let named = gl.get_parameter(UNMASKED_RENDERER).ok()?.as_string()?;
    Some(named.contains("Apple"))
}

/// Ask GitHub for the newest release, and pick the file this machine wants.
///
/// `None` for every way this can come to nothing — no desktop to offer, no
/// network, a rate limit, a release with no file for this machine — because
/// the caller does the same thing with all of them: point at the releases
/// page. Nothing here is worth a message, and none of it is worth a retry: a
/// press on the chip still lands somewhere useful.
pub async fn look() -> Option<Build> {
    let desk = desk()?;
    let window = web_sys::window()?;

    let response: web_sys::Response =
        JsFuture::from(window.fetch_with_str(LATEST)).await.ok()?.dyn_into().ok()?;
    if !response.ok() {
        return None;
    }
    let body = JsFuture::from(response.text().ok()?).await.ok()?.as_string()?;
    let release: serde_json::Value = serde_json::from_str(&body).ok()?;

    // `v0.5.1` in the tag, `0.5.1` everywhere a person reads it — the same
    // shape `update::version` keeps.
    let version = release.get("tag_name")?.as_str()?.trim_start_matches('v').to_string();

    let asset =
        release.get("assets")?.as_array()?.iter().find(|asset| {
            asset.get("name").and_then(|n| n.as_str()).is_some_and(|n| desk.wants(n))
        })?;

    Some(Build {
        desk,
        name: asset.get("name")?.as_str()?.to_string(),
        url: asset.get("browser_download_url")?.as_str()?.to_string(),
        bytes: asset.get("size").and_then(serde_json::Value::as_u64).unwrap_or(0),
        version,
    })
}

/// Send somebody to an address, out of the page.
///
/// A new tab first, because a release file answers with
/// `Content-Disposition: attachment` and the tab it opens closes itself the
/// moment the download starts. Where that is refused — a pop-up blocker, or a
/// press the browser did not count as one — the same window goes instead,
/// which for an attachment is a download and no navigation at all, and for the
/// releases page is a page you can come back from. The board is in the
/// browser's own database either way; see `webfs.rs`.
pub fn go(url: &str) {
    let Some(window) = web_sys::window() else { return };
    if matches!(window.open_with_url_and_target(url, "_blank"), Ok(Some(_))) {
        return;
    }
    let _ = window.location().set_href(url);
}

/// A file size in the words a download would use.
pub fn size(bytes: u64) -> String {
    match bytes {
        0 => String::new(),
        under if under < 1024 * 1024 => format!("{} KB", under.div_ceil(1024)),
        over => format!("{:.1} MB", over as f64 / (1024.0 * 1024.0)),
    }
}
