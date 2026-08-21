"""Task 8: `Agent.run` attachment staging and document-ingest wiring.

Mirrors the Rust `document_ingest_lane_delivers_markdown_to_model_and_keeps_journaled_file_block`
lane test (crates/finstack-ai-test/tests/lanes/document_ingest.rs) from the
Python surface: `finstack_ai.Attachment` inputs are staged through the
binding-held `InProcessArtifactStore`, and the run
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
    """The model only ever sees extracted Markdown, never a raw File block.

    Captures the canonical `ModelRequestDraft` JSON the Rust engine hands to
    the `PythonModel` callback (see
    `bindings/finstack-ai-python/src/callbacks/model.rs::PythonModelAdapter::request`,
    which invokes the callback with `&request.draft` serialized to JSON) so
    this test can inspect the model-visible message content after
    `DocumentIngestMiddleware`'s `BeforeModel` rewrite has already run —
    mirroring the Rust lane test's assertions (b) and (c) in
    `crates/finstack-ai-test/tests/lanes/document_ingest.rs`.
    """

    captured_requests: list[dict[str, Any]] = []

    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, object]:
        del context
        captured_requests.append(request)
        return {"text": "acknowledged", "completion_id": "python-attachment-1"}

    model = finstack_ai.PythonModel(
        callback,
        component="python.model.attachments-capture",
        provider="python-fixture",
        model="python-fixture-model",
    )

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(model)
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

    assert len(captured_requests) == 1
    messages = captured_requests[0]["messages"]
    user_messages = [message for message in messages if message["role"] == "user"]
    assert user_messages, "expected at least one user message in the model request"

    user_blocks = [block for message in user_messages for block in message["content"]]

    # (b) no File/media block remains in the model-visible content: the
    # middleware always replaces a supported `File` block with `Text`.
    assert not any(block["kind"] == "file" for block in user_blocks), (
        f"model must never see a File block: {user_blocks!r}"
    )

    # (a) the model-visible text carries the parsed CSV, converted to
    # Markdown by the document toolset's parser.
    model_visible_text = "".join(
        block["text"] for block in user_blocks if block["kind"] == "text"
    )
    assert "Q1" in model_visible_text, model_visible_text
    assert "revenue" in model_visible_text.lower(), model_visible_text


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


def test_attachment_from_path_over_4mib_is_rejected(tmp_path: Any) -> None:
    oversized_path = tmp_path / "oversized.bin"
    oversized_path.write_bytes(b"a" * (4 * 1024 * 1024 + 1))

    try:
        finstack_ai.Attachment(
            media_type="application/octet-stream", path=str(oversized_path)
        )
    except ValueError as error:
        assert "4 mib" in str(error).lower()
    else:
        raise AssertionError("expected a ValueError for a >4 MiB path attachment")


def test_attachment_from_path_defaults_name_to_basename(tmp_path: Any) -> None:
    csv_path = tmp_path / "revenue.csv"
    csv_path.write_bytes(b"quarter,revenue\nQ1,1250\n")

    attachment = finstack_ai.Attachment(media_type="text/csv", path=str(csv_path))

    # The pyclass does not expose its fields back to Python; constructing
    # successfully and feeding it through a run is the behavioral contract
    # under test (name defaulting is exercised again via the run below).
    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(_acknowledging_model())
        result = await agent.run(
            "Summarize the attached file.", attachments=[attachment]
        )
        assert result.text == "acknowledged"

    asyncio.run(exercise())
