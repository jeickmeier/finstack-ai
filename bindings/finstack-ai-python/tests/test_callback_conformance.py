"""Non-default PR-030 Model and Toolset port-conformance fixture."""

from __future__ import annotations

import asyncio
from typing import Any

import pytest

import finstack_ai


pytestmark = pytest.mark.skipif(
    not hasattr(finstack_ai._finstack_ai, "_callback_port_conformance"),
    reason="callback conformance fixture feature is not enabled",
)


def _tool() -> dict[str, Any]:
    return {
        "id": "python.conformance",
        "model_name": "conformance",
        "title": "Conformance",
        "description": "Return one normalized conformance value.",
        "input_schema": {
            "additionalProperties": False,
            "properties": {"value": {"type": "string"}},
            "required": ["value"],
            "type": "object",
        },
        "output_schema": {
            "additionalProperties": False,
            "properties": {"value": {"type": "string"}},
            "required": ["value"],
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


def test_python_model_and_toolset_pass_public_port_conformance() -> None:
    async def model_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context, request
        return {"text": "conformant", "completion_id": "python-conformance-1"}

    async def tool_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context
        return {"output": request["call"]["arguments"]}

    async def exercise() -> tuple[str, str]:
        model = finstack_ai.PythonModel(
            model_callback,
            component="python.model.conformance",
            provider="python-conformance",
            model="python-conformance-model",
        )
        toolset = finstack_ai.PythonToolset(
            tool_callback,
            component="python.toolset.conformance",
            name="python-conformance-tools",
            tools=[_tool()],
        )
        return await finstack_ai._finstack_ai._callback_port_conformance(model, toolset)

    assert asyncio.run(exercise()) == ("model", "toolset")
