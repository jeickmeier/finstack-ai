"""Task 11: debug parse-to-markdown helpers exposed on the Python facade.

`finstack_ai.parse_document_markdown` and `finstack_ai.parse_document` call
`finstack-ai-tools-document`'s `parser::parse` directly, without an `Agent`
or `Run`, so a developer can see exactly what the document-ingest middleware
would inject for a given file.
"""

from __future__ import annotations

from pathlib import Path

import pytest

import finstack_ai

FIXTURES_DIR = Path(__file__).resolve().parents[3] / "fixtures" / "documents"

SAMPLE_CSV = b"quarter,revenue\nQ1,1250\nQ2,1310\n"


def test_parse_document_markdown_returns_markdown_for_csv() -> None:
    markdown = finstack_ai.parse_document_markdown(
        media_type="text/csv", data=SAMPLE_CSV
    )

    assert isinstance(markdown, str)
    assert "1250" in markdown
    assert "revenue" in markdown


def test_parse_document_markdown_from_docx_path() -> None:
    docx_path = FIXTURES_DIR / "sample.docx"
    assert docx_path.exists(), (
        "fixtures/documents/sample.docx is required for this test"
    )

    markdown = finstack_ai.parse_document_markdown(
        media_type="application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        path=str(docx_path),
    )

    assert isinstance(markdown, str)
    assert markdown != ""


def test_parse_document_returns_detailed_result_for_csv() -> None:
    result = finstack_ai.parse_document(media_type="text/csv", data=SAMPLE_CSV)

    assert result["format"] == "csv"
    assert "revenue" in result["markdown"]
    assert result["page_count"] is None
    assert result["classification"] is None
    assert result["requires_ocr"] is False
    assert result["truncated"] is False


def test_parse_document_markdown_requires_exactly_one_of_data_or_path() -> None:
    with pytest.raises(ValueError, match="exactly one of data or path"):
        finstack_ai.parse_document_markdown(media_type="text/csv")

    with pytest.raises(ValueError, match="exactly one of data or path"):
        finstack_ai.parse_document_markdown(
            media_type="text/csv",
            data=SAMPLE_CSV,
            path=str(FIXTURES_DIR / "sample.csv"),
        )


def test_parse_document_markdown_raises_on_corrupt_input() -> None:
    corrupt_path = FIXTURES_DIR / "corrupt.bin"
    assert corrupt_path.exists(), (
        "fixtures/documents/corrupt.bin is required for this test"
    )

    with pytest.raises(ValueError, match="document_unsupported_format"):
        finstack_ai.parse_document_markdown(
            media_type="application/octet-stream", path=str(corrupt_path)
        )


def test_parse_document_markdown_raises_on_oversized_path(tmp_path: Path) -> None:
    oversized = tmp_path / "oversized.csv"
    oversized.write_bytes(b"a" * (4 * 1024 * 1024 + 1))

    with pytest.raises(ValueError, match="4 MiB"):
        finstack_ai.parse_document_markdown(media_type="text/csv", path=str(oversized))
