"""Offline tests of the explicitly authorized, identity-bound URL migration."""
import unittest
from types import SimpleNamespace

from sync_gitee import Api, NotFoundError, SyncError, migrate_legacy_path
from test_sync_gitee import Opener
from urllib.error import HTTPError


class GiteeMigrationTests(unittest.TestCase):
    def fixture(self, slug="routedeck", uncertain=False):
        target = {"id": 50078322, "name": "Serylane", "path": slug,
                  "owner": {"login": "cmmuu"}, "private": False,
                  "default_branch": "main", "has_issues": False, "has_wiki": True}
        writes = []
        def request(path, method="GET", data=None):
            if path == "/user":
                return {"login": "cmmuu"}
            if path.endswith("/branches/main"):
                return {"commit": {"sha": "a" * 40}}
            if path != "/repos/cmmuu/" + target["path"]:
                raise NotFoundError("missing")
            if method == "PATCH":
                writes.append(data.copy())
                target.update({key: (value == "true" if value in ("true", "false") else value)
                               for key, value in data.items()})
                if uncertain:
                    raise SyncError("lost response")
            return target.copy()
        github = SimpleNamespace(request=lambda path: {
            "id": 1355770287, "full_name": "CMMUU/serylane", "private": False})
        gitee = SimpleNamespace(request=request, pages=lambda path: [{"id": 42, "tag_name": "v0.7.6"}])
        return github, gitee, target, writes

    def test_same_repository_is_renamed_once_and_preserves_settings(self):
        for uncertain in (False, True):
            gh, ge, target, writes = self.fixture(uncertain=uncertain)
            self.assertTrue(migrate_legacy_path(gh, ge, True))
            self.assertEqual(writes, [{"name": "Serylane", "path": "serylane",
                                      "has_issues": "false", "has_wiki": "true"}])
            self.assertEqual(target["id"], 50078322)
            self.assertTrue(migrate_legacy_path(gh, ge, True))
            self.assertEqual(len(writes), 1)

    def test_preview_and_already_migrated_repository_do_not_write(self):
        gh, ge, target, writes = self.fixture()
        self.assertFalse(migrate_legacy_path(gh, ge))
        self.assertEqual(writes, [])
        target["path"] = "serylane"
        self.assertTrue(migrate_legacy_path(gh, ge))
        self.assertEqual(writes, [])

    def test_conflicts_and_changed_identity_stop_before_patch(self):
        for slug in ("routedeck", "serylane"):
            for key, value in (("id", 1), ("private", True), ("default_branch", "other"),
                               ("owner", {"login": "other"})):
                gh, ge, target, writes = self.fixture(slug)
                target[key] = value
                with self.assertRaisesRegex(SyncError, "mismatched fields: " + key):
                    migrate_legacy_path(gh, ge, True)
                self.assertEqual(writes, [])
        gh, ge, target, writes = self.fixture()
        gh.request = lambda path: {"id": 1}
        with self.assertRaises(SyncError):
            migrate_legacy_path(gh, ge, True)
        self.assertEqual(writes, [])

    def test_post_write_verification_detects_changed_releases(self):
        gh, ge, target, writes = self.fixture()
        ge.pages = lambda path: [{"id": 42 if "routedeck" in path else 43, "tag_name": "v0.7.6"}]
        with self.assertRaisesRegex(SyncError, "preserve"):
            migrate_legacy_path(gh, ge, True)
        self.assertEqual(len(writes), 1)

    def test_only_explicit_not_found_allows_migration(self):
        gh, ge, target, writes = self.fixture()
        original = ge.request
        def unavailable(path, *args):
            if path.endswith("/serylane"):
                raise SyncError("network unavailable")
            return original(path, *args)
        ge.request = unavailable
        with self.assertRaises(SyncError):
            migrate_legacy_path(gh, ge, True)
        self.assertEqual(writes, [])
        api = Api("gitee", "offline")
        api.opener = Opener([HTTPError("https://gitee.com", 404, "missing", {}, None)])
        with self.assertRaises(NotFoundError):
            api.request("/repos/cmmuu/serylane")
        self.assertEqual(len(api.opener.requests), 1)


if __name__ == "__main__":
    unittest.main()
