"""Allowlist checks for the search benchmark launcher."""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))

from search import approved_binary, benchmark_argv


def test_approved_binary_accepts_search_scale_under_target(tmp_path: Path) -> None:
    target = tmp_path / "target"
    binary = target / "release-fast" / "examples" / "search_scale"
    binary.parent.mkdir(parents=True)
    binary.write_bytes(b"")

    assert approved_binary(binary, target) == binary.resolve()


def test_approved_binary_rejects_other_names(tmp_path: Path) -> None:
    target = tmp_path / "target"
    binary = target / "release-fast" / "examples" / "other"
    binary.parent.mkdir(parents=True)
    binary.write_bytes(b"")

    with pytest.raises(SystemExit, match="search_scale"):
        approved_binary(binary, target)


def test_approved_binary_rejects_paths_outside_target(tmp_path: Path) -> None:
    binary = tmp_path / "search_scale"
    binary.write_bytes(b"")

    with pytest.raises(SystemExit, match="search_scale"):
        approved_binary(binary, tmp_path / "target")


def test_benchmark_argv_uses_typed_fields(tmp_path: Path) -> None:
    target = tmp_path / "target"
    binary = target / "release-fast" / "examples" / "search_scale"
    binary.parent.mkdir(parents=True)
    binary.write_bytes(b"")
    root = tmp_path / "corpus"

    argv = benchmark_argv(
        binary,
        root,
        100,
        100,
        100,
        5,
        reuse=True,
        memory_only=False,
        target_root=target,
    )

    assert argv == [
        str(binary.resolve()),
        "--root",
        str(root.resolve()),
        "--chunks",
        "100",
        "--memories",
        "100",
        "--journal-entries",
        "100",
        "--samples",
        "5",
        "--reuse",
    ]
