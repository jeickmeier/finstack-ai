#!/usr/bin/env python3
"""Collect experimental Rust source coverage from wasm-bindgen tests."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
OUTPUT = REPO_ROOT / "target" / "coverage" / "wasm"
PACKAGE = "finstack-ai-kernel"
CRATE_NAME = PACKAGE.replace("-", "_")
TEST_TARGET = "wasm_coverage"


def run(command: list[str], *, env: dict[str, str], capture: bool = False) -> str:
    completed = subprocess.run(
        command,
        cwd=REPO_ROOT,
        env=env,
        check=False,
        text=True,
        capture_output=capture,
    )
    if completed.returncode != 0:
        detail = f"{completed.stdout}\n{completed.stderr}" if capture else ""
        raise SystemExit(
            f"command failed ({completed.returncode}): {' '.join(command)}\n{detail}"
        )
    return completed.stdout if capture else ""


def llvm_ir_paths(cargo_output: str) -> list[Path]:
    paths: set[Path] = set()
    for line in cargo_output.splitlines():
        try:
            message = json.loads(line)
        except json.JSONDecodeError:
            continue
        if message.get("reason") != "compiler-artifact":
            continue
        target = message.get("target")
        if not isinstance(target, dict) or target.get("name") not in {
            CRATE_NAME,
            TEST_TARGET,
        }:
            continue
        filenames = message.get("filenames")
        if not isinstance(filenames, list):
            continue
        for filename in filenames:
            if not isinstance(filename, str):
                continue
            artifact = Path(filename)
            if artifact.suffix == ".wasm":
                candidate = artifact.with_suffix(".ll")
            elif artifact.suffix == ".rlib":
                stem = artifact.stem.removeprefix("lib")
                candidate = artifact.with_name(f"{stem}.ll")
            else:
                continue
            if candidate.is_file():
                paths.add(candidate)
    if not paths:
        raise SystemExit("coverage build produced no LLVM IR artifacts")
    return sorted(paths)


def tool_path(sysroot: Path, host: str, name: str) -> Path:
    path = sysroot / "lib" / "rustlib" / host / "bin" / name
    if not path.is_file():
        raise SystemExit(
            f"missing {name}; install llvm-tools-preview for the coverage toolchain"
        )
    return path


def wasm_clang() -> str:
    candidates = [
        os.environ.get("FINSTACK_WASM_CLANG"),
        "/opt/homebrew/opt/llvm/bin/clang",
        shutil.which("clang"),
    ]
    for candidate in dict.fromkeys(path for path in candidates if path):
        if not Path(candidate).is_file():
            continue
        completed = subprocess.run(
            [candidate, "--print-targets"],
            check=False,
            text=True,
            capture_output=True,
        )
        if completed.returncode == 0 and "WebAssembly" in completed.stdout:
            return candidate
    raise SystemExit(
        "no clang with the WebAssembly target is available; set FINSTACK_WASM_CLANG"
    )


def main() -> int:
    nightly = os.environ.get("FINSTACK_COVERAGE_NIGHTLY")
    if not nightly:
        raise SystemExit("FINSTACK_COVERAGE_NIGHTLY is required")
    runner = shutil.which("wasm-bindgen-test-runner")
    if runner is None:
        raise SystemExit("wasm-bindgen-test-runner is not installed")
    clang = wasm_clang()

    if OUTPUT.exists():
        shutil.rmtree(OUTPUT)
    objects_dir = OUTPUT / "objects"
    html_dir = OUTPUT / "html"
    objects_dir.mkdir(parents=True)

    env = {
        **os.environ,
        "CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUNNER": runner,
        "CC_wasm32_unknown_unknown": clang,
        "LLVM_PROFILE_FILE": str(OUTPUT / "coverage-%p-%m.profraw"),
        "RUSTFLAGS": (
            "-Cinstrument-coverage -Zno-profiler-runtime --emit=llvm-ir "
            "--cfg=wasm_bindgen_unstable_test_coverage"
        ),
    }
    cargo = [
        "cargo",
        f"+{nightly}",
        "test",
        "--locked",
        "-p",
        PACKAGE,
        "--target",
        "wasm32-unknown-unknown",
        "--test",
        TEST_TARGET,
        "--message-format=json",
    ]
    cargo_output = run(cargo, env=env, capture=True)
    ir_paths = llvm_ir_paths(cargo_output)

    profiles = sorted(OUTPUT.glob("*.profraw"))
    if not profiles:
        raise SystemExit("wasm-bindgen tests produced no .profraw coverage data")

    verbose = run(["rustc", f"+{nightly}", "-vV"], env=os.environ.copy(), capture=True)
    host = next(
        (
            line.removeprefix("host: ")
            for line in verbose.splitlines()
            if line.startswith("host: ")
        ),
        None,
    )
    if host is None:
        raise SystemExit("coverage toolchain did not report its host triple")
    sysroot = Path(
        run(
            ["rustc", f"+{nightly}", "--print", "sysroot"],
            env=os.environ.copy(),
            capture=True,
        ).strip()
    )
    llc = tool_path(sysroot, host, "llc")
    llvm_profdata = tool_path(sysroot, host, "llvm-profdata")
    llvm_cov = tool_path(sysroot, host, "llvm-cov")
    analysis_triple = f"{host.split('-', 1)[0]}-unknown-linux-gnu"

    objects: list[Path] = []
    for index, ir_path in enumerate(ir_paths):
        output = objects_dir / f"{index}-{ir_path.stem}.o"
        run(
            [
                str(llc),
                "-filetype=obj",
                f"-mtriple={analysis_triple}",
                str(ir_path),
                "-o",
                str(output),
            ],
            env=env,
        )
        objects.append(output)

    profile = OUTPUT / "coverage.profdata"
    run(
        [
            str(llvm_profdata),
            "merge",
            "-sparse",
            *map(str, profiles),
            "-o",
            str(profile),
        ],
        env=env,
    )
    primary, *additional = objects
    object_args = [
        argument for path in additional for argument in ("-object", str(path))
    ]
    source = REPO_ROOT / "crates" / "finstack-ai-kernel" / "src"
    run(
        [
            str(llvm_cov),
            "show",
            str(primary),
            *object_args,
            f"-instr-profile={profile}",
            "-show-instantiations=false",
            "-format=html",
            f"-output-dir={html_dir}",
            "-sources",
            str(source),
        ],
        env=env,
    )
    lcov = run(
        [
            str(llvm_cov),
            "export",
            str(primary),
            *object_args,
            f"-instr-profile={profile}",
            "-format=lcov",
            "-sources",
            str(source),
        ],
        env=env,
        capture=True,
    )
    (OUTPUT / "lcov.info").write_text(lcov, encoding="utf-8")
    summary = run(
        [
            str(llvm_cov),
            "report",
            str(primary),
            *object_args,
            f"-instr-profile={profile}",
            "-sources",
            str(source),
        ],
        env=env,
        capture=True,
    )
    (OUTPUT / "summary.txt").write_text(summary, encoding="utf-8")
    print(summary, end="")
    print(
        f"wrote {html_dir.relative_to(REPO_ROOT)} and {(OUTPUT / 'lcov.info').relative_to(REPO_ROOT)}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
