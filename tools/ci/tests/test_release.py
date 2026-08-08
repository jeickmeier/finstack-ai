"""Tests for release smoke helpers."""

from __future__ import annotations

import json
import os
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

REPO_ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO_ROOT / "tools"))

from ci import release  # noqa: E402


class ReleaseHelperTests(unittest.TestCase):
    """Focused unit coverage for packaging helpers."""

    def test_sha256_file_is_stable(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "payload.bin"
            path.write_bytes(b"finstack-ai-ci-smoke")
            first = release.sha256_file(path)
            second = release.sha256_file(path)
            self.assertEqual(first, second)
            self.assertEqual(len(first), 64)

    def test_binary_name_uses_exe_suffix_on_windows(self) -> None:
        path = release.binary_path(Path("/tmp/target"))
        if sys.platform.startswith("win"):
            self.assertTrue(str(path).endswith(".exe"))
        else:
            self.assertEqual(path.name, "finstack-ai-ci-smoke")

    def test_cargo_package_metadata_reads_workspace_version(self) -> None:
        package = release.cargo_package_metadata()
        self.assertEqual(package["version"], "0.0.1")
        self.assertEqual(package["facade_dependency"], "finstack-ai")
        self.assertIn("native-tokio", package["features"])

    def test_git_commit_fails_closed_in_ci_without_identity(self) -> None:
        with mock.patch.dict(os.environ, {"CI": "true"}, clear=False):
            with mock.patch("ci.release.subprocess.run", side_effect=OSError("no git")):
                with self.assertRaises(SystemExit) as raised:
                    release.git_commit(require=True)
        self.assertIn("requires a git commit identity", str(raised.exception))

    def test_write_metadata_uses_cargo_version_and_commit(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            artifact_dir = Path(tmp)
            binary = artifact_dir / "finstack-ai-ci-smoke"
            binary.write_bytes(b"bin")
            with (
                mock.patch("ci.release.git_commit", return_value="abc123"),
                mock.patch("ci.release.rustc_version", return_value="rustc 1.97.1"),
                mock.patch(
                    "ci.release.host_triple", return_value="aarch64-apple-darwin"
                ),
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
            self.assertEqual(metadata["version"], "0.0.1")
            self.assertEqual(metadata["commit"], "abc123")
            self.assertEqual(metadata["features"], ["native-tokio"])
            self.assertEqual(metadata["facade_dependency"], "finstack-ai")
            self.assertEqual(metadata["sha256"], "deadbeef")


if __name__ == "__main__":
    unittest.main()
