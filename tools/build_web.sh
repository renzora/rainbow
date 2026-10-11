#!/bin/sh
# Builds the web version of Rainbow into dist/ (serve that folder, e.g. `python -m http.server -d dist`).
#
# Needs the wasm32-unknown-unknown target and wasm-bindgen-cli at the same version as the
# wasm-bindgen crate in Cargo.lock. Set RAINBOW_CH_PROXY to the Clone Hero proxy's address
# (see tools/clonehero-proxy) to include the leaderboards.
set -e
cd "$(dirname "$0")/.."
cargo build --profile web --target wasm32-unknown-unknown
rm -rf dist
mkdir -p dist
wasm-bindgen --target web --no-typescript --out-dir dist/pkg --out-name rainbow \
  target/wasm32-unknown-unknown/web/rainbow.wasm
# Smaller and faster with binaryen's optimiser, when it's installed.
if command -v wasm-opt >/dev/null 2>&1; then
  wasm-opt -O2 --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals \
    dist/pkg/rainbow_bg.wasm -o dist/pkg/rainbow_bg.wasm
fi
cp web/index.html dist/
cp assets/icon-256.png dist/icon.png
mkdir -p dist/c
cp docs/c/index.html dist/c/
# Downloaded after the app starts (see theme::noto_sans_jp).
mkdir -p dist/fonts
cp assets/fonts/NotoSansJP.ttf dist/fonts/
echo "Built dist/ ($(du -sh dist/pkg/rainbow_bg.wasm | cut -f1) of WebAssembly)"
