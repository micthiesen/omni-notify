#!/bin/sh
# Downloads <url> to <dest> and fails unless its SHA-256 equals <sha256>. Used by
# the Dockerfile's download stages so each pinned asset caches on its own.
#
# Usage: fetch-verified.sh <url> <sha256> <dest>
set -eu

URL=${1:?usage: fetch-verified.sh <url> <sha256> <dest>}
SHA256=${2:?usage: fetch-verified.sh <url> <sha256> <dest>}
DEST=${3:?usage: fetch-verified.sh <url> <sha256> <dest>}

mkdir -p "$(dirname "$DEST")"
curl -fsSL --retry 3 -A "OpenAI File Downloader, XaiImageApiFetch/1.0" \
  -o "$DEST.partial" "$URL"
echo "$SHA256  $DEST.partial" | sha256sum -c -
mv "$DEST.partial" "$DEST"
