"""P1.1: native `InstructionsMiddleware` wrapper.

The middleware injects frozen policy entries as protected System context
items at ``prepare_context``, so a capture model must see the policy text
in the model-visible request. Registration identity is the crate's own
declared ``finstack.middleware.instructions`` v1.0.0.
"""

from __future__ import annotations

import asyncio
from typing import Any

import finstack_ai


def _capture_model(captured: list[dict[str, Any]]) -> finstack_ai.PythonModel:
    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, object]:
        del context
        captured.append(request)
        return {"text": "acknowledged", "completion_id": "python-instructions-1"}

    return finstack_ai.PythonModel(
        callback,
        component="python.model.instructions",
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


def test_policy_entries_reach_the_model_request() -> None:
    middleware = finstack_ai.InstructionsMiddleware(
        [
            ("citations", "Always cite the source document id."),
            ("tone", "Answer in complete sentences."),
        ]
    )
    assert middleware.component == "finstack.middleware.instructions"

    captured: list[dict[str, Any]] = []

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(
            _capture_model(captured),
            middleware=[middleware],
        )
        result = await agent.run("What does the report say?")
        assert result.text == "acknowledged"

    asyncio.run(exercise())

    assert len(captured) == 1
    text = _all_text(captured[0])
    assert "Always cite the source document id." in text, text
    assert "Answer in complete sentences." in text, text


def test_native_middleware_is_chain_registrable_where_callbacks_are_not() -> None:
    """`InstructionsMiddleware` is the first Python-registrable middleware.

    The engine's middleware chain requires recompute-safe invocations, and
    `PythonMiddleware` callbacks are declared non-repeatable, so registering
    one fails at agent build (pre-existing engine behavior, pinned here).
    The native wrapper registers cleanly in the same ``middleware=[...]``
    parameter.
    """

    async def middleware_callback(
        context: finstack_ai.CallbackContext, request: object
    ) -> object:
        del context, request
        return "continue"

    async def exercise() -> None:
        try:
            await finstack_ai.Agent.from_python(
                _capture_model([]),
                middleware=[
                    finstack_ai.InstructionsMiddleware([("policy", "Policy line.")]),
                    finstack_ai.PythonMiddleware(
                        middleware_callback,
                        component="python.middleware.instructions-co",
                        stages=["before_run"],
                    ),
                ],
            )
        except finstack_ai.ConfigurationError as error:
            assert "recompute-safe" in str(error), str(error)
        else:
            raise AssertionError(
                "expected the non-repeatable python middleware to be rejected"
            )

    asyncio.run(exercise())


def test_empty_entries_rejected() -> None:
    try:
        finstack_ai.InstructionsMiddleware([])
    except ValueError as error:
        assert "entries_empty" in str(error), str(error)
    else:
        raise AssertionError("expected a ValueError for empty entries")


def test_blank_label_rejected() -> None:
    try:
        finstack_ai.InstructionsMiddleware([(" ", "text")])
    except ValueError as error:
        assert "entry_label_empty" in str(error), str(error)
    else:
        raise AssertionError("expected a ValueError for a blank label")
