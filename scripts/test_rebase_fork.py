#!/usr/bin/env python3
"""Exercise the CI fork rebase against real, isolated Git histories."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import rebase_fork


RUN = subprocess.run


class TestRebaseFork(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name)
        self.upstream = root / "upstream"
        self.checkout = root / "checkout"
        self.upstream.mkdir()
        self.git(self.upstream, "init", "-b", "trunk")
        self.git(self.upstream, "config", "user.name", "Test")
        self.git(self.upstream, "config", "user.email", "test@example.com")
        (self.upstream / "base.txt").write_text("base\n")
        self.git(self.upstream, "add", ".")
        self.git(self.upstream, "commit", "-m", "base")
        self.git(root, "clone", str(self.upstream), str(self.checkout))
        self.git(self.checkout, "config", "user.name", "Test")
        self.git(self.checkout, "config", "user.email", "test@example.com")
        self.git(
            self.checkout, "config",
            f"url.{self.upstream}.insteadOf", "https://github.com/parent/project.git",
        )
        self.metadata = {
            "fork": True,
            "parent": {"full_name": "parent/project", "default_branch": "trunk"},
        }

    def git(self, cwd, *args):
        return RUN(
            ["git", *args], cwd=cwd, check=True,
            text=True, capture_output=True,
        ).stdout.strip()

    def commit(self, repo, name, content):
        (repo / name).write_text(content)
        self.git(repo, "add", name)
        self.git(repo, "commit", "-m", name)
        return self.git(repo, "rev-parse", "HEAD")

    def invoke(self, api_error=False):
        def run(args, **kwargs):
            if args[0] == "gh":
                self.assertEqual(args, ["gh", "api", "repos/fork/project"])
                if api_error:
                    raise subprocess.CalledProcessError(1, args, stderr="API denied")
                return subprocess.CompletedProcess(args, 0, json.dumps(self.metadata))
            return RUN(args, cwd=self.checkout, **kwargs)

        with patch.dict(os.environ, {"GITHUB_REPOSITORY": "fork/project"}):
            with patch.object(rebase_fork.subprocess, "run", side_effect=run):
                return rebase_fork.main()

    def test_non_fork_is_unchanged(self):
        self.metadata = {"fork": False}
        before = self.git(self.checkout, "rev-parse", "HEAD")
        self.assertEqual(self.invoke(), 0)
        self.assertEqual(self.git(self.checkout, "rev-parse", "HEAD"), before)
        self.assertEqual(self.git(self.checkout, "remote"), "origin")

    def test_detached_fork_rebases_and_does_not_push(self):
        fork_tip = self.commit(self.checkout, "fork.txt", "fork work\n")
        self.git(self.checkout, "checkout", "--detach")
        upstream_tip = self.commit(self.upstream, "upstream.txt", "new upstream\n")
        self.assertEqual(self.invoke(), 0)
        self.git(self.checkout, "merge-base", "--is-ancestor", upstream_tip, "HEAD")
        self.assertEqual((self.checkout / "fork.txt").read_text(), "fork work\n")
        self.assertEqual((self.checkout / "upstream.txt").read_text(), "new upstream\n")
        self.assertNotEqual(self.git(self.checkout, "rev-parse", "HEAD"), fork_tip)
        self.assertEqual(self.git(self.checkout, "rev-parse", "trunk"), fork_tip)
        self.assertEqual(self.git(self.upstream, "rev-parse", "HEAD"), upstream_tip)

    def test_already_current_fork_is_unchanged(self):
        before = self.git(self.checkout, "rev-parse", "HEAD")
        self.assertEqual(self.invoke(), 0)
        self.assertEqual(self.git(self.checkout, "rev-parse", "HEAD"), before)

    def test_conflict_fails_and_aborts_rebase(self):
        fork_tip = self.commit(self.checkout, "base.txt", "fork change\n")
        self.commit(self.upstream, "base.txt", "upstream change\n")
        self.assertEqual(self.invoke(), 1)
        self.assertEqual(self.git(self.checkout, "rev-parse", "HEAD"), fork_tip)
        self.assertEqual(self.git(self.checkout, "status", "--porcelain"), "")

    def test_api_failure_stops_ci(self):
        self.assertEqual(self.invoke(api_error=True), 1)

    def test_missing_parent_stops_ci(self):
        self.metadata["parent"] = None
        self.assertEqual(self.invoke(), 1)

    def test_missing_fork_flag_stops_ci(self):
        self.metadata = {}
        self.assertEqual(self.invoke(), 1)

    def test_fetch_failure_stops_ci(self):
        self.metadata["parent"]["default_branch"] = "missing"
        before = self.git(self.checkout, "rev-parse", "HEAD")
        self.assertEqual(self.invoke(), 1)
        self.assertEqual(self.git(self.checkout, "rev-parse", "HEAD"), before)


if __name__ == "__main__":
    unittest.main(verbosity=2)
