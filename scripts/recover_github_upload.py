"""Bounded upload transport; all publication decisions remain in tagged source."""
import http.client
import json
import os
from pathlib import Path
import re
import ssl
import sys
import time
from urllib.parse import urlencode


class UploadError(Exception):
    pass


def upload(release_id, path, label, token, connection_factory=http.client.HTTPSConnection):
    if not isinstance(release_id, int) or release_id <= 0:
        raise UploadError("Invalid release identity")
    if not re.fullmatch(r"[A-Za-z0-9_.-]+", path.name) or not token:
        raise UploadError("Invalid upload name or authentication")
    size = path.stat().st_size
    if size <= 0 or path.is_symlink():
        raise UploadError("Invalid upload file")
    query = {"name": path.name}
    if label:
        query["label"] = label
    endpoint = f"/repos/CMMUU/serylane/releases/{release_id}/assets?{urlencode(query)}"
    connection = connection_factory("uploads.github.com", timeout=30, context=ssl.create_default_context())
    started = time.monotonic()
    sent = 0
    progress = 0
    print(f"Uploading {path.name}: {size} bytes", flush=True)
    try:
        connection.putrequest("POST", endpoint)
        for key, value in {
            "Authorization": f"Bearer {token}",
            "Accept": "application/vnd.github+json",
            "User-Agent": "Serylane-Verified-Release-Recovery",
            "Content-Type": "application/octet-stream",
            "Content-Length": str(size),
        }.items():
            connection.putheader(key, value)
        connection.endheaders()
        with path.open("rb") as stream:
            while block := stream.read(1024 * 1024):
                if time.monotonic() - started > 300:
                    raise UploadError("Upload deadline exceeded; inspect draft before resuming")
                connection.send(block)
                sent += len(block)
                percent = sent * 100 // size
                if percent >= progress + 25 or sent == size:
                    progress = percent
                    print(f"Upload body {path.name}: {sent}/{size} bytes", flush=True)
        if sent != size:
            raise UploadError("Upload file changed; inspect draft before resuming")
        # GitHub may acknowledge storage/digest processing later than the body.
        # Keep the send idle timeout short, but allow bounded server confirmation.
        connection.sock.settimeout(120)
        response = connection.getresponse()
        if response.status != 201:
            raise UploadError(f"Upload HTTP {response.status}; inspect draft before resuming")
        data = response.read(1024 * 1024 + 1)
        if len(data) > 1024 * 1024:
            raise UploadError("Oversized upload response")
        asset = json.loads(data)
        if not isinstance(asset, dict) or asset.get("name") != path.name:
            raise UploadError("Unexpected upload response")
        print(f"Upload response {path.name}: HTTP 201 in {time.monotonic()-started:.1f}s; awaiting publisher hash verification", flush=True)
        return asset
    except (OSError, http.client.HTTPException, ValueError):
        # Do not expose headers, tokens, raw server bodies, or retry an uncertain POST.
        raise UploadError("Upload transport failed; inspect draft before resuming") from None
    finally:
        connection.close()


def main():
    if len(sys.argv) < 3 or sys.argv[1] != "--source":
        raise UploadError("Expected verified original source directory")
    root = Path(sys.argv[2]).resolve()
    sys.path.insert(0, str(root / "scripts"))
    import publish_github_release as original
    if Path(original.__file__).resolve() != root / "scripts/publish_github_release.py":
        raise UploadError("Unexpected original publisher")

    class RecoveryGitHub(original.GitHub):
        def upload(self, release_id, path):
            try:
                return upload(release_id, path, original.asset_label(path.name), os.environ.get("GH_TOKEN", ""))
            except UploadError as error:
                raise original.ReleaseError(str(error)) from None

    # Only the POST transport changes. main() still checks clean original source,
    # immutable tag, all six packages, signatures, hashes, conflicts and final set.
    original.GitHub = RecoveryGitHub
    sys.argv = [str(root / "scripts/publish_github_release.py"), *sys.argv[3:]]
    try:
        original.main()
    except original.ReleaseError as error:
        raise UploadError(str(error)) from None
    except (OSError, ValueError, KeyError):
        raise UploadError("Original publisher stopped safely; inspect its verified progress and draft assets") from None


if __name__ == "__main__":
    try:
        main()
    except UploadError as error:
        raise SystemExit(str(error))
