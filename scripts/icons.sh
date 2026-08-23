#!/usr/bin/env sh
# Render every icon the installers need from `packaging/icon.svg`.
#
# The results are **committed**, not built in CI. Three of the four release
# jobs need them and only one of those runs on a Mac, so generating them on a
# runner would make every other job wait on that one for a file that changes
# about once a year. Run this by hand when the SVG changes, and commit what
# falls out.
#
#   sh scripts/icons.sh
#
# Needs ImageMagick and python3. It deliberately does *not* need `iconutil`,
# which only exists on macOS: the `.icns` container is a length-prefixed list
# of PNGs and packing one is thirty lines, which is a much better trade than a
# build step that only one contributor's laptop can run.
set -eu

root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
svg="$root/packaging/icon.svg"
out="$root/packaging/icons"

command -v magick >/dev/null || { echo "icons: ImageMagick (magick) is not on PATH" >&2; exit 1; }
command -v python3 >/dev/null || { echo "icons: python3 is not on PATH" >&2; exit 1; }
[ -f "$svg" ] || { echo "icons: $svg is missing" >&2; exit 1; }

rm -rf "$out"
mkdir -p "$out/png"

# Rendered from the SVG at each size rather than downscaled from one large PNG.
# The mark has 3px hairlines on the cards; resampling those from 1024 turns
# them to grey mush by 32, where rendering at the target size keeps them.
for size in 16 24 32 48 64 128 256 512 1024; do
  magick -background none "$svg" -resize "${size}x${size}" \
    "$out/png/mbrd-${size}.png"
done

# Windows. One file holding the sizes Explorer actually asks for; above 256 it
# never does, and each one it does not ask for is dead weight in every
# executable the icon is linked into.
magick "$out/png/mbrd-16.png" "$out/png/mbrd-24.png" "$out/png/mbrd-32.png" \
       "$out/png/mbrd-48.png" "$out/png/mbrd-64.png" "$out/png/mbrd-128.png" \
       "$out/png/mbrd-256.png" "$out/mbrd.ico"

# Linux. The `hicolor` layout, which is what a `.deb` and an `.rpm` copy into
# `/usr/share/icons/hicolor` verbatim.
for size in 16 24 32 48 64 128 256 512; do
  mkdir -p "$out/hicolor/${size}x${size}/apps"
  cp "$out/png/mbrd-${size}.png" "$out/hicolor/${size}x${size}/apps/mbrd.png"
done
mkdir -p "$out/hicolor/scalable/apps"
cp "$svg" "$out/hicolor/scalable/apps/mbrd.svg"

# macOS. See the note at the top about not needing `iconutil`.
python3 - "$out" <<'PY'
import struct, sys, pathlib

out = pathlib.Path(sys.argv[1])

# OSType -> the pixel size of the PNG that goes in it. The @2x types carry the
# same pixels as their @1x counterpart at twice the size, which is why 32, 256
# and 512 each appear twice: `ic11` is "16pt at 2x" and `icp5` is "32px", and
# a Retina Mac asks for the first while a Dock at 32 asks for the second.
SLOTS = [
    (b"icp4", 16), (b"icp5", 32), (b"ic11", 32), (b"ic12", 64),
    (b"ic07", 128), (b"ic13", 256), (b"ic08", 256), (b"ic14", 512),
    (b"ic09", 512), (b"ic10", 1024),
]

chunks = b""
for ostype, size in SLOTS:
    data = (out / "png" / f"mbrd-{size}.png").read_bytes()
    # Each entry's length counts its own 8-byte header, which is the one place
    # a hand-written icns usually goes wrong.
    chunks += ostype + struct.pack(">I", len(data) + 8) + data

icns = b"icns" + struct.pack(">I", len(chunks) + 8) + chunks
(out / "mbrd.icns").write_bytes(icns)
print(f"icons: mbrd.icns, {len(SLOTS)} sizes, {len(icns) // 1024} KiB")
PY

echo "icons: wrote $(find "$out" -type f | wc -l | tr -d ' ') files under packaging/icons"
