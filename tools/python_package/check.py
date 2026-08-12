#!/usr/bin/env python3
"""Validate and compare staged Python wheel and source artifacts."""

from __future__ import annotations

import argparse
import hashlib
import json
import tarfile
import tomllib
import zipfile
from email.parser import Parser
from pathlib import Path, PurePosixPath

PACKAGE_VERSION = "0.0.1"
MAX_WHEEL_BYTES = 10 * 1024 * 1024
REPO_ROOT = Path(__file__).resolve().parents[2]


def sha256_file(path: Path) -> str:
    """Return a file's SHA-256 digest."""
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def artifact_pair(directory: Path) -> tuple[Path, Path]:
    """Return the sole wheel and source distribution in a staging directory."""
    wheels = sorted(directory.glob("*.whl"))
    sdists = sorted(directory.glob("*.tar.gz"))
    if len(wheels) != 1 or len(sdists) != 1:
        raise SystemExit(
            f"{directory} must contain exactly one wheel and one .tar.gz sdist; "
            f"found {len(wheels)} wheel(s) and {len(sdists)} sdist(s)"
        )
    return wheels[0], sdists[0]


def safe_archive_name(name: str) -> bool:
    """Return whether an archive member is a normalized relative POSIX path."""
    path = PurePosixPath(name)
    return not path.is_absolute() and ".." not in path.parts and "\\" not in name


def require_suffix(names: set[str], suffix: str) -> None:
    """Require one archive member ending in the supplied suffix."""
    if not any(name.endswith(suffix) for name in names):
        raise SystemExit(f"artifact is missing required member: *{suffix}")


def validate_source_versions() -> None:
    """Require Cargo and Python package release versions to remain aligned."""
    workspace = tomllib.loads((REPO_ROOT / "Cargo.toml").read_text(encoding="utf-8"))
    pyproject = tomllib.loads(
        (REPO_ROOT / "bindings/finstack-ai-python/pyproject.toml").read_text(
            encoding="utf-8"
        )
    )
    cargo_version = workspace["workspace"]["package"]["version"]
    python_version = pyproject["project"]["version"]
    if cargo_version != PACKAGE_VERSION or python_version != PACKAGE_VERSION:
        raise SystemExit(
            "workspace, Python project, and artifact-checker versions must match: "
            f"cargo={cargo_version!r}, python={python_version!r}, "
            f"checker={PACKAGE_VERSION!r}"
        )


def validate_wheel(path: Path) -> dict[str, object]:
    """Validate wheel contents, metadata, typing, licensing, and size."""
    if "abi3" in path.name:
        raise SystemExit(f"wheel must use a per-interpreter ABI, not abi3: {path.name}")
    if path.stat().st_size > MAX_WHEEL_BYTES:
        raise SystemExit(
            f"wheel exceeds {MAX_WHEEL_BYTES} byte budget: {path.stat().st_size} bytes"
        )

    with zipfile.ZipFile(path) as archive:
        names = set(archive.namelist())
        unsafe = sorted(name for name in names if not safe_archive_name(name))
        if unsafe:
            raise SystemExit(f"wheel contains unsafe paths: {unsafe}")
        unwanted = sorted(
            name for name in names if "__pycache__" in name or name.endswith(".pyc")
        )
        if unwanted:
            raise SystemExit(f"wheel contains generated Python bytecode: {unwanted}")

        for suffix in (
            "finstack_ai/__init__.py",
            "finstack_ai/_finstack_ai.pyi",
            "finstack_ai/py.typed",
            "finstack_ai/providers/__init__.py",
            "finstack_ai/providers/openai_compatible.py",
            ".dist-info/licenses/LICENSE-APACHE",
            ".dist-info/licenses/LICENSE-MIT",
        ):
            require_suffix(names, suffix)
        if not any(
            name.startswith("finstack_ai/_finstack_ai.")
            and name.endswith((".so", ".pyd"))
            for name in names
        ):
            raise SystemExit("wheel is missing the per-interpreter native extension")

        metadata_names = sorted(
            name for name in names if name.endswith(".dist-info/METADATA")
        )
        if len(metadata_names) != 1:
            raise SystemExit("wheel must contain exactly one METADATA file")
        metadata = Parser().parsestr(archive.read(metadata_names[0]).decode("utf-8"))
        expected_metadata = {
            "Name": "finstack-ai",
            "Version": PACKAGE_VERSION,
            "Requires-Python": ">=3.11",
            "License-Expression": "MIT OR Apache-2.0",
        }
        for key, expected in expected_metadata.items():
            if metadata[key] != expected:
                raise SystemExit(
                    f"wheel metadata {key} must be {expected!r}; got {metadata[key]!r}"
                )

    return {
        "name": path.name,
        "bytes": path.stat().st_size,
        "sha256": sha256_file(path),
    }


def validate_sdist(path: Path) -> dict[str, object]:
    """Validate source distribution paths and release-critical contents."""
    with tarfile.open(path, "r:gz") as archive:
        members = archive.getmembers()
        names = {member.name for member in members}
        unsafe = sorted(
            member.name for member in members if not safe_archive_name(member.name)
        )
        if unsafe:
            raise SystemExit(f"sdist contains unsafe paths: {unsafe}")
        links = sorted(
            member.name for member in members if member.issym() or member.islnk()
        )
        if links:
            raise SystemExit(f"sdist contains links: {links}")
        unwanted = sorted(
            name
            for name in names
            if "/.git/" in f"/{name}/"
            or "/target/" in f"/{name}/"
            or "__pycache__" in name
            or name.endswith(".pyc")
        )
        if unwanted:
            raise SystemExit(f"sdist contains generated or private paths: {unwanted}")

        for suffix in (
            "/Cargo.lock",
            "/Cargo.toml",
            "/LICENSE-APACHE",
            "/LICENSE-MIT",
            "/pyproject.toml",
            "/bindings/finstack-ai-python/src/lib.rs",
            "/python/finstack_ai/__init__.py",
            "/python/finstack_ai/_finstack_ai.pyi",
            "/python/finstack_ai/py.typed",
            "/crates/finstack-ai-kernel/src/lib.rs",
            "/extensions/providers/finstack-ai-provider-openai-compatible/src/lib.rs",
        ):
            require_suffix(names, suffix)

    return {
        "name": path.name,
        "bytes": path.stat().st_size,
        "sha256": sha256_file(path),
    }


def compare_artifacts(first: Path, second: Path) -> dict[str, object]:
    """Validate two builds and require byte-identical staged artifacts."""
    first_wheel, first_sdist = artifact_pair(first)
    second_wheel, second_sdist = artifact_pair(second)
    if first_wheel.name != second_wheel.name or first_sdist.name != second_sdist.name:
        raise SystemExit("repeated builds produced different artifact names")

    first_results = {
        "wheel": validate_wheel(first_wheel),
        "sdist": validate_sdist(first_sdist),
    }
    second_results = {
        "wheel": validate_wheel(second_wheel),
        "sdist": validate_sdist(second_sdist),
    }
    for kind in ("wheel", "sdist"):
        if first_results[kind]["sha256"] != second_results[kind]["sha256"]:
            raise SystemExit(
                f"repeated {kind} builds are not byte-for-byte reproducible"
            )
    return {
        "artifact_budget_bytes": MAX_WHEEL_BYTES,
        "first": first_results,
        "second": second_results,
        "reproducible": True,
    }


def validate_artifacts(directory: Path) -> dict[str, object]:
    """Validate one staged wheel and source distribution without comparing builds."""
    wheel, sdist = artifact_pair(directory)
    return {
        "artifact_budget_bytes": MAX_WHEEL_BYTES,
        "artifacts": {
            "wheel": validate_wheel(wheel),
            "sdist": validate_sdist(sdist),
        },
        "reproducibility": "not evaluated",
    }


def main() -> None:
    """Parse arguments, validate staged packages, and print a JSON summary."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--validate-only",
        action="store_true",
        help="validate one artifact directory without comparing build digests",
    )
    parser.add_argument("first", type=Path, help="first artifact directory")
    parser.add_argument(
        "second", type=Path, nargs="?", help="second artifact directory"
    )
    args = parser.parse_args()
    validate_source_versions()
    if args.validate_only:
        if args.second is not None:
            parser.error("--validate-only accepts exactly one artifact directory")
        result = validate_artifacts(args.first)
    else:
        if args.second is None:
            parser.error("a second artifact directory is required for comparison")
        result = compare_artifacts(args.first, args.second)
    print(json.dumps(result, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
