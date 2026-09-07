#!/usr/bin/env python3
"""Build and encode test-only plugin guests and published reference components.

`--check` rebuilds every component. Byte-identity against the committed
files is a same-host obligation: rustc wasm32 output is not cross-OS
identical (see `.github/workflows/ci.yml` for the JS glue precedent).
Linux skips that compare unless `FINSTACK_PLUGIN_WASM_REPRO=1`.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
GUESTS = REPO_ROOT / "plugins" / "finstack-ai-plugin-host" / "fixtures" / "guests"
REFERENCES = REPO_ROOT / "plugins" / "reference"
ENCODER = REPO_ROOT / "scripts" / "plugin_wasm" / "encoder"
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


def run(command: list[str], cwd: Path, env: dict[str, str] | None = None) -> None:
    completed = subprocess.run(command, cwd=cwd, check=False, env=env)
    if completed.returncode != 0:
        raise SystemExit(completed.returncode)


def _remap_flags() -> list[str]:
    """Hide host paths so Linux CI and Darwin rebuilds compare equal."""
    flags: list[str] = []
    prefixes: list[Path] = [REPO_ROOT]
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    prefixes.append(cargo_home)
    rustup_home = Path(os.environ.get("RUSTUP_HOME", Path.home() / ".rustup"))
    toolchains = rustup_home / "toolchains"
    if toolchains.is_dir():
        prefixes.extend(path for path in toolchains.iterdir() if path.is_dir())
    sysroot = subprocess.run(
        ["rustc", "--print", "sysroot"],
        check=False,
        capture_output=True,
        text=True,
    )
    if sysroot.returncode == 0 and sysroot.stdout.strip():
        prefixes.append(Path(sysroot.stdout.strip()))
    seen: set[str] = set()
    for prefix in sorted(
        {path.resolve() for path in prefixes}, key=lambda path: -len(str(path))
    ):
        mapped = str(prefix)
        if mapped in seen:
            continue
        seen.add(mapped)
        if prefix == REPO_ROOT.resolve():
            flags.append(f"--remap-path-prefix={mapped}=/finstack")
        elif prefix == cargo_home.resolve():
            flags.append(f"--remap-path-prefix={mapped}=/cargo")
        else:
            flags.append(f"--remap-path-prefix={mapped}=/rustc-sysroot")
    return flags


def _build_env() -> dict[str, str]:
    env = os.environ.copy()
    env["CARGO_INCREMENTAL"] = "0"
    remaps = _remap_flags()
    encoded = env.get("CARGO_ENCODED_RUSTFLAGS")
    if encoded:
        env["CARGO_ENCODED_RUSTFLAGS"] = encoded + "\x1f" + "\x1f".join(remaps)
    else:
        existing = env.get("RUSTFLAGS", "")
        env["RUSTFLAGS"] = f"{existing} {' '.join(remaps)}".strip()
    return env


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
        env=_build_env(),
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


def _compare_committed_bytes() -> bool:
    match os.environ.get("FINSTACK_PLUGIN_WASM_REPRO"):
        case "1":
            return True
        case "0":
            return False
        case _:
            return not sys.platform.startswith("linux")


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
            if generated.read_bytes() == checked.read_bytes():
                continue
            if _compare_committed_bytes():
                raise SystemExit(f"component drift: {checked}")
            print(
                f"component host-rebuild differs (cross-OS rustc); source still builds: {checked}"
            )


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
