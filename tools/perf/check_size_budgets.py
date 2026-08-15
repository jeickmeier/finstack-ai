#!/usr/bin/env python3
"""Fail when ratified wheel, WASM, or CLI size budgets are exceeded."""

from __future__ import annotations

import argparse
import gzip
import json
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
TABLE_PATH = REPO_ROOT / "docs" / "implementation" / "perf-size-budgets.json"


def load_table() -> dict[str, object]:
    return json.loads(TABLE_PATH.read_text(encoding="utf-8"))


def resolve_path(entry: dict[str, object]) -> Path | None:
    raw_path = entry.get("path")
    if isinstance(raw_path, str):
        path = REPO_ROOT / raw_path
        return path if path.is_file() else None
    glob = entry.get("path_glob")
    if isinstance(glob, str):
        matches = sorted(REPO_ROOT.glob(glob))
        return matches[-1] if matches else None
    return None


def check_entry(entry: dict[str, object]) -> dict[str, object]:
    artifact_id = str(entry["id"])
    path = resolve_path(entry)
    required = bool(entry.get("required", False))
    result: dict[str, object] = {
        "id": artifact_id,
        "path": None if path is None else str(path.relative_to(REPO_ROOT)),
        "present": path is not None,
        "required": required,
        "within_budget": True,
    }
    if path is None:
        result["within_budget"] = not required
        result["detail"] = (
            "missing required artifact" if required else "optional artifact absent"
        )
        return result
    raw = path.read_bytes()
    raw_len = len(raw)
    gzip_len = len(gzip.compress(raw))
    result["bytes"] = raw_len
    result["gzip_bytes"] = gzip_len
    max_bytes = entry.get("max_bytes")
    max_gzip = entry.get("max_gzip_bytes")
    over: list[str] = []
    if isinstance(max_bytes, int) and raw_len > max_bytes:
        over.append(f"{raw_len} > {max_bytes} bytes")
    if isinstance(max_gzip, int) and gzip_len > max_gzip:
        over.append(f"{gzip_len} > {max_gzip} gzip bytes")
    if over:
        result["within_budget"] = False
        result["detail"] = "; ".join(over)
    else:
        result["detail"] = "within ratified budget or unratified optional"
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--out",
        type=Path,
        default=REPO_ROOT
        / "docs"
        / "implementation"
        / "artifacts"
        / "pr-063"
        / "size-budget-report.json",
    )
    args = parser.parse_args()
    table = load_table()
    artifacts = table.get("artifacts")
    if not isinstance(artifacts, list):
        print("error: size-budget table is missing artifacts", file=sys.stderr)
        return 1
    report = {
        "format_version": 1,
        "workload": table.get("workload"),
        "results": [
            check_entry(entry) for entry in artifacts if isinstance(entry, dict)
        ],
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    failed = False
    for result in report["results"]:
        status = "PASS" if result["within_budget"] else "FAIL"
        print(f"{status} {result['id']}: {result['detail']}")
        if not result["within_budget"]:
            failed = True
    print(f"wrote {args.out.relative_to(REPO_ROOT)}")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
