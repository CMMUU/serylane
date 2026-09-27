#!/usr/bin/env bash
# Read-only layout/integrity gate; valid for isolated ad-hoc debug bundles too.
# This does not assert Developer ID trust, notarization or successful login/TUN.
set -euo pipefail
bundle=${1:?Usage: verify-macos-layout.sh /path/to/Serylane.app}
plist_value() { /usr/libexec/PlistBuddy -c "Print :$2" "$1"; }
assert_value() {
  actual=$(plist_value "$1" "$2")
  if [ "$actual" != "$3" ]; then
    echo "Bundle property mismatch: $2" >&2
    exit 1
  fi
}
for name in serylane mihomo-tun-helper serylane-login-helper mihomo; do
  executable="$bundle/Contents/MacOS/$name"
  test -f "$executable" && test -x "$executable" && test ! -L "$executable" || {
    echo "Missing regular executable: $name" >&2; exit 1;
  }
  file "$executable" | grep -q 'Mach-O' || { echo "Not a Mach-O executable: $name" >&2; exit 1; }
  codesign --verify --strict "$executable"
done
assert_value "$bundle/Contents/Info.plist" CFBundleIdentifier com.cmmuu.mihomodesktop
assert_value "$bundle/Contents/Info.plist" CFBundleExecutable serylane
agent="$bundle/Contents/Library/LaunchAgents/com.cmmuu.mihomodesktop.login.plist"
daemon="$bundle/Contents/Library/LaunchDaemons/com.cmmuu.mihomodesktop.tun-helper.plist"
for plist in "$agent" "$daemon"; do
  test -f "$plist" && test ! -L "$plist" || { echo 'Missing regular service plist' >&2; exit 1; }
  plutil -lint "$plist"
  assert_value "$plist" AssociatedBundleIdentifiers:0 com.cmmuu.mihomodesktop
done
assert_value "$agent" Label com.cmmuu.mihomodesktop.login
assert_value "$agent" BundleProgram Contents/MacOS/serylane-login-helper
assert_value "$agent" RunAtLoad true
assert_value "$daemon" Label com.cmmuu.mihomodesktop.tun-helper
assert_value "$daemon" BundleProgram Contents/MacOS/mihomo-tun-helper
assert_value "$daemon" MachServices:com.cmmuu.mihomodesktop.tun-helper true
codesign --verify --deep --strict "$bundle"
echo 'macOS bundle layout and code integrity verified (not a notarization or runtime acceptance result).'
