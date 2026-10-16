#!/usr/bin/env python3
"""Gate a signed candidate through the product coordinator on disposable paths."""
import argparse
import hashlib
import json
import os
import re
from pathlib import Path
import platform
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parent.parent
REGRESSION_BASELINE = "0.7.27"

def stable(value):
    if not isinstance(value, str) or not re.fullmatch(r"(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)\.(?:0|[1-9][0-9]*)", value):
        raise ValueError("invalid stable version")
    return tuple(map(int, value.split(".")))

def select_baselines(releases, candidate):
    current = stable(candidate)
    versions = set()
    for release in releases:
        if release.get("draft") or release.get("prerelease"):
            continue
        tag = release.get("tag_name", "")
        if not tag.startswith("v"):
            continue
        try:
            if stable(tag[1:]) < current:
                versions.add(tag[1:])
        except ValueError:
            continue
    latest = sorted(versions, key=stable, reverse=True)[:2]
    if len(latest) != 2:
        raise ValueError("two published baseline versions are required")
    return list(dict.fromkeys(latest + [REGRESSION_BASELINE]))

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
    candidate_version = json.loads((ROOT / "src-tauri/tauri.conf.json").read_text())["version"]
    with tempfile.TemporaryDirectory(prefix="serylane-baseline-list-") as temporary:
        index = Path(temporary) / "releases.json"
        download("https://api.github.com/repos/CMMUU/serylane/releases?per_page=100", index)
        baselines = select_baselines(json.loads(index.read_text()), candidate_version)
    for baseline_version in baselines:
        gate(candidate, key, os_name, baseline_version)

def install_environment(candidate, baseline, baseline_version):
    return dict(os.environ, SERYLANE_UPDATE_ARCHIVE=str(candidate), SERYLANE_UPDATE_BASELINE=str(baseline), SERYLANE_UPDATE_BASELINE_VERSION=baseline_version)

def gate(candidate, key, os_name, baseline_version):
    with tempfile.TemporaryDirectory(prefix="serylane-updater-gate-") as temporary:
        root = Path(temporary)
        manifest = root / "manifest.json"
        base = f"https://github.com/CMMUU/serylane/releases/download/v{baseline_version}"
        download(f"{base}/latest-serylane.json", manifest)
        document = json.loads(manifest.read_text())
        assert document["version"] == baseline_version
        entry = document["platforms"][key]
        filename = entry["url"].rsplit("/", 1)[1]
        suffix = {"darwin-aarch64": "aarch64.app.tar.gz", "darwin-x86_64": "x64.app.tar.gz", "linux-aarch64": "aarch64.AppImage", "linux-x86_64": "amd64.AppImage"}[key]
        assert filename == f"Serylane_{baseline_version}_{suffix}"
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
        if stable(baseline_version) >= (0, 7, 28):
            command = [str(baseline / "Contents/MacOS/serylane")] if os_name == "darwin" else [str(baseline), "--appimage-extract-and-run"]
            # Published old process, read-only early exit; never start its GUI/proxy.
            result = subprocess.run(command + ["--serylane-update-probe"], check=True, capture_output=True, text=True, timeout=30)
            identity = json.loads(result.stdout)
            assert identity["version"] == baseline_version and identity["identifier"] == "com.cmmuu.mihomodesktop"
            assert identity["arch"] == ("x86_64" if key.endswith("x86_64") else "aarch64")
        env = install_environment(candidate, baseline, baseline_version)
        run("cargo", "test", "--manifest-path", "src-tauri/Cargo.toml", "--lib", "app_update::native_tests::updater_native_release_artifact_transaction", "--", "--ignored", "--exact", "--nocapture", env=env)
        print(f"PASS: {key} published {baseline_version} -> signed candidate -> new-process confirmation -> higher-version fixture")

if __name__ == "__main__":
    main()
