#!/usr/bin/env python3
"""Selected wasm-host graph, generated-glue, and bundle-size checks."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
GENERATED_DIR = REPO_ROOT / "bindings" / "finstack-ai-wasm" / "js" / "generated"
WASM_PACKAGE = "finstack-ai-wasm"
KERNEL_PACKAGE = "finstack-ai-kernel"
FORBIDDEN_WASM = frozenset(
    {
        "tokio",
        "reqwest",
        "rusqlite",
        "pyo3",
        "wasmtime",
        "hyper",
        "native-tls",
        "finstack-ai-provider-openai",
        "finstack-ai-provider-ollama",
        "finstack-ai-provider-anthropic",
        "finstack-ai-provider-gateway",
        "finstack-ai-tools-mcp",
        "finstack-ai-tools-shell",
        "finstack-ai-tools-subagent",
        "finstack-ai-tools-skills",
        "finstack-ai-tools-skill-import",
        "finstack-ai-context-repository",
        "finstack-ai-context-memory",
        "finstack-ai-middleware-compaction",
        "finstack-ai-middleware-verify",
        "finstack-ai-observer-log",
        "finstack-ai-observer-otel",
        "finstack-ai-observer-metrics",
        "opentelemetry",
        "opentelemetry_sdk",
        "opentelemetry-otlp",
        "opentelemetry-http",
        "prometheus",
        "finstack-ai-remote-child",
        "finstack-ai-sandbox-e2b",
        "finstack-ai-server",
        "finstack-ai-workflow-local",
        "finstack-ai-workflow-temporal",
        "rustls",
        "rustls-pki-types",
        "rustls-webpki",
        "tokio-rustls",
    }
)
SECRET_ROOTS = (
    REPO_ROOT / "bindings" / "finstack-ai-wasm" / "js" / "src",
    REPO_ROOT / "bindings" / "finstack-ai-wasm" / "js" / "README.md",
    REPO_ROOT / "bindings" / "finstack-ai-wasm" / "js" / "harness.html",
    REPO_ROOT / "bindings" / "finstack-ai-wasm" / "js" / "worker-harness.html",
    REPO_ROOT / "bindings" / "finstack-ai-wasm" / "js" / "generated",
    REPO_ROOT / "examples" / "browser-minimal" / "index.html",
    REPO_ROOT / "examples" / "browser-minimal" / "main.ts",
    REPO_ROOT / "examples" / "browser-minimal" / "worker.ts",
    REPO_ROOT / "examples" / "browser-minimal" / "README.md",
    REPO_ROOT / "examples" / "README.md",
    REPO_ROOT / "bindings" / "finstack-ai-wasm" / "js" / "docs" / "browser-security.md",
    REPO_ROOT / "bindings" / "finstack-ai-wasm" / "js" / "docs" / "benchmarks.md",
    REPO_ROOT / "examples" / "ts-alpha-install" / "README.md",
    REPO_ROOT / "examples" / "ts-alpha-install" / "main.ts",
)
SECRET_PATTERNS = (
    "apiKey",
    "OPENAI_API_KEY",
    "ANTHROPIC_API_KEY",
    "sk-[A-Za-z0-9]{8,}",
)
FORBIDDEN_KERNEL = FORBIDDEN_WASM | frozenset(
    {"wasm-bindgen", "wasm-bindgen-futures", "js-sys"}
)
REQUIRED_WASM = frozenset({"finstack-ai-store-memory"})


def run_cargo_tree(package: str, extra_args: list[str]) -> set[str]:
    command = [
        "cargo",
        "tree",
        "-p",
        package,
        "--locked",
        "--prefix",
        "none",
        "--format",
        "{p}",
        "--edges",
        "normal",
        *extra_args,
    ]
    completed = subprocess.run(
        command,
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    names: set[str] = set()
    for line in completed.stdout.splitlines():
        package = line.strip()
        if not package:
            continue
        names.add(package.split()[0])
    return names


def check_graph() -> int:
    selected_graphs = {
        "finstack-ai-wasm": run_cargo_tree(
            WASM_PACKAGE,
            [
                "--target",
                "wasm32-unknown-unknown",
                "--no-default-features",
                "--features",
                "scripted-trace",
            ],
        ),
        "finstack-ai": run_cargo_tree(
            "finstack-ai",
            [
                "--target",
                "wasm32-unknown-unknown",
                "--no-default-features",
                "--features",
                "wasm-host",
            ],
        ),
        "finstack-ai-runtime": run_cargo_tree(
            "finstack-ai-runtime",
            [
                "--target",
                "wasm32-unknown-unknown",
                "--no-default-features",
                "--features",
                "wasm-host",
            ],
        ),
    }
    kernel_names = run_cargo_tree(
        KERNEL_PACKAGE, ["--target", "wasm32-unknown-unknown"]
    )
    for name, graph in selected_graphs.items():
        hits = sorted(graph & FORBIDDEN_WASM)
        if hits:
            print(
                f"error: {name} wasm-host graph contains forbidden crates: {hits}",
                file=sys.stderr,
            )
            return 1
        if "tokio" in graph:
            print(
                f"error: selected {name} wasm-host graph resolved Tokio",
                file=sys.stderr,
            )
            return 1
        if name == "finstack-ai-wasm":
            missing = sorted(REQUIRED_WASM - graph)
            if missing:
                print(
                    f"error: {name} wasm-host graph is missing required crates: {missing}",
                    file=sys.stderr,
                )
                return 1
    kernel_hits = sorted(kernel_names & FORBIDDEN_KERNEL)
    if kernel_hits:
        print(
            f"error: kernel wasm graph contains forbidden crates: {kernel_hits}",
            file=sys.stderr,
        )
        return 1
    print("wasm graph: no Tokio, sockets, filesystem drivers, or kernel wasm-bindgen")
    for name, graph in selected_graphs.items():
        print(f"{name} nodes: {len(graph)}")
    print(f"kernel package nodes: {len(kernel_names)}")
    return 0


def file_digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def generated_paths() -> list[Path]:
    if not GENERATED_DIR.is_dir():
        return []
    return sorted(
        path
        for path in GENERATED_DIR.iterdir()
        if path.is_file() and path.name != "GENERATED.md"
    )


def check_generated() -> int:
    paths = generated_paths()
    if not paths:
        print(
            "error: generated wasm glue is missing; run `mise run build-wasm -- release`",
            file=sys.stderr,
        )
        return 1
    print("generated glue:")
    for path in paths:
        print(
            f"  {path.relative_to(REPO_ROOT)} sha256 {file_digest(path)} bytes {path.stat().st_size}"
        )
    return 0


def brotli_size(data: bytes) -> int | None:
    try:
        import brotli
    except ImportError:
        return None
    return len(brotli.compress(data))


def check_size() -> int:
    wasm_files = [path for path in generated_paths() if path.suffix == ".wasm"]
    if not wasm_files:
        print("error: generated .wasm is missing", file=sys.stderr)
        return 1
    report: dict[str, object] = {"files": []}
    for path in generated_paths():
        raw = path.read_bytes()
        entry = {
            "path": str(path.relative_to(REPO_ROOT)),
            "bytes": len(raw),
            "gzip_bytes": len(gzip.compress(raw)),
            "sha256": hashlib.sha256(raw).hexdigest(),
        }
        compressed = brotli_size(raw)
        if compressed is not None:
            entry["brotli_bytes"] = compressed
        report["files"].append(entry)
        print(
            f"{entry['path']}: {entry['bytes']} bytes, gzip {entry['gzip_bytes']}"
            + (f", brotli {entry['brotli_bytes']}" if "brotli_bytes" in entry else "")
        )
    report_path = (
        REPO_ROOT
        / "docs"
        / "implementation"
        / "artifacts"
        / "pr-038"
        / "bundle-size.json"
    )
    report_path.parent.mkdir(parents=True, exist_ok=True)
    report_path.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(f"wrote {report_path.relative_to(REPO_ROOT)}")
    return 0


def snapshot_generated() -> dict[str, str]:
    return {
        str(path.relative_to(REPO_ROOT)): file_digest(path)
        for path in generated_paths()
    }


def write_snapshot(path: Path) -> int:
    snapshot = snapshot_generated()
    if not snapshot:
        print("error: generated wasm glue is missing", file=sys.stderr)
        return 1
    path.write_text(
        json.dumps(snapshot, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(f"wrote snapshot {path}")
    return 0


def compare_snapshot(path: Path) -> int:
    expected = json.loads(path.read_text(encoding="utf-8"))
    actual = snapshot_generated()
    if expected != actual:
        print(
            "error: consecutive wasm/glue builds are not byte-identical",
            file=sys.stderr,
        )
        print(f"expected: {json.dumps(expected, indent=2, sort_keys=True)}")
        print(f"actual: {json.dumps(actual, indent=2, sort_keys=True)}")
        return 1
    print("consecutive wasm/glue builds are byte-identical")
    return 0


def check_dirty() -> int:
    completed = subprocess.run(
        [
            "git",
            "status",
            "--porcelain",
            "--",
            "bindings/finstack-ai-wasm/js/generated",
            "bindings/finstack-ai-wasm/js/dist",
        ],
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    if completed.stdout.strip():
        print("error: generated wasm tree is dirty after regenerate", file=sys.stderr)
        print(completed.stdout)
        return 1
    print("generated wasm tree matches the worktree")
    return 0


def iter_secret_files() -> list[Path]:
    files: list[Path] = []
    for root in SECRET_ROOTS:
        if root.is_file():
            files.append(root)
            continue
        if not root.is_dir():
            continue
        for path in root.rglob("*"):
            if path.is_file() and path.suffix in {
                ".ts",
                ".js",
                ".md",
                ".html",
                ".d.ts",
            }:
                files.append(path)
    return files


def check_secrets() -> int:
    compiled = [re.compile(pattern) for pattern in SECRET_PATTERNS]
    hits: list[str] = []
    for path in iter_secret_files():
        text = path.read_text(encoding="utf-8")
        for pattern in compiled:
            if pattern.search(text):
                hits.append(f"{path.relative_to(REPO_ROOT)} matches {pattern.pattern}")
    if hits:
        print("error: provider-secret patterns found in JS sources", file=sys.stderr)
        for hit in hits:
            print(f"  {hit}", file=sys.stderr)
        return 1
    print("wasm secret scan: no credential field or provider-secret pattern")
    return 0


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "mode",
        choices=(
            "graph",
            "generated",
            "size",
            "snapshot",
            "compare",
            "dirty",
            "secrets",
            "all",
        ),
        help="check to run",
    )
    parser.add_argument(
        "path",
        nargs="?",
        type=Path,
        help="snapshot path for snapshot/compare modes",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if args.mode == "graph":
        return check_graph()
    if args.mode == "generated":
        return check_generated()
    if args.mode == "size":
        return check_size()
    if args.mode == "snapshot":
        if args.path is None:
            print("error: snapshot mode requires a path", file=sys.stderr)
            return 2
        return write_snapshot(args.path)
    if args.mode == "compare":
        if args.path is None:
            print("error: compare mode requires a path", file=sys.stderr)
            return 2
        return compare_snapshot(args.path)
    if args.mode == "dirty":
        return check_dirty()
    if args.mode == "secrets":
        return check_secrets()
    status = check_graph()
    if status != 0:
        return status
    status = check_secrets()
    if status != 0:
        return status
    status = check_generated()
    if status != 0:
        return status
    return check_size()


if __name__ == "__main__":
    raise SystemExit(main())
