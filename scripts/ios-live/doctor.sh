#!/usr/bin/env bash
set -u

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck disable=SC1091
source "$SCRIPT_DIR/common.sh"

failures=0

check_command() {
  if command -v "$1" >/dev/null 2>&1; then
    echo "✓ $1: $(command -v "$1")"
  else
    echo "✗ Missing $1"
    failures=$((failures + 1))
  fi
}

echo "Omni Live iOS doctor"
check_command xcodebuild
check_command xcrun
check_command xcodegen
check_command curl
check_command jq

if xcodebuild -version >/dev/null 2>&1; then
  xcodebuild -version | sed 's/^/  /'
else
  echo "✗ Xcode is not selected correctly"
  failures=$((failures + 1))
fi

if [[ -e /Library/Developer/PrivateFrameworks/CoreSimulator.framework ]]; then
  echo "✓ CoreSimulator system component installed"
else
  echo "✗ CoreSimulator system component is missing"
  echo "  Run once in Terminal, then rerun this doctor:"
  echo "  sudo installer -pkg /Applications/Xcode.app/Contents/Resources/Packages/XcodeSystemResources.pkg -target /"
  failures=$((failures + 1))
fi

if [[ -f "$IOS_PROJECT_DIR/Config/Local.xcconfig" ]]; then
  echo "✓ Config/Local.xcconfig exists"
  if grep -Eq '^OMNI_DEVELOPMENT_TEAM = [A-Z0-9]{10}$' "$IOS_PROJECT_DIR/Config/Local.xcconfig"; then
    echo "✓ Apple development team configured"
  else
    echo "✗ OMNI_DEVELOPMENT_TEAM is missing or malformed"
    failures=$((failures + 1))
  fi
  if grep -Eq '^OMNI_DEFAULT_AUTH_TOKEN = .{24,}$' "$IOS_PROJECT_DIR/Config/Local.xcconfig"; then
    echo "✓ Omni control token configured"
  else
    echo "✗ OMNI_DEFAULT_AUTH_TOKEN must be at least 24 characters"
    failures=$((failures + 1))
  fi

  token="$(sed -n 's/^OMNI_DEFAULT_AUTH_TOKEN = //p' "$IOS_PROJECT_DIR/Config/Local.xcconfig" | tail -1)"
  server_xcconfig="$(sed -n 's/^OMNI_DEFAULT_SERVER_URL = //p' "$IOS_PROJECT_DIR/Config/Local.xcconfig" | tail -1)"
  server="${server_xcconfig/"\$()"/}"
  diagnostics_file="/tmp/omni-live-doctor-diagnostics.json"
  if [[ -n "$token" && -n "$server" ]] && curl -fsS --max-time 5 \
    -A "OpenAI File Downloader, XaiImageApiFetch/1.0" \
    -H "Authorization: Bearer $token" \
    "$server/api/ios-controls/diagnostics" \
    -o "$diagnostics_file"
  then
    echo "✓ Omni control API authenticated at $server"
    if jq -e '.apnsEnabled == true' "$diagnostics_file" >/dev/null; then
      registration_count="$(jq -r '.registrationCount' "$diagnostics_file")"
      echo "✓ Server APNs enabled ($registration_count registered control(s))"
    else
      echo "✗ Server API is reachable but APNs is disabled"
      failures=$((failures + 1))
    fi
  else
    echo "✗ Could not authenticate to the Omni control API at $server"
    failures=$((failures + 1))
  fi
else
  echo "✗ Config/Local.xcconfig is missing"
  echo "  Run: pnpm ios:configure -- --team YOUR_TEAM_ID"
  failures=$((failures + 1))
fi

if security find-identity -v -p codesigning 2>/dev/null | grep -q 'Apple Development'; then
  echo "✓ Apple Development signing identity available"
else
  echo "! No Apple Development signing identity found yet"
  echo "  Sign into your Apple account in Xcode Settings > Accounts once."
fi

echo
xcrun devicectl list devices 2>/dev/null || echo "! No paired iOS device currently visible"

if (( failures > 0 )); then
  echo
  echo "Doctor found $failures required setup item(s)."
  exit 1
fi
echo
echo "All required CLI setup checks passed."
