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


def main() -> int:
    failures: list[str] = []
    for path in tracked_files():
        if not path.is_file():
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        relative_path = path.relative_to(REPO_ROOT)
        for line_number, line in enumerate(text.splitlines(), start=1):
            if any(pattern.search(line) for pattern in PATTERNS):
                failures.append(
                    f"{relative_path.as_posix()}:{line_number}:{line.strip()}"
                )
            if path.suffix.lower() == ".md":
                matches = list(INLINE_LINK.finditer(line))
                reference = REFERENCE_LINK.match(line)
                if reference is not None:
                    matches.append(reference)
                for match in matches:
                    target = match.group("target").strip().strip("<>")
                    target = target.split(maxsplit=1)[0]
                    if (
                        not target
                        or target.startswith(("#", "/"))
                        or target.lower().startswith(REMOTE_SCHEMES)
                    ):
                        continue
                    local = unquote(target.split("#", 1)[0].split("?", 1)[0])
                    if local and not (path.parent / local).exists():
                        failures.append(
                            f"{relative_path.as_posix()}:{line_number}:"
                            f"missing Markdown link target {target}"
                        )
    if failures:
        print("stale repository references found:")
        print("\n".join(failures))
        return 1
    print("ok: repository references are current")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
