"""Offline manifest/publication tests; no accounts, credentials or remote writes."""
import hashlib
import json
from pathlib import Path
import tempfile
import unittest
from updater_release import stage_updaters, updater_names, UPDATER_MANIFESTS


class UpdaterReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="routedeck-updater-test-")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        (self.root / "src-tauri").mkdir()
        self.config = {"bundle": {"createUpdaterArtifacts": True}, "plugins": {"updater": {"pubkey": "synthetic-public-key"}}}
        self.write_config()
        self.artifacts = self.root / "artifacts"
        self.output = self.root / "staged"
        self.output.mkdir()
        self.verified = []
        for folder, (_, original, _) in updater_names("0.7.0").items():
            directory = self.artifacts / folder
            directory.mkdir(parents=True)
            names = [original]
            if folder.startswith("windows-"):
                names.append(original.replace("-setup.exe", "_en-US.msi"))
            for name in names:
                (directory / name).write_bytes(f"Synthetic package: {folder}/{name}".encode())
                (directory / (name + ".sig")).write_text("synthetic signature", encoding="utf-8")
                if name.endswith(".msi"):
                    (self.output / name).write_bytes((directory / name).read_bytes())

    def write_config(self):
        (self.root / "src-tauri/tauri.conf.json").write_text(json.dumps(self.config), encoding="utf-8")

    def prepare(self, verify=None):
        return stage_updaters(self.root, self.artifacts, self.output, "0.7.0", "Release notes", verify=verify or (lambda root, asset, signature: self.verified.append(asset.name)), published_at="2026-09-05T00:00:00Z")

    def test_channels_have_independent_urls_but_identical_versions_hashes_signatures(self):
        self.prepare()
        self.assertEqual(len(self.verified), 8)
        github = json.loads((self.output / "latest.json").read_text())
        gitee = json.loads((self.output / "latest-gitee.json").read_text())
        self.assertEqual(github["version"], gitee["version"])
        self.assertFalse((self.output / "latest-serylane.json").exists(),
                         "Historical releases must not gain new manifest assets")
        self.assertEqual(len(github["platforms"]), 10)
        for target, data in github["platforms"].items():
            mirror = gitee["platforms"][target]
            self.assertTrue(data["url"].startswith("https://github.com/CMMUU/routedeck/releases/download/v0.7.0/"))
            self.assertTrue(mirror["url"].startswith("https://gitee.com/cmmuu/routedeck/releases/download/v0.7.0/"))
            for key in ["sha256", "signature", "size"]:
                self.assertEqual(data[key], mirror[key])
            asset = self.output / data["url"].rsplit("/", 1)[-1]
            self.assertEqual(data["sha256"], hashlib.sha256(asset.read_bytes()).hexdigest())

    def test_missing_signature_or_failed_verification_prevents_manifest_publication(self):
        def reject(*args):
            raise ValueError("Wrong signing key")
        with self.assertRaisesRegex(ValueError, "Wrong signing key"):
            self.prepare(verify=reject)
        self.assertFalse((self.output / "latest.json").exists())
        next(self.artifacts.rglob("*.sig")).unlink()
        with self.assertRaisesRegex(ValueError, "Missing"):
            self.prepare()
        self.assertFalse((self.output / "latest-gitee.json").exists())

    def test_unsigned_configuration_fails_closed(self):
        self.config["plugins"]["updater"]["pubkey"] = ""
        self.write_config()
        with self.assertRaisesRegex(ValueError, "public key"):
            self.prepare()

    def test_historical_release_without_updater_remains_supported(self):
        self.config["bundle"]["createUpdaterArtifacts"] = False
        self.write_config()
        self.assertEqual(self.prepare(), [])

    def test_serylane_packages_keep_byte_identical_signed_legacy_aliases(self):
        self.config["productName"] = "Serylane"
        self.write_config()
        for folder in self.artifacts.iterdir():
            for asset in list(folder.iterdir()):
                asset.rename(asset.with_name(asset.name.replace("RouteDeck", "Serylane", 1)))
        self.prepare()
        manifest = json.loads((self.output / "latest.json").read_text())
        self.assertTrue(all((self.output / name).is_file() for name in UPDATER_MANIFESTS))
        github = json.loads((self.output / "latest-serylane.json").read_text())
        gitee = json.loads((self.output / "latest-serylane-gitee.json").read_text())
        hk = json.loads((self.output / "latest-serylane-hk.json").read_text())
        legacy_gitee = json.loads((self.output / "latest-gitee.json").read_text())
        self.assertEqual(set(github["platforms"]), set(manifest["platforms"]))
        for target, data in manifest["platforms"].items():
            legacy = data["url"].rsplit("/", 1)[-1]
            public = legacy.replace("RouteDeck", "Serylane", 1)
            if target.endswith("-msi"):
                # The ordinary package collector provides the public MSI.
                arch = "arm64" if "aarch64" in target else "x64"
                public_bytes = (self.artifacts / f"windows-{arch}" / public).read_bytes()
            else:
                public_bytes = (self.output / public).read_bytes()
            self.assertEqual((self.output / legacy).read_bytes(), public_bytes)
            self.assertEqual((self.output / (legacy + ".sig")).read_bytes(), (self.output / (public + ".sig")).read_bytes())
            self.assertTrue(data["url"].startswith("https://github.com/CMMUU/routedeck/"))
            self.assertTrue(legacy_gitee["platforms"][target]["url"].endswith("/" + legacy))
            self.assertEqual(github["platforms"][target]["url"],
                             f"https://github.com/CMMUU/serylane/releases/download/v0.7.0/{public}")
            self.assertEqual(gitee["platforms"][target]["url"],
                             f"https://gitee.com/cmmuu/serylane/releases/download/v0.7.0/{public}")
            self.assertEqual(hk["platforms"][target]["url"], f"https://files.cmmuu.com/releases/serylane/v0.7.0/{public}")
            for current in (github, gitee, hk):
                self.assertEqual(current["version"], manifest["version"])
                for key in ("sha256", "size", "signature"):
                    self.assertEqual(current["platforms"][target][key], data[key])
            self.assertTrue((self.output / public).is_file(), "Every current URL names a staged package")
