#!/usr/bin/env python3
"""Fail when the intended release tag already names a different commit."""

from __future__ import annotations

import argparse
import subprocess
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]


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

    head = git("rev-parse", "HEAD")
    existing = subprocess.run(
        ["git", "rev-parse", "--verify", f"refs/tags/{args.tag}^{{commit}}"],
        cwd=REPO_ROOT,
        check=False,
        capture_output=True,
        text=True,
    )
    if existing.returncode != 0:
        print(f"ok: {args.tag} is unused")
        return 0
    tagged = existing.stdout.strip()
    if tagged != head:
        raise SystemExit(
            f"release tag conflict: {args.tag} names {tagged}, candidate is {head}"
        )
    print(f"ok: {args.tag} already names the candidate commit")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
