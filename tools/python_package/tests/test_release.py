"""Unit tests for deterministic Python release metadata."""

from __future__ import annotations

from pathlib import Path
from typing import cast

from python_package import release


def test_cyclonedx_sbom_is_deterministic_and_binds_subjects(tmp_path: Path) -> None:
    wheel = tmp_path / "finstack_ai-0.0.2-py3-none-any.whl"
    sdist = tmp_path / "finstack_ai-0.0.2.tar.gz"
    wheel.write_bytes(b"wheel")
    sdist.write_bytes(b"sdist")
    revision = "a" * 40

    first = release.sbom([wheel, sdist], revision)
    second = release.sbom([wheel, sdist], revision)

    assert first == second
    assert first["bomFormat"] == "CycloneDX"
    assert first["specVersion"] == "1.6"
    metadata = cast(dict[str, object], first["metadata"])
    component = cast(dict[str, str], metadata["component"])
    properties = cast(list[dict[str, str]], metadata["properties"])
    components = cast(list[dict[str, str]], first["components"])
    assert component["version"] == "0.0.2"
    assert {item["name"] for item in properties} >= {
        "finstack-ai:source-revision",
        f"finstack-ai:subject:{wheel.name}:sha256",
        f"finstack-ai:subject:{sdist.name}:sha256",
    }
    assert components == sorted(
        components, key=lambda item: (item["name"], item["version"])
    )
