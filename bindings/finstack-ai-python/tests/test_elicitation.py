"""Elicitation toolset: ask_user parks the run and the answer becomes the tool result."""

from __future__ import annotations

import asyncio
import json
from typing import Any

import finstack_ai


def _resolution(
    request: dict[str, object], response: dict[str, object]
) -> dict[str, object]:
    return {
        "interaction_id": request["interaction_id"],
        "resolution_id": "python-elicitation-1",
        "principal": {
            "issuer": "finstack-ai-python",
            "subject": "local-user",
            "tenant_scope": "python-local",
        },
        "authorization": {
            "policy_version": "python-policy-v1",
            "decision_id": "python-decision-v1",
        },
        "response": response,
    }


async def _wait_for_interaction(run: finstack_ai.Run) -> dict[str, object]:
    for _ in range(200):
        pending = await run.list_interactions()
        if pending:
            return pending[0]
        await asyncio.sleep(0.01)
    raise AssertionError("timed out waiting for an outstanding interaction")


def test_ask_user_parks_and_the_answer_reaches_the_model() -> None:
    model_calls = 0
    second_request: dict[str, Any] = {}

    async def model_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context
        nonlocal model_calls
        model_calls += 1
        if model_calls == 1:
            return {
                "text": "",
                "completion_id": "elicit-call-1",
                "tool_calls": [
                    {
                        "name": "ask_user",
                        "arguments": {
                            "prompt": "What is the position limit?",
                            "kind": None,
                            "options": None,
                            "response_schema": None,
                        },
                    }
                ],
            }
        second_request.update(request)
        return {"text": "limit recorded", "completion_id": "elicit-call-2"}

    async def exercise() -> str:
        toolset = finstack_ai.ElicitationToolset(ask_user=True)
        agent = await finstack_ai.Agent.from_python(
            finstack_ai.PythonModel(
                model_callback,
                component="python.model.elicit",
                provider="python-fixture",
                model="python-fixture-model",
            ),
            [toolset],
            "Ask the user when information is missing.",
        )
        run = agent.start("size the position")
        pending = await _wait_for_interaction(run)
        assert pending["kind"] == {"kind": "free_text"}
        assert "What is the position limit?" in json.dumps(pending["prompt"])
        await run.resolve_interaction(_resolution(pending, {"answer": "250k USD"}))
        return (await run.result()).text

    assert asyncio.run(exercise()) == "limit recorded"
    assert model_calls == 2
    assert "250k USD" in json.dumps(second_request), (
        "the user's answer must reach the model as the tool result"
    )


def test_typed_elicitation_tool_uses_registered_schema() -> None:
    model_calls = 0

    async def model_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context, request
        nonlocal model_calls
        model_calls += 1
        if model_calls == 1:
            return {
                "text": "",
                "completion_id": "typed-call-1",
                "tool_calls": [
                    {
                        "name": "confirm_trade_params",
                        "arguments": {"context": "Buy 100 AAPL @ market."},
                    }
                ],
            }
        return {"text": "confirmed", "completion_id": "typed-call-2"}

    async def exercise() -> str:
        toolset = finstack_ai.ElicitationToolset(
            tools=[
                {
                    "name": "confirm_trade_params",
                    "title": "Confirm trade parameters",
                    "description": "Ask the operator to confirm trade parameters.",
                    "prompt": "Please confirm the trade parameters.",
                    "kind": "form",
                    "response_schema": {
                        "type": "object",
                        "properties": {"confirmed": {"type": "boolean"}},
                        "required": ["confirmed"],
                    },
                }
            ]
        )
        agent = await finstack_ai.Agent.from_python(
            finstack_ai.PythonModel(
                model_callback,
                component="python.model.typed-elicit",
                provider="python-fixture",
                model="python-fixture-model",
            ),
            [toolset],
            "Confirm before executing.",
        )
        run = agent.start("buy 100 AAPL")
        pending = await _wait_for_interaction(run)
        assert pending["kind"] == {"kind": "form"}
        schema = pending["response_schema"]
        assert '"confirmed"' in json.dumps(schema)
        assert "Buy 100 AAPL @ market." in json.dumps(pending["prompt"])
        await run.resolve_interaction(
            _resolution(pending, {"answer": {"confirmed": True}})
        )
        return (await run.result()).text

    assert asyncio.run(exercise()) == "confirmed"
    assert model_calls == 2


def test_ask_user_rejects_non_string_answer() -> None:
    model_calls = 0

    async def model_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context, request
        nonlocal model_calls
        model_calls += 1
        return {
            "text": "",
            "completion_id": "elicit-reject-1",
            "tool_calls": [
                {
                    "name": "ask_user",
                    "arguments": {
                        "prompt": "What is the position limit?",
                        "kind": None,
                        "options": None,
                        "response_schema": None,
                    },
                }
            ],
        }

    async def exercise() -> None:
        toolset = finstack_ai.ElicitationToolset(ask_user=True)
        agent = await finstack_ai.Agent.from_python(
            finstack_ai.PythonModel(
                model_callback,
                component="python.model.elicit-reject",
                provider="python-fixture",
                model="python-fixture-model",
            ),
            [toolset],
            "Ask the user when information is missing.",
        )
        run = agent.start("size the position")
        pending = await _wait_for_interaction(run)
        try:
            await run.resolve_interaction(_resolution(pending, {"answer": 1}))
        except finstack_ai.RuntimeError:
            still = await run.list_interactions()
            assert still, "invalid resolve must not commit"
            await run.cancel()
            return
        raise AssertionError("non-string free-text answer must be rejected")

    asyncio.run(exercise())
    assert model_calls == 1


def test_typed_elicitation_rejects_string_answer() -> None:
    model_calls = 0

    async def model_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context, request
        nonlocal model_calls
        model_calls += 1
        return {
            "text": "",
            "completion_id": "typed-reject-1",
            "tool_calls": [
                {
                    "name": "confirm_trade_params",
                    "arguments": {"context": "Buy 100 AAPL @ market."},
                }
            ],
        }

    async def exercise() -> None:
        toolset = finstack_ai.ElicitationToolset(
            tools=[
                {
                    "name": "confirm_trade_params",
                    "title": "Confirm trade parameters",
                    "description": "Ask the operator to confirm trade parameters.",
                    "prompt": "Please confirm the trade parameters.",
                    "kind": "form",
                    "response_schema": {
                        "type": "object",
                        "properties": {"confirmed": {"type": "boolean"}},
                        "required": ["confirmed"],
                    },
                }
            ]
        )
        agent = await finstack_ai.Agent.from_python(
            finstack_ai.PythonModel(
                model_callback,
                component="python.model.typed-elicit-reject",
                provider="python-fixture",
                model="python-fixture-model",
            ),
            [toolset],
            "Confirm before executing.",
        )
        run = agent.start("buy 100 AAPL")
        pending = await _wait_for_interaction(run)
        try:
            await run.resolve_interaction(_resolution(pending, {"answer": "nope"}))
        except finstack_ai.RuntimeError:
            still = await run.list_interactions()
            assert still, "invalid resolve must not commit"
            await run.cancel()
            return
        raise AssertionError("typed tool string answer must be rejected")

    asyncio.run(exercise())
    assert model_calls == 1


def test_elicitation_toolset_requires_at_least_one_tool() -> None:
    try:
        finstack_ai.ElicitationToolset()
    except (TypeError, ValueError):
        return
    raise AssertionError("empty elicitation toolset must be rejected")
