import os
from pathlib import Path
import subprocess
import unittest


class AppleReleaseEnvironmentTests(unittest.TestCase):
    keys = ("APPLE_CERTIFICATE", "APPLE_CERTIFICATE_PASSWORD", "APPLE_SIGNING_IDENTITY",
            "APPLE_TEAM_ID", "APPLE_API_KEY", "APPLE_API_ISSUER", "APPLE_API_KEY_P8")

    def run_check(self, values):
        env = {k: v for k, v in os.environ.items() if k not in self.keys}
        env.update(values)
        return subprocess.run(["bash", str(Path(__file__).with_name("check-apple-release-env.sh"))],
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


if __name__ == "__main__":
    unittest.main()
