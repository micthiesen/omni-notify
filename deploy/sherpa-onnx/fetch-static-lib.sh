#!/bin/sh
# Downloads the sherpa-onnx static library archive that sherpa-onnx-sys links,
# verifies its SHA-256 and leaves it in the given directory. Point
# SHERPA_ONNX_ARCHIVE_DIR at that directory so the crate's build script uses this
# verified copy instead of downloading an unchecked one.
#
# Usage: fetch-static-lib.sh <archive-dir> [x86_64|aarch64]
# The version must equal the workspace's sherpa-onnx-sys pin (Cargo.toml).
set -eu

VERSION=1.13.6
DEST=${1:?usage: fetch-static-lib.sh <archive-dir> [x86_64|aarch64]}
ARCH=${2:-$(uname -m)}

case "$ARCH" in
  x86_64 | amd64)
    ASSET_ARCH=x64
    SHA256=fe4968c3cd2ad3dfd1ea925500e5542aef70dd7119847257ffde95d6260cd629
    ;;
  aarch64 | arm64)
    ASSET_ARCH=aarch64
    SHA256=4a602cf8b5158df9667f459bb13574eb297d8df82060e113ac75feb7ad51a420
    ;;
  *)
    echo "Unsupported architecture: $ARCH" >&2
    exit 1
    ;;
esac

ARCHIVE="sherpa-onnx-v${VERSION}-linux-${ASSET_ARCH}-static-lib.tar.bz2"
URL="https://github.com/k2-fsa/sherpa-onnx/releases/download/v${VERSION}/${ARCHIVE}"

mkdir -p "$DEST"
curl -fsSL --retry 3 -A "OpenAI File Downloader, XaiImageApiFetch/1.0" \
  -o "$DEST/$ARCHIVE.partial" "$URL"
echo "$SHA256  $DEST/$ARCHIVE.partial" | sha256sum -c -
mv "$DEST/$ARCHIVE.partial" "$DEST/$ARCHIVE"
