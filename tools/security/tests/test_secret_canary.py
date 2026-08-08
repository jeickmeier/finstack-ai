"""Tests for secret canary helpers."""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(REPO_ROOT / "tools"))

from security import secret_canary  # noqa: E402


class SecretCanaryTests(unittest.TestCase):
    """Focused coverage for canary assembly and fixture metadata."""

    def test_assembled_token_is_not_committed_as_a_whole(self) -> None:
        token = secret_canary.assembled_token()
        self.assertTrue(token.startswith("finstack_canary_"))
        self.assertGreaterEqual(len(token), len("finstack_canary_") + 40)
        fixture_root = REPO_ROOT / "fixtures" / "security" / "canary-redaction"
        for path in fixture_root.rglob("*"):
            if path.is_file():
                text = path.read_text(encoding="utf-8")
                self.assertNotIn(token, text)

    def test_manifest_marks_fixture_non_evidentiary(self) -> None:
        manifest = secret_canary.load_fixture_manifest()
        self.assertFalse(manifest["evidence_eligible"])
        self.assertTrue(manifest["contract_only_not_executed"])


if __name__ == "__main__":
    unittest.main()
