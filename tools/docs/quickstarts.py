#!/usr/bin/env python3
"""Execute offline public-package quick starts."""

from __future__ import annotations

import subprocess
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
PUBLIC_README_MARKERS = (
    REPO_ROOT / "crates" / "finstack-ai" / "README.md",
    REPO_ROOT / "crates" / "finstack-ai-kernel" / "README.md",
    REPO_ROOT / "crates" / "finstack-ai-runtime" / "README.md",
    REPO_ROOT / "crates" / "finstack-ai-protocol" / "README.md",
    REPO_ROOT / "bindings" / "finstack-ai-python" / "README.md",
    REPO_ROOT / "bindings" / "finstack-ai-wasm" / "js" / "README.md",
    REPO_ROOT / "crates" / "finstack-ai-server" / "README.md",
    REPO_ROOT / "examples" / "rust-minimal" / "README.md",
    REPO_ROOT / "examples" / "python-minimal" / "python-callback" / "README.md",
    REPO_ROOT / "examples" / "durable-interaction" / "README.md",
)


def run(command: list[str]) -> None:
    print("+", " ".join(command), flush=True)
    subprocess.run(command, cwd=REPO_ROOT, check=True)


def require_quick_start(path: Path) -> None:
    text = path.read_text(encoding="utf-8")
    if "## Quick start" not in text:
        raise SystemExit(f"{path.relative_to(REPO_ROOT)} missing ## Quick start")


def main() -> int:
    for path in PUBLIC_README_MARKERS:
        require_quick_start(path)

    html = (REPO_ROOT / "examples" / "browser-minimal" / "index.html").read_text(
        encoding="utf-8"
    )
    for label in ("Run scripted session", "Inspect last session", "Clear local data"):
        if label not in html:
            raise SystemExit(f"browser-minimal missing labeled control: {label}")

    uv = [
        "uv",
        "run",
        "--isolated",
        "--no-project",
        "--with-editable",
        "bindings/finstack-ai-python",
    ]
    run(
        [
            "cargo",
            "run",
            "-p",
            "finstack-ai-native-examples",
            "--bin",
            "minimal",
            "--offline",
            "--locked",
        ]
    )
    run(
        [
            "cargo",
            "run",
            "-p",
            "finstack-ai-native-examples",
            "--bin",
            "service",
            "--offline",
            "--locked",
        ]
    )
    run(
        [
            "cargo",
            "run",
            "-p",
            "finstack-ai-example-durable-interaction",
            "--offline",
            "--locked",
        ]
    )
    run([*uv, "python", "examples/python-minimal/python-callback/main.py"])
    run([*uv, "python", "examples/python-minimal/rust-backed/main.py"])
    run([*uv, "python", "examples/python-minimal/service/main.py"])
    print("ok public-package quick starts")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
