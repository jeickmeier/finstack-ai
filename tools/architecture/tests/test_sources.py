"""Source-guard unit tests."""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
TOOL_DIR = REPO_ROOT / "tools" / "architecture"
sys.path.insert(0, str(TOOL_DIR.parent))

from architecture.check import (  # noqa: E402
    _KERNEL_IO_PATTERNS,
    _TASK_LOCAL_PATTERNS,
    _UNBOUNDED_PATTERNS,
    CheckContext,
    check_sources,
    load_toml,
    scan_file,
    strip_rust_noise,
)


class SourceGuardTests(unittest.TestCase):
    def test_detects_unbounded_channel(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "lib.rs"
            path.write_text("fn f() { let _ = unbounded_channel(); }\n", encoding="utf-8")
            findings = scan_file(path, _UNBOUNDED_PATTERNS)
            self.assertTrue(findings)

    def test_detects_task_local(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "lib.rs"
            path.write_text("task_local! { static X: u8; }\n", encoding="utf-8")
            findings = scan_file(path, _TASK_LOCAL_PATTERNS)
            self.assertTrue(findings)

    def test_detects_kernel_io(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "lib.rs"
            path.write_text("use std::fs::File;\n", encoding="utf-8")
            findings = scan_file(path, _KERNEL_IO_PATTERNS)
            self.assertTrue(findings)

    def test_ignores_commented_unbounded(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "lib.rs"
            path.write_text("// let _ = unbounded_channel();\nfn ok() {}\n", encoding="utf-8")
            findings = scan_file(path, _UNBOUNDED_PATTERNS)
            self.assertFalse(findings)

    def test_block_comment_preserves_line_numbers(self) -> None:
        src = "fn a() {}\n/*\nline2\nline3\n*/\nfn bad() { let _ = unbounded_channel(); }\n"
        cleaned = strip_rust_noise(src)
        self.assertEqual(cleaned.count("\n"), src.count("\n"))
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "lib.rs"
            path.write_text(src, encoding="utf-8")
            findings = scan_file(path, _UNBOUNDED_PATTERNS)
            self.assertEqual(len(findings), 1)
            self.assertEqual(findings[0][0], 6)

    def test_production_tree_has_no_source_findings(self) -> None:
        policy = load_toml(TOOL_DIR / "policy.toml")
        ctx = CheckContext(repo_root=REPO_ROOT, policy=policy, allowlist=[])
        check_sources(ctx)
        source_ids = {"ARCH001", "ARCH003", "ARCH005", "ARCH007", "ARCH008", "ARCH009"}
        failing = [d for d in ctx.failing() if d.check_id in source_ids]
        self.assertEqual(failing, [], msg="\n".join(d.format() for d in failing))


if __name__ == "__main__":
    unittest.main()
