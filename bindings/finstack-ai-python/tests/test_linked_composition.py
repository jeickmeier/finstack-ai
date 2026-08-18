"""Linked-provider factories accept the same Python ports as from_python."""

from __future__ import annotations

import asyncio
import importlib.util
from pathlib import Path
from typing import Any

import pytest

import finstack_ai

_HANDLES_SPEC = importlib.util.spec_from_file_location(
    "finstack_ai_test_handles",
    Path(__file__).with_name("test_handles.py"),
)
assert _HANDLES_SPEC is not None and _HANDLES_SPEC.loader is not None
_handles = importlib.util.module_from_spec(_HANDLES_SPEC)
_HANDLES_SPEC.loader.exec_module(_handles)
_server = _handles._server
_ollama_ndjson = _handles._ollama_ndjson


def _echo_tool() -> dict[str, Any]:
    return {
        "id": "python.echo",
        "model_name": "echo",
        "title": "Echo",
        "description": "Echo one validated string value.",
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


def _toolset() -> finstack_ai.PythonToolset:
    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context, request
        return {"output": {"value": "unused"}}

    return finstack_ai.PythonToolset(
        callback,
        component="python.toolset.linked-fixture",
        name="linked-fixture-tools",
        tools=[_echo_tool()],
    )


def _observer() -> finstack_ai.PythonObserver:
    async def observe(batch: list[dict[str, Any]]) -> None:
        del batch

    return finstack_ai.PythonObserver(
        observe,
        component="python.observer.linked-fixture",
        payload_mode="redacted",
    )


def test_openai_api_key_constructs_without_a_request() -> None:
    async def construct() -> None:
        agent = await finstack_ai.Agent.openai(
            "fixture-model",
            "Answer concisely.",
            api_key="sk-openai-secret-canary-056",
        )
        assert agent.capability_catalog() == []

    asyncio.run(construct())


def test_openai_rejects_unknown_reasoning_effort() -> None:
    async def construct() -> None:
        with pytest.raises(finstack_ai.ConfigurationError, match="reasoning_effort"):
            await finstack_ai.Agent.openai(
                "fixture-model",
                api_key="sk-openai-secret-canary-056",
                reasoning_effort="turbo",
            )

    asyncio.run(construct())


def test_openai_rejects_unknown_reasoning_summary() -> None:
    async def construct() -> None:
        with pytest.raises(finstack_ai.ConfigurationError, match="reasoning_summary"):
            await finstack_ai.Agent.openai(
                "fixture-model",
                api_key="sk-openai-secret-canary-056",
                reasoning_summary="verbose",
            )

    asyncio.run(construct())


def test_openai_accepts_reasoning_settings() -> None:
    async def construct() -> None:
        agent = await finstack_ai.Agent.openai(
            "fixture-model",
            "Answer concisely.",
            api_key="sk-openai-secret-canary-056",
            reasoning_effort="low",
            reasoning_summary="auto",
        )
        assert agent.capability_catalog() == []

    asyncio.run(construct())


def test_gateway_constructs_without_a_request() -> None:
    async def construct() -> None:
        agent = await finstack_ai.Agent.gateway(
            "https://api.example.test/v1/responses",
            "fixture-model",
            wire_protocol="openai_responses",
            credential_name="prod",
            hard_input_bytes=1_000_000,
            auth="bearer",
            api_key="sk-gateway-secret-canary-045",
        )
        assert agent.capability_catalog() == []

    asyncio.run(construct())


def test_gateway_rejects_missing_hard_input_bytes() -> None:
    async def construct() -> None:
        with pytest.raises(finstack_ai.ConfigurationError, match="hard_input_bytes"):
            await finstack_ai.Agent.gateway(
                "https://api.example.test/v1/responses",
                "fixture-model",
                wire_protocol="openai_responses",
                credential_name="prod",
            )

    asyncio.run(construct())


def test_gateway_rejects_plaintext_non_loopback() -> None:
    async def construct() -> None:
        with pytest.raises(finstack_ai.ConfigurationError, match="plaintext HTTP"):
            await finstack_ai.Agent.gateway(
                "http://api.example.test/v1/responses",
                "fixture-model",
                wire_protocol="openai_responses",
                credential_name="prod",
                hard_input_bytes=1_000_000,
                auth="none",
            )

    asyncio.run(construct())


def test_gateway_http_credentials_do_not_leak_the_canary() -> None:
    canary = "sk-gateway-secret-canary-045"

    async def construct() -> None:
        with pytest.raises(finstack_ai.ConfigurationError, match="HTTPS") as raised:
            await finstack_ai.Agent.gateway(
                "http://127.0.0.1:9/v1/responses",
                "fixture-model",
                wire_protocol="openai_responses",
                credential_name="prod",
                hard_input_bytes=1_000_000,
                auth="bearer",
                api_key=canary,
            )
        assert canary not in str(raised.value)

    asyncio.run(construct())


def test_openai_api_key_is_keyword_only() -> None:
    async def construct() -> None:
        with pytest.raises(TypeError):
            await finstack_ai.Agent.openai(
                "fixture-model",
                None,
                "sk-should-not-be-positional",
            )

    asyncio.run(construct())


def test_linked_factories_accept_python_toolset_and_observer_at_construct() -> None:
    async def construct() -> None:
        ports = {"toolsets": [_toolset()], "observers": [_observer()]}
        agents = [
            await finstack_ai.Agent.openai(
                "fixture-model",
                api_key="sk-openai-secret-canary-056",
                **ports,
            ),
            await finstack_ai.Agent.ollama(
                "http://127.0.0.1:11434", "fixture-model", **ports
            ),
            await finstack_ai.Agent.anthropic(
                "http://127.0.0.1:9", "fixture-model", **ports
            ),
        ]
        assert all(agent.capability_catalog() == [] for agent in agents)

    asyncio.run(construct())


def test_ollama_observer_receives_loopback_events() -> None:
    seen: list[dict[str, Any]] = []

    async def observe(batch: list[dict[str, Any]]) -> None:
        seen.extend(batch)

    observer = finstack_ai.PythonObserver(
        observe,
        component="python.observer.linked-run",
        payload_mode="redacted",
    )

    async def exercise(server: Any) -> str:
        agent = await finstack_ai.Agent.ollama(
            server.base_url,
            "fixture-model",
            "Answer concisely.",
            observers=[observer],
        )
        return (await agent.run("say hello")).text

    with _server(_ollama_ndjson(["hello", " world"])) as server:
        text = asyncio.run(exercise(server))

    assert text == "hello world"
    assert any(event.get("kind") == "run_completed" for event in seen)
