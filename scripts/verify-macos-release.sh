#!/usr/bin/env bash
set -euo pipefail
: "${APPLE_TEAM_ID:?Set the expected Developer ID team}"
bundle=${1:?Usage: verify-macos-release.sh /path/to/Serylane.app}
"$(dirname "$0")/verify-macos-layout.sh" "$bundle"
for name in serylane mihomo-tun-helper serylane-login-helper mihomo; do
  file="$bundle/Contents/MacOS/$name"
  test -x "$file" || { echo "Missing executable: $name" >&2; exit 1; }
  codesign --verify --strict "$file"
  details=$(codesign -d --verbose=4 "$file" 2>&1)
  grep -Fq "TeamIdentifier=$APPLE_TEAM_ID" <<< "$details" || { echo "Signing team mismatch: $name" >&2; exit 1; }
  grep -Fq '(runtime)' <<< "$details" || { echo "Hardened runtime missing: $name" >&2; exit 1; }
  if grep -Fq 'Signature=adhoc' <<< "$details"; then
    echo "Release contains ad-hoc executable: $name" >&2; exit 1
  fi
done
codesign --verify --deep --strict "$bundle"
xcrun stapler validate "$bundle"
spctl --assess --type execute --verbose=2 "$bundle"
echo 'macOS release identity, hardened runtime and stapled notarization verified.'
