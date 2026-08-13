"""PR-032 shared capability-catalog and Python activation conformance."""

from __future__ import annotations

import asyncio
import json
from pathlib import Path
from typing import Any

import finstack_ai


_REPO_ROOT = Path(__file__).resolve().parents[3]


def _text_values(value: object) -> list[str]:
    if isinstance(value, dict):
        return [
            item
            for key, nested in value.items()
            for item in _text_values(nested)
            if key == "text" or not isinstance(nested, str)
        ]
    if isinstance(value, list):
        return [item for nested in value for item in _text_values(nested)]
    return [value] if isinstance(value, str) else []


def test_all_activation_modes_share_rust_owned_trace_and_stable_prefix() -> None:
    requests: list[dict[str, Any]] = []

    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context
        requests.append(request)
        return {"text": "ok", "completion_id": f"capability-{len(requests)}"}

    async def exercise() -> tuple[finstack_ai.RunResult, finstack_ai.RunResult, str]:
        model = finstack_ai.PythonModel(
            callback,
            component="python.model.capability",
            provider="python-fixture",
            model="python-capability-model",
        )
        capabilities = [
            finstack_ai.Capability(
                "python.capability.always",
                "Baseline safety guidance",
                ["Always instruction."],
                activation="always",
            ),
            finstack_ai.Capability(
                "python.capability.application",
                "Application selected accounting guidance",
                ["Application instruction."],
                activation="application",
            ),
            finstack_ai.Capability(
                "python.capability.research",
                "Research financial statements",
                ["Research instruction."],
                activation="model",
            ),
        ]
        agent = await finstack_ai.Agent.from_python(
            model,
            instruction="Stable prefix.",
            capabilities=capabilities,
            active_capabilities=["python.capability.application"],
        )
        assert agent.capability_catalog() == [
            {
                "id": "python.capability.research",
                "description": "Research financial statements",
            }
        ]
        first = await agent.run("say hello")
        second = await agent.run("research financial statements")
        return first, second, agent.compact_capability_catalog()

    first, second, catalog = asyncio.run(exercise())
    assert first.active_capabilities == [
        {"id": "python.capability.always", "source": "always"},
        {"id": "python.capability.application", "source": "application"},
    ]
    assert second.active_capabilities == [
        {"id": "python.capability.always", "source": "always"},
        {"id": "python.capability.application", "source": "application"},
        {"id": "python.capability.research", "source": "model"},
    ]
    assert catalog == "python.capability.research: Research financial statements"
    golden = json.loads(
        (
            _REPO_ROOT
            / "fixtures/compatibility/golden-trace/v1/structured-output"
            / "valid--pr012-capabilities.json"
        ).read_text()
    )
    assert first.trace[: len(golden["records"])] == golden["records"]
    assert second.trace[: len(golden["records"])] == golden["records"]

    first_text = _text_values(requests[0]["messages"])
    second_text = _text_values(requests[1]["messages"])
    assert first_text.index("Stable prefix.") < first_text.index("Always instruction.")
    assert second_text.index("Stable prefix.") < second_text.index(
        "Always instruction."
    )
    assert first_text[:2] == second_text[:2]
    assert "Research instruction." not in first_text
    assert "Research instruction." in second_text


def test_capability_configuration_fails_closed() -> None:
    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context, request
        return {"text": "unused", "completion_id": "unused"}

    model = finstack_ai.PythonModel(
        callback,
        component="python.model.capability-invalid",
        provider="python-fixture",
        model="python-capability-model",
    )
    capability = finstack_ai.Capability(
        "python.capability.model-only",
        "Model selected capability",
        ["Model instruction."],
        activation="model",
    )

    async def exercise() -> None:
        await finstack_ai.Agent.from_python(
            model,
            capabilities=[capability],
            active_capabilities=["python.capability.model-only"],
        )

    try:
        asyncio.run(exercise())
    except finstack_ai.ConfigurationError as error:
        assert error.code == "agent_run_invalid_configuration"
        assert "capability_not_application_activated" in str(error)
    else:
        raise AssertionError("model capability was accepted as application activation")
