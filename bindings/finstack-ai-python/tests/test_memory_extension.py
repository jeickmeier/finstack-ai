"""MemoryExtension: native memory composition exposed to Python.

The three handles returned by :class:`finstack_ai.MemoryExtension` are
accepted directly in the existing ``toolsets`` / ``context_providers`` /
``observers`` factory parameters, alongside the trusted Python callback
ports. The end-to-end cases below use ``Agent.from_python`` with a scripted
Python model callback, so they stay fully offline.
"""

from __future__ import annotations

import asyncio
import json
from pathlib import Path
from typing import Any

import pytest

import finstack_ai


def _model(
    callback: finstack_ai._finstack_ai.Callback,
) -> finstack_ai.PythonModel:
    return finstack_ai.PythonModel(
        callback,
        component="python.model.memory-fixture",
        provider="python-fixture",
        model="python-fixture-model",
        callback_timeout_seconds=5.0,
    )


def test_in_process_extension_exposes_three_handles() -> None:
    memory = finstack_ai.MemoryExtension.in_process(tenant="t1")

    assert memory.tenant == "t1"
    assert memory.toolset() is not None
    assert memory.context_provider() is not None
    assert memory.observer() is not None


def test_sqlite_extension_constructs_on_a_file(tmp_path: Path) -> None:
    memory = finstack_ai.MemoryExtension.sqlite(
        path=str(tmp_path / "memory.db"),
        tenant="t1",
        user="u1",
        agent="a1",
        workspace="w1",
        manage=True,
    )

    assert memory.tenant == "t1"
    assert memory.toolset() is not None
    assert (tmp_path / "memory.db").exists()


def test_invalid_tenant_raises_configuration_error() -> None:
    with pytest.raises(finstack_ai.ConfigurationError):
        finstack_ai.MemoryExtension.in_process(tenant="")


def test_sqlite_rejects_an_unopenable_path(tmp_path: Path) -> None:
    with pytest.raises(finstack_ai.ConfigurationError):
        finstack_ai.MemoryExtension.sqlite(
            path=str(tmp_path / "missing-dir" / "memory.db"),
            tenant="t1",
        )


def test_policy_gates_the_exposed_tools() -> None:
    read_only = finstack_ai.MemoryExtension.in_process(tenant="t1", write=False)
    assert read_only.toolset().tool_count == 2

    manage = finstack_ai.MemoryExtension.in_process(tenant="t1", manage=True)
    assert manage.toolset().tool_count == 5


def test_remember_then_recall_across_two_agents(tmp_path: Path) -> None:
    # Recall requires the extension tenant to equal the run's tenant scope,
    # which is "python-local" for Agent.run / Agent.start.
    memory = finstack_ai.MemoryExtension.sqlite(
        path=str(tmp_path / "memory.db"),
        tenant="python-local",
    )
    body = "The quarterly close deadline is the fifth business day."

    write_calls = 0

    async def write_model(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        nonlocal write_calls
        del context, request
        write_calls += 1
        if write_calls == 1:
            return {
                "text": "",
                "completion_id": "memory-write-1",
                "tool_calls": [
                    {
                        "name": "remember",
                        "arguments": {
                            "id": "close-deadline",
                            "keywords": ["quarterly", "close", "deadline"],
                            "body": body,
                        },
                    }
                ],
            }
        return {"text": "stored", "completion_id": "memory-write-2"}

    recalled: list[str] = []

    async def read_model(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context
        recalled.append(json.dumps(request))
        return {"text": "recalled", "completion_id": "memory-read-1"}

    async def exercise() -> None:
        writer = await finstack_ai.Agent.from_python(
            _model(write_model),
            [memory.toolset()],
            "Use tools when needed.",
        )
        assert (await writer.run("remember the close deadline")).text == "stored"

        reader = await finstack_ai.Agent.from_python(
            _model(read_model),
            context_providers=[memory.context_provider()],
        )
        assert (await reader.run("when is the quarterly close deadline?")).text == (
            "recalled"
        )

    asyncio.run(exercise())

    assert write_calls == 2
    assert recalled, "the reader model callback must have run"
    assert any(body in payload for payload in recalled), (
        f"memory recall did not reach the model request: {recalled}"
    )


def test_recall_rejects_a_tenant_that_is_not_the_run_scope() -> None:
    memory = finstack_ai.MemoryExtension.in_process(tenant="some-other-tenant")

    async def model_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context, request
        return {"text": "unreachable", "completion_id": "memory-scope-1"}

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(
            _model(model_callback),
            context_providers=[memory.context_provider()],
        )
        with pytest.raises(
            finstack_ai.RuntimeError, match="context_contribution_invalid"
        ):
            await agent.run("anything")

    asyncio.run(exercise())


def test_observer_registers_on_a_linked_factory() -> None:
    memory = finstack_ai.MemoryExtension.in_process(tenant="t1")

    async def construct() -> None:
        agent = await finstack_ai.Agent.openai(
            "fixture-model",
            api_key="sk-openai-secret-canary-056",
            toolsets=[memory.toolset()],
            context_providers=[memory.context_provider()],
            observers=[memory.observer()],
        )
        assert agent.capability_catalog() == []

    asyncio.run(construct())
