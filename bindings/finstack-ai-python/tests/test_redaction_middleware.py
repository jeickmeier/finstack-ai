"""P2.2: native `RedactionMiddleware` wrapper.

A seeded secret in the prompt is replaced with a ``[REDACTED:<kind>]``
marker in the model-visible request; the run still completes.
"""

from __future__ import annotations

import asyncio
from typing import Any

import finstack_ai

_EMAIL = "alice@example.com"


def _capture_model(captured: list[dict[str, Any]]) -> finstack_ai.PythonModel:
    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, object]:
        del context
        captured.append(request)
        return {"text": "acknowledged", "completion_id": "python-redaction-1"}

    return finstack_ai.PythonModel(
        callback,
        component="python.model.redaction",
        provider="python-fixture",
        model="python-fixture-model",
    )


def _all_text(request: dict[str, Any]) -> str:
    return "".join(
        block["text"]
        for message in request["messages"]
        for block in message["content"]
        if block["kind"] == "text"
    )


def test_seeded_email_is_redacted_in_model_view() -> None:
    middleware = finstack_ai.RedactionMiddleware()
    assert middleware.component == "finstack.middleware.redaction"

    captured: list[dict[str, Any]] = []

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(
            _capture_model(captured), middleware=[middleware]
        )
        result = await agent.run(f"Please email {_EMAIL} about the report.")
        assert result.text == "acknowledged"

    asyncio.run(exercise())

    text = _all_text(captured[0])
    assert _EMAIL not in text, text
    assert "[REDACTED:email]" in text, text


def test_disabled_detector_leaves_text_untouched() -> None:
    captured: list[dict[str, Any]] = []

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(
            _capture_model(captured),
            middleware=[finstack_ai.RedactionMiddleware(detect_emails=False)],
        )
        await agent.run(f"Please email {_EMAIL} about the report.")

    asyncio.run(exercise())

    assert _EMAIL in _all_text(captured[0])


def test_unsupported_output_policy_rejected() -> None:
    try:
        finstack_ai.RedactionMiddleware(output_policy="observe")
    except ValueError as error:
        assert "output_policy" in str(error)
    else:
        raise AssertionError("expected a ValueError for an unsupported policy")
