#!/usr/bin/env python3
"""Build and encode the PR-051/PR-052 test-only plugin guests."""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
GUESTS = REPO_ROOT / "plugins" / "finstack-ai-plugin-host" / "fixtures" / "guests"
ENCODER = REPO_ROOT / "tools" / "plugin_wasm" / "encoder"
GUEST_NAMES = ("echo-toolset", "reference-context", "trap-toolset", "fuel-burner")


def run(command: list[str], cwd: Path) -> None:
    completed = subprocess.run(command, cwd=cwd, check=False)
    if completed.returncode != 0:
        raise SystemExit(completed.returncode)


def build_guest(name: str) -> Path:
    manifest = GUESTS / name / "Cargo.toml"
    run(
        [
            "cargo",
            "build",
            "--manifest-path",
            str(manifest),
            "--target",
            "wasm32-unknown-unknown",
            "--release",
        ],
        REPO_ROOT,
    )
    crate = {
        "echo-toolset": "finstack_ai_plugin_guest_echo_toolset",
        "reference-context": "finstack_ai_plugin_guest_reference_context",
        "trap-toolset": "finstack_ai_plugin_guest_trap_toolset",
        "fuel-burner": "finstack_ai_plugin_guest_fuel_burner",
    }[name]
    target_dir = GUESTS / name / "target" / "wasm32-unknown-unknown" / "release"
    # Guests are not workspace members; cargo writes next to the manifest.
    core = target_dir / f"{crate}.wasm"
    if not core.is_file():
        # Fallback if CARGO_TARGET_DIR is the workspace target.
        core = (
            REPO_ROOT
            / "target"
            / "wasm32-unknown-unknown"
            / "release"
            / f"{crate}.wasm"
        )
    if not core.is_file():
        raise SystemExit(f"missing core wasm for {name}: {core}")
    return core


def encode(core: Path, dest: Path) -> None:
    dest.parent.mkdir(parents=True, exist_ok=True)
    run(
        [
            "cargo",
            "run",
            "--manifest-path",
            str(ENCODER / "Cargo.toml"),
            "--quiet",
            "--",
            str(core),
            str(dest),
        ],
        REPO_ROOT,
    )


def generate() -> None:
    for name in GUEST_NAMES:
        core = build_guest(name)
        encode(core, GUESTS / name / "component.wasm")


def check() -> None:
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        for name in GUEST_NAMES:
            core = build_guest(name)
            generated = tmp_path / f"{name}.wasm"
            encode(core, generated)
            checked = GUESTS / name / "component.wasm"
            if not checked.is_file():
                raise SystemExit(f"missing checked-in component: {checked}")
            if generated.read_bytes() != checked.read_bytes():
                raise SystemExit(f"component drift: {checked}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.check:
        check()
    else:
        generate()


if __name__ == "__main__":
    main()
