#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/common.sh"

device_id="${1:-}"
if [[ -z "$device_id" ]]; then
  echo "Usage: pnpm ios:install -- DEVICE_IDENTIFIER" >&2
  echo "Available devices:" >&2
  xcrun devicectl list devices >&2 || true
  exit 2
fi

bash "$SCRIPT_DIR/doctor.sh"
generate_project
xcodebuild -project "$IOS_PROJECT" -scheme "$IOS_SCHEME" \
  -configuration Debug -destination "id=$device_id" \
  -derivedDataPath "$IOS_DERIVED_DATA" \
  -allowProvisioningUpdates -allowProvisioningDeviceRegistration build

app_path="$IOS_DERIVED_DATA/Build/Products/Debug-iphoneos/Omni Live.app"
if [[ ! -d "$app_path" ]]; then
  echo "Built app not found at $app_path" >&2
  exit 1
fi
xcrun devicectl device install app --device "$device_id" "$app_path"
echo "Installed Omni Live. Open it once, then add Omni Live Stream in Control Center."
