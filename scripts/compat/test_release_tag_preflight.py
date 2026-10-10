"""Release-tag allowlist for the preflight script."""

from __future__ import annotations

import importlib.util
from pathlib import Path

import pytest

_SPEC = importlib.util.spec_from_file_location(
    "release_tag_preflight",
    Path(__file__).resolve().parent / "release_tag_preflight.py",
)
if _SPEC is None or _SPEC.loader is None:
    raise RuntimeError("release_tag_preflight module is missing")
_MODULE = importlib.util.module_from_spec(_SPEC)
_SPEC.loader.exec_module(_MODULE)
validated_tag = _MODULE.validated_tag


def test_validated_tag_accepts_numeric_version() -> None:
    assert validated_tag("v1.0.0") == "v1.0.0"
    assert validated_tag("v10.2.3") == "v10.2.3"


def test_validated_tag_rejects_injection() -> None:
    for value in ("v1.0.0;touch", "v1.0.0 --upload-pack=x", "HEAD", "v1.0", ""):
        with pytest.raises(SystemExit, match="v1.0.0"):
            validated_tag(value)
