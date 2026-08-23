## Installing

**macOS** — `.dmg`, `aarch64` for Apple Silicon or `x64` for Intel. Drag mbrd to
Applications, then run this once. The app is unsigned, and without it macOS
refuses to open it at all — the right-click → Open trick is not enough on recent
versions.

```sh
xattr -dr com.apple.quarantine /Applications/mbrd.app
```

**Windows** — `.exe`, no installer: put it wherever you keep things and run it.
SmartScreen calls it an unrecognised app: **More info** → **Run anyway**.

**Linux** — `.tar.gz`, one binary inside. It is built against glibc 2.35, so
anything from Ubuntu 22.04 or Fedora 36 onward will run it. It needs a Vulkan
driver and `libxkbcommon-x11`, both of which a desktop install normally already
has:

```sh
sudo dnf install libxkbcommon-x11    # Fedora
sudo apt install libxkbcommon-x11-0  # Debian / Ubuntu
```

Nothing here is signed with an OS vendor certificate, which is what both
warnings are about. There is no updater — a new version is a new download.

Opening a board: run it with a path, or start it with none for a demonstration
board and use `Ctrl`+`P`. On macOS the Finder will not offer mbrd for a `.mbrd`
file yet.
