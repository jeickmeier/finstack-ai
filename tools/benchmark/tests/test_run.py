"""Unit tests for benchmark metadata staging."""

from __future__ import annotations

import json
import tempfile
from pathlib import Path
from unittest import mock

import pytest

from benchmark import run as bench

REPO_ROOT = Path(__file__).resolve().parents[3]


def test_validate_metadata_rejects_unknown_fields() -> None:
    metadata = {
        "format_version": 1,
        "package": "finstack-ai-test",
        "workload": "x",
        "commit": "0123456789abcdef",
        "rustc": "rustc 1.97.1",
        "target": "aarch64-apple-darwin",
        "host_triple": "aarch64-apple-darwin",
        "platform": {"system": "Darwin", "machine": "arm64", "release": "1"},
        "features": [],
        "profile": "bench",
        "sample_settings": {
            "measurement_time_secs": 1.0,
            "sample_size": 10,
        },
        "artifact_paths": {
            "raw_samples_dir": "target/benchmark/raw",
            "metadata_file": "target/benchmark/metadata.json",
        },
        "generated_at_unix_ms": 0,
        "mystery": True,
    }
    with pytest.raises(SystemExit):
        bench.validate_metadata(metadata)


def test_validate_metadata_accepts_fixture_shape() -> None:
    path = (
        REPO_ROOT
        / "fixtures/compatibility/benchmark-report/v1/metadata/valid--minimal.json"
    )
    metadata = json.loads(path.read_text(encoding="utf-8"))
    bench.validate_metadata(metadata)


def test_write_metadata_is_sorted_json() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "metadata.json"
        metadata = {
            "format_version": 1,
            "package": "finstack-ai-test",
            "workload": "x",
            "commit": "0123456789abcdef",
            "rustc": "rustc 1.97.1",
            "target": "aarch64-apple-darwin",
            "host_triple": "aarch64-apple-darwin",
            "platform": {"system": "Darwin", "machine": "arm64", "release": "1"},
            "features": [],
            "profile": "bench",
            "sample_settings": {
                "measurement_time_secs": 1.0,
                "sample_size": 10,
            },
            "artifact_paths": {
                "raw_samples_dir": "target/benchmark/raw",
                "metadata_file": "target/benchmark/metadata.json",
            },
            "generated_at_unix_ms": 0,
        }
        bench.validate_metadata(metadata)
        bench.write_metadata(metadata, path)
        text = path.read_text(encoding="utf-8")
        assert text == json.dumps(metadata, indent=2, sort_keys=True) + "\n"


def test_build_metadata_includes_required_identity_fields() -> None:
    raw_dir = bench.ARTIFACT_ROOT / "unit" / "raw"
    metadata_path = bench.ARTIFACT_ROOT / "unit" / "metadata.json"
    with (
        mock.patch.object(bench, "git_commit", return_value="abc1234deadbeef"),
        mock.patch.object(bench, "rustc_version", return_value="rustc 1.97.1"),
        mock.patch.object(bench, "host_triple", return_value="aarch64-apple-darwin"),
        mock.patch.object(bench, "package_features", return_value=[]),
    ):
        metadata = bench.build_metadata(
            smoke=True,
            raw_dir=raw_dir,
            metadata_path=metadata_path,
        )
    for key in ("rustc", "target", "commit", "features", "platform", "host_triple"):
        assert key in metadata
    assert (
        metadata["workload"] == "reducer-model-tool-throughput-and-idle-session-memory"
    )
    assert "model stream throughput" in metadata["notes"]
    assert "idle-session RSS" in metadata["notes"]
    bench.validate_metadata(metadata)


def test_parse_idle_memory_requires_one_complete_integer_result() -> None:
    parsed = bench.parse_idle_memory(
        'noise\nIDLE_SESSION_MEMORY {"baseline_kib":10,"resident_kib":20,'
        '"incremental_kib":10,"sessions":2,"bytes_per_session":5120}\n'
    )
    assert parsed["bytes_per_session"] == 5120
    with pytest.raises(SystemExit):
        bench.parse_idle_memory("missing")
