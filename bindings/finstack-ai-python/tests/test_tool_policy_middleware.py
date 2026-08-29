"""P2.3: native `ToolPolicyMiddleware` wrapper.

The default-allowed set narrows the model-visible tool list (tools are
matched by stable tool id); an allowed tool round-trips while a filtered
one never reaches the model.
"""

from __future__ import annotations

import asyncio
from typing import Any

import finstack_ai

_ALLOWED_ID = "python.policy.allowed"
_DENIED_ID = "python.policy.denied"


def _tool(tool_id: str, model_name: str) -> dict[str, Any]:
    return {
        "id": tool_id,
        "model_name": model_name,
        "title": model_name.title(),
        "description": f"{model_name} test tool",
        "input_schema": {
            "additionalProperties": False,
            "properties": {"value": {"type": "integer"}},
            "required": ["value"],
            "type": "object",
        },
        "output_schema": {
            "additionalProperties": False,
            "properties": {"ok": {"type": "boolean"}},
            "required": ["ok"],
            "type": "object",
        },
        "execution": "sequential",
        "side_effect": "read_only",
        "retry_safety": "safe_to_retry",
        "approval": {"requirement": "not_required", "reason": None, "attributes": {}},
        "max_result_bytes": 4_096,
        "metadata": {},
    }


def _toolset() -> finstack_ai.PythonToolset:
    async def tool_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context, request
        return {"output": {"ok": True}}

    return finstack_ai.PythonToolset(
        tool_callback,
        component="python.toolset.policy",
        name="python-policy-tools",
        tools=[_tool(_ALLOWED_ID, "allowed"), _tool(_DENIED_ID, "denied")],
    )


def _capture_model(captured: list[dict[str, Any]]) -> finstack_ai.PythonModel:
    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, object]:
        del context
        captured.append(request)
        return {"text": "acknowledged", "completion_id": "python-tool-policy-1"}

    return finstack_ai.PythonModel(
        callback,
        component="python.model.tool-policy",
        provider="python-fixture",
        model="python-fixture-model",
    )


def _tool_names(request: dict[str, Any]) -> set[str]:
    return {tool.get("model_name") for tool in request.get("tools", [])}


def test_default_allowlist_narrows_visible_tools() -> None:
    middleware = finstack_ai.ToolPolicyMiddleware(default_allowed=[_ALLOWED_ID])
    assert middleware.component == "finstack.middleware.tool-policy"

    captured: list[dict[str, Any]] = []

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(
            _capture_model(captured),
            toolsets=[_toolset()],
            middleware=[middleware],
        )
        result = await agent.run("hello")
        assert result.text == "acknowledged"

    asyncio.run(exercise())

    names = _tool_names(captured[0])
    assert "allowed" in names, names
    assert "denied" not in names, names


def test_without_policy_both_tools_visible() -> None:
    captured: list[dict[str, Any]] = []

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(
            _capture_model(captured), toolsets=[_toolset()]
        )
        await agent.run("hello")

    asyncio.run(exercise())

    names = _tool_names(captured[0])
    assert {"allowed", "denied"} <= names, names


def test_empty_policy_rejected() -> None:
    try:
        finstack_ai.ToolPolicyMiddleware()
    except ValueError as error:
        assert "empty_policy" in str(error), str(error)
    else:
        raise AssertionError("expected a ValueError for an empty policy")


def test_unknown_jailbreak_action_rejected() -> None:
    try:
        finstack_ai.ToolPolicyMiddleware(
            jailbreak_patterns=["ignore previous"], jailbreak_action="observe"
        )
    except ValueError as error:
        assert "jailbreak_action" in str(error)
    else:
        raise AssertionError("expected a ValueError for an unknown action")
