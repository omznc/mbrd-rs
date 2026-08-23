//! Two GET requests, and the limits on them.
//!
//! `ureq` rather than reqwest, and blocking rather than async. The lockfile
//! already carries a full reqwest, hyper and tokio through
//! `gpui_http_client` — but gpui's default client is `NullHttpClient`, so
//! reaching the real one means standing a tokio runtime up beside gpui's smol
//! executor. That is a lot of machinery to own for two requests that happen
//! once a day, and it is machinery that has to keep working across gpui
//! upgrades.
//!
//! Blocking is not a compromise here, it is the point: every call in this
//! module runs on `cx.background_executor()`, which is a thread pool, exactly
//! as the image decode in `board_view.rs` does. The one thing worth being
//! careful about is that a download holds one of those threads for its whole
//! life, which is why [`download`] has a timeout rather than trusting the
//! other end to hang up.

use std::io::Read;
use std::time::Duration;

use anyhow::{bail, ensure, Context as _, Result};

/// How long to wait on the manifest.
///
/// Short. Nothing is blocked on this — it happens in the background and its
/// failure mode is saying nothing — so a slow answer is worth less than a
/// thread back.
const CHECK_TIMEOUT: Duration = Duration::from_secs(15);

/// How long to give the whole download.
///
/// Generous, because it is tens of megabytes and somebody may be on a train,
/// but not unbounded: a connection that stalls forever holds a pool thread
/// forever, and the app would give no sign of it.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(15 * 60);

/// The largest manifest worth reading.
///
/// It is two kilobytes of JSON. This bound exists so that a URL that has
/// stopped being a manifest — a login page, an error page, an accidental
/// redirect to something enormous — is refused rather than read into memory.
const MANIFEST_CEILING: u64 = 64 * 1024;

/// Who we say we are. Enough for GitHub to see the traffic for what it is.
fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .user_agent(concat!("mbrd/", env!("CARGO_PKG_VERSION")))
        .timeout_global(Some(CHECK_TIMEOUT))
        .build()
        .into()
}

/// Fetch something small, as text.
///
/// Used for the manifest and its signature, both of which are tiny and both of
/// which are checked before a single byte of them is believed.
pub fn fetch_small(url: &str) -> Result<String> {
    let mut response = agent().get(url).call().with_context(|| format!("could not reach {url}"))?;

    let status = response.status();
    ensure!(status.is_success(), "{url} answered {status}");

    let mut body = String::new();
    response
        .body_mut()
        .as_reader()
        .take(MANIFEST_CEILING)
        .read_to_string(&mut body)
        .with_context(|| format!("could not read {url}"))?;

    Ok(body)
}

/// Stream a download into `sink`, refusing to exceed `expected` bytes.
///
/// The size is enforced rather than trusted in both directions. Too many bytes
/// stops mid-stream, so a manifest that lies — or a URL that has quietly
/// become something else — cannot fill a disk before anything checks a hash.
/// Too few is caught at the end, because a truncated download would otherwise
/// reach the hash check and be reported as corruption rather than as the
/// connection dropping, which sends whoever is debugging it the wrong way.
///
/// `progress` is called with the running byte count. It is how the status line
/// stays honest during the slowest thing the app ever does.
pub fn download(
    url: &str,
    expected: u64,
    sink: &mut impl std::io::Write,
    mut progress: impl FnMut(u64),
) -> Result<()> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(concat!("mbrd/", env!("CARGO_PKG_VERSION")))
        .timeout_global(Some(DOWNLOAD_TIMEOUT))
        .build()
        .into();

    let mut response = agent.get(url).call().with_context(|| format!("could not reach {url}"))?;
    let status = response.status();
    ensure!(status.is_success(), "{url} answered {status}");

    // One byte past what was promised, so that overrun is *detected* rather
    // than silently truncated to exactly the expected length — which would
    // look like a clean download of the wrong thing.
    let mut reader = response.body_mut().as_reader().take(expected + 1);
    let mut buffer = vec![0u8; 64 * 1024];
    let mut written = 0u64;

    loop {
        let read = reader.read(&mut buffer).context("the download stopped early")?;
        if read == 0 {
            break;
        }
        written += read as u64;
        if written > expected {
            bail!("{url} sent more than the {expected} bytes it promised");
        }
        sink.write_all(&buffer[..read]).context("could not write the download")?;
        progress(written);
    }

    ensure!(written == expected, "{url} sent {written} bytes of the {expected} it promised");
    Ok(())
}
