#!/usr/bin/env python3
"""Copy the toolset template, build, encode, and run it through PluginHost."""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
TEMPLATE = REPO_ROOT / "plugins" / "templates" / "toolset-plugin"
CONTEXT_TEMPLATE = REPO_ROOT / "plugins" / "templates" / "context-plugin"
GUEST_SDK = REPO_ROOT / "plugins" / "finstack-ai-guest-sdk"
ENCODER = REPO_ROOT / "scripts" / "plugin_wasm" / "encoder"


def run(command: list[str], cwd: Path, env: dict[str, str] | None = None) -> None:
    completed = subprocess.run(command, cwd=cwd, check=False, env=env)
    if completed.returncode != 0:
        raise SystemExit(completed.returncode)


def build_template(src: Path, dest: Path, crate: str) -> Path:
    shutil.copytree(
        src, dest, ignore=shutil.ignore_patterns("target", "component.wasm")
    )
    cargo = dest / "Cargo.toml"
    text = cargo.read_text(encoding="utf-8")
    cargo.write_text(
        text.replace(
            'path = "../../finstack-ai-guest-sdk"',
            f'path = "{GUEST_SDK}"',
        ),
        encoding="utf-8",
    )
    run(
        [
            "cargo",
            "build",
            "--manifest-path",
            str(dest / "Cargo.toml"),
            "--target",
            "wasm32-unknown-unknown",
            "--release",
        ],
        dest,
    )
    core = dest / "target" / "wasm32-unknown-unknown" / "release" / f"{crate}.wasm"
    if not core.is_file():
        raise SystemExit(f"missing template wasm: {core}")
    encoded = dest / "component.wasm"
    run(
        [
            "cargo",
            "run",
            "--manifest-path",
            str(ENCODER / "Cargo.toml"),
            "--quiet",
            "--",
            str(core),
            str(encoded),
        ],
        REPO_ROOT,
    )
    return encoded


def main() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        toolset = build_template(
            TEMPLATE,
            tmp_path / "toolset",
            "finstack_ai_plugin_template_toolset",
        )
        build_template(
            CONTEXT_TEMPLATE,
            tmp_path / "context",
            "finstack_ai_plugin_template_context",
        )
        env = os.environ.copy()
        env["FINSTACK_TEMPLATE_WASM"] = str(toolset)
        run(
            [
                "cargo",
                "test",
                "-p",
                "finstack-ai-plugin-host",
                "--offline",
                "--locked",
                "--lib",
                "--",
                "reference_tests::template_project_builds_and_runs",
                "--exact",
                "--ignored",
                "--nocapture",
            ],
            REPO_ROOT,
            env,
        )


if __name__ == "__main__":
    main()
