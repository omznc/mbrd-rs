#!/usr/bin/env node
/**
 * Generate `crates/core/src/formats.rs` — what a file extension is called.
 *
 * The catalogue is **data**, and data of a size nobody should write by hand:
 * some thirteen hundred extensions, each with a name and a shelf to sit on.
 * The original does not write it either — its `import/formats.ts` is itself
 * generated from an analyser's tables — so this is a generator onto a
 * generator, and the alternative is a hand-kept subset that goes stale the
 * moment somebody drops a `.sldprt` on the board.
 *
 * The result is **committed**, like the icons: the input lives in another
 * repository, so a build step that reached for it would be a build that only
 * works on a machine with both checkouts side by side.
 *
 *   node scripts/formats.mjs [path/to/mbrd/web/assets/js/import/formats.ts]
 *
 * Defaults to `../mbrd/web/assets/js/import/formats.ts`, which is where the
 * original sits when the two are checked out beside each other.
 */
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const source = resolve(
  here,
  "..",
  process.argv[2] ?? "../mbrd/web/assets/js/import/formats.ts",
);
const out = resolve(here, "..", "crates/core/src/formats.rs");

const text = readFileSync(source, "utf8");

/**
 * Pull one `export const NAME = <literal>;` out of the source.
 *
 * A brace counter rather than a regex over the whole literal: `EXT_NAME` runs
 * to four hundred lines and carries braces inside its strings is not a risk
 * worth the shortcut, but a lazy `[\s\S]*?}` would stop at the first `}` and a
 * greedy one at the last in the file. Counting is the honest read.
 */
function literal(name) {
  const start = text.indexOf(`export const ${name}`);
  if (start < 0) throw new Error(`${name} is not in ${source}`);
  const open = text.indexOf("=", start) + 1;
  let depth = 0;
  for (let i = open; i < text.length; i++) {
    const c = text[i];
    if (c === "{" || c === "[") depth++;
    else if (c === "}" || c === "]") {
      depth--;
      if (depth === 0) return text.slice(open, i + 1);
    } else if (c === '"') {
      // Skip the string whole, so a brace inside one is not counted.
      i = text.indexOf('"', i + 1);
      if (i < 0) throw new Error(`unterminated string in ${name}`);
    }
  }
  throw new Error(`unterminated literal for ${name}`);
}

/** The literals are JSON but for unquoted keys and trailing commas. */
const json = (src) =>
  JSON.parse(
    src
      .replace(/([{,]\s*)([A-Za-z_][A-Za-z0-9_]*)\s*:/g, '$1"$2":')
      .replace(/,(\s*[}\]])/g, "$1"),
  );

const categories = json(literal("CATEGORIES"));
const families = json(literal("FAMILIES"));
const extFamily = json(literal("EXT_FAMILY"));
const extName = json(literal("EXT_NAME"));

// The categories, in the order the source declares them — which is roughly
// "what people drop most" and is the order the Rust enum is written in.
const catKeys = Object.keys(categories);
const variant = (key) =>
  ({
    images: "Images",
    audio: "Audio",
    video: "Video",
    design: "Design",
    documents: "Documents",
    data: "Data",
    threed: "ThreeD",
    archives: "Archives",
    maps: "Maps",
    games: "Games",
    security: "Security",
    system: "System",
  })[key] ?? (() => {
    throw new Error(`no Rust name for the category "${key}"`);
  })();

const rustStr = (s) => `"${s.replace(/\\/g, "\\\\").replace(/"/g, '\\"')}"`;

/** A `&[(&str, T)]` sorted by its key, wrapped to something readable. */
function table(entries, value) {
  const rows = entries
    .slice()
    .sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))
    .map(([k, v]) => `(${rustStr(k)}, ${value(v)})`);
  const lines = [];
  let line = "   ";
  for (const row of rows) {
    const next = `${line} ${row},`;
    if (next.length > 96) {
      lines.push(line);
      line = `    ${row},`;
    } else {
      line = next;
    }
  }
  if (line.trim()) lines.push(line);
  return lines.join("\n");
}

const names = Object.entries(extName);
const shelves = Object.entries(extFamily).filter(([, i]) => families[i]);

const rust = `//! What a file extension is called, and what shelf it sits on.
//!
//! **Generated. Do not edit by hand** — run \`node scripts/formats.mjs\`, which
//! reads the original's own generated catalogue and writes this. See that
//! script for where the table comes from and why it is committed rather than
//! built.
//!
//! ${names.length} extensions named, across ${shelves.length} filed into ${families.length} families in
//! ${catKeys.length} categories.
//!
//! ## This names, it does not classify
//!
//! Nothing in here decides what *kind of card* a file becomes. That is
//! \`import::by_extension\`'s job and it stays a short hand-written table on
//! purpose: this catalogue knows a \`.cr3\` is a photograph, and calling it an
//! image card would put a frame on the board that can never draw, because
//! nothing in this tree decodes Canon RAW. A name is safe to be generous with;
//! a card type is not.
//!
//! So what this is for is the *word* on a card and in the information rail. A
//! \`.sldprt\` reading "SolidWorks part" rather than "file" is the whole of the
//! feature, and it costs a binary search.

/// The twelve shelves everything is filed on.
///
/// Coarser than a family and useful where a family is too specific to show —
/// a card that says "Images" is a card somebody can sort by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
${catKeys.map((k) => `    ${variant(k)},`).join("\n")}
}

impl Category {
    /// What the shelf is called, spelled the way the original spells it.
    pub fn label(self) -> &'static str {
        match self {
${catKeys.map((k) => `            Self::${variant(k)} => ${rustStr(categories[k])},`).join("\n")}
        }
    }
}

/// A family of formats — a shelf's worth, with the shelf named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Family {
    pub label: &'static str,
    pub category: Category,
}

/// What this format is called: "Hasselblad RAW", "7-Zip archive".
///
/// \`None\` for an extension the catalogue has never heard of, which is a real
/// answer — the caller has its own word for that and it is not this module's
/// to invent.
pub fn name(ext: &str) -> Option<&'static str> {
    lookup(&NAMES, ext).copied()
}

/// Which family this format belongs to, where it belongs to one.
///
/// Broader than [\`name\`] and answered for fewer extensions: a handful of
/// formats are named outright without any family claiming them.
pub fn family(ext: &str) -> Option<Family> {
    lookup(&SHELVES, ext).map(|&i| FAMILIES[i as usize])
}

/// A binary search over a table sorted at generation time.
///
/// Sorted here rather than at startup, which is what lets both tables be
/// \`static\` and cost nothing until something is dropped on the board. The
/// order is byte order over ASCII, which is what
/// \`the_tables_are_sorted_the_way_the_search_expects\` holds.
fn lookup<'t, T>(table: &'t [(&'static str, T)], ext: &str) -> Option<&'t T> {
    // The catalogue is lowercase with no leading dot, and a name off a disk is
    // neither reliably. Borrowed where it already matches, so the ordinary
    // case allocates nothing.
    let key = ext.trim_start_matches('.');
    let owned;
    let key = if key.bytes().any(|b| b.is_ascii_uppercase()) {
        owned = key.to_ascii_lowercase();
        owned.as_str()
    } else {
        key
    };
    let at = table.binary_search_by_key(&key, |(k, _)| *k).ok()?;
    Some(&table[at].1)
}

/// Every family, addressed by index from [\`SHELVES\`].
static FAMILIES: [Family; ${families.length}] = [
${families
  .map(
    (f) =>
      `    Family { label: ${rustStr(f.label)}, category: Category::${variant(f.category)} },`,
  )
  .join("\n")}
];

/// Extension to what the format is called. Sorted; see [\`lookup\`].
#[rustfmt::skip]
static NAMES: [(&str, &str); ${names.length}] = [
${table(names, rustStr)}
];

/// Extension to an index into [\`FAMILIES\`]. Sorted; see [\`lookup\`].
#[rustfmt::skip]
static SHELVES: [(&str, u8); ${shelves.length}] = [
${table(shelves, (v) => String(v))}
];

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole premise of the search. A table that came out of the generator
    /// out of order would find about half of what it was asked for, silently.
    #[test]
    fn the_tables_are_sorted_the_way_the_search_expects() {
        assert!(NAMES.windows(2).all(|p| p[0].0 < p[1].0), "NAMES is out of order");
        assert!(SHELVES.windows(2).all(|p| p[0].0 < p[1].0), "SHELVES is out of order");
    }

    /// Every index in the table addresses a family that exists.
    #[test]
    fn every_shelf_points_at_a_family() {
        for (ext, i) in SHELVES {
            assert!((i as usize) < FAMILIES.len(), "{ext} is filed on a shelf that is not there");
        }
    }

    #[test]
    fn a_format_nobody_here_can_open_is_still_named() {
        assert_eq!(name("sldprt"), Some("SolidWorks part"));
        assert_eq!(family("sldprt").map(|f| f.category), Some(Category::ThreeD));
    }

    /// A name off a disk arrives however the file manager spelled it.
    #[test]
    fn the_case_and_the_dot_are_both_forgiven() {
        assert_eq!(name("PNG"), name("png"));
        assert_eq!(name(".Png"), name("png"));
    }

    #[test]
    fn a_stranger_is_a_stranger_rather_than_a_guess() {
        assert_eq!(name("xyzzy"), None);
        assert_eq!(family("xyzzy"), None);
    }
}
`;

writeFileSync(out, rust);
console.log(
  `${out}: ${names.length} names, ${shelves.length} shelves, ${families.length} families`,
);
