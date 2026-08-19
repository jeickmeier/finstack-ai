#!/usr/bin/env python3
"""Run in-tree starters against staged RC artifacts (PR-065-A02)."""

from __future__ import annotations

import json
import os
import re
import shutil
import subprocess
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
STAGE = REPO_ROOT / "target" / "pr-065-rc"
PYTHON = REPO_ROOT / "bindings" / "finstack-ai-python"
JS_PACKAGE = REPO_ROOT / "bindings" / "finstack-ai-wasm" / "js"
STARTERS = (
    "finstack-ai-native-examples",
    "finstack-ai-example-durable-interaction",
)


def run(
    command: list[str], cwd: Path | None = None
) -> subprocess.CompletedProcess[str]:
    print("+", " ".join(command), flush=True)
    completed = subprocess.run(
        command,
        cwd=cwd or REPO_ROOT,
        text=True,
        env={**os.environ, "SOURCE_DATE_EPOCH": "0", "CARGO_INCREMENTAL": "0"},
    )
    if completed.returncode != 0:
        raise SystemExit(completed.returncode)
    return completed


def cargo_metadata() -> dict[str, object]:
    completed = subprocess.run(
        ["cargo", "metadata", "--locked", "--format-version", "1"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        check=True,
    )
    return json.loads(completed.stdout)


def workspace_closure(names: tuple[str, ...]) -> list[dict[str, object]]:
    metadata = cargo_metadata()
    workspace_ids = set(metadata["workspace_members"])
    by_name = {
        pkg["name"]: pkg for pkg in metadata["packages"] if pkg["id"] in workspace_ids
    }
    needed: set[str] = set()
    stack = list(names)
    while stack:
        name = stack.pop()
        if name in needed or name not in by_name:
            continue
        needed.add(name)
        pkg = by_name[name]
        for dep in pkg.get("dependencies", []):
            dep_name = dep["name"]
            if dep_name in by_name:
                stack.append(dep_name)
    return [by_name[name] for name in sorted(needed)]


def package_list(name: str) -> list[str]:
    completed = subprocess.run(
        [
            "cargo",
            "package",
            "--list",
            "--locked",
            "--offline",
            "-p",
            name,
            "--allow-dirty",
        ],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        check=True,
    )
    return [line.strip() for line in completed.stdout.splitlines() if line.strip()]


def copy_package(pkg: dict[str, object], dest_root: Path) -> str:
    manifest = Path(str(pkg["manifest_path"]))
    src_root = manifest.parent
    rel = src_root.relative_to(REPO_ROOT)
    dest = dest_root / rel
    dest.mkdir(parents=True, exist_ok=True)
    for entry in package_list(str(pkg["name"])):
        source = src_root / entry
        if not source.is_file():
            # cargo package --list can name workspace license files.
            source = REPO_ROOT / entry
        if not source.is_file():
            continue
        target = dest / entry
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, target)
    return rel.as_posix()


def rewrite_members(text: str, members: list[str]) -> str:
    block = "members = [\n" + "".join(f'    "{item}",\n' for item in members) + "]"
    return re.sub(r"members = \[[^\]]*\]", block, text, count=1, flags=re.S)


def write_rc_example(dest: Path, name: str, source: Path, extra: str) -> None:
    dest.mkdir(parents=True)
    shutil.copytree(source / "src", dest / "src")
    (dest / "Cargo.toml").write_text(
        f"""[package]
name = "{name}"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
publish = false

[lints]
workspace = true

{extra}
""",
        encoding="utf-8",
    )


def rust_starters(stage: Path) -> None:
    packages = workspace_closure(STARTERS)
    members = []
    for pkg in packages:
        if str(pkg["name"]) in STARTERS:
            continue
        members.append(copy_package(pkg, stage))
    members.extend(
        [
            "examples/rc-rust-minimal",
            "examples/rc-durable-interaction",
        ]
    )
    root_toml = (REPO_ROOT / "Cargo.toml").read_text(encoding="utf-8")
    (stage / "Cargo.toml").write_text(
        rewrite_members(root_toml, members), encoding="utf-8"
    )
    shutil.copy2(REPO_ROOT / "Cargo.lock", stage / "Cargo.lock")
    licenses = REPO_ROOT / "licenses"
    if licenses.is_dir():
        shutil.copytree(licenses, stage / "licenses")
    write_rc_example(
        stage / "examples" / "rc-rust-minimal",
        "finstack-ai-native-examples",
        REPO_ROOT / "examples" / "rust-minimal",
        """[dependencies]
finstack-ai = { workspace = true, features = ["native-tokio"] }
finstack-ai-provider-ollama = { workspace = true }
finstack-ai-store-memory = { workspace = true }
finstack-ai-tools-calculator = { workspace = true }
finstack-ai-tools-filesystem = { workspace = true }
finstack-ai-tools-shell = { workspace = true }
finstack-ai-context-repository = { workspace = true }
finstack-ai-context-memory = { workspace = true }
finstack-ai-middleware-compaction = { workspace = true }
finstack-ai-middleware-verify = { workspace = true }
tokio = { workspace = true, features = ["io-util", "net", "rt-multi-thread"] }
""",
    )
    write_rc_example(
        stage / "examples" / "rc-durable-interaction",
        "finstack-ai-example-durable-interaction",
        REPO_ROOT / "examples" / "durable-interaction",
        """[dependencies]
finstack-ai-kernel = { workspace = true }
finstack-ai-runtime = { workspace = true, default-features = false, features = ["native-tokio"] }
finstack-ai-store-sqlite = { workspace = true }
finstack-ai-test = { workspace = true }
tempfile = { workspace = true }
tokio = { workspace = true, features = ["rt", "macros", "time"] }
""",
    )
    run(
        [
            "cargo",
            "generate-lockfile",
            "--offline",
            "--manifest-path",
            str(stage / "Cargo.toml"),
        ],
        stage,
    )
    metadata = json.loads(
        subprocess.run(
            [
                "cargo",
                "metadata",
                "--locked",
                "--offline",
                "--format-version",
                "1",
                "--manifest-path",
                str(stage / "Cargo.toml"),
            ],
            cwd=stage,
            capture_output=True,
            text=True,
            check=True,
        ).stdout
    )
    workspace_root = Path(str(metadata["workspace_root"])).resolve()
    if workspace_root != stage.resolve():
        raise SystemExit(
            f"rust RC still maps to workspace {workspace_root}, not {stage}"
        )
    repo_crates = (REPO_ROOT / "crates").resolve()
    for pkg in metadata["packages"]:
        manifest = Path(str(pkg["manifest_path"])).resolve()
        if repo_crates in manifest.parents and str(pkg["name"]).startswith(
            "finstack-ai"
        ):
            raise SystemExit(
                f"rust RC path-mapped repo crate {pkg['name']}: {manifest}"
            )
    run(
        [
            "cargo",
            "run",
            "--offline",
            "--locked",
            "--manifest-path",
            str(stage / "examples" / "rc-rust-minimal" / "Cargo.toml"),
            "--bin",
            "minimal",
        ],
        stage,
    )
    run(
        [
            "cargo",
            "run",
            "--offline",
            "--locked",
            "--manifest-path",
            str(stage / "examples" / "rc-durable-interaction" / "Cargo.toml"),
        ],
        stage,
    )


def python_starters(wheel: Path) -> None:
    if "editable" in str(wheel):
        raise SystemExit("python RC must not use an editable workspace mapping")
    for script in (
        REPO_ROOT / "examples" / "python-minimal" / "python-callback" / "main.py",
        REPO_ROOT / "examples" / "python-minimal" / "rust-backed" / "main.py",
        REPO_ROOT / "examples" / "python-minimal" / "service" / "main.py",
    ):
        run(
            [
                "uv",
                "run",
                "--isolated",
                "--no-project",
                "--with",
                str(wheel),
                "python",
                str(script),
            ]
        )


def ts_starter(tarball: Path) -> None:
    with tempfile.TemporaryDirectory(prefix="finstack-ts-rc-") as raw:
        copied = Path(raw) / "ts-alpha-install"
        shutil.copytree(
            REPO_ROOT / "examples" / "ts-alpha-install",
            copied,
            ignore=shutil.ignore_patterns("node_modules"),
        )
        run(["npm", "install", "--omit=dev", str(tarball)], copied)
        run(["npm", "install", "--ignore-scripts", "typescript@5.9.2"], copied)
        run(["npx", "tsc", "--noEmit", "-p", "tsconfig.json"], copied)
        pkg = json.loads((copied / "package.json").read_text(encoding="utf-8"))
        resolved = (pkg.get("dependencies") or {}).get("@finstack/ai", "")
        if "bindings/finstack-ai-wasm" in str(resolved):
            raise SystemExit("ts RC still path-maps the workspace JS package")


def browser_starter() -> None:
    html = (REPO_ROOT / "examples" / "browser-minimal" / "index.html").read_text(
        encoding="utf-8"
    )
    for label in ("Run scripted session", "Inspect last session", "Clear local data"):
        if label not in html:
            raise SystemExit(f"browser-minimal missing labeled control: {label}")
    if '"@finstack/ai": "/dist/index.js"' not in html:
        raise SystemExit("browser-minimal must consume staged /dist/, not repo src/")
    if "bindings/finstack-ai-wasm/js/src" in html:
        raise SystemExit("browser-minimal path-maps workspace JS src")


def build_python_wheel(out: Path) -> Path:
    out.mkdir(parents=True, exist_ok=True)
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
            str(out),
        ]
    )
    wheels = sorted(out.glob("*.whl"))
    if not wheels:
        raise SystemExit("maturin did not write a wheel")
    return wheels[-1]


def pack_npm(out: Path) -> Path:
    if not (JS_PACKAGE / "dist" / "index.js").is_file():
        raise SystemExit(
            "npm dist/ missing; run `mise run build-wasm -- release` first"
        )
    out.mkdir(parents=True, exist_ok=True)
    completed = subprocess.run(
        ["npm", "pack", "--pack-destination", str(out)],
        cwd=JS_PACKAGE,
        capture_output=True,
        text=True,
        check=True,
    )
    name = completed.stdout.strip().splitlines()[-1]
    packed = out / name
    if not packed.is_file():
        raise SystemExit(f"npm pack did not write {packed}")
    return packed


def main() -> int:
    if STAGE.exists():
        shutil.rmtree(STAGE)
    STAGE.mkdir(parents=True)
    rust_starters(STAGE / "rust")
    wheel = build_python_wheel(STAGE / "python")
    python_starters(wheel)
    tarball = pack_npm(STAGE / "npm")
    ts_starter(tarball)
    browser_starter()
    print(
        f"ok starter RC against staged artifacts under {STAGE.relative_to(REPO_ROOT)}"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
