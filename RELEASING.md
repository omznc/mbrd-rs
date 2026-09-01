# Releasing

Eleven artifacts attached to a GitHub release, plus a signed manifest the app
reads to update itself.

Nothing is signed with an Apple or Microsoft certificate — see **Code signing**
at the bottom for what that costs — so **mbrd's own signing key is the entire
trust boundary for updates**. That is the one part of this document worth
reading before touching anything else: see **The signing key**.

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

| Platform | Artifact | For | Built on |
|---|---|---|---|
| macOS arm64 | `mbrd_0.2.0_aarch64.dmg` | people | `macos-latest` |
| macOS arm64 | `mbrd_0.2.0_aarch64.app.tar.gz` | the updater | `macos-latest` |
| macOS Intel | `mbrd_0.2.0_x64.dmg` | people | `macos-latest`, cross-compiled |
| macOS Intel | `mbrd_0.2.0_x64.app.tar.gz` | the updater | `macos-latest`, cross-compiled |
| Windows | `mbrd_0.2.0_x64-setup.exe` | people | `windows-latest` |
| Windows | `mbrd_0.2.0_x64.exe` | people, and the updater | `windows-latest` |
| Linux | `mbrd_0.2.0_x86_64.AppImage` | people | `ubuntu-22.04` |
| Linux | `mbrd_0.2.0_amd64.deb` | people | `ubuntu-22.04` |
| Linux | `mbrd-0.2.0-1.x86_64.rpm` | people | `ubuntu-22.04` |
| Linux | `mbrd_0.2.0_x86_64-linux.tar.gz` | people, and the updater | `ubuntu-22.04` |
| — | `latest.json` + `.minisig` | the updater | `ubuntu-24.04` |

The `manifest` job runs last and hashes what the build jobs published. It fails
the run if any expected artifact is missing, rather than publishing a manifest
that describes three platforms out of four.

To prove the matrix still builds without cutting a release, run the workflow by
hand from the Actions tab. A manual run creates no release and signs nothing; it
uploads the artifacts to the run itself, which is how you check the app starts on
a platform you do not own.

Packaging breakage is caught earlier than that. `ci.yml` has a `package` job on
every pull request that assembles the macOS bundle, validates the desktop entry
and the MIME definition, and compiles the NSIS script with `/WX` — on the three
platforms, in debug. It used to be that a typo in any of those shell steps was
found by pushing a tag and watching the release fail, at which point the tag has
to be moved.

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

## The signing key

Updates are verified against an ed25519 key in minisign format. The **public**
half is compiled into every released binary; the **private** half signs
`latest.json` in the `manifest` job.

| | where | what it is |
|---|---|---|
| public | repository **variable** `MBRD_UPDATE_PUBLIC_KEY` | compiled in via `MBRD_UPDATE_KEY` |
| private | **secret** `MBRD_UPDATE_SECRET_KEY` on the `release` environment | signs `latest.json` |

Both are set. The public key is
`RWR0avteXBtqMMDhN9opkNwXRC2bKN1t3TFlRZBouOFHjuICcMCCAT1R`.

A variable and not a secret for the public half, deliberately: hiding a public
key only hides it from the people who might want to check a release against it.

The key is **passwordless**, and that is a considered choice rather than a
shortcut. In CI it is protected by being a GitHub secret; a password held as a
*second* GitHub secret would protect it from nothing the first is not already
exposed to. The backup copy is where encryption belongs, and a password manager
provides it.

The `manifest` job verifies its own signature against the public variable
immediately after making it, because a public and private half that have
drifted apart produce a release that every install silently refuses — and
silence is the only symptom.

### Why `rsign2` and not the `minisign` package

The key was generated with `rsign`, and `rsign` is what is known to read it
back. The two implementations agree on the *signature* format — that is the
part the app checks, and `manifest.rs` tests it against a real signature — but
whether one can load the other's passwordless secret key is an assumption
nobody has tested. On the one path whose failure would break the trust root,
the workflow uses the tool that made the key.

To regenerate a pair from scratch:

```sh
cargo install rsign2
rsign generate -W -s mbrd.key -p mbrd.pub -c "mbrd update signing key"
```

A build with no `MBRD_UPDATE_KEY` cannot install anything at all. That is the
default, and it is the correct behaviour for anybody building this themselves:
they are not publishing a signed manifest, so an app of theirs that installed one
would be trusting a stranger's.

### If the key is lost

There is no graceful answer, and pretending otherwise would be worse than saying
so plainly.

Every installed copy trusts exactly the key it was compiled with. A new key is
untrusted by every install that predates it, so those installs stop being offered
updates and stay where they are until somebody reinstalls by hand. **Key rotation
is not a feature that can be added later** — the old binaries are already out
there and they will not learn a second key.

So: the private key survives, or people reinstall. It lives on the `release`
environment for that reason — worth adding required reviewers to it — and there
must be a copy somewhere that is not GitHub.

## What the updater does, and where it refuses

`crates/app/src/update/`. The parts worth
knowing when cutting a release:

- The client asks one fixed URL, `releases/latest/download/latest.json`, which
  GitHub always resolves to the newest published release. **A draft release is
  not published**, so nothing is offered until the release is published in the
  UI — which makes publishing the moment the update goes out, not tagging.
- One signature over the whole manifest; a SHA-256 per artifact inside it. The
  signature is checked before the JSON is parsed.
- The manifest is indexed by target triple — and by triple plus format for the
  two Linux packages, so `x86_64-unknown-linux-gnu` has a `.deb` and a `.rpm`
  key beside it — and the keys in the workflow have to match the ones the
  binaries build for themselves. A
  test in `manifest.rs` parses a verbatim copy of the workflow's output for
  exactly this reason: the two are connected by nothing but a `printf`, and if
  they drift the symptom is that nobody is ever offered anything.
- A `.deb` or `.rpm` install is **updated, not refused** — see below.
- It still refuses anything under `/usr` that no package of ours owns, a
  Flatpak or Snap, a target it cannot write, and a packaged install on a machine
  with no `pkexec` to ask with. Those cases are not silent — the app still says
  a new version exists and says where to get it.

To test an update end to end without publishing: install the previous release,
then point the app at a manifest you control. There is no substitute for doing
this on each platform before the first release that carries the updater.

### How a `.deb` or `.rpm` updates itself

`crates/app/src/update/package.rs`. This one is worth writing down because it is
the only install shape where the app does *not* put the new version in place
itself, and the reason is not squeamishness: `dpkg` records a hash for
`/usr/bin/mbrd`, and a `rename` over that file leaves the package database
describing something that is no longer there. It would work once and be wrong
for ever after. So the update for those installs is the package, downloaded
through the same signed manifest and verified against the same SHA-256 as
everything else, and then handed to the tool that owns the file.

Three questions, answered in this order:

- **Which package is this?** One build becomes both, so `MBRD_PACKAGED` cannot
  say. The runtime signals are `/var/lib/dpkg/info/mbrd.list` first — which is
  exactly this package — then an rpm database, then dpkg's status file. No
  subprocess: `dpkg -S` walks every file list on the machine and this question
  is asked during launch. Only for a target that *is* `/usr/bin/mbrd`, which is
  where both of our packages put the binary; anywhere else, installing the
  package would update a file the running app is not, and the restart would come
  back up on the old one having reported success.
- **How to ask for the permission?** `pkexec`, or the path is off and the old
  "update it through your package manager" sentence comes back — before the
  download rather than after it. Not `sudo`, which wants a terminal the app does
  not have, and certainly not a password box of our own.
- **What to run?** `apt-get install -y <file>` or `dnf install -y <file>`, with
  `dpkg -i`, `zypper` and `rpm -U` behind them. The dependency-resolving tool
  first: a release that needs a library the last one did not is precisely what
  `dpkg -i` installs broken and `rpm -U` refuses outright.

The download is staged in the *cache* directory rather than beside the target —
nothing is renamed on this path, and `/usr/bin` is not ours to write in — and a
dismissed password prompt keeps it, so pressing the button again costs nothing.
The install runs on the background executor, because it blocks for as long as
somebody takes to answer a prompt.

This is also why the packaging step sets `MBRD_UPDATE_KEY` as well as
`MBRD_PACKAGED`: a build with no key never asks for a manifest at all, so
without it the whole path would be dead in the one build shape it exists for.
**Packages published before this existed carry no key**, so nobody already on
one can be offered the version that fixes it — that is a one-time reinstall by
hand, and worth a line in the release notes when it ships.

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

There is now a desktop entry, an icon and a MIME type, so the tarball is no
longer the only Linux artifact — it is joined by an AppImage, a `.deb` and an
`.rpm`. The AppImage is the one to recommend: it carries its own copies of the
shared libraries the bare binary expects to find, which is what makes it the
only genuinely standalone Linux build. Windows has been standalone since
`crt-static` and macOS since the bundle; Linux was the gap.

The `.deb` and `.rpm` set `MBRD_PACKAGED` at build time, which is what stops the
in-app updater writing over a file `dpkg` or `rpm` believes it owns. There is a
`/usr` prefix check behind that as a safety net, but the marker is the exact
answer.

## Why only the Linux release installs anything for media

All three platforms play sound and video, each through its own backend and its
own system decoder — `pipeline.rs` against GStreamer, `pipeline_mac.rs` against
AVFoundation, `pipeline_win.rs` against the Media Foundation Media Engine. The
deps sit under three `cfg(target_os = ...)` tables in `crates/app/Cargo.toml`,
so a build only ever pulls one set.

The difference at release time is that GStreamer is a *link-time* dependency
and not an optional one — a binary built against it will not start on a machine
without it — while the other two are part of the OS and always there. So Linux
is the only platform where shipping media is a packaging job at all.

What that means for a release:

- The Linux runner installs `libgstreamer1.0-dev` and
  `libgstreamer-plugins-base1.0-dev` to build, and the `-plugins-good` and
  `-libav` runtime sets for the AppImage to copy. No other runner installs
  anything, and no other runner needs to.
- The AppImage runs `linuxdeploy-plugin-gstreamer`, which is what carries the
  plugins in. `ldd` finds the libraries by itself; the plugins are `dlopen`ed
  and it cannot. An AppImage built without it starts and then has no decoder
  for anything, which is a worse failure than not shipping media at all.
- The `.deb` requires the two libraries and *recommends* the codecs; the `.rpm`
  requires the libraries. A machine with the libraries and no codecs runs the
  app and opens every board — only the play button says it cannot.
- The Windows `.exe` and the macOS `.app` install nothing, bundle nothing and
  are still one file and one bundle. That is the whole reason those two
  backends are native rather than a fourth copy of GStreamer; **Why three
  media backends** below has what the other choice would have cost.
- Neither of those two can be built or type-checked on the Linux runner that
  builds them, so a media change is checked by the platform's own job or not at
  all. A tag is a bad place to find that out — see `CONTRIBUTING.md`.

## Building locally

```sh
cargo build --release -p mbrd
```

`[profile.release]` in `Cargo.toml` is what the shipped builds use: fat LTO,
one codegen unit, stripped, `panic = "abort"`. The abort is safe here — the
only `catch_unwind` anywhere in gpui is in its test support — and it drops the
unwinding tables, which on a binary that is mostly gpui is worth having.

A local build has no `MBRD_UPDATE_KEY`, so it cannot install an update and does
not try. That is deliberate; see **The signing key**. `Ctrl`+`U` in such a build
says so rather than doing nothing.

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

The updater takes most of the sting out of it, though, and this is worth
understanding because it is the strongest argument for having built one. The
quarantine attribute is applied by the program that *downloaded* the file —
browsers opt into it — and is not applied to a file written by our own process.
So an app that replaces itself never acquires it. **The `xattr` incantation is a
one-time cost at first install rather than a cost per release.**

On **Windows** it is a warning rather than a wall: SmartScreen calls it an
unrecognised app and **More info → Run anyway** gets past it. Reputation
accrues to a signing certificate over downloads, so a cheap certificate does not
fix it immediately either.

On **Linux** nobody asks.

## What is settled, and why

| question | answer |
| --- | --- |
| OS vendor code signing | **No.** Unsigned stays unsigned. Trust for updates comes from our own key, not Apple's or Microsoft's. |
| How far the updater goes | **Full self-replacement.** Check, download, verify, swap, relaunch — and on a `.deb` or `.rpm`, hand the package to its own package manager instead of swapping. |
| Linux artifacts | **AppImage, `.deb`, `.rpm`**, alongside the tarball. |
| Windows artifacts | **Portable `.exe`** plus an NSIS per-user installer. |

The first two interact, which is the point of listing them together: **unsigned
plus self-replacing means the updater's own signature is the entire security
boundary.** There is no OS check behind it to catch a mistake, so a bug in the
verification path is remote code execution on every machine that has the app.
That is why the manifest and its key came before the download path, and why the
swap refuses more often than it agrees — see **The signing key** above.

### Why three media backends rather than GStreamer everywhere

Every platform plays and no platform ships a decoder: `main.rs` picks the
backend by target — GStreamer on Linux, AVFoundation on macOS, the Media
Foundation Media Engine on Windows — and each one is the stack that machine
already has.

The alternative was GStreamer everywhere. One file instead of three, and it
would have cost exactly what this document is about: roughly 100 MB of MSVC DLLs
beside the `.exe`, which the installer could carry and the *portable single
file* could not — so the two Windows artifacts would have stopped being the same
program; and `GStreamer.framework` inside `mbrd.app/Contents/Frameworks` with an
`install_name_tool` pass and a real signature, which the ad-hoc `codesign` this
release does would not survive.

Two more backends is more code to be wrong in, and it buys back a single-file
Windows build, an unchanged `.app`, and nothing new to install on either.
`Stack` is the whole door and all three fit through it; `spill.rs` is the part
they share. So the packaging cost lands on Linux alone, which is the same shape
as everything else here.

## What isn't automated

- **Version bumping.** Deliberately manual — one number, and it has to agree
  with the tag, which CI checks.
- **The icons.** Rendered from `packaging/icon.svg` by `scripts/icons.sh` and
  **committed**, not built in CI. Three of the four build jobs need them and
  only one of those runs on a Mac, so generating them on a runner would make
  every other job wait on that one for files that change about once a year. Run
  the script by hand when the SVG changes and commit what falls out; the
  `package` job checks they are still there.
- **Key rotation.** Cannot be automated and cannot be done at all. See above.
- **arm64 Linux, and arm64 Windows.** No runner reason not to; nobody has
  asked. The manifest is indexed by triple, so adding one is a data change.
- **Flatpak.** Not built. The app reads `argv` and the XDG directories
  directly, and a sandbox changes both. The updater already refuses to run
  inside one.
