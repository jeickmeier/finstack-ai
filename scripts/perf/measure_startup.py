#!/usr/bin/env python3
"""Measure warm-FS CLI startup and write a secret-free report."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_OUT = (
    REPO_ROOT
    / "docs"
    / "implementation"
    / "artifacts"
    / "pr-063"
    / "startup-report.json"
)


def run(
    command: list[str], cwd: Path | None = None
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=cwd or REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )


def main() -> int:
    out = Path(sys.argv[1]) if len(sys.argv) > 1 else DEFAULT_OUT
    run(
        [
            "cargo",
            "build",
            "-p",
            "finstack-ai-native-examples",
            "--bin",
            "minimal",
            "--release",
            "--offline",
            "--locked",
        ]
    )
    binary = REPO_ROOT / "target" / "release" / "minimal"
    if not binary.is_file():
        print("error: release minimal binary is missing", file=sys.stderr)
        return 1
    # Cold-ish first run, then the warm-FS measurement.
    run([str(binary)])
    started = time.perf_counter()
    run([str(binary)])
    warm_ms = (time.perf_counter() - started) * 1000.0
    report = {
        "format_version": 1,
        "workload": "minimal-cli-startup-v1",
        "binary": "target/release/minimal",
        "bytes": binary.stat().st_size,
        "warm_full_example_run_ms": warm_ms,
        "note": "The in-tree minimal bin performs a scripted model-only run. NFR-PERF-006 warm-FS startup is process-image load; kernel init is the Criterion kernel_micro group. This script records size plus the warm full example.",
        "pid": os.getpid(),
    }
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
