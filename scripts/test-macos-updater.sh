#!/usr/bin/env bash
# Real signed bundle / Tauri updater tests, confined to disposable directories.
set -euo pipefail
cd "$(dirname "$0")/.."
test "$(uname -s)" = Darwin
if [ $# -gt 0 ]; then
  test -d "$1"
  export SERYLANE_UPDATE_BUNDLE="$(cd "$(dirname "$1")" && pwd)/$(basename "$1")"
  cargo test --manifest-path src-tauri/Cargo.toml --lib macos_native_release_artifact_uses_product_backup_validator -- --ignored --nocapture
else
  cargo test --manifest-path src-tauri/Cargo.toml --lib macos_native_ -- --nocapture
fi
