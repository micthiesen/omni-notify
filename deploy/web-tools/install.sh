#!/bin/sh
# Installs the pinned frontend build tools (trunk, wasm-bindgen, wasm-opt) into
# <bin-dir> after verifying each release archive's SHA-256. Used by the Docker
# web stage and CI so both build crates/omni-web with identical tools.
#
# Usage: install.sh <bin-dir> [x86_64|aarch64]
# wasm-bindgen must equal the workspace's wasm-bindgen crate pin (Cargo.toml);
# wasm-opt must equal the version crates/omni-web/Trunk.toml requests.
set -eu

TRUNK_VERSION=0.21.14
WASM_BINDGEN_VERSION=0.2.129
BINARYEN_VERSION=version_133
UA="OpenAI File Downloader, XaiImageApiFetch/1.0"

BIN=${1:?usage: install.sh <bin-dir> [x86_64|aarch64]}
ARCH=${2:-$(uname -m)}

case "$ARCH" in
  x86_64 | amd64)
    ARCH=x86_64
    TRUNK_SHA256=f2b4680cd239693a646a2795e4633c625328d7b2a044fbe749fa3a2fe9e7036b
    WASM_BINDGEN_SHA256=82d12bb940e2d4e72e0d5605387fc1b8ca179044e012b620f0ce4e7440e8320e
    BINARYEN_SHA256=2dc9c7813f5375db93d96ead4b78222fcc3e2677bbb832297af4797782a37489
    ;;
  aarch64 | arm64)
    ARCH=aarch64
    TRUNK_SHA256=b1d8e60e454f7fc182d9a4d95d1506ffbae947d8ba90f8f6f02da93b60f980f9
    WASM_BINDGEN_SHA256=2ed4351c35dd9440308bbb02767d47ea278efe851a52465300f3c94f5b6c2a87
    BINARYEN_SHA256=89c07ea56faf38d0fbecf36ca8ec0721756716185f265b568e133d427f299bf8
    ;;
  *)
    echo "Unsupported architecture: $ARCH" >&2
    exit 1
    ;;
esac

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

fetch() { # <url> <sha256> <file>
  curl -fsSL --retry 3 -A "$UA" -o "$WORK/$3" "$1"
  echo "$2  $WORK/$3" | sha256sum -c -
}

fetch "https://github.com/trunk-rs/trunk/releases/download/v${TRUNK_VERSION}/trunk-${ARCH}-unknown-linux-gnu.tar.gz" \
  "$TRUNK_SHA256" trunk.tar.gz
WB="wasm-bindgen-${WASM_BINDGEN_VERSION}-${ARCH}-unknown-linux-musl"
fetch "https://github.com/wasm-bindgen/wasm-bindgen/releases/download/${WASM_BINDGEN_VERSION}/${WB}.tar.gz" \
  "$WASM_BINDGEN_SHA256" wasm-bindgen.tar.gz
fetch "https://github.com/WebAssembly/binaryen/releases/download/${BINARYEN_VERSION}/binaryen-${BINARYEN_VERSION}-${ARCH}-linux.tar.gz" \
  "$BINARYEN_SHA256" binaryen.tar.gz

mkdir -p "$BIN"
tar -xzf "$WORK/trunk.tar.gz" -C "$WORK" trunk
tar -xzf "$WORK/wasm-bindgen.tar.gz" -C "$WORK" "$WB/wasm-bindgen"
tar -xzf "$WORK/binaryen.tar.gz" -C "$WORK" "binaryen-${BINARYEN_VERSION}/bin/wasm-opt"
install -m 0755 "$WORK/trunk" "$BIN/trunk"
install -m 0755 "$WORK/$WB/wasm-bindgen" "$BIN/wasm-bindgen"
install -m 0755 "$WORK/binaryen-${BINARYEN_VERSION}/bin/wasm-opt" "$BIN/wasm-opt"
