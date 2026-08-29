"""P1.5: native skills toolset and model-driven capability activation.

Ports the essential cases of the Rust `capability_activation` tests: the
scripted model calls ``capability_activate``; the capability's instruction
takes effect on the following model request; the run's
``active_capabilities`` lists it; and a capability-gated toolset's tools
appear only after activation.
"""

from __future__ import annotations

import asyncio
from typing import Any

import finstack_ai

_CAPABILITY = "python.capability.research"
_GATED_TOOLSET = "python.tools.research"


def _write_tool() -> dict[str, Any]:
    return {
        "id": "python.research.write",
        "model_name": "write",
        "title": "Write",
        "description": "Write one validated integer value.",
        "input_schema": {
            "additionalProperties": False,
            "properties": {"value": {"type": "integer"}},
            "required": ["value"],
            "type": "object",
        },
        "output_schema": {
            "additionalProperties": False,
            "properties": {"ok": {"type": "boolean"}, "value": {"type": "integer"}},
            "required": ["ok", "value"],
            "type": "object",
        },
        "execution": "sequential",
        "side_effect": "read_only",
        "retry_safety": "safe_to_retry",
        "approval": {
            "requirement": "not_required",
            "reason": None,
            "attributes": {},
        },
        "max_result_bytes": 4_096,
        "metadata": {},
    }


def _research_capability() -> finstack_ai.Capability:
    return finstack_ai.Capability(
        _CAPABILITY,
        "Research notes",
        ["Research instruction."],
        activation="model",
        toolsets=[_GATED_TOOLSET],
    )


def _gated_toolset() -> finstack_ai.PythonToolset:
    async def tool_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context
        return {"output": {"ok": True, "value": request["call"]["arguments"]["value"]}}

    return finstack_ai.PythonToolset(
        tool_callback,
        component=_GATED_TOOLSET,
        name="python-research-tools",
        tools=[_write_tool()],
    )


def _activating_model(captured: list[dict[str, Any]]) -> finstack_ai.PythonModel:
    calls = 0

    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context
        nonlocal calls
        captured.append(request)
        calls += 1
        if calls == 1:
            return {
                "text": "",
                "completion_id": "python-skills-activate-1",
                "tool_calls": [
                    {"name": "capability_activate", "arguments": {"id": _CAPABILITY}}
                ],
            }
        return {"text": "done", "completion_id": "python-skills-activate-2"}

    return finstack_ai.PythonModel(
        callback,
        component="python.model.skills",
        provider="python-fixture",
        model="python-fixture-model",
    )


def _tool_names(request: dict[str, Any]) -> set[str]:
    return {
        tool.get("model_name") or tool.get("name") for tool in request.get("tools", [])
    }


def test_model_activation_commits_and_gates_tools() -> None:
    captured: list[dict[str, Any]] = []

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(
            _activating_model(captured),
            toolsets=[finstack_ai.SkillsToolset()],
            capabilities=[_research_capability()],
            capability_toolsets=[_gated_toolset()],
        )
        result = await agent.run("activate research")
        assert result.text == "done"
        assert any(item["id"] == _CAPABILITY for item in result.active_capabilities), (
            result.active_capabilities
        )

    asyncio.run(exercise())

    assert len(captured) == 2
    first, second = captured

    # Before activation: skills tools visible, gated tool hidden.
    assert "capability_activate" in _tool_names(first)
    assert "capability_list" in _tool_names(first)
    assert "write" not in _tool_names(first)

    # After activation: the gated tool appears and the capability's
    # instruction is now part of the model-visible request.
    assert "write" in _tool_names(second), _tool_names(second)
    all_text = "".join(
        block["text"]
        for message in second["messages"]
        for block in message["content"]
        if block["kind"] == "text"
    )
    assert "Research instruction." in all_text, all_text


def test_skills_toolset_rejected_on_linked_factories() -> None:
    async def exercise() -> None:
        try:
            await finstack_ai.Agent.ollama(
                "http://127.0.0.1:9",  # never reached: rejection is pre-network
                "test-model",
                toolsets=[finstack_ai.SkillsToolset()],
            )
        except ValueError as error:
            assert "from_python" in str(error), str(error)
        else:
            raise AssertionError("expected SkillsToolset rejection")

    asyncio.run(exercise())


def test_capability_component_reference_requires_valid_id() -> None:
    try:
        finstack_ai.Capability(
            "python.capability.bad",
            "Bad refs",
            ["x"],
            activation="model",
            toolsets=["not a valid component id!"],
        )
    except TypeError:
        pass
    else:
        raise AssertionError("expected a TypeError for an invalid component id")
