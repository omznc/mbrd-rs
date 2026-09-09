//! Themes from open-vsx.org: search, download, and put in the themes folder.
//!
//! The point of `vscode.rs` was that this app reads somebody else's format.
//! This module is the other half of that bargain: if the format is theirs,
//! then the tens of thousands of themes already written in it should be one
//! search field away, rather than something a person fetches in a browser,
//! unzips by hand and copies into a directory they have to be told about.
//!
//! ## Why open-vsx and not the Visual Studio Marketplace
//!
//! The marketplace has more themes and a licence that says only Microsoft's
//! own products may use its gallery. open-vsx.org is the Eclipse Foundation's
//! open registry, it exists so that editors which are not VS Code have
//! somewhere to look, and its API is a documented REST one with no key. There
//! is no version of this feature that points at the other one.
//!
//! ## What is downloaded, and what is kept
//!
//! A `.vsix` is a zip. Inside it, `extension/package.json` lists what the
//! extension contributes, and `contributes.themes[]` is the part this cares
//! about: a label, a `uiTheme`, and a path to a colour theme file. Every one
//! of those becomes one file in `dirs::themes()`, and the rest of the
//! archive — the icons, the screenshots, the changelog, the code — is read
//! past and dropped.
//!
//! What lands on disk is **not** a copy of the file that was in the archive.
//! It is the file with its `include` chain laid flat, its `tokenColors`
//! dropped, and two keys added: `author` for the publisher and `family` for
//! the extension it came from, which is how the settings page can say where a
//! theme came from later. It is still a colour theme any other reader of the
//! format accepts. The reason for rewriting rather than copying is that a
//! theme in the folder has to stand alone: the file it included is not going
//! to be there, and a half-resolved theme would load as a handful of colours
//! over the built-in palette without saying so.
//!
//! Nothing from the archive is written under a path the archive chose. The
//! file name is built here, out of the namespace, the extension name and the
//! theme's label — so a `.vsix` with `../../.bashrc` in it writes a file
//! called `bashrc.json` in the themes folder, and nothing else.
//!
//! ## Why blocking
//!
//! The same answer `update/net.rs` gives at length, and for the same reason:
//! `ureq` on `cx.background_executor()`. Both calls here are bounded and both
//! have a timeout, because each one holds a thread from that pool for its
//! whole life.

use anyhow::{bail, ensure, Context as _, Result};
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::HashMap;

#[cfg(not(target_family = "wasm"))]
use std::io::Read as _;
#[cfg(not(target_family = "wasm"))]
use std::time::Duration;

/// How long to wait on a search.
///
/// Short, because somebody is looking at the field while it runs. A search
/// that has not answered in this long has failed as far as they are
/// concerned, whatever the other end thinks.
#[cfg(not(target_family = "wasm"))]
const SEARCH_TIMEOUT: Duration = Duration::from_secs(15);

/// How long to give a whole install.
///
/// Longer, because it is a download, and shorter than the app updater's
/// fifteen minutes, because a theme extension is measured in kilobytes and
/// anything taking minutes has gone wrong rather than slow.
#[cfg(not(target_family = "wasm"))]
const INSTALL_TIMEOUT: Duration = Duration::from_secs(120);

/// The largest search answer worth reading.
#[cfg(not(target_family = "wasm"))]
const SEARCH_CEILING: u64 = 2 * 1024 * 1024;

/// The largest `.vsix` worth reading.
///
/// A colour theme is a few kilobytes of JSON. Extensions are this big because
/// of screenshots and animated demonstrations, none of which is read. The
/// bound exists so that an extension carrying a hundred megabytes of video is
/// refused before it is in memory, rather than after.
#[cfg(not(target_family = "wasm"))]
const VSIX_CEILING: u64 = 24 * 1024 * 1024;

/// The largest file inside the archive worth decompressing.
///
/// Checked against the size the archive *claims* before anything is read, and
/// again while reading, because a zip header is written by whoever made the
/// zip. Without it a small `.vsix` can decompress to an arbitrarily large
/// amount of memory.
#[cfg(not(target_family = "wasm"))]
const ENTRY_CEILING: u64 = 4 * 1024 * 1024;

/// How many results to ask for.
///
/// A list somebody scans, not a catalogue somebody pages through. Twenty-five
/// is about two screens, and the order below puts the answer near the top —
/// so a query whose answer is not in the first twenty-five is a query worth
/// retyping rather than scrolling.
#[cfg(not(target_family = "wasm"))]
const RESULTS: usize = 25;

/// One extension, as a search result row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The publisher, in the registry's own words. Half of the identity.
    pub namespace: String,
    /// The extension, in the registry's own words. The other half.
    pub name: String,
    /// What to show. The extension's `displayName`, or its `name` when it has
    /// none — which is common enough to be worth the fallback.
    pub display: String,
    /// One line about it, already trimmed to something a row can hold.
    pub description: String,
    /// How many people have downloaded it. Shown because it is the only
    /// signal in the answer that separates a theme thousands of people use
    /// from one that was published yesterday.
    pub downloads: u64,
    /// Where the `.vsix` is, when the search said. Empty means ask again by
    /// name at install time; see [`install`].
    pub vsix: String,
}

/// What open-vsx answers a search with.
#[derive(Debug, Deserialize)]
struct Answer {
    #[serde(default)]
    extensions: Vec<Listing>,
}

/// One extension in that answer, and also the whole answer to a lookup by
/// name — the two endpoints return the same object.
#[derive(Debug, Deserialize)]
struct Listing {
    namespace: String,
    name: String,
    #[serde(rename = "displayName", default)]
    display: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(rename = "downloadCount", default)]
    downloads: u64,
    #[serde(default)]
    files: Downloads,
}

#[derive(Debug, Default, Deserialize)]
struct Downloads {
    #[serde(default)]
    download: Option<String>,
}

impl Listing {
    fn into_found(self) -> Found {
        Found {
            display: self.display.clone().unwrap_or_else(|| self.name.clone()),
            description: self.description.unwrap_or_default(),
            downloads: self.downloads,
            vsix: self.files.download.unwrap_or_default(),
            namespace: self.namespace,
            name: self.name,
        }
    }
}

/// An extension's `package.json`, down to the one array this reads.
#[derive(Debug, Deserialize)]
struct Manifest {
    #[serde(default)]
    contributes: Contributes,
}

#[derive(Debug, Default, Deserialize)]
struct Contributes {
    #[serde(default)]
    themes: Vec<Contributed>,
}

/// One theme an extension contributes.
///
/// `label` is the name it is shown under and lives here rather than in the
/// theme file, which is why `vscode::File::name` is optional. `path` is
/// relative to the extension root.
#[derive(Debug, Deserialize)]
struct Contributed {
    #[serde(default)]
    label: Option<String>,
    #[serde(rename = "uiTheme", default)]
    ui: Option<String>,
    path: String,
}

/// Search the registry for themes.
///
/// `category=Themes` rather than filtering afterwards: the registry knows
/// which extensions are themes, and a query for "dark" without it comes back
/// full of language servers.
///
/// Sorted by download count, not by the registry's `relevance`, which is the
/// default and is measurably wrong for this: a search for "dracula" ranked by
/// relevance answers with five forks of it before the theme four hundred
/// thousand people have installed. Download count is a poor measure of a good
/// theme and a very good measure of *the one that was meant* — which is the
/// question a search field is being asked.
#[cfg(target_family = "wasm")]
pub fn search(_query: &str) -> Result<Vec<Found>> {
    bail!("this build cannot install themes")
}

#[cfg(not(target_family = "wasm"))]
pub fn search(query: &str) -> Result<Vec<Found>> {
    let url = format!(
        "https://open-vsx.org/api/-/search?query={}&category=Themes&size={RESULTS}&sortBy=downloadCount&sortOrder=desc",
        escape(query)
    );
    let body = fetch(&url, SEARCH_CEILING, SEARCH_TIMEOUT)?;
    let answer: Answer = serde_json::from_slice(&body)
        .context("open-vsx answered with something that is not a search result")?;
    Ok(answer.extensions.into_iter().map(Listing::into_found).collect())
}

/// Download one extension and write every theme in it to the themes folder.
///
/// Answers with the name of each theme written, because that is what the
/// settings page has to say afterwards — an extension may contribute one
/// theme or nine, and "installed" without saying what is not an answer to
/// "where did it go".
///
/// One theme that cannot be read does not stop the others: an extension with
/// a broken light half still has a working dark one, and refusing both would
/// be refusing the one somebody asked for. All of them failing is an error.
#[cfg(target_family = "wasm")]
pub fn install(_found: &Found) -> Result<Vec<String>> {
    bail!("this build cannot install themes")
}

#[cfg(not(target_family = "wasm"))]
pub fn install(found: &Found) -> Result<Vec<String>> {
    let folder =
        crate::dirs::themes().context("there is nowhere to keep a theme on this machine")?;

    // The search answer usually carries the download URL already. When it
    // does not — the lookup endpoint is also how a caller that only has a
    // name gets here — ask for the extension by name and take it from there.
    let url = match found.vsix.is_empty() {
        false => found.vsix.clone(),
        true => {
            let at = format!(
                "https://open-vsx.org/api/{}/{}",
                escape(&found.namespace),
                escape(&found.name)
            );
            let body = fetch(&at, SEARCH_CEILING, SEARCH_TIMEOUT)?;
            let listing: Listing = serde_json::from_slice(&body).with_context(|| {
                format!("{at} answered with something that is not an extension")
            })?;
            listing.files.download.unwrap_or_default()
        }
    };
    ensure!(!url.is_empty(), "open-vsx has no download for {}", found.name);

    let archive = fetch(&url, VSIX_CEILING, INSTALL_TIMEOUT)?;
    let files = unpack(&archive)?;
    let written = convert(&files, found)?;

    crate::store::create_dir_all(&folder)
        .with_context(|| format!("could not make {}", folder.display()))?;

    let mut names = Vec::new();
    for (file, text, name) in written {
        let path = folder.join(&file);
        crate::store::write(&path, text.as_bytes())
            .with_context(|| format!("could not write {}", path.display()))?;
        names.push(name);
    }
    Ok(names)
}

/// Every JSON file in the archive, keyed by its path inside the extension.
///
/// `extension/` comes off the front, because that prefix is the `.vsix`
/// container's and the paths in `package.json` are relative to what is under
/// it. Anything that is not JSON is skipped without being read — which is
/// almost all of a `.vsix` by weight.
#[cfg(not(target_family = "wasm"))]
fn unpack(bytes: &[u8]) -> Result<HashMap<String, String>> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .context("open-vsx sent something that is not an extension")?;

    let mut files = HashMap::new();
    for at in 0..archive.len() {
        let entry = archive.by_index(at).context("could not read the extension")?;
        if !entry.is_file() {
            continue;
        }
        let Some(path) = entry.enclosed_name() else { continue };
        let path = path.to_string_lossy().replace('\\', "/");
        let Some(path) = path.strip_prefix("extension/") else { continue };
        if !path.to_ascii_lowercase().ends_with(".json") {
            continue;
        }
        // The claimed size first, so an entry that says it is enormous is
        // refused before a byte of it is decompressed; then the read itself is
        // bounded anyway, because the claim is written by whoever made the zip.
        if entry.size() > ENTRY_CEILING {
            continue;
        }
        let mut text = String::new();
        if entry.take(ENTRY_CEILING).read_to_string(&mut text).is_err() {
            continue;
        }
        files.insert(path.to_string(), text);
    }
    Ok(files)
}

/// Turn an unpacked extension into the files to write.
///
/// Answers with a `(file name, contents, theme name)` for each theme, and
/// touches nothing on disk — which is what makes the interesting half of an
/// install testable without a network or a folder.
fn convert(
    files: &HashMap<String, String>,
    found: &Found,
) -> Result<Vec<(String, String, String)>> {
    let manifest = files.get("package.json").context("the extension has no package.json")?;
    let manifest: Manifest = serde_json::from_str(&crate::vscode::strip(manifest))
        .context("the extension's package.json cannot be read")?;

    ensure_themes(&manifest)?;

    // What the labels are written in, where they are not written in words.
    // Absent from most extensions, and load-bearing in the ones that have it.
    let nls: Map<String, Value> = files
        .get("package.nls.json")
        .and_then(|text| serde_json::from_str(&crate::vscode::strip(text)).ok())
        .unwrap_or_default();

    let mut out = Vec::new();
    for theme in &manifest.contributes.themes {
        let path = tidy(&theme.path);
        // A broken half is skipped rather than fatal; see the note on
        // `install`. The check below is what makes "all of them broken" the
        // error instead.
        let Ok(file) = crate::vscode::resolve(&path, files) else { continue };

        let name = theme
            .label
            .as_deref()
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .and_then(|label| said(label, &nls))
            .or_else(|| file.name.clone())
            .unwrap_or_else(|| found.display.clone());

        // The file's own `type` first: it is the theme saying what it is, and
        // `uiTheme` is the manifest saying it on the theme's behalf. Both are
        // absent often enough that `vscode::appearance_of` still has to guess
        // from the background, which is why neither is required here.
        let kind = file.kind.clone().or_else(|| kind_of(theme.ui.as_deref()));

        let mut body = Map::new();
        body.insert("name".into(), Value::String(name.clone()));
        if let Some(kind) = kind {
            body.insert("type".into(), Value::String(kind));
        }
        body.insert("author".into(), Value::String(found.namespace.clone()));
        body.insert("family".into(), Value::String(found.display.clone()));
        body.insert("colors".into(), Value::Object(file.colors));

        let text = serde_json::to_string_pretty(&Value::Object(body))
            .context("could not write the theme out")?;
        out.push((file_name(found, &name), text + "\n", name));
    }

    ensure!(!out.is_empty(), "no theme in {} could be read", found.display);
    Ok(out)
}

/// A label, in words rather than in a placeholder.
///
/// An extension that is translated does not put its theme's name in
/// `package.json`; it puts `%themeLabel%` there and the words in
/// `package.nls.json` beside it. The Solarized themes VS Code itself ships are
/// written this way, and without this they arrive in the picker under a row
/// called `%themeLabel%` — which is a row nobody can choose and, worse, one
/// that reads as this app having broken.
///
/// Only `package.nls.json`, which is the untranslated original. The
/// per-language files beside it are not read: the theme's *name* is the one
/// string in an extension that is usually left in English anyway, and picking
/// a language would mean picking one, which is a question this app has never
/// had to ask.
///
/// A placeholder with nothing behind it answers `None` rather than itself, so
/// the caller falls through to the theme's own name.
fn said(label: &str, nls: &Map<String, Value>) -> Option<String> {
    let Some(key) = label.strip_prefix('%').and_then(|rest| rest.strip_suffix('%')) else {
        return Some(label.to_string());
    };
    let said = nls.get(key)?.as_str()?.trim();
    match said.is_empty() {
        true => None,
        false => Some(said.to_string()),
    }
}

/// The one thing worth failing an install over: the extension is not a theme.
fn ensure_themes(manifest: &Manifest) -> Result<()> {
    if manifest.contributes.themes.is_empty() {
        bail!("this extension contributes no colour theme");
    }
    Ok(())
}

/// What one theme is called on disk.
///
/// Built here rather than taken from the archive — see the module note. Three
/// parts so that two publishers may both ship a theme called "Dark" and both
/// end up in the folder, and in this order so that a folder listing groups an
/// extension's themes together.
fn file_name(found: &Found, theme: &str) -> String {
    format!("{}.{}.{}.json", slug(&found.namespace), slug(&found.name), slug(theme))
}

/// A path inside the extension, in the form `vscode::resolve` keys on.
fn tidy(path: &str) -> String {
    let path = path.replace('\\', "/");
    let path = path.strip_prefix("./").unwrap_or(&path);
    path.strip_prefix("extension/").unwrap_or(path).to_string()
}

/// `uiTheme` in the words the theme format uses for the same thing.
///
/// The two high-contrast values map to the format's own high-contrast names
/// rather than to dark and light, because `vscode::appearance_of` already
/// knows what to do with them and flattening them here would throw away the
/// only thing they say.
fn kind_of(ui: Option<&str>) -> Option<String> {
    Some(
        match ui? {
            "vs" => "light",
            "vs-dark" => "dark",
            "hc-black" => "hc",
            "hc-light" => "hcLight",
            _ => return None,
        }
        .to_string(),
    )
}

/// A word safe to put in a file name on all three platforms.
///
/// Everything that is not a letter or a digit becomes a dash, runs of dashes
/// collapse, and the result is cut short. That is stricter than any of the
/// three platforms needs and deliberately so: it is one rule instead of three
/// tables, and a theme file name nobody can type is no better than one the
/// filesystem refuses.
fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
        if out.len() >= 40 {
            break;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("theme");
    }
    out
}

/// Percent-encode a query for a URL.
///
/// Forty lines of dependency avoided for six. The unreserved set is the one
/// RFC 3986 names; everything else is encoded, including the space, which is
/// the character every real query has in it.
#[cfg(not(target_family = "wasm"))]
fn escape(text: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

/// One bounded GET.
///
/// The same shape as `update/net.rs`, and separate from it because the two
/// have different ceilings, different timeouts and no caller in common —
/// sharing them would mean one function with four arguments describing which
/// of the two it is being.
#[cfg(not(target_family = "wasm"))]
fn fetch(url: &str, ceiling: u64, timeout: Duration) -> Result<Vec<u8>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(concat!("mbrd/", env!("CARGO_PKG_VERSION")))
        .timeout_global(Some(timeout))
        .build()
        .into();

    let mut response = agent.get(url).call().with_context(|| format!("could not reach {url}"))?;
    let status = response.status();
    ensure!(status.is_success(), "{url} answered {status}");

    let mut body = Vec::new();
    // One byte past the ceiling, so that going over is detected rather than
    // silently truncated to exactly the ceiling — which would look like a
    // clean answer that happens not to parse.
    response
        .body_mut()
        .as_reader()
        .take(ceiling + 1)
        .read_to_end(&mut body)
        .with_context(|| format!("could not read {url}"))?;
    ensure!(body.len() as u64 <= ceiling, "{url} sent more than {ceiling} bytes");

    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found() -> Found {
        Found {
            namespace: "dracula-theme".into(),
            name: "theme-dracula".into(),
            display: "Dracula Official".into(),
            description: String::new(),
            downloads: 0,
            vsix: String::new(),
        }
    }

    fn extension(files: &[(&str, &str)]) -> HashMap<String, String> {
        files.iter().map(|(at, text)| (at.to_string(), text.to_string())).collect()
    }

    #[test]
    fn an_extension_becomes_one_file_for_each_theme_it_contributes() {
        let files = extension(&[
            (
                "package.json",
                r##"{"contributes":{"themes":[
                    {"label":"Dracula","uiTheme":"vs-dark","path":"./themes/dracula.json"},
                    {"label":"Dracula Soft","uiTheme":"vs-dark","path":"./themes/soft.json"}
                ]}}"##,
            ),
            ("themes/dracula.json", r##"{"colors":{"editor.background":"#282a36"}}"##),
            ("themes/soft.json", r##"{"colors":{"editor.background":"#31333f"}}"##),
        ]);
        let out = convert(&files, &found()).expect("both themes are readable");
        let names: Vec<&str> = out.iter().map(|(_, _, name)| name.as_str()).collect();
        assert_eq!(names, ["Dracula", "Dracula Soft"]);
        assert_eq!(out[0].0, "dracula-theme.theme-dracula.dracula.json");
        assert_eq!(out[1].0, "dracula-theme.theme-dracula.dracula-soft.json");
    }

    #[test]
    fn what_is_written_stands_alone_without_the_file_it_included() {
        // The reason this module rewrites rather than copies. The light half
        // is six colours over the dark one, and the dark one is not going to
        // be in anybody's themes folder.
        let files = extension(&[
            (
                "package.json",
                r##"{"contributes":{"themes":[{"label":"Half","uiTheme":"vs","path":"./light.json"}]}}"##,
            ),
            (
                "dark.json",
                r##"{"colors":{"editor.background":"#101010","editor.foreground":"#eeeeee"}}"##,
            ),
            (
                "light.json",
                r##"{"include":"./dark.json","colors":{"editor.background":"#ffffff"}}"##,
            ),
        ]);
        let out = convert(&files, &found()).expect("the light half is readable");
        let written: Value = serde_json::from_str(&out[0].1).expect("valid JSON comes out");
        let colors = &written["colors"];
        assert_eq!(colors["editor.background"], "#ffffff", "the near file has the last word");
        assert_eq!(colors["editor.foreground"], "#eeeeee", "the included file is still in there");
        assert!(written.get("include").is_none(), "nothing is left to follow");
    }

    #[test]
    fn a_theme_carries_where_it_came_from() {
        // How the settings page says "Dracula Official" under a row later.
        // Neither key is a VS Code one; see `vscode::File`.
        let files = extension(&[
            (
                "package.json",
                r##"{"contributes":{"themes":[{"label":"Dracula","path":"themes/d.json"}]}}"##,
            ),
            ("themes/d.json", r##"{"colors":{"editor.background":"#282a36"}}"##),
        ]);
        let out = convert(&files, &found()).expect("the theme is readable");
        let written: Value = serde_json::from_str(&out[0].1).expect("valid JSON comes out");
        assert_eq!(written["author"], "dracula-theme");
        assert_eq!(written["family"], "Dracula Official");
    }

    #[test]
    fn the_manifest_says_what_the_theme_file_does_not() {
        let files = extension(&[
            (
                "package.json",
                r##"{"contributes":{"themes":[{"label":"Day","uiTheme":"vs","path":"d.json"}]}}"##,
            ),
            ("d.json", r##"{"colors":{"editor.background":"#ffffff"}}"##),
        ]);
        let out = convert(&files, &found()).expect("the theme is readable");
        let written: Value = serde_json::from_str(&out[0].1).expect("valid JSON comes out");
        assert_eq!(written["type"], "light");
    }

    #[test]
    fn the_theme_file_outranks_the_manifest_about_itself() {
        let files = extension(&[
            (
                "package.json",
                r##"{"contributes":{"themes":[{"label":"Odd","uiTheme":"vs","path":"d.json"}]}}"##,
            ),
            ("d.json", r##"{"type":"dark","colors":{"editor.background":"#101010"}}"##),
        ]);
        let out = convert(&files, &found()).expect("the theme is readable");
        let written: Value = serde_json::from_str(&out[0].1).expect("valid JSON comes out");
        assert_eq!(written["type"], "dark");
    }

    #[test]
    fn a_theme_file_that_is_missing_does_not_take_its_neighbour_with_it() {
        let files = extension(&[
            (
                "package.json",
                r##"{"contributes":{"themes":[
                    {"label":"Gone","path":"./themes/gone.json"},
                    {"label":"Here","path":"./themes/here.json"}
                ]}}"##,
            ),
            ("themes/here.json", r##"{"colors":{"editor.background":"#282a36"}}"##),
        ]);
        let out = convert(&files, &found()).expect("one half is enough");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].2, "Here");
    }

    #[test]
    fn a_translated_extension_is_named_in_words_rather_than_in_a_placeholder() {
        // What VS Code's own Solarized themes do, and what put a row called
        // `%themeLabel%` in the picker before this.
        let files = extension(&[
            (
                "package.json",
                r##"{"contributes":{"themes":[{"label":"%themeLabel%","path":"d.json"}]}}"##,
            ),
            ("package.nls.json", r##"{"themeLabel":"Solarized Dark"}"##),
            ("d.json", r##"{"colors":{"editor.background":"#002b36"}}"##),
        ]);
        let out = convert(&files, &found()).expect("the theme is readable");
        assert_eq!(out[0].2, "Solarized Dark");
        assert_eq!(out[0].0, "dracula-theme.theme-dracula.solarized-dark.json");
    }

    #[test]
    fn a_placeholder_with_nothing_behind_it_falls_through_to_the_theme_itself() {
        let files = extension(&[
            (
                "package.json",
                r##"{"contributes":{"themes":[{"label":"%gone%","path":"d.json"}]}}"##,
            ),
            ("d.json", r##"{"name":"Solarized Dark","colors":{"editor.background":"#002b36"}}"##),
        ]);
        let out = convert(&files, &found()).expect("the theme is readable");
        assert_eq!(out[0].2, "Solarized Dark");
    }

    #[test]
    fn an_extension_that_is_not_a_theme_is_refused() {
        let files = extension(&[("package.json", r##"{"contributes":{"commands":[]}}"##)]);
        assert!(convert(&files, &found()).is_err());
    }

    #[test]
    fn an_extension_whose_themes_are_all_missing_is_refused() {
        // Not the same as the one above, and worth telling apart: this one
        // said it had themes.
        let files = extension(&[(
            "package.json",
            r##"{"contributes":{"themes":[{"label":"Gone","path":"./gone.json"}]}}"##,
        )]);
        assert!(convert(&files, &found()).is_err());
    }

    #[test]
    fn a_manifest_may_be_jsonc_like_everything_else_in_this_format() {
        let files = extension(&[
            (
                "package.json",
                r##"{
                    // published from a template
                    "contributes":{"themes":[{"label":"Day","path":"d.json"},]}
                }"##,
            ),
            ("d.json", r##"{"colors":{"editor.background":"#ffffff"}}"##),
        ]);
        assert_eq!(convert(&files, &found()).expect("comments are legal here").len(), 1);
    }

    #[test]
    fn nothing_the_archive_says_reaches_a_path() {
        // The whole of the module note's safety claim in one assertion: the
        // file name is built from the three names, and a theme label that is
        // a path traversal is a label like any other.
        let mut escaping = found();
        escaping.namespace = "../..".into();
        let name = file_name(&escaping, "../../../.bashrc");
        assert_eq!(name, "theme.theme-dracula.bashrc.json");
        assert!(!name.contains('/') && !name.contains(".."));
    }

    #[test]
    fn a_file_name_stays_short_enough_to_write() {
        let mut long = found();
        long.name = "a".repeat(300);
        let name = file_name(&long, &"b".repeat(300));
        assert!(name.len() < 128, "{name} is too long for a filesystem");
    }

    #[test]
    fn a_query_with_a_space_in_it_is_still_one_query() {
        assert_eq!(escape("solarized dark"), "solarized%20dark");
        assert_eq!(escape("c++"), "c%2B%2B");
        assert_eq!(escape("night-owl_2.0~"), "night-owl_2.0~");
    }

    #[test]
    fn a_path_is_read_the_same_however_the_manifest_wrote_it() {
        assert_eq!(tidy("./themes/dark.json"), "themes/dark.json");
        assert_eq!(tidy("themes/dark.json"), "themes/dark.json");
        assert_eq!(tidy(".\\themes\\dark.json"), "themes/dark.json");
        assert_eq!(tidy("extension/themes/dark.json"), "themes/dark.json");
    }

    #[test]
    fn a_search_answer_becomes_rows() {
        let body = r##"{"offset":0,"totalSize":1,"extensions":[
            {"namespace":"dracula-theme","name":"theme-dracula","version":"2.25.1",
             "displayName":"Dracula Official","description":"Dark theme",
             "downloadCount":4200000,
             "files":{"download":"https://open-vsx.org/api/x/y/1/file/z.vsix"}}
        ]}"##;
        let answer: Answer = serde_json::from_str(body).expect("this is what the API sends");
        let rows: Vec<Found> = answer.extensions.into_iter().map(Listing::into_found).collect();
        assert_eq!(rows[0].display, "Dracula Official");
        assert_eq!(rows[0].downloads, 4_200_000);
        assert!(rows[0].vsix.ends_with(".vsix"));
    }

    #[test]
    fn a_result_with_nothing_but_a_name_is_still_a_row() {
        // Every field but the two names is optional in practice, and a row
        // that cannot be built is a theme nobody can install.
        let body = r##"{"extensions":[{"namespace":"someone","name":"quiet"}]}"##;
        let answer: Answer = serde_json::from_str(body).expect("this parses");
        let row = answer.extensions.into_iter().map(Listing::into_found).next().unwrap();
        assert_eq!(row.display, "quiet");
        assert_eq!(row.description, "");
        assert!(row.vsix.is_empty(), "install asks again by name");
    }
}
