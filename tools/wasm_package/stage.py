#!/usr/bin/env python3
"""Stage signed-ready npm alpha artifacts without publishing."""

from __future__ import annotations

import hashlib
import json
import shutil
import subprocess
import tarfile
import tempfile
from datetime import datetime, timezone
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
JS_PACKAGE = REPO_ROOT / "bindings" / "finstack-ai-wasm" / "js"
EXAMPLE = REPO_ROOT / "examples" / "ts-alpha-install"
OUT_DIR = REPO_ROOT / "docs" / "implementation" / "artifacts" / "pr-038" / "npm-staging"


def run(command: list[str], cwd: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command,
        cwd=cwd,
        check=True,
        capture_output=True,
        text=True,
    )


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    digest.update(path.read_bytes())
    return digest.hexdigest()


def git_revision() -> str:
    return run(["git", "rev-parse", "HEAD"], REPO_ROOT).stdout.strip()


def pack_once(work: Path) -> Path:
    work.mkdir(parents=True, exist_ok=True)
    completed = run(["npm", "pack", "--pack-destination", str(work)], JS_PACKAGE)
    name = completed.stdout.strip().splitlines()[-1]
    packed = work / name
    if not packed.is_file():
        raise SystemExit(f"npm pack did not write {packed}")
    return packed


def write_sbom(package: dict[str, object], lock: dict[str, object], dest: Path) -> None:
    packages = [
        {
            "type": "library",
            "name": package["name"],
            "version": package["version"],
            "bom-ref": f"pkg:npm/{package['name']}@{package['version']}",
        }
    ]
    for name, meta in sorted((lock.get("packages") or {}).items()):
        if name in {"", "packages"} or not isinstance(meta, dict):
            continue
        version = meta.get("version")
        if not isinstance(version, str):
            continue
        purl_name = name.removeprefix("node_modules/")
        packages.append(
            {
                "type": "library",
                "name": purl_name,
                "version": version,
                "bom-ref": f"pkg:npm/{purl_name}@{version}",
            }
        )
    sbom = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.6",
        "version": 1,
        "metadata": {
            "component": {
                "type": "library",
                "name": package["name"],
                "version": package["version"],
            }
        },
        "components": packages,
    }
    dest.write_text(json.dumps(sbom, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def install_example(tarball: Path, work: Path) -> None:
    copied = work / "ts-alpha-install"
    shutil.copytree(EXAMPLE, copied, ignore=shutil.ignore_patterns("node_modules"))
    run(["npm", "install", "--omit=dev", str(tarball)], copied)
    run(["npm", "install", "--ignore-scripts", "typescript@5.9.2"], copied)
    run(["npx", "tsc", "--noEmit", "-p", "tsconfig.json"], copied)


def main() -> int:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    package = json.loads((JS_PACKAGE / "package.json").read_text(encoding="utf-8"))
    lock = json.loads((JS_PACKAGE / "package-lock.json").read_text(encoding="utf-8"))
    if package.get("private") is not True:
        print("error: package.json must stay private so accidental publish fails")
        return 1
    with tempfile.TemporaryDirectory(prefix="finstack-npm-stage-") as raw:
        work = Path(raw)
        first = pack_once(work / "a")
        second = pack_once(work / "b")
        if first.read_bytes() != second.read_bytes():
            print("error: consecutive npm pack outputs are not byte-identical")
            return 1
        staged = OUT_DIR / first.name
        shutil.copy2(first, staged)
        checksums = OUT_DIR / "SHA256SUMS"
        checksums.write_text(f"{sha256(staged)}  {staged.name}\n", encoding="utf-8")
        sbom = OUT_DIR / "sbom.cdx.json"
        write_sbom(package, lock, sbom)
        members = []
        with tarfile.open(staged, "r:gz") as archive:
            members = sorted(
                member.name for member in archive.getmembers() if member.isfile()
            )
        if "package/dist/index.d.ts" not in members:
            print("error: packed tarball is missing TypeScript declarations")
            return 1
        manifest = {
            "format_version": 1,
            "package": package["name"],
            "version": package["version"],
            "source_revision": git_revision(),
            "staged_not_published": True,
            "cross_binding_checkpoint_not_cut": True,
            "python_half": "PR-032 staged 0.0.2 candidate",
            "tarball": staged.name,
            "tarball_sha256": sha256(staged),
            "sbom_sha256": sha256(sbom),
            "generated_at": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
            "members": members,
        }
        (OUT_DIR / "staged-manifest.json").write_text(
            json.dumps(manifest, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
        install_example(staged, work)
    print(f"staged {staged.relative_to(REPO_ROOT)}")
    print("checkpoint available / not cut; package remains unpublished")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
