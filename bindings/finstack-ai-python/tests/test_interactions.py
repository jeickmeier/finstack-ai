"""PR-044 Python list/resolve and approval-envelope proofs."""

from __future__ import annotations

import asyncio
from typing import Any

import finstack_ai


def _write_tool() -> dict[str, Any]:
    return {
        "id": "python.write",
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
        "side_effect": "non_idempotent_write",
        "retry_safety": "at_most_once",
        "approval": {
            "requirement": "required",
            "reason": None,
            "attributes": {},
        },
        "max_result_bytes": 4_096,
        "metadata": {},
    }


def _resolution(request: dict[str, object], approved: bool) -> dict[str, object]:
    return {
        "interaction_id": request["interaction_id"],
        "resolution_id": "python-resolution-1",
        "principal": {
            "issuer": "finstack-ai-python",
            "subject": "local-user",
            "tenant_scope": "python-local",
        },
        "authorization": {
            "policy_version": "python-policy-v1",
            "decision_id": "python-decision-v1",
        },
        "response": {"approved": approved},
    }


async def _wait_for_interaction(run: finstack_ai.Run) -> dict[str, object]:
    for _ in range(200):
        pending = await run.list_interactions()
        if pending:
            return pending[0]
        await asyncio.sleep(0.01)
    raise AssertionError("timed out waiting for an outstanding interaction")


def test_python_list_and_resolve_grant_the_protected_write_tool() -> None:
    model_calls = 0
    tool_requests: list[dict[str, Any]] = []

    async def model_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context, request
        nonlocal model_calls
        model_calls += 1
        if model_calls == 1:
            return {
                "text": "",
                "completion_id": "python-write-call-1",
                "tool_calls": [{"name": "write", "arguments": {"value": 1}}],
            }
        return {"text": "write complete", "completion_id": "python-write-call-2"}

    async def tool_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        assert context.kind == "toolset"
        tool_requests.append(request)
        return {"output": {"ok": True, "value": request["call"]["arguments"]["value"]}}

    async def exercise() -> str:
        toolset = finstack_ai.PythonToolset(
            tool_callback,
            component="python.toolset.write",
            name="python-write-tools",
            tools=[_write_tool()],
        )
        agent = await finstack_ai.Agent.from_python(
            finstack_ai.PythonModel(
                model_callback,
                component="python.model.write",
                provider="python-fixture",
                model="python-fixture-model",
            ),
            [toolset],
            "Use tools when needed.",
        )
        run = agent.start("write 1")
        assert await run.list_interactions() == []
        pending = await _wait_for_interaction(run)
        assert pending["kind"] == {"kind": "approval"}
        await run.resolve_interaction(_resolution(pending, True))
        return (await run.result()).text

    assert asyncio.run(exercise()) == "write complete"
    assert model_calls == 2
    assert len(tool_requests) == 1


def test_python_denied_approval_never_calls_the_protected_tool() -> None:
    tool_requests: list[dict[str, Any]] = []
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
                "completion_id": "python-write-deny-1",
                "tool_calls": [{"name": "write", "arguments": {"value": 1}}],
            }
        return {"text": "denied without write", "completion_id": "python-write-deny-2"}

    async def tool_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context
        tool_requests.append(request)
        return {"output": {"ok": True, "value": 1}}

    async def exercise() -> str:
        toolset = finstack_ai.PythonToolset(
            tool_callback,
            component="python.toolset.write-deny",
            name="python-write-deny-tools",
            tools=[_write_tool()],
        )
        agent = await finstack_ai.Agent.from_python(
            finstack_ai.PythonModel(
                model_callback,
                component="python.model.write-deny",
                provider="python-fixture",
                model="python-fixture-model",
            ),
            [toolset],
            "Use tools when needed.",
        )
        run = agent.start("write 1")
        pending = await _wait_for_interaction(run)
        await run.resolve_interaction(_resolution(pending, False))
        return (await run.result()).text

    assert asyncio.run(exercise()) == "denied without write"
    assert tool_requests == []
