"""Waiver / allowlist field validation tests."""

from __future__ import annotations

import tempfile
from pathlib import Path

from architecture.check import load_toml, parse_allowlist, validate_exception_dict

REPO_ROOT = Path(__file__).resolve().parents[3]
TOOL_DIR = REPO_ROOT / "tools" / "architecture"
FIXTURE = REPO_ROOT / "fixtures" / "architecture" / "cases" / "valid-waiver.toml"


def test_valid_waiver_fixture_complete() -> None:
    policy = load_toml(TOOL_DIR / "policy.toml")
    required = policy["exceptions"]["required_fields"]
    data = load_toml(FIXTURE)
    entries = data.get("exceptions", [])
    assert len(entries) == 1
    assert validate_exception_dict(entries[0], required) == []


def test_each_required_field_removal_fails() -> None:
    policy = load_toml(TOOL_DIR / "policy.toml")
    required = policy["exceptions"]["required_fields"]
    data = load_toml(FIXTURE)
    base = dict(data["exceptions"][0])
    for field in required:
        broken = dict(base)
        broken.pop(field, None)
        missing = validate_exception_dict(broken, required)
        assert missing, field


def test_wildcard_subject_rejected() -> None:
    policy = load_toml(TOOL_DIR / "policy.toml")
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "allowlist.toml"
        valid = load_toml(FIXTURE)["exceptions"][0]
        lines = ["[[exceptions]]"]
        for key, value in valid.items():
            if key == "subject":
                value = "crates/*"
            if isinstance(value, str):
                lines.append(f'{key} = "{value}"')
            else:
                lines.append(f"{key} = {value!r}")
        path.write_text("\n".join(lines) + "\n", encoding="utf-8")
        _entries, diags = parse_allowlist(path, policy)
        assert any("wildcard" in d.message for d in diags)
