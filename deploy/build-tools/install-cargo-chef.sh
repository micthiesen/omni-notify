#!/bin/sh
# Installs the pinned prebuilt cargo-chef into <bin-dir> after verifying the
# release archive's SHA-256, instead of compiling it with `cargo install`.
#
# Usage: install-cargo-chef.sh <bin-dir> [x86_64|aarch64]
set -eu

VERSION=0.1.78
BIN=${1:?usage: install-cargo-chef.sh <bin-dir> [x86_64|aarch64]}
ARCH=${2:-$(uname -m)}

case "$ARCH" in
  x86_64 | amd64)
    ARCH=x86_64
    SHA256=70ef940ef90d04d122f0176fdb8d6c39069191b484a1eaa29b327370c2e1c3c0
    ;;
  aarch64 | arm64)
    ARCH=aarch64
    SHA256=a47e13fba89c2895f5a5c3d0844acd2a5fd416eceb3a6f9dfb26e28155099f4e
    ;;
  *)
    echo "Unsupported architecture: $ARCH" >&2
    exit 1
    ;;
esac

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

NAME="cargo-chef-${ARCH}-unknown-linux-gnu"
"$(dirname "$0")/fetch-verified.sh" \
  "https://github.com/LukeMathWalker/cargo-chef/releases/download/v${VERSION}/${NAME}.tar.xz" \
  "$SHA256" "$WORK/cargo-chef.tar.xz"
tar -xJf "$WORK/cargo-chef.tar.xz" -C "$WORK" "$NAME/cargo-chef"
mkdir -p "$BIN"
install -m 0755 "$WORK/$NAME/cargo-chef" "$BIN/cargo-chef"
