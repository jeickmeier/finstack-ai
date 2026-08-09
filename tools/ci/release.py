#!/usr/bin/env python3
"""Build and package the private CI release-smoke binary."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
PACKAGE = "finstack-ai-ci-smoke"
BINARY_NAME = "finstack-ai-ci-smoke"
ARTIFACT_DIR = REPO_ROOT / "target" / "ci-release"


def resolve_tool(name: str) -> str:
    """Resolve a CLI tool for subprocess use when PATH is incomplete under uv.

    On Windows CI, ``uv run`` can drop ``CARGO_HOME/bin`` from PATH even though
    the outer mise/GitHub Actions environment still exports ``CARGO_HOME``.
    """
    exe = f"{name}.exe" if os.name == "nt" else name
    for candidate_name in (name, exe):
        found = shutil.which(candidate_name)
        if found:
            return found

    if name in {"cargo", "rustc", "rustup"}:
        roots: list[Path] = []
        cargo_home = os.environ.get("CARGO_HOME")
        if cargo_home:
            roots.append(Path(cargo_home))
        roots.append(Path.home() / ".cargo")
        for root in roots:
            candidate = root / "bin" / exe
            if candidate.is_file():
                return str(candidate)

    mise = shutil.which("mise") or (
        shutil.which("mise.exe") if os.name == "nt" else None
    )
    if mise:
        proc = subprocess.run(
            [mise, "which", name],
            check=False,
            capture_output=True,
            text=True,
        )
        path = proc.stdout.strip()
        if proc.returncode == 0 and path:
            return path

    raise FileNotFoundError(
        f"{name} executable not found on PATH; ensure mise tools are installed"
    )


def run(command: list[str], *, cwd: Path, env: dict[str, str] | None = None) -> None:
    """Run a command and fail loudly on non-zero exit."""
    print("+", " ".join(command), flush=True)
    subprocess.run(command, cwd=cwd, env=env, check=True)


def require_ci_commit() -> bool:
    """Return True when CI must fail closed without a git commit identity."""
    return os.environ.get("CI") == "true" or os.environ.get("GITHUB_ACTIONS") == "true"


def git_commit(*, require: bool | None = None) -> str:
    """Return the current HEAD commit.

    In CI (`CI=true` or `GITHUB_ACTIONS=true`), absence of a commit identity is
    fatal. Locally, return `unknown` when git is unavailable.
    """
    if require is None:
        require = require_ci_commit()
    try:
        completed = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=REPO_ROOT,
            check=True,
            capture_output=True,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError) as exc:
        if require:
            raise SystemExit(
                "release-smoke requires a git commit identity in CI"
            ) from exc
        return "unknown"
    commit = completed.stdout.strip()
    if not commit:
        if require:
            raise SystemExit("release-smoke received empty git commit identity in CI")
        return "unknown"
    return commit


def rustc_version() -> str:
    """Return the active rustc version string."""
    completed = subprocess.run(
        [resolve_tool("rustc"), "--version"],
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return completed.stdout.strip()


def host_triple() -> str:
    """Return the host target triple from rustc."""
    completed = subprocess.run(
        [resolve_tool("rustc"), "-vV"],
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    for line in completed.stdout.splitlines():
        if line.startswith("host:"):
            return line.split(":", 1)[1].strip()
    raise RuntimeError("unable to determine rustc host triple")


def cargo_package_metadata() -> dict[str, object]:
    """Return Cargo metadata for the smoke package and its enabled features."""
    completed = subprocess.run(
        [
            resolve_tool("cargo"),
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--no-deps",
        ],
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    metadata = json.loads(completed.stdout)
    for package in metadata.get("packages", []):
        if package.get("name") != PACKAGE:
            continue
        enabled: set[str] = set()
        for dep in package.get("dependencies", []):
            if dep.get("name") != "finstack-ai":
                continue
            if dep.get("uses_default_features", True):
                enabled.add("default")
            enabled.update(dep.get("features") or [])
        return {
            "version": package["version"],
            "features": sorted(enabled),
            "facade_dependency": "finstack-ai",
        }
    raise SystemExit(f"package {PACKAGE} not found in cargo metadata")


def sha256_file(path: Path) -> str:
    """Return the SHA-256 digest of a file."""
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def binary_path(target_dir: Path) -> Path:
    """Return the built binary path for the current OS."""
    name = f"{BINARY_NAME}.exe" if os.name == "nt" else BINARY_NAME
    return target_dir / "release" / name


def release_env(extra: dict[str, str] | None = None) -> dict[str, str]:
    """Environment variables that keep release builds deterministic."""
    env = os.environ.copy()
    env.update(
        {
            "CARGO_INCREMENTAL": "0",
            "SOURCE_DATE_EPOCH": "0",
            "RUSTFLAGS": "-C debuginfo=0",
        }
    )
    if extra:
        env.update(extra)
    return env


def write_metadata(artifact_dir: Path, binary: Path, digest: str) -> Path:
    """Write machine-readable build metadata beside the binary."""
    package = cargo_package_metadata()
    metadata = {
        "package": PACKAGE,
        "binary": binary.name,
        "version": package["version"],
        "commit": git_commit(),
        "rustc": rustc_version(),
        "host_triple": host_triple(),
        "platform": {
            "system": platform.system(),
            "machine": platform.machine(),
            "release": platform.release(),
        },
        "features": package["features"],
        "facade_dependency": package["facade_dependency"],
        "profile": "release",
        "sha256": digest,
    }
    path = artifact_dir / "build-metadata.json"
    path.write_text(
        json.dumps(metadata, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    return path


def build_release(*, target_dir: Path) -> Path:
    """Build the smoke binary into an isolated target directory."""
    target_dir.mkdir(parents=True, exist_ok=True)
    env = release_env({"CARGO_TARGET_DIR": str(target_dir)})
    run(
        [
            resolve_tool("cargo"),
            "build",
            "--release",
            "-p",
            PACKAGE,
            "--locked",
        ],
        cwd=REPO_ROOT,
        env=env,
    )
    binary = binary_path(target_dir)
    if not binary.is_file():
        raise FileNotFoundError(f"expected release binary at {binary}")
    return binary


def package_smoke() -> Path:
    """Build, execute, checksum, and stage the release-smoke artifacts."""
    # Fail early in CI if commit identity is missing.
    git_commit()
    build_dir = REPO_ROOT / "target" / "ci-release-build"
    binary = build_release(target_dir=build_dir)
    run([str(binary)], cwd=REPO_ROOT)

    ARTIFACT_DIR.mkdir(parents=True, exist_ok=True)
    staged = ARTIFACT_DIR / binary.name
    shutil.copy2(binary, staged)
    digest = sha256_file(staged)
    checksum_path = ARTIFACT_DIR / "SHA256SUMS"
    checksum_path.write_text(f"{digest}  {staged.name}\n", encoding="utf-8")
    metadata_path = write_metadata(ARTIFACT_DIR, staged, digest)
    print(f"staged binary: {staged}")
    print(f"sha256: {digest}")
    print(f"metadata: {metadata_path}")
    return staged


def reproducible() -> None:
    """Build twice in isolated target dirs and compare digests."""
    if platform.system() != "Linux":
        print(
            "release-reproducible is Linux-only; skipping digest comparison on "
            f"{platform.system()}",
            flush=True,
        )
        return

    first_dir = REPO_ROOT / "target" / "ci-repro-a"
    second_dir = REPO_ROOT / "target" / "ci-repro-b"
    for path in (first_dir, second_dir):
        if path.exists():
            shutil.rmtree(path)

    first = build_release(target_dir=first_dir)
    second = build_release(target_dir=second_dir)
    first_digest = sha256_file(first)
    second_digest = sha256_file(second)
    print(f"build-a sha256: {first_digest}")
    print(f"build-b sha256: {second_digest}")
    if first_digest != second_digest:
        raise SystemExit(
            f"release builds are not reproducible: {first_digest} != {second_digest}"
        )
    print("release builds match")


def main(argv: list[str] | None = None) -> int:
    """CLI entrypoint."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "command",
        choices=("smoke", "reproducible"),
        help="smoke packages artifacts; reproducible compares two clean builds",
    )
    args = parser.parse_args(argv)
    if args.command == "smoke":
        package_smoke()
    else:
        reproducible()
    return 0


if __name__ == "__main__":
    sys.exit(main())
