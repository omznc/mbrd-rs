#!/usr/bin/env python3
"""Retake the screenshots in `docs/screenshots`, from `docs/screenshots/studio.mbrd`.

    python3 scripts/screenshots.py            # both themes, every shot
    python3 scripts/screenshots.py dark       # one theme
    python3 scripts/screenshots.py dark board # one shot

The app is undecorated and a Wayland session refuses screen captures to
unsandboxed clients, so this does not shoot the desktop it is run from. It
starts a **nested Xwayland with no window manager on it**, which is what makes
every shot identical in size: the window is the only thing on that display, it
sits at (0,0), and it can be sized exactly through `XConfigureWindow` rather
than by asking a compositor nicely. GPUI picks its backend by looking at
`WAYLAND_DISPLAY` first, so the app is launched with that variable *removed*
and `DISPLAY` pointing at the nested server.

Nothing composites a window on that display, so the capture comes back with
square corners; the 12px radius and the shadow are added here afterwards,
which is also where the backdrop comes from.

The app is driven with XTEST — real key and button events, delivered by the X
server the same way a keyboard would. Two things about that are worth knowing
before editing the shot list:

- **A press is what gives the window keyboard focus.** With no window manager
  to hand focus over, `SetInputFocus` alone is not enough; `Session.focus`
  clicks an empty corner of the canvas first, and keys sent before that go
  nowhere.
- **The coordinates in `SHOTS` are screen pixels** in a 1280x800 window whose
  camera has just been fitted with `f`, which `Session` does on the way up so
  that one shot can be retaken on its own. Change the board and they move.

Settings come from a throwaway `XDG_CONFIG_HOME` so a retake cannot disturb
whoever is running it, with one exception: the Appearance panel prints the
themes folder's real path, so that one shot runs against the real `~/.config`
and puts the file back afterwards. Boards are copied to `/tmp` before they are
opened, because mbrd saves a second after every change and `f` is a change.

Needs `python3-xlib`, ImageMagick, and Xwayland.
"""
import json
import os
import shutil
import subprocess
import sys
import time

from Xlib import X, XK, display as xdisplay
from Xlib.ext import xtest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DISPLAY = ":99"
SCREEN = (1600, 1000)
WIN = (1280, 800)
RADIUS, PAD = 12, 120
BACKDROP = {"dark": ("#292c33", "#14161a"), "light": ("#f6f1e8", "#e0d9cb")}

BIN = os.environ.get("MBRD_BIN") or os.path.join(ROOT, "target/debug/mbrd")
BOARD = os.environ.get("MBRD_BOARD") or os.path.join(ROOT, "docs/screenshots/studio.mbrd")
OUT = os.path.join(ROOT, "docs/screenshots")
WORK = "/tmp/mbrd-screenshots"
CONF = os.path.join(WORK, "config")


# The shots, per theme: a name, and what to do to the window before it is
# taken. `real_config` is the one flag any of them needs — see the note above.
def SHOTS(mode):
    rest = lambda s: s.key("Escape")    # the session already fitted the camera
    return {
        "dark": [
            ("board", rest, {}),
            ("opened", lambda s: (s.click(765, 207), s.key("o"), s.click(1207, 65)), {}),
            ("palette", lambda s: (s.key("Escape"), s.key("Escape"), s.double_shift()), {}),
            ("menu", lambda s: (s.key("Escape"), s.click(485, 412, button=3)), {}),
        ],
        "light": [
            ("board", rest, {}),
            ("opened", lambda s: (s.click(287, 207), s.key("o"), s.click(1207, 65)), {}),
            ("settings", lambda s: (s.key("Escape"), s.key("Escape"),
                                    s.click(199, 17), s.click(265, 262)), {"real_config": True}),
            ("find", lambda s: (s.key("Escape"), s.key("f", ["Control_L"]), s.type("stone")), {}),
        ],
    }[mode]


def write_settings(mode, real):
    directory = os.path.expanduser("~/.config/mbrd") if real else os.path.join(CONF, "mbrd")
    os.makedirs(directory, exist_ok=True)
    with open(os.path.join(directory, "settings.json"), "w") as f:
        json.dump({"motion": True, "update": False, "mode": mode, "theme": "Ink",
                   "theme_light": "Sepia", "welcomed": True, "newBoardSnap": False,
                   "newBoardGridStep": 64.0}, f)


class Session:
    """One nested X server with one mbrd on it."""

    def __init__(self, mode, real_config=False):
        write_settings(mode, real_config)
        self.x = subprocess.Popen(
            ["Xwayland", DISPLAY, "-geometry", "%dx%d" % SCREEN, "-noreset"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        for _ in range(60):
            if os.path.exists("/tmp/.X11-unix/X" + DISPLAY[1:]):
                break
            time.sleep(0.2)
        time.sleep(0.6)

        board = os.path.join(WORK, "board-%s.mbrd" % mode)
        shutil.copy(BOARD, board)
        env = dict(os.environ)
        env.pop("WAYLAND_DISPLAY", None)
        if real_config:
            env.pop("XDG_CONFIG_HOME", None)
        else:
            env["XDG_CONFIG_HOME"] = CONF
        env.update(DISPLAY=DISPLAY, XDG_STATE_HOME=os.path.join(WORK, "state"),
                   XDG_CACHE_HOME=os.path.join(WORK, "cache"))
        self.app = subprocess.Popen([BIN, board], env=env,
                                    stdout=open(os.path.join(WORK, "app.log"), "w"),
                                    stderr=subprocess.STDOUT)

        self.d = xdisplay.Display(DISPLAY)
        root = self.d.screen().root
        self.win = None
        for _ in range(150):
            for w in root.query_tree().children:
                geometry = w.get_geometry()
                if geometry.width > 200 and geometry.height > 200:
                    self.win = w
            if self.win:
                break
            time.sleep(0.2)
        if not self.win:
            raise SystemExit("no window appeared — see " + os.path.join(WORK, "app.log"))
        self.win.configure(x=0, y=0, width=WIN[0], height=WIN[1])
        self.d.sync()
        time.sleep(8)                      # the board, its pictures, and the first paint
        self.focus()
        self.key("f")                      # fit, which is what the coordinates below assume
        self.key("Escape")

    def focus(self):
        self.win.set_input_focus(X.RevertToParent, X.CurrentTime)
        self.click(1240, 60)               # empty canvas; a press is what focuses the window
        self.d.sync()

    def move(self, x, y):
        xtest.fake_input(self.d, X.MotionNotify, x=x, y=y)
        self.d.sync()
        time.sleep(0.15)

    def click(self, x, y, button=1, double=False):
        self.move(x, y)
        for _ in range(2 if double else 1):
            xtest.fake_input(self.d, X.ButtonPress, button)
            xtest.fake_input(self.d, X.ButtonRelease, button)
            self.d.sync()
            time.sleep(0.06 if double else 0.2)
        time.sleep(0.6)

    def key(self, name, mods=()):
        codes = [self.d.keysym_to_keycode(XK.string_to_keysym(m)) for m in mods]
        code = self.d.keysym_to_keycode(XK.string_to_keysym(name))
        for c in codes:
            xtest.fake_input(self.d, X.KeyPress, c)
        xtest.fake_input(self.d, X.KeyPress, code)
        xtest.fake_input(self.d, X.KeyRelease, code)
        for c in reversed(codes):
            xtest.fake_input(self.d, X.KeyRelease, c)
        self.d.sync()
        time.sleep(0.5)

    def double_shift(self):
        code = self.d.keysym_to_keycode(XK.string_to_keysym("Shift_L"))
        for _ in range(2):
            xtest.fake_input(self.d, X.KeyPress, code)
            xtest.fake_input(self.d, X.KeyRelease, code)
            self.d.sync()
            time.sleep(0.08)
        time.sleep(0.6)

    def type(self, text):
        for character in text:
            self.key(character)

    def shot(self, name):
        time.sleep(1.6)
        raw = os.path.join(WORK, name + ".png")
        subprocess.run(["import", "-display", DISPLAY, "-window", hex(self.win.id), raw],
                       check=True)
        return raw

    def close(self):
        self.app.terminate()
        time.sleep(0.6)
        self.app.kill()
        self.x.terminate()
        time.sleep(0.4)
        self.x.kill()


def dress(raw, name, mode):
    """The corners nothing composited, and the backdrop underneath."""
    mask = os.path.join(WORK, "mask.png")
    plain = os.path.join(WORK, "plain-" + name + ".png")
    shadow = os.path.join(WORK, "shadow.png")
    top, bottom = BACKDROP[mode]
    subprocess.run(["magick", "-size", "%dx%d" % WIN, "xc:black", "-fill", "white", "-draw",
                    "roundrectangle 0,0,%d,%d,%d,%d" % (WIN[0] - 1, WIN[1] - 1, RADIUS, RADIUS),
                    mask], check=True)
    subprocess.run(["magick", raw, mask, "-alpha", "off", "-compose", "CopyOpacity",
                    "-composite", plain], check=True)
    subprocess.run(["magick", plain, "-background", "black", "-shadow", "55x24+0+14", shadow],
                   check=True)
    subprocess.run(["magick", "-size", "%dx%d" % (WIN[0] + PAD * 2, WIN[1] + PAD * 2),
                    "gradient:%s-%s" % (top, bottom),
                    shadow, "-gravity", "center", "-geometry", "+0+14", "-composite",
                    plain, "-gravity", "center", "-composite", "-strip",
                    os.path.join(OUT, name + ".png")], check=True)


def main():
    modes = [sys.argv[1]] if len(sys.argv) > 1 else ["dark", "light"]
    only = sys.argv[2:]
    os.makedirs(WORK, exist_ok=True)
    os.makedirs(OUT, exist_ok=True)
    if not os.path.exists(BIN):
        raise SystemExit("no binary at %s — build one, or set MBRD_BIN" % BIN)

    for mode in modes:
        wanted = [s for s in SHOTS(mode) if not only or s[0] in only]
        # The one shot that runs against the real settings file is taken in its
        # own session, with the file put back the moment it is done.
        for group_real in (False, True):
            group = [s for s in wanted if bool(s[2].get("real_config")) is group_real]
            if not group:
                continue
            saved = None
            real_settings = os.path.expanduser("~/.config/mbrd/settings.json")
            if group_real and os.path.exists(real_settings):
                saved = open(real_settings).read()
            session = Session(mode, real_config=group_real)
            try:
                for name, act, _ in group:
                    act(session)
                    full = "%s-%s" % (name, mode)
                    dress(session.shot(full), full, mode)
                    print("wrote docs/screenshots/%s.png" % full, flush=True)
            finally:
                session.close()
                if saved is not None:
                    open(real_settings, "w").write(saved)


if __name__ == "__main__":
    main()
