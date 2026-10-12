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

# Omni-HMAC signature, identical to OmniSignature.sign in the app:
# HMAC-SHA256(token, "ts\nnonce\nMETHOD\npath\nsha256hex(body)") as lowercase hex.
# HMAC is built from two SHA-256 passes (RFC 2104) so the token only travels through
# pipes and shell builtins, never through a process's command line.
# Usage: omni_sign TOKEN TIMESTAMP NONCE METHOD PATH BODY_FILE
omni_sign() {
  local token="$1" timestamp="$2" nonce="$3" method="$4" path="$5" body_file="$6"
  local body_hash key_hex ipad="" opad="" i byte inner
  body_hash="$(openssl dgst -sha256 -r "$body_file" | cut -d' ' -f1)"
  key_hex="$(printf '%s' "$token" | xxd -p | tr -d '\n')"
  if (( ${#key_hex} > 128 )); then
    key_hex="$(printf '%s' "$token" | openssl dgst -sha256 -r | cut -d' ' -f1)"
  fi
  while (( ${#key_hex} < 128 )); do key_hex+="00"; done
  for ((i = 0; i < 128; i += 2)); do
    byte=$((16#${key_hex:i:2}))
    printf -v ipad '%s%02x' "$ipad" $((byte ^ 0x36))
    printf -v opad '%s%02x' "$opad" $((byte ^ 0x5c))
  done
  inner="$({ printf '%s' "$ipad" | xxd -r -p
    printf '%s\n%s\n%s\n%s\n%s' "$timestamp" "$nonce" "$method" "$path" "$body_hash"
  } | openssl dgst -sha256 -binary | xxd -p | tr -d '\n')"
  { printf '%s' "$opad" | xxd -r -p; printf '%s' "$inner" | xxd -r -p; } \
    | openssl dgst -sha256 -r | cut -d' ' -f1
}

# Signed GET against an /api/ios-controls route. Usage: omni_get SERVER TOKEN PATH OUTPUT
omni_get() {
  local server="$1" token="$2" path="$3" output="$4"
  local empty timestamp nonce signature rest base_path=""
  server="${server%/}"
  # A server like https://host/base signs /base/api/... (the full request path).
  rest="${server#*://}"
  if [[ "$rest" == */* ]]; then base_path="/${rest#*/}"; base_path="${base_path%/}"; fi
  empty="$(mktemp)"
  timestamp="$(date +%s)"
  nonce="$(uuidgen | tr '[:upper:]' '[:lower:]')"
  signature="$(omni_sign "$token" "$timestamp" "$nonce" GET "$base_path$path" "$empty")"
  rm -f "$empty"
  curl -fsS --max-time 5 \
    -A "OpenAI File Downloader, XaiImageApiFetch/1.0" \
    -H "Authorization: Omni-HMAC $signature" \
    -H "X-Omni-Timestamp: $timestamp" \
    -H "X-Omni-Nonce: $nonce" \
    "$server$path" -o "$output"
}
