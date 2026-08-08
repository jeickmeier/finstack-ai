"""Tests for secret canary helpers."""

from __future__ import annotations

from pathlib import Path

from security import secret_canary

REPO_ROOT = Path(__file__).resolve().parents[3]


def test_assembled_token_is_not_committed_as_a_whole() -> None:
    token = secret_canary.assembled_token()
    assert token.startswith("finstack_canary_")
    assert len(token) >= len("finstack_canary_") + 40
    fixture_root = REPO_ROOT / "fixtures" / "security" / "canary-redaction"
    for path in fixture_root.rglob("*"):
        if path.is_file():
            text = path.read_text(encoding="utf-8")
            assert token not in text


def test_manifest_marks_fixture_non_evidentiary() -> None:
    manifest = secret_canary.load_fixture_manifest()
    assert not manifest["evidence_eligible"]
    assert manifest["contract_only_not_executed"]


def test_canary_paths_cover_lockfiles() -> None:
    paths = set(secret_canary.CANARY_PATHS)
    assert "examples/Cargo.lock" in paths
    assert "fixtures/Cargo.lock" in paths
    assert "examples/uv.lock" in paths
    assert "fixtures/uv.lock" in paths
