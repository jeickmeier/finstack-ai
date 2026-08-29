"""P3.1: native `CalculatorToolset` wrapper — scripted tool round trip."""

from __future__ import annotations

import asyncio
from typing import Any

import finstack_ai


def test_calculator_round_trip() -> None:
    toolset = finstack_ai.CalculatorToolset()
    assert toolset.component == "python.tools.calculator"

    tool_results: list[dict[str, Any]] = []
    calls = 0

    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context
        nonlocal calls
        calls += 1
        if calls == 1:
            return {
                "text": "",
                "completion_id": "python-calc-1",
                "tool_calls": [
                    {
                        "name": "calculator",
                        "arguments": {"operation": "add", "operands": [19.0, 23.0]},
                    }
                ],
            }
        for message in request["messages"]:
            for block in message["content"]:
                if block["kind"] == "tool_result":
                    tool_results.append(block)
        return {"text": "the sum is 42", "completion_id": "python-calc-2"}

    model = finstack_ai.PythonModel(
        callback,
        component="python.model.calculator",
        provider="python-fixture",
        model="python-fixture-model",
    )

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(model, toolsets=[toolset])
        result = await agent.run("what is 19 + 23?")
        assert result.text == "the sum is 42"

    asyncio.run(exercise())

    assert calls == 2
    assert tool_results, "expected the tool result in the follow-up request"
    assert "42" in str(tool_results), tool_results


def test_invalid_component_rejected() -> None:
    try:
        finstack_ai.CalculatorToolset("not a component id!")
    except ValueError:
        pass
    else:
        raise AssertionError("expected a ValueError for an invalid component id")
