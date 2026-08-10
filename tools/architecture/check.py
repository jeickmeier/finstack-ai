#!/usr/bin/env python3
"""Architecture and dependency enforcement for finstack-ai.

Thin entry point intended for ``mise run architecture``. Uses only the Python
standard library plus ``cargo metadata`` / Cargo manifests. No Cargo xtask.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time
import tomllib
from collections.abc import Iterator, Mapping, Sequence
from dataclasses import dataclass, field
from datetime import date
from pathlib import Path
from typing import Any

TOOL_DIR = Path(__file__).resolve().parent
REPO_ROOT = TOOL_DIR.parents[1]
DEFAULT_POLICY = TOOL_DIR / "policy.toml"
DEFAULT_ALLOWLIST = TOOL_DIR / "allowlist.toml"


def resolve_tool(name: str) -> str:
    """Resolve a CLI tool for subprocess use when PATH is incomplete under uv.

    On Windows CI, ``uv run`` can drop ``CARGO_HOME/bin`` from PATH even though
    the outer mise/GitHub Actions environment still exports ``CARGO_HOME``.
    """
    exe = f"{name}.exe" if os.name == "nt" else name
    for candidate_name in (name, exe):
        found = shutil.which(candidate_name)
        if found:
            return found

    if name in {"cargo", "rustc", "rustup"}:
        roots: list[Path] = []
        cargo_home = os.environ.get("CARGO_HOME")
        if cargo_home:
            roots.append(Path(cargo_home))
        roots.append(Path.home() / ".cargo")
        for root in roots:
            candidate = root / "bin" / exe
            if candidate.is_file():
                return str(candidate)

    mise = shutil.which("mise") or (shutil.which("mise.exe") if os.name == "nt" else None)
    if mise:
        proc = subprocess.run(
            [mise, "which", name],
            check=False,
            capture_output=True,
            text=True,
        )
        path = proc.stdout.strip()
        if proc.returncode == 0 and path:
            return path

    raise FileNotFoundError(f"{name} executable not found on PATH; ensure mise tools are installed")


# Stable check IDs mapped to governing standards.
CHECK_RULES: dict[str, tuple[str, ...]] = {
    "ARCH001": ("ENG-ARCH-001", "ENG-ARCH-003", "ENG-ARCH-006"),
    "ARCH002": ("ENG-ARCH-002",),
    "ARCH003": ("ENG-ARCH-003",),
    "ARCH004": ("ENG-ARCH-001", "ENG-ARCH-003"),
    "ARCH005": ("ENG-ARCH-003",),
    "ARCH006": ("ENG-ARCH-005",),
    "ARCH007": ("ENG-ARCH-001", "ENG-SEM-002"),
    "ARCH008": ("ENG-ARCH-001",),  # unbounded channels / Arch §22.2
    "ARCH009": ("ENG-ARCH-003",),  # task-local public context / Arch §22.2
    "ARCH010": ("ENG-ARCH-003",),  # allowlist / Eng §14
    "ARCH011": ("ENG-ARCH-003",),  # feature separation / Eng §3.2
    "ARCH012": ("ENG-ARCH-001", "ENG-ARCH-003"),  # WASM graph
}

REMEDIATIONS: dict[str, str] = {
    "ARCH001": "Move I/O, async runtimes, bindings, and plugin engines out of the kernel dependency closure.",
    "ARCH002": "Keep exactly six primary ports; a seventh port requires an ADR before merge.",
    "ARCH003": (
        "Depend on runtime-owned contracts, never concrete provider/tool/store crates, from kernel/runtime core."
    ),
    "ARCH004": "Restore the fixed crate direction: kernel <- runtime <- SDK <- bindings/apps; protocol stays outward.",
    "ARCH005": "Remove central concrete provider/tool match registries from the kernel.",
    "ARCH006": "Keep host-language callbacks coarse; do not expose per-token callback APIs.",
    "ARCH007": "Keep kernel semantics deterministic; pass time and randomness as explicit inputs.",
    "ARCH008": "Replace unbounded channels with bounded queues and an explicit backpressure policy.",
    "ARCH009": "Pass request context explicitly; do not publish task-local/thread-local request context.",
    "ARCH010": "Record a complete approved exception with ADR id and allowlist entry, or remove the deviation.",
    "ARCH011": "Keep runtime defaults empty; facade pass-through only; WASM uses defaults-off + wasm-host only.",
    "ARCH012": "Ensure the browser WASM graph enables only wasm-host and contains no Tokio/native-I/O crates.",
}


@dataclass(frozen=True, order=True)
class Diagnostic:
    """One architecture failure or waived finding."""

    check_id: str
    subject: str
    message: str
    location: str = ""
    path: str = ""
    waived: bool = False

    def rules(self) -> tuple[str, ...]:
        return CHECK_RULES.get(self.check_id, ())

    def format(self) -> str:
        rules = ", ".join(self.rules()) or "—"
        status = "WAIVED" if self.waived else self.check_id
        lines = [
            f"{status} [{rules}] {self.subject}:",
            f"  {self.message}",
        ]
        if self.path:
            lines.append(f"  path: {self.path}")
        if self.location:
            lines.append(f"  at: {self.location}")
        if not self.waived:
            lines.append(f"  remediation: {REMEDIATIONS.get(self.check_id, 'See Engineering Standards §3.')}")
        return "\n".join(lines)


@dataclass
class ExceptionEntry:
    """Parsed allowlist exception."""

    raw: dict[str, Any]
    id: str
    status: str
    check_id: str
    subject: str
    adr: str
    exception_id: str
    expires_on: date | None
    expires_at_gate: str | None


@dataclass
class CheckContext:
    """Shared state for one architecture run."""

    repo_root: Path
    policy: dict[str, Any]
    allowlist: list[ExceptionEntry]
    diagnostics: list[Diagnostic] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)
    today: date = field(default_factory=date.today)

    def add(
        self,
        check_id: str,
        subject: str,
        message: str,
        *,
        location: str = "",
        path: str = "",
    ) -> None:
        waived = self._is_waived(check_id, subject)
        self.diagnostics.append(
            Diagnostic(
                check_id=check_id,
                subject=subject,
                message=message,
                location=location,
                path=path,
                waived=waived,
            )
        )

    def warn(self, message: str) -> None:
        """Record a non-failing warning that must still appear in the report."""
        self.warnings.append(message)

    def _is_waived(self, check_id: str, subject: str) -> bool:
        non_waivable = set(self.policy.get("exceptions", {}).get("non_waivable_check_ids", []))
        if check_id in non_waivable:
            return False
        for entry in self.allowlist:
            if entry.status != "Approved":
                continue
            if entry.check_id != check_id:
                continue
            if entry.subject != subject:
                continue
            if entry.expires_on is not None and entry.expires_on < self.today:
                continue
            return True
        return False

    def failing(self) -> list[Diagnostic]:
        return [d for d in self.diagnostics if not d.waived]


def load_toml(path: Path) -> dict[str, Any]:
    with path.open("rb") as handle:
        data = tomllib.load(handle)
    if not isinstance(data, dict):
        raise ValueError(f"{path}: expected a TOML table")
    return data


def parse_allowlist(path: Path, policy: Mapping[str, Any]) -> tuple[list[ExceptionEntry], list[Diagnostic]]:
    """Parse allowlist.toml and emit diagnostics for incomplete entries."""
    if not path.is_file():
        return [], [
            Diagnostic(
                check_id="ARCH010",
                subject=str(path),
                message="allowlist file is missing",
                location=str(path),
            )
        ]
    data = load_toml(path)
    required = list(policy.get("exceptions", {}).get("required_fields", []))
    entries: list[ExceptionEntry] = []
    diagnostics: list[Diagnostic] = []
    for raw in data.get("exceptions", []) or []:
        if not isinstance(raw, dict):
            diagnostics.append(
                Diagnostic(
                    check_id="ARCH010",
                    subject=str(path),
                    message="exception entry must be a table",
                    location=str(path),
                )
            )
            continue
        missing = [field for field in required if field not in raw or raw[field] in (None, "")]
        subject = str(raw.get("subject", "<missing-subject>"))
        if missing:
            diagnostics.append(
                Diagnostic(
                    check_id="ARCH010",
                    subject=subject,
                    message=f"exception missing required fields: {', '.join(missing)}",
                    location=str(path),
                )
            )
            continue
        if "*" in subject or subject.endswith("/"):
            diagnostics.append(
                Diagnostic(
                    check_id="ARCH010",
                    subject=subject,
                    message="exception subject must be exact (no wildcards)",
                    location=str(path),
                )
            )
            continue
        expires_on: date | None = None
        expires_raw = raw.get("expires_on")
        if expires_raw:
            expires_on = date.fromisoformat(str(expires_raw))
        expires_at_gate = raw.get("expires_at_gate")
        if expires_on is None and not expires_at_gate:
            diagnostics.append(
                Diagnostic(
                    check_id="ARCH010",
                    subject=subject,
                    message="exception requires expires_on or expires_at_gate",
                    location=str(path),
                )
            )
            continue
        status = str(raw["status"])
        if status != "Approved":
            diagnostics.append(
                Diagnostic(
                    check_id="ARCH010",
                    subject=subject,
                    message=f"exception status {status!r} cannot suppress checks (must be Approved)",
                    location=str(path),
                )
            )
        if expires_on is not None and expires_on < date.today():
            diagnostics.append(
                Diagnostic(
                    check_id="ARCH010",
                    subject=subject,
                    message=f"exception expired on {expires_on.isoformat()}",
                    location=str(path),
                )
            )
        entries.append(
            ExceptionEntry(
                raw=raw,
                id=str(raw["id"]),
                status=status,
                check_id=str(raw["check_id"]),
                subject=subject,
                adr=str(raw["adr"]),
                exception_id=str(raw["exception_id"]),
                expires_on=expires_on,
                expires_at_gate=str(expires_at_gate) if expires_at_gate else None,
            )
        )
    return entries, diagnostics


def validate_exception_dict(raw: Mapping[str, Any], required_fields: Sequence[str]) -> list[str]:
    """Return missing required field names for a waiver fixture dict."""
    missing: list[str] = []
    for name in required_fields:
        if name not in raw or raw[name] in (None, ""):
            missing.append(name)
    if "expires_on" not in missing and "expires_at_gate" not in missing:
        if not raw.get("expires_on") and not raw.get("expires_at_gate"):
            missing.append("expires_on|expires_at_gate")
    return missing


def run_cargo_metadata(
    manifest_path: Path,
    *,
    features: Sequence[str] | None = None,
    no_default_features: bool = False,
    filter_platform: str | None = None,
    offline: bool = False,
    locked: bool = True,
) -> dict[str, Any]:
    """Invoke cargo metadata and return parsed JSON."""
    cmd = [
        resolve_tool("cargo"),
        "metadata",
        "--format-version",
        "1",
        "--manifest-path",
        str(manifest_path),
    ]
    if locked and (manifest_path.parent / "Cargo.lock").is_file():
        cmd.append("--locked")
    if offline:
        cmd.append("--offline")
    if no_default_features:
        cmd.append("--no-default-features")
    if features:
        cmd.extend(["--features", ",".join(features)])
    if filter_platform:
        cmd.extend(["--filter-platform", filter_platform])
    proc = subprocess.run(cmd, check=False, capture_output=True, text=True)
    if proc.returncode != 0:
        raise RuntimeError(f"cargo metadata failed ({proc.returncode}): {proc.stderr.strip() or proc.stdout.strip()}")
    return json.loads(proc.stdout)


def run_cargo_tree(
    manifest_path: Path,
    package: str,
    *,
    target: str,
) -> list[tuple[int, str]]:
    """Resolve one selected package graph without workspace feature unification."""
    cmd = [
        resolve_tool("cargo"),
        "tree",
        "--manifest-path",
        str(manifest_path),
        "--package",
        package,
        "--target",
        target,
        "--no-default-features",
        "--edges",
        "normal",
        "--prefix",
        "depth",
        "--locked",
    ]
    proc = subprocess.run(cmd, check=False, capture_output=True, text=True)
    if proc.returncode != 0:
        raise RuntimeError(f"cargo tree failed ({proc.returncode}): {proc.stderr.strip() or proc.stdout.strip()}")
    resolved: list[tuple[int, str]] = []
    for line in proc.stdout.splitlines():
        match = re.match(r"^(\d+)([^ ]+)", line)
        if match:
            resolved.append((int(match.group(1)), match.group(2)))
    return resolved


def posix_rel(path: Path, root: Path) -> str:
    return path.resolve().relative_to(root.resolve()).as_posix()


def classify_package(manifest_dir: Path, repo_root: Path, policy: Mapping[str, Any]) -> str | None:
    """Classify a workspace package by its manifest directory."""
    rel = posix_rel(manifest_dir, repo_root)
    # Prefer the longest matching prefix.
    matches: list[tuple[int, str]] = []
    for role, prefixes in policy.get("classification", {}).items():
        for prefix in prefixes:
            normalized = prefix.rstrip("/")
            if rel == normalized or rel.startswith(normalized + "/"):
                matches.append((len(normalized), role))
    if not matches:
        return None
    matches.sort(reverse=True)
    return matches[0][1]


def package_id_map(metadata: Mapping[str, Any]) -> dict[str, dict[str, Any]]:
    return {pkg["id"]: pkg for pkg in metadata.get("packages", [])}


def resolve_workspace_packages(metadata: Mapping[str, Any]) -> list[dict[str, Any]]:
    members = set(metadata.get("workspace_members", []))
    return [pkg for pkg in metadata.get("packages", []) if pkg["id"] in members]


def find_package_by_name(metadata: Mapping[str, Any], name: str) -> dict[str, Any] | None:
    for pkg in metadata.get("packages", []):
        if pkg["name"] == name:
            return pkg
    return None


def resolve_node_deps(
    metadata: Mapping[str, Any],
    package_id: str,
) -> list[str]:
    for node in metadata.get("resolve", {}).get("nodes", []):
        if node["id"] == package_id:
            # Cargo's `dependencies` list flattens normal, build, and dev edges.
            # Architecture constraints describe the production consumer graph;
            # test-only dependencies are checked by their own compile/supply-chain
            # gates and must not create false production-closure violations.
            detailed = node.get("deps")
            if detailed is not None:
                return [
                    dep["pkg"]
                    for dep in detailed
                    if not dep.get("dep_kinds") or any(kind.get("kind") != "dev" for kind in dep["dep_kinds"])
                ]
            return list(node.get("dependencies", []))
    return []


def transitive_closure(metadata: Mapping[str, Any], root_id: str) -> list[str]:
    """BFS over resolve graph; returns package ids including root."""
    seen: set[str] = set()
    order: list[str] = []
    queue = [root_id]
    while queue:
        current = queue.pop(0)
        if current in seen:
            continue
        seen.add(current)
        order.append(current)
        for dep in resolve_node_deps(metadata, current):
            if dep not in seen:
                queue.append(dep)
    return order


def shortest_dep_path(
    metadata: Mapping[str, Any],
    root_id: str,
    target_id: str,
) -> list[str]:
    """Return package names along the shortest path root -> target."""
    id_map = package_id_map(metadata)
    parent: dict[str, str | None] = {root_id: None}
    queue = [root_id]
    while queue:
        current = queue.pop(0)
        if current == target_id:
            break
        for dep in resolve_node_deps(metadata, current):
            if dep not in parent:
                parent[dep] = current
                queue.append(dep)
    if target_id not in parent:
        return []
    chain: list[str] = []
    cursor: str | None = target_id
    while cursor is not None:
        chain.append(id_map[cursor]["name"])
        cursor = parent[cursor]
    chain.reverse()
    return chain


def read_manifest(path: Path) -> dict[str, Any]:
    return load_toml(path)


def check_workspace_edges(ctx: CheckContext, metadata: Mapping[str, Any]) -> None:
    allowed = ctx.policy.get("edges", {})
    id_map = package_id_map(metadata)
    members = {pkg["id"]: pkg for pkg in resolve_workspace_packages(metadata)}
    roles: dict[str, str] = {}
    for pkg_id, pkg in members.items():
        manifest_dir = Path(pkg["manifest_path"]).parent
        role = classify_package(manifest_dir, ctx.repo_root, ctx.policy)
        if role is None:
            ctx.add(
                "ARCH004",
                pkg["name"],
                f"workspace package under {posix_rel(manifest_dir, ctx.repo_root)} has no allowed classification",
                location=pkg["manifest_path"],
            )
            continue
        roles[pkg_id] = role

    for pkg_id, pkg in members.items():
        depender_role = roles.get(pkg_id)
        if depender_role is None:
            continue
        allowed_roles = set(allowed.get(depender_role, []))
        for dep_id in resolve_node_deps(metadata, pkg_id):
            if dep_id not in members:
                continue
            dep_pkg = id_map[dep_id]
            dep_role = roles.get(dep_id)
            if dep_role is None:
                continue
            if dep_role not in allowed_roles:
                path = f"{pkg['name']} ({depender_role}) -> {dep_pkg['name']} ({dep_role})"
                ctx.add(
                    "ARCH004",
                    pkg["name"],
                    f"forbidden workspace edge to {dep_pkg['name']} ({dep_role})",
                    location=pkg["manifest_path"],
                    path=path,
                )


def check_kernel_forbidden(ctx: CheckContext, metadata: Mapping[str, Any]) -> None:
    kernel_name = ctx.policy["packages"]["kernel"]
    kernel = find_package_by_name(metadata, kernel_name)
    if kernel is None:
        ctx.add("ARCH001", kernel_name, "kernel package missing from cargo metadata")
        return
    exact = set(ctx.policy.get("kernel", {}).get("forbidden_exact", {}).get("names", []))
    prefixes = list(ctx.policy.get("kernel", {}).get("forbidden_prefixes", {}).get("prefixes", []))
    id_map = package_id_map(metadata)
    for dep_id in transitive_closure(metadata, kernel["id"]):
        if dep_id == kernel["id"]:
            continue
        dep = id_map[dep_id]
        name = dep["name"]
        forbidden = name in exact or any(name.startswith(prefix) for prefix in prefixes)
        if not forbidden:
            continue
        path = " -> ".join(shortest_dep_path(metadata, kernel["id"], dep_id))
        ctx.add(
            "ARCH001",
            kernel_name,
            ("forbidden direct dependency " if path.count("->") <= 1 else "forbidden transitive dependency ")
            + f"`{name}`",
            path=path or name,
        )


def check_features(ctx: CheckContext) -> None:
    packages = ctx.policy["packages"]
    runtime_manifest = ctx.repo_root / "crates" / packages["runtime"] / "Cargo.toml"
    sdk_manifest = ctx.repo_root / "crates" / packages["sdk"] / "Cargo.toml"
    wasm_manifest = ctx.repo_root / "bindings" / packages["wasm_binding"] / "Cargo.toml"

    runtime = read_manifest(runtime_manifest)
    runtime_default = runtime.get("features", {}).get("default", [])
    if runtime_default:
        ctx.add(
            "ARCH011",
            packages["runtime"],
            f"runtime default features must be empty, found {runtime_default!r}",
            location=str(runtime_manifest),
        )
    if "native-tokio" not in runtime.get("features", {}):
        ctx.add(
            "ARCH011",
            packages["runtime"],
            "runtime missing native-tokio feature",
            location=str(runtime_manifest),
        )
    if "wasm-host" not in runtime.get("features", {}):
        ctx.add(
            "ARCH011",
            packages["runtime"],
            "runtime missing wasm-host feature",
            location=str(runtime_manifest),
        )

    sdk = read_manifest(sdk_manifest)
    expected_default = ctx.policy.get("features", {}).get("facade_default_features", ["native-tokio"])
    actual_default = sdk.get("features", {}).get("default", [])
    if actual_default != expected_default:
        ctx.add(
            "ARCH011",
            packages["sdk"],
            f"facade default features must be {expected_default!r}, found {actual_default!r}",
            location=str(sdk_manifest),
        )
    pass_through = ctx.policy.get("features", {}).get("facade_pass_through", {})
    for feature, expected in pass_through.items():
        actual = sdk.get("features", {}).get(feature)
        if actual != [expected] and actual != expected:
            ctx.add(
                "ARCH011",
                packages["sdk"],
                f"facade feature {feature!r} must pass through {expected!r}, found {actual!r}",
                location=str(sdk_manifest),
            )
    runtime_dep = sdk.get("dependencies", {}).get("finstack-ai-runtime", {})
    if isinstance(runtime_dep, dict) and runtime_dep.get("default-features", True) is not False:
        # workspace = true with default-features false is required; also accept explicit table
        if runtime_dep.get("workspace") and "default-features" not in runtime_dep:
            # Workspace dependency inherits; check workspace.dependencies in root.
            root = read_manifest(ctx.repo_root / "Cargo.toml")
            ws_dep = root.get("workspace", {}).get("dependencies", {}).get("finstack-ai-runtime", {})
            if isinstance(ws_dep, dict) and ws_dep.get("default-features") is False:
                pass
            else:
                ctx.add(
                    "ARCH011",
                    packages["sdk"],
                    "facade must depend on runtime with default-features = false",
                    location=str(sdk_manifest),
                )
        elif runtime_dep.get("default-features") is not False:
            ctx.add(
                "ARCH011",
                packages["sdk"],
                "facade must depend on runtime with default-features = false",
                location=str(sdk_manifest),
            )

    wasm = read_manifest(wasm_manifest)
    wasm_dep = wasm.get("dependencies", {}).get("finstack-ai", {})
    if not isinstance(wasm_dep, dict):
        ctx.add(
            "ARCH011",
            packages["wasm_binding"],
            "wasm binding must depend on finstack-ai with an explicit feature table",
            location=str(wasm_manifest),
        )
        return
    if wasm_dep.get("default-features", True) is not False:
        # workspace dep may omit; check workspace + package features
        root = read_manifest(ctx.repo_root / "Cargo.toml")
        ws_dep = root.get("workspace", {}).get("dependencies", {}).get("finstack-ai", {})
        defaults_disabled = wasm_dep.get("default-features") is False or (
            isinstance(ws_dep, dict) and ws_dep.get("default-features") is False
        )
        if not defaults_disabled:
            ctx.add(
                "ARCH011",
                packages["wasm_binding"],
                "wasm binding must disable facade default features",
                location=str(wasm_manifest),
            )
    features = wasm_dep.get("features", [])
    required = set(ctx.policy.get("features", {}).get("wasm_binding_required_features", ["wasm-host"]))
    if not required.issubset(set(features)):
        ctx.add(
            "ARCH011",
            packages["wasm_binding"],
            f"wasm binding must enable facade features {sorted(required)!r}, found {features!r}",
            location=str(wasm_manifest),
        )
    forbidden = set(ctx.policy.get("features", {}).get("wasm_binding_forbidden_features", []))
    # "default" is controlled by default-features = false, not features list.
    feature_forbidden = forbidden - {"default"}
    overlap = feature_forbidden.intersection(features)
    if overlap:
        ctx.add(
            "ARCH011",
            packages["wasm_binding"],
            f"wasm binding enables forbidden features {sorted(overlap)!r}",
            location=str(wasm_manifest),
        )


def check_wasm_graph(ctx: CheckContext) -> None:
    """Resolve the browser binding graph and reject Tokio/native-I/O packages.

    Metadata is rooted at the wasm binding manifest so feature selection follows
    that package (defaults off + ``wasm-host``) rather than the workspace facade
    default ``native-tokio``. Walking the whole-workspace resolve would otherwise
    unify Tokio into the graph once runtime drivers exist.
    """
    wasm_name = ctx.policy["packages"]["wasm_binding"]
    manifest = ctx.repo_root / "bindings" / wasm_name / "Cargo.toml"
    if not manifest.is_file():
        ctx.add(
            "ARCH012",
            wasm_name,
            f"wasm binding manifest missing at {posix_rel(manifest, ctx.repo_root)}",
            location=str(manifest),
        )
        return
    try:
        metadata = run_cargo_metadata(
            manifest,
            no_default_features=True,
            filter_platform="wasm32-unknown-unknown",
            offline=False,
            locked=True,
        )
    except RuntimeError as exc:
        ctx.add(
            "ARCH012",
            wasm_name,
            f"unable to resolve wasm graph from binding manifest: {exc}",
            location=str(manifest),
        )
        return

    wasm_pkg = find_package_by_name(metadata, wasm_name)
    if wasm_pkg is None:
        ctx.add("ARCH012", wasm_name, "wasm binding missing from binding-rooted metadata")
        return

    resolve = metadata.get("resolve") or {}
    root_id = resolve.get("root")
    if root_id and root_id != wasm_pkg["id"]:
        root_name = package_id_map(metadata).get(root_id, {}).get("name", root_id)
        ctx.warn(
            f"ARCH012 coverage degraded: expected resolve root {wasm_name!r}, "
            f"got {root_name!r}; walking {wasm_name} closure anyway"
        )

    exact = set(ctx.policy.get("wasm", {}).get("forbidden_exact", {}).get("names", []))
    node_ids = {n["id"] for n in resolve.get("nodes", [])}
    if wasm_pkg["id"] not in node_ids:
        ctx.warn(
            f"ARCH012 coverage degraded: {wasm_name} is absent from the "
            "wasm32 resolve graph; browser dependency scan was skipped"
        )
        return

    try:
        selected_graph = run_cargo_tree(
            manifest,
            wasm_name,
            target="wasm32-unknown-unknown",
        )
    except RuntimeError as exc:
        ctx.add(
            "ARCH012",
            wasm_name,
            f"unable to resolve selected WASM package graph: {exc}",
            location=str(manifest),
        )
        return
    stack: list[str] = []
    for depth, name in selected_graph:
        stack[depth:] = [name]
        if name in exact:
            ctx.add(
                "ARCH012",
                wasm_name,
                f"browser WASM graph contains forbidden package `{name}`",
                path=" -> ".join(stack),
            )


def strip_rust_noise(source: str) -> str:
    """Remove comments and string literals while preserving line numbers."""

    def _keep_newlines(match: re.Match[str]) -> str:
        return "\n" * match.group(0).count("\n")

    # Block comments: replace with an equal number of newlines so later findings
    # keep stable 1-based line numbers.
    source = re.sub(r"/\*.*?\*/", _keep_newlines, source, flags=re.DOTALL)
    # Line comments.
    source = re.sub(r"//.*?$", " ", source, flags=re.MULTILINE)
    # Raw strings (best-effort) then ordinary strings/chars.
    source = re.sub(r'r#+".*?"#+', _keep_newlines, source, flags=re.DOTALL)
    source = re.sub(r'r".*?"', _keep_newlines, source, flags=re.DOTALL)
    source = re.sub(r'"(?:\\.|[^"\\])*"', '""', source)
    source = re.sub(r"'(?:\\.|[^'\\])'", "''", source)
    return source


_KERNEL_IO_PATTERNS: list[tuple[re.Pattern[str], str]] = [
    (
        re.compile(r"\bstd::(?:fs|net|process|env|time)::"),
        "kernel uses std I/O or ambient time/env",
    ),
    (
        re.compile(r"\bstd::(?:fs|net|process|env|time)\b"),
        "kernel uses std I/O or ambient time/env",
    ),
    (
        re.compile(r"\b(?:tokio|async_std|reqwest|hyper|rusqlite|sqlx|pyo3|wasm_bindgen|wasmtime)::"),
        "kernel imports forbidden host crate",
    ),
    (
        re.compile(r"\brand::thread_rng\b|\bStdRng\b|\bSystemTime\b|\bInstant::now\b"),
        "kernel uses ambient time/randomness",
    ),
]

_UNBOUNDED_PATTERNS: list[tuple[re.Pattern[str], str]] = [
    (re.compile(r"\bunbounded_channel\s*!?\s*\("), "unbounded_channel constructor"),
    (re.compile(r"\bmpsc::unbounded\s*\("), "mpsc::unbounded constructor"),
    (re.compile(r"\basync_channel::unbounded\s*\("), "async_channel::unbounded constructor"),
    (
        re.compile(r"\bcrossbeam_channel::unbounded\s*\("),
        "crossbeam_channel::unbounded constructor",
    ),
]

_TASK_LOCAL_PATTERNS: list[tuple[re.Pattern[str], str]] = [
    (re.compile(r"\btask_local!\s*\{"), "tokio::task_local! / task_local! macro"),
    (re.compile(r"\bthread_local!\s*\{"), "thread_local! macro in public contracts"),
]

_CONCRETE_IMPORT_PATTERNS: list[tuple[re.Pattern[str], str]] = [
    (
        re.compile(r"\b(?:use|extern\s+crate)\s+finstack_ai_(?:provider|tools|store|observer|plugin)_[A-Za-z0-9_]+"),
        "concrete extension/plugin import",
    ),
    (
        re.compile(r"\b(?:use|extern\s+crate)\s+extensions::"),
        "concrete extensions path import",
    ),
]

_REGISTRY_PATTERNS: list[tuple[re.Pattern[str], str]] = [
    (
        re.compile(
            r"\bmatch\s+[A-Za-z0-9_:]+\s*\{[^}]{0,400}\b(?:OpenAI|Anthropic|ProviderKind|ToolKind)\b",
            re.DOTALL,
        ),
        "central concrete provider/tool match registry",
    ),
]


def iter_rust_sources(ctx: CheckContext) -> Iterator[Path]:
    include_roots = ctx.policy.get("source", {}).get("roots", {}).get("include", [])
    exclude_roots = ctx.policy.get("source", {}).get("roots", {}).get("exclude", [])
    for include in include_roots:
        root = ctx.repo_root / include
        if not root.exists():
            continue
        for path in root.rglob("*.rs"):
            rel = posix_rel(path, ctx.repo_root)
            if any(rel == ex.rstrip("/") or rel.startswith(ex.rstrip("/") + "/") for ex in exclude_roots):
                continue
            if "/target/" in f"/{rel}/":
                continue
            yield path


def scan_file(path: Path, patterns: Sequence[tuple[re.Pattern[str], str]]) -> list[tuple[int, str, str]]:
    text = path.read_text(encoding="utf-8")
    cleaned = strip_rust_noise(text)
    # Map cleaned offsets approximately by scanning original lines.
    findings: list[tuple[int, str, str]] = []
    for line_no, line in enumerate(cleaned.splitlines(), start=1):
        for pattern, label in patterns:
            if pattern.search(line):
                findings.append((line_no, label, line.strip()[:120]))
    return findings


def check_sources(ctx: CheckContext) -> None:
    packages = ctx.policy["packages"]
    kernel_root = f"crates/{packages['kernel']}/"
    runtime_root = f"crates/{packages['runtime']}/"
    sdk_root = f"crates/{packages['sdk']}/"
    contract_roots = (kernel_root, runtime_root, sdk_root)

    for path in iter_rust_sources(ctx):
        rel = posix_rel(path, ctx.repo_root)
        if rel.startswith(kernel_root):
            for line_no, label, snippet in scan_file(path, _KERNEL_IO_PATTERNS):
                check_id = "ARCH007" if "time" in label or "random" in label else "ARCH001"
                ctx.add(
                    check_id,
                    packages["kernel"],
                    f"{label}: {snippet}",
                    location=f"{rel}:{line_no}",
                )
            for line_no, label, snippet in scan_file(path, _REGISTRY_PATTERNS):
                ctx.add(
                    "ARCH005",
                    packages["kernel"],
                    f"{label}: {snippet}",
                    location=f"{rel}:{line_no}",
                )

        if rel.startswith(kernel_root) or rel.startswith(runtime_root):
            for line_no, label, snippet in scan_file(path, _CONCRETE_IMPORT_PATTERNS):
                ctx.add(
                    "ARCH003",
                    Path(rel).parts[1] if len(Path(rel).parts) > 1 else rel,
                    f"{label}: {snippet}",
                    location=f"{rel}:{line_no}",
                )

        if any(rel.startswith(root) for root in contract_roots):
            for line_no, label, snippet in scan_file(path, _UNBOUNDED_PATTERNS):
                ctx.add(
                    "ARCH008",
                    rel,
                    f"{label}: {snippet}",
                    location=f"{rel}:{line_no}",
                )
            # task-local only in public contract modules (lib.rs / ports / public mod roots)
            name = Path(rel).name
            if name in {"lib.rs", "mod.rs"} or "/ports/" in rel or rel.endswith("/ports.rs"):
                for line_no, label, snippet in scan_file(path, _TASK_LOCAL_PATTERNS):
                    ctx.add(
                        "ARCH009",
                        rel,
                        f"{label}: {snippet}",
                        location=f"{rel}:{line_no}",
                    )


def check_allowlist_adrs(ctx: CheckContext) -> None:
    """Ensure allowlist ADR references match accepted ADR index rows."""
    adr_register = ctx.repo_root / "docs" / "implementation" / "adr-register.md"
    text = adr_register.read_text(encoding="utf-8") if adr_register.is_file() else ""
    for entry in ctx.allowlist:
        if not entry.adr.startswith("ADR-"):
            ctx.add(
                "ARCH010",
                entry.subject,
                f"allowlist entry {entry.id} ADR must look like ADR-NNN",
                location=str(DEFAULT_ALLOWLIST),
            )
            continue
        # Require a table cell (`| ADR-NNN |`), not a prose mention.
        if not re.search(rf"\|\s*{re.escape(entry.adr)}\s*\|", text):
            ctx.add(
                "ARCH010",
                entry.subject,
                f"allowlist entry {entry.id} references unknown ADR {entry.adr}",
                location=str(DEFAULT_ALLOWLIST),
            )


def run_checks(
    repo_root: Path,
    *,
    policy_path: Path = DEFAULT_POLICY,
    allowlist_path: Path = DEFAULT_ALLOWLIST,
    metadata: Mapping[str, Any] | None = None,
    skip_wasm_graph: bool = False,
) -> CheckContext:
    policy = load_toml(policy_path)
    allowlist, allowlist_diags = parse_allowlist(allowlist_path, policy)
    ctx = CheckContext(repo_root=repo_root, policy=policy, allowlist=allowlist)
    ctx.diagnostics.extend(allowlist_diags)
    check_allowlist_adrs(ctx)

    if metadata is None:
        metadata = run_cargo_metadata(repo_root / "Cargo.toml", locked=True)
    check_workspace_edges(ctx, metadata)
    check_kernel_forbidden(ctx, metadata)
    check_features(ctx)
    if not skip_wasm_graph:
        check_wasm_graph(ctx)
    check_sources(ctx)
    ctx.diagnostics.sort()
    return ctx


def format_report(ctx: CheckContext, *, elapsed_ms: float) -> str:
    lines = [
        "finstack-ai architecture check",
        f"repo: {ctx.repo_root}",
        f"elapsed_ms: {elapsed_ms:.1f}",
        "",
    ]
    for warning in ctx.warnings:
        lines.append(f"WARN: {warning}")
        lines.append("")
    if not ctx.diagnostics:
        if ctx.warnings:
            lines.append(f"PASS: no architecture findings, {len(ctx.warnings)} warning(s)")
        else:
            lines.append("PASS: no architecture findings")
        return "\n".join(lines) + "\n"
    for diag in ctx.diagnostics:
        lines.append(diag.format())
        lines.append("")
    failing = ctx.failing()
    waived = [d for d in ctx.diagnostics if d.waived]
    if failing:
        lines.append(f"FAIL: {len(failing)} finding(s), {len(waived)} waived, {len(ctx.warnings)} warning(s)")
    else:
        lines.append(f"PASS: 0 failing, {len(waived)} waived, {len(ctx.warnings)} warning(s)")
    return "\n".join(lines) + "\n"


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo-root", type=Path, default=REPO_ROOT)
    parser.add_argument("--policy", type=Path, default=DEFAULT_POLICY)
    parser.add_argument("--allowlist", type=Path, default=DEFAULT_ALLOWLIST)
    parser.add_argument("--json", action="store_true", help="Emit diagnostics as JSON")
    parser.add_argument("--skip-wasm-graph", action="store_true")
    args = parser.parse_args(argv)

    started = time.perf_counter()
    try:
        ctx = run_checks(
            args.repo_root.resolve(),
            policy_path=args.policy.resolve(),
            allowlist_path=args.allowlist.resolve(),
            skip_wasm_graph=args.skip_wasm_graph,
        )
    except Exception as exc:  # noqa: BLE001 - top-level CLI boundary
        print(f"architecture check error: {exc}", file=sys.stderr)
        return 2
    elapsed_ms = (time.perf_counter() - started) * 1000.0

    if args.json:
        payload = {
            "elapsed_ms": elapsed_ms,
            "diagnostics": [
                {
                    "check_id": d.check_id,
                    "rules": list(d.rules()),
                    "subject": d.subject,
                    "message": d.message,
                    "location": d.location,
                    "path": d.path,
                    "waived": d.waived,
                }
                for d in ctx.diagnostics
            ],
            "warnings": list(ctx.warnings),
            "failed": len(ctx.failing()),
        }
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        sys.stdout.write(format_report(ctx, elapsed_ms=elapsed_ms))

    return 1 if ctx.failing() else 0


if __name__ == "__main__":
    raise SystemExit(main())
