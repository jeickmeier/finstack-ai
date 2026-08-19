#!/usr/bin/env python3
"""Baseline vs patch-line checksum pair (PR-065-A04)."""

from __future__ import annotations

import shutil
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from release_stage import REPO_ROOT, checksum_text, sha256, stage

WORK = REPO_ROOT / "target" / "pr-065-hotfix"
RECORD = REPO_ROOT / "docs" / "implementation" / "artifacts" / "pr-065"
PINNED = "crate-package-lists/finstack-ai.list"


def main() -> int:
    if WORK.exists():
        shutil.rmtree(WORK)
    baseline = WORK / "B"
    hotfix = WORK / "H"
    stage(baseline, label="1.0.0")
    stage(
        hotfix,
        label="1.0.0-hotfix",
        extra={
            "hotfix-line.txt": (
                "PR-065 patch-line rehearsal. Consumers pin SHA256SUMS-B. "
                "Maintainers would publish H as the next patch. "
                "Do not yank, publish, or tag from this rehearsal.\n"
            )
        },
    )
    sums_b = checksum_text(baseline)
    sums_h = checksum_text(hotfix)
    RECORD.mkdir(parents=True, exist_ok=True)
    (RECORD / "SHA256SUMS-B").write_text(sums_b, encoding="utf-8")
    (RECORD / "SHA256SUMS-H").write_text(sums_h, encoding="utf-8")
    if sums_b == sums_h:
        print("error: baseline and hotfix checksum sets are identical")
        return 1
    pinned = baseline / PINNED
    if not pinned.is_file():
        print(f"error: missing consumer pin artifact {PINNED}")
        return 1
    expected = sha256(pinned)
    listed = {
        line.split()[1]: line.split()[0] for line in sums_b.splitlines() if line.strip()
    }
    if listed.get(PINNED) != expected:
        print(f"error: consumer pin of {PINNED} does not verify against SHA256SUMS-B")
        return 1
    print(f"ok hotfix rehearsal B≠H; pin {PINNED}={expected}")
    print(f"recorded under {RECORD.relative_to(REPO_ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
