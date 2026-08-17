#!/usr/bin/env python3
"""Measure release-profile and TLS/provider graph costs without changing defaults.

This is a host-local packaging probe. It does not rewrite ``[profile.release]``,
does not drop linked Python providers, and does not switch reqwest onto rustls.
Full maturin wheel rebuilds and Criterion NFR re-benches against an alternate
profile are opt-in because they contend with the default release artifacts.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import subprocess
import sys
import time
from pathlib import Path
from typing import Mapping

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_OUT = (
    REPO_ROOT
    / "docs"
    / "implementation"
    / "artifacts"
    / "perf-phase5"
    / "packaging-profile-report.json"
)
WHEEL_BUDGET_BYTES = 10_485_760

PROFILES: dict[str, dict[str, str]] = {
    "default-release": {},
    "thin-lto": {
        "CARGO_PROFILE_RELEASE_LTO": "thin",
        "CARGO_PROFILE_RELEASE_CODEGEN_UNITS": "16",
    },
}

BUILD_PACKAGES: tuple[tuple[str, tuple[str, ...], str], ...] = (
    ("finstack-ai-kernel", (), "libfinstack_ai_kernel.rlib"),
    (
        "finstack-ai-runtime",
        ("--features", "native-tokio"),
        "libfinstack_ai_runtime.rlib",
    ),
    ("finstack-ai-python", (), "lib_finstack_ai.dylib"),
)

TREE_TARGETS: tuple[tuple[str, tuple[str, ...]], ...] = (
    ("finstack-ai-provider-openai-compatible", ()),
    ("finstack-ai-provider-openai-compatible", ("--features", "vendored-tls")),
    ("finstack-ai-provider-anthropic", ()),
    ("finstack-ai-provider-anthropic", ("--features", "vendored-tls")),
    ("finstack-ai-python", ()),
    ("finstack-ai-server", ()),
)

INVERT_TARGETS: tuple[tuple[str, str, tuple[str, ...]], ...] = (
    ("finstack-ai-python", "native-tls", ()),
    ("finstack-ai-python", "rustls", ()),
    ("finstack-ai-python", "openssl-src", ()),
    ("finstack-ai-python", "openssl-src", ("--edges", "normal,build")),
    ("finstack-ai-server", "rustls", ()),
    ("finstack-ai-server", "native-tls", ()),
)


def run(
    command: list[str],
    *,
    env: Mapping[str, str] | None = None,
    check: bool = True,
) -> subprocess.CompletedProcess[str]:
    merged = os.environ.copy()
    if env is not None:
        merged.update(env)
    return subprocess.run(
        command,
        cwd=REPO_ROOT,
        check=check,
        capture_output=True,
        text=True,
        env=merged,
    )


def invert_edges(package: str, crate: str, extra: tuple[str, ...]) -> dict[str, object]:
    command = [
        "cargo",
        "tree",
        "-p",
        package,
        "--locked",
        "--offline",
        "-i",
        crate,
        *(extra if extra else ("--edges", "normal")),
    ]
    completed = run(command, check=False)
    present = completed.returncode == 0 and bool(completed.stdout.strip())
    edges = " ".join(extra) if extra else "--edges normal"
    return {
        "package": package,
        "crate": crate,
        "edges": edges,
        "present": present,
        "detail": "present" if present else "absent",
    }


def unique_packages(package: str, extra: tuple[str, ...]) -> dict[str, object]:
    command = [
        "cargo",
        "tree",
        "-p",
        package,
        "--locked",
        "--offline",
        "--edges",
        "normal",
        "--prefix",
        "none",
        "--format",
        "{p}",
        *extra,
    ]
    completed = run(command, check=False)
    label = f"{package}{' ' + ' '.join(extra) if extra else ''}"
    if completed.returncode != 0:
        return {
            "target": label,
            "ok": False,
            "error": completed.stderr.strip().splitlines()[-1]
            if completed.stderr.strip()
            else f"exit {completed.returncode}",
        }
    packages = sorted(
        {line.strip() for line in completed.stdout.splitlines() if line.strip()}
    )
    names = sorted({item.split(" ", 1)[0] for item in packages})
    return {
        "target": label,
        "ok": True,
        "unique_packages": len(packages),
        "unique_names": len(names),
    }


def artifact_path(target_dir: Path, filename: str) -> Path | None:
    candidate = target_dir / "release" / filename
    if candidate.is_file():
        return candidate
    if filename.endswith(".dylib"):
        matches = sorted(
            (target_dir / "release" / "deps").glob("lib_finstack_ai*.dylib")
        )
        if matches:
            return matches[-1]
    return None


def build_package(
    package: str,
    extra: tuple[str, ...],
    filename: str,
    profile_name: str,
    profile_env: Mapping[str, str],
) -> dict[str, object]:
    target_dir = REPO_ROOT / "target" / "packaging-profile" / profile_name
    target_dir.mkdir(parents=True, exist_ok=True)
    command = [
        "cargo",
        "build",
        "-p",
        package,
        "--release",
        "--locked",
        "--offline",
        *extra,
    ]
    env = {
        "CARGO_TARGET_DIR": str(target_dir),
        "CARGO_INCREMENTAL": "0",
        **profile_env,
    }
    started = time.perf_counter()
    completed = run(command, env=env, check=False)
    elapsed_s = time.perf_counter() - started
    artifact = (
        artifact_path(target_dir, filename) if completed.returncode == 0 else None
    )
    result: dict[str, object] = {
        "package": package,
        "profile": profile_name,
        "ok": completed.returncode == 0,
        "elapsed_s": round(elapsed_s, 3),
        "target_dir": str(target_dir.relative_to(REPO_ROOT)),
    }
    if artifact is not None:
        result["artifact"] = str(artifact.relative_to(REPO_ROOT))
        result["bytes"] = artifact.stat().st_size
    if completed.returncode != 0:
        result["error"] = _compiler_error(completed.stderr, completed.stdout)
    return result


def _compiler_error(stderr: str, stdout: str) -> str:
    lines = [
        line.strip()
        for line in f"{stderr}\n{stdout}".splitlines()
        if line.startswith("error")
    ]
    if lines:
        return lines[-1]
    tail = (stderr or stdout).strip().splitlines()
    return tail[-1] if tail else "build failed"


def rustc_version() -> str:
    completed = run(["rustc", "--version"], check=False)
    return completed.stdout.strip() if completed.returncode == 0 else "unknown"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    parser.add_argument(
        "--skip-builds",
        action="store_true",
        help="Record cargo-tree graphs only; skip isolated release builds.",
    )
    parser.add_argument(
        "--packages",
        default="kernel,runtime,python",
        help="Comma-separated build set: kernel,runtime,python",
    )
    return parser.parse_args()


def selected_builds(raw: str) -> tuple[tuple[str, tuple[str, ...], str], ...]:
    aliases = {
        "kernel": "finstack-ai-kernel",
        "runtime": "finstack-ai-runtime",
        "python": "finstack-ai-python",
    }
    wanted = {
        aliases.get(item.strip(), item.strip())
        for item in raw.split(",")
        if item.strip()
    }
    return tuple(item for item in BUILD_PACKAGES if item[0] in wanted)


def main() -> int:
    args = parse_args()
    graphs = [unique_packages(package, extra) for package, extra in TREE_TARGETS]
    invert = [
        invert_edges(package, crate, extra) for package, crate, extra in INVERT_TARGETS
    ]
    builds: list[dict[str, object]] = []
    if not args.skip_builds:
        for profile_name, profile_env in PROFILES.items():
            for package, extra, filename in selected_builds(args.packages):
                print(f"building {package} profile={profile_name}", file=sys.stderr)
                builds.append(
                    build_package(package, extra, filename, profile_name, profile_env)
                )
    report = {
        "format_version": 1,
        "workload": "packaging-profile-v1",
        "host": {
            "system": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "rustc": rustc_version(),
        },
        "default_release_profile": {
            "codegen_units": 1,
            "lto": True,
            "strip": "symbols",
            "incremental": False,
        },
        "measured_profiles": {
            "default-release": "workspace [profile.release] (full LTO, codegen-units=1)",
            "thin-lto": "CARGO_PROFILE_RELEASE_LTO=thin, CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16",
        },
        "wheel_budget_bytes": WHEEL_BUDGET_BYTES,
        "public_python_providers": [
            "openai-compatible",
            "anthropic",
            "ollama",
        ],
        "disposition": {
            "default_profile_changed": False,
            "providers_dropped": False,
            "rustls_switched": False,
            "full_wheel_rebuilt": False,
            "nfr_runtime_rebenched": False,
        },
        "graphs": graphs,
        "invert": invert,
        "builds": builds,
    }
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps(report, indent=2, sort_keys=True))
    print(f"wrote {args.out.relative_to(REPO_ROOT)}", file=sys.stderr)
    failed = any(not item.get("ok", False) for item in graphs) or any(
        not item.get("ok", False) for item in builds
    )
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
