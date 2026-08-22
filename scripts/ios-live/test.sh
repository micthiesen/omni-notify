#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/common.sh"

generate_project
destination="${IOS_SIMULATOR_DESTINATION:-platform=iOS Simulator,name=iPad Pro 13-inch (M5)}"
xcodebuild -project "$IOS_PROJECT" -scheme "$IOS_SCHEME" \
  -configuration Debug -destination "$destination" \
  -derivedDataPath "$IOS_DERIVED_DATA" CODE_SIGNING_ALLOWED=NO test
