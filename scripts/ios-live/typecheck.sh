#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/common.sh"

SDK="$(xcrun --sdk iphoneos --show-sdk-path)"
mkdir -p /tmp/omni-live-swift-modules /tmp/omni-live-controls-swift-modules

(cd "$IOS_PROJECT_DIR" && xcrun swiftc -typecheck \
  -module-cache-path /tmp/omni-live-swift-modules \
  -sdk "$SDK" -target arm64-apple-ios18.0 -module-name OmniLive \
  Shared/*.swift App/*.swift)
(cd "$IOS_PROJECT_DIR" && xcrun swiftc -typecheck \
  -application-extension \
  -module-cache-path /tmp/omni-live-controls-swift-modules \
  -sdk "$SDK" -target arm64-apple-ios18.0 -module-name OmniLiveControls \
  Shared/*.swift Controls/*.swift)

echo "App and control extension type-check against $(basename "$SDK")."

static_binary="/tmp/omni-live-static-tests"
(cd "$IOS_PROJECT_DIR" && xcrun swiftc \
  -module-cache-path /tmp/omni-live-static-test-modules \
  Shared/LiveSlotState.swift Shared/OmniSettings.swift \
  Shared/LiveSlotResolution.swift StaticTests/StaticContractTests.swift \
  -o "$static_binary")
"$static_binary"

plutil -lint \
  "$IOS_PROJECT_DIR/App/Info.plist" \
  "$IOS_PROJECT_DIR/Controls/Info.plist" \
  "$IOS_PROJECT_DIR/App/OmniLive.entitlements" \
  "$IOS_PROJECT_DIR/Controls/OmniLiveControls.entitlements" \
  "$IOS_PROJECT/project.pbxproj" >/dev/null
echo "Info plists, entitlements, and generated Xcode project are valid."
