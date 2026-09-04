//! Pulling a pasted address down onto the board.
//!
//! A link to an MP4 is a video somebody meant to put on their board, not a
//! rectangle with a URL written on it. So a paste of one fetches the bytes and
//! becomes the card those bytes deserve, and `import.rs` does the rest — the
//! same classify, hash and measure a dropped file goes through, because a file
//! that arrived over the wire is a file.
//!
//! Everything here is bounded on purpose, because this is the one place the
//! app follows an address somebody else chose:
//!
//! 1. **Only what is worth having.** [`worth_fetching`] is a list of media
//!    families, deliberately shorter than `import::classify`'s catalogue. A
//!    link to a page stays a link; a link to a hundred-megabyte release zip
//!    stays a link. The test is "would this become a card you can *look* at".
//! 2. **Ask before downloading.** A URL whose path says nothing — every CDN
//!    and share link — gets a `HEAD` first, and the answer decides. Only then
//!    is a byte of body read. See [`asked`].
//! 3. **A ceiling, enforced while reading.** [`CEILING`] is checked against
//!    `Content-Length` *and* against the bytes as they arrive, because the
//!    header is something the other end says rather than something it owes.
//! 4. **Timeouts.** Every request runs on a background thread from
//!    `cx.background_executor()`, and a stalled connection holds one of those
//!    threads for as long as it is allowed to. Same reasoning as
//!    `update/net.rs`, which is where the `ureq`-rather-than-reqwest choice is
//!    written down.
//!
//! Failure is never louder than a link. Anything that goes wrong here — no
//! network, a 404, a type this build will not take, a file over the ceiling —
//! comes back as an error the paste turns into the link card it would have
//! made anyway. Nobody loses what they pasted.
//!
//! ## What is not here
//!
//! No page scraping. An address that answers with HTML becomes a link card and
//! that is the whole of it: no `og:image`, no title, no favicon. Those would
//! each be a second request to a place the paste never named, and a link card
//! that quietly fetched three things is not a link card anybody asked for.

// The reading and the measuring belong to the `ureq` half of this module,
// which a browser has no use for: see `asked` and `pull`, whose web arms
// answer without a network. What is left — sniffing a name and a type out of a
// URL — is the same on both.
#[cfg(not(target_family = "wasm"))]
use std::io::Read;
#[cfg(not(target_family = "wasm"))]
use std::time::Duration;

#[cfg(not(target_family = "wasm"))]
use anyhow::ensure;
use anyhow::{bail, Context as _, Result};

/// The most this will pull down for one card.
///
/// Deliberately the same number `import::WORTH_ASKING` uses, and deliberately a
/// *refusal* rather than the question a dropped file gets. A file already on
/// the disk is one somebody chose byte by byte and the app only has to ask
/// whether they meant it; a URL is a promise, and the honest thing to do with
/// a promise this large is decline it and leave the address on the board.
#[cfg(not(target_family = "wasm"))]
const CEILING: u64 = crate::import::WORTH_ASKING as u64;

/// How long to wait on the `HEAD` that decides whether to bother.
///
/// Short. Nothing has been placed on the board yet, somebody is watching the
/// status line, and the fallback — a link card — is one they can live with.
#[cfg(not(target_family = "wasm"))]
const ASK_TIMEOUT: Duration = Duration::from_secs(10);

/// How long to give the download itself.
///
/// Long enough for a video on a bad connection, short enough that a stalled
/// socket does not hold a pool thread for the rest of the session.
#[cfg(not(target_family = "wasm"))]
const FETCH_TIMEOUT: Duration = Duration::from_secs(5 * 60);

/// A file pulled off the web, ready for `import::ready`.
#[derive(Debug)]
pub struct Fetched {
    /// What to call it: the last part of the path, or the host where the path
    /// had nothing to offer. Only a hint — `import::classify` believes the
    /// bytes first, and this matters for the formats bytes cannot identify.
    pub name: String,
    pub bytes: Vec<u8>,
}

/// Whether a pasted address is worth going and looking at.
///
/// The cheap half of the decision, made with no network at all: a bare domain
/// or a page has nothing on the end of it a board can draw, and asking anyway
/// would mean every pasted link making a request that was always going to come
/// back "this is a web page".
///
/// Returning `true` is not a promise that anything will be embedded — it only
/// means the question is worth asking. [`embed`] is where the answer is.
pub fn worth_trying(url: &str) -> bool {
    let Some(path) = path_of(url) else { return false };
    match extension(path) {
        // A path that names its type settles it either way, with no request.
        Some(ext) => worth_fetching(&ext),
        // No extension at all: a CDN link, a share link, or a bare domain.
        // The first two are exactly what the `HEAD` is for; the last has an
        // empty path and is not worth a request.
        None => !path.trim_matches('/').is_empty(),
    }
}

/// Fetch what is at `url`, or say why not.
///
/// Runs on a background thread and blocks it. Every arm that returns `Err` is
/// a link card at the call site, so the wording is for a status line rather
/// than for a log.
pub fn embed(url: &str) -> Result<Fetched> {
    let path = path_of(url).context("not a web address")?;
    let named = extension(path).filter(|ext| worth_fetching(ext));

    // A path that says what it is skips the question entirely: one request for
    // `photo.jpg` rather than two.
    let ext = match named {
        Some(ext) => Some(ext),
        None => match asked(url)? {
            Says::Embed(ext) => Some(ext),
            Says::Link => bail!("that link is not something to embed"),
            // The server would not answer a `HEAD`. Rather than give up on a
            // file that is probably fine, let the `GET`'s own headers decide —
            // they arrive before the body does, so nothing is downloaded on
            // the strength of a guess.
            Says::Unsure => None,
        },
    };

    let bytes = pull(url, ext.is_none())?;
    Ok(Fetched { name: name_for(url, path, ext.as_deref()), bytes })
}

/// What a `HEAD` had to say about an address whose path said nothing.
///
/// Two of the three are unreachable in a build with no `HEAD` to send — the
/// web one, where `asked` is always `Unsure` and the `GET` decides. Kept whole
/// rather than split per platform: this is what the *protocol* can say, and it
/// does not change with who is asking.
#[cfg_attr(target_family = "wasm", allow(dead_code))]
enum Says {
    /// A media type worth having, and what to call the file.
    Embed(String),
    /// A page, or something with no type at all. Leave it as a link.
    Link,
    /// The server would not answer the question. The `GET` will have to.
    Unsure,
}

/// Ask what is at `url` without downloading it.
///
/// Every message that comes back out of here is a status line somebody reads
/// once, beside the link card the failure leaves behind — so they say what
/// happened and not which URL it happened to. The address is right there on
/// the board.
#[cfg(target_family = "wasm")]
fn asked(_url: &str) -> Result<Says> {
    // WASM EXPERIMENT: no blocking HTTP in a browser. See `net.rs`.
    Ok(Says::Unsure)
}

#[cfg(not(target_family = "wasm"))]
fn asked(url: &str) -> Result<Says> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(concat!("mbrd/", env!("CARGO_PKG_VERSION")))
        .timeout_global(Some(ASK_TIMEOUT))
        .build()
        .into();

    let response = match agent.head(url).call() {
        Ok(response) => response,
        // A refused `HEAD` is not a refused URL. Plenty of servers — and a
        // fair few CDNs in front of them — answer `GET` and nothing else, and
        // a signed CDN link commonly answers `403` to anything but the exact
        // request it was signed for. None of those is an answer about the
        // file, so the `GET` is still worth making.
        Err(ureq::Error::StatusCode(403 | 405 | 501)) => return Ok(Says::Unsure),
        // Anything else is an answer: it is gone, or it needs a login.
        Err(ureq::Error::StatusCode(code)) => bail!("that link answered {code}"),
        Err(_) => bail!("could not reach that link"),
    };

    if let Some(length) = header(&response, "content-length").and_then(|v| v.parse::<u64>().ok()) {
        ensure!(
            length <= CEILING,
            "that file is {}MB — too large to embed",
            length / (1024 * 1024)
        );
    }
    Ok(match header(&response, "content-type").and_then(|kind| from_mime(&kind)) {
        Some(ext) => Says::Embed(ext),
        None => Says::Link,
    })
}

/// Download the body, refusing to go past the ceiling.
///
/// `check` asks for the response's own `Content-Type` to be believed before
/// the body is read — the arm for a server that would not answer a `HEAD`.
#[cfg(target_family = "wasm")]
fn pull(_url: &str, _check: bool) -> Result<Vec<u8>> {
    bail!("this build cannot download that")
}

#[cfg(not(target_family = "wasm"))]
fn pull(url: &str, check: bool) -> Result<Vec<u8>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(concat!("mbrd/", env!("CARGO_PKG_VERSION")))
        .timeout_global(Some(FETCH_TIMEOUT))
        .build()
        .into();

    let mut response = match agent.get(url).call() {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(code)) => bail!("that link answered {code}"),
        Err(_) => bail!("could not reach that link"),
    };

    if check {
        let kind = header(&response, "content-type");
        ensure!(
            kind.as_deref().and_then(from_mime).is_some(),
            "that link is not something to embed"
        );
    }
    if let Some(length) = header(&response, "content-length").and_then(|v| v.parse::<u64>().ok()) {
        ensure!(
            length <= CEILING,
            "that file is {}MB — too large to embed",
            length / (1024 * 1024)
        );
    }

    // One byte past the ceiling, so that going over is *detected* rather than
    // silently truncated to exactly the ceiling — which would look like a
    // clean download of a file that is missing its end.
    let mut reader = response.body_mut().as_reader().take(CEILING + 1);
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes).context("the download stopped early")?;
    ensure!(bytes.len() as u64 <= CEILING, "that file is too large to embed");
    ensure!(!bytes.is_empty(), "that link sent nothing");
    Ok(bytes)
}

/// A header, lowercased name, as a `String`.
#[cfg(not(target_family = "wasm"))]
fn header(response: &ureq::http::Response<ureq::Body>, name: &str) -> Option<String> {
    response.headers().get(name)?.to_str().ok().map(str::to_string)
}

/// The families this build will pull down, by extension.
///
/// Shorter than `import::classify`'s list, and the difference is the point.
/// Classify's job is to never lose a file somebody already has; this one's job
/// is to decide whether to go and *get* a file, which is a question about
/// whether the result is worth looking at. So: pictures, video, sound, meshes,
/// documents and fonts — every family that becomes a card with something on
/// its face. Not archives, not executables, not source: a link to a release
/// zip is a link to a release zip, and downloading eighty megabytes of it to
/// draw a grey rectangle helps nobody.
///
/// Pages are excluded by not being in the list, which also keeps `.html` from
/// ever reaching a `HEAD`.
pub fn worth_fetching(ext: &str) -> bool {
    matches!(
        ext,
        // Pictures, including the ones this build cannot decode yet: they
        // arrive as named file cards, exactly as they do off the disk, and a
        // build that grows a decoder gets them for free.
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "ico" | "svg"
            | "avif" | "heic" | "heif" | "jxl"
            // Video.
            | "mp4" | "m4v" | "mov" | "webm" | "mkv" | "avi" | "wmv" | "flv" | "mpg" | "mpeg"
            | "ogv" | "3gp" | "mts"
            // Sound.
            | "mp3" | "wav" | "flac" | "ogg" | "oga" | "opus" | "m4a" | "aac" | "aiff" | "aif"
            | "wma" | "alac" | "ape"
            // Meshes and CAD.
            | "glb" | "gltf" | "obj" | "stl" | "fbx" | "dae" | "3mf" | "ply" | "usdz" | "blend"
            | "step" | "stp" | "iges" | "igs" | "sldprt" | "sldasm"
            // Documents, fonts, and the two text forms a note is made of.
            | "pdf" | "ttf" | "otf" | "ttc" | "woff" | "woff2" | "md" | "markdown" | "txt"
    )
}

/// A media type, as an extension, for the types worth fetching.
///
/// Narrower than [`worth_fetching`] on purpose: this is the answer to a
/// question about a URL that gave no hint at all, and `text/plain` is what half
/// the web serves an error message as. A link whose *path* says `.txt` is
/// somebody pointing at a text file; a link that merely comes back as text is
/// usually an API saying no. So text is not sniffed into — only pictures,
/// video, sound, meshes, documents and fonts, all of which mean what they say.
#[cfg(not(target_family = "wasm"))]
fn from_mime(kind: &str) -> Option<String> {
    // `image/png; charset=binary` and friends. The parameters are never the
    // answer here.
    let kind = kind.split(';').next()?.trim().to_ascii_lowercase();
    let ext = match kind.as_str() {
        "image/png" | "image/apng" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/bmp" | "image/x-ms-bmp" => "bmp",
        "image/tiff" => "tiff",
        "image/svg+xml" => "svg",
        "image/x-icon" | "image/vnd.microsoft.icon" => "ico",
        "image/avif" => "avif",
        "image/heic" | "image/heif" => "heic",
        "image/jxl" => "jxl",

        "video/mp4" => "mp4",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        "video/x-matroska" => "mkv",
        "video/mpeg" => "mpg",
        "video/ogg" => "ogv",
        "video/x-msvideo" => "avi",

        "audio/mpeg" | "audio/mp3" => "mp3",
        "audio/wav" | "audio/x-wav" | "audio/vnd.wave" => "wav",
        "audio/flac" | "audio/x-flac" => "flac",
        "audio/ogg" => "ogg",
        "audio/opus" => "opus",
        "audio/mp4" | "audio/x-m4a" => "m4a",
        "audio/aac" => "aac",

        "model/gltf-binary" => "glb",
        "model/gltf+json" => "gltf",
        "model/obj" | "text/prs.wavefront-obj" => "obj",
        "model/stl" | "application/sla" | "application/vnd.ms-pki.stl" => "stl",
        "model/3mf" => "3mf",
        "model/vnd.usdz+zip" => "usdz",
        "model/ply" => "ply",

        "application/pdf" => "pdf",
        "font/ttf" | "application/x-font-ttf" => "ttf",
        "font/otf" | "application/x-font-otf" => "otf",
        "font/woff" | "application/font-woff" => "woff",
        "font/woff2" => "woff2",

        _ => return None,
    };
    Some(ext.to_string())
}

/// The path part of an address, or nothing if it is not one.
///
/// The query and the fragment are cut off first, which is the whole reason
/// this is not `url.rsplit('.')`: `photo.png?v=2` and `clip.mp4#t=30` are the
/// ordinary spellings of both, and an extension of `png?v=2` matches nothing.
fn path_of(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("https://").or_else(|| url.strip_prefix("http://"))?;
    let rest = rest.split(['?', '#']).next().unwrap_or("");
    // Past the host. A bare domain has no slash at all, and gets an empty path
    // rather than being mistaken for one — `example.org` is not an `.org` file.
    match rest.find('/') {
        Some(at) => Some(&rest[at..]),
        None => Some(""),
    }
}

/// The extension of the last segment of a path, lowercased.
fn extension(path: &str) -> Option<String> {
    let last = path.rsplit('/').next()?;
    let (stem, ext) = last.rsplit_once('.')?;
    // A leading dot is a name, not an extension, and an extension with a
    // slash or a space in it came from something that is not a filename.
    (!stem.is_empty()
        && !ext.is_empty()
        && ext.len() <= 8
        && ext.chars().all(|c| c.is_alphanumeric()))
    .then(|| ext.to_ascii_lowercase())
}

/// What to call the file once it is on the board.
///
/// The last path segment where there is one, because that is the name whoever
/// published it chose. Where the path has nothing — a CDN link ending in an
/// opaque id, or in nothing at all — the host stands in, with the extension
/// the type sniff worked out stuck on so `import::classify` has the hint it
/// would have had from a real filename.
fn name_for(url: &str, path: &str, sniffed: Option<&str>) -> String {
    let last = path.rsplit('/').find(|part| !part.is_empty()).unwrap_or("");
    if extension(path).is_some_and(|ext| worth_fetching(&ext)) {
        return last.to_string();
    }
    let host = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .and_then(|rest| rest.split(['/', '?', '#']).next())
        .unwrap_or("pasted");
    let stem = if last.is_empty() { host } else { last };
    match sniffed {
        Some(ext) => format!("{stem}.{ext}"),
        None => stem.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // A server, so the round trip is tested rather than described
    // -----------------------------------------------------------------------
    //
    // Hand-rolled onto a loopback socket rather than mocked, because the thing
    // worth testing here is the *sequence* — a `HEAD`, what its answer decides,
    // and whether a `GET` follows — and a mock of `ureq` would only be able to
    // say that the code calls the functions it calls. It is also why these do
    // not reach the internet: a test that needs the network is a test that
    // fails on a train, and this one would fail for reasons that have nothing
    // to do with the paste it is about.

    /// Answer `replies` in order, one connection each, and say where.
    fn serving(replies: Vec<Vec<u8>>) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for reply in replies {
                let Ok((mut stream, _)) = listener.accept() else { return };
                // Read the request head and stop. Nothing here sends a body,
                // so the blank line is the end of it — and reading past it
                // would block until the client gave up.
                let mut seen = Vec::new();
                let mut byte = [0u8; 1];
                while !seen.ends_with(b"\r\n\r\n") && seen.len() < 8192 {
                    match std::io::Read::read(&mut stream, &mut byte) {
                        Ok(1) => seen.push(byte[0]),
                        _ => break,
                    }
                }
                let _ = std::io::Write::write_all(&mut stream, &reply);
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    /// One HTTP response. `Connection: close` throughout, so that each request
    /// in a test is a fresh connection the server above can count.
    fn reply(status: &str, headers: &[(&str, String)], body: &[u8]) -> Vec<u8> {
        let mut out = format!("HTTP/1.1 {status}\r\nConnection: close\r\n");
        for (name, value) in headers {
            out.push_str(&format!("{name}: {value}\r\n"));
        }
        out.push_str(&format!("Content-Length: {}\r\n\r\n", body.len()));
        let mut out = out.into_bytes();
        out.extend_from_slice(body);
        out
    }

    fn kind(mime: &str) -> Vec<(&'static str, String)> {
        vec![("Content-Type", mime.to_string())]
    }

    #[test]
    fn a_link_that_names_its_type_is_fetched_without_being_asked_about() {
        // One reply queued, so a second request would hang and fail the test —
        // which is the assertion: a path ending `.png` skips the `HEAD`.
        let at = serving(vec![reply("200 OK", &kind("image/png"), b"PNGBYTES")]);
        let got = embed(&format!("{at}/photos/sunset.png")).unwrap();
        assert_eq!(got.name, "sunset.png");
        assert_eq!(got.bytes, b"PNGBYTES");
    }

    #[test]
    fn a_cdn_link_is_asked_about_and_then_fetched() {
        let at = serving(vec![
            reply("200 OK", &kind("image/png"), b""),
            reply("200 OK", &kind("image/png"), b"PNGBYTES"),
        ]);
        let got = embed(&format!("{at}/assets/8f3a91c2")).unwrap();
        // The name the sniff worked out, so `import::classify` has the hint a
        // real filename would have given it.
        assert_eq!(got.name, "8f3a91c2.png");
        assert_eq!(got.bytes, b"PNGBYTES");
    }

    #[test]
    fn a_page_is_refused_before_a_byte_of_it_is_read() {
        // Again one reply: a second request would mean the body was fetched.
        let at = serving(vec![reply("200 OK", &kind("text/html; charset=utf-8"), b"")]);
        let why = embed(&format!("{at}/some/article")).unwrap_err().to_string();
        assert!(why.contains("not something to embed"), "{why}");
    }

    #[test]
    fn a_server_that_will_not_answer_a_head_still_gets_its_get() {
        let at = serving(vec![
            reply("405 Method Not Allowed", &[], b""),
            reply("200 OK", &kind("video/mp4"), b"MP4BYTES"),
        ]);
        let got = embed(&format!("{at}/watch/9931")).unwrap();
        assert_eq!(got.bytes, b"MP4BYTES");
    }

    #[test]
    fn a_get_that_turns_out_to_be_a_page_is_still_refused() {
        // The other half of the arm above: the `HEAD` was no help, so the
        // `GET`'s own headers have to be the check.
        let at = serving(vec![
            reply("405 Method Not Allowed", &[], b""),
            reply("200 OK", &kind("text/html"), b"<!doctype html>"),
        ]);
        let why = embed(&format!("{at}/watch/9931")).unwrap_err().to_string();
        assert!(why.contains("not something to embed"), "{why}");
    }

    #[test]
    fn a_file_over_the_ceiling_is_refused_rather_than_downloaded() {
        let at = serving(vec![reply(
            "200 OK",
            &[("Content-Type", "video/mp4".into()), ("Content-Length", (CEILING + 1).to_string())],
            b"",
        )]);
        let why = embed(&format!("{at}/huge/film.mp4")).unwrap_err().to_string();
        assert!(why.contains("too large"), "{why}");
    }

    #[test]
    fn a_dead_link_says_what_it_answered() {
        let at = serving(vec![reply("404 Not Found", &[], b"")]);
        let why = embed(&format!("{at}/gone/thing.png")).unwrap_err().to_string();
        assert!(why.contains("404"), "{why}");
    }

    #[test]
    fn a_query_string_does_not_hide_the_extension() {
        assert_eq!(
            extension(path_of("https://a.com/x/clip.mp4?token=9").unwrap()).as_deref(),
            Some("mp4")
        );
        assert_eq!(
            extension(path_of("https://a.com/x/clip.mp4#t=30").unwrap()).as_deref(),
            Some("mp4")
        );
    }

    #[test]
    fn a_bare_domain_is_not_a_file_named_after_its_suffix() {
        // The one that would otherwise turn every link to a `.org` into a
        // download of a text file.
        assert_eq!(path_of("https://rust-lang.org"), Some(""));
        assert!(!worth_trying("https://rust-lang.org"));
        assert!(!worth_trying("https://example.org/"));
    }

    #[test]
    fn a_page_is_left_as_a_link_without_a_request() {
        assert!(!worth_trying("https://example.com/index.html"));
        assert!(!worth_trying("https://example.com/thing.php"));
        // Not because `.zip` is unreadable, but because a release archive is
        // not something to pull down to draw.
        assert!(!worth_trying("https://example.com/release-v2.zip"));
    }

    #[test]
    fn media_the_board_can_draw_is_worth_trying() {
        for url in [
            "https://a.com/clip.mp4",
            "https://a.com/loop.gif",
            "https://a.com/deep/nested/part.obj",
            "https://a.com/paper.pdf",
            "https://a.com/face.woff2",
        ] {
            assert!(worth_trying(url), "{url}");
        }
    }

    #[test]
    fn a_path_with_no_extension_is_worth_asking_about() {
        // Every CDN link ever. The `HEAD` is what decides these.
        assert!(worth_trying("https://cdn.a.com/assets/8f3a91c2"));
        assert!(worth_trying("https://a.com/media/download"));
    }

    #[test]
    fn the_type_decides_when_the_path_will_not() {
        assert_eq!(from_mime("image/png").as_deref(), Some("png"));
        assert_eq!(from_mime("video/mp4; codecs=avc1").as_deref(), Some("mp4"));
        assert_eq!(from_mime("IMAGE/JPEG").as_deref(), Some("jpg"));
        assert_eq!(from_mime("text/html; charset=utf-8"), None);
        // Deliberately not sniffed into — see `from_mime`'s own note.
        assert_eq!(from_mime("text/plain"), None);
        assert_eq!(from_mime("application/octet-stream"), None);
    }

    #[test]
    fn a_fetched_file_keeps_the_name_it_was_published_under() {
        let url = "https://a.com/photos/sunset.jpg";
        assert_eq!(name_for(url, path_of(url).unwrap(), None), "sunset.jpg");
    }

    #[test]
    fn a_nameless_url_borrows_the_host_and_the_sniffed_type() {
        let url = "https://cdn.a.com/8f3a91c2";
        assert_eq!(name_for(url, path_of(url).unwrap(), Some("png")), "8f3a91c2.png");
        let bare = "https://cdn.a.com/";
        assert_eq!(name_for(bare, path_of(bare).unwrap(), Some("mp4")), "cdn.a.com.mp4");
    }

    #[test]
    fn an_extension_is_letters_and_digits_and_short() {
        // The tail of a sentence, and of a version number. Neither is a file.
        assert_eq!(extension("/a/thing.this-is-not-an-extension"), None);
        assert_eq!(extension("/a/.hidden"), None);
    }
}
