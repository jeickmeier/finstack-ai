#!/usr/bin/env python3
"""Check relative markdown links on the PR-060 audited set."""

from __future__ import annotations

import re
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
LINK = re.compile(r"\[[^\]]*\]\(([^)]+)\)")
SKIP_PREFIXES = ("http://", "https://", "mailto:", "#")
ROOT_FILES = (
    "README.md",
    "CONTRIBUTING.md",
    "GOVERNANCE.md",
    "SECURITY.md",
)
DOC_ROOTS = (
    REPO_ROOT / "docs" / "README.md",
    REPO_ROOT / "docs" / "site",
    REPO_ROOT / "docs" / "rfcs",
    REPO_ROOT / "docs" / "implementation" / "README.md",
    REPO_ROOT / "docs" / "implementation" / "threat-model-g7-review.md",
    REPO_ROOT / "docs" / "implementation" / "release-rehearsal.md",
    REPO_ROOT / "docs" / "implementation" / "compatibility-governance.md",
    REPO_ROOT / "examples",
    REPO_ROOT / "plugins",
)


def markdown_files() -> list[Path]:
    files = [REPO_ROOT / name for name in ROOT_FILES]
    for root in DOC_ROOTS:
        if root.is_file():
            files.append(root)
        elif root.is_dir():
            files.extend(sorted(root.rglob("*.md")))
    return files


def targets(raw: str) -> list[str]:
    href = raw.strip()
    if href.startswith(SKIP_PREFIXES) or href.startswith("<"):
        return []
    if " " in href:
        href = href.split(" ", 1)[0]
    return [href.split("#", 1)[0]]


def main() -> int:
    missing: list[str] = []
    checked = 0
    for path in markdown_files():
        text = path.read_text(encoding="utf-8")
        for match in LINK.finditer(text):
            for href in targets(match.group(1)):
                if not href:
                    continue
                checked += 1
                dest = (path.parent / href).resolve()
                if not dest.exists():
                    missing.append(f"{path.relative_to(REPO_ROOT)} -> {href}")
    if missing:
        print("broken relative links:")
        for item in missing:
            print(f"  {item}")
        return 1
    print(f"ok {checked} relative links")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
