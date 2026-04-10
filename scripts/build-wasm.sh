#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

# Prefer newer wasm-opt from ~/.local/bin if available
if [[ -x "$HOME/.local/bin/wasm-opt" ]]; then
  export PATH="$HOME/.local/bin:$PATH"
fi

echo "Building gnata-rs WASM (--target web)..."

# Use regex-lite for smaller binary (579KB vs 1.3MB) and opt-level=3 for
# runtime speed (30% faster eval vs opt-level="z", 830KB final).
# Build without wasm-opt (Rust 2024 emits bulk-memory ops that wasm-pack's
# built-in wasm-opt invocation doesn't handle). We run wasm-opt manually after.
CARGO_PROFILE_RELEASE_OPT_LEVEL=3 \
  wasm-pack build --target web --out-dir ../../pkg crates/gnata-rs --no-opt \
  -- --no-default-features --features regex-lite

# Run wasm-opt with speed optimization (-O3) and bulk-memory enabled
if command -v wasm-opt &>/dev/null; then
  echo "Optimizing with wasm-opt ($(wasm-opt --version))..."
  wasm-opt -O3 --enable-bulk-memory --enable-nontrapping-float-to-int -o pkg/gnata_bg.wasm pkg/gnata_bg.wasm
fi

echo "Done. Artifacts in pkg/"
ls -lh pkg/gnata_bg.wasm pkg/gnata.js pkg/gnata.d.ts
