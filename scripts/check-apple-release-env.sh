#!/usr/bin/env bash
# Keep strict Apple validation, with an explicit compatibility mode for the
# established GitHub updater-signed releases. Never log credential values.
set -euo pipefail
allow_legacy=false
case "${1:-}" in
  '') ;;
  --allow-legacy) allow_legacy=true ;;
  *) echo 'Usage: check-apple-release-env.sh [--allow-legacy]' >&2; exit 2 ;;
esac
test "$#" -le 1 || exit 2
missing=0
present=0
keys=(APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY APPLE_TEAM_ID APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_P8)
for key in "${keys[@]}"; do
  if [ -n "${!key:-}" ]; then present=$((present + 1)); fi
done
mode_output() {
  printf 'apple_mode=%s\n' "$1"
  if [ -n "${GITHUB_OUTPUT:-}" ]; then
    printf 'apple_mode=%s\n' "$1" >> "$GITHUB_OUTPUT"
  fi
}
if [ "$present" = 0 ] && [ "$allow_legacy" = true ]; then
  mode_output legacy
  echo 'Using the established updater-signed release workflow; macOS uses ad-hoc code signing, not Developer ID or Apple notarization.'
  exit 0
fi
for key in "${keys[@]}"; do
  if [ -z "${!key:-}" ]; then
    printf 'Missing Apple release credential: %s\n' "$key" >&2
    missing=1
  fi
done
test "$missing" = 0 || exit 1
case "$APPLE_SIGNING_IDENTITY" in
  'Developer ID Application:'*) ;;
  *) echo 'A Developer ID Application identity is required for macOS release.' >&2; exit 1 ;;
esac
mode_output notarized
echo 'Apple credential names are present; signing and notarization must still pass.'
