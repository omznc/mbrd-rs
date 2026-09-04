#!/usr/bin/env bash
#
# Build the web app into `dist/`, ready to be served by anything that can serve
# files.
#
#   scripts/build-web.sh            # release, optimised
#   scripts/build-web.sh --dev      # a fast build for looking at a change
#
# The output is static and has no server requirements beyond the two MIME types
# every host already knows: `application/wasm` for the module and
# `text/javascript` for the glue. There is no back end and no API — the boards
# live in the browser's own database, on the machine they were made on.
#
# ## Why the three steps are three steps
#
# `cargo build` produces a wasm module with wasm-bindgen's placeholders in it;
# `wasm-bindgen` turns those into the JavaScript that imports it and the final
# `.wasm`; `wasm-opt` then reduces what came out, by roughly a fifth. Only the
# first is a compiler — the other two are post-processing, and the last one is
# optional, which is why a missing `wasm-opt` is a warning here rather than a
# failure.
set -euo pipefail

cd "$(dirname "$0")/.."

profile="release"
target_dir="release"
for arg in "$@"; do
  case "$arg" in
    --dev) profile="dev"; target_dir="debug" ;;
    *) echo "unknown argument: $arg" >&2; exit 2 ;;
  esac
done

out="dist"
wasm="target/wasm32-unknown-unknown/$target_dir/mbrd.wasm"

# What a cache is named after, and what a request is busted by. The version is
# the one thing a person reads; the commit is what makes two builds of the same
# version different files.
version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
commit="$(git rev-parse --short HEAD 2>/dev/null || echo local)"
stamp="$version+$commit"

echo "==> building mbrd $stamp for the web ($profile)"
rustup target add wasm32-unknown-unknown >/dev/null 2>&1 || true

if [ "$profile" = "release" ]; then
  cargo build -p mbrd --release --target wasm32-unknown-unknown
else
  cargo build -p mbrd --target wasm32-unknown-unknown
fi

echo "==> generating the JavaScript that loads it"
if ! command -v wasm-bindgen >/dev/null; then
  # Pinned to the crate in the lockfile. A mismatch between the two is the
  # classic wasm-bindgen failure and it reports itself as a schema version
  # error at load time, in the browser, rather than here.
  wanted="$(sed -n '/^name = "wasm-bindgen"$/{n;s/^version = "\(.*\)"/\1/p;}' Cargo.lock | head -1)"
  echo "    installing wasm-bindgen-cli $wanted"
  cargo install wasm-bindgen-cli --version "$wanted"
fi

rm -rf "$out"
mkdir -p "$out"
wasm-bindgen --target web --no-typescript --out-dir "$out" --out-name mbrd "$wasm"

if command -v wasm-opt >/dev/null; then
  echo "==> optimising"
  # `-O3` for speed and `--strip-debug` for size; the two together take about a
  # fifth off what wasm-bindgen produced. `--enable-bulk-memory` and friends are
  # not optional flags so much as a description of what LLVM already emitted:
  # without them wasm-opt refuses a module using features it has not been told
  # about.
  wasm-opt -O3 --strip-debug \
    --enable-bulk-memory --enable-nontrapping-float-to-int \
    --enable-sign-ext --enable-mutable-globals --enable-reference-types \
    -o "$out/mbrd_bg.wasm.opt" "$out/mbrd_bg.wasm"
  mv "$out/mbrd_bg.wasm.opt" "$out/mbrd_bg.wasm"
else
  echo "==> wasm-opt not found, skipping (the module works, it is just larger)"
fi

echo "==> assembling the page"
cp web/index.html web/manifest.webmanifest web/icon.svg web/icon-192.png web/icon-512.png "$out/"
[ -f web/CNAME ] && cp web/CNAME "$out/"
sed "s/__VERSION__/$stamp/" web/sw.js > "$out/sw.js"

# `.nojekyll`, because GitHub Pages otherwise runs the output through Jekyll,
# and Jekyll drops every file whose name begins with an underscore. Nothing
# here starts with one today; this is so that the day something does, it is not
# a deploy that silently serves a 404 for one file.
touch "$out/.nojekyll"

size="$(du -h "$out/mbrd_bg.wasm" | cut -f1)"
gz="$(gzip -c "$out/mbrd_bg.wasm" | wc -c | awk '{printf "%.1fM", $1/1048576}')"
echo "==> $out/ is ready — $size of wasm, about $gz over the wire"
echo "    try it with:  python3 -m http.server -d $out 8000"
