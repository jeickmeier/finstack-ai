#!/usr/bin/env python3
"""Run Criterion benches and stage machine-readable benchmark metadata.

Thin entry point for ``mise run benchmark`` / ``benchmark-smoke``. Benchmark
regression is intentionally non-merge-blocking (PR-003 reservation; PR-005).
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import subprocess
import time
from collections.abc import Sequence
from pathlib import Path

TOOL_DIR = Path(__file__).resolve().parent
REPO_ROOT = TOOL_DIR.parents[1]
PACKAGE = "finstack-ai-test"
WORKLOAD = "conformance-noop-and-pr009-reducer-groups"
ARTIFACT_ROOT = REPO_ROOT / "target" / "benchmark"
METADATA_SCHEMA = (
    REPO_ROOT / "schemas" / "benchmark-report" / "v1" / "metadata.schema.json"
)


def run(command: list[str], *, cwd: Path, env: dict[str, str] | None = None) -> None:
    """Run a command and fail loudly on non-zero exit."""
    print("+", " ".join(command), flush=True)
    subprocess.run(command, cwd=cwd, env=env, check=True)


def require_ci_commit() -> bool:
    """Return True when CI must fail closed without a git commit identity."""
    return os.environ.get("CI") == "true" or os.environ.get("GITHUB_ACTIONS") == "true"


def git_commit(*, require: bool | None = None) -> str:
    """Return the current HEAD commit."""
    if require is None:
        require = require_ci_commit()
    try:
        completed = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=REPO_ROOT,
            check=True,
            capture_output=True,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError) as exc:
        if require:
            raise SystemExit("benchmark requires a git commit identity in CI") from exc
        return "unknown"
    commit = completed.stdout.strip()
    if not commit:
        if require:
            raise SystemExit("benchmark received empty git commit identity in CI")
        return "unknown"
    return commit


def rustc_version() -> str:
    """Return the active rustc version string."""
    completed = subprocess.run(
        ["rustc", "--version"],
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return completed.stdout.strip()


def host_triple() -> str:
    """Return the host target triple from rustc."""
    completed = subprocess.run(
        ["rustc", "-vV"],
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    for line in completed.stdout.splitlines():
        if line.startswith("host:"):
            return line.split(":", 1)[1].strip()
    raise RuntimeError("unable to determine rustc host triple")


def package_features() -> list[str]:
    """Return declared features for the benchmark package."""
    completed = subprocess.run(
        [
            "cargo",
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--no-deps",
        ],
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    metadata = json.loads(completed.stdout)
    for package in metadata.get("packages", []):
        if package.get("name") != PACKAGE:
            continue
        features = package.get("features") or {}
        return sorted(str(name) for name in features)
    raise SystemExit(f"package {PACKAGE} not found in cargo metadata")


def build_metadata(
    *,
    smoke: bool,
    raw_dir: Path,
    metadata_path: Path,
) -> dict[str, object]:
    """Construct schema-shaped benchmark metadata."""
    sample_size = 10 if smoke else 20
    measurement = 0.2 if smoke else 1.0
    warm_up = 0.1 if smoke else 0.3
    return {
        "format_version": 1,
        "package": PACKAGE,
        "workload": WORKLOAD,
        "commit": git_commit(),
        "rustc": rustc_version(),
        "target": host_triple(),
        "host_triple": host_triple(),
        "platform": {
            "system": platform.system(),
            "machine": platform.machine(),
            "release": platform.release(),
        },
        "features": package_features(),
        "profile": "bench",
        "sample_settings": {
            "measurement_time_secs": measurement,
            "sample_size": sample_size,
            "warm_up_time_secs": warm_up,
        },
        "artifact_paths": {
            "raw_samples_dir": str(raw_dir.relative_to(REPO_ROOT)),
            "metadata_file": str(metadata_path.relative_to(REPO_ROOT)),
        },
        "generated_at_unix_ms": int(time.time() * 1000),
        "notes": (
            "Non-blocking aggregate Criterion evidence: no-op fixture "
            "load/normalize/compare and real PR-009 reducer execution groups. "
            "Framework-only paths; no external model latency."
        ),
    }


def validate_metadata(metadata: dict[str, object]) -> None:
    """Validate metadata against the repository JSON Schema using stdlib checks.

    Full draft validation is performed by the Rust harness and fixture corpus.
    This helper enforces required fields and rejects unknown top-level keys so
    the Python staging path cannot silently drift.
    """
    schema = json.loads(METADATA_SCHEMA.read_text(encoding="utf-8"))
    required = schema.get("required", [])
    properties = schema.get("properties", {})
    allowed = set(properties)
    missing = [key for key in required if key not in metadata]
    if missing:
        raise SystemExit(f"benchmark metadata missing required fields: {missing}")
    unknown = sorted(set(metadata) - allowed)
    if unknown:
        raise SystemExit(f"benchmark metadata has unknown fields: {unknown}")
    if metadata.get("format_version") != 1:
        raise SystemExit("benchmark metadata format_version must be 1")


def compile_benches() -> None:
    """Compile Criterion benches without running them."""
    run(
        [
            "cargo",
            "bench",
            "-p",
            PACKAGE,
            "--bench",
            "conformance",
            "--locked",
            "--no-run",
        ],
        cwd=REPO_ROOT,
    )


def run_benches(*, smoke: bool, raw_dir: Path) -> None:
    """Execute Criterion and stage raw samples under ``raw_dir``."""
    raw_dir.mkdir(parents=True, exist_ok=True)
    # Criterion writes under CARGO_TARGET_DIR/criterion by default.
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(REPO_ROOT / "target")
    args = [
        "cargo",
        "bench",
        "-p",
        PACKAGE,
        "--bench",
        "conformance",
        "--locked",
    ]
    if smoke:
        # `--quick` is mutually exclusive with explicit sample/time overrides.
        args.extend(["--", "--quick"])
    run(args, cwd=REPO_ROOT, env=env)
    criterion_dir = REPO_ROOT / "target" / "criterion"
    if criterion_dir.is_dir():
        staged = raw_dir / "criterion"
        if staged.exists():
            shutil.rmtree(staged)
        shutil.copytree(criterion_dir, staged)


def write_metadata(metadata: dict[str, object], path: Path) -> None:
    """Write sorted JSON metadata beside raw samples."""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(metadata, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "mode",
        choices=("smoke", "full", "compile"),
        help="compile-only, short smoke run, or full non-blocking benchmark",
    )
    args = parser.parse_args(argv)

    if args.mode == "compile":
        compile_benches()
        print("benchmark: compile ok")
        return 0

    smoke = args.mode == "smoke"
    raw_dir = ARTIFACT_ROOT / ("smoke" if smoke else "full") / "raw"
    metadata_path = ARTIFACT_ROOT / ("smoke" if smoke else "full") / "metadata.json"
    run_benches(smoke=smoke, raw_dir=raw_dir)
    metadata = build_metadata(smoke=smoke, raw_dir=raw_dir, metadata_path=metadata_path)
    validate_metadata(metadata)
    write_metadata(metadata, metadata_path)
    print(f"benchmark: wrote {metadata_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
