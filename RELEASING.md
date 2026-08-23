# Releasing

Four executables, one per desktop platform, attached to a GitHub release.
There is no installer, no updater and no signing key — which makes this a
much shorter document than it would otherwise be, and pushes some cost onto
whoever downloads it. See **Code signing** at the bottom for what that costs.

## Cutting a release

The `version` under `[workspace.package]` in `Cargo.toml` is the source of
truth. Bump it, then tag to match:

```sh
# edit version in Cargo.toml, then:
git commit -am "Release v0.2.0"
git tag v0.2.0
git push origin master --tags
```

The tag and that version have to agree. They are checked against each other in
the `notes` job, which fails the run before anything builds rather than
publishing a release whose title and contents disagree — the mismatch is free
to make and invisible afterwards.

`.github/workflows/release.yml` fires on the tag and builds four executables in
parallel: macOS arm64, macOS x64, Linux x64, Windows x64. It opens the release
as a **draft** so you can look at it before anyone downloads — publish it in the
GitHub UI when all four jobs are green.

| Platform | Artifact | Built on |
|---|---|---|
| macOS Apple Silicon | `mbrd_0.2.0_aarch64.dmg` | `macos-latest` |
| macOS Intel | `mbrd_0.2.0_x64.dmg` | `macos-latest`, cross-compiled |
| Linux | `mbrd_0.2.0_x86_64-linux.tar.gz` | `ubuntu-22.04` |
| Windows | `mbrd_0.2.0_x64.exe` | `windows-latest` |

To prove the matrix still builds without cutting a release, run the workflow by
hand from the Actions tab. A manual run creates no release; it uploads the same
four artifacts to the run itself, which is how you check the app starts on a
platform you do not own.

### Where the release notes come from

The `notes` job builds the body once: `scripts/changelog.mjs` for the changes,
then `.github/install-notes.md` after it. Changes first, because whoever is
reading has already decided to download; the standing paragraph about
Gatekeeper and SmartScreen after it, because a download page is the only place
anybody is told why their operating system is shouting at them.

`changelog.mjs` reads the commits between the previous tag and this one and
groups them by conventional-commit type — `feat` under **New**, `fix` under
**Fixed**, and so on, with anything unrecognised falling into **Other** rather
than being dropped, because a commit missing from the notes is worse than one
filed badly. It walks with `--first-parent`, so a merged branch appears once as
itself rather than as fifteen lines of working notes.

It runs before the builds and in its own job, so five jobs cannot disagree
about what the release says. It is also what creates the draft: one job makes
it, four upload into it, and none of them has to race the others for the right
to open it.

## Why Windows has to be built on Windows

gpui compiles its HLSL shaders with `fxc.exe` from the Windows SDK, and its
build script gates that step on `#[cfg(target_os = "windows")]` — the **host**,
not the target. Cross-compiling from Linux therefore skips shader compilation
without complaint and then fails on the bindings that step was supposed to
write. There is no flag for it and no reasonable way around it; the runner is
the Windows machine.

The workflow does not trust gpui to find `fxc.exe` on its own. It looks on
`PATH` first and then at one hardcoded SDK version, which is a path that has
already moved once; a step walks the SDK's `bin` directory for the newest x64
copy and exports `GPUI_FXC_PATH`, which the build script prefers over both.

The same is true in the other direction and for the same reason: the macOS
build shells out to `xcrun metal`, so it needs a Mac. Only the two macOS
targets share a runner, and they can because Metal shaders compile to
architecture-independent AIR — the Intel build is a genuine cross-compile that
costs nothing extra.

## Why the macOS artifact is a bundle and not a binary

`cargo build` produces a Mach-O that runs perfectly well from a terminal and is
useless as an application: no name in the menu bar, no Dock identity, and no
way to double-click it out of Applications. The workflow assembles a `.app`
around it from `packaging/macos/Info.plist`, substituting the version, and puts
that in a `.dmg` next to a symlink to `/Applications` so the window that opens
is a drag-to-install rather than a folder with a mystery in it.

The `codesign --sign -` on the bundle is not security. An ad-hoc signature is
what makes an arm64 binary runnable at all — macOS kills unsigned ones on
launch — and wrapping a signed binary in a bundle is a new identity that has to
be signed again.

## Why the Windows exe is statically linked

`RUSTFLAGS: -C target-feature=+crt-static` links the Visual C++ runtime into
the executable. Without it the file depends on a redistributable that a machine
may or may not have, and the failure on a machine that does not is a dialog
about a missing DLL — which is a poor first impression from something being
handed over as one file. It costs a little size, and the whole point of the
artifact is that it is one file.

If the link ever breaks on it, that is the one line to remove; the exe then
needs the Visual C++ 2015-2022 redistributable, which is worth saying in
`install-notes.md` on the way past.

## Why Linux is pinned to 22.04

The binary links against whatever glibc the runner has, and glibc is only
forward-compatible: a binary built against 2.39 will not start on a system with
2.35, while the reverse is fine. Building on the newest Ubuntu would produce
something that refuses to run on a distribution two years old. 22.04 puts the
floor at glibc 2.35 — Ubuntu 22.04, Fedora 36 — which is old enough.

It is a `.tar.gz` with a binary in it rather than an AppImage, a `.deb` and an
`.rpm`, because there is nothing to install: no desktop entry, no icon, no
shared data. When there is, this is the place that changes.

## Building locally

```sh
cargo build --release -p mbrd
```

`[profile.release]` in `Cargo.toml` is what the shipped builds use: fat LTO,
one codegen unit, stripped, `panic = "abort"`. The abort is safe here — the
only `catch_unwind` anywhere in gpui is in its test support — and it drops the
unwinding tables, which on a binary that is mostly gpui is worth having.

Expect tens of megabytes whatever you do. gpui statically links a renderer, a
font stack and an SVG renderer, and there is no honest way to make that small.

## Code signing — what "unsigned" actually costs

Nothing here is signed with an OS vendor certificate.

On **macOS** that is the expensive one. A downloaded `.dmg` carries a quarantine
attribute, and an unsigned, unnotarised app under quarantine is not merely
warned about — recent versions refuse to open it at all, and the old
right-click → Open trick no longer helps. The only way in is `xattr -dr
com.apple.quarantine`, which `install-notes.md` spells out. An Apple Developer
account and notarisation would remove that; nothing else will.

On **Windows** it is a warning rather than a wall: SmartScreen calls it an
unrecognised app and **More info → Run anyway** gets past it. Reputation
accrues to a signing certificate over downloads, so a cheap certificate does not
fix it immediately either.

On **Linux** nobody asks.

## What isn't automated

- **Version bumping.** Deliberately manual — one number, and it has to agree
  with the tag, which CI checks.
- **An icon.** There is none, on any platform. The macOS bundle has no
  `CFBundleIconFile` and the Windows exe has no embedded resource, so both get
  the system default. An `.icns` and an `.ico` and two lines each would fix it.
- **Opening a `.mbrd` from the Finder.** Deliberately not declared. macOS hands
  a double-clicked document to an app as an Apple Event, and `main.rs` reads
  `argv`; declaring the type would make the Finder offer mbrd for a board and
  then have mbrd open the demonstration board instead, which is worse than not
  being offered. gpui exposes the hook — it wants `on_open_urls` — and when
  that is wired up, `CFBundleDocumentTypes` goes into `Info.plist`.
- **arm64 Linux, and arm64 Windows.** No runner reason not to; nobody has
  asked.
