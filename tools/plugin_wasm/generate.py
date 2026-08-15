#!/usr/bin/env python3
"""Build and encode test-only plugin guests and published reference components."""

from __future__ import annotations

import argparse
import subprocess
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
GUESTS = REPO_ROOT / "plugins" / "finstack-ai-plugin-host" / "fixtures" / "guests"
REFERENCES = REPO_ROOT / "plugins" / "reference"
ENCODER = REPO_ROOT / "tools" / "plugin_wasm" / "encoder"
GUEST_NAMES = ("echo-toolset", "trap-toolset", "fuel-burner")
REFERENCE_NAMES = ("calculator", "filesystem-sandbox", "context-provider")
GUEST_CRATES = {
    "echo-toolset": "finstack_ai_plugin_guest_echo_toolset",
    "trap-toolset": "finstack_ai_plugin_guest_trap_toolset",
    "fuel-burner": "finstack_ai_plugin_guest_fuel_burner",
}
REFERENCE_CRATES = {
    "calculator": "finstack_ai_plugin_ref_calculator",
    "filesystem-sandbox": "finstack_ai_plugin_ref_filesystem_sandbox",
    "context-provider": "finstack_ai_plugin_ref_context",
}


def run(command: list[str], cwd: Path) -> None:
    completed = subprocess.run(command, cwd=cwd, check=False)
    if completed.returncode != 0:
        raise SystemExit(completed.returncode)


def build(manifest: Path, crate: str) -> Path:
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
    target_dir = manifest.parent / "target" / "wasm32-unknown-unknown" / "release"
    core = target_dir / f"{crate}.wasm"
    if not core.is_file():
        core = (
            REPO_ROOT
            / "target"
            / "wasm32-unknown-unknown"
            / "release"
            / f"{crate}.wasm"
        )
    if not core.is_file():
        raise SystemExit(f"missing core wasm for {crate}: {core}")
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


def items() -> list[tuple[Path, str, str]]:
    listed: list[tuple[Path, str, str]] = []
    for name in GUEST_NAMES:
        listed.append((GUESTS / name, name, GUEST_CRATES[name]))
    for name in REFERENCE_NAMES:
        listed.append((REFERENCES / name, name, REFERENCE_CRATES[name]))
    return listed


def generate() -> None:
    for root, _name, crate in items():
        core = build(root / "Cargo.toml", crate)
        encode(core, root / "component.wasm")


def check() -> None:
    import tempfile

    with tempfile.TemporaryDirectory() as tmp:
        tmp_path = Path(tmp)
        for root, name, crate in items():
            core = build(root / "Cargo.toml", crate)
            generated = tmp_path / f"{name}.wasm"
            encode(core, generated)
            checked = root / "component.wasm"
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
