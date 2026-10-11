"""Reject references to removed planning and delivery-history artifacts."""

from __future__ import annotations

import re
import subprocess
from pathlib import Path
from urllib.parse import unquote

REPO_ROOT = Path(__file__).resolve().parents[2]
PATTERNS = (
    re.compile(("p" + "r-") + r"\d{3}", re.IGNORECASE),
    re.compile(r"\blogical" + r"[- ]" + "p" + r"r\b", re.IGNORECASE),
    re.compile("delivery" + "-ledger", re.IGNORECASE),
    re.compile("evidence" + "-register", re.IGNORECASE),
    re.compile("docs/" + r"(?:planning|implementation|site|rfcs)(?:/|\b)"),
    re.compile(r"\bTDD(?:\s+§|\s+\\u00a7|\s+\d)", re.IGNORECASE),
)
INLINE_LINK = re.compile(r"!?\[[^\]]*\]\((?P<target>[^)]+)\)")
REFERENCE_LINK = re.compile(r"^\s*\[[^\]]+\]:\s*(?P<target>\S+)")
REMOTE_SCHEMES = ("http://", "https://", "mailto:", "data:")


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


def link_targets(line: str) -> list[str]:
    """Return the inline and reference-definition link targets on ``line``."""
    matches = list(INLINE_LINK.finditer(line))
    reference = REFERENCE_LINK.match(line)
    if reference is not None:
        matches.append(reference)
    return [
        match.group("target").strip().strip("<>").split(maxsplit=1)[0]
        for match in matches
    ]


def is_missing_local_target(directory: Path, target: str) -> bool:
    """Return whether ``target`` names a relative path absent under ``directory``."""
    if (
        not target
        or target.startswith(("#", "/"))
        or target.lower().startswith(REMOTE_SCHEMES)
    ):
        return False
    local = unquote(target.split("#", 1)[0].split("?", 1)[0])
    return bool(local) and not (directory / local).exists()


def file_failures(path: Path) -> list[str]:
    """Return stale-reference and broken-link failures for one tracked file."""
    try:
        text = path.read_text(encoding="utf-8")
    except UnicodeDecodeError:
        return []
    relative = path.relative_to(REPO_ROOT).as_posix()
    is_markdown = path.suffix.lower() == ".md"
    failures: list[str] = []
    for line_number, line in enumerate(text.splitlines(), start=1):
        if any(pattern.search(line) for pattern in PATTERNS):
            failures.append(f"{relative}:{line_number}:{line.strip()}")
        if not is_markdown:
            continue
        failures.extend(
            f"{relative}:{line_number}:missing Markdown link target {target}"
            for target in link_targets(line)
            if is_missing_local_target(path.parent, target)
        )
    return failures


def main() -> int:
    failures: list[str] = []
    for path in tracked_files():
        if path.is_file():
            failures.extend(file_failures(path))
    if failures:
        print("stale repository references found:")
        print("\n".join(failures))
        return 1
    print("ok: repository references are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
