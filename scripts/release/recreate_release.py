#!/usr/bin/env python3
"""Verify two-run checksum identity for an unpublished 1.0.0 staging build."""

from __future__ import annotations

import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from release_stage import REPO_ROOT, checksum_text, stage

WORK = REPO_ROOT / "target" / "release-staging" / "recreate"
RECORD = WORK / "records"


def main() -> int:
    if WORK.exists():
        shutil.rmtree(WORK)
    first = WORK / "run-a"
    second = WORK / "run-b"
    stage(first, label="1.0.0")
    stage(second, label="1.0.0")
    left = checksum_text(first)
    right = checksum_text(second)
    RECORD.mkdir(parents=True, exist_ok=True)
    (RECORD / "run-a.SHA256SUMS").write_text(left, encoding="utf-8")
    (RECORD / "run-b.SHA256SUMS").write_text(right, encoding="utf-8")
    shutil.copy2(first / "provenance.json", RECORD / "provenance.json")
    if left != right:
        print("error: two recreate runs produced different SHA-256 sets")
        return 1
    print(f"ok two-run recreate identity under {WORK.relative_to(REPO_ROOT)}")
    print(f"recorded checksums under {RECORD.relative_to(REPO_ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
