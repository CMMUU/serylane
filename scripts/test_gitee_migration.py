"""Offline tests of the explicitly authorized, identity-bound URL migration."""
import unittest
from types import SimpleNamespace

from sync_gitee import Api, NotFoundError, SyncError, migrate_legacy_path, restore_public_repository
from test_sync_gitee import Opener
from urllib.error import HTTPError


class GiteeMigrationTests(unittest.TestCase):
    def test_public_restoration_is_explicit_identity_bound_and_idempotent(self):
        for uncertain in (False, True):
            gh, ge, target, writes = self.fixture("serylane", uncertain=uncertain)
            target["private"] = True
            self.assertFalse(restore_public_repository(gh, ge, ge))
            self.assertEqual(writes, [])
            self.assertTrue(restore_public_repository(gh, ge, ge, True))
            self.assertFalse(target["private"])
            self.assertEqual(len(writes), 1)
            self.assertEqual(writes[0]["private"], "false")
            self.assertTrue(restore_public_repository(gh, ge, ge, True))
            self.assertEqual(len(writes), 1)

    def test_public_restore_never_guesses_visibility_or_changes_other_repositories(self):
        for key, value in [("private", None), ("private", "true"), ("id", 1), ("path", "other"), ("owner", {"login": "other"}), ("name", "other"), ("default_branch", "other")]:
            gh, ge, target, writes = self.fixture("serylane")
            target[key] = value
            with self.assertRaises(SyncError): restore_public_repository(gh, ge, ge, True)
            self.assertEqual(writes, [])

    def test_anonymous_probe_has_no_credentials_and_must_see_original_public_repo(self):
        self.assertNotIn("Authorization", Api("gitee", "").headers())
        gh, ge, target, writes = self.fixture("serylane")
        anon = SimpleNamespace(request=lambda path: {"id": 1, "private": False})
        with self.assertRaisesRegex(SyncError, "anonymous"): restore_public_repository(gh, ge, anon, True)
        self.assertEqual(writes, [])

    def test_delayed_opening_is_reported_without_repeating_privacy_write(self):
        gh, ge, target, writes = self.fixture("serylane")
        target["private"] = True
        original = ge.request
        def request(path, method="GET", data=None):
            result = original(path, method, data)
            if method == "PATCH": target["private"] = True
            return result
        ge.request = request
        with self.assertRaisesRegex(SyncError, "opening/review"): restore_public_repository(gh, ge, ge, True)
        self.assertEqual(len(writes), 1)

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
