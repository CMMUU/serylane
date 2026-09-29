import os
from pathlib import Path
import subprocess
import tempfile
import unittest


class AppleReleaseEnvironmentTests(unittest.TestCase):
    keys = ("APPLE_CERTIFICATE", "APPLE_CERTIFICATE_PASSWORD", "APPLE_SIGNING_IDENTITY",
            "APPLE_TEAM_ID", "APPLE_API_KEY", "APPLE_API_ISSUER", "APPLE_API_KEY_P8")

    def run_check(self, values, *args):
        env = {k: v for k, v in os.environ.items()
               if k not in self.keys and k not in ("GITHUB_OUTPUT", "GITHUB_STEP_SUMMARY")}
        env.update(values)
        return subprocess.run(["bash", str(Path(__file__).with_name("check-apple-release-env.sh")), *args],
                              env=env, text=True, capture_output=True, timeout=5)

    def test_all_missing_names_are_reported_without_building(self):
        result = self.run_check({})
        self.assertNotEqual(result.returncode, 0)
        for key in self.keys:
            self.assertIn(key, result.stderr)

    def test_presence_does_not_claim_credentials_or_notarization_valid(self):
        values = dict.fromkeys(self.keys, "sensitive-fixture-never-print")
        values["APPLE_SIGNING_IDENTITY"] = "Developer ID Application: Fixture"
        result = self.run_check(values)
        self.assertEqual(result.returncode, 0)
        self.assertNotIn("sensitive-fixture", result.stdout + result.stderr)
        self.assertIn("must still pass", result.stdout)

    def test_adhoc_signing_is_not_a_release_identity(self):
        values = dict.fromkeys(self.keys, "sensitive-fixture-never-print")
        values["APPLE_SIGNING_IDENTITY"] = "-"
        result = self.run_check(values)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn("sensitive-fixture", result.stdout + result.stderr)

    def test_explicit_compatibility_mode_preserves_the_previous_github_workflow(self):
        result = self.run_check({}, "--allow-legacy")
        self.assertEqual(result.returncode, 0)
        self.assertIn("apple_mode=legacy", result.stdout)
        self.assertIn("not Developer ID or Apple notarization", result.stdout)

    def test_partial_credentials_never_silently_downgrade_to_legacy(self):
        for key in self.keys:
            with self.subTest(key=key):
                result = self.run_check({key: "sensitive-fixture-never-print"}, "--allow-legacy")
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn("apple_mode=", result.stdout)
                self.assertNotIn("sensitive-fixture", result.stdout + result.stderr)

    def test_fully_configured_compatibility_mode_still_requires_apple_verification(self):
        values = dict.fromkeys(self.keys, "sensitive-fixture-never-print")
        values["APPLE_SIGNING_IDENTITY"] = "Developer ID Application: Fixture"
        result = self.run_check(values, "--allow-legacy")
        self.assertEqual(result.returncode, 0)
        self.assertIn("apple_mode=notarized", result.stdout)
        self.assertNotIn("apple_mode=legacy", result.stdout)
        values["APPLE_SIGNING_IDENTITY"] = "-"
        self.assertNotEqual(self.run_check(values, "--allow-legacy").returncode, 0)

    def test_github_output_only_receives_the_selected_mode(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "output"
            result = self.run_check({"GITHUB_OUTPUT": str(output)}, "--allow-legacy")
            self.assertEqual(result.returncode, 0)
            self.assertEqual(output.read_text(), "apple_mode=legacy\n")

    def test_unknown_options_fail_closed(self):
        for args in [("--legacy",), ("--allow-legacy", "extra")]:
            self.assertNotEqual(self.run_check({}, *args).returncode, 0)

    def test_workflow_preserves_updater_signature_and_checks_both_macos_paths(self):
        root = Path(__file__).resolve().parents[1]
        workflow = (root / ".github/workflows/release.yml").read_text()
        self.assertIn("apple_mode: ${{ steps.apple_signing.outputs.apple_mode }}", workflow)
        self.assertIn("run: bash scripts/check-apple-release-env.sh --allow-legacy", workflow)
        self.assertIn("if: runner.os == 'macOS' && needs.source.outputs.apple_mode == 'notarized'", workflow)
        self.assertIn('if [ -z "$TAURI_SIGNING_PRIVATE_KEY" ]; then', workflow)
        self.assertIn('./scripts/verify-macos-release.sh src-tauri/target/release/bundle/macos/Serylane.app', workflow)
        self.assertIn('./scripts/verify-macos-layout.sh src-tauri/target/release/bundle/macos/Serylane.app', workflow)
        self.assertNotIn('continue-on-error: true', workflow)


if __name__ == "__main__":
    unittest.main()
