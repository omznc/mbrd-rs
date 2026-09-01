## Installing

**macOS** — `.dmg`, `aarch64` for Apple Silicon or `x64` for Intel. Drag mbrd to
Applications, then run this **once**. The app is unsigned, and without it macOS
refuses to open it at all — the right-click → Open trick is not enough on recent
versions.

```sh
xattr -dr com.apple.quarantine /Applications/mbrd.app
```

Once, and not again per release: that attribute is set by whatever *downloaded*
the file, and mbrd's own updater does not set it on what it installs. So this is
a first-install cost, and every update after it is a double-click.

**Windows** — two downloads, pick one.

- `-setup.exe` — installs to your user account (no administrator prompt), puts
  mbrd in the Start menu, and makes `.mbrd` files open when double-clicked.
- `.exe` — no installer. Put it wherever you keep things and run it.

Either way SmartScreen calls it an unrecognised app the first time:
**More info** → **Run anyway**.

**Linux** — four downloads, in the order most people want them.

- `.AppImage` — one file, nothing to install, brings its own libraries.
  `chmod +x` it and run it. This is the one to take if you are not sure.
- `.deb` / `.rpm` — installs to `/usr/bin` with a launcher entry, an icon and
  `.mbrd` files associated. Your package manager owns it afterwards, and mbrd
  updates it the way that package manager would: it downloads the next
  release's package and asks for permission to install it, so nothing goes
  behind `dpkg`'s or `rpm`'s back.
- `.tar.gz` — the bare binary, for putting somewhere yourself.

The bare binary and the packages are built against glibc 2.35, so anything from
Ubuntu 22.04 or Fedora 36 onward will run them. They need a Vulkan driver,
`libxkbcommon-x11` and GStreamer, all of which a desktop install normally
already has:

```sh
sudo dnf install libxkbcommon-x11 gstreamer1 gstreamer1-plugins-base    # Fedora
sudo apt install libxkbcommon-x11-0 libgstreamer1.0-0 \
                 libgstreamer-plugins-base1.0-0                         # Debian
```

For sound and video you want the codecs too — `gstreamer1-plugins-good` and
`gstreamer1-plugin-libav` on Fedora, `gstreamer1.0-plugins-good` and
`gstreamer1.0-libav` on Debian. Without them mbrd runs and every board opens;
only the play button says it has no decoder. The `.deb` and `.rpm` ask for
these for you.

The AppImage needs none of it — that is the point of it. It carries the
codecs as well, so a clip plays there whatever the machine has.

**Windows and macOS need nothing installed at all.** They play through the
decoder the system already has — Media Foundation on Windows, AVFoundation on
macOS — so there is no runtime to fetch, nothing beside the binary, and the
Windows download stays the single portable `.exe` it always was. The list above
is a Linux list because Linux is the platform whose media stack is a package
rather than part of the OS.

What differs between the three is which formats the machine can decode, and
that was always going to be true: a codec you do not have is a codec you do not
have. Everything else — every card, every board, the whole canvas — is the same
program on all three.

## Updating

mbrd looks for a new version once a day and says so in the status bar. `Ctrl`+`U`
then downloads it, checks it, installs it and restarts — one press per step, so
nothing happens because you pressed a key to see what it did.

Releases are signed with mbrd's own key and the app verifies that signature
before it writes anything. Nothing here is signed with an Apple or Microsoft
certificate, which is what both operating-system warnings above are about.

If you installed the `.deb` or the `.rpm`, the last step is a permission prompt
rather than a restart: the new version is the *package*, and `apt` or `dnf`
installs it so that what is on disk and what your package manager thinks is
installed go on agreeing. mbrd says so before the prompt appears. Nothing else
about it differs — the same signed manifest, the same checked download.

One catch, once. The early packages shipped without the key the updater checks
signatures against, so they never look for a new version at all. If mbrd has
never offered you one on a `.deb` or `.rpm` install, that is why: install the
current package by hand, and it will keep itself up to date from then on.

Where mbrd can do neither — a Flatpak, a Snap, a copy somewhere in `/usr` that
no package owns, a machine with no `pkexec` to ask with — it says a new version
exists and leaves the installing to whatever put it there.

To turn the check off entirely, either set `"update": false` in mbrd's
`settings.json` or set `MBRD_NO_UPDATE=1` in the environment. Off means no
request is made, not just no message shown.

## Opening a board

Run it with a path, or start it with none for a demonstration board and use
`Ctrl`+`P`. Double-clicking a `.mbrd` works on macOS, on Windows if you used the
installer, and on Linux if you used the `.deb` or `.rpm`.
