"""P2.4: native metrics, otel, billing, and notify observers.

Each observer is asserted on its observable read surface after a run, not
on construction alone.
"""

from __future__ import annotations

import asyncio
import json
from typing import Any

import finstack_ai


def _model() -> finstack_ai.PythonModel:
    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, object]:
        del context, request
        return {"text": "acknowledged", "completion_id": "python-observers-1"}

    return finstack_ai.PythonModel(
        callback,
        component="python.model.native-observers",
        provider="python-fixture",
        model="python-fixture-model",
    )


def _eliciting_model() -> finstack_ai.PythonModel:
    calls = 0

    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context, request
        nonlocal calls
        calls += 1
        if calls == 1:
            return {
                "text": "",
                "completion_id": "python-observers-ask-1",
                "tool_calls": [
                    {
                        "name": "ask_user",
                        "arguments": {
                            "prompt": "Which quarter?",
                            "kind": None,
                            "options": None,
                            "response_schema": None,
                        },
                    }
                ],
            }
        return {"text": "done", "completion_id": "python-observers-ask-2"}

    return finstack_ai.PythonModel(
        callback,
        component="python.model.native-observers-ask",
        provider="python-fixture",
        model="python-fixture-model",
    )


def test_metrics_observer_renders_prometheus_after_run() -> None:
    observer = finstack_ai.MetricsObserver()
    assert observer.component == "finstack.observer.metrics"

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(_model(), observers=[observer])
        await agent.run("hello")

    asyncio.run(exercise())

    text = observer.encode_prometheus()
    assert "finstack_" in text, text
    assert "# TYPE" in text, text


def test_otel_observer_captures_spans() -> None:
    observer = finstack_ai.OtelObserver()
    assert observer.component == "finstack.observer.otel"

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(_model(), observers=[observer])
        await agent.run("hello")

    asyncio.run(exercise())

    spans = observer.spans()
    assert spans, "expected at least one captured span"
    assert all("name" in span and "attributes" in span for span in spans)


def test_billing_observer_exports_ledger_jsonl() -> None:
    observer = finstack_ai.BillingObserver()
    assert observer.component == "finstack.observer.billing"

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(_model(), observers=[observer])
        await agent.run("hello")

    asyncio.run(exercise())

    export = observer.export_jsonl()
    lines = [json.loads(line) for line in export.splitlines() if line.strip()]
    assert lines, export
    assert any("unattributed_effects" in line for line in lines), export


def test_notify_observer_announces_interaction_lifecycle() -> None:
    notifications: list[dict[str, Any]] = []

    def sink(payload: str) -> None:
        notifications.append(json.loads(payload))

    observer = finstack_ai.NotifyObserver(sink)
    assert observer.component == "finstack.observer.notify"

    async def wait_for_interaction(run: finstack_ai.Run) -> dict[str, object]:
        for _ in range(200):
            pending = await run.list_interactions()
            if pending:
                return pending[0]
            await asyncio.sleep(0.01)
        raise AssertionError("timed out waiting for an outstanding interaction")

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(
            _eliciting_model(),
            toolsets=[finstack_ai.ElicitationToolset(ask_user=True)],
            observers=[observer],
        )
        run = agent.start("ask me")
        pending = await wait_for_interaction(run)
        assert pending, "expected an interaction request"
        await run.cancel()

    asyncio.run(exercise())

    assert observer.delivered >= 1, (observer.delivered, observer.failed)
    assert notifications, "sink should have received the requested event"
    assert any("interaction_id" in item for item in notifications)


def test_notify_policy_bounds_rejected() -> None:
    try:
        finstack_ai.NotifyObserver(lambda payload: None, request_timeout_ms=1)
    except ValueError:
        pass
    else:
        raise AssertionError("expected a ValueError for an out-of-bounds timeout")
