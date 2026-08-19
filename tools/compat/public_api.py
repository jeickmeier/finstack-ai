#!/usr/bin/env python3
"""Compare cargo-public-api dumps to frozen Rust public-API baselines.

Covers finstack-ai-kernel, finstack-ai-runtime, finstack-ai, and every
extensions/** crate. Runtime also dumps `native-tokio` and `wasm-host`
because its default feature set is empty. Python/JS name lists stay in
public_items.py.
"""

from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
BASELINE_DIR = (
    REPO_ROOT / "fixtures" / "compatibility" / "public-rust-api" / "cargo-public-api"
)
# cargo-public-api 0.52 rustdoc JSON minimum. Check-only; never crate builds.
NIGHTLY = "nightly-2025-08-02"
CORE_CRATES = (
    REPO_ROOT / "crates" / "finstack-ai-kernel",
    REPO_ROOT / "crates" / "finstack-ai-runtime",
    REPO_ROOT / "crates" / "finstack-ai",
)


def crate_dirs() -> list[Path]:
    crates = list(CORE_CRATES)
    extensions = REPO_ROOT / "extensions"
    crates.extend(sorted(path.parent for path in extensions.glob("*/**/Cargo.toml")))
    return crates


def package_name(crate_dir: Path) -> str:
    for line in (crate_dir / "Cargo.toml").read_text(encoding="utf-8").splitlines():
        if line.startswith("name = "):
            return line.split("=", 1)[1].strip().strip('"')
    raise SystemExit(f"{crate_dir}: missing package name")


def baseline_path(name: str) -> Path:
    return BASELINE_DIR / f"{name}.txt"


def ensure_nightly() -> None:
    listed = subprocess.run(
        ["rustup", "toolchain", "list"],
        check=True,
        capture_output=True,
        text=True,
    )
    if NIGHTLY in listed.stdout:
        return
    subprocess.run(
        [
            "mise",
            "install",
            f"rust@{NIGHTLY}",
        ],
        check=True,
    )


def cargo_public_api_bin() -> str:
    which = subprocess.run(
        ["mise", "where", "cargo:cargo-public-api"],
        check=True,
        capture_output=True,
        text=True,
    )
    root = Path(which.stdout.strip())
    for candidate in (root / "bin" / "cargo-public-api", root / "cargo-public-api"):
        if candidate.is_file():
            return str(candidate)
    raise SystemExit(f"cargo-public-api not found under {root}")


# Extra feature dumps for crates whose default features hide the production
# surface. Runtime `default = []`, so `native-tokio` / `wasm-host` names
# never appear in the default dump.
FEATURED_DUMPS: dict[str, tuple[str, ...]] = {
    "finstack-ai-runtime": ("native-tokio", "wasm-host"),
}


def dump_label(name: str, feature: str | None) -> str:
    return name if feature is None else f"{name}+{feature}"


def dump_crate(crate_dir: Path, features: str | None = None) -> str:
    cmd = [
        cargo_public_api_bin(),
        "--color",
        "never",
        "-ss",
        "--manifest-path",
        str(crate_dir / "Cargo.toml"),
    ]
    if features:
        cmd.extend(["--features", features])
    proc = subprocess.run(
        cmd,
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        label = features or "default"
        raise SystemExit(
            f"{crate_dir} ({label}): cargo-public-api failed ({proc.returncode})"
        )
    return proc.stdout


def write_list(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def compare(current: str, baseline: str) -> tuple[list[str], list[str]]:
    current_lines = [line for line in current.splitlines() if line]
    baseline_lines = [line for line in baseline.splitlines() if line]
    current_set = set(current_lines)
    baseline_set = set(baseline_lines)
    removed = [line for line in baseline_lines if line not in current_set]
    added = [line for line in current_lines if line not in baseline_set]
    return removed, added


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if not args.write and not args.check:
        parser.error("pass --write or --check")
    ensure_nightly()
    failed = False
    for crate_dir in crate_dirs():
        name = package_name(crate_dir)
        features = (None,) + FEATURED_DUMPS.get(name, ())
        for feature in features:
            label = dump_label(name, feature)
            current = dump_crate(crate_dir, feature)
            path = baseline_path(label)
            if args.write:
                write_list(path, current)
                continue
            if not path.is_file():
                print(f"{label}: missing baseline {path}", file=sys.stderr)
                failed = True
                continue
            removed, added = compare(current, path.read_text(encoding="utf-8"))
            if removed:
                print(f"{label}: removed public API:", file=sys.stderr)
                for line in removed:
                    print(f"  - {line}", file=sys.stderr)
                failed = True
            if added:
                print(f"{label}: added public API:", file=sys.stderr)
                for line in added:
                    print(f"  + {line}", file=sys.stderr)
                failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
