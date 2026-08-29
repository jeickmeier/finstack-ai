"""P1.4: native `LogObserver` wrapper.

The observer writes one NDJSON line per observed run event; a completed
run's log must include a ``run_completed`` kind. Registration identity is
the crate's declared ``finstack.observer.log`` v0.0.4.
"""

from __future__ import annotations

import asyncio
import json
from pathlib import Path
from typing import Any

import finstack_ai


def _model() -> finstack_ai.PythonModel:
    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, object]:
        del context, request
        return {"text": "acknowledged", "completion_id": "python-log-observer-1"}

    return finstack_ai.PythonModel(
        callback,
        component="python.model.log-observer",
        provider="python-fixture",
        model="python-fixture-model",
    )


def _logged_kinds(path: Path) -> list[str]:
    lines = [line for line in path.read_text().splitlines() if line.strip()]
    events = [json.loads(line) for line in lines]
    kinds: list[str] = []
    for event in events:
        body = event.get("body")
        if isinstance(body, dict) and len(body) == 1:
            kinds.append(next(iter(body)))
        elif isinstance(body, str):
            kinds.append(body)
        elif "kind" in event:
            kinds.append(event["kind"])
    return kinds


def test_run_events_land_in_the_log_file(tmp_path: Path) -> None:
    log_path = tmp_path / "events.ndjson"
    observer = finstack_ai.LogObserver(str(log_path))
    assert observer.component == "finstack.observer.log"

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(_model(), observers=[observer])
        result = await agent.run("say hello")
        assert result.text == "acknowledged"

    asyncio.run(exercise())

    assert log_path.is_file()
    text = log_path.read_text()
    assert "run_completed" in text, text
    # Every line is valid JSON (NDJSON contract).
    for line in text.splitlines():
        if line.strip():
            json.loads(line)


def test_payload_mode_round_trips(tmp_path: Path) -> None:
    """metadata_only omits message text; full includes it."""
    metadata_path = tmp_path / "metadata.ndjson"
    full_path = tmp_path / "full.ndjson"

    async def run_with(observer: finstack_ai.LogObserver) -> None:
        agent = await finstack_ai.Agent.from_python(_model(), observers=[observer])
        await agent.run("UNIQUE_PROMPT_MARKER")

    asyncio.run(run_with(finstack_ai.LogObserver(str(metadata_path))))
    asyncio.run(run_with(finstack_ai.LogObserver(str(full_path), "full")))

    assert "UNIQUE_PROMPT_MARKER" not in metadata_path.read_text()
    assert "UNIQUE_PROMPT_MARKER" in full_path.read_text()


def test_stderr_variant_constructs() -> None:
    observer = finstack_ai.LogObserver.stderr("redacted")
    assert observer.component == "finstack.observer.log"


def test_unsupported_payload_mode_rejected(tmp_path: Path) -> None:
    try:
        finstack_ai.LogObserver(str(tmp_path / "x.ndjson"), "credential")
    except TypeError as error:
        assert "payload_mode" in str(error)
    else:
        raise AssertionError("expected a TypeError for an unsupported mode")
