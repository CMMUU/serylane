#!/usr/bin/env bash
set -euo pipefail
bundle=${1:?Usage: verify-macos-release.sh /path/to/Serylane.app}
bash "$(dirname "$0")/verify-macos-identity.sh" "$bundle"
xcrun stapler validate "$bundle"
spctl --assess --type execute --verbose=2 "$bundle"
echo 'macOS release identity, hardened runtime and stapled notarization verified.'
