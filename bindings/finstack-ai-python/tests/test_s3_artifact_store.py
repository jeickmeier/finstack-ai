"""P4.2: S3 artifact store option (`artifact_store=`).

Without a live bucket the tests pin the construction contract (strict
endpoint/bucket/credential validation, mutual exclusion with
``artifact_path``); a live round trip runs only when
FINSTACK_TEST_S3_ENDPOINT/_BUCKET/_REGION are set.
"""

from __future__ import annotations

import asyncio
import os
from pathlib import Path
from typing import Any

import pytest

import finstack_ai

_LIVE = all(
    os.environ.get(name)
    for name in (
        "FINSTACK_TEST_S3_ENDPOINT",
        "FINSTACK_TEST_S3_BUCKET",
        "FINSTACK_TEST_S3_REGION",
    )
)


def _model() -> finstack_ai.PythonModel:
    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, object]:
        del context, request
        return {"text": "acknowledged", "completion_id": "python-s3-1"}

    return finstack_ai.PythonModel(
        callback,
        component="python.model.s3",
        provider="python-fixture",
        model="python-fixture-model",
    )


def test_store_constructs_and_registers() -> None:
    store = finstack_ai.S3ArtifactStore(
        "https://s3.example.com", "test-bucket", "us-east-1"
    )

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(_model(), artifact_store=store)
        result = await agent.run("hello")
        assert result.text == "acknowledged"

    asyncio.run(exercise())


def test_invalid_bucket_rejected() -> None:
    try:
        finstack_ai.S3ArtifactStore("https://s3.example.com", "NOT/VALID", "us-east-1")
    except ValueError:
        pass
    else:
        raise AssertionError("expected a ValueError for an invalid bucket")


def test_half_credentials_rejected() -> None:
    try:
        finstack_ai.S3ArtifactStore(
            "https://s3.example.com",
            "test-bucket",
            "us-east-1",
            access_key_id="AKIA",
        )
    except ValueError as error:
        assert "together" in str(error)
    else:
        raise AssertionError("expected credential-pairing rejection")


def test_mutually_exclusive_with_artifact_path(tmp_path: Path) -> None:
    store = finstack_ai.S3ArtifactStore(
        "https://s3.example.com", "test-bucket", "us-east-1"
    )

    async def exercise() -> None:
        try:
            await finstack_ai.Agent.from_python(
                _model(),
                artifact_path=str(tmp_path / "artifacts"),
                artifact_store=store,
            )
        except ValueError as error:
            assert "mutually exclusive" in str(error)
        else:
            raise AssertionError("expected mutual-exclusion rejection")

    asyncio.run(exercise())


@pytest.mark.skipif(not _LIVE, reason="FINSTACK_TEST_S3_* not set")
def test_live_s3_attachment_round_trip() -> None:
    store = finstack_ai.S3ArtifactStore(
        os.environ["FINSTACK_TEST_S3_ENDPOINT"],
        os.environ["FINSTACK_TEST_S3_BUCKET"],
        os.environ["FINSTACK_TEST_S3_REGION"],
        access_key_id=os.environ.get("FINSTACK_TEST_S3_ACCESS_KEY_ID"),
        secret_access_key=os.environ.get("FINSTACK_TEST_S3_SECRET_ACCESS_KEY"),
    )

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(_model(), artifact_store=store)
        result = await agent.run(
            "summarize",
            attachments=[
                finstack_ai.Attachment(
                    data=b"a,b\n1,2\n", media_type="text/csv", name="live.csv"
                )
            ],
        )
        assert result.text == "acknowledged"

    asyncio.run(exercise())
