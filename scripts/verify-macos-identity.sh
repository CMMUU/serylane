#!/usr/bin/env bash
# Read-only identity gate for local candidates and notarized releases alike.
# Passing this gate alone does not assert notarization or successful TUN.
set -euo pipefail
: "${APPLE_TEAM_ID:?Set the expected Developer ID team}"
bundle=${1:?Usage: verify-macos-identity.sh /path/to/Serylane.app}
bash "$(dirname "$0")/verify-macos-layout.sh" "$bundle"
requirement='=anchor apple generic and certificate leaf[field.1.2.840.113635.100.6.1.13] exists'
for name in serylane mihomo-tun-helper serylane-login-helper mihomo; do
  executable="$bundle/Contents/MacOS/$name"
  codesign --verify --strict -R "$requirement" "$executable"
  details=$(codesign -d --verbose=4 "$executable" 2>&1)
  grep -Fxq "TeamIdentifier=$APPLE_TEAM_ID" <<< "$details" || { echo "Signing team mismatch: $name" >&2; exit 1; }
  grep -Fq '(runtime)' <<< "$details" || { echo "Hardened runtime missing: $name" >&2; exit 1; }
done
codesign --verify --deep --strict -R "$requirement" "$bundle"
echo 'Developer ID, matching teams and hardened runtime verified; notarization and TUN runtime require separate acceptance.'
