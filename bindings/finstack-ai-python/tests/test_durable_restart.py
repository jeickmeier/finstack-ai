"""PR-048 Python durable-restart subset (N1–N2, I1 inspect, A1, completed open).

``Agent.open_session`` still inspects only. ``Lane.resume`` respawns the
parked owner after E3/E4a. Child-run and ``complete_external`` routing
stay PR-079.
"""

from __future__ import annotations

import asyncio
from typing import Any

import finstack_ai

from test_handles import _agent, _ollama_ndjson, _server
from test_interactions import _resolution, _wait_for_interaction, _write_tool


def test_session_and_lane_survive_open_after_drop() -> None:
    async def exercise(server: object) -> None:
        agent = await _agent(server)  # type: ignore[arg-type]
        session = await agent.create_session("tenant-a")
        main = await session.lane("main")
        research = await session.create_lane("research")
        session_id = session.session_id
        research_id = research.lane_id
        del session
        opened = await agent.open_session(session_id, "tenant-a")
        assert opened.session_id == session_id
        lanes = {lane.lane_id for lane in await opened.list_lanes()}
        assert main.lane_id in lanes
        assert research_id in lanes
        inspect = await (await opened.lane("research")).inspect()
        assert inspect["name"] == "research"

    with _server(_ollama_ndjson(["ok"])) as server:
        asyncio.run(exercise(server))


def test_completed_run_inspects_after_open_session() -> None:
    async def exercise(server: object) -> None:
        agent = await _agent(server)  # type: ignore[arg-type]
        run = agent.start("say hello")
        result = await run.result()
        assert result.text == "hello"
        opened = await agent.open_session(
            result.session.session_id, result.locator.tenant_scope
        )
        assert opened.session_id == result.session.session_id

    with _server(_ollama_ndjson(["hello"])) as server:
        asyncio.run(exercise(server))


def test_awaiting_interaction_lane_survives_open_after_drop() -> None:
    async def model_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context, request
        return {
            "text": "",
            "completion_id": "python-restart-write-1",
            "tool_calls": [{"name": "write", "arguments": {"value": 1}}],
        }

    async def tool_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context
        return {"output": {"ok": True, "value": request["call"]["arguments"]["value"]}}

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(
            finstack_ai.PythonModel(
                model_callback,
                component="python.model.restart-write",
                provider="python-fixture",
                model="python-fixture-model",
            ),
            [
                finstack_ai.PythonToolset(
                    tool_callback,
                    component="python.toolset.restart-write",
                    name="python-restart-write-tools",
                    tools=[_write_tool()],
                )
            ],
            "Use tools when needed.",
        )
        run = agent.start("write 1")
        pending = await _wait_for_interaction(run)
        assert pending["kind"] == {"kind": "approval"}
        session_id = run.locator.session_id
        tenant = run.locator.tenant_scope
        run_id = run.locator.run_id
        del run
        opened = await agent.open_session(session_id, tenant)
        lane = await opened.lane("main")
        inspect = await lane.inspect()
        assert inspect["active_run_id"] == run_id
        await lane.resume(agent)
        inspect = await lane.inspect()
        assert inspect["active_run_id"] == run_id
        assert _resolution(pending, True)["interaction_id"] == pending["interaction_id"]

    asyncio.run(exercise())


def test_resume_respawns_parked_run_and_completes() -> None:
    model_calls = 0

    async def model_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context, request
        nonlocal model_calls
        model_calls += 1
        if model_calls == 1:
            return {
                "text": "",
                "completion_id": "python-resume-write-1",
                "tool_calls": [{"name": "write", "arguments": {"value": 1}}],
            }
        return {"text": "write complete", "completion_id": "python-resume-write-2"}

    async def tool_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context
        return {"output": {"ok": True, "value": request["call"]["arguments"]["value"]}}

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(
            finstack_ai.PythonModel(
                model_callback,
                component="python.model.resume-write",
                provider="python-fixture",
                model="python-fixture-model",
            ),
            [
                finstack_ai.PythonToolset(
                    tool_callback,
                    component="python.toolset.resume-write",
                    name="python-resume-write-tools",
                    tools=[_write_tool()],
                )
            ],
            "Use tools when needed.",
        )
        run = agent.start("write 1")
        pending = await _wait_for_interaction(run)
        lane = await run.session.lane("main")
        await lane.suspend()
        await lane.resume(agent)
        await run.resolve_interaction(_resolution(pending, True))
        assert (await run.result()).text == "write complete"

    asyncio.run(exercise())


def test_activated_capabilities_remain_on_completed_result_after_open() -> None:
    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context, request
        return {"text": "ok", "completion_id": "python-restart-cap-1"}

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(
            finstack_ai.PythonModel(
                callback,
                component="python.model.restart-cap",
                provider="python-fixture",
                model="python-capability-model",
            ),
            instruction="Stable prefix.",
            capabilities=[
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
            ],
            active_capabilities=["python.capability.application"],
        )
        result = await agent.run("say hello")
        assert result.active_capabilities == [
            {"id": "python.capability.always", "source": "always"},
            {"id": "python.capability.application", "source": "application"},
        ]
        opened = await agent.open_session(
            result.session.session_id, result.locator.tenant_scope
        )
        assert opened.session_id == result.session.session_id
        assert result.text == "ok"

    asyncio.run(exercise())
