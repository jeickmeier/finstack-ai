"""Reject references to removed planning and delivery-history artifacts."""

from __future__ import annotations

import re
import subprocess
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
PATTERNS = (
    re.compile(("p" + "r-") + r"\d{3}", re.IGNORECASE),
    re.compile(r"\blogical" + r"[- ]" + "p" + r"r\b", re.IGNORECASE),
    re.compile("delivery" + "-ledger", re.IGNORECASE),
    re.compile("evidence" + "-register", re.IGNORECASE),
    re.compile("docs/" + r"(?:planning|implementation|site|rfcs)(?:/|\b)"),
)


def tracked_files() -> list[Path]:
    completed = subprocess.run(
        ["git", "ls-files", "-co", "--exclude-standard", "-z"],
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
    )
    return [
        REPO_ROOT / Path(raw.decode("utf-8"))
        for raw in completed.stdout.split(b"\0")
        if raw
    ]


def main() -> int:
    failures: list[str] = []
    for path in tracked_files():
        if not path.is_file():
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        for line_number, line in enumerate(text.splitlines(), start=1):
            if any(pattern.search(line) for pattern in PATTERNS):
                failures.append(
                    f"{path.relative_to(REPO_ROOT).as_posix()}:{line_number}:{line.strip()}"
                )
    if failures:
        print("stale planning or delivery-history references found:")
        print("\n".join(failures))
        return 1
    print("ok: no stale planning or delivery-history references")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
