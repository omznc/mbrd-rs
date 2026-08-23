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
  will tell you to update through that rather than replacing itself.
- `.tar.gz` — the bare binary, for putting somewhere yourself.

The bare binary and the packages are built against glibc 2.35, so anything from
Ubuntu 22.04 or Fedora 36 onward will run them. They need a Vulkan driver and
`libxkbcommon-x11`, both of which a desktop install normally already has:

```sh
sudo dnf install libxkbcommon-x11    # Fedora
sudo apt install libxkbcommon-x11-0  # Debian / Ubuntu
```

The AppImage needs neither — that is the point of it.

## Updating

mbrd looks for a new version once a day and says so in the status bar. `Ctrl`+`U`
then downloads it, checks it, installs it and restarts — one press per step, so
nothing happens because you pressed a key to see what it did.

Releases are signed with mbrd's own key and the app verifies that signature
before it writes anything. Nothing here is signed with an Apple or Microsoft
certificate, which is what both operating-system warnings above are about.

Where mbrd cannot replace itself — a `.deb` or `.rpm` your package manager owns,
a Flatpak, anything in `/usr` — it says a new version exists and leaves the
installing to whatever put it there.

To turn the check off entirely, either set `"update": false` in mbrd's
`settings.json` or set `MBRD_NO_UPDATE=1` in the environment. Off means no
request is made, not just no message shown.

## Opening a board

Run it with a path, or start it with none for a demonstration board and use
`Ctrl`+`P`. Double-clicking a `.mbrd` works on macOS, on Windows if you used the
installer, and on Linux if you used the `.deb` or `.rpm`.
