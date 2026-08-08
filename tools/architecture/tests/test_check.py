"""Unit tests for architecture policy enforcement."""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
TOOL_DIR = REPO_ROOT / "tools" / "architecture"
sys.path.insert(0, str(TOOL_DIR.parent))

from architecture.check import (  # noqa: E402
    CHECK_RULES,
    Diagnostic,
    check_kernel_forbidden,
    check_workspace_edges,
    classify_package,
    load_toml,
    parse_allowlist,
    run_checks,
    shortest_dep_path,
    strip_rust_noise,
    validate_exception_dict,
)


def _pkg(name: str, pkg_id: str, manifest: str, deps: list[str] | None = None) -> dict:
    return {
        "name": name,
        "id": pkg_id,
        "manifest_path": manifest,
        "dependencies": [{"name": d, "kind": None} for d in (deps or [])],
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


class ClassifyTests(unittest.TestCase):
    def setUp(self) -> None:
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
            with self.subTest(rel=rel):
                self.assertEqual(
                    classify_package(REPO_ROOT / rel, REPO_ROOT, self.policy),
                    role,
                )


class ForbiddenCaseFileTests(unittest.TestCase):
    def test_cases_toml_lists_acceptance_crates(self) -> None:
        cases = load_toml(REPO_ROOT / "fixtures/architecture/forbidden-kernel/cases.toml")
        names = {case["name"] for case in cases["cases"]}
        self.assertEqual(names, {"tokio", "reqwest", "rusqlite", "pyo3", "wasmtime"})
        self.assertTrue(any(case.get("kind") == "transitive" for case in cases["cases"]))


class KernelForbiddenTests(unittest.TestCase):
    def setUp(self) -> None:
        from architecture.check import CheckContext

        self.policy = load_toml(TOOL_DIR / "policy.toml")
        self.ctx = CheckContext(repo_root=REPO_ROOT, policy=self.policy, allowlist=[])

    def test_detects_direct_tokio(self) -> None:
        kernel = _pkg(
            "finstack-ai-kernel",
            "kernel-id",
            str(REPO_ROOT / "crates/finstack-ai-kernel/Cargo.toml"),
        )
        tokio = _pkg("tokio", "tokio-id", "/virtual/tokio/Cargo.toml")
        meta = _metadata(
            [kernel, tokio],
            {"kernel-id": ["tokio-id"], "tokio-id": []},
            ["kernel-id"],
        )
        check_kernel_forbidden(self.ctx, meta)
        failing = self.ctx.failing()
        self.assertTrue(any(d.check_id == "ARCH001" and "tokio" in d.message for d in failing))
        self.assertIn("ENG-ARCH-001", CHECK_RULES["ARCH001"])

    def test_detects_transitive_reqwest(self) -> None:
        kernel = _pkg(
            "finstack-ai-kernel",
            "kernel-id",
            str(REPO_ROOT / "crates/finstack-ai-kernel/Cargo.toml"),
        )
        bridge = _pkg("bridge", "bridge-id", "/virtual/bridge/Cargo.toml")
        reqwest = _pkg("reqwest", "reqwest-id", "/virtual/reqwest/Cargo.toml")
        meta = _metadata(
            [kernel, bridge, reqwest],
            {
                "kernel-id": ["bridge-id"],
                "bridge-id": ["reqwest-id"],
                "reqwest-id": [],
            },
            ["kernel-id"],
        )
        check_kernel_forbidden(self.ctx, meta)
        paths = [d.path for d in self.ctx.failing() if d.check_id == "ARCH001"]
        self.assertTrue(any("reqwest" in p for p in paths))
        self.assertEqual(
            shortest_dep_path(meta, "kernel-id", "reqwest-id"),
            ["finstack-ai-kernel", "bridge", "reqwest"],
        )

    def test_parameterized_forbidden_names(self) -> None:
        for name in ("tokio", "reqwest", "rusqlite", "pyo3", "wasmtime"):
            with self.subTest(name=name):
                ctx = type(self.ctx)(repo_root=REPO_ROOT, policy=self.policy, allowlist=[])
                kernel = _pkg(
                    "finstack-ai-kernel",
                    "kernel-id",
                    str(REPO_ROOT / "crates/finstack-ai-kernel/Cargo.toml"),
                )
                bad = _pkg(name, f"{name}-id", f"/virtual/{name}/Cargo.toml")
                meta = _metadata(
                    [kernel, bad],
                    {"kernel-id": [f"{name}-id"], f"{name}-id": []},
                    ["kernel-id"],
                )
                check_kernel_forbidden(ctx, meta)
                self.assertTrue(ctx.failing(), msg=f"expected failure for {name}")


class EdgeTests(unittest.TestCase):
    def test_rejects_runtime_to_protocol(self) -> None:
        from architecture.check import CheckContext

        policy = load_toml(TOOL_DIR / "policy.toml")
        ctx = CheckContext(repo_root=REPO_ROOT, policy=policy, allowlist=[])
        runtime = _pkg(
            "finstack-ai-runtime",
            "runtime-id",
            str(REPO_ROOT / "crates/finstack-ai-runtime/Cargo.toml"),
        )
        protocol = _pkg(
            "finstack-ai-protocol",
            "protocol-id",
            str(REPO_ROOT / "crates/finstack-ai-protocol/Cargo.toml"),
        )
        meta = _metadata(
            [runtime, protocol],
            {"runtime-id": ["protocol-id"], "protocol-id": []},
            ["runtime-id", "protocol-id"],
        )
        check_workspace_edges(ctx, meta)
        self.assertTrue(any(d.check_id == "ARCH004" for d in ctx.failing()))


class SourceStripTests(unittest.TestCase):
    def test_strips_comments_and_strings(self) -> None:
        src = 'use foo;\n// unbounded_channel(\n/* mpsc::unbounded( */\nlet s = "unbounded_channel(";\n'
        cleaned = strip_rust_noise(src)
        self.assertNotIn("unbounded_channel(", cleaned)


class WaiverTests(unittest.TestCase):
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
        self.assertEqual(validate_exception_dict(valid, required), [])
        for field in required:
            broken = dict(valid)
            broken.pop(field)
            missing = validate_exception_dict(broken, required)
            self.assertTrue(missing, msg=f"expected missing {field}")

    def test_parse_allowlist_rejects_incomplete(self) -> None:
        policy = load_toml(TOOL_DIR / "policy.toml")
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "allowlist.toml"
            path.write_text(
                """
[[exceptions]]
id = "ALW-bad"
status = "Approved"
check_id = "ARCH008"
subject = "x"
""",
                encoding="utf-8",
            )
            _entries, diags = parse_allowlist(path, policy)
            self.assertTrue(any(d.check_id == "ARCH010" for d in diags))


class RealWorkspaceTests(unittest.TestCase):
    def test_current_workspace_passes(self) -> None:
        # WASM binding must enable wasm-host before this passes.
        wasm_toml = (REPO_ROOT / "bindings/finstack-ai-wasm/Cargo.toml").read_text(encoding="utf-8")
        if 'features = ["wasm-host"]' not in wasm_toml:
            self.skipTest("wasm-host wiring not applied yet")
        ctx = run_checks(REPO_ROOT, skip_wasm_graph=False)
        failing = ctx.failing()
        self.assertEqual(failing, [], msg="\n".join(d.format() for d in failing))
        self.assertEqual(ctx.warnings, [])


class WasmGraphTests(unittest.TestCase):
    def test_missing_resolve_node_emits_warning_not_failure(self) -> None:
        from architecture.check import CheckContext, check_wasm_graph

        policy = load_toml(TOOL_DIR / "policy.toml")
        ctx = CheckContext(repo_root=REPO_ROOT, policy=policy, allowlist=[])
        # Force the skip path by monkeypatching metadata to omit the wasm node.
        import architecture.check as check_mod

        original = check_mod.run_cargo_metadata

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

        check_mod.run_cargo_metadata = fake_metadata  # type: ignore[assignment]
        try:
            check_wasm_graph(ctx)
        finally:
            check_mod.run_cargo_metadata = original  # type: ignore[assignment]
        self.assertEqual(ctx.failing(), [])
        self.assertTrue(any("coverage degraded" in w for w in ctx.warnings))


class DiagnosticFormatTests(unittest.TestCase):
    def test_includes_rules_and_remediation(self) -> None:
        diag = Diagnostic(
            check_id="ARCH001",
            subject="finstack-ai-kernel",
            message="forbidden transitive dependency `tokio`",
            path="finstack-ai-kernel -> bridge -> tokio",
        )
        text = diag.format()
        self.assertIn("ENG-ARCH-001", text)
        self.assertIn("remediation:", text)
        self.assertIn("tokio", text)


if __name__ == "__main__":
    unittest.main()
