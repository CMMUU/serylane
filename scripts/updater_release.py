"""Stage signed, byte-identical updater packages and channel-specific manifests."""
import datetime
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess


LEGACY_CHANNELS = (
    ("latest.json", "https://github.com/CMMUU/routedeck"),
    ("latest-gitee.json", "https://gitee.com/cmmuu/routedeck"),
)
SERYLANE_CHANNELS = (
    ("latest-serylane.json", "https://github.com/CMMUU/serylane"),
    # Canonical Gitee path was renamed with the original repository identity.
    ("latest-serylane-gitee.json", "https://gitee.com/cmmuu/serylane"),
)
HK_CHANNEL = ("latest-serylane-hk.json", "https://files.cmmuu.com/releases/serylane")
UPDATER_MANIFESTS = frozenset(name for name, _ in (*LEGACY_CHANNELS, *SERYLANE_CHANNELS, HK_CHANNEL))


def updater_names(version, product_name="RouteDeck"):
    prefix = f"RouteDeck_{version}"
    source_prefix = f"{product_name}_{version}"
    return {
        "macos-aarch64": ("darwin-aarch64", f"{product_name}.app.tar.gz", f"{prefix}_aarch64.app.tar.gz"),
        "macos-x64": ("darwin-x86_64", f"{product_name}.app.tar.gz", f"{prefix}_x64.app.tar.gz"),
        "windows-x64": ("windows-x86_64", f"{source_prefix}_x64-setup.exe", f"{prefix}_x64-setup.exe"),
        "windows-arm64": ("windows-aarch64", f"{source_prefix}_arm64-setup.exe", f"{prefix}_arm64-setup.exe"),
        "linux-x64": ("linux-x86_64", f"{source_prefix}_amd64.AppImage", f"{prefix}_amd64.AppImage"),
        "linux-arm64": ("linux-aarch64", f"{source_prefix}_aarch64.AppImage", f"{prefix}_aarch64.AppImage"),
    }


def one_file(folder, name):
    matches = list(folder.rglob(name))
    if len(matches) != 1 or matches[0].is_symlink() or not matches[0].is_file() or not matches[0].stat().st_size:
        raise ValueError(f"Missing, duplicated or invalid updater asset: {folder.name}/{name}")
    if matches[0].stat().st_size > 256 * 1024 * 1024:
        raise ValueError(f"Updater asset exceeds 256 MiB: {name}")
    return matches[0]


def verify_signature(root, asset, signature):
    result = subprocess.run([os.environ.get("NODE_BINARY", "node"), str(root / "scripts/verify-update-signature.mjs"),
                             str(root / "src-tauri/tauri.conf.json"), str(asset), str(signature)],
                            capture_output=True, check=False)
    if result.returncode:
        raise ValueError(f"Updater signature does not verify with the application public key: {asset.name}")


def stage_updaters(root, artifacts, output, version, notes, verify=verify_signature, published_at=None):
    config = json.loads((root / "src-tauri/tauri.conf.json").read_text(encoding="utf-8"))
    if config.get("bundle", {}).get("createUpdaterArtifacts") is not True:
        return []  # Historical releases without an updater remain reproducible.
    if not config.get("plugins", {}).get("updater", {}).get("pubkey"):
        raise ValueError("An updater public key is required")
    if not re.fullmatch(r"(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)", version):
        raise ValueError("Invalid updater version")
    if published_at is None:
        stamp = subprocess.run(["git", "-C", str(root), "show", "-s", "--format=%cI", "HEAD"], capture_output=True, text=True, check=True)
        published_at = datetime.datetime.fromisoformat(stamp.stdout.strip()).astimezone(datetime.timezone.utc).isoformat().replace("+00:00", "Z")
    platforms = {}
    staged = []
    product_name = config.get("productName", "RouteDeck")
    if product_name not in {"RouteDeck", "Serylane"}:
        raise ValueError("Unrecognized package product name")
    for folder, (target, source_name, name) in updater_names(version, product_name).items():
        package = one_file(artifacts / folder, source_name)
        signature = one_file(artifacts / folder, source_name + ".sig")
        if signature.stat().st_size > 4096:
            raise ValueError("Updater signature is too large")
        verify(root, package, signature)
        public_name = name.replace("RouteDeck_", product_name + "_", 1)
        copies = [(package, name), (signature, name + ".sig")]
        if public_name != name:
            copies += [(package, public_name), (signature, public_name + ".sig")]
        for source, destination_name in copies:
            destination = output / destination_name
            if destination.exists() and destination.read_bytes() != source.read_bytes():
                raise ValueError(f"Updater conflicts with installer: {destination_name}")
            if not destination.exists():
                shutil.copyfile(source, destination)
            staged.append(destination)
        with package.open("rb") as stream:
            digest = hashlib.file_digest(stream, "sha256").hexdigest()
        platforms[target] = {"name": name, "signature": signature.read_text(encoding="utf-8").strip(),
                             "size": package.stat().st_size, "sha256": digest}
        if target.startswith("windows-"):
            platforms[target + "-nsis"] = platforms[target].copy()
            msi_name = name.replace("-setup.exe", "_en-US.msi")
            msi_source_name = source_name.replace("-setup.exe", "_en-US.msi")
            msi = one_file(artifacts / folder, msi_source_name)
            msi_signature = one_file(artifacts / folder, msi_source_name + ".sig")
            if msi_signature.stat().st_size > 4096:
                raise ValueError("Updater signature is too large")
            verify(root, msi, msi_signature)
            # The public MSI is in the installer set; legacy updater names are
            # strict compatibility aliases of the same signed bytes.
            for destination_name in dict.fromkeys((msi_name, msi_source_name)):
                for source, name_to_copy in ((msi, destination_name), (msi_signature, destination_name + ".sig")):
                    destination = output / name_to_copy
                    if destination.exists() and destination.read_bytes() != source.read_bytes():
                        raise ValueError("MSI alias conflicts with installer")
                    if not destination.exists():
                        shutil.copyfile(source, destination)
                    staged.append(destination)
            with msi.open("rb") as stream:
                digest = hashlib.file_digest(stream, "sha256").hexdigest()
            platforms[target + "-msi"] = {"name": msi_name, "signature": msi_signature.read_text(encoding="utf-8").strip(), "size": msi.stat().st_size, "sha256": digest}
    # Do not rewrite these URLs to the new repository slug: <= 0.7.6 clients
    # validate the exact legacy URL. GitHub redirects it to CMMUU/serylane.
    # The public installers and links use Serylane; these are signed-byte aliases.
    channels = [(name, base, False) for name, base in LEGACY_CHANNELS]
    if product_name == "Serylane":
        channels.extend((name, base, True) for name, base in (*SERYLANE_CHANNELS, HK_CHANNEL))
    for manifest_name, base, current_brand in channels:
        manifest = {"version": version, "notes": notes, "pub_date": published_at, "platforms": {}}
        for target, data in platforms.items():
            manifest["platforms"][target] = {key: value for key, value in data.items() if key != "name"}
            name = data["name"].replace("RouteDeck_", "Serylane_", 1) if current_brand else data["name"]
            if not (output / name).is_file():
                raise ValueError(f"Manifest asset was not staged: {name}")
            prefix = f"{base}/v{version}" if manifest_name == HK_CHANNEL[0] else f"{base}/releases/download/v{version}"
            manifest["platforms"][target]["url"] = f"{prefix}/{name}"
        path = output / manifest_name
        path.write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n")
        staged.append(path)
    return staged
