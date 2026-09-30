"""Exercise the real shell gates without signing or accessing a keychain."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


class MacOSIdentityGateTests(unittest.TestCase):
    def run_gate(self, *, release=False, team="FIXTURETEAM", runtime=True, developer=True, notarized=True):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ("verify-macos-identity.sh", "verify-macos-release.sh"):
                shutil.copyfile(Path(__file__).with_name(name), root / name)
            (root / "verify-macos-layout.sh").write_text("exit 0\n")
            mock = root / "bin"
            mock.mkdir()
            (mock / "codesign").write_text('''#!/bin/bash
set -eu
echo "codesign $*" >> "$TRACE"
if [ "$1" = '-d' ]; then
  echo "TeamIdentifier=$ACTUAL_TEAM"
  test "$HAS_RUNTIME" != true || echo 'flags=0x10000(runtime)'
else
  # Every strict signature gate must demand Developer ID explicitly; a team
  # name or an Apple Distribution certificate alone is insufficient.
  case "$*" in *'100.6.1.13'*) ;; *) exit 91 ;; esac
  test "$DEVELOPER_ID" = true || exit 92
fi
''')
            (mock / "xcrun").write_text('''#!/bin/bash
echo "xcrun $*" >> "$TRACE"
test "$NOTARIZED" = true
''')
            (mock / "spctl").write_text('#!/bin/bash\necho "spctl $*" >> "$TRACE"\n')
            for script in mock.iterdir():
                script.chmod(0o755)
            env = dict(os.environ, PATH=f"{mock}{os.pathsep}{os.environ['PATH']}",
                       APPLE_TEAM_ID="FIXTURETEAM", ACTUAL_TEAM=team, TRACE=str(root / "trace"),
                       HAS_RUNTIME=str(runtime).lower(), DEVELOPER_ID=str(developer).lower(),
                       NOTARIZED=str(notarized).lower())
            gate = "verify-macos-release.sh" if release else "verify-macos-identity.sh"
            result = subprocess.run(["bash", str(root / gate), str(root / "Serylane.app")],
                                    env=env, capture_output=True, text=True, timeout=5)
            return result, (root / "trace").read_text()

    def test_candidate_checks_every_executable_without_claiming_notarization(self):
        result, trace = self.run_gate()
        self.assertEqual(result.returncode, 0, result.stderr)
        for name in ("serylane", "mihomo-tun-helper", "serylane-login-helper", "mihomo"):
            self.assertIn(f"Contents/MacOS/{name}\n", trace)
        self.assertIn("notarization and TUN runtime require separate acceptance", result.stdout)
        self.assertNotIn("xcrun", trace)

    def test_invalid_identity_stops_before_notarization(self):
        for kwargs in ({"team": "OTHERTEAM"}, {"team": "FIXTURETEAM_suffix"}, {"runtime": False}, {"developer": False}):
            with self.subTest(**kwargs):
                result, trace = self.run_gate(release=True, **kwargs)
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn("xcrun", trace)

    def test_release_requires_stapling_and_gatekeeper(self):
        result, trace = self.run_gate(release=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("xcrun stapler validate", trace)
        self.assertIn("spctl --assess --type execute", trace)
        result, trace = self.run_gate(release=True, notarized=False)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("spctl", trace)


if __name__ == "__main__":
    unittest.main()
