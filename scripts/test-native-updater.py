#!/usr/bin/env python3
"""Gate a signed candidate through the product coordinator on disposable paths."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parent.parent
BASELINE = "0.7.27"  # Reproducible published baseline containing the old preflight regression.

def run(*args, **kwargs):
    subprocess.run(list(map(str, args)), check=True, cwd=ROOT, **kwargs)

def download(url, output):
    run("curl", "--fail", "--location", "--retry", "2", "--connect-timeout", "15", "--max-time", "240", "--silent", "--show-error", url, "--output", output)

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("archive", type=Path)
    args = parser.parse_args()
    candidate = args.archive.resolve(strict=True)
    signature = Path(str(candidate) + ".sig")
    run("node", "scripts/verify-update-signature.mjs", "src-tauri/tauri.conf.json", candidate, signature)
    os_name = "darwin" if platform.system() == "Darwin" else "linux"
    if platform.system() not in ("Darwin", "Linux"):
        raise SystemExit("Use the guarded Windows installer acceptance script on Windows")
    arch = {"arm64": "aarch64", "aarch64": "aarch64", "x86_64": "x86_64"}[platform.machine()]
    key = f"{os_name}-{arch}"
    with tempfile.TemporaryDirectory(prefix="serylane-updater-gate-") as temporary:
        root = Path(temporary)
        manifest = root / "manifest.json"
        base = f"https://github.com/CMMUU/serylane/releases/download/v{BASELINE}"
        download(f"{base}/latest-serylane.json", manifest)
        document = json.loads(manifest.read_text())
        assert document["version"] == BASELINE
        entry = document["platforms"][key]
        filename = entry["url"].rsplit("/", 1)[1]
        suffix = {"darwin-aarch64": "aarch64.app.tar.gz", "darwin-x86_64": "x64.app.tar.gz", "linux-aarch64": "aarch64.AppImage", "linux-x86_64": "amd64.AppImage"}[key]
        assert filename == f"Serylane_{BASELINE}_{suffix}"
        assert entry["url"] == f"{base}/{filename}"
        archive = root / filename
        download(entry["url"], archive)
        data = archive.read_bytes()
        assert len(data) == entry["size"] and hashlib.sha256(data).hexdigest() == entry["sha256"]
        old_signature = root / "baseline.sig"
        old_signature.write_text(entry["signature"])
        run("node", "scripts/verify-update-signature.mjs", "src-tauri/tauri.conf.json", archive, old_signature)
        if os_name == "darwin":
            with tarfile.open(archive) as tar:
                for entry in tar.getmembers():
                    path = Path(entry.name)
                    assert not path.is_absolute() and ".." not in path.parts
                    assert path.parts[0] == "Serylane.app" and not entry.issym() and not entry.islnk()
            run("tar", "-xzf", archive, "-C", root)
            baseline = root / "Serylane.app"
        else:
            archive.chmod(0o755)
            baseline = archive
        env = dict(os.environ, SERYLANE_UPDATE_ARCHIVE=str(candidate), SERYLANE_UPDATE_BASELINE=str(baseline), SERYLANE_UPDATE_BASELINE_VERSION=BASELINE)
        run("cargo", "test", "--manifest-path", "src-tauri/Cargo.toml", "--lib", "app_update::native_tests::updater_native_release_artifact_transaction", "--", "--ignored", "--exact", "--nocapture", env=env)
        print(f"PASS: {key} published {BASELINE} -> signed candidate -> new-process confirmation -> higher-version fixture")

if __name__ == "__main__":
    main()
