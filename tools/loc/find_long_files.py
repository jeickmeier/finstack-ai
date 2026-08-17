#!/usr/bin/env python3
"""List source files whose physical line count exceeds a threshold.

Default threshold is 1000 lines, matching the team's "files should stay
reviewable" bar. Vendor, cache, and generated trees are skipped so the
report is useful for refactor decisions.
"""

from __future__ import annotations

import argparse
import os
import sys
from collections.abc import Iterator
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_THRESHOLD = 1000
SKIP_DIR_NAMES = frozenset(
    {
        ".git",
        ".venv",
        "__pycache__",
        ".pytest_cache",
        ".ruff_cache",
        ".mypy_cache",
        "node_modules",
        "target",
        "wheels",
        "playwright-report",
        "test-results",
        "blob-report",
        "dist",
        "generated",
        "playwright",
    }
)
SOURCE_SUFFIXES = frozenset(
    {
        ".cjs",
        ".js",
        ".mjs",
        ".py",
        ".pyi",
        ".rs",
        ".ts",
        ".tsx",
        ".wit",
    }
)


def iter_source_files(root: Path, suffixes: frozenset[str]) -> Iterator[Path]:
    """Yield source files under ``root``, pruning skipped directories."""
    for dirpath, dirnames, filenames in os.walk(root, followlinks=False):
        dirnames[:] = sorted(name for name in dirnames if name not in SKIP_DIR_NAMES)
        for name in sorted(filenames):
            path = Path(dirpath) / name
            if path.suffix in suffixes:
                yield path


def count_lines(path: Path) -> int | None:
    """Return physical line count, or ``None`` if the file is not UTF-8 text."""
    try:
        text = path.read_text(encoding="utf-8")
    except (OSError, UnicodeDecodeError):
        return None
    if "\0" in text:
        return None
    return len(text.splitlines())


def find_long_files(
    root: Path,
    threshold: int,
    suffixes: frozenset[str],
) -> list[tuple[int, Path]]:
    """Return ``(line_count, path)`` pairs strictly above ``threshold``, longest first."""
    hits: list[tuple[int, Path]] = []
    for path in iter_source_files(root, suffixes):
        loc = count_lines(path)
        if loc is not None and loc > threshold:
            hits.append((loc, path))
    hits.sort(key=lambda item: (-item[0], str(item[1])))
    return hits


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--root",
        type=Path,
        default=REPO_ROOT,
        help="Directory to scan (default: repository root)",
    )
    parser.add_argument(
        "--threshold",
        type=int,
        default=DEFAULT_THRESHOLD,
        help=f"Report files with more than this many lines (default: {DEFAULT_THRESHOLD})",
    )
    parser.add_argument(
        "--extensions",
        default=",".join(sorted(SOURCE_SUFFIXES)),
        help="Comma-separated suffixes to include (default: source languages)",
    )
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    if args.threshold < 0:
        print("error: --threshold must be >= 0", file=sys.stderr)
        return 2
    root = args.root.resolve()
    if not root.is_dir():
        print(f"error: root is not a directory: {root}", file=sys.stderr)
        return 2
    suffixes = frozenset(
        item if item.startswith(".") else f".{item}"
        for item in (part.strip() for part in args.extensions.split(","))
        if item
    )
    hits = find_long_files(root, args.threshold, suffixes)
    if not hits:
        print(f"No source files exceed {args.threshold} lines under {root}")
        return 0
    loc_width = max(len(str(loc)) for loc, _ in hits)
    print(f"{'LOC':>{loc_width}}  Path")
    print(f"{'-' * loc_width}  ----")
    for loc, path in hits:
        try:
            display = path.relative_to(root)
        except ValueError:
            display = path
        print(f"{loc:>{loc_width}}  {display}")
    noun = "file" if len(hits) == 1 else "files"
    print()
    print(f"{len(hits)} {noun} exceed {args.threshold} lines")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
