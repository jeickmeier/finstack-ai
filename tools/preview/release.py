#!/usr/bin/env python3
"""Reproducibly stage the current ``0.0.2-alpha-candidate`` bundle."""

from __future__ import annotations

import hashlib
import json
import os
import platform
import shutil
import subprocess
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
CHECKPOINT = "0.0.2-alpha-candidate"
PACKAGE_VERSION = "0.0.2"
STAGE = REPO_ROOT / "target" / "preview" / CHECKPOINT
PACKAGES = (
    "finstack-ai-kernel",
    "finstack-ai-runtime",
    "finstack-ai",
    "finstack-ai-provider-openai-compatible",
    "finstack-ai-store-memory",
    "finstack-ai-tools-calculator",
    "finstack-ai-tools-filesystem",
)
PACKAGE_ROOTS = {
    "finstack-ai-kernel": REPO_ROOT / "crates" / "finstack-ai-kernel",
    "finstack-ai-runtime": REPO_ROOT / "crates" / "finstack-ai-runtime",
    "finstack-ai": REPO_ROOT / "crates" / "finstack-ai",
    "finstack-ai-provider-openai-compatible": REPO_ROOT
    / "extensions"
    / "providers"
    / "finstack-ai-provider-openai-compatible",
    "finstack-ai-store-memory": REPO_ROOT
    / "extensions"
    / "stores"
    / "finstack-ai-store-memory",
    "finstack-ai-tools-calculator": REPO_ROOT
    / "extensions"
    / "toolsets"
    / "finstack-ai-tools-calculator",
    "finstack-ai-tools-filesystem": REPO_ROOT
    / "extensions"
    / "toolsets"
    / "finstack-ai-tools-filesystem",
}
LOCAL_PATCH_ROOTS = {
    **PACKAGE_ROOTS,
    "finstack-ai-test": REPO_ROOT / "crates" / "finstack-ai-test",
}


def run(command: list[str], *, env: dict[str, str] | None = None) -> None:
    """Run one repository command and fail on non-zero exit."""
    print("+", " ".join(command), flush=True)
    subprocess.run(command, cwd=REPO_ROOT, env=env, check=True)


def capture(command: list[str]) -> str:
    """Run one command and return its stdout."""
    print("+", " ".join(command), flush=True)
    result = subprocess.run(
        command,
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout


def sha256(path: Path) -> str:
    """Return one file's SHA-256 digest."""
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def git_commit() -> str:
    """Return the exact source commit used for staging."""
    result = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def ensure_clean_worktree() -> None:
    """Require every staged source byte to belong to the recorded commit."""
    status = capture(["git", "status", "--porcelain", "--untracked-files=all"])
    if status:
        raise SystemExit("preview staging requires a clean worktree")


def patch_arguments(package: str) -> list[str]:
    """Return local registry patches for unpublished workspace dependencies."""
    arguments: list[str] = []
    for dependency, root in sorted(LOCAL_PATCH_ROOTS.items()):
        if dependency == package:
            continue
        arguments.extend(["--config", f'patch.crates-io.{dependency}.path="{root}"'])
    return arguments


def package_set(target: Path) -> dict[str, Path]:
    """Build and verify one complete set of Cargo publication archives."""
    if target.exists():
        shutil.rmtree(target)
    target.mkdir(parents=True)
    dry_runs: dict[str, list[str]] = {}
    archives: dict[str, Path] = {}
    for package in PACKAGES:
        output = capture(
            [
                "cargo",
                "package",
                "-p",
                package,
                "--locked",
                "--list",
            ]
        )
        files = [line for line in output.splitlines() if line]
        dry_runs[package] = files
        run(
            [
                "cargo",
                "package",
                "-p",
                package,
                "--locked",
                *patch_arguments(package),
            ]
        )
        archive = (
            REPO_ROOT / "target" / "package" / f"{package}-{PACKAGE_VERSION}.crate"
        )
        if not archive.is_file():
            raise SystemExit(f"Cargo package archive is missing: {archive}")
        archive = Path(shutil.copy2(archive, target / archive.name))
        archives[archive.name] = archive
    (target / "cargo-publication-dry-runs.json").write_text(
        json.dumps(dry_runs, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    if len(archives) != len(PACKAGES):
        raise SystemExit(
            f"expected {len(PACKAGES)} source archives, found {len(archives)}"
        )
    return archives


def release_binary() -> Path:
    """Build and return the minimal native preview binary."""
    env = os.environ.copy()
    env.update({"CARGO_INCREMENTAL": "0", "SOURCE_DATE_EPOCH": "0"})
    run(
        [
            "cargo",
            "build",
            "--release",
            "--locked",
            "-p",
            "finstack-ai-native-examples",
            "--bin",
            "minimal",
        ],
        env=env,
    )
    name = "minimal.exe" if os.name == "nt" else "minimal"
    path = REPO_ROOT / "target" / "release" / name
    if not path.is_file():
        raise SystemExit(f"preview binary is missing: {path}")
    return path


def copy_evidence() -> None:
    """Copy documentation and measurement artifacts into the stage."""
    shutil.copytree(REPO_ROOT / "docs" / "site", STAGE / "docs")
    shutil.copy2(REPO_ROOT / "CHANGELOG.md", STAGE / "CHANGELOG.md")
    notes = (
        (REPO_ROOT / "CHANGELOG.md")
        .read_text(encoding="utf-8")
        .split("## [Unreleased]", 1)[1]
    )
    (STAGE / "release-notes.md").write_text(
        f"# finstack-ai {CHECKPOINT}\n\n{notes.strip()}\n", encoding="utf-8"
    )
    report = REPO_ROOT / "target" / "preview" / "performance-report.json"
    if not report.is_file():
        raise SystemExit(
            "preview performance report is missing; run preview-performance"
        )
    shutil.copy2(report, STAGE / "performance-report.json")
    shutil.copytree(
        REPO_ROOT / "target" / "benchmark" / "smoke",
        STAGE / "benchmark-smoke",
    )


def stage() -> None:
    """Create two package sets, compare them, and stage the preview bundle."""
    ensure_clean_worktree()
    first_root = REPO_ROOT / "target" / "preview-package-a"
    first = package_set(first_root)
    second = package_set(REPO_ROOT / "target" / "preview-package-b")
    comparisons = []
    for name, first_path in first.items():
        second_path = second.get(name)
        if second_path is None:
            raise SystemExit(f"second package set is missing {name}")
        first_digest = sha256(first_path)
        second_digest = sha256(second_path)
        matches = first_digest == second_digest
        comparisons.append(
            {
                "archive": name,
                "first_sha256": first_digest,
                "second_sha256": second_digest,
                "matches": matches,
            }
        )
        if not matches:
            raise SystemExit(f"package archive is not reproducible: {name}")
    if STAGE.exists():
        shutil.rmtree(STAGE)
    (STAGE / "packages").mkdir(parents=True)
    for name, path in first.items():
        shutil.copy2(path, STAGE / "packages" / name)
    shutil.copy2(
        first_root / "cargo-publication-dry-runs.json",
        STAGE / "cargo-publication-dry-runs.json",
    )
    binary = release_binary()
    (STAGE / "bin").mkdir()
    shutil.copy2(binary, STAGE / "bin" / binary.name)
    copy_evidence()
    (STAGE / "reproducibility.json").write_text(
        json.dumps(
            {"format_version": 1, "package_archives": comparisons},
            indent=2,
            sort_keys=True,
        )
        + "\n",
        encoding="utf-8",
    )
    files = sorted(path for path in STAGE.rglob("*") if path.is_file())
    manifest = {
        "checkpoint": CHECKPOINT,
        "checkpoint_status": "candidate_not_cut",
        "commit": git_commit(),
        "host": platform.platform(),
        "publication": "staged_not_published",
        "files": [
            {"path": str(path.relative_to(STAGE)), "sha256": sha256(path)}
            for path in files
        ],
    }
    (STAGE / "manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(f"native preview: staged {STAGE}")


if __name__ == "__main__":
    stage()
