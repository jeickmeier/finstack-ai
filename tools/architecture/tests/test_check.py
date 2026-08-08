"""Unit tests for architecture policy enforcement."""

from __future__ import annotations

import tempfile
from pathlib import Path
from unittest import mock

import architecture.check as check_mod
from architecture.check import (
    CHECK_RULES,
    CheckContext,
    Diagnostic,
    check_kernel_forbidden,
    check_wasm_graph,
    check_workspace_edges,
    classify_package,
    load_toml,
    parse_allowlist,
    run_checks,
    shortest_dep_path,
    strip_rust_noise,
    validate_exception_dict,
)

REPO_ROOT = Path(__file__).resolve().parents[3]
TOOL_DIR = REPO_ROOT / "tools" / "architecture"


def _pkg(name: str, pkg_id: str, manifest: str, deps: list[str] | None = None) -> dict:
    return {
        "name": name,
        "id": pkg_id,
        "manifest_path": manifest,
        "dependencies": [{"name": d, "kind": None} for d in deps or []],
    }


def _metadata(packages: list[dict], edges: dict[str, list[str]], members: list[str]) -> dict:
    return {
        "packages": packages,
        "workspace_members": members,
        "resolve": {
            "nodes": [{"id": pkg_id, "dependencies": deps} for pkg_id, deps in edges.items()],
            "root": members[0] if members else None,
        },
    }


class ClassifyTests:
    def setup_method(self) -> None:
        self.policy = load_toml(TOOL_DIR / "policy.toml")

    def test_core_roles(self) -> None:
        cases = {
            "crates/finstack-ai-kernel": "kernel",
            "crates/finstack-ai-runtime": "runtime",
            "crates/finstack-ai": "sdk",
            "crates/finstack-ai-protocol": "protocol",
            "crates/finstack-ai-test": "test",
            "bindings/finstack-ai-wasm": "binding",
            "extensions/providers/foo": "provider",
            "extensions/stores/bar": "store",
        }
        for rel, role in cases.items():
            assert classify_package(REPO_ROOT / rel, REPO_ROOT, self.policy) == role


class ForbiddenCaseFileTests:
    def test_cases_toml_lists_acceptance_crates(self) -> None:
        cases = load_toml(REPO_ROOT / "fixtures/architecture/forbidden-kernel/cases.toml")
        names = {case["name"] for case in cases["cases"]}
        assert names == {"tokio", "reqwest", "rusqlite", "pyo3", "wasmtime"}
        assert any(case.get("kind") == "transitive" for case in cases["cases"])


class KernelForbiddenTests:
    def setup_method(self) -> None:
        self.policy = load_toml(TOOL_DIR / "policy.toml")
        self.ctx = CheckContext(repo_root=REPO_ROOT, policy=self.policy, allowlist=[])

    def test_detects_direct_tokio(self) -> None:
        kernel = _pkg("finstack-ai-kernel", "kernel-id", str(REPO_ROOT / "crates/finstack-ai-kernel/Cargo.toml"))
        tokio = _pkg("tokio", "tokio-id", "/virtual/tokio/Cargo.toml")
        meta = _metadata([kernel, tokio], {"kernel-id": ["tokio-id"], "tokio-id": []}, ["kernel-id"])
        check_kernel_forbidden(self.ctx, meta)
        failing = self.ctx.failing()
        assert any(d.check_id == "ARCH001" and "tokio" in d.message for d in failing)
        assert "ENG-ARCH-001" in CHECK_RULES["ARCH001"]

    def test_detects_transitive_reqwest(self) -> None:
        kernel = _pkg("finstack-ai-kernel", "kernel-id", str(REPO_ROOT / "crates/finstack-ai-kernel/Cargo.toml"))
        bridge = _pkg("bridge", "bridge-id", "/virtual/bridge/Cargo.toml")
        reqwest = _pkg("reqwest", "reqwest-id", "/virtual/reqwest/Cargo.toml")
        meta = _metadata(
            [kernel, bridge, reqwest],
            {"kernel-id": ["bridge-id"], "bridge-id": ["reqwest-id"], "reqwest-id": []},
            ["kernel-id"],
        )
        check_kernel_forbidden(self.ctx, meta)
        paths = [d.path for d in self.ctx.failing() if d.check_id == "ARCH001"]
        assert any("reqwest" in p for p in paths)
        assert shortest_dep_path(meta, "kernel-id", "reqwest-id") == ["finstack-ai-kernel", "bridge", "reqwest"]

    def test_parameterized_forbidden_names(self) -> None:
        for name in ("tokio", "reqwest", "rusqlite", "pyo3", "wasmtime"):
            ctx = type(self.ctx)(repo_root=REPO_ROOT, policy=self.policy, allowlist=[])
            kernel = _pkg("finstack-ai-kernel", "kernel-id", str(REPO_ROOT / "crates/finstack-ai-kernel/Cargo.toml"))
            bad = _pkg(name, f"{name}-id", f"/virtual/{name}/Cargo.toml")
            meta = _metadata([kernel, bad], {"kernel-id": [f"{name}-id"], f"{name}-id": []}, ["kernel-id"])
            check_kernel_forbidden(ctx, meta)
            assert ctx.failing(), f"expected failure for {name}"


class EdgeTests:
    def test_rejects_runtime_to_protocol(self) -> None:
        policy = load_toml(TOOL_DIR / "policy.toml")
        ctx = CheckContext(repo_root=REPO_ROOT, policy=policy, allowlist=[])
        runtime = _pkg("finstack-ai-runtime", "runtime-id", str(REPO_ROOT / "crates/finstack-ai-runtime/Cargo.toml"))
        protocol = _pkg(
            "finstack-ai-protocol", "protocol-id", str(REPO_ROOT / "crates/finstack-ai-protocol/Cargo.toml")
        )
        meta = _metadata(
            [runtime, protocol], {"runtime-id": ["protocol-id"], "protocol-id": []}, ["runtime-id", "protocol-id"]
        )
        check_workspace_edges(ctx, meta)
        assert any(d.check_id == "ARCH004" for d in ctx.failing())


class SourceStripTests:
    def test_strips_comments_and_strings(self) -> None:
        src = 'use foo;\n// unbounded_channel(\n/* mpsc::unbounded( */\nlet s = "unbounded_channel(";\n'
        cleaned = strip_rust_noise(src)
        assert "unbounded_channel(" not in cleaned


class WaiverTests:
    def test_required_fields(self) -> None:
        policy = load_toml(TOOL_DIR / "policy.toml")
        required = policy["exceptions"]["required_fields"]
        valid = {
            "id": "ALW-test",
            "status": "Approved",
            "check_id": "ARCH008",
            "rule": "ENG-ARCH-001",
            "subject": "crates/example/src/lib.rs",
            "adr": "ADR-001",
            "exception_id": "EX-test-aaaaaaaaaaaa",
            "scope": "PR-002 test",
            "reason": "fixture",
            "risk": "none",
            "compensating_control": "tests",
            "owner": "me@jeickmeier.com",
            "approver": "me@jeickmeier.com",
            "created_on": "2026-08-08",
            "expires_on": "2027-01-01",
            "removal_issue": "issue-1",
            "public_compatibility_affected": "No",
            "security_affected": "No",
            "evidence": "fixture",
        }
        assert validate_exception_dict(valid, required) == []
        for field in required:
            broken = dict(valid)
            broken.pop(field)
            missing = validate_exception_dict(broken, required)
            assert missing, f"expected missing {field}"

    def test_parse_allowlist_rejects_incomplete(self) -> None:
        policy = load_toml(TOOL_DIR / "policy.toml")
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "allowlist.toml"
            path.write_text(
                '\n[[exceptions]]\nid = "ALW-bad"\nstatus = "Approved"\ncheck_id = "ARCH008"\nsubject = "x"\n',
                encoding="utf-8",
            )
            _entries, diags = parse_allowlist(path, policy)
            assert any(d.check_id == "ARCH010" for d in diags)


class RealWorkspaceTests:
    def test_current_workspace_passes(self) -> None:
        wasm_toml = (REPO_ROOT / "bindings/finstack-ai-wasm/Cargo.toml").read_text(encoding="utf-8")
        if 'features = ["wasm-host"]' not in wasm_toml:
            self.skipTest("wasm-host wiring not applied yet")
        ctx = run_checks(REPO_ROOT, skip_wasm_graph=False)
        failing = ctx.failing()
        assert failing == [], "\n".join(d.format() for d in failing)
        assert ctx.warnings == []


class WasmGraphTests:
    def test_missing_resolve_node_emits_warning_not_failure(self) -> None:
        policy = load_toml(TOOL_DIR / "policy.toml")
        ctx = CheckContext(repo_root=REPO_ROOT, policy=policy, allowlist=[])

        def fake_metadata(*_args, **_kwargs):
            wasm_manifest = str((REPO_ROOT / "bindings/finstack-ai-wasm/Cargo.toml").resolve())
            return {
                "packages": [
                    {
                        "name": "finstack-ai-wasm",
                        "id": "wasm-id",
                        "manifest_path": wasm_manifest,
                        "dependencies": [],
                    }
                ],
                "workspace_members": ["wasm-id"],
                "resolve": {"root": "wasm-id", "nodes": []},
            }

        with mock.patch.object(check_mod, "run_cargo_metadata", side_effect=fake_metadata):
            check_wasm_graph(ctx)
        assert ctx.failing() == []
        assert any("coverage degraded" in w for w in ctx.warnings)


class DiagnosticFormatTests:
    def test_includes_rules_and_remediation(self) -> None:
        diag = Diagnostic(
            check_id="ARCH001",
            subject="finstack-ai-kernel",
            message="forbidden transitive dependency `tokio`",
            path="finstack-ai-kernel -> bridge -> tokio",
        )
        text = diag.format()
        assert "ENG-ARCH-001" in text
        assert "remediation:" in text
        assert "tokio" in text
