#!/usr/bin/env python3
"""Fail unless public package metadata declares MIT OR Apache-2.0."""

from __future__ import annotations

import json
import re
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
EXPECTED = "MIT OR Apache-2.0"


def cargo_workspace_license() -> str | None:
    text = (REPO_ROOT / "Cargo.toml").read_text(encoding="utf-8")
    match = re.search(r'(?m)^license\s*=\s*"([^"]+)"', text)
    return match.group(1) if match else None


def main() -> int:
    errors: list[str] = []
    workspace = cargo_workspace_license()
    if workspace != EXPECTED:
        errors.append(f"Cargo.toml workspace license={workspace!r}")
    if "license-files" not in (REPO_ROOT / "Cargo.toml").read_text(encoding="utf-8"):
        errors.append("Cargo.toml missing license-files")

    pyproject = (
        REPO_ROOT / "bindings" / "finstack-ai-python" / "pyproject.toml"
    ).read_text(encoding="utf-8")
    if f'license = "{EXPECTED}"' not in pyproject:
        errors.append("Python pyproject.toml license")
    if "license-files" not in pyproject:
        errors.append("Python pyproject.toml license-files")

    package = json.loads(
        (REPO_ROOT / "bindings" / "finstack-ai-wasm" / "js" / "package.json").read_text(
            encoding="utf-8"
        )
    )
    if package.get("license") != EXPECTED:
        errors.append(f"npm license={package.get('license')!r}")

    if errors:
        print("license metadata failures:")
        for item in errors:
            print(f"  {item}")
        return 1
    print(f"ok public package license is {EXPECTED}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
