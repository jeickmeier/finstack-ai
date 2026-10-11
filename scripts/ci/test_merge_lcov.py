"""Merge behavior for the Rust and WASM LCOV reports imported by SonarQube."""

from __future__ import annotations

import importlib.util
import shutil
import sys
from pathlib import Path

_SPEC = importlib.util.spec_from_file_location(
    "merge_lcov",
    Path(__file__).resolve().parent / "merge_lcov.py",
)
if _SPEC is None or _SPEC.loader is None:
    raise RuntimeError("merge_lcov module is missing")
_MODULE = importlib.util.module_from_spec(_SPEC)
sys.modules[_SPEC.name] = _MODULE
_SPEC.loader.exec_module(_MODULE)
merge_reports = _MODULE.merge_reports


def test_merge_sums_overlapping_hits_and_keeps_either_side() -> None:
    root = Path("/repo")
    native = """\
SF:/repo/crates/kernel/src/lib.rs
FN:1,parse
FNDA:2,parse
DA:1,2
DA:2,0
BRDA:4,0,0,1
BRDA:4,0,1,-
end_of_record
"""
    wasm = """\
SF:/repo/crates/kernel/src/lib.rs
FN:1,parse
FNDA:1,parse
DA:2,3
DA:3,1
BRDA:4,0,1,4
end_of_record
SF:/repo/crates/kernel/src/wasm.rs
DA:9,1
end_of_record
"""

    rendered = merge_reports([native, wasm], root)

    assert "SF:crates/kernel/src/lib.rs" in rendered
    assert "SF:crates/kernel/src/wasm.rs" in rendered
    assert "FNDA:3,parse" in rendered
    assert "DA:1,2" in rendered
    assert "DA:2,3" in rendered
    assert "DA:3,1" in rendered
    assert "BRDA:4,0,0,1" in rendered
    assert "BRDA:4,0,1,4" in rendered
    assert "/repo/" not in rendered


def test_merge_drops_paths_outside_the_repository() -> None:
    root = Path("/repo")
    report = """\
SF:/rustc/library/core/src/lib.rs
DA:1,1
end_of_record
SF:/repo/crates/kernel/src/lib.rs
DA:8,1
end_of_record
"""

    rendered = merge_reports([report], root)

    assert "SF:crates/kernel/src/lib.rs" in rendered
    assert "rustc" not in rendered


def test_main_writes_one_merged_report(tmp_path: Path) -> None:
    root = tmp_path / "repo"
    source = root / "crates" / "kernel" / "src"
    source.mkdir(parents=True)
    jail = _MODULE.REPO_ROOT / "target" / "lcov-merge-test"
    report = jail / "native.info"
    output = jail / "out" / "lcov.info"
    shutil.rmtree(jail, ignore_errors=True)
    jail.mkdir(parents=True)
    report.write_text(
        f"SF:{source / 'lib.rs'}\nDA:1,1\nend_of_record\n",
        encoding="utf-8",
    )
    try:
        assert (
            _MODULE.main([str(report), "--output", str(output), "--root", str(root)])
            == 0
        )
        rendered = output.read_text(encoding="utf-8")
        assert "SF:crates/kernel/src/lib.rs" in rendered
    finally:
        shutil.rmtree(jail, ignore_errors=True)


def test_coverage_path_rejects_paths_outside_target(tmp_path: Path) -> None:
    try:
        _MODULE.coverage_path(tmp_path / "lcov.info")
    except SystemExit as error:
        assert "outside" in str(error)
    else:
        raise AssertionError("path outside target/ must fail")

    escaped = _MODULE.REPO_ROOT / "target" / ".." / "Cargo.toml"
    try:
        _MODULE.coverage_path(escaped)
    except SystemExit as error:
        assert "outside" in str(error)
    else:
        raise AssertionError("target/.. must fail")


def test_main_requires_an_output_path() -> None:
    try:
        _MODULE.main(["coverage.info"])
    except SystemExit as error:
        assert "output" in str(error)
    else:
        raise AssertionError("missing --output must fail")


def test_merge_rejects_a_report_with_no_repository_files() -> None:
    try:
        merge_reports(["SF:/tmp/other.rs\nDA:1,1\nend_of_record\n"], Path("/repo"))
    except SystemExit as error:
        assert str(error) == "LCOV merge produced no repository files"
    else:
        raise AssertionError("expected the merge to reject an empty report")
