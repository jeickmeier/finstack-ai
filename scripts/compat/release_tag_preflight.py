#!/usr/bin/env python3
"""Fail when the intended release tag already names a different commit."""

from __future__ import annotations

import argparse
import re
import subprocess
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
_RELEASE_TAG = re.compile(r"v[0-9]+\.[0-9]+\.[0-9]+")


def validated_tag(value: str) -> str:
    """Return ``value`` when it is a ``vMAJOR.MINOR.PATCH`` tag."""
    match = _RELEASE_TAG.fullmatch(value)
    if match is None:
        raise SystemExit("release tag must look like v1.0.0")
    return match.group(0)


def git(*args: str) -> str:
    return subprocess.run(
        ["git", *args],
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tag", help="intended release tag, for example v1.0.0")
    args = parser.parse_args()
    tag = validated_tag(args.tag)

    head = git("rev-parse", "HEAD")
    existing = subprocess.run(
        ["git", "rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}"],
        cwd=REPO_ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if existing.returncode != 0:
        print(f"ok: {tag} is unused")
        return 0
    tagged = existing.stdout.strip()
    if tagged != head:
        raise SystemExit(
            f"release tag conflict: {tag} names {tagged}, candidate is {head}"
        )
    print(f"ok: {tag} already names the candidate commit")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
