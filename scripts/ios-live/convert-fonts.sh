#!/usr/bin/env bash
set -euo pipefail

# Rebuilds the bundled iOS TTFs from the web's vendored Geist woff2 files. iOS cannot
# load woff2; CoreText decodes it on the Mac and woff2ttf.swift writes the same
# tables back out as a plain sfnt (no glyph changes). Run after a web font update.
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/common.sh"

web_fonts="$IOS_PROJECT_DIR/../../crates/omni-web/assets/fonts"
ios_fonts="$IOS_PROJECT_DIR/Resources/Fonts"
tool="/tmp/omni-woff2ttf"
xcrun swiftc -O "$SCRIPT_DIR/woff2ttf.swift" -o "$tool"
"$tool" "$web_fonts/Geist-Variable.woff2" "$ios_fonts/Geist-Variable.ttf"
"$tool" "$web_fonts/GeistMono-Variable.woff2" "$ios_fonts/GeistMono-Variable.ttf"
cp "$web_fonts/OFL.txt" "$ios_fonts/OFL.txt"
echo "Wrote Geist TTFs to $ios_fonts"
