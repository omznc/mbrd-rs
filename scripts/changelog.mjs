#!/usr/bin/env node
/**
 * Release notes for a tag, read out of the commits it contains.
 *
 * The release body is this, then `.github/install-notes.md` — the changes
 * first, because whoever is reading the page has usually already decided to
 * download and wants to know what they are getting; the standing paragraph
 * about Gatekeeper and SmartScreen after it, because a download page is the
 * only place anybody is told why their operating system is shouting at them.
 *
 * Usage:
 *   node scripts/changelog.mjs            # HEAD, against the last tag before it
 *   node scripts/changelog.mjs v0.6.0     # a specific tag
 */
import { execFileSync } from "node:child_process";

const git = (...args) =>
  execFileSync("git", args, { encoding: "utf8", maxBuffer: 32 * 1024 * 1024 });

/**
 * Conventional-commit type to heading, in the order the sections appear.
 *
 * The headings are words rather than the type names: "New" and "Fixed" are
 * what a release page is for, where "feat" and "fix" are what the commit
 * message needed to be machine-readable. Everything unrecognised — including a
 * subject with no type at all — falls into Other rather than being dropped,
 * because a commit silently missing from the notes is worse than one filed
 * badly.
 */
const SECTIONS = [
  { heading: "Breaking", types: [] /* filled by the `!` marker, not a type */ },
  { heading: "New", types: ["feat"] },
  { heading: "Fixed", types: ["fix"] },
  { heading: "Faster", types: ["perf"] },
  { heading: "Changed", types: ["refactor", "style"] },
  { heading: "Docs", types: ["docs"] },
  { heading: "Build", types: ["build", "ci", "chore", "test"] },
  { heading: "Other", types: [] },
];

/**
 * Commits that are about cutting the release rather than in it. The version
 * bump is the tag; saying so twice adds nothing.
 */
const NOT_A_CHANGE = /^(Release v?\d|chore\(release\)|Merge branch )/;

const SUBJECT = /^(\w+)(?:\(([^)]+)\))?(!)?:\s*(.+)$/;
const TRAILING_PR = /\s*\(#(\d+)\)\s*$/;
const MERGE_PR = /^Merge pull request #(\d+) from /;

function slug() {
  // Set for every workflow run; the remote is the fallback for a local run.
  if (process.env.GITHUB_REPOSITORY) return process.env.GITHUB_REPOSITORY;
  const url = git("remote", "get-url", "origin").trim();
  const match = /github\.com[:/](.+?)(?:\.git)?$/.exec(url);
  if (!match) throw new Error(`can't read a GitHub slug out of "${url}"`);
  return match[1];
}

/** The tag before `ref`, or null when `ref` is the first release. */
function previousTag(ref) {
  try {
    // stderr swallowed on purpose: the first release has no earlier tag, and
    // git says so on stderr, which would otherwise land in the release body.
    return execFileSync("git", ["describe", "--tags", "--abbrev=0", "--match", "v*", `${ref}^`], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
    }).trim();
  } catch {
    return null;
  }
}

/**
 * The commits in the range, one per change rather than one per commit object.
 *
 * `--first-parent` is what makes that true under either merge strategy. A
 * squash-merged PR is a single commit on master and shows up as itself; a
 * merge commit shows up once, with the branch's own commits — which are
 * working notes, not release notes — left on the side it came from. Without it
 * a PR of fifteen commits would list fifteen lines saying nothing.
 */
function commits(range) {
  const FIELD = "\x1f";
  const RECORD = "\x1e";
  const raw = git("log", "--first-parent", `--pretty=format:%H${FIELD}%s${FIELD}%b${RECORD}`, range);

  return raw
    .split(RECORD)
    .map((chunk) => chunk.trim())
    .filter(Boolean)
    .map((chunk) => {
      const [hash, subject, body = ""] = chunk.split(FIELD);
      return { hash, subject, body };
    });
}

function classify({ hash, subject, body }) {
  let title = subject;
  let pr = null;

  // A merge commit carries the PR number in its subject and the PR *title* in
  // the first line of its body, which is the only readable half of the two.
  const merge = MERGE_PR.exec(subject);
  if (merge) {
    pr = merge[1];
    title = body.split("\n").find((line) => line.trim()) ?? subject;
  }

  // A squash merge puts it the other way round: the PR title is the subject
  // and the number is stuck on the end of it.
  const trailing = TRAILING_PR.exec(title);
  if (trailing) {
    pr = pr ?? trailing[1];
    title = title.replace(TRAILING_PR, "");
  }

  const parsed = SUBJECT.exec(title);
  const type = parsed?.[1]?.toLowerCase() ?? null;
  const scope = parsed?.[2] ?? null;
  const breaking = Boolean(parsed?.[3]) || /^BREAKING[ -]CHANGE:/m.test(body);
  const text = parsed?.[4] ?? title;

  const heading = breaking
    ? "Breaking"
    : (SECTIONS.find((s) => s.types.includes(type))?.heading ?? "Other");

  return { hash, pr, heading, scope, text };
}

function render(entries, { repo, from, to }) {
  const lines = [];

  for (const { heading } of SECTIONS) {
    const group = entries.filter((e) => e.heading === heading);
    if (!group.length) continue;

    lines.push(`### ${heading}`, "");
    for (const { hash, pr, scope, text } of group) {
      // The PR where there is one — it holds the discussion and the diff, and
      // the commit holds only the diff. A repo that has never opened one just
      // never takes this branch.
      const link = pr
        ? `[#${pr}](https://github.com/${repo}/pull/${pr})`
        : `[\`${hash.slice(0, 7)}\`](https://github.com/${repo}/commit/${hash})`;
      lines.push(`- ${scope ? `**${scope}:** ` : ""}${text} ${link}`);
    }
    lines.push("");
  }

  if (!lines.length) lines.push("No commits since the last release.", "");

  if (from) {
    lines.push(
      `**Every change:** [\`${from}...${to}\`](https://github.com/${repo}/compare/${from}...${to})`,
    );
  }

  return lines.join("\n").trim();
}

const ref = process.argv[2] ?? "HEAD";
const from = previousTag(ref);
const repo = slug();

const entries = commits(from ? `${from}..${ref}` : ref)
  .filter((c) => !NOT_A_CHANGE.test(c.subject))
  .map(classify);

process.stdout.write(`${render(entries, { repo, from, to: ref })}\n`);
