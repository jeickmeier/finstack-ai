"""Tests for release smoke helpers."""

from __future__ import annotations

import json
import os
import sys
import tempfile
from pathlib import Path
from unittest import mock

import pytest

from ci import release


def test_sha256_file_is_stable() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "payload.bin"
        path.write_bytes(b"finstack-ai-ci-smoke")
        first = release.sha256_file(path)
        second = release.sha256_file(path)
        assert first == second
        assert len(first) == 64


def test_binary_name_uses_exe_suffix_on_windows() -> None:
    path = release.binary_path(Path("/tmp/target"))
    if sys.platform.startswith("win"):
        assert str(path).endswith(".exe")
    else:
        assert path.name == "finstack-ai-ci-smoke"


def test_cargo_package_metadata_reads_workspace_version() -> None:
    package = release.cargo_package_metadata()
    assert package["version"] == "0.0.1"
    assert package["facade_dependency"] == "finstack-ai"
    assert "native-tokio" in package["features"]


def test_git_commit_fails_closed_in_ci_without_identity() -> None:
    with mock.patch.dict(os.environ, {"CI": "true"}, clear=False):
        with mock.patch("ci.release.subprocess.run", side_effect=OSError("no git")):
            with pytest.raises(SystemExit) as raised:
                release.git_commit(require=True)
    assert "requires a git commit identity" in str(raised.value)


def test_write_metadata_uses_cargo_version_and_commit() -> None:
    with tempfile.TemporaryDirectory() as tmp:
        artifact_dir = Path(tmp)
        binary = artifact_dir / "finstack-ai-ci-smoke"
        binary.write_bytes(b"bin")
        with (
            mock.patch("ci.release.git_commit", return_value="abc123"),
            mock.patch("ci.release.rustc_version", return_value="rustc 1.97.1"),
            mock.patch("ci.release.host_triple", return_value="aarch64-apple-darwin"),
            mock.patch(
                "ci.release.cargo_package_metadata",
                return_value={
                    "version": "0.0.1",
                    "features": ["native-tokio"],
                    "facade_dependency": "finstack-ai",
                },
            ),
        ):
            path = release.write_metadata(artifact_dir, binary, "deadbeef")
        metadata = json.loads(path.read_text(encoding="utf-8"))
        assert metadata["version"] == "0.0.1"
        assert metadata["commit"] == "abc123"
        assert metadata["features"] == ["native-tokio"]
        assert metadata["facade_dependency"] == "finstack-ai"
        assert metadata["sha256"] == "deadbeef"
