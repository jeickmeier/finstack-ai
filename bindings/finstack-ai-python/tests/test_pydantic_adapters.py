"""PR-031 optional Pydantic tool and structured-output adapters."""

from __future__ import annotations

import asyncio
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Annotated, Any

import pytest

import finstack_ai


pydantic = pytest.importorskip("pydantic")
from typing_extensions import TypedDict  # noqa: E402 - optional test dependency


_REPO_ROOT = Path(__file__).resolve().parents[3]


def _records_in_order(trace: list[str], expected: list[str]) -> bool:
    # Later session/conversation records may precede or interleave a mid-run
    # golden snapshot. The fixture language stays the same; require order only.
    index = 0
    for kind in trace:
        if index < len(expected) and kind == expected[index]:
            index += 1
    return index == len(expected)


class Answer(pydantic.BaseModel):
    """Structured answer fixture."""

    answer: int


def _model(callback: finstack_ai._finstack_ai.Callback) -> finstack_ai.PythonModel:
    return finstack_ai.PythonModel(
        callback,
        component="python.model.pydantic-fixture",
        provider="python-pydantic-fixture",
        model="python-pydantic-model",
    )


def test_structured_output_type_retries_through_the_kernel() -> None:
    calls = 0

    async def model_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        nonlocal calls
        del context
        calls += 1
        assert request["output"]["json_schema"]["schema"]["draft"] == "draft202012"
        if calls == 1:
            return {"json": {"wrong": 1}, "completion_id": "invalid-structured"}
        return {"json": {"answer": 42}, "completion_id": "valid-structured"}

    async def exercise() -> tuple[finstack_ai.RunResult, list[str]]:
        agent = await finstack_ai.Agent.from_python(
            _model(model_callback),
            output_type=pydantic.TypeAdapter(Answer),
        )
        run = agent.start("answer", max_output_retries=1)
        events = asyncio.create_task(_collect_event_json(run))
        result = await run.result()
        return result, await events

    result, events = asyncio.run(exercise())
    assert result.text == ""
    assert result.output == Answer(answer=42)
    assert result.retry_attempts == 1
    assert calls == 2
    assert sum("effect_requested" in event for event in events) >= 3
    golden = json.loads(
        (
            _REPO_ROOT
            / "fixtures/compatibility/golden-trace/v1/structured-output"
            / "valid--pr012-validation-retry.json"
        ).read_text()
    )
    assert _records_in_order(result.trace, golden["records"])


@dataclass
class Location:
    """Dataclass input fixture."""

    city: str


class Options(TypedDict):
    """TypedDict input fixture."""

    uppercase: bool


class EchoResult(pydantic.BaseModel):
    """Pydantic output fixture."""

    value: str


@finstack_ai.tool
async def typed_echo(
    location: Location,
    options: Options,
    suffixes: list[str],
) -> EchoResult:
    """Echo one typed value."""

    value = location.city + "".join(suffixes)
    return EchoResult(value=value.upper() if options["uppercase"] else value)


def test_annotated_tool_supports_dataclass_typeddict_and_typeadapter_shapes() -> None:
    model_calls = 0

    async def model_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        nonlocal model_calls
        del context, request
        model_calls += 1
        if model_calls == 1:
            return {
                "text": "",
                "completion_id": "pydantic-tool-call",
                "tool_calls": [
                    {
                        "name": "typed_echo",
                        "arguments": {
                            "location": {"city": "toronto"},
                            "options": {"uppercase": True},
                            "suffixes": ["!"],
                        },
                    }
                ],
            }
        return {"text": "typed tool complete", "completion_id": "pydantic-tool-done"}

    async def exercise() -> finstack_ai.RunResult:
        toolset = finstack_ai.pydantic_toolset(
            typed_echo,
            component="python.toolset.pydantic-fixture",
            name="pydantic-fixture-tools",
        )
        agent = await finstack_ai.Agent.from_python(_model(model_callback), [toolset])
        return await agent.run("echo")

    assert typed_echo.schema_generation == 1
    assert typed_echo.input_schema["additionalProperties"] is False
    assert typed_echo.output_schema is not None
    result = asyncio.run(exercise())
    assert result.text == "typed tool complete"
    assert model_calls == 2
    golden = json.loads(
        (
            _REPO_ROOT
            / "fixtures/compatibility/golden-trace/v1/tool-batch"
            / "valid--pr010-continue-model.json"
        ).read_text()
    )
    start = result.trace.index("tool_batch_opened")
    expected = golden["records_after_open"]
    semantic_trace = [kind for kind in result.trace[start:] if kind in expected]
    assert semantic_trace[: len(expected)] == expected


def test_schema_generation_is_cached_until_explicit_refresh() -> None:
    @finstack_ai.tool
    def cached(value: int) -> Answer:
        """Cache one schema pair."""

        return Answer(answer=value)

    before_input = cached.input_schema
    before_output = cached.output_schema
    assert cached.schema_generation == 1
    assert cached.input_schema == before_input
    assert cached.output_schema == before_output
    assert cached.schema_generation == 1

    cached.refresh_schema()
    assert cached.schema_generation == 2
    assert cached.input_schema == before_input
    assert cached.output_schema == before_output


def test_invalid_tool_arguments_follow_native_validation_without_python_entry() -> None:
    invoked = 0
    model_calls = 0

    @finstack_ai.tool
    def integer_only(value: int) -> Answer:
        """Require one integer."""

        nonlocal invoked
        invoked += 1
        return Answer(answer=value)

    async def model_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        nonlocal model_calls
        del context, request
        model_calls += 1
        if model_calls == 1:
            return {
                "text": "",
                "completion_id": "invalid-tool-args",
                "tool_calls": [{"name": "integer_only", "arguments": {"value": "no"}}],
            }
        return {"text": "recovered", "completion_id": "invalid-tool-recovered"}

    async def exercise() -> tuple[str, list[str]]:
        toolset = finstack_ai.pydantic_toolset(
            integer_only,
            component="python.toolset.invalid-pydantic-fixture",
            name="invalid-pydantic-fixture-tools",
        )
        agent = await finstack_ai.Agent.from_python(_model(model_callback), [toolset])
        run = agent.start("invalid tool")
        events = asyncio.create_task(_collect_event_json(run))
        result = await run.result()
        return result.text, await events

    text, events = asyncio.run(exercise())
    assert text == "recovered"
    assert invoked == 0
    assert "tool_arguments_invalid" in "\n".join(events)


def test_invalid_python_result_reenters_native_output_validation() -> None:
    model_calls = 0

    @finstack_ai.tool
    def invalid_result(value: int) -> Answer:
        """Return a deliberately invalid annotated result."""

        return {"wrong": value}  # type: ignore[return-value]

    async def model_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        nonlocal model_calls
        del context, request
        model_calls += 1
        if model_calls == 1:
            return {
                "text": "",
                "completion_id": "invalid-tool-output",
                "tool_calls": [{"name": "invalid_result", "arguments": {"value": 1}}],
            }
        return {"text": "output recovered", "completion_id": "output-recovered"}

    async def exercise() -> tuple[str, list[str]]:
        toolset = finstack_ai.pydantic_toolset(
            invalid_result,
            component="python.toolset.invalid-output-fixture",
            name="invalid-output-fixture-tools",
        )
        agent = await finstack_ai.Agent.from_python(_model(model_callback), [toolset])
        run = agent.start("invalid output")
        events = asyncio.create_task(_collect_event_json(run))
        result = await run.result()
        return result.text, await events

    text, events = asyncio.run(exercise())
    assert text == "output recovered"
    assert "tool_output_invalid" in "\n".join(events)


def test_unsupported_schema_keyword_reports_exact_pointer() -> None:
    with pytest.raises(
        TypeError,
        match=r"unsupported JSON Schema keyword 'minLength'.*/properties/value/minLength",
    ):

        @finstack_ai.tool
        def constrained(value: Annotated[str, pydantic.Field(min_length=1)]) -> str:
            """Unsupported provider constraint."""

            return value


def test_structured_output_rejects_non_object_root_precisely() -> None:
    async def model_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context, request
        return {"json": [1], "completion_id": "root-array"}

    with pytest.raises(
        TypeError, match="structured_output schema root must be an object"
    ):
        finstack_ai.Agent.from_python(_model(model_callback), output_type=list[int])


async def _collect_event_json(run: finstack_ai.Run) -> list[str]:
    return [event.to_json() async for batch in run.events() for event in batch.events()]
