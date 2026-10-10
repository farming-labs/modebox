#!/usr/bin/env bash
# Build the QuickJS guest to wasm32-unknown-unknown and copy it where the host
# crate embeds it. Needs a wasm-capable clang (wasi-sdk 24).
#
# The build is path-free and pinned (guest/rust-toolchain.toml), so the
# committed wasm carries no paths from the machine that built it and CI can
# rebuild it and compare byte for byte.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) SDK_SUFFIX=arm64-macos ;;
  Darwin-x86_64) SDK_SUFFIX=x86_64-macos ;;
  Linux-aarch64) SDK_SUFFIX=arm64-linux ;;
  *) SDK_SUFFIX=x86_64-linux ;;
esac
WASI_SDK="${WASI_SDK:-$HOME/.cache/wasi-sdk/wasi-sdk-24.0-$SDK_SUFFIX}"
if [[ ! -x "$WASI_SDK/bin/clang" ]]; then
  echo "wasi-sdk 24 not found at $WASI_SDK (set WASI_SDK)" >&2
  exit 1
fi
CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export CC_wasm32_unknown_unknown="$WASI_SDK/bin/clang"
export AR_wasm32_unknown_unknown="$WASI_SDK/bin/llvm-ar"
# QuickJS's C sources are compiled from Cargo's build directory, so map that too.
# Shell-style flag parsing keeps paths with spaces intact.
export CC_SHELL_ESCAPED_FLAGS=1
q() { printf '%q' "$1"; }
export CFLAGS_wasm32_unknown_unknown="-O3 -ffile-prefix-map=$(q "$ROOT/guest/target")=/target -ffile-prefix-map=$(q "$CARGO_HOME")=/cargo -ffile-prefix-map=$(q "$WASI_SDK")=/wasi-sdk"
# One flag per \x1f-separated field, so paths with spaces survive. Setting this
# replaces guest/.cargo/config.toml's rustflags, so the stack size is repeated.
SEP=$'\x1f'
export CARGO_ENCODED_RUSTFLAGS="-Clink-arg=-zstack-size=4194304${SEP}--remap-path-prefix=$CARGO_HOME=/cargo${SEP}--remap-path-prefix=$ROOT=/modebox"
cd "$ROOT/guest"
cargo build --release "$@"
mkdir -p "$ROOT/crates/modebox-core/guest"
cp target/wasm32-unknown-unknown/release/modebox_guest.wasm "$ROOT/crates/modebox-core/guest/modebox_guest.wasm"
ls -la "$ROOT/crates/modebox-core/guest/modebox_guest.wasm"
