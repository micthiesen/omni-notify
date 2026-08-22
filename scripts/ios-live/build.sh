#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/common.sh"

mode="${1:-device}"
generate_project

if [[ "$mode" == "simulator" ]]; then
  xcodebuild -project "$IOS_PROJECT" -scheme "$IOS_SCHEME" \
    -configuration Debug -sdk iphonesimulator \
    -derivedDataPath "$IOS_DERIVED_DATA" CODE_SIGNING_ALLOWED=NO build
else
  xcodebuild -project "$IOS_PROJECT" -scheme "$IOS_SCHEME" \
    -configuration Debug -destination 'generic/platform=iOS' \
    -derivedDataPath "$IOS_DERIVED_DATA" \
    -allowProvisioningUpdates build
fi
