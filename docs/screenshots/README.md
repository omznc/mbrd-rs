# Screenshots

Desktop, 1280×800, both themes — `Ink` where the app is dark and `Sepia` where
it is light. Every shot is the same board, [`studio.mbrd`](studio.mbrd), which
is in here so that a retake is the same picture and not a new one.

| Shot | What it is |
|---|---|
| `board-dark.png`, `board-light.png` | The whole board: pictures, notes, colours, a video, a fence, labelled ropes, tags |
| `opened-dark.png`, `opened-light.png` | One card opened on the whole window, with the info rail out |
| `palette-dark.png` | Every command there is — `Shift` `Shift` |
| `menu-dark.png` | Right-click, on a video card |
| `find-light.png` | `Ctrl`+`F`, part-way through a word |
| `settings-light.png` | *Settings → Application → Appearance* |

The window sits on a backdrop with a 12px corner radius and a shadow, because
nothing composites a transparent corner on the display these are taken on — see
below. The board is a moodboard for a room that does not exist, made of
photographs nobody has to be asked about: every picture in it is CC0, and each
one is credited at the foot of this file.

## Retaking them

```sh
python3 scripts/screenshots.py            # both themes, every shot
python3 scripts/screenshots.py dark       # one theme
python3 scripts/screenshots.py dark board # one shot
```

Needs `python3-xlib`, ImageMagick and Xwayland, and a binary at
`target/debug/mbrd` (or `MBRD_BIN` pointing at one).

The app is undecorated and a Wayland session refuses screen captures to
unsandboxed clients, so this does not shoot the desktop it runs from: it starts
a nested Xwayland with no window manager on it, where the window is the only
thing on the display, sits at (0, 0), and can be sized exactly through
`XConfigureWindow`. That is what makes every shot the same size. Two
consequences are worth knowing, and both are handled in the script rather than
by hand afterwards: nothing composites the window, so the capture has square
corners and the radius and shadow are added after; and with no window manager
to hand focus over, a press is what gives the window the keyboard, so keys sent
before the first click go nowhere.

The script's coordinates are pixels in a fitted 1280×800 window. Move a card on
the board and they move with it.

## The pictures

All CC0, all from [StockSnap](https://stocksnap.io), which is where the
[Openverse](https://openverse.org) search that found them points. CC0 asks for
nothing, so this list is a courtesy rather than a licence condition.

| In the board | By | From |
|---|---|---|
| `ceramics-shelf.jpg` | Barn Images | https://stocksnap.io/photo/ceramics-pottery-SWDR6XR7YS |
| `studio-corner.jpg` | Bench Accounting | https://stocksnap.io/photo/decor-design-60RZGXHD06 |
| `eucalyptus.jpg` | Jazmin Quaynor | https://stocksnap.io/photo/still-items-R7J4JSU0EE |
| `planter.jpg` | Kari Shea | https://stocksnap.io/photo/plants-nature-EV3FZHNKX6 |
| `afternoon-light.jpg` | Jon Tyson | https://stocksnap.io/photo/stairs-shadow-QRK2S101A6 |
| `wool-swatch.jpg` | Seacoast Sage | https://stocksnap.io/photo/textile-texture-JXLLDMKHEE |
| `quilted-cotton.jpg` | Kristin Hardwick | https://stocksnap.io/photo/blue-fabric-GAIMVKKE48 |
| `poured-concrete.jpg` | The Building Envelope | https://stocksnap.io/photo/city-building-ASBLAB40D2 |
| `stacked-stone.jpg` | Matt Bango | https://stocksnap.io/photo/brick-wall-CZOWLU0G8C |
| `dried-stems.jpg` | Vintage RS | https://stocksnap.io/photo/flower-arrangment-LQS5SHPEF0 |

`walkthrough.mp4` on the board is five seconds of `studio-corner.jpg` panned by
`ffmpeg`, so the video card is a real video rather than a poster pretending to
be one.
