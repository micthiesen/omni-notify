#!/usr/bin/env bash
# shellcheck disable=SC2034
set -euo pipefail

IOS_PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../ios/OmniLive" && pwd)"
IOS_PROJECT="$IOS_PROJECT_DIR/OmniLive.xcodeproj"
IOS_SCHEME="OmniLive"
IOS_DERIVED_DATA="$IOS_PROJECT_DIR/DerivedData"

require_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

generate_project() {
  require_command xcodegen
  (cd "$IOS_PROJECT_DIR" && xcodegen generate)
}
