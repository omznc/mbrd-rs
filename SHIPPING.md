# Shipping

The plan for standalone builds, installers, and an updater that runs inside the
app. `RELEASING.md` describes how a release is cut *today* — four bare
executables, no installer, no updater, no signing key. This describes what it
should become, and in what order, because most of it depends on the rest of it.

**Status: built, except where marked.** The operational half has moved into
`RELEASING.md`, which is the document to read when actually cutting a release.
What is left here is the *reasoning* — why the pieces are shaped the way they
are — which is the half that does not belong in a procedure.

| slice | | |
|---|---|---|
| 0 | Icons | **done** — `packaging/icon.svg`, `scripts/icons.sh` |
| 1 | Platform directories | **done** — `crates/app/src/dirs.rs` |
| 2 | Desktop integration | **done** — `.desktop`, MIME, `CFBundleDocumentTypes`, NSIS `HKCU` |
| 3 | The artifact matrix | **done** — eleven artifacts, plus a `package` job on every PR |
| 4 | The signed manifest | **done** — `update/manifest.rs`, signed in the `manifest` job |
| 5 | The check | **done** — `Ctrl`+`U`, launch check, status-bar reporting |
| 6 | Download and swap | **done** — `update/install.rs`, `update/eligible.rs` |
| 7 | Documentation | **done** |

The signing key is generated and installed: public half as the repository
variable `MBRD_UPDATE_PUBLIC_KEY`, private half as `MBRD_UPDATE_SECRET_KEY` on
the `release` environment, and a backup under `~/.config/mbrd-release-key/`
that wants moving into a password manager and then deleting.

Not done, and deliberately: **Flatpak** (the sandbox changes both argv handling
and the XDG directories), **arm64 Linux and Windows** (nobody has asked; the
manifest is indexed by triple so it is a data change), and **an end-to-end
install test** — which cannot be automated here and has to be run by hand on
each platform before the first release that carries the updater.

---

## What is settled

| question | answer |
| --- | --- |
| OS vendor code signing | **No.** Unsigned stays unsigned. Trust for updates comes from our own key, not Apple's or Microsoft's. |
| How far the updater goes | **Full self-replacement.** Check, download, verify, swap, relaunch. |
| Linux artifacts | **AppImage, `.deb`, `.rpm`**, alongside the tarball that already exists. |
| Windows artifacts | **Portable `.exe`** (unchanged) **plus an NSIS per-user installer.** |

Two of those interact, and it is worth saying out loud before anything else:
**unsigned plus self-replacing means the updater's own signature is the entire
security boundary.** There is no OS check behind it to catch a mistake. A bug
in the verification path is remote code execution on every machine that has the
app. That is why the manifest work (slice 4) comes before the download work
(slice 6), and why the swap refuses more often than it agrees.

---

## What "standalone" already means, per platform

The phrase means different things in three places, and only one of them is
actually missing.

- **Windows** — already standalone. `-C target-feature=+crt-static` links the
  Visual C++ runtime in, so the `.exe` runs on a machine that has never had a
  redistributable.
- **macOS** — already standalone. The `.app` carries everything, and gpui
  statically links the renderer and the font stack.
- **Linux** — **not** standalone, and this is the real gap. The tarball's binary
  needs a glibc no older than 22.04's, `libxkbcommon-x11`, fontconfig, freetype
  and a Vulkan driver, and `install-notes.md` currently hands somebody a `dnf`
  command and hopes. The AppImage is the answer to that: one file, its own
  copies of the shared libraries, and nothing to install.

So "standalone builds" is mostly a Linux job, and "installers" is mostly a
Windows and Linux job.

---

## The artifact matrix

| platform | today | planned |
| --- | --- | --- |
| macOS arm64 | `.dmg` | `.dmg` + `.app.tar.gz` |
| macOS x64 | `.dmg` | `.dmg` + `.app.tar.gz` |
| Windows x64 | `.exe` | `.exe` + `-setup.exe` |
| Linux x64 | `.tar.gz` | `.tar.gz` + `.AppImage` + `.deb` + `.rpm` |
| — | — | `latest.json` + `latest.json.minisig` |

The second macOS artifact exists for the updater alone. A `.dmg` has to be
attached with `hdiutil` before anything can be read out of it, which means
shelling out to a tool and mounting a filesystem in order to move a directory
one level up — where a tarball of the same `.app` is two crates and no
subprocess. Humans keep getting the `.dmg`, which is the thing a Mac user
expects to be handed.

---

## The slices, in dependency order

### 0. Icons and identity — done

Nothing else can be built without this. An NSIS installer with the default icon
looks like malware; a `.desktop` file with no icon gets a grey square in the
launcher; the macOS bundle has no `CFBundleIconFile` today and gets the generic
application icon in the Dock.

- `packaging/icon.svg` as the single source, and `scripts/icons.sh` rendering
  everything else from it: `icon.icns` (via `iconutil`), `icon.ico` (via
  ImageMagick), and `hicolor` PNGs at 16/24/32/48/64/128/256/512.
- Rendered artefacts are **committed**, not generated in CI. `iconutil` needs a
  Mac and the Linux and Windows jobs need the results; generating them on a
  runner that has to be macOS in order to produce a file every other job
  depends on is a dependency between jobs that buys nothing.
- Wiring: `CFBundleIconFile` in `Info.plist`; a `build.rs` on the app crate
  using `embed-resource` for the Windows executable; `hicolor` for Linux.

The mark is three tilted cards joined by ropes, in the app's own palette —
every colour in the SVG is lifted from `theme.rs` rather than invented, so the
icon and the thing it opens agree. It is built to survive 16 pixels: three
large shapes, clear gaps, one accent colour, and interior detail deliberately
low-contrast so it blurs into the tint rather than into mud.

`scripts/icons.sh` renders the PNGs, the `.ico` and the `hicolor` tree, and
packs the `.icns` by hand rather than shelling out to `iconutil` — the ICNS
container is a length-prefixed list of PNGs, and thirty lines of Python is a
better trade than a build step only a Mac can run.

### 1. Platform directories — `crates/app/src/dirs.rs` — done

`prefs.rs` and `recent.rs` each compute their own path from `XDG_CONFIG_HOME` /
`XDG_STATE_HOME` with a `$HOME` fallback. On Windows neither variable is set and
`HOME` is usually not either, so `store()` returns `None`, every read is the
default and every write is discarded — silently, by design, because both modules
are deliberately best-effort. That is the right behaviour for a missing file and
the wrong behaviour for a platform.

One module, three functions — `config()`, `state()`, `cache()` — resolving:

| | Linux | macOS | Windows |
| --- | --- | --- | --- |
| config | `$XDG_CONFIG_HOME/mbrd` | `~/Library/Application Support/mbrd` | `%APPDATA%\mbrd` |
| state | `$XDG_STATE_HOME/mbrd` | `~/Library/Application Support/mbrd` | `%LOCALAPPDATA%\mbrd` |
| cache | `$XDG_CACHE_HOME/mbrd` | `~/Library/Caches/mbrd` | `%LOCALAPPDATA%\mbrd\cache` |

macOS folds config and state into one directory because it has no separate
notion of them; the *distinction* stays in the module names, so the comment in
`prefs.rs` about backups carrying settings and not history stays true where the
platform can express it and is honestly unenforceable where it cannot.

`prefs.rs` and `recent.rs` move onto it. The updater needs the same three
directories anyway — cache for the download, state for the last-checked stamp —
so this is not overhead invented for Windows' sake.

No `dirs` crate. It is forty lines of `env::var_os` and this workspace has eight
direct dependencies.

### 2. Desktop integration — done

- **Linux.** A `mbrd.desktop` and a `mbrd.xml` shared-mime-info declaring
  `application/x-mbrd` for `*.mbrd`, installed by the `.deb` and `.rpm` and
  bundled into the AppImage. This is what makes a board double-clickable and
  what puts the app in a launcher.
- **Windows.** The NSIS installer registers `.mbrd` under `HKCU` — per-user, so
  no elevation. The portable `.exe` registers nothing, which is the point of it.
- **macOS.** `CFBundleDocumentTypes` in `Info.plist`, which `RELEASING.md`
  deliberately leaves out today and gives the correct reason for: the Finder
  hands a double-clicked document over as an Apple Event and `main.rs` reads
  `argv`, so declaring the type would make the Finder offer mbrd for a board and
  then have mbrd open the demonstration board. gpui's `on_open_urls`
  (`app.rs:188`) is the hook. **Wire the hook first, declare the type second** —
  in that order, in one commit, or the intermediate state is the broken one the
  existing comment warns about.

This slice is genuinely optional and can be dropped without affecting anything
below it. It is here because installers are the moment it becomes cheap: the
`.deb`, the `.rpm` and the NSIS script all need a file list anyway.

### 3. The artifact matrix in CI — done

`release.yml` grows packaging steps. The build matrix does not change — same
four targets, same runners, same reasons (`fxc.exe` needs Windows, `xcrun metal`
needs a Mac, glibc pins Linux to 22.04).

- **macOS**: after the existing `.app` assembly and ad-hoc `codesign`, also
  `tar -czf` the bundle. Order matters — tar the *signed* bundle, or the
  updater installs one that macOS kills on launch.
- **Windows**: `makensis` against `packaging/windows/mbrd.nsi`. Per-user install
  to `%LOCALAPPDATA%\Programs\mbrd`, which means no UAC prompt *and* means the
  installed executable is writable by the app that is running it — the updater
  works out of the box in the installed location, which a `Program Files`
  install would not.
- **Linux**: `cargo-deb` and `cargo-generate-rpm` for the packages;
  `linuxdeploy` plus its GTK-less plugin set for the AppImage. Both packages set
  a marker (see slice 6) so the app knows it is distro-owned.

A `package` job is added to `ci.yml` so packaging breakage is caught on a pull
request rather than on a tag. It builds the installers and throws them away;
`ci.yml`'s comment about being deliberately Linux-only gets amended, because
this part genuinely cannot be.

### 4. The manifest and the key — done

The client needs one URL that always answers with the truth about the newest
release. GitHub gives that for free:

```
https://github.com/omznc/mbrd-rs/releases/latest/download/latest.json
```

`latest.json`:

```json
{
  "version": "0.3.0",
  "notes": "https://github.com/omznc/mbrd-rs/releases/tag/v0.3.0",
  "targets": {
    "aarch64-apple-darwin":     { "url": "…app.tar.gz", "size": 41234567, "sha256": "…" },
    "x86_64-apple-darwin":      { "url": "…app.tar.gz", "size": 42345678, "sha256": "…" },
    "x86_64-pc-windows-msvc":   { "url": "….exe",       "size": 39876543, "sha256": "…" },
    "x86_64-unknown-linux-gnu": { "url": "….tar.gz",    "size": 40123456, "sha256": "…" }
  }
}
```

The **whole manifest** is signed with ed25519, detached, minisign format; the
public key is a `const` in the binary. One signature gates everything, and the
per-target `sha256` inside it is what the downloaded bytes are measured against.
That is the shape Sparkle and Tauri both settled on and it is the right one: the
expensive asymmetric check happens once on two kilobytes, and the thirty-megabyte
download is checked with a hash that the signature has already vouched for.

- **Verification only in the app.** `minisign-verify` — no signing code, no
  private key handling, a few hundred lines.
- The private key lives in a GitHub Actions environment secret. A separate
  `manifest` job signs, because it has to run *after* the builds — it hashes
  what they published.
- `size` is in the manifest so a download can be refused before it starts rather
  than after it has filled somebody's disk.

**On losing the key.** There is no graceful answer and pretending otherwise is
worse than saying so. Clients trust exactly the key compiled into them; a new
key is untrusted by every install that predates it, and those installs are
stranded on notify-only until somebody reinstalls by hand. Rotation is not a
feature that can be added later — either the key survives or people reinstall.
`RELEASING.md` says this in the same words, next to where the key is described.

### 5. The check — `crates/app/src/update/` — done

- **When.** Once on launch, a few seconds after the window is up so it never
  competes with the first paint, and on demand from a new
  `Command::CheckForUpdates`. `Ctrl`+`U` is free, and nothing claims it.
- **How often.** At most once a day, stamped in the state directory. **Never on
  the first run**: an app whose first act is to phone home is a poor first
  impression, and it has nothing to report anyway.
- **Opting out.** `"update": false` in `settings.json`, and `MBRD_NO_UPDATE=1`
  in the environment — the same two-way arrangement `prefs.rs` already gives
  `motion`, and for the same reason: a setting somebody needs before they will
  run the app should not require running the app.
- **Saying so.** Through the existing status bar. `say` for "0.3.0 is out —
  `Ctrl`+`U` to install", `warn` for a check that failed. No new UI surface, no
  dialogue, no badge. A moodboard interrupting somebody to talk about itself is
  the thing to avoid, and `board_view.rs` already has exactly the right
  mechanism: a line that says something and then stops saying it.
- **A failed check is not an error.** No network, a proxy, GitHub down — all
  ordinary. Logged, not warned about, unless the check was asked for by hand, in
  which case silence would be the confusing answer.

**HTTP.** `ureq` with rustls. The lockfile already contains a full reqwest and
tokio through `gpui_http_client`, but gpui's default client is `NullHttpClient`
(`app.rs:2343`) — the stack is linked and unreachable, and reaching it means
standing up a tokio runtime on a thread beside gpui's smol executor, which is
what Zed does and is more machinery than two GET requests deserve. `ureq` is
blocking, shares the `rustls 0.23` already in the tree, and runs on
`cx.background_executor().spawn` exactly like the image decode in
`board_view.rs:3635` does.

It is no longer only the update path. `fetch.rs` uses the same agent, the same
executor and the same reasoning to follow a pasted address that points at a
file — but with its own bounds, because the update path follows one URL this
project signed and that one follows whatever is on somebody's clipboard. The
difference between the two is written down there rather than here.

### 6. The download and the swap — `update/install.rs` — done

Download to the cache directory, streamed, with the manifest's `size` as a
ceiling. Verify the sha256. Only then touch anything the app runs from.

**Staging must be on the target's own filesystem.** The final move is a
`rename(2)`, which is atomic and cannot cross devices; staging in `/tmp` and
renaming into `/usr/local/bin` fails on most machines. Stage in a temporary
directory beside the target, move, then clean up.

Per platform:

- **Linux, tarball or AppImage.** Extract beside the target, `rename` onto it.
  Under an AppImage the target is `$APPIMAGE`, which is the file the app owns;
  the mount point it is running from is read-only and is not it.
- **Windows.** The running `.exe` cannot be overwritten but *can* be renamed.
  `mbrd.exe` → `mbrd.exe.old`, new file into place, restart, sweep the `.old` on
  next launch. The portable executable and the NSIS-installed one are the same
  file in different places, so one path covers both.
- **macOS.** A bundle swap, not a binary swap: replacing
  `mbrd.app/Contents/MacOS/mbrd` in place breaks the ad-hoc signature that is
  the only reason the app runs on Apple Silicon at all. Stage the new `.app`
  in the same parent directory, `rename` the old to `mbrd.app.old`, `rename` the
  new into place, restart, sweep on next launch.

**Relaunching** is `cx.set_restart_path` and `cx.restart()`. gpui already writes
the shim for all three platforms (`app.rs:1153`) — a detached helper that polls
until our process exits and then reopens the app, `open` on macOS,
`Start-Process` on Windows, `exec` on Linux. Nothing here needs writing.

**It must not restart over unsaved work.** The swap is gated on a clean ledger.
If the board is dirty, the offer becomes "save and install", and declining
leaves the downloaded update staged for next launch rather than throwing it away.

#### When it refuses — `update/eligible.rs`

Pure, no I/O, therefore tested. It refuses when:

- the target is not writable by this process, or sits under `/usr`
- the build carries the packaged marker the `.deb` and `.rpm` set
- `FLATPAK_ID` or `SNAP` is in the environment
- this is a debug build
- the manifest has no entry for this target triple
- the offered version is not strictly newer

A refusal is not silence. The check still runs and still reports, it just
reports the honest thing — *"0.3.0 is out; install it with `dnf upgrade
mbrd`"* — because a package a distribution owns is a package the distribution
should replace. This is also where notify-only lives, so the notify-only
behaviour is a real code path with real users rather than a fallback nobody
exercises.

#### The one good thing about being unsigned

`install-notes.md` currently tells macOS users to run `xattr -dr
com.apple.quarantine`. That attribute is set by the program that *downloaded*
the file — browsers opt into it — and is not set on a file written by our own
process. So an app that updates itself never acquires it. **The quarantine
incantation becomes a one-time cost at first install rather than a cost per
release**, which is worth saying in `install-notes.md`, because it is the
strongest argument for the updater in a world with no Developer ID.

### 7. Documentation — done

- `RELEASING.md`: the four-artifact table becomes eleven; the signing key,
  where it lives, and the paragraph about losing it; the AppImage and package
  build steps; "What isn't automated" loses the icon entry and the `.mbrd`
  association entry and gains key rotation.
- `.github/install-notes.md`: the new artifacts, which one to pick, and the
  quarantine-is-once-now note.
- `README.md`: the Linux dependency list moves to a footnote, because the
  AppImage is the recommendation and it needs none of them.
- `ROADMAP.md`: one line pointing here, under `Done` once it is.

---

## Dependencies added

| crate | why | roughly |
| --- | --- | --- |
| `ureq` (rustls) | the update check, and following a pasted link | ~300 KB, shares the existing `rustls 0.23` |
| `minisign-verify` | verify the manifest | ~20 KB, verify-only |
| `tar` + `flate2` | the macOS and Linux payloads | ~150 KB |
| `embed-resource` (build) | the Windows icon | build-time only |

Version comparison is hand-rolled — three numbers and a `<`, about twenty lines
with tests, against a `semver` dependency that would be the workspace's ninth
for a job the workspace does once. On a binary that is already tens of megabytes
of statically linked renderer, the total is noise.

No zip handling is added: the Windows payload is a bare `.exe` with no archive
around it, so the `zip` crate stays where it is, reading `.mbrd` files in
`mbrd-core` and nothing else.

## Testing

Pure and unit-tested: eligibility, version comparison, manifest parsing,
signature verification against a fixture key, and the target-triple lookup.

The swap is tested against dummy files in a temporary directory — the rename
dance on each platform, including the Windows `.old` sweep and the macOS bundle
move — without involving a real application or a real network.

End to end is manual and stays manual. `workflow_dispatch` already builds the
matrix without publishing; installing the previous release and pointing
`MBRD_UPDATE_URL` at a test manifest is the check, and it has to be run on each
of the three platforms before the first release that carries the updater.

## Order, and what can be dropped

**0 → 1 → 3 → 4 → 5 → 6** is the spine. Slice 2 (desktop integration) and the
`.deb`/`.rpm` half of slice 3 are genuinely optional and can be cut without
touching anything else; the AppImage is not, because it is the answer to
"standalone" on the one platform where the app is not.

The two slices that are cheap and immediately useful on their own are **1**,
which fixes settings silently not working on Windows today, and **5**, which is
a day's work and gets people onto new versions even before anything can install
itself.

## Still open

- **The icon.** Slice 0 is blocked on a design. Everything after it is not, but
  the installers should not ship without one.
- **arm64 Linux and arm64 Windows.** Still no runner reason not to, still
  nobody asking. The manifest's target map makes adding one a data change.
- **Where the update is announced.** GitHub releases are the source of truth
  here, which means the check is a request to `github.com` and anybody counting
  those can count installs. Self-hosting the manifest moves that count rather
  than removing it.
