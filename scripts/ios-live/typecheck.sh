#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/common.sh"

SDK="$(xcrun --sdk iphoneos --show-sdk-path)"
TARGET="arm64-apple-ios26.0"
cd "$IOS_PROJECT_DIR"

typecheck() {
  local module="$1"
  shift
  mkdir -p "/tmp/omni-ios-typecheck/$module"
  xcrun swiftc -typecheck -swift-version 6 \
    -module-cache-path "/tmp/omni-ios-typecheck/$module" \
    -sdk "$SDK" -target "$TARGET" -module-name "$module" "$@"
}

typecheck OmniLive Shared/*.swift SharedControls/*.swift DesignKit/*.swift App/*.swift
typecheck OmniLiveControls -application-extension \
  Shared/*.swift SharedControls/*.swift DesignKit/*.swift Controls/*.swift Widgets/*.swift
typecheck OmniNotificationService -application-extension Shared/*.swift NotificationService/*.swift
echo "App, widget/control extension and notification service type-check against $(basename "$SDK")."

# Pure contract checks that run on the Mac without a simulator.
static_binary="/tmp/omni-live-static-tests"
xcrun swiftc \
  -module-cache-path /tmp/omni-live-static-test-modules \
  Shared/*.swift StaticTests/StaticContractTests.swift \
  -o "$static_binary"
"$static_binary"

# The bash signer (doctor.sh) must agree with OmniSignature in the app.
body_file="$(mktemp)"
printf '%s' '{"a":1}' > "$body_file"
bash_signature="$(omni_sign test-token-0123456789abcdef 1760000000 \
  00000000-0000-4000-8000-000000000000 PUT /api/ios-controls/device "$body_file")"
rm -f "$body_file"
swift_signature="$("$static_binary" --sign)"
if [[ "$bash_signature" != "$swift_signature" ]]; then
  echo "Bash and Swift Omni-HMAC signatures differ: $bash_signature vs $swift_signature" >&2
  exit 1
fi
echo "Bash and Swift Omni-HMAC signers agree."

plutil -lint \
  App/Info.plist \
  Controls/Info.plist \
  NotificationService/Info.plist \
  App/OmniLive.entitlements \
  Controls/OmniLiveControls.entitlements \
  NotificationService/OmniNotificationService.entitlements \
  "$IOS_PROJECT/project.pbxproj" >/dev/null
echo "Info plists, entitlements, and generated Xcode project are valid."
