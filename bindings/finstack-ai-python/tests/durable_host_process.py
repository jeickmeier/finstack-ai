"""Independent-process fixture for durable admission, approval and crash recovery."""

from __future__ import annotations

import asyncio
import json
import os
import sys
from pathlib import Path
from typing import Any

import finstack_ai
from test_interactions import _write_tool


async def exercise(phase: str, directory: Path) -> None:
    calls = directory / "writes.txt"

    async def model(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context
        has_result = any(
            block["kind"] == "tool_result"
            for message in request["messages"]
            for block in message["content"]
        )
        if has_result:
            return {"text": "durable complete", "completion_id": "durable-completed"}
        return {
            "text": "",
            "completion_id": "durable-write",
            "tool_calls": [{"name": "write", "arguments": {"value": 1}}],
        }

    async def tool(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context
        with calls.open("a") as output:
            output.write("dispatched\n")
            output.flush()
            os.fsync(output.fileno())
        if phase == "crash_tool":
            os._exit(91)
        return {"output": {"ok": True, "value": request["call"]["arguments"]["value"]}}

    async def observer(batch: list[dict[str, Any]]) -> None:
        if phase == "crash_settlement" and any(
            "run_completed" in event.get("body", {}) for event in batch
        ):
            os._exit(92)

    agent = await finstack_ai.Agent.from_python(
        finstack_ai.PythonModel(
            model,
            component="python.model.durable-host",
            provider="scripted",
            model="fixture-1",
        ),
        [
            finstack_ai.PythonToolset(
                tool,
                component="python.tools.durable-host",
                name="writes",
                tools=[_write_tool()],
            )
        ],
        observers=[
            finstack_ai.PythonObserver(
                observer, component="python.observer.durable-host", payload_mode="full"
            )
        ],
    )
    host = await finstack_ai.DurableHost.open(
        str(directory / "host.sqlite"),
        {"approval": agent},
        drive_timeout_seconds=1.0,
        lease_ttl_seconds=2.0,
    )
    if phase == "park":
        locator = await host.start("approval", "write one value", timeout_seconds=120)
        (directory / "locator.json").write_text(json.dumps(locator.to_dict()))
        assert not calls.exists(), "admission must not dispatch"
        tick = await host.tick()
        assert tick["failures"] == 0, tick
        assert tick["sessions_reparked"] == 1, tick
        assert len(host.pending()) == 1
        assert not calls.exists(), "approval must precede dispatch"
        os._exit(0)

    locator = finstack_ai.Locator.from_dict(
        json.loads((directory / "locator.json").read_text())
    )
    if phase in {"uncertain", "missing_descriptor", "descriptor_version"}:
        expected_code = {
            "uncertain": "durable_effect_uncertain",
            "missing_descriptor": "durable_descriptor_missing",
            "descriptor_version": "durable_descriptor_version",
        }[phase]
        try:
            await host.tick()
        except finstack_ai.RuntimeError as error:
            assert error.code == expected_code, error
        else:
            raise AssertionError("unfinished external operation was silently repeated")
        if phase == "uncertain":
            assert calls.read_text().splitlines() == ["dispatched"]
        else:
            assert not calls.exists()
        await host.shutdown()
        return

    if phase != "cleanup":
        pending = host.pending()[0]
        resolution = {
            "resolution_id": "decision-1",
            "principal": pending["principal"],
            "evidence": pending["evidence"],
            "payload": {"approved": phase != "reject"},
        }
        host.resolve(pending["interaction_id"], resolution)
        try:
            host.resolve(pending["interaction_id"], resolution)
        except finstack_ai.RuntimeError as error:
            assert error.code == "not_open", error
        else:
            raise AssertionError("buffered resolution unexpectedly remained open")

    tick = await host.tick()
    assert tick["failures"] == 0, tick
    state = await host.inspect(locator)
    assert state["terminal"], state
    assert not host.pending()
    if phase == "reject":
        assert not calls.exists()
    else:
        assert state["phase"] == "completed", state
        assert calls.read_text().splitlines() == ["dispatched"]
    assert (await host.tick())["sessions_resumed"] == 0
    await host.shutdown()


if __name__ == "__main__":
    asyncio.run(exercise(sys.argv[1], Path(sys.argv[2])))
