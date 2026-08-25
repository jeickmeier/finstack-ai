#!/usr/bin/env python3
"""Shared unpublished lockstep staging for recreate and hotfix rehearsal."""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
WORKSPACE_VERSION = tomllib.loads(
    (REPO_ROOT / "Cargo.toml").read_text(encoding="utf-8")
)["workspace"]["package"]["version"]
CRATES = (
    "finstack-ai",
    "finstack-ai-kernel",
    "finstack-ai-runtime",
    "finstack-ai-protocol",
)
JS_PACKAGE = REPO_ROOT / "bindings" / "finstack-ai-wasm" / "js"
PYTHON = REPO_ROOT / "bindings" / "finstack-ai-python"
WIT_ROOTS = (
    REPO_ROOT / "plugins" / "finstack-ai-wit" / "wit" / "v0.0.4",
    REPO_ROOT / "plugins" / "finstack-ai-wit" / "wit" / "v1.0.0",
)
PLUGIN_LOCK = REPO_ROOT / "plugins" / "reference" / "plugin.lock.json"
MISE_PINS = {
    "rust": "1.97.1",
    "python": "3.14",
    "uv": "0.10.11",
    "node": "22.18.0",
}


def run(
    command: list[str], cwd: Path | None = None
) -> subprocess.CompletedProcess[str]:
    completed = subprocess.run(
        command,
        cwd=cwd or REPO_ROOT,
        capture_output=True,
        text=True,
        env={
            **os.environ,
            "SOURCE_DATE_EPOCH": "0",
            "CARGO_INCREMENTAL": "0",
        },
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


def checksum_text(directory: Path) -> str:
    lines = []
    for path in sorted(p for p in directory.rglob("*") if p.is_file()):
        relative = path.relative_to(directory).as_posix()
        if relative == "SHA256SUMS":
            continue
        lines.append(f"{sha256(path)}  {relative}\n")
    return "".join(lines)


def crate_sbom() -> dict[str, object]:
    metadata = json.loads(
        run(["cargo", "metadata", "--locked", "--format-version", "1"]).stdout
    )
    components = []
    for package in sorted(
        metadata["packages"], key=lambda item: (item["name"], item["version"])
    ):
        components.append(
            {
                "type": "library",
                "name": package["name"],
                "version": package["version"],
                "bom-ref": f"pkg:cargo/{package['name']}@{package['version']}",
            }
        )
    return {
        "bomFormat": "CycloneDX",
        "specVersion": "1.6",
        "version": 1,
        "metadata": {
            "component": {
                "type": "application",
                "name": "finstack-ai",
                "version": WORKSPACE_VERSION,
            }
        },
        "components": components,
    }


def stage(
    work: Path, *, label: str = WORKSPACE_VERSION, extra: dict[str, str] | None = None
) -> None:
    if work.exists():
        shutil.rmtree(work)
    work.mkdir(parents=True)
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

    wit_out = work / "wit"
    wit_out.mkdir()
    for root in WIT_ROOTS:
        if not root.is_dir():
            continue
        dest = wit_out / root.name
        shutil.copytree(root, dest)
    if PLUGIN_LOCK.is_file():
        shutil.copy2(PLUGIN_LOCK, work / "plugin.lock.json")

    (work / "sbom.crates.cdx.json").write_text(
        json.dumps(crate_sbom(), indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    if (JS_PACKAGE / "package.json").is_file() and (
        JS_PACKAGE / "package-lock.json"
    ).is_file():
        package = json.loads((JS_PACKAGE / "package.json").read_text(encoding="utf-8"))
        lock = json.loads(
            (JS_PACKAGE / "package-lock.json").read_text(encoding="utf-8")
        )
        npm_packages = [
            {
                "type": "library",
                "name": package["name"],
                "version": package["version"],
                "bom-ref": f"pkg:npm/{package['name']}@{package['version']}",
            }
        ]
        for name, meta in sorted((lock.get("packages") or {}).items()):
            if not isinstance(meta, dict):
                continue
            version = meta.get("version")
            if not isinstance(version, str):
                continue
            purl_name = name.removeprefix("node_modules/") or str(package["name"])
            npm_packages.append(
                {
                    "type": "library",
                    "name": purl_name,
                    "version": version,
                    "bom-ref": f"pkg:npm/{purl_name}@{version}",
                }
            )
        (work / "sbom.npm.cdx.json").write_text(
            json.dumps(
                {
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
                    "components": npm_packages,
                },
                indent=2,
                sort_keys=True,
            )
            + "\n",
            encoding="utf-8",
        )

    if extra:
        for name, body in extra.items():
            (work / name).write_text(body, encoding="utf-8")

    revision = run(["git", "rev-parse", "HEAD"]).stdout.strip()
    rustc = run(["rustc", "--version"]).stdout.strip()
    statement = {
        "format_version": 1,
        "kind": "release recreate",
        "version": WORKSPACE_VERSION,
        "label": label,
        "staged_not_published": True,
        "source_revision": revision,
        "rustc": rustc,
        "mise_pins": MISE_PINS,
        "source_date_epoch": "0",
        "ga_tag_procedure": "not executed by staging tooling",
    }
    (work / "provenance.json").write_text(
        json.dumps(statement, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
    )
    write_checksums(work)
