import contextlib
import io
import json
from pathlib import Path
import ssl
import tempfile
import unittest
from unittest.mock import patch

from recover_github_upload import upload, UploadError


class Connection:
    def __init__(self, status=201, fail=False):
        self.status, self.fail, self.headers, self.body = status, fail, {}, b""
        self.closed = False
        self.sock = self

    def settimeout(self, timeout):
        self.response_timeout = timeout

    def putrequest(self, method, endpoint):
        self.method, self.endpoint = method, endpoint

    def putheader(self, key, value):
        self.headers[key] = value

    def endheaders(self):
        pass

    def send(self, data):
        if self.fail:
            raise TimeoutError("secret-token raw error")
        self.body += data

    def getresponse(self):
        return self

    def read(self, limit):
        return json.dumps({"name": "Serylane_0.7.13_x64-setup.exe", "state": "uploaded"}).encode()

    def close(self):
        self.closed = True


class RecoveryUploadTests(unittest.TestCase):
    def exercise(self, connection):
        calls = []
        def factory(host, **options):
            calls.append(host)
            self.assertEqual(host, "uploads.github.com")
            self.assertEqual(options["timeout"], 30)
            self.assertEqual(options["context"].verify_mode, ssl.CERT_REQUIRED)
            self.assertTrue(options["context"].check_hostname)
            return connection
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "Serylane_0.7.13_x64-setup.exe"
            path.write_bytes(b"original-signed-bytes")
            with contextlib.redirect_stdout(io.StringIO()) as output:
                try:
                    result = upload(123, path, "Serylane Windows x64", "secret-token", factory)
                finally:
                    self.assertNotIn("secret-token", output.getvalue())
                    self.assertEqual(calls, ["uploads.github.com"])
            return result

    def test_exact_bytes_length_host_and_no_redirect_transport(self):
        c = Connection()
        self.assertEqual(self.exercise(c)["state"], "uploaded")
        self.assertEqual(c.body, b"original-signed-bytes")
        self.assertEqual(c.headers["Content-Length"], str(len(c.body)))
        self.assertEqual(c.headers["Authorization"], "Bearer secret-token")
        self.assertEqual(c.method, "POST")
        self.assertIn("label=Serylane+Windows+x64", c.endpoint)
        self.assertTrue(c.closed)
        self.assertEqual(c.response_timeout, 120)

    def test_rejections_are_not_retried_or_followed(self):
        for status in [302, 403, 422, 502]:
            c = Connection(status=status)
            with self.assertRaisesRegex(UploadError, f"HTTP {status}"):
                self.exercise(c)
            self.assertTrue(c.closed)

    def test_uncertain_timeout_is_sanitized_and_never_retried(self):
        c = Connection(fail=True)
        with self.assertRaisesRegex(UploadError, "^Upload transport failed; inspect draft before resuming$"):
            self.exercise(c)
        self.assertTrue(c.closed)

    def test_total_deadline_is_bounded(self):
        with patch("recover_github_upload.time.monotonic", side_effect=[0, 301]):
            with self.assertRaisesRegex(UploadError, "deadline exceeded"):
                self.exercise(Connection())


if __name__ == "__main__":
    unittest.main()
