#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/common.sh"

# Unit tests (OmniLiveTests) and widget snapshots (OmniWidgetSnapshotTests, PNGs in
# /tmp/omni-widget-snapshots) on an iPhone and an iPad simulator. Override with
# IOS_SIMULATOR_DESTINATION to run on one destination only.
generate_project
if [[ -n "${IOS_SIMULATOR_DESTINATION:-}" ]]; then
  destinations=("$IOS_SIMULATOR_DESTINATION")
else
  destinations=(
    "platform=iOS Simulator,name=iPhone 17 Pro"
    "platform=iOS Simulator,name=iPad Pro 13-inch (M5)"
  )
fi
for destination in "${destinations[@]}"; do
  echo "== $destination"
  xcodebuild -project "$IOS_PROJECT" -scheme "$IOS_SCHEME" \
    -configuration Debug -destination "$destination" \
    -derivedDataPath "$IOS_DERIVED_DATA" CODE_SIGNING_ALLOWED=NO test
done
