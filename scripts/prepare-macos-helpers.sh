#!/usr/bin/env bash
set -euo pipefail
directory=${1:?Usage: prepare-macos-helpers.sh compiled-binary-directory}
test "$(uname -s)" = Darwin || { echo 'macOS build preparation requires Darwin.' >&2; exit 1; }
helpers=(mihomo-tun-helper serylane-login-helper)
for name in "${helpers[@]}"; do
  binary="$directory/$name"
  test -f "$binary" && test -x "$binary" && test ! -L "$binary" || {
    echo "Missing regular compiled helper: $name" >&2; exit 1;
  }
  file "$binary" | grep -q 'Mach-O' || { echo "Not a Mach-O helper: $name" >&2; exit 1; }
done
# The arm64 linker supplies ad-hoc signatures; the Intel linker may not.
# Tauri can sign the main executable before additional binaries, which fails
# when the containing bundle has an unsigned nested helper. Prepare compiled
# inputs BEFORE bundling. Tauri still performs final signing/notarization and
# only then generates updater signatures and installer hashes.
for name in "${helpers[@]}"; do
  codesign --force --sign - --timestamp=none "$directory/$name"
  codesign --verify --strict "$directory/$name"
done
echo 'Compiled macOS helpers prepared; final bundle verification is still required.'
