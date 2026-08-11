#!/usr/bin/env python3
"""Evaluate versioned warning-only native preview thresholds."""

from __future__ import annotations

import json
import os
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
THRESHOLDS = Path(__file__).with_name("thresholds-v1.json")
OUTPUT = REPO_ROOT / "target" / "preview" / "performance-report.json"


def read_json(path: Path) -> dict[str, object]:
    """Read one required JSON object."""
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise SystemExit(f"expected JSON object: {path}")
    return value


def criterion_mean(path: Path) -> float:
    """Read Criterion's mean point estimate in nanoseconds."""
    value = read_json(path)
    try:
        return float(value["mean"]["point_estimate"])  # type: ignore[index]
    except (KeyError, TypeError, ValueError) as exc:
        raise SystemExit(f"invalid Criterion estimate: {path}") from exc


def release_binary() -> Path:
    """Return the platform-specific staged release-smoke binary."""
    name = "finstack-ai-ci-smoke.exe" if os.name == "nt" else "finstack-ai-ci-smoke"
    return REPO_ROOT / "target" / "ci-release" / name


def evaluate() -> dict[str, object]:
    """Evaluate all initial warning thresholds and return a report."""
    policy = read_json(THRESHOLDS)
    thresholds = policy.get("thresholds")
    if not isinstance(thresholds, dict):
        raise SystemExit("preview threshold policy is missing thresholds")
    idle = read_json(
        REPO_ROOT
        / "target"
        / "benchmark"
        / "smoke"
        / "raw"
        / "idle-session-memory.json"
    )
    measurements = {
        "idle_bytes_per_session": int(idle["bytes_per_session"]),
        "model_reducer_mean_ns": criterion_mean(
            REPO_ROOT
            / "target"
            / "criterion"
            / "scripted_model_reducer"
            / "execute_model_only_completion"
            / "new"
            / "estimates.json"
        ),
        "model_stream_256_items_mean_ns": criterion_mean(
            REPO_ROOT
            / "target"
            / "criterion"
            / "model_stream_throughput"
            / "assemble_256_text_items"
            / "new"
            / "estimates.json"
        ),
        "release_smoke_binary_bytes": release_binary().stat().st_size,
    }
    checks = []
    for name, value in measurements.items():
        maximum = thresholds[f"{name}_max"]
        status = "pass" if float(value) <= float(maximum) else "warning"
        checks.append(
            {
                "measurement": name,
                "value": value,
                "maximum": maximum,
                "status": status,
            }
        )
    return {
        "format_version": policy["format_version"],
        "mode": policy["mode"],
        "checks": checks,
        "status": "warning"
        if any(c["status"] == "warning" for c in checks)
        else "pass",
    }


def main() -> int:
    """Write and print the deterministic performance warning report."""
    report = evaluate()
    OUTPUT.parent.mkdir(parents=True, exist_ok=True)
    OUTPUT.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps(report, indent=2, sort_keys=True))
    print(f"preview performance: wrote {OUTPUT}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
