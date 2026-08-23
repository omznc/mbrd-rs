//! Three numbers and a comparison.
//!
//! Hand-rolled rather than `semver`, which would be the workspace's ninth
//! direct dependency for a job that is twenty lines and one trap. The trap is
//! worth naming, because it is the reason this is not a string comparison:
//! `"0.10.0" < "0.9.0"` is true of the text and false of the versions, and an
//! updater that gets it the wrong way round stops offering updates for good at
//! the tenth release and does it silently.
//!
//! Pre-release suffixes are **rejected** rather than ordered. `0.3.0-rc1` has a
//! well-known place in semver's ordering and no place at all in this app: a
//! release candidate is not something to push at everybody who has the app
//! open. Failing to parse it means the manifest is refused and nothing is
//! offered, which is the safe direction to fail in.

use std::fmt;

/// A released version — the thing a tag and `[workspace.package]` agree on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    // The field order *is* the comparison. `derive(Ord)` on a struct compares
    // fields in declaration order, which is exactly major-then-minor-then-patch,
    // so there is no hand-written `cmp` here to get wrong.
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    /// This build's own version, from Cargo.
    ///
    /// Panics only if `[workspace.package] version` is not three numbers, which
    /// would fail the release workflow's tag check long before it reached
    /// anybody.
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("the crate's own version parses")
    }

    /// `0.3.0`, or `v0.3.0` — the tag spelling is accepted because the tag is
    /// what a human copies when they are testing this by hand.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);

        let mut parts = text.split('.');
        let mut number = || -> Option<u32> {
            let part = parts.next()?;
            // `u32::from_str` accepts a leading `+`, and `+3` is not a version
            // component. Check the shape before trusting the parse.
            if part.is_empty() || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            part.parse().ok()
        };

        let version = Self { major: number()?, minor: number()?, patch: number()? };
        // Exactly three. A trailing `.4` or a `-rc1` left over means this was
        // not the kind of version this app releases, and guessing is worse
        // than declining.
        parts.next().is_none().then_some(version)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).expect("a version the tests wrote themselves")
    }

    #[test]
    fn ten_is_after_nine() {
        // The whole reason this module is not a string comparison.
        assert!(v("0.10.0") > v("0.9.0"));
        assert!(v("0.9.0") < v("0.10.0"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert!(v("0.2.10") > v("0.2.9"));
    }

    #[test]
    fn ordering_runs_major_then_minor_then_patch() {
        let mut all = [v("1.0.0"), v("0.0.1"), v("0.1.0"), v("1.0.1"), v("0.2.0")];
        all.sort();
        let shown: Vec<String> = all.iter().map(Version::to_string).collect();
        assert_eq!(shown, ["0.0.1", "0.1.0", "0.2.0", "1.0.0", "1.0.1"]);
    }

    #[test]
    fn a_tag_spelling_is_accepted() {
        assert_eq!(v("v0.3.0"), v("0.3.0"));
        assert_eq!(v("  0.3.0  "), v("0.3.0"));
    }

    #[test]
    fn anything_that_is_not_three_numbers_is_refused() {
        // Each of these has an obvious intent and no obvious ordering, and the
        // safe direction to fail in is "offer nothing".
        for bad in [
            "0.3",
            "0.3.0.1",
            "0.3.0-rc1",
            "0.3.0+build",
            "",
            "v",
            "..",
            "0..0",
            "-1.0.0",
            "+1.0.0",
            "0.3.x",
            "latest",
            "0. 3.0",
        ] {
            assert!(Version::parse(bad).is_none(), "{bad:?} should not have parsed");
        }
    }

    #[test]
    fn the_crates_own_version_parses() {
        // `current()` panics on a malformed version, and the one version that
        // must never do that is this one.
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }
}
