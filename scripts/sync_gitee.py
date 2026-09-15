#!/usr/bin/env python3
"""One-way repository/release synchronization. No GitHub writes; no asset replacement."""
from __future__ import annotations

import argparse
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
import hashlib
import http.client
from itertools import islice
import json
import os
from pathlib import Path
import re
import shlex
import subprocess
import sys
import tempfile
import time
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode, urljoin, urlsplit
from urllib.request import Request, HTTPRedirectHandler, build_opener
import uuid
from updater_release import UPDATER_MANIFESTS

# Website readiness marker follows the same verified-file barrier as updater manifests.
RELEASE_MANIFESTS = UPDATER_MANIFESTS | {"downloads.json"}

# Both canonical repositories now use the product slug. Legacy migration below
# is opt-in and bound to the original numeric repository identities.
SOURCE_REPOS = {"serylane": "serylane"}
REPOS = set(SOURCE_REPOS)
GH_OWNER, GE_OWNER = "CMMUU", "cmmuu"
GH_API, GE_API = "https://api.github.com", "https://gitee.com/api/v5"
MAX_JSON, MAX_ASSET = 8 * 1024 * 1024, 512 * 1024 * 1024
# Gitee documents 100 M per file. Allow 100 MiB; the service remains the final
# authority. Keep the total budget conservative, including staging old/new files.
GE_MAX_ASSET, GE_MAX_TOTAL = 100 * 1024 * 1024, 1_000_000_000
STABLE_TAG = r"v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)"
READ_ATTEMPTS = 3
# Cross-region uploads have sustained ~25 KiB/s while otherwise healthy: a
# 100 MiB AppImage cannot finish in 30 minutes. Retain a finite two-hour total
# deadline and low-speed detection; never retry a write without re-inspection.
UPLOAD_TIMEOUT = 7200
TRANSIENT_HTTP = {408, 429, 500, 502, 503, 504}
GH_STORAGE = {"release-assets.githubusercontent.com", "objects.githubusercontent.com", "github-releases.githubusercontent.com"}
GE_STORAGE = {"foruda.gitee.com"}


class SyncError(Exception):
    pass


class RetryableReadError(SyncError):
    pass


class NotFoundError(SyncError):
    pass


def configured_bytes(name, default, maximum):
    value = os.environ.get(name, "") or str(default)
    if not re.fullmatch(r"[0-9]+", value) or not 0 <= int(value) <= maximum:
        raise SyncError(f"{name} must be a nonnegative byte count within the supported limit")
    return int(value)


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


def safe_name(value):
    if (not isinstance(value, str) or not value or value in {".", ".."}
            or value.endswith((" ", ".")) or any(ord(c) < 32 or c in '/\\:<>"|?*' for c in value)):
        raise SyncError("Unsafe attachment filename")
    return value


def sha256(path):
    h = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def source_digest(asset):
    value = asset.get("digest")
    if value is None:
        return None
    if not isinstance(value, str) or not re.fullmatch(r"sha256:[0-9a-fA-F]{64}", value):
        raise SyncError("Unsupported GitHub asset digest")
    return value[7:].lower()


def stable_releases(releases):
    ordered, seen, ids = [], set(), set()
    for release in releases:
        tag = release.get("tag_name", "")
        match = re.fullmatch(STABLE_TAG, tag) if isinstance(tag, str) else None
        if release.get("draft") is not False or release.get("prerelease") is not False or not match:
            continue
        release_id = release.get("id")
        if tag in seen or type(release_id) is not int or release_id <= 0 or release_id in ids:
            raise SyncError("Ambiguous published stable releases; retention stopped")
        seen.add(tag)
        ids.add(release_id)
        ordered.append((tuple(map(int, match.groups())), release))
    return [release for _, release in sorted(ordered, key=lambda item: item[0], reverse=True)]


def asset_identity(asset):
    return tuple(asset.get(key) for key in ("id", "name", "size", "digest", "state", "updated_at"))


def release_metadata_matches(actual, expected, keys):
    for key in keys:
        left, right = actual.get(key), expected[key]
        if key == "body" and isinstance(left, str) and isinstance(right, str):
            # Gitee's web editor returns CRLF; retain Markdown whitespace and
            # the original source body when writing the API.
            left, right = left.replace("\r\n", "\n"), right.replace("\r\n", "\n")
        if left != right:
            return False
    return True


def validate_pair(repo, source, target):
    if repo not in REPOS:
        raise SyncError("Repository is outside the permitted migration scope")
    for info, owner in ((source, GH_OWNER), (target, GE_OWNER)):
        if (str(info.get("owner", {}).get("login", "")).casefold() != owner.casefold()
                or type(info.get("private")) is not bool):
            raise SyncError("Repository owner, name or visibility could not be confirmed")
    if str(source.get("full_name", "")).casefold() != f"{GH_OWNER}/{SOURCE_REPOS[repo]}".casefold():
        raise SyncError("GitHub source repository does not match the requested scope")
    target_url = f"https://gitee.com/{GE_OWNER}/{repo}".casefold()
    if (str(target.get("path", "")).casefold() != repo.casefold()
            or str(target.get("html_url", "")).casefold() not in {target_url, target_url + ".git"}):
        raise SyncError("Gitee target path does not match the requested scope")
    if source["private"] and not target["private"]:
        raise SyncError("Private GitHub source must never synchronize to a public Gitee target")


def checked_url(url, allowed_hosts):
    parsed = urlsplit(url)
    try:
        port = parsed.port
    except ValueError:
        raise SyncError("Invalid download port") from None
    if (parsed.scheme != "https" or parsed.hostname not in allowed_hosts or port not in (None, 443)
            or parsed.username or parsed.password or parsed.fragment):
        raise SyncError("Untrusted download redirect; no credentials were forwarded")
    return parsed


class Api:
    def __init__(self, service, token, storage_hosts=()):
        self.service, self.token = service, token
        self.base = GH_API if service == "github" else GE_API
        self.host = "api.github.com" if service == "github" else "gitee.com"
        self.storage_hosts = GH_STORAGE if service == "github" else GE_STORAGE | set(storage_hosts)
        self.opener = build_opener(NoRedirect())

    def fork(self):
        # Each transfer owns its HTTP handlers/connections. Credentials remain
        # in memory and all redirect/host restrictions are preserved.
        return Api(self.service, self.token, self.storage_hosts)

    def headers(self):
        headers = {"Authorization": "Bearer " + self.token, "User-Agent": "CMMUU-Gitee-Sync/1", "Accept": "application/json"}
        if self.service == "github":
            headers["X-GitHub-Api-Version"] = "2022-11-28"
        return headers

    def request(self, path, method="GET", data=None):
        for attempt in range(READ_ATTEMPTS):
            try:
                return self._request_once(path, method, data)
            except RetryableReadError:
                if attempt == READ_ATTEMPTS - 1:
                    raise
                time.sleep(2 ** attempt)

    def _request_once(self, path, method="GET", data=None):
        if self.service == "github" and method != "GET":
            raise SyncError("GitHub writes are not supported")
        if method == "DELETE" and not re.fullmatch(
                rf"/repos/{GE_OWNER}/(?:{'|'.join(sorted(REPOS))})/releases/[1-9]\d*/attach_files/[1-9]\d*", path):
            raise SyncError("Deletion is restricted to individual mirrored Gitee release attachments")
        if not path.startswith("/") or "access_token=" in path or "token=" in path:
            raise SyncError("Invalid API path")
        headers, body = self.headers(), None
        if data is not None:
            # Gitee documents access_token in formData for writes; keep it in
            # the in-memory body, never a URL, file, command argument or log.
            fields = {**data, "access_token": self.token}
            body = urlencode(fields).encode("utf-8")
            headers["Content-Type"] = "application/x-www-form-urlencoded"
        req = Request(self.base + path, data=body, headers=headers, method=method)
        try:
            with self.opener.open(req, timeout=45) as response:
                raw = response.read(MAX_JSON + 1)
                if len(raw) > MAX_JSON:
                    raise SyncError("API response exceeds the metadata limit")
                if method == "DELETE" and response.status == 204 and not raw:
                    return None
                return json.loads(raw)
        except HTTPError as error:
            # Never print response bodies, full URLs or signed query strings.
            if method == "GET" and error.code == 404:
                raise NotFoundError(f"{self.service} API returned HTTP 404") from None
            if method == "GET" and error.code in TRANSIENT_HTTP:
                raise RetryableReadError(f"{self.service} API temporarily unavailable (HTTP {error.code})") from None
            raise SyncError(f"{self.service} API returned HTTP {error.code}; no write was retried") from None
        except (URLError, TimeoutError, OSError, ValueError, http.client.HTTPException):
            error_type = RetryableReadError if method == "GET" else SyncError
            raise error_type(f"{self.service} API request failed; details suppressed to protect credentials") from None

    def pages(self, path):
        rows = []
        for page in range(1, 1001):
            data = self.request(path + ("&" if "?" in path else "?") + urlencode({"page": page, "per_page": 100}))
            if not isinstance(data, list):
                raise SyncError("Unexpected paginated API response")
            rows.extend(data)
            if len(data) < 100:
                return rows
        raise SyncError("Pagination limit exceeded")

    def download(self, path, destination, expected_size, expected_sha=None):
        for attempt in range(READ_ATTEMPTS):
            try:
                return self._download_once(path, destination, expected_size, expected_sha)
            except RetryableReadError:
                if attempt == READ_ATTEMPTS - 1:
                    raise
                time.sleep(2 ** attempt)

    def _download_once(self, path, destination, expected_size, expected_sha=None):
        if type(expected_size) is not int or not 0 <= expected_size <= MAX_ASSET:
            raise SyncError("Attachment size is outside the supported limit")
        destination = Path(destination)
        cached_sha = None
        if destination.exists():
            if destination.is_symlink() or not destination.is_file() or destination.stat().st_size != expected_size:
                raise SyncError("Existing local attachment has a different size or type")
            actual = sha256(destination)
            if expected_sha is not None and actual == expected_sha:
                return actual
            if expected_sha is not None:
                raise SyncError("Existing local attachment has a different SHA-256; it was not overwritten")
            # Older GitHub assets have no digest. Fetch their bytes again and
            # compare to the cache; a local hash alone is not source evidence.
            cached_sha = actual
        url, headers = self.base + path, self.headers()
        headers["Accept"] = "application/octet-stream"
        allowed = {self.host} | self.storage_hosts
        part = destination.with_name(destination.name + ".part-" + uuid.uuid4().hex)
        destination.parent.mkdir(parents=True, exist_ok=True)
        try:
            for redirect in range(6):
                checked_url(url, allowed)
                try:
                    response = self.opener.open(Request(url, headers=headers), timeout=90)
                    break
                except HTTPError as error:
                    if error.code in TRANSIENT_HTTP:
                        raise RetryableReadError(f"{self.service} attachment temporarily unavailable (HTTP {error.code})") from None
                    if error.code not in (301, 302, 303, 307, 308) or redirect == 5:
                        raise SyncError(f"{self.service} attachment returned HTTP {error.code}") from None
                    location = error.headers.get("Location", "")
                    if not location or self.token in location:
                        raise SyncError("Missing redirect or a redirect containing an authentication credential")
                    location = urljoin(url, location)
                    checked_url(location, allowed)
                    # A signed download URL is used only in memory. Strip auth
                    # even on same-host redirects; never follow credentialed URLs.
                    headers = {"Accept": "application/octet-stream", "User-Agent": "CMMUU-Gitee-Sync/1"}
                    url = location
            size, hasher = 0, hashlib.sha256()
            with response, part.open("xb") as stream:
                while True:
                    block = response.read(1024 * 1024)
                    if not block:
                        break
                    size += len(block)
                    if size > expected_size:
                        raise SyncError("Attachment is larger than source metadata")
                    hasher.update(block)
                    stream.write(block)
            actual = hasher.hexdigest()
            if size != expected_size or (expected_sha is not None and actual != expected_sha):
                # An idempotent CDN read can finish early or return stale bytes.
                # Retry the read, never an upload, and never commit a bad file.
                raise RetryableReadError(
                    f"{self.service} attachment size or SHA-256 validation failed for {destination.name}: "
                    f"received {size} bytes, expected {expected_size}; verified output was not written")
            if cached_sha is not None:
                if actual != cached_sha:
                    raise SyncError("Cached unhashed attachment differs from the current source; neither file was overwritten")
            else:
                part.rename(destination)
            return actual
        except (URLError, TimeoutError, OSError, ValueError, http.client.HTTPException):
            raise RetryableReadError(f"{self.service} attachment transfer failed; safe to rerun") from None
        finally:
            if part.exists():
                part.unlink()

    def upload(self, path, file):
        if self.service != "gitee" or not re.fullmatch(
                rf"/repos/{GE_OWNER}/(?:{'|'.join(sorted(REPOS))})/releases/[1-9]\d*/attach_files", path):
            raise SyncError("Uploads are restricted to the Gitee API")
        if not isinstance(self.token, str) or not self.token or any(c in self.token for c in "\r\n\0"):
            raise SyncError("Invalid Gitee upload credential")
        file = Path(file)
        name = safe_name(file.name)
        if not file.is_file() or file.stat().st_size > MAX_ASSET:
            raise SyncError("Invalid Gitee upload file")
        # curl streams multipart files and handles early HTTP responses while
        # sending. A blocking HTTPSConnection.send loop cannot read such a
        # response until the entire file has been sent, and its socket timeout
        # is not a total transfer deadline. Keep credentials in stdin config,
        # never argv, URLs, source, logs or public diagnostic artifacts.
        def quote(value):
            value = str(value).replace('\\', '\\\\').replace('"', '\\"')
            return '"' + value.replace('\n', '\\n').replace('\r', '\\r') + '"'
        try:
            with tempfile.TemporaryDirectory(prefix="gitee-upload-") as temporary:
                response_file = Path(temporary) / "response.json"
                response_file.touch(mode=0o600)
                # Quote both the curl config value and the multipart filename.
                # The payload path is absolute and owned by the verified cache.
                multipart = f'file=@{quote(file.resolve())};filename={quote(name)};type=application/octet-stream'
                options = [
                    ("url", GE_API + path), ("proto", "=https"),
                    ("connect-timeout", 30), ("max-time", UPLOAD_TIMEOUT),
                    ("speed-limit", 32768), ("speed-time", 120),
                    ("max-filesize", MAX_JSON), ("expect100-timeout", 2),
                    ("user-agent", "CMMUU-Gitee-Sync/1"),
                    ("header", "Accept: application/json"),
                    ("header", "Authorization: Bearer " + self.token),
                    ("form-string", "access_token=" + self.token),
                    ("form", multipart), ("output", response_file),
                    ("write-out", "%{http_code} %{size_upload} %{speed_upload} %{time_total}"),
                ]
                config = "silent\nshow-error\n" + "".join(f"{key} = {quote(value)}\n" for key, value in options)
                # --disable is first: no user curlrc may add redirects, retries,
                # insecure TLS or alternate endpoints. No -L or --retry here.
                result = subprocess.run(["curl", "--disable", "--config", "-"], input=config,
                                        text=True, capture_output=True, timeout=UPLOAD_TIMEOUT + 30)
                metrics = re.fullmatch(r"(\d{3}) (\d+) ([\d.]+) ([\d.]+)\s*", result.stdout)
                if metrics:
                    status, sent, speed, elapsed = metrics.groups()
                    print(f"Gitee upload transport: {name}, HTTP {status}, {sent} bytes, {elapsed}s, {speed} bytes/s", flush=True)
                if result.returncode or not metrics:
                    raise SyncError(f"Gitee upload transport exited {result.returncode}; outcome is uncertain, inspect existing attachments before retry")
                if int(metrics[1]) != 201 or response_file.stat().st_size > MAX_JSON:
                    raise SyncError(f"Gitee upload returned HTTP {metrics[1]}; do not blindly retry POST")
                response = json.loads(response_file.read_bytes())
                if not isinstance(response, dict):
                    raise SyncError("Invalid Gitee upload response")
                return response
        except (OSError, ValueError, subprocess.TimeoutExpired):
            raise SyncError("Gitee upload outcome is uncertain; rerun to inspect existing attachments") from None


def verify_manifest(files):
    manifests = [item for item in files if item["name"] == "SHA256SUMS.txt"]
    if not manifests:
        return
    path = Path(manifests[0]["path"])
    if path.stat().st_size > 65536:
        raise SyncError("SHA256SUMS.txt exceeds the expected size")
    rows = {}
    for line in path.read_text(encoding="utf-8-sig").splitlines():
        if not line:
            continue
        match = re.fullmatch(r"([0-9a-fA-F]{64}) [ *](.+)", line)
        if not match or match[2] in rows:
            raise SyncError("Malformed or ambiguous SHA256SUMS.txt")
        rows[safe_name(match[2])] = match[1].lower()
    expected = {item["name"]: item["sha256"] for item in files if item not in manifests}
    if rows != expected:
        raise SyncError("SHA256SUMS.txt does not exactly match the release attachments")


def git_credential():
    """Invoked ONLY by Git; stdout is the credential pipe, never a normal log."""
    fields = dict(line.rstrip("\n").split("=", 1) for line in sys.stdin if "=" in line)
    repo = os.environ.get("SYNC_REPO", "")
    host, path = fields.get("host"), fields.get("path", "").removesuffix(".git")
    owner = GH_OWNER if host == "github.com" else GE_OWNER
    source_repo = SOURCE_REPOS.get(repo) if host == "github.com" else repo
    if repo not in REPOS or fields.get("protocol") != "https" or host not in {"github.com", "gitee.com"} or path.casefold() != f"{owner}/{source_repo}".casefold():
        return
    token = os.environ.get("GITHUB_TOKEN" if host == "github.com" else "GITEE_TOKEN", "")
    if token and sys.argv[-1] == "get":
        username = "x-access-token" if host == "github.com" else GE_OWNER
        sys.stdout.write(f"username={username}\npassword={token}\n\n")


def git_failure_kind(stderr):
    # Classify locally; never log Git stderr, which may contain credentials.
    message = stderr.lower()
    if any(value in message for value in ("authentication failed", "could not read username", "error: 401", "error: 403")):
        return "authorization"
    if "atomic" in message and "support" in message:
        return "unsupported atomic push"
    if any(value in message for value in ("non-fast-forward", "fetch first", "already exists")):
        return "conflicting refs or existing destination"
    if any(value in message for value in ("connection reset", "connection timed out", "timed out", "tls", "ssl", "http/2", "could not resolve", "failed to connect", "remote end hung up", "error: 500", "error: 502", "error: 503", "error: 504")):
        return "transient network"
    return "unclassified transport failure"


def git_run(repo, *args):
    env = os.environ.copy()
    env.update({"SYNC_REPO": repo, "GIT_TERMINAL_PROMPT": "0", "GCM_INTERACTIVE": "Never",
                "GIT_TRACE": "0", "GIT_TRACE_CURL": "0", "GIT_CURL_VERBOSE": "0",
                "GIT_CONFIG_GLOBAL": os.devnull, "GIT_CONFIG_SYSTEM": os.devnull,
                "GIT_ALLOW_PROTOCOL": "https"})
    helper = "!" + shlex.quote(Path(sys.executable).as_posix()) + " " + shlex.quote(Path(__file__).resolve().as_posix()) + " _git_credential"
    command = ["git", "-c", "http.version=HTTP/1.1", "-c", "credential.helper=", "-c", "credential.helper=" + helper,
               "-c", "credential.useHttpPath=true", "-c", "core.askPass=", *args]
    operation_args = args[2:] if args[:1] == ("--git-dir",) else args
    operation = operation_args[0] if operation_args else "unknown"
    if operation not in {"clone", "fetch", "push", "ls-remote", "rev-parse", "remote", "for-each-ref"}:
        operation = "other"
    attempts = READ_ATTEMPTS if operation in {"fetch", "ls-remote"} else 1
    for attempt in range(attempts):
        try:
            result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=300)
        except (OSError, subprocess.TimeoutExpired):
            kind = "transient network"
        else:
            if not result.returncode:
                return result.stdout.strip()
            kind = git_failure_kind(result.stderr or "")
        if kind != "transient network" or attempt == attempts - 1:
            raise SyncError(f"Git {operation} failed ({kind}); no credential output is logged and remote refs were not forced") from None
        time.sleep(2 ** attempt)


def migrate_legacy_path(github, gitee, apply=False):
    """Rename only the approved original repository; never create or replace one."""
    source = github.request("/repos/CMMUU/serylane")
    if (source.get("id") != 1355770287 or source.get("full_name") != "CMMUU/serylane"
            or source.get("private") is not False):
        raise SyncError("GitHub migration source identity changed")
    if str(gitee.request("/user").get("login", "")).casefold() != GE_OWNER:
        raise SyncError("Gitee migration requires the approved owner")

    def checked(info, slug):
        if (type(info.get("id")) is not int or info["id"] != 50078322
                or str(info.get("owner", {}).get("login", "")).casefold() != GE_OWNER
                or info.get("path") != slug or info.get("private") is not False
                or info.get("default_branch") != "main"):
            raise SyncError("Gitee migration identity/path/visibility/default branch changed")
        return info

    new_path = "/repos/cmmuu/serylane"
    old_path = "/repos/cmmuu/routedeck"
    try:
        current = gitee.request(new_path)
    except NotFoundError:
        current = None
    if current is not None:
        checked(current, "serylane")
        if current.get("name") != "Serylane":
            raise SyncError("Canonical Gitee repository has an unexpected display name")
        print("Canonical Gitee path verified: cmmuu/serylane (original repository)", flush=True)
        return True

    previous = checked(gitee.request(old_path), "routedeck")
    if not apply:
        print("Preview: rename original Gitee repository cmmuu/routedeck to cmmuu/serylane")
        return False
    # Preserve documented API defaults when exposed by repository metadata.
    fields = {"name": "Serylane", "path": "serylane"}
    for key in ("has_issues", "has_wiki", "can_comment", "merge_enabled", "squash_enabled", "rebase_enabled"):
        if type(previous.get(key)) is bool:
            fields[key] = str(previous[key]).lower()
    before_releases = {row["id"]: row["tag_name"] for row in gitee.pages(old_path + "/releases")}
    before_head = gitee.request(old_path + "/branches/main")["commit"]["sha"]
    write_error = None
    try:
        gitee.request(old_path, "PATCH", fields)
    except SyncError as error:
        # An uncertain write is never repeated. Verify the new path separately.
        write_error = error
    try:
        confirmed = checked(gitee.request(new_path), "serylane")
    except SyncError:
        if write_error is not None:
            raise write_error
        raise
    after_releases = {row["id"]: row["tag_name"] for row in gitee.pages(new_path + "/releases")}
    after_head = gitee.request(new_path + "/branches/main")["commit"]["sha"]
    if (confirmed.get("name") != "Serylane" or before_releases != after_releases or before_head != after_head
            or any(confirmed.get(key) != previous[key] for key in fields if key not in {"name", "path"})):
        raise SyncError("Gitee rename did not preserve the checked repository state")
    print("Gitee renamed and verified: cmmuu/serylane; repository ID, main and releases preserved", flush=True)
    return True


class Sync:
    def __init__(self, repo, github, gitee, work, max_asset_bytes=GE_MAX_ASSET,
                 max_total_bytes=GE_MAX_TOTAL, other_attachment_bytes=0, transfer_workers=1):
        self.repo, self.gh, self.ge, self.work = repo, github, gitee, Path(work)
        if repo not in REPOS:
            raise SyncError("Unsupported repository")
        self.source_path = f"/repos/{GH_OWNER}/{SOURCE_REPOS[repo]}"
        self.target_path = f"/repos/{GE_OWNER}/{repo}"
        self.source = None
        self.max_asset_bytes, self.max_total_bytes = max_asset_bytes, max_total_bytes
        self.other_attachment_bytes = other_attachment_bytes
        self.release_assets = {}
        if type(transfer_workers) is not int or not 1 <= transfer_workers <= 3:
            raise SyncError("Transfer concurrency must be between 1 and 3")
        self.transfer_workers = transfer_workers

    def guard(self):
        self.source = self.gh.request(self.source_path)
        target = self.ge.request(self.target_path)
        validate_pair(self.repo, self.source, target)
        # Public repository metadata can succeed without valid credentials.
        # /user must authenticate the intended owner before any external write.
        identity = self.ge.request("/user")
        if str(identity.get("login", "")).casefold() != GE_OWNER.casefold():
            raise SyncError("Gitee credential identity must match the permitted target owner")
        return target

    def sync_refs(self):
        self.guard()
        self.work.mkdir(parents=True, exist_ok=True)
        bare = self.work / (self.repo + ".git")
        source_url = f"https://github.com/{GH_OWNER}/{SOURCE_REPOS[self.repo]}.git"
        if bare.exists():
            if (bare.is_symlink() or git_run(self.repo, "--git-dir", str(bare), "rev-parse", "--is-bare-repository") != "true"
                    or git_run(self.repo, "--git-dir", str(bare), "remote", "get-url", "origin") != source_url):
                raise SyncError("Existing working mirror does not match the source repository")
            git_run(self.repo, "--git-dir", str(bare), "fetch", "--prune", "origin")
        else:
            git_run(self.repo, "clone", "--mirror", source_url, str(bare))
        self.guard()  # Recheck privacy immediately before the first external write.
        destination = f"https://gitee.com/{GE_OWNER}/{self.repo}.git"
        # Explicit namespaces: no deletion, force push, pull refs or remote configs.
        push_error = None
        try:
            git_run(self.repo, "--git-dir", str(bare), "push", "--atomic", destination,
                    "refs/heads/*:refs/heads/*", "refs/tags/*:refs/tags/*")
        except SyncError as error:
            # A lost response can follow a successful push. Do not retry the
            # write: independently read every expected ref before accepting it.
            push_error = error
        expected = dict(line.split(" ", 1) for line in git_run(self.repo, "--git-dir", str(bare), "for-each-ref",
                        "--format=%(refname) %(objectname)", "refs/heads", "refs/tags").splitlines())
        actual = {ref: sha for sha, ref in (line.split() for line in git_run(self.repo, "ls-remote", "--refs", destination).splitlines())}
        if any(actual.get(ref) != sha for ref, sha in expected.items()):
            if push_error is not None:
                raise push_error
            raise SyncError("Gitee branches or tags did not match the source after push")
        return bare

    def sync_display_name(self):
        # Display-name maintenance never changes the canonical URL. Migration
        # is a separate, explicitly authorized operation above.
        target = self.guard()
        if target.get("name") == "Serylane":
            return
        target_id = target.get("id")
        if type(target_id) is not int or target_id <= 0:
            raise SyncError("Gitee repository identity is missing; no name change attempted")
        self.ge.request(self.target_path, "PATCH", {"name": "Serylane"})
        confirmed = self.guard()
        if confirmed.get("id") != target_id or confirmed.get("name") != "Serylane":
            raise SyncError("Gitee did not confirm the display name on the same repository")
        print("Gitee display name verified: Serylane; canonical URL unchanged", flush=True)

    def source_assets(self, release, enforce_quota=True):
        release_id = release.get("id")
        if type(release_id) is not int or release_id <= 0:
            raise SyncError("Source release ID is missing or invalid")
        if release_id in self.release_assets:
            assets = self.release_assets[release_id]
            if enforce_quota and any(asset["size"] > self.max_asset_bytes for asset in assets):
                raise SyncError(f"GitHub attachment exceeds the configured Gitee per-file limit ({self.max_asset_bytes} bytes)")
            return assets
        assets = self.gh.pages(f"{self.source_path}/releases/{release_id}/assets")
        names, ids = set(), set()
        for asset in assets:
            name = safe_name(asset.get("name"))
            asset_id = asset.get("id")
            if (name.casefold() in names or asset.get("state") != "uploaded"
                    or type(asset_id) is not int or asset_id <= 0 or asset_id in ids):
                raise SyncError("Incomplete or ambiguous GitHub attachment")
            names.add(name.casefold())
            ids.add(asset_id)
            size = asset.get("size")
            if type(size) is not int or not 0 <= size <= min(self.max_asset_bytes if enforce_quota else MAX_ASSET, MAX_ASSET):
                raise SyncError(f"GitHub attachment exceeds the configured Gitee per-file limit ({self.max_asset_bytes} bytes)")
            if asset.get("url") != GH_API + f"{self.source_path}/releases/assets/{asset['id']}":
                raise SyncError("GitHub attachment belongs to a different repository")
            source_digest(asset)
        self.release_assets[release_id] = assets
        return assets

    def release_files(self, release, enforce_quota=True):
        assets = self.source_assets(release) if enforce_quota else self.source_assets(release, False)
        files = []
        directory = self.work / "assets" / self.repo / str(release["id"])
        for asset in assets:
            file = directory / asset["name"]
            print(f"Verifying GitHub source: {release['tag_name']}/{asset['name']} ({asset['size']} bytes)", flush=True)
            actual = self.gh.download(f"{self.source_path}/releases/assets/{asset['id']}",
                                      file, asset["size"], source_digest(asset))
            files.append({"name": asset["name"], "path": file, "size": asset["size"], "sha256": actual})
        verify_manifest(files)
        return files

    def retention_plan(self, releases, keep):
        ordered = stable_releases(releases)
        retained = ordered[:keep]
        if not retained:
            raise SyncError("No published stable release is available; no retention cleanup allowed")
        sources = {release["tag_name"]: release for release in ordered}
        retained_tags = {release["tag_name"] for release in retained}
        existing, total, candidates = {}, self.other_attachment_bytes, []
        for target in self.ge.pages(self.target_path + "/releases"):
            tag, release_id = target.get("tag_name"), target.get("id")
            if not isinstance(tag, str) or not tag or tag in existing or type(release_id) is not int or release_id <= 0:
                raise SyncError("Ambiguous destination release metadata")
            attachments, ids = {}, set()
            for asset in self.ge.pages(f"{self.target_path}/releases/{release_id}/attach_files"):
                name, size, asset_id = safe_name(asset.get("name")), asset.get("size"), asset.get("id")
                if (name.casefold() in attachments or type(size) is not int or size < 0
                        or type(asset_id) is not int or asset_id <= 0 or asset_id in ids):
                    raise SyncError("Ambiguous destination attachment metadata; retention stopped")
                attachments[name.casefold()] = asset
                ids.add(asset_id)
                total += size
            existing[tag] = attachments
            # Unknown releases, preview releases and target-only files are not
            # mirrored history and never become deletion candidates.
            if tag in retained_tags or tag not in sources or target.get("prerelease") is not False or not attachments:
                continue
            source = sources[tag]
            source_assets = {asset["name"].casefold(): asset for asset in self.source_assets(source, False)}
            removable = []
            for key, asset in attachments.items():
                original = source_assets.get(key)
                if original is None:
                    continue
                if (asset["name"], asset["size"]) != (original["name"], original["size"]):
                    raise SyncError("Historical Gitee attachment conflicts with its GitHub backup; no cleanup allowed")
                removable.append({"target": asset, "source": original})
            if removable:
                candidates.append({"source": source, "target": target, "assets": removable,
                                   "bytes": sum(item["target"]["size"] for item in removable)})
        added = 0
        for release in retained:
            assets = self.source_assets(release)
            if not assets:
                raise SyncError("Newest stable release has no assets; no cleanup allowed")
            for asset in assets:
                present = existing.get(release["tag_name"], {}).get(asset["name"].casefold())
                if present is None:
                    added += asset["size"]
                elif (present["name"], present["size"]) != (asset["name"], asset["size"]):
                    raise SyncError("Existing Gitee attachment conflicts with the source; no files were replaced")
        candidates.sort(key=lambda group: tuple(map(int, group["source"]["tag_name"][1:].split("."))))
        freed = sum(group["bytes"] for group in candidates)
        final_bytes = total + added - freed
        if final_bytes > self.max_total_bytes:
            raise SyncError(f"Retained releases and protected files need {final_bytes} bytes, above {self.max_total_bytes}; no cleanup allowed")
        before, after, staged = [], [], total + added
        for group in candidates:
            if staged > self.max_total_bytes:
                before.append(group)
                staged -= group["bytes"]
            else:
                after.append(group)
        print(f"Retention: keep {', '.join(release['tag_name'] for release in retained)}; "
              f"{total} existing/reserved + {added} new - {freed} retired = {final_bytes} bytes", flush=True)
        for group in candidates:
            phase = "before transfer (space needed)" if group in before else "after verified transfer"
            print(f"Retire Gitee attachments only: {group['source']['tag_name']}, release ID {group['target']['id']}, "
                  f"{len(group['assets'])} file(s), {group['bytes']} bytes, {phase}", flush=True)
        return {"retained": retained, "before": before, "after": after, "keep": keep}

    def check_retention_source(self, plan, release):
        # Stop on concurrent publishing, withdrawn releases or replaced source
        # assets. Recompute selection from GitHub, never from a stale event tag.
        current = stable_releases(self.gh.pages(self.source_path + "/releases"))
        identity = lambda rows: [(row["id"], row["tag_name"]) for row in rows]
        if identity(current[:plan["keep"]]) != identity(plan["retained"]):
            raise SyncError("Published stable releases changed during retention; cleanup stopped")
        if (release["id"], release["tag_name"]) not in identity(current):
            raise SyncError("GitHub backup release was withdrawn; cleanup stopped")
        expected = self.release_assets.pop(release["id"])
        actual = self.source_assets(release, False)
        if sorted(map(asset_identity, expected)) != sorted(map(asset_identity, actual)):
            raise SyncError("GitHub backup assets changed during retention; cleanup stopped")

    def verify_retirement(self, group):
        files = {item["name"]: item for item in self.release_files(group["source"], False)}
        for item in group["assets"]:
            asset = item["target"]
            original = files[asset["name"]]
            print(f"Verifying retirement backup: {group['source']['tag_name']}/{asset['name']} ({asset['size']} bytes)", flush=True)
            check = self.work / "verify" / str(group["target"]["id"]) / (str(asset["id"]) + ".retire-" + uuid.uuid4().hex)
            try:
                self.ge.download(f"{self.target_path}/releases/{group['target']['id']}/attach_files/{asset['id']}/download",
                                 check, original["size"], original["sha256"])
            finally:
                if check.exists():
                    check.unlink()
            item["sha256"] = original["sha256"]
        print(f"Verified GitHub backup and Gitee bytes: {group['source']['tag_name']}", flush=True)

    def retire_attachments(self, plan, group):
        self.check_retention_source(plan, group["source"])
        if group["source"]["tag_name"] in {row["tag_name"] for row in plan["retained"]}:
            raise SyncError("Refusing to delete a retained release attachment")
        endpoint = f"{self.target_path}/releases/{group['target']['id']}"
        target = self.ge.request(endpoint)
        if (target.get("id"), target.get("tag_name"), target.get("prerelease")) != (
                group["target"]["id"], group["source"]["tag_name"], False):
            raise SyncError("Destination release changed before cleanup")
        for item in sorted(group["assets"], key=lambda row: (row["target"]["name"] not in RELEASE_MANIFESTS, row["target"]["name"])):
            asset = item["target"]
            if not re.fullmatch(r"[0-9a-f]{64}", item.get("sha256", "")):
                raise SyncError("No verified byte-identical GitHub backup; refusing deletion")
            # Older assets without GitHub digests need fresh source bytes again
            # immediately before removal; never trust a cached file alone.
            if source_digest(item["source"]) is None:
                check = self.work / "verify" / ("source-retire-" + uuid.uuid4().hex)
                try:
                    self.gh.download(f"{self.source_path}/releases/assets/{item['source']['id']}",
                                     check, asset["size"], item["sha256"])
                finally:
                    if check.exists():
                        check.unlink()
            self.guard()
            rows = self.ge.pages(endpoint + "/attach_files")
            matches = [row for row in rows if row.get("id") == asset["id"] or str(row.get("name", "")).casefold() == asset["name"].casefold()]
            if not matches:
                continue  # Already removed; do not invent a new deletion target.
            if len(matches) != 1 or any(matches[0].get(key) != asset[key] for key in ("id", "name", "size")):
                raise SyncError("Destination attachment changed before cleanup")
            error = None
            try:
                self.ge.request(f"{endpoint}/attach_files/{asset['id']}", "DELETE")
            except SyncError as failure:
                error = failure
            # An empty/lost response is not permission to repeat DELETE. Read
            # back first, and stop if absence cannot be independently confirmed.
            remaining = self.ge.pages(endpoint + "/attach_files")
            if any(row.get("id") == asset["id"] or row.get("name") == asset["name"] for row in remaining):
                raise error or SyncError("Gitee did not confirm attachment deletion; no delete was retried")
            print(f"Removed Gitee {group['source']['tag_name']}/{asset['name']} ({asset['size']} bytes); GitHub original retained", flush=True)

    def run_retention(self, releases, keep, apply, bare):
        plan = self.retention_plan(releases, keep)
        if not apply:
            print("Retention preflight only: byte verification and writes require --apply. No remote changes.", flush=True)
            return
        # Verify all incoming files and all recoverable outgoing bytes BEFORE
        # the first deletion. Only free the oldest history needed for staging;
        # retire the rest after every retained installer/manifest verifies.
        for release in plan["retained"]:
            self.release_files(release)
            self.check_retention_source(plan, release)
        for group in plan["before"] + plan["after"]:
            self.verify_retirement(group)
        for release in plan["retained"]:
            self.check_retention_source(plan, release)
        for group in plan["before"]:
            self.retire_attachments(plan, group)
        self.plan_release_capacity(plan["retained"])
        for release in reversed(plan["retained"]):
            self.check_retention_source(plan, release)
            self.sync_release(release, bare)
        for group in plan["after"]:
            self.retire_attachments(plan, group)
        self.plan_release_capacity(plan["retained"])

    def plan_release_capacity(self, releases):
        # Include every existing Gitee release, including target-only releases.
        # Regular repository attachments are not listed by the release API;
        # reserve their known usage via GITEE_OTHER_ATTACHMENT_BYTES.
        existing, total = {}, self.other_attachment_bytes
        for release in self.ge.pages(self.target_path + "/releases"):
            release_id, tag = release.get("id"), release.get("tag_name")
            if type(release_id) is not int or not isinstance(tag, str) or not tag or tag in existing:
                raise SyncError("Ambiguous destination release metadata")
            by_name = {}
            for asset in self.ge.pages(f"{self.target_path}/releases/{release_id}/attach_files"):
                name, size = safe_name(asset.get("name")), asset.get("size")
                if name.casefold() in by_name or type(size) is not int or size < 0:
                    raise SyncError("Ambiguous destination attachment size or name")
                by_name[name.casefold()] = (name, size)
                total += size
            existing[tag] = by_name
        added, seen = 0, set()
        for release in releases:
            if release.get("draft"):
                continue
            tag = release.get("tag_name")
            if not isinstance(tag, str) or not tag or tag in seen:
                raise SyncError("Ambiguous source release tags")
            seen.add(tag)
            for asset in self.source_assets(release):
                present = existing.get(tag, {}).get(asset["name"].casefold())
                if present is not None:
                    if present != (asset["name"], asset["size"]):
                        raise SyncError("Existing Gitee attachment conflicts with the source; no files were replaced")
                else:
                    added += asset["size"]
        if total + added > self.max_total_bytes:
            raise SyncError(f"Gitee attachment capacity exceeded: projected {total + added} bytes, limit {self.max_total_bytes}; no release writes started")
        print(f"Attachment capacity checked: {total} existing/reserved + {added} new bytes (limit {self.max_total_bytes})", flush=True)

    def ensure_attachment(self, release_id, item):
        endpoint = f"{self.target_path}/releases/{release_id}/attach_files"
        matches = [asset for asset in self.ge.pages(endpoint) if asset.get("name") == item["name"]]
        if len(matches) > 1:
            raise SyncError("Duplicate destination attachment names; no files were replaced")
        if not matches:
            self.guard()
            if Path(item["path"]).stat().st_size != item["size"] or sha256(item["path"]) != item["sha256"]:
                raise SyncError("Local source attachment changed before upload")
            print(f"Uploading Gitee attachment: {item['name']} ({item['size']} bytes)", flush=True)
            self.ge.upload(endpoint, item["path"])
            matches = [asset for asset in self.ge.pages(endpoint) if asset.get("name") == item["name"]]
            if len(matches) != 1:
                raise SyncError("Upload completed without one unambiguous destination attachment")
        asset = matches[0]
        if type(asset.get("id")) is not int or (type(asset.get("size")) is int and asset["size"] != item["size"]):
            raise SyncError("Destination attachment metadata differs; it was not replaced")
        # Gitee AttachFile has no documented digest. Compare downloaded bytes,
        # including existing same-name attachments, instead of trusting the name.
        check = self.work / "verify" / str(release_id) / (str(asset["id"]) + ".verify-" + uuid.uuid4().hex)
        print(f"Verifying Gitee attachment: {item['name']} ({item['size']} bytes)", flush=True)
        try:
            self.ge.download(f"{endpoint}/{asset['id']}/download", check, item["size"], item["sha256"])
        finally:
            if check.exists():
                check.unlink()
        print(f"Verified Gitee attachment: {item['name']}", flush=True)

    def transfer_attachments(self, release_id, files):
        manifests = RELEASE_MANIFESTS
        regular = sorted((item for item in files if item["name"] not in manifests), key=lambda item: item["name"])
        if self.transfer_workers == 1:
            for item in regular:
                self.ensure_attachment(release_id, item)
        else:
            def transfer(item):
                worker = Sync(self.repo, self.gh.fork(), self.ge.fork(), self.work,
                              self.max_asset_bytes, self.max_total_bytes, self.other_attachment_bytes)
                worker.ensure_attachment(release_id, item)

            # Bound both running and queued requests. On failure, stop assigning
            # more files and let in-flight requests finish for safe reconciliation.
            pending_files = iter(regular)
            with ThreadPoolExecutor(max_workers=self.transfer_workers) as pool:
                pending = {pool.submit(transfer, item) for item in islice(pending_files, self.transfer_workers)}
                while pending:
                    finished, pending = wait(pending, return_when=FIRST_COMPLETED)
                    try:
                        for future in finished:
                            future.result()
                    except BaseException:
                        for future in pending:
                            future.cancel()
                        raise
                    pending.update(pool.submit(transfer, item) for item in islice(pending_files, len(finished)))
        # This barrier is intentionally outside the pool: every installer,
        # signature and checksum must verify before any current/legacy manifest.
        for item in sorted((item for item in files if item["name"] in manifests), key=lambda item: item["name"]):
            self.ensure_attachment(release_id, item)

    def sync_release(self, release, bare):
        if release.get("draft"):
            return  # Gitee release API has no documented equivalent of GitHub drafts.
        tag = release.get("tag_name", "")
        if not isinstance(tag, str) or not tag or "\x00" in tag:
            raise SyncError("Invalid release tag")
        commit = git_run(self.repo, "--git-dir", str(bare), "rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}")
        if not re.fullmatch(r"[0-9a-f]{40,64}", commit):
            raise SyncError("Release tag does not resolve to a source commit")
        files = self.release_files(release)
        matches = [row for row in self.ge.pages(self.target_path + "/releases") if row.get("tag_name") == tag]
        if len(matches) > 1:
            raise SyncError("Duplicate destination releases for one tag")
        metadata = {"tag_name": tag, "name": release.get("name") or tag, "body": release.get("body") or "",
                    "prerelease": bool(release.get("prerelease")), "target_commitish": commit}
        self.guard()
        if not matches:
            target = self.ge.request(self.target_path + "/releases", "POST", {**metadata, "prerelease": str(metadata["prerelease"]).lower()})
        else:
            target = matches[0]
            if not release_metadata_matches(target, metadata, ("name", "body", "prerelease")):
                target = self.ge.request(f"{self.target_path}/releases/{target['id']}", "PATCH",
                                         {key: str(value).lower() if type(value) is bool else value for key, value in metadata.items() if key != "target_commitish"})
        if type(target.get("id")) is not int or target.get("tag_name") != tag:
            raise SyncError("Gitee release response does not match the source tag")
        # Gitee has no draft assets. Make updater manifests visible only after
        # every installer and signature has been uploaded and hash-verified.
        self.transfer_attachments(target["id"], files)
        confirmed = self.ge.request(f"{self.target_path}/releases/{target['id']}")
        if not release_metadata_matches(confirmed, metadata, ("tag_name", "name", "body", "prerelease")):
            raise SyncError("Gitee release metadata did not match after synchronization")
        print(f"Synchronized {self.repo}: {tag}, {len(files)} attachment(s)", flush=True)

    def run(self, scope, apply, release_tag=None, keep_latest_releases=None, sync_display_name=False):
        if keep_latest_releases is not None and (type(keep_latest_releases) is not int or not 1 <= keep_latest_releases <= 10):
            raise SyncError("Release retention must keep between 1 and 10 stable versions")
        if release_tag is not None and (scope != "all" or not re.fullmatch(r"v(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)", release_tag)):
            raise SyncError("A focused release sync requires --scope all and an exact stable release tag")
        self.guard()
        if sync_display_name:
            if apply:
                self.sync_display_name()
            else:
                print("Preview: align Gitee display name with Serylane; do not change its URL")
        releases = self.gh.pages(self.source_path + "/releases") if scope == "all" else []
        selected = releases
        if release_tag is not None:
            selected = [release for release in releases if release.get("tag_name") == release_tag and not release.get("draft")]
            if len(selected) != 1:
                raise SyncError("The requested release is not uniquely published on GitHub; no sync writes started")
        # Code updates remain useful even when the separate attachment quota
        # is exhausted. Privacy/ref checks still run before any Git push.
        bare = self.sync_refs() if apply else None
        if apply:
            print(f"Synchronized and verified {self.repo} branches and tags", flush=True)
        if scope == "all" and keep_latest_releases is not None:
            # Retention supersedes an older focused event, so edits to expired
            # releases and scheduled audits cannot resurrect evicted binaries.
            self.run_retention(releases, keep_latest_releases, apply, bare)
            return
        if scope == "all":
            # Focus only limits transfers. Capacity/conflict checks still cover
            # every source and destination release, including historical assets.
            self.plan_release_capacity(releases)
        if not apply:
            print(f"Preflight passed for {self.repo}; {len(releases)} release(s) found. No external changes without --apply.")
            return
        for release in reversed(selected):
            self.sync_release(release, bare)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", choices=sorted(REPOS), required=True)
    parser.add_argument("--scope", choices=("refs", "all"), default="all")
    parser.add_argument("--release-tag", help="Transfer one published stable release; still check total attachment capacity")
    parser.add_argument("--keep-latest-releases", type=int, choices=range(1, 11),
                        help="Opt in to retiring byte-verified Gitee history outside the latest N stable versions; never delete tags or GitHub assets")
    parser.add_argument("--transfer-workers", type=int, choices=(1, 2, 3), default=1,
                        help="Bound concurrent attachment transfers; updater manifests always wait for every file to verify")
    parser.add_argument("--work-dir", type=Path, required=True)
    parser.add_argument("--apply", action="store_true", help="Explicitly authorize writes to the checked Gitee repository")
    parser.add_argument("--sync-display-name", action="store_true",
                        help="Align the Gitee display name with Serylane without changing its URL")
    parser.add_argument("--migrate-legacy-path", action="store_true",
                        help="Migrate only original repository ID 50078322 to cmmuu/serylane before synchronization")
    args = parser.parse_args()
    gh_token, ge_token = os.environ.get("GITHUB_TOKEN"), os.environ.get("GITEE_TOKEN")
    if not gh_token or not ge_token:
        raise SyncError("Set GITHUB_TOKEN and GITEE_TOKEN in the environment; never pass tokens as arguments")
    extra_hosts = {value.strip().lower() for value in os.environ.get("GITEE_ASSET_HOSTS", "").split(",") if value.strip()}
    if any(not re.fullmatch(r"[a-z0-9]+(?:[.-][a-z0-9]+)*", host) for host in extra_hosts):
        raise SyncError("GITEE_ASSET_HOSTS accepts exact hostnames only, without wildcards or URLs")
    max_asset = configured_bytes("GITEE_MAX_ASSET_BYTES", GE_MAX_ASSET, MAX_ASSET)
    max_total = configured_bytes("GITEE_MAX_TOTAL_BYTES", GE_MAX_TOTAL, 100_000_000_000)
    reserved = configured_bytes("GITEE_OTHER_ATTACHMENT_BYTES", 0, max_total)
    github, gitee = Api("github", gh_token), Api("gitee", ge_token, extra_hosts)
    if args.migrate_legacy_path and not migrate_legacy_path(github, gitee, args.apply):
        return
    Sync(args.repo, github, gitee, args.work_dir,
         max_asset, max_total, reserved, args.transfer_workers).run(args.scope, args.apply, args.release_tag, args.keep_latest_releases, args.sync_display_name)


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "_git_credential":
        git_credential()
    else:
        try:
            main()
        except SyncError as error:
            print(f"Sync stopped: {error}", file=sys.stderr)
            raise SystemExit(1)
