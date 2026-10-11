"""Staged Rust starter manifests stay inside the workspace dependency table."""

from __future__ import annotations

import importlib.util
import sys
import tomllib
from pathlib import Path

_REPO = Path(__file__).resolve().parents[2]
_SPEC = importlib.util.spec_from_file_location(
    "starter_rc",
    Path(__file__).resolve().parent / "starter_rc.py",
)
if _SPEC is None or _SPEC.loader is None:
    raise RuntimeError("starter_rc module is missing")
_MODULE = importlib.util.module_from_spec(_SPEC)
sys.modules[_SPEC.name] = _MODULE
_SPEC.loader.exec_module(_MODULE)
dependencies_block = _MODULE.dependencies_block


def test_rc_examples_only_inherit_declared_workspace_dependencies() -> None:
    root = tomllib.loads((_REPO / "Cargo.toml").read_text(encoding="utf-8"))
    declared = set(root["workspace"]["dependencies"])
    examples = (
        _REPO / "examples" / "rust-minimal" / "Cargo.toml",
        _REPO / "examples" / "durable-interaction" / "Cargo.toml",
    )
    for manifest in examples:
        block = dependencies_block(manifest)
        assert "[lints]" not in block
        assert "finstack-ai-context-memory" not in block
        table = tomllib.loads(block)["dependencies"]
        for name, spec in table.items():
            if isinstance(spec, dict) and spec.get("workspace") is True:
                assert name in declared, f"{manifest.name} depends on undeclared {name}"
    minimal = dependencies_block(examples[0])
    assert "finstack-ai-memory" in minimal
    assert "finstack-ai-kernel" in minimal
    durable = dependencies_block(examples[1])
    assert "finstack-ai-workflow-local" in durable


def test_dependencies_block_keeps_a_table_that_ends_the_file(tmp_path: Path) -> None:
    manifest = tmp_path / "Cargo.toml"
    manifest.write_text(
        '[package]\nname = "example"\n\n[dependencies]\nfoo = { workspace = true }\n',
        encoding="utf-8",
    )
    block = dependencies_block(manifest)
    assert block.startswith("[dependencies]\n")
    assert "foo = { workspace = true }" in block
    assert "[package]" not in block


def test_dependencies_block_stops_at_the_next_section(tmp_path: Path) -> None:
    manifest = tmp_path / "Cargo.toml"
    manifest.write_text(
        "[dependencies]\nfoo = { workspace = true }\n\n[lints]\nworkspace = true\n",
        encoding="utf-8",
    )
    block = dependencies_block(manifest)
    assert "foo = { workspace = true }" in block
    assert "[lints]" not in block
