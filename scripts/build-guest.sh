#!/usr/bin/env bash
# Build the QuickJS guest to wasm32-unknown-unknown and copy it where the host
# crate embeds it. Needs a wasm-capable clang (wasi-sdk).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WASI_SDK="${WASI_SDK:-$HOME/.cache/wasi-sdk/wasi-sdk-24.0-arm64-macos}"
if [[ ! -x "$WASI_SDK/bin/clang" ]]; then
  echo "wasi-sdk not found at $WASI_SDK (set WASI_SDK)" >&2
  exit 1
fi
export CC_wasm32_unknown_unknown="$WASI_SDK/bin/clang"
export AR_wasm32_unknown_unknown="$WASI_SDK/bin/llvm-ar"
export CFLAGS_wasm32_unknown_unknown="-O3"
cd "$ROOT/guest"
cargo build --release "$@"
mkdir -p "$ROOT/crates/modebox-core/guest"
cp target/wasm32-unknown-unknown/release/modebox_guest.wasm "$ROOT/crates/modebox-core/guest/modebox_guest.wasm"
ls -la "$ROOT/crates/modebox-core/guest/modebox_guest.wasm"
