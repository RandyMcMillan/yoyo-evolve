#!/usr/bin/env python3
"""Rebase a fork's CI checkout onto its parent's default branch; never push."""
import json
import os
import re
import subprocess
import sys


def main():
    repository = os.environ.get("GITHUB_REPOSITORY", "")
    if not re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", repository):
        print("::error::GITHUB_REPOSITORY must be owner/repo.", file=sys.stderr)
        return 1

    try:
        response = subprocess.run(
            ["gh", "api", f"repos/{repository}"],
            check=True, capture_output=True, text=True,
        )
        metadata = json.loads(response.stdout)
        if not isinstance(metadata, dict) or type(metadata.get("fork")) is not bool:
            raise ValueError("GitHub metadata has no boolean fork flag")
        if not metadata["fork"]:
            print("Not a fork; continuing CI without a rebase.", flush=True)
            return 0

        parent = metadata.get("parent")
        if not isinstance(parent, dict):
            raise ValueError("Fork metadata has no parent repository")
        upstream = parent.get("full_name")
        branch = parent.get("default_branch")
        if not isinstance(upstream, str) or not re.fullmatch(
            r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", upstream
        ):
            raise ValueError("Fork parent has no valid repository name")
        if not isinstance(branch, str) or not branch:
            raise ValueError("Fork parent has no default branch")
        subprocess.run(["git", "check-ref-format", f"refs/heads/{branch}"], check=True)
        print(f"Fork detected: rebasing CI checkout onto {upstream}:{branch}", flush=True)
        subprocess.run(
            ["git", "fetch", "--no-tags", f"https://github.com/{upstream}.git",
             f"+refs/heads/{branch}:refs/ci/upstream"],
            check=True,
        )
        result = subprocess.run(
            ["git", "-c", "user.name=GitHub Actions",
             "-c", "user.email=github-actions[bot]@users.noreply.github.com",
             "rebase", "--no-autostash", "refs/ci/upstream"],
        )
        if result.returncode:
            subprocess.run(["git", "rebase", "--abort"], check=True)
            raise ValueError("Upstream rebase failed; resolve conflicts before continuing CI")
        print("Upstream rebase complete (checkout only; nothing pushed).", flush=True)
        return 0
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        print(f"::error::Fork rebase failed: {error}", file=sys.stderr)
        if isinstance(error, subprocess.CalledProcessError) and error.stderr:
            print(error.stderr, file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
