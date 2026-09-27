"""Bounded parallel transfers with independent clients and a manifest barrier."""
import threading
import unittest
import hashlib
import io
import tempfile
from pathlib import Path
from unittest.mock import patch

import sync_gitee as sync


class TransferTests(unittest.TestCase):
    def job(self, workers=3):
        return sync.Sync("serylane", sync.Api("github", "offline-gh"),
                         sync.Api("gitee", "offline-ge", {"extra.example"}), ".", transfer_workers=workers)

    def test_fork_has_separate_http_handlers_and_identical_security_scope(self):
        for service in ("github", "gitee"):
            original = sync.Api(service, "offline-only", {"extra.example"})
            child = original.fork()
            self.assertIsNot(child, original)
            self.assertIsNot(child.opener, original.opener)
            self.assertEqual(child.storage_hosts, original.storage_hosts)
            self.assertEqual(child.headers(), original.headers())
            self.assertEqual(child.base, original.base)

    def test_worker_count_cannot_be_unbounded_or_implicitly_enabled(self):
        self.assertEqual(self.job(1).transfer_workers, 1)
        for count in (0, -1, 4, True, "3"):
            with self.assertRaisesRegex(sync.SyncError, "between 1 and 3"):
                self.job(count)

    def test_three_independent_workers_verify_every_file_before_manifests(self):
        job = self.job()
        barrier, lock = threading.Barrier(3, timeout=10), threading.Lock()
        calls, clients = [], []
        state = {"active": 0, "peak": 0, "verified": 0}
        names = ["latest.json", "f.zip", "a.zip", "latest-gitee.json", "e.zip", "b.zip", "c.zip", "d.zip"]

        def ensure(worker, release_id, item):
            self.assertEqual(release_id, 12)
            name = item["name"]
            if name.startswith("latest"):
                self.assertEqual(state["verified"], 6)
                self.assertEqual(state["active"], 0)
                calls.append(name)
                return
            with lock:
                state["active"] += 1
                state["peak"] = max(state["peak"], state["active"])
                clients.append((worker.gh, worker.ge))
            barrier.wait()
            with lock:
                state["active"] -= 1
                state["verified"] += 1
                calls.append(name)

        with patch.object(sync.Sync, "ensure_attachment", autospec=True, side_effect=ensure):
            job.transfer_attachments(12, [{"name": name} for name in names])
        self.assertEqual(state["peak"], 3)
        self.assertEqual(calls[-2:], ["latest-gitee.json", "latest.json"])
        self.assertCountEqual(calls[:-2], [name for name in names if name.endswith(".zip")])
        self.assertEqual(len({id(ge.opener) for _, ge in clients}), 6)
        self.assertTrue(all(ge is not job.ge and gh is not job.gh for gh, ge in clients))

    def test_failure_stops_new_assignments_and_never_publishes_manifests(self):
        job, calls = self.job(), []
        first_wave = threading.Barrier(3, timeout=10)
        failed_future_done = threading.Event()

        class ControlledExecutor(sync.ThreadPoolExecutor):
            def submit(self, function, item):
                future = super().submit(function, item)
                if item["name"] == "a.zip":
                    future.add_done_callback(lambda completed: failed_future_done.set())
                return future

        def ensure(worker, release_id, item):
            calls.append(item["name"])
            # Client setup and OS scheduling need not finish in filename order.
            # Hold the two successful initial transfers until the failed future
            # is actually complete; only then is new assignment a violation.
            if item["name"] in {"a.zip", "b.zip", "c.zip"}:
                first_wave.wait()
            if item["name"] == "a.zip":
                raise sync.SyncError("Uncertain upload; do not retry")
            self.assertTrue(failed_future_done.wait(10))

        names = ["a.zip", "b.zip", "c.zip", "d.zip", "e.zip", "latest.json", "latest-gitee.json"]
        with patch.object(sync, "ThreadPoolExecutor", ControlledExecutor), \
                patch.object(sync.Sync, "ensure_attachment", autospec=True, side_effect=ensure):
            with self.assertRaisesRegex(sync.SyncError, "Uncertain upload"):
                job.transfer_attachments(12, [{"name": name} for name in names])
        self.assertTrue(set(calls).issubset({"a.zip", "b.zip", "c.zip"}))
        self.assertEqual(calls.count("a.zip"), 1)

    def test_truncated_read_retries_without_publishing_partial_output(self):
        payload = b"verified attachment content"
        api = sync.Api("gitee", "fixture-secret")
        with tempfile.TemporaryDirectory() as folder:
            destination = Path(folder) / "Serylane_sample.bin"
            with patch.object(api.opener, "open", side_effect=[io.BytesIO(payload[:4]), io.BytesIO(payload)]) as opened, \
                    patch.object(sync.time, "sleep"):
                result = api.download("/repos/cmmuu/serylane/releases/1/attach_files/2/download",
                                      destination, len(payload), hashlib.sha256(payload).hexdigest())
            self.assertEqual(opened.call_count, 2)
            self.assertEqual(result, hashlib.sha256(payload).hexdigest())
            self.assertEqual(destination.read_bytes(), payload)
            self.assertEqual(list(Path(folder).glob("*.part-*")), [])

    def test_repeated_hash_mismatch_remains_fatal_and_never_commits_bad_bytes(self):
        api = sync.Api("gitee", "fixture-secret")
        with tempfile.TemporaryDirectory() as folder:
            destination = Path(folder) / "Serylane_sample.bin"
            with patch.object(api.opener, "open", side_effect=lambda *a, **kw: io.BytesIO(b"bad!")) as opened, \
                    patch.object(sync.time, "sleep"):
                with self.assertRaises(sync.SyncError) as result:
                    api.download("/repos/cmmuu/serylane/releases/1/attach_files/2/download",
                                 destination, 4, hashlib.sha256(b"good").hexdigest())
            self.assertEqual(opened.call_count, sync.READ_ATTEMPTS)
            self.assertIn(destination.name, str(result.exception))
            self.assertNotIn(api.token, str(result.exception))
            self.assertFalse(destination.exists())
            self.assertEqual(list(Path(folder).glob("*.part-*")), [])

    def test_website_catalog_and_all_update_manifests_wait_for_verified_payloads(self):
        job, calls = self.job(1), []
        names = ["downloads.json", "latest-serylane.json", "latest-serylane-gitee.json", "latest-serylane-hk.json",
                 "latest.json", "latest-gitee.json", "app.zip", "app.zip.sig", "SHA256SUMS.txt"]
        def ensure(release_id, item):
            if item["name"] in sync.RELEASE_MANIFESTS:
                self.assertTrue({"app.zip", "app.zip.sig", "SHA256SUMS.txt"}.issubset(calls))
            calls.append(item["name"])
        with patch.object(job, "ensure_attachment", side_effect=ensure):
            job.transfer_attachments(12, [{"name": name} for name in names])
        self.assertCountEqual(calls[-len(sync.RELEASE_MANIFESTS):], sync.RELEASE_MANIFESTS)

    def test_serial_default_preserves_manifest_last_order(self):
        job, calls = self.job(1), []
        with patch.object(job, "ensure_attachment", side_effect=lambda release_id, item: calls.append(item["name"])):
            job.transfer_attachments(12, [{"name": name} for name in ["latest.json", "b.zip", "latest-gitee.json", "a.zip"]])
        self.assertEqual(calls, ["a.zip", "b.zip", "latest-gitee.json", "latest.json"])


if __name__ == "__main__":
    unittest.main()
