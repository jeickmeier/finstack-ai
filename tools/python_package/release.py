#!/usr/bin/env python3
"""Stage deterministic Python distributions, checksums, SBOM, and manifest."""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import tomllib
from pathlib import Path

from python_package import check

REPO_ROOT = Path(__file__).resolve().parents[2]
PACKAGE_NAME = "finstack-ai"
PACKAGE_VERSION = "0.0.2"
SBOM_NAME = "finstack-ai-python.cdx.json"


def source_revision() -> str:
    """Return the current immutable source revision."""
    result = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return result.stdout.strip()


def require_clean_source() -> None:
    """Fail when staging inputs are not completely committed."""
    result = subprocess.run(
        ["git", "status", "--porcelain", "--untracked-files=all"],
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    if result.stdout:
        raise SystemExit("Python release staging requires a clean worktree")


def cargo_components() -> list[dict[str, object]]:
    """Return deterministic CycloneDX components from the committed Cargo lock."""
    lock = tomllib.loads((REPO_ROOT / "Cargo.lock").read_text(encoding="utf-8"))
    components = []
    for package in sorted(
        lock["package"], key=lambda item: (item["name"], item["version"])
    ):
        name = package["name"]
        version = package["version"]
        component: dict[str, object] = {
            "type": "library",
            "bom-ref": f"pkg:cargo/{name}@{version}",
            "name": name,
            "version": version,
            "purl": f"pkg:cargo/{name}@{version}",
        }
        if checksum := package.get("checksum"):
            component["hashes"] = [{"alg": "SHA-256", "content": checksum}]
        components.append(component)
    return components


def sbom(subjects: list[Path], revision: str) -> dict[str, object]:
    """Return one deterministic CycloneDX 1.6 SBOM for staged subjects."""
    components = cargo_components()
    subject_properties = [
        {
            "name": f"finstack-ai:subject:{subject.name}:sha256",
            "value": check.sha256_file(subject),
        }
        for subject in subjects
    ]
    return {
        "bomFormat": "CycloneDX",
        "specVersion": "1.6",
        "version": 1,
        "metadata": {
            "component": {
                "type": "library",
                "bom-ref": f"pkg:pypi/{PACKAGE_NAME}@{PACKAGE_VERSION}",
                "name": PACKAGE_NAME,
                "version": PACKAGE_VERSION,
                "purl": f"pkg:pypi/{PACKAGE_NAME}@{PACKAGE_VERSION}",
            },
            "properties": [
                {"name": "finstack-ai:source-revision", "value": revision},
                {"name": "finstack-ai:publication", "value": "staged_not_published"},
                *subject_properties,
            ],
        },
        "components": components,
        "dependencies": [
            {
                "ref": f"pkg:pypi/{PACKAGE_NAME}@{PACKAGE_VERSION}",
                "dependsOn": [component["bom-ref"] for component in components],
            }
        ],
    }


def stage(first: Path, second: Path, output: Path, revision: str) -> dict[str, object]:
    """Validate repeated builds and stage one checksummed release candidate."""
    check.validate_source_versions()
    comparison = check.compare_artifacts(first, second)
    if output.exists():
        shutil.rmtree(output)
    output.mkdir(parents=True)
    subjects = []
    for source in check.artifact_pair(first):
        target = output / source.name
        shutil.copy2(source, target)
        subjects.append(target)
    sbom_path = output / SBOM_NAME
    sbom_path.write_text(
        json.dumps(sbom(subjects, revision), indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    checksummed = [*subjects, sbom_path]
    sums = "".join(
        f"{check.sha256_file(path)}  {path.name}\n"
        for path in sorted(checksummed, key=lambda path: path.name)
    )
    (output / "SHA256SUMS").write_text(sums, encoding="utf-8")
    manifest = {
        "format_version": 1,
        "package": PACKAGE_NAME,
        "version": PACKAGE_VERSION,
        "checkpoint": "0.0.2-alpha-candidate",
        "checkpoint_status": "candidate_not_cut",
        "source_revision": revision,
        "publication": "staged_not_published",
        "sbom": SBOM_NAME,
        "subjects": [
            {
                "name": subject.name,
                "bytes": subject.stat().st_size,
                "sha256": check.sha256_file(subject),
            }
            for subject in subjects
        ],
        "reproducible": comparison["reproducible"],
    }
    (output / "release-manifest.json").write_text(
        json.dumps(manifest, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    return manifest


def main() -> None:
    """Parse arguments and stage the release candidate."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("first", type=Path)
    parser.add_argument("second", type=Path)
    parser.add_argument("output", type=Path)
    parser.add_argument("--source-revision")
    parser.add_argument("--allow-dirty", action="store_true")
    args = parser.parse_args()
    if not args.allow_dirty:
        require_clean_source()
    revision = args.source_revision or source_revision()
    if len(revision) != 40 or any(char not in "0123456789abcdef" for char in revision):
        raise SystemExit("source revision must be a lowercase 40-hex Git commit")
    result = stage(args.first, args.second, args.output, revision)
    print(json.dumps(result, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
