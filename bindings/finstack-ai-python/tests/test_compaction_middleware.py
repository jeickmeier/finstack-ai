"""P1.2: native `CompactionMiddleware` wrapper (sliding window, large tool output).

Compaction rewrites only the model-visible context projection; the journal
keeps canonical history. The behavioral proof is a multi-turn lane: with a
tiny sliding window, turn one's marker text is dropped from turn two's
model-visible request while the run still completes.
"""

from __future__ import annotations

import asyncio
from typing import Any

import finstack_ai


def _model(captured: list[dict[str, Any]]) -> finstack_ai.PythonModel:
    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, object]:
        del context
        captured.append(request)
        return {"text": "acknowledged", "completion_id": "python-compaction-1"}

    return finstack_ai.PythonModel(
        callback,
        component="python.model.compaction",
        provider="python-fixture",
        model="python-fixture-model",
        context_window_tokens=131_072,
    )


def _request_text(request: dict[str, Any]) -> str:
    return "".join(
        block["text"]
        for message in request["messages"]
        for block in message["content"]
        if block["kind"] == "text"
    )


def test_both_strategies_register_and_runs_complete() -> None:
    sliding = finstack_ai.CompactionMiddleware.sliding_window(120_000, 24_000)
    assert sliding.component == "finstack.middleware.compaction"

    async def exercise() -> None:
        # High thresholds: the Continue path — runs complete untouched.
        agent = await finstack_ai.Agent.from_python(_model([]), middleware=[sliding])
        result = await agent.run("short question")
        assert result.text == "acknowledged"
        assert "run_completed" in result.trace

        large = finstack_ai.CompactionMiddleware.large_tool_output(
            120_000, 24_000, 4_096
        )
        agent = await finstack_ai.Agent.from_python(_model([]), middleware=[large])
        result = await agent.run("short question")
        assert "run_completed" in result.trace

    asyncio.run(exercise())


def test_tiny_sliding_window_drops_old_turns_from_model_view() -> None:
    captured: list[dict[str, Any]] = []

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(
            _model(captured),
            middleware=[finstack_ai.CompactionMiddleware.sliding_window(64, 0)],
        )
        session = await agent.create_session("python-local")
        lane = await session.lane("main")

        first = lane.run(agent, "MARKER_ONE " * 120)
        assert (await first.result()).text == "acknowledged"
        second = lane.run(agent, "And the follow-up question?")
        assert (await second.result()).text == "acknowledged"

    asyncio.run(exercise())

    assert len(captured) == 2
    assert "MARKER_ONE" in _request_text(captured[0])
    # Turn two's model-visible projection has been compacted: the oldest
    # unprotected context (turn one's oversized prompt) is gone.
    assert "MARKER_ONE" not in _request_text(captured[1]), _request_text(captured[1])


def test_zero_threshold_rejected() -> None:
    try:
        finstack_ai.CompactionMiddleware.sliding_window(0, 0)
    except ValueError as error:
        assert "invalid_compaction_thresholds" in str(error), str(error)
    else:
        raise AssertionError("expected a ValueError for a zero threshold")


def test_one_strategy_per_instance() -> None:
    """Two compaction instances share one component id; the engine rejects
    duplicate registration rather than merging strategies."""

    async def exercise() -> None:
        try:
            await finstack_ai.Agent.from_python(
                _model([]),
                middleware=[
                    finstack_ai.CompactionMiddleware.sliding_window(120_000, 24_000),
                    finstack_ai.CompactionMiddleware.large_tool_output(
                        120_000, 24_000, 4_096
                    ),
                ],
            )
        except finstack_ai.ConfigurationError as error:
            assert "registration_duplicate" in str(error), str(error)
        else:
            raise AssertionError("expected duplicate-component rejection")

    asyncio.run(exercise())


def test_summarize_strategy_summarizes_via_authorized_model() -> None:
    """A tiny summarize threshold routes a compaction model request to the
    registered model and lands the summary in the model-visible view."""
    captured: list[dict[str, Any]] = []
    calls = 0

    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context
        nonlocal calls
        calls += 1
        captured.append(request)
        return {"text": "acknowledged", "completion_id": f"python-summarize-{calls}"}

    model = finstack_ai.PythonModel(
        callback,
        component="python.model.summarize",
        provider="python-fixture",
        model="python-fixture-model",
        context_window_tokens=131_072,
    )

    async def exercise() -> None:
        # Threshold above one turn but below two: turn one passes, turn two
        # summarizes the prior turn through the authorized model.
        agent = await finstack_ai.Agent.from_python(
            model,
            middleware=[
                finstack_ai.CompactionMiddleware.summarize(
                    1_000,
                    0,
                    model_component="python.model.summarize",
                    budget_scope="01234567-89ab-7cde-89ab-0123456789ab",
                )
            ],
        )
        session = await agent.create_session("python-local")
        lane = await session.lane("main")
        first = lane.run(agent, "SUMMARIZE_ME " * 90)
        assert (await first.result()).text == "acknowledged"
        second = lane.run(agent, "FOLLOW_UP " * 90)
        assert (await second.result()).text == "acknowledged"

    asyncio.run(exercise())

    # The scripted model served at least one extra (summary) request.
    assert calls >= 3, calls


def test_summarize_invalid_budget_scope_rejected() -> None:
    try:
        finstack_ai.CompactionMiddleware.summarize(
            64, 0, model_component="python.model.x", budget_scope="not-a-uuid"
        )
    except ValueError:
        pass
    else:
        raise AssertionError("expected a ValueError for an invalid budget scope")
