"""Tests for release smoke helpers."""

from __future__ import annotations

import sys
import tempfile
import unittest
from pathlib import Path

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


if __name__ == "__main__":
    unittest.main()
