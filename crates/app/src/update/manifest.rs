//! What the server says is available, and why it should be believed.
//!
//! Nothing here touches the network — [`Manifest::verify`] is handed bytes and
//! a signature and answers yes or no. That is the point: this is the module
//! where a mistake is remote code execution on every machine running the app,
//! so it is the module with no I/O in it and a test for every rule.
//!
//! ## The shape of the trust
//!
//! One ed25519 signature over the whole manifest, and a SHA-256 per artifact
//! *inside* it. So the expensive asymmetric check happens once, on two
//! kilobytes, and the thirty-megabyte download is checked against a hash the
//! signature has already vouched for. Sparkle and Tauri both landed here and
//! they are right: signing each artifact separately buys nothing and costs a
//! verification path per platform.
//!
//! ## Why the URLs are checked and not just used
//!
//! A signed manifest could still name `http://` or a host we do not publish
//! from, and a signature only says "we wrote this" — not "this is sensible".
//! The two together are what makes a stolen or mis-generated manifest less
//! useful than it looks, and neither costs anything to enforce.

use std::collections::BTreeMap;

use anyhow::{ensure, Context as _, Result};
use minisign_verify::{PublicKey, Signature};
use serde_json::Value;

use super::version::Version;

/// Where artifacts are allowed to come from.
///
/// Releases live on `github.com` and are served from `objects.githubusercontent.com`
/// after the redirect, so both are named. A signed manifest pointing anywhere
/// else is a signed manifest that is wrong, and this is the cheapest place to
/// notice.
const HOSTS: [&str; 2] = ["github.com", "objects.githubusercontent.com"];

/// The largest artifact worth believing in, as a sanity bound on `size`.
///
/// The real artifacts are tens of megabytes; this is not a tight limit, it is
/// the one that stops a mistyped manifest from asking somebody's laptop to
/// stream a terabyte before anything checks anything.
const SIZE_CEILING: u64 = 512 * 1024 * 1024;

/// One platform's download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Artifact {
    pub url: String,
    /// Bytes. Checked before the download starts and again after it ends.
    pub size: u64,
    /// Lowercase hex, 64 characters.
    pub sha256: String,
}

/// What is available, once it has been believed.
///
/// There is no way to build one of these except through [`Manifest::verify`],
/// which is deliberate: a `Manifest` in hand is a manifest whose signature
/// checked out, so no code downstream has to remember to ask.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Manifest {
    pub version: Version,
    /// Where the release notes are, for somebody who wants to read before
    /// installing.
    pub notes: String,
    targets: BTreeMap<String, Artifact>,
}

impl Manifest {
    /// Check the signature, then read it.
    ///
    /// In that order, and the order is the whole security property: nothing in
    /// `json` is parsed, trusted or acted on until the signature over those
    /// exact bytes has verified against `key`.
    pub fn verify(json: &[u8], signature: &str, key: &str) -> Result<Self> {
        let key = PublicKey::from_base64(key.trim())
            .context("the built-in update key is not a valid minisign public key")?;
        let signature =
            Signature::decode(signature.trim()).context("the manifest's signature is malformed")?;
        key.verify(json, &signature, false)
            .map_err(|err| anyhow::anyhow!("the manifest's signature does not check out: {err}"))?;

        Self::parse(json)
    }

    /// Read a manifest whose signature has already been checked.
    ///
    /// Private, so that the only way to a `Manifest` runs through [`verify`].
    /// The tests reach it directly, which is the one place skipping the
    /// signature is the point rather than a shortcut.
    ///
    /// [`verify`]: Manifest::verify
    fn parse(json: &[u8]) -> Result<Self> {
        let value: Value = serde_json::from_slice(json).context("the manifest is not JSON")?;

        let version = value
            .get("version")
            .and_then(Value::as_str)
            .and_then(Version::parse)
            .context("the manifest has no readable `version`")?;

        let notes =
            value.get("notes").and_then(Value::as_str).unwrap_or_default().trim().to_string();
        ensure!(
            notes.is_empty() || notes.starts_with("https://"),
            "the manifest's `notes` is not an https URL"
        );

        let listed = value
            .get("targets")
            .and_then(Value::as_object)
            .context("the manifest has no `targets`")?;

        let mut targets = BTreeMap::new();
        for (triple, entry) in listed {
            let artifact = Self::artifact(entry)
                .with_context(|| format!("the manifest's entry for {triple} is unusable"))?;
            targets.insert(triple.clone(), artifact);
        }
        ensure!(!targets.is_empty(), "the manifest lists no targets at all");

        Ok(Self { version, notes, targets })
    }

    fn artifact(entry: &Value) -> Result<Artifact> {
        let url = entry.get("url").and_then(Value::as_str).context("no `url`")?.trim().to_string();

        // Both halves matter. `https` alone would allow any host; a host check
        // on a `http://` URL would be checking the label on a plaintext
        // download.
        let rest = url.strip_prefix("https://").context("`url` is not https")?;
        let host = rest.split(['/', ':', '?', '#']).next().unwrap_or_default();
        ensure!(HOSTS.contains(&host), "`url` points at {host}, which is not a release host");

        let size = entry.get("size").and_then(Value::as_u64).context("no `size`")?;
        ensure!(size > 0, "`size` is zero");
        ensure!(size <= SIZE_CEILING, "`size` of {size} bytes is not believable");

        let sha256 = entry
            .get("sha256")
            .and_then(Value::as_str)
            .context("no `sha256`")?
            .trim()
            .to_ascii_lowercase();
        ensure!(sha256.len() == 64, "`sha256` is not 64 characters");
        ensure!(sha256.bytes().all(|b| b.is_ascii_hexdigit()), "`sha256` is not hexadecimal");

        Ok(Artifact { url, size, sha256 })
    }

    /// What this build should download, if the manifest has anything for it.
    ///
    /// `None` is ordinary rather than an error: a release that skipped a
    /// platform, or a build for a target nothing is published for, both land
    /// here and both mean "nothing to offer" rather than "something is wrong".
    pub fn artifact_for(&self, target: &str) -> Option<&Artifact> {
        self.targets.get(target)
    }

    /// Whether this is worth telling somebody about.
    ///
    /// Strictly newer. Equal is the ordinary case — most launches — and older
    /// means somebody has been moved backwards deliberately, which is not
    /// something an updater should quietly undo.
    pub fn is_newer_than(&self, current: Version) -> bool {
        self.version > current
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A manifest that is fine, so each test can spoil exactly one thing.
    fn good() -> Value {
        serde_json::json!({
            "version": "0.3.0",
            "notes": "https://github.com/omznc/mbrd-rs/releases/tag/v0.3.0",
            "targets": {
                "x86_64-unknown-linux-gnu": {
                    "url": "https://github.com/omznc/mbrd-rs/releases/download/v0.3.0/mbrd.tar.gz",
                    "size": 40123456u64,
                    "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                }
            }
        })
    }

    fn parse(value: &Value) -> Result<Manifest> {
        Manifest::parse(value.to_string().as_bytes())
    }

    #[test]
    fn a_good_manifest_reads() {
        let manifest = parse(&good()).expect("this one is fine");
        assert_eq!(manifest.version, Version::parse("0.3.0").unwrap());
        let artifact =
            manifest.artifact_for("x86_64-unknown-linux-gnu").expect("the target it lists");
        assert_eq!(artifact.size, 40123456);
        assert!(manifest.artifact_for("aarch64-apple-darwin").is_none());
    }

    #[test]
    fn only_a_strictly_newer_version_is_offered() {
        let manifest = parse(&good()).unwrap();
        assert!(manifest.is_newer_than(Version::parse("0.2.9").unwrap()));
        // Equal is what almost every launch sees.
        assert!(!manifest.is_newer_than(Version::parse("0.3.0").unwrap()));
        // Older is somebody having been moved back deliberately.
        assert!(!manifest.is_newer_than(Version::parse("0.4.0").unwrap()));
    }

    #[test]
    fn a_url_that_is_not_https_is_refused() {
        let mut m = good();
        m["targets"]["x86_64-unknown-linux-gnu"]["url"] =
            "http://github.com/omznc/mbrd-rs/x.tar.gz".into();
        assert!(parse(&m).is_err());
    }

    #[test]
    fn a_url_on_another_host_is_refused() {
        // The attack this exists for: a manifest that verifies, because the
        // key leaked or because it is an old one being replayed, but points
        // the download somewhere else.
        for url in [
            "https://example.com/mbrd.tar.gz",
            "https://github.com.example.com/mbrd.tar.gz",
            "https://notgithub.com/mbrd.tar.gz",
            "https://evil.com/?x=github.com",
        ] {
            let mut m = good();
            m["targets"]["x86_64-unknown-linux-gnu"]["url"] = url.into();
            assert!(parse(&m).is_err(), "{url} should have been refused");
        }
    }

    #[test]
    fn a_host_with_a_port_or_credentials_is_still_measured_by_its_host() {
        let mut m = good();
        m["targets"]["x86_64-unknown-linux-gnu"]["url"] =
            "https://github.com:443/omznc/mbrd-rs/x.tar.gz".into();
        assert!(parse(&m).is_ok(), "an explicit port on the right host is fine");
    }

    #[test]
    fn a_hash_that_is_not_a_hash_is_refused() {
        for sha in [
            "",
            "abc",
            // 63 characters.
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b85",
            // 64 characters, one of them not hex.
            "z3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ] {
            let mut m = good();
            m["targets"]["x86_64-unknown-linux-gnu"]["sha256"] = sha.into();
            assert!(parse(&m).is_err(), "{sha:?} should have been refused");
        }
    }

    #[test]
    fn a_hash_is_compared_in_one_case() {
        let mut m = good();
        let upper = m["targets"]["x86_64-unknown-linux-gnu"]["sha256"]
            .as_str()
            .unwrap()
            .to_ascii_uppercase();
        m["targets"]["x86_64-unknown-linux-gnu"]["sha256"] = upper.into();
        let manifest = parse(&m).expect("upper case hex is still hex");
        assert_eq!(
            manifest.artifact_for("x86_64-unknown-linux-gnu").unwrap().sha256,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
            "it should have been folded on the way in, not at every comparison"
        );
    }

    #[test]
    fn an_unbelievable_size_is_refused() {
        for size in [0u64, SIZE_CEILING + 1, u64::MAX] {
            let mut m = good();
            m["targets"]["x86_64-unknown-linux-gnu"]["size"] = size.into();
            assert!(parse(&m).is_err(), "{size} should have been refused");
        }
    }

    #[test]
    fn a_manifest_missing_its_parts_is_refused() {
        for spoil in ["version", "targets"] {
            let mut m = good();
            m.as_object_mut().unwrap().remove(spoil);
            assert!(parse(&m).is_err(), "a manifest with no {spoil} should have been refused");
        }
        let mut empty = good();
        empty["targets"] = serde_json::json!({});
        assert!(parse(&empty).is_err(), "a manifest listing nothing should have been refused");
    }

    #[test]
    fn one_unusable_target_spoils_the_manifest() {
        // Rather than being skipped. A manifest with a broken entry was
        // generated by something that was not working properly, and trusting
        // the rest of what it says is a guess.
        let mut m = good();
        m["targets"]["x86_64-pc-windows-msvc"] =
            serde_json::json!({ "url": "https://github.com/x" });
        assert!(parse(&m).is_err());
    }

    #[test]
    fn the_manifest_the_release_workflow_writes_is_one_this_can_read() {
        // Copied verbatim from a run of the `Write the manifest` step in
        // `.github/workflows/release.yml`, against artifacts of a known size.
        //
        // This is the seam that cannot be tested any other way: the manifest
        // is assembled by `printf` in a shell script and read by the parser
        // above, and nothing else connects the two. If they drift, every
        // install silently refuses every update and the only symptom is that
        // nobody is ever offered one.
        const FROM_CI: &str = r#"{
  "version": "0.3.0",
  "notes": "https://github.com/omznc/mbrd-rs/releases/tag/v0.3.0",
  "targets": {
    "aarch64-apple-darwin": { "url": "https://github.com/omznc/mbrd-rs/releases/download/v0.3.0/mbrd_0.3.0_aarch64.app.tar.gz", "size": 100000, "sha256": "449ecdf39351d0bc0763e8db86eaad22aaed6e2a154df9a6c79b16abc6db0e98" },
    "x86_64-apple-darwin": { "url": "https://github.com/omznc/mbrd-rs/releases/download/v0.3.0/mbrd_0.3.0_x64.app.tar.gz", "size": 100000, "sha256": "7bb8d863736722256cb93d4017f525b58b66ea002093d8a0297253512c80f086" },
    "x86_64-pc-windows-msvc": { "url": "https://github.com/omznc/mbrd-rs/releases/download/v0.3.0/mbrd_0.3.0_x64.exe", "size": 100000, "sha256": "0faeecbacd022dd92fa8b6acc1677adb4b3f7bbd068c5712bf2989ba68f0c62f" },
    "x86_64-unknown-linux-gnu": { "url": "https://github.com/omznc/mbrd-rs/releases/download/v0.3.0/mbrd_0.3.0_x86_64-linux.tar.gz", "size": 100000, "sha256": "ac55424209c66e2cf9ba1b7ada7a01ba2243d1e19908101f2808530665f9f130" }
  }
}"#;

        let manifest = Manifest::parse(FROM_CI.as_bytes()).expect("CI writes a readable manifest");
        assert_eq!(manifest.version, Version::parse("0.3.0").unwrap());

        // Every target the release workflow publishes has to be a target this
        // can look up — a triple misspelt on either side is a platform that
        // never gets an update.
        for triple in [
            "aarch64-apple-darwin",
            "x86_64-apple-darwin",
            "x86_64-pc-windows-msvc",
            "x86_64-unknown-linux-gnu",
        ] {
            let artifact = manifest.artifact_for(triple).unwrap_or_else(|| {
                panic!("the workflow publishes {triple} and this cannot find it")
            });
            assert_eq!(artifact.size, 100_000);
        }

        // And this build's own triple is one of them, on the platforms the
        // workflow actually builds for. A target nobody publishes is a build
        // that will never be offered anything, which is correct behaviour but
        // worth noticing rather than discovering.
        if matches!(
            super::super::TARGET,
            "aarch64-apple-darwin"
                | "x86_64-apple-darwin"
                | "x86_64-pc-windows-msvc"
                | "x86_64-unknown-linux-gnu"
        ) {
            assert!(
                manifest.artifact_for(super::super::TARGET).is_some(),
                "{} is built by the workflow but absent from its manifest",
                super::super::TARGET
            );
        }
    }

    // A real minisign key pair and a real signature over a real manifest,
    // generated once from a fixed seed so the fixture is stable. The key is a
    // test key and signs nothing that ships.
    //
    // These two tests are the only ones that exercise the actual cryptography
    // rather than the shape of the JSON around it — and they are the ones that
    // matter, because everything else in this module assumes a `Manifest` in
    // hand is one whose signature checked out.
    const TEST_PUBLIC_KEY: &str = "RWQLrcD/7hI0VgOhB7/zzhC+HXDdGOdLwJln5NYwm6UNXx3chmQSVTG4";

    const SIGNED_MANIFEST: &str = r#"{
  "version": "0.3.0",
  "notes": "https://github.com/omznc/mbrd-rs/releases/tag/v0.3.0",
  "targets": {
    "x86_64-unknown-linux-gnu": { "url": "https://github.com/omznc/mbrd-rs/releases/download/v0.3.0/mbrd_0.3.0_x86_64-linux.tar.gz", "size": 40123456, "sha256": "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855" }
  }
}
"#;

    const GOOD_SIGNATURE: &str = r#"untrusted comment: signature from mbrd test key
RUQLrcD/7hI0VvhoP+yqXOLDlBF80BYAQxxh5SJKQfESuTs+jCRzrdnd8wrevp/CDkKjn2vhxp0f/dJfCyQlOhmlBlqaHbP6bQo=
trusted comment: timestamp:1700000000	file:latest.json	prehashed
07n6Oldd2VKijw8myySpjapJKUY2qRh1EmtukFuEo0P4NFZ4unDY5DAUTqunHzJmv3SKWAXNx9XHqKOKAu6HBA==
"#;

    #[test]
    fn a_real_signature_from_the_real_key_verifies() {
        let manifest =
            Manifest::verify(SIGNED_MANIFEST.as_bytes(), GOOD_SIGNATURE, TEST_PUBLIC_KEY)
                .expect("a genuine signature over these bytes should verify");
        assert_eq!(manifest.version, Version::parse("0.3.0").unwrap());
        assert!(manifest.artifact_for("x86_64-unknown-linux-gnu").is_some());
    }

    #[test]
    fn a_manifest_changed_after_signing_does_not_verify() {
        // The whole point of the exercise. Every one of these is a byte an
        // attacker who could reach the download would want to change: where it
        // points, how big it is, what it hashes to, and which version it
        // claims to be.
        let tampered = [
            SIGNED_MANIFEST.replace("0.3.0", "9.9.9"),
            SIGNED_MANIFEST.replace("mbrd_0.3.0_x86_64-linux.tar.gz", "mbrd_evil.tar.gz"),
            SIGNED_MANIFEST.replace("40123456", "40123457"),
            SIGNED_MANIFEST.replace("e3b0c442", "00000000"),
            // Even a byte that changes nothing anybody reads.
            format!("{SIGNED_MANIFEST} "),
        ];
        for (i, json) in tampered.iter().enumerate() {
            assert_ne!(json.as_str(), SIGNED_MANIFEST, "case {i} changed nothing");
            assert!(
                Manifest::verify(json.as_bytes(), GOOD_SIGNATURE, TEST_PUBLIC_KEY).is_err(),
                "case {i} was altered after signing and still verified"
            );
        }
    }

    #[test]
    fn a_good_signature_against_a_different_key_does_not_verify() {
        // A different key id, so this fails before the maths even runs — which
        // is the case a key rotation would produce.
        const OTHER: &str = "RWRUXlrsLLtsGkT9U3jFcHiQz+8Fbd6xGGpHf5b3PXCBqIoUTFHNwsUZ";
        assert!(Manifest::verify(SIGNED_MANIFEST.as_bytes(), GOOD_SIGNATURE, OTHER).is_err());
    }

    #[test]
    fn a_legacy_signature_is_not_accepted() {
        // `verify` passes `allow_legacy: false`, so a non-prehashed signature
        // is refused whatever else is true of it. The release workflow signs
        // with `-H` for exactly this reason, and the two have to agree.
        let legacy = GOOD_SIGNATURE.replacen("RUQ", "RWQ", 1);
        assert!(Manifest::verify(SIGNED_MANIFEST.as_bytes(), &legacy, TEST_PUBLIC_KEY).is_err());
    }

    #[test]
    fn a_signature_from_the_wrong_key_does_not_verify() {
        // Generated once with `minisign -G`, and used here rather than a real
        // release key so that the fixture can live in the repository.
        const KEY: &str = "RWRUXlrsLLtsGkT9U3jFcHiQz+8Fbd6xGGpHf5b3PXCBqIoUTFHNwsUZ";
        let json = good().to_string();
        // Not a signature over anything; the point is that garbage is refused
        // rather than crashing.
        assert!(Manifest::verify(json.as_bytes(), "not a signature", KEY).is_err());
        assert!(Manifest::verify(json.as_bytes(), "", KEY).is_err());
    }

    #[test]
    fn a_malformed_key_is_refused_rather_than_ignored() {
        let json = good().to_string();
        assert!(Manifest::verify(json.as_bytes(), "sig", "not a key").is_err());
        assert!(Manifest::verify(json.as_bytes(), "sig", "").is_err());
    }
}
