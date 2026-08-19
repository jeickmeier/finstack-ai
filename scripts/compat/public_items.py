#!/usr/bin/env python3
"""Extract and compare frozen public-item lists.

Python and JavaScript keep name-list baselines. Rust `pub use` scrape is
retired; `public_api.py` (cargo-public-api) owns Rust signatures.
"""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
PYTHON_INIT = (
    REPO_ROOT
    / "bindings"
    / "finstack-ai-python"
    / "python"
    / "finstack_ai"
    / "__init__.py"
)
JS_INDEX = REPO_ROOT / "bindings" / "finstack-ai-wasm" / "js" / "src" / "index.ts"
BASELINES = {
    "python": REPO_ROOT
    / "fixtures"
    / "compatibility"
    / "breaking"
    / "python"
    / "valid--v0.1.0-exports.txt",
    "js": REPO_ROOT
    / "fixtures"
    / "compatibility"
    / "breaking"
    / "js"
    / "valid--v0.1.0-exports.txt",
}
MUTATIONS = {
    "python": REPO_ROOT
    / "fixtures"
    / "compatibility"
    / "breaking"
    / "python"
    / "invalid--renamed-export.txt",
    "js": REPO_ROOT
    / "fixtures"
    / "compatibility"
    / "breaking"
    / "js"
    / "invalid--renamed-export.txt",
}

IDENT_RE = re.compile(r"\b([A-Z][A-Za-z0-9_]*|[a-z][A-Za-z0-9_]*)\b")
PY_FROM_RE = re.compile(r"from \.[^\n]+ import \((.*?)\)", re.S)
PY_DEF_RE = re.compile(r"^def ([A-Za-z_][A-Za-z0-9_]*)\(", re.M)
JS_EXPORT_RE = re.compile(r"export (?:type )?\{([^}]+)\}", re.S)
JS_FN_RE = re.compile(r"^export (?:async )?function ([A-Za-z_][A-Za-z0-9_]*)", re.M)
JS_CONST_RE = re.compile(r"^export const ([A-Za-z_][A-Za-z0-9_]*)", re.M)


def python_items() -> list[str]:
    text = PYTHON_INIT.read_text(encoding="utf-8")
    names: set[str] = set()
    for block in PY_FROM_RE.findall(text):
        for ident in IDENT_RE.findall(block):
            if ident in {"cast", "TypedDict"}:
                continue
            names.add(ident)
    names.update(PY_DEF_RE.findall(text))
    names.update({"__version__", "BuildMetadata"})
    return sorted(names)


def js_items() -> list[str]:
    text = JS_INDEX.read_text(encoding="utf-8")
    names: set[str] = set()
    for block in JS_EXPORT_RE.findall(text):
        for part in block.split(","):
            ident = part.strip().split(" as ")[-1].strip()
            if ident:
                names.add(ident)
    names.update(JS_FN_RE.findall(text))
    names.update(JS_CONST_RE.findall(text))
    return sorted(names)


EXTRACTORS = {"python": python_items, "js": js_items}


def write_list(path: Path, items: list[str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(items) + "\n", encoding="utf-8")


def read_list(path: Path) -> list[str]:
    return [line for line in path.read_text(encoding="utf-8").splitlines() if line]


def compare(current: list[str], baseline: list[str]) -> list[str]:
    missing = [item for item in baseline if item not in set(current)]
    return missing


def added(current: list[str], baseline: list[str]) -> list[str]:
    known = set(baseline)
    return [item for item in current if item not in known]


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if not args.write and not args.check:
        parser.error("pass --write or --check")
    rust = subprocess.run(
        [
            sys.executable,
            str(REPO_ROOT / "scripts" / "compat" / "public_api.py"),
            "--write" if args.write else "--check",
        ],
        cwd=REPO_ROOT,
        check=False,
    )
    failed = rust.returncode != 0
    for family, extract in EXTRACTORS.items():
        current = extract()
        baseline_path = BASELINES[family]
        mutation_path = MUTATIONS[family]
        if args.write:
            write_list(baseline_path, current)
            mutated = list(current)
            if not mutated:
                raise SystemExit(f"{family}: empty public list")
            mutated[0] = mutated[0] + "Renamed"
            write_list(mutation_path, mutated)
            continue
        if not baseline_path.is_file() or not mutation_path.is_file():
            print(f"{family}: missing baseline or mutation fixture", file=sys.stderr)
            failed = True
            continue
        baseline = read_list(baseline_path)
        missing = compare(current, baseline)
        if missing:
            print(
                f"{family}: removed public items: {', '.join(missing)}", file=sys.stderr
            )
            failed = True
        extra = added(current, baseline)
        if extra:
            print(f"{family}: added public items: {', '.join(extra)}", file=sys.stderr)
            failed = True
        mutation = read_list(mutation_path)
        if not compare(current, mutation):
            print(
                f"{family}: renamed-item mutation would silently pass",
                file=sys.stderr,
            )
            failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
