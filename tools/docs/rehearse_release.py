#!/usr/bin/env python3
"""Two-run local staging checksum identity for unpublished 0.1.0."""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
WORK = REPO_ROOT / "target" / "pr-061-rehearsal"
RECORD = REPO_ROOT / "docs" / "implementation" / "artifacts" / "pr-061" / "rehearsal"
CRATES = (
    "finstack-ai",
    "finstack-ai-kernel",
    "finstack-ai-runtime",
    "finstack-ai-protocol",
)
JS_PACKAGE = REPO_ROOT / "bindings" / "finstack-ai-wasm" / "js"
PYTHON = REPO_ROOT / "bindings" / "finstack-ai-python"


def run(
    command: list[str], cwd: Path | None = None
) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(
        command,
        cwd=cwd or REPO_ROOT,
        capture_output=True,
        text=True,
        env={**os.environ, "SOURCE_DATE_EPOCH": "0"},
    )
    if completed.returncode != 0:
        sys_stderr = completed.stderr or completed.stdout
        raise SystemExit(
            f"command failed ({completed.returncode}): {' '.join(command)}\n{sys_stderr}"
        )
    return completed


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    digest.update(path.read_bytes())
    return digest.hexdigest()


def write_checksums(directory: Path) -> None:
    lines = []
    for path in sorted(p for p in directory.rglob("*") if p.is_file()):
        relative = path.relative_to(directory).as_posix()
        if relative == "SHA256SUMS":
            continue
        lines.append(f"{sha256(path)}  {relative}\n")
    (directory / "SHA256SUMS").write_text("".join(lines), encoding="utf-8")


def comparable_checksums(directory: Path) -> str:
    lines = []
    for path in sorted(p for p in directory.rglob("*") if p.is_file()):
        relative = path.relative_to(directory).as_posix()
        if relative == "SHA256SUMS" or relative.endswith(".whl"):
            continue
        lines.append(f"{sha256(path)}  {relative}\n")
    return "".join(lines)


def stage(work: Path) -> None:
    work.mkdir(parents=True, exist_ok=True)
    lists = work / "crate-package-lists"
    lists.mkdir()
    for crate in CRATES:
        completed = run(
            [
                "cargo",
                "package",
                "--list",
                "--locked",
                "--offline",
                "-p",
                crate,
                "--allow-dirty",
            ]
        )
        (lists / f"{crate}.list").write_text(completed.stdout, encoding="utf-8")

    python_out = work / "python"
    python_out.mkdir()
    run(
        [
            "uv",
            "build",
            "--sdist",
            "--project",
            str(PYTHON),
            "--out-dir",
            str(python_out),
        ]
    )
    run(
        [
            "uv",
            "run",
            "--no-project",
            "--with",
            "maturin==1.14.1",
            "maturin",
            "build",
            "--offline",
            "--locked",
            "--manifest-path",
            str(PYTHON / "Cargo.toml"),
            "--out",
            str(python_out),
        ]
    )

    if (JS_PACKAGE / "dist" / "index.js").is_file():
        completed = run(["npm", "pack", "--pack-destination", str(work)], JS_PACKAGE)
        name = completed.stdout.strip().splitlines()[-1]
        packed = work / name
        if not packed.is_file():
            raise SystemExit(f"npm pack did not write {packed}")

    plugin_lock = REPO_ROOT / "plugins" / "reference" / "plugin.lock.json"
    if plugin_lock.is_file():
        shutil.copy2(plugin_lock, work / "plugin.lock.json")

    revision = run(["git", "rev-parse", "HEAD"]).stdout.strip()
    rustc = run(["rustc", "--version"]).stdout.strip()
    statement = {
        "format_version": 1,
        "kind": "preview rehearsal",
        "version": "0.1.0",
        "staged_not_published": True,
        "public_tag": "blocked pending named G7-D and git tag v0.1.0",
        "source_revision": revision,
        "rustc": rustc,
        "mise_pins": {
            "rust": "1.97.1",
            "python": "3.14",
            "uv": "0.10.11",
            "node": "22.18.0",
        },
        "notes": (
            "uv build wheel-from-sdist fails with maturin locked=true outside "
            "the workspace; rehearsal uses uv build --sdist plus in-tree "
            "maturin build --offline --locked. Wheel bytes are recorded but "
            "excluded from two-run identity because rustc/maturin embed "
            "non-reproducible metadata. Sdist, crate lists, plugin.lock, and "
            "npm pack (when dist/ exists) must match."
        ),
    }
    (work / "provenance.json").write_text(
        json.dumps(statement, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    write_checksums(work)


def main() -> int:
    if WORK.exists():
        shutil.rmtree(WORK)
    first = WORK / "run-a"
    second = WORK / "run-b"
    stage(first)
    stage(second)
    left = comparable_checksums(first)
    right = comparable_checksums(second)
    RECORD.mkdir(parents=True, exist_ok=True)
    (RECORD / "run-a.comparable").write_text(left, encoding="utf-8")
    (RECORD / "run-b.comparable").write_text(right, encoding="utf-8")
    shutil.copy2(first / "SHA256SUMS", RECORD / "run-a.SHA256SUMS")
    shutil.copy2(second / "SHA256SUMS", RECORD / "run-b.SHA256SUMS")
    shutil.copy2(first / "provenance.json", RECORD / "provenance.json")
    if left != right:
        print("error: two rehearsal runs produced different comparable checksums")
        return 1
    print(f"ok two-run rehearsal identity under {WORK.relative_to(REPO_ROOT)}")
    print(f"recorded checksums under {RECORD.relative_to(REPO_ROOT)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
