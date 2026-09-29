#!/usr/bin/env bash
# Check names/presence before starting six expensive package jobs. Never log values.
set -euo pipefail
missing=0
for key in APPLE_CERTIFICATE APPLE_CERTIFICATE_PASSWORD APPLE_SIGNING_IDENTITY APPLE_TEAM_ID APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_P8; do
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
echo 'Apple credential names are present; signing and notarization must still pass.'
