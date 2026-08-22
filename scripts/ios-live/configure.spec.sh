#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
fixture_root="/tmp/omni-ios-configure-spec"
fixture_scripts="$fixture_root/scripts/ios-live"
fixture_config="$fixture_root/ios/OmniLive/Config"

mkdir -p "$fixture_scripts" "$fixture_config"
cp "$SCRIPT_DIR/common.sh" "$SCRIPT_DIR/configure.sh" "$fixture_scripts/"

bash "$fixture_scripts/configure.sh" \
  --team ABCDE12345 \
  --token abcdefghijklmnopqrstuvwxyz123456 \
  --server https://omni.boris/base \
  --bundle-id com.example.OmniLive >/dev/null

result="$fixture_config/Local.xcconfig"
grep -Fxq 'OMNI_DEVELOPMENT_TEAM = ABCDE12345' "$result"
grep -Fxq "OMNI_DEFAULT_SERVER_URL = https:/\$()/omni.boris/base" "$result"
grep -Fxq 'OMNI_DEFAULT_AUTH_TOKEN = abcdefghijklmnopqrstuvwxyz123456' "$result"
grep -Fxq 'OMNI_BUNDLE_ID = com.example.OmniLive' "$result"
grep -Fxq 'OMNI_APP_GROUP = group.com.example.OmniLive' "$result"

if bash "$fixture_scripts/configure.sh" \
  --team short \
  --token abcdefghijklmnopqrstuvwxyz123456 >/dev/null 2>&1
then
  echo "Invalid Team ID unexpectedly succeeded" >&2
  exit 1
fi

if bash "$fixture_scripts/configure.sh" \
  --team ABCDE12345 \
  --token 'not safe enough / characters' >/dev/null 2>&1
then
  echo "Invalid token unexpectedly succeeded" >&2
  exit 1
fi

if bash "$fixture_scripts/configure.sh" \
  --team ABCDE12345 \
  --token abcdefghijklmnopqrstuvwxyz123456 \
  --server http://custom.example >/dev/null 2>&1
then
  echo "Unapproved insecure server unexpectedly succeeded" >&2
  exit 1
fi

echo "iOS configure fixtures passed: values, URL escaping, and validation"
