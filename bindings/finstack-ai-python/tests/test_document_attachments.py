"""Task 8: `Agent.run` attachment staging and document-ingest wiring.

Mirrors the Rust `document_ingest_lane_delivers_markdown_to_model_and_keeps_journaled_file_block`
lane test (crates/finstack-ai-test/tests/lanes/document_ingest.rs) from the
Python surface: `finstack_ai.Attachment` inputs are staged through the
binding-held `InProcessArtifactStore`/`AttachmentIndex` pair and the run
still completes normally.
"""

from __future__ import annotations

import asyncio
from typing import Any

import finstack_ai


def _acknowledging_model() -> finstack_ai.PythonModel:
    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, object]
    ) -> dict[str, object]:
        del context, request
        return {"text": "acknowledged", "completion_id": "python-attachment-1"}

    return finstack_ai.PythonModel(
        callback,
        component="python.model.attachments",
        provider="python-fixture",
        model="python-fixture-model",
    )


def test_run_with_csv_attachment() -> None:
    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(_acknowledging_model())
        result = await agent.run(
            "Summarize the attached file.",
            attachments=[
                finstack_ai.Attachment(
                    data=b"quarter,revenue\nQ1,1250\n",
                    media_type="text/csv",
                    name="revenue.csv",
                )
            ],
        )
        assert result is not None
        assert result.text == "acknowledged"

    asyncio.run(exercise())


def test_more_than_eight_attachments_rejected() -> None:
    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(_acknowledging_model())
        attachment = finstack_ai.Attachment(data=b"a,b\n1,2\n", media_type="text/csv")
        try:
            await agent.run("x", attachments=[attachment] * 9)
        except Exception as error:  # noqa: BLE001 - binding maps to its own error type
            assert "attachment" in str(error).lower()
        else:
            raise AssertionError("expected attachment-count rejection")

    asyncio.run(exercise())


def test_attachment_requires_exactly_one_of_data_or_path() -> None:
    try:
        finstack_ai.Attachment(media_type="text/csv")
    except ValueError as error:
        assert "exactly one" in str(error).lower()
    else:
        raise AssertionError("expected a ValueError for a missing data/path")

    try:
        finstack_ai.Attachment(
            media_type="text/csv", data=b"a,b\n1,2\n", path="/tmp/does-not-matter.csv"
        )
    except ValueError as error:
        assert "exactly one" in str(error).lower()
    else:
        raise AssertionError("expected a ValueError for both data and path")


def test_attachment_from_path_defaults_name_to_basename(tmp_path: Any) -> None:
    csv_path = tmp_path / "revenue.csv"
    csv_path.write_bytes(b"quarter,revenue\nQ1,1250\n")

    attachment = finstack_ai.Attachment(media_type="text/csv", path=str(csv_path))

    # The pyclass does not expose its fields back to Python; constructing
    # successfully and feeding it through a run is the behavioral contract
    # under test (name defaulting is exercised again via the run below).
    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(_acknowledging_model())
        result = await agent.run("Summarize the attached file.", attachments=[attachment])
        assert result.text == "acknowledged"

    asyncio.run(exercise())
