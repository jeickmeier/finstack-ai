"""Linked-provider factories accept the same Python ports as from_python."""

from __future__ import annotations

import asyncio
import importlib.util
from pathlib import Path
from typing import Any

import finstack_ai
import pytest

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


def test_re_resolve_returns_a_new_agent() -> None:
    async def construct() -> None:
        agent = await finstack_ai.Agent.openai(
            "fixture-model",
            api_key="sk-openai-secret-canary-056",
        )
        resolved = await agent.re_resolve()
        assert resolved.capability_catalog() == []

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


def test_openrouter_constructs_with_openrouter_media() -> None:
    async def construct() -> None:
        agent = await finstack_ai.Agent.openrouter(
            "fixture-model",
            api_key="sk-openrouter-secret-canary-056",
            toolsets=[
                finstack_ai.OpenRouterMediaToolset("sk-openrouter-media-canary-056")
            ],
        )
        assert agent.capability_catalog() == []

    asyncio.run(construct())


def test_openrouter_rejects_duplicate_media_registrations() -> None:
    async def construct() -> None:
        media = finstack_ai.OpenRouterMediaToolset("media-secret")
        with pytest.raises(finstack_ai.ConfigurationError, match="duplicate component"):
            await finstack_ai.Agent.openrouter(
                "fixture-model", api_key="model-secret", toolsets=[media, media]
            )

    asyncio.run(construct())


def test_openrouter_media_requires_explicit_api_key() -> None:
    with pytest.raises(TypeError, match="api_key"):
        finstack_ai.OpenRouterMediaToolset(referer="https://example.test")


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


def test_gateway_constructs_with_openrouter_media() -> None:
    async def construct() -> None:
        agent = await finstack_ai.Agent.gateway(
            "https://api.example.test/v1/responses",
            "fixture-model",
            wire_protocol="openai_responses",
            credential_name="prod",
            hard_input_bytes=1_000_000,
            auth="bearer",
            api_key="sk-gateway-secret-canary-045",
            toolsets=[
                finstack_ai.OpenRouterMediaToolset("sk-openrouter-media-canary-056")
            ],
        )
        assert agent.capability_catalog() == []

    asyncio.run(construct())


def test_gateway_rejects_openai_chat() -> None:
    async def construct() -> None:
        with pytest.raises(finstack_ai.ConfigurationError, match="openai_chat"):
            await finstack_ai.Agent.gateway(
                "https://api.example.test/v1/responses",
                "fixture-model",
                wire_protocol="openai_chat",
                credential_name="prod",
                hard_input_bytes=1_000_000,
                auth="bearer",
                api_key="sk-gateway-secret-canary-045",
            )

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


def test_e2b_toolset_can_be_composed() -> None:
    toolset = finstack_ai.E2bSandboxToolset(
        "e2b-secret-canary-045", endpoint="https://api.e2b.dev"
    )
    assert toolset.tool_count == 1


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
            await finstack_ai.Agent.gemini(
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


def test_openrouter_constructs_with_the_media_pipeline(tmp_path: Path) -> None:
    async def construct() -> None:
        media = finstack_ai.OpenRouterMediaToolset("media-secret")
        compose = finstack_ai.VideoComposeToolset(
            "/usr/bin/ffmpeg",
            "/usr/bin/ffprobe",
            str(tmp_path / "scratch"),
            render_timeout_s=300,
        )
        pipeline = finstack_ai.MediaPipelineToolset(
            media,
            compose,
            max_scenes=4,
            max_total_video_s=120,
            max_concurrent_jobs=2,
            sqlite_state_path=str(tmp_path / "render-state.sqlite3"),
        )
        for factory, args, options in [
            (
                finstack_ai.Agent.openrouter,
                ("fixture-model",),
                {"api_key": "model-secret"},
            ),
            (finstack_ai.Agent.ollama, ("http://127.0.0.1:1", "fixture-model"), {}),
        ]:
            agent = await factory(
                *args,
                **options,
                toolsets=[pipeline, media, compose],
                artifact_path=str(tmp_path / "artifacts"),
            )
            assert agent.capability_catalog() == []

    asyncio.run(construct())


def test_media_pipeline_requires_typed_dependencies() -> None:
    media = finstack_ai.OpenRouterMediaToolset("media-secret")
    with pytest.raises(TypeError, match="compose"):
        finstack_ai.MediaPipelineToolset(
            media, max_scenes=4, max_total_video_s=120, max_concurrent_jobs=2
        )


def test_partial_video_compose_configuration_is_rejected(tmp_path: Path) -> None:
    with pytest.raises(TypeError, match="ffmpeg_path"):
        finstack_ai.VideoComposeToolset(
            ffprobe_path="/usr/bin/ffprobe",
            scratch_dir=str(tmp_path),
            render_timeout_s=300,
        )


@pytest.mark.parametrize(
    "tools",
    [finstack_ai.OpenAiMediaToolset(""), finstack_ai.OpenRouterMediaToolset("")],
)
def test_empty_media_credentials_fail_without_leaking_model_credentials(
    tools: Any,
) -> None:
    async def construct() -> None:
        with pytest.raises(finstack_ai.ConfigurationError) as caught:
            await finstack_ai.Agent.openai(
                "fixture", api_key="model-secret-canary", toolsets=[tools]
            )
        assert "model-secret-canary" not in str(caught.value)

    asyncio.run(construct())


@pytest.mark.parametrize("max_result_bytes", [0, 8 * 1024 * 1024 + 1])
@pytest.mark.parametrize(
    "toolset_type", [finstack_ai.OpenAiMediaToolset, finstack_ai.OpenRouterMediaToolset]
)
def test_media_result_bounds_still_fail_closed(
    toolset_type: Any, max_result_bytes: int
) -> None:
    async def construct() -> None:
        tools = toolset_type("media-secret", max_result_bytes=max_result_bytes)
        with pytest.raises(finstack_ai.ConfigurationError):
            await finstack_ai.Agent.ollama(
                "http://127.0.0.1:1", "fixture", toolsets=[tools]
            )

    asyncio.run(construct())


@pytest.mark.parametrize("render_timeout_s", [0, 3601])
def test_video_timeout_bounds_still_fail_closed(
    tmp_path: Path, render_timeout_s: int
) -> None:
    async def construct() -> None:
        tools = finstack_ai.VideoComposeToolset(
            "/usr/bin/ffmpeg",
            "/usr/bin/ffprobe",
            str(tmp_path),
            render_timeout_s=render_timeout_s,
        )
        with pytest.raises(finstack_ai.ConfigurationError):
            await finstack_ai.Agent.ollama(
                "http://127.0.0.1:1", "fixture", toolsets=[tools]
            )

    asyncio.run(construct())
