"""PR-030 trusted Python callback adapter behavior."""

from __future__ import annotations

import asyncio
import json
import threading
from concurrent.futures import ThreadPoolExecutor
from typing import Any
from pathlib import Path

import pytest

import finstack_ai


_REPO_ROOT = Path(__file__).resolve().parents[3]


def _model(
    callback: finstack_ai._finstack_ai.Callback,
    *,
    timeout: float = 1.0,
) -> finstack_ai.PythonModel:
    return finstack_ai.PythonModel(
        callback,
        component="python.model.fixture",
        provider="python-fixture",
        model="python-fixture-model",
        callback_timeout_seconds=timeout,
    )


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


def test_async_python_model_runs_through_rust_agent() -> None:
    seen: list[tuple[dict[str, object], dict[str, Any]]] = []

    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        seen.append((context.to_dict(), request))
        await asyncio.sleep(0)
        return {"text": "hello from Python", "completion_id": "python-1"}

    async def exercise() -> finstack_ai.RunResult:
        agent = await finstack_ai.Agent.from_python(_model(callback))
        return await agent.run("hello")

    result = asyncio.run(exercise())
    assert result.text == "hello from Python"
    context, request = seen[0]
    assert context["kind"] == "model"
    assert context["cancelled"] is False
    assert request["model"] == "python-fixture-model"


def test_sync_python_model_runs_off_the_event_loop_thread() -> None:
    callback_threads: list[int] = []

    def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context, request
        callback_threads.append(threading.get_ident())
        return {"text": "sync", "completion_id": "python-sync-1"}

    async def exercise() -> tuple[int, str]:
        event_loop_thread = threading.get_ident()
        agent = await finstack_ai.Agent.from_python(_model(callback))
        result = await agent.run("hello")
        return event_loop_thread, result.text

    event_loop_thread, text = asyncio.run(exercise())
    assert text == "sync"
    assert callback_threads != [event_loop_thread]


def test_coarse_port_adapters_validate_and_cache_registration() -> None:
    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context, request
        return {"items": [], "estimated_tokens": 0, "bytes": 0, "cache_key": None}

    async def observe(batch: list[dict[str, Any]]) -> None:
        del batch

    context = finstack_ai.PythonContextProvider(
        callback,
        component="python.context.fixture",
    )
    middleware = finstack_ai.PythonMiddleware(
        callback,
        component="python.middleware.fixture",
        stages=["before_run", "before_finalize"],
        priority=10,
    )
    observer = finstack_ai.PythonObserver(
        observe,
        component="python.observer.fixture",
        payload_mode="redacted",
    )
    assert context.component == "python.context.fixture"
    assert middleware.component == "python.middleware.fixture"
    assert observer.component == "python.observer.fixture"
    with pytest.raises(TypeError, match="stages must not be empty"):
        finstack_ai.PythonMiddleware(
            callback,
            component="python.middleware.invalid",
            stages=[],
        )
    with pytest.raises(TypeError, match="unsupported observer payload_mode"):
        finstack_ai.PythonObserver(
            observe,
            component="python.observer.invalid",
            payload_mode="credential",
        )


@pytest.mark.parametrize(
    ("kind", "fixture"),
    [
        (
            "interaction_resolution",
            "interaction-resolution-command/roundtrip--valid.json",
        ),
        (
            "external_effect_completion",
            "external-effect-completion-command/roundtrip--failed.json",
        ),
    ],
)
def test_prebeta_external_shapes_match_rust_public_fixtures(
    kind: str, fixture: str
) -> None:
    path = _REPO_ROOT / "fixtures/compatibility/public-rust-api/v1" / fixture
    value = json.loads(path.read_text())["input"]
    assert finstack_ai.normalize_prebeta_shape(kind, value) == value


def test_prebeta_child_lineage_shape_matches_rust() -> None:
    value = {
        "parent_run_id": "01234567-89ab-7cde-89ab-0123456789ab",
        "parent_effect_id": "01234567-89ab-7cde-89ab-0123456789ad",
        "child": {
            "operation": {
                "tenant_scope": "tenant-a",
                "session_id": "01234567-89ab-7cde-89ab-0123456789ae",
                "lane_id": "01234567-89ab-7cde-89ab-0123456789af",
                "run_id": "01234567-89ab-7cde-89ab-0123456789b0",
            }
        },
        "request_digest": "00" * 32,
        "placement": "compatible_lane_in_parent_session",
    }
    assert finstack_ai.normalize_prebeta_shape("child_run_prepared", value) == value


def test_prebeta_shape_rejects_unknown_fields() -> None:
    fixture = (
        _REPO_ROOT
        / "fixtures/compatibility/public-rust-api/v1"
        / "interaction-resolution-command/roundtrip--valid.json"
    )
    value = json.loads(fixture.read_text())["input"]
    value["unexpected"] = True
    with pytest.raises(TypeError, match="invalid pre-beta shape"):
        finstack_ai.normalize_prebeta_shape("interaction_resolution", value)


def test_python_model_and_toolset_complete_a_tool_cycle() -> None:
    model_calls = 0
    tool_requests: list[dict[str, Any]] = []

    async def model_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        nonlocal model_calls
        del context, request
        model_calls += 1
        if model_calls == 1:
            return {
                "text": "",
                "completion_id": "python-tool-call-1",
                "tool_calls": [{"name": "echo", "arguments": {"value": "hi"}}],
            }
        return {"text": "tool complete", "completion_id": "python-tool-call-2"}

    async def tool_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        assert context.kind == "toolset"
        tool_requests.append(request)
        return {"output": request["call"]["arguments"]}

    async def exercise() -> str:
        toolset = finstack_ai.PythonToolset(
            tool_callback,
            component="python.toolset.fixture",
            name="python-fixture-tools",
            tools=[_echo_tool()],
        )
        assert toolset.tool_count == 1
        agent = await finstack_ai.Agent.from_python(
            _model(model_callback), [toolset], "Use tools when needed."
        )
        run = agent.start("echo hi")
        events = asyncio.create_task(_collect_event_json(run))
        try:
            return (await run.result()).text
        except finstack_ai.RuntimeError as error:
            observed = await events
            pytest.fail(
                f"tool cycle failed after {model_calls} model calls and "
                f"{len(tool_requests)} tool calls: {observed}: {error}"
            )

    assert asyncio.run(exercise()) == "tool complete"
    assert model_calls == 2
    assert len(tool_requests) == 1
    assert tool_requests[0]["call"]["arguments"] == {"value": "hi"}


def test_callback_cancellation_is_forwarded_and_context_expires() -> None:
    retained: list[finstack_ai.CallbackContext] = []
    started = threading.Event()
    cancellation_seen = threading.Event()

    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del request
        retained.append(context)
        started.set()
        await context.wait_cancelled()
        assert context.cancelled is True
        cancellation_seen.set()
        return {"text": "late", "completion_id": "python-late-1"}

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(_model(callback))
        run = agent.start("wait")
        events = asyncio.create_task(_collect_event_json(run))
        assert await asyncio.to_thread(started.wait, 1)
        await run.cancel()
        try:
            await run.result()
        except finstack_ai.FinstackError as error:
            assert isinstance(error, finstack_ai.CancelledError), await events
            assert error.code == "agent_run_cancelled"
        else:
            pytest.fail("cancelled callback run completed successfully")
        assert await asyncio.to_thread(cancellation_seen.wait, 1)

    asyncio.run(exercise())
    with pytest.raises(finstack_ai.RuntimeError) as caught:
        _ = retained[0].run_id
    assert caught.value.code == "python_callback_context_settled"


@pytest.mark.parametrize("mode", ["exception", "invalid", "timeout"])
def test_callback_failures_are_stable_and_do_not_leak_exception_text(mode: str) -> None:
    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context, request
        if mode == "exception":
            raise ValueError("SECRET_CALLBACK_CANARY")
        if mode == "timeout":
            await asyncio.sleep(1)
            return {"text": "late", "completion_id": "python-late-2"}
        return {"unknown": True}

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(
            _model(callback, timeout=0.01 if mode == "timeout" else 1.0)
        )
        with pytest.raises(finstack_ai.RuntimeError) as caught:
            await agent.run("fail")
        assert caught.value.code == "agent_run_runtime_failure"
        assert "SECRET_CALLBACK_CANARY" not in str(caught.value)

    asyncio.run(exercise())


def test_python_callback_agent_is_shareable_across_threads() -> None:
    def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context, request
        return {"text": "shared", "completion_id": "python-shared-1"}

    async def build() -> finstack_ai.Agent:
        # This case proves cross-thread sharing rather than timeout behavior.
        # Use the public adapter default so a loaded free-threaded Windows
        # runner cannot turn scheduler latency into a false concurrency error.
        return await finstack_ai.Agent.from_python(_model(callback, timeout=30.0))

    agent = asyncio.run(build())

    def run(_: int) -> str:
        async def execute() -> str:
            return (await agent.run("hello")).text

        return asyncio.run(execute())

    with ThreadPoolExecutor(max_workers=4) as executor:
        assert list(executor.map(run, range(16))) == ["shared"] * 16


async def _collect_event_json(run: finstack_ai.Run) -> list[str]:
    return [event.to_json() async for batch in run.events() for event in batch.events()]
