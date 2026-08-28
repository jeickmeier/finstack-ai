"""Live Session/Lane handles and identity hooks."""

from __future__ import annotations

import asyncio

import finstack_ai
from test_handles import _agent, _ollama_ndjson, _server


def test_session_create_lane_inspect_and_identity_bind() -> None:
    async def exercise(server: object) -> None:
        agent = await _agent(server)  # type: ignore[arg-type]
        session = await agent.create_session("tenant-a")
        session_snapshot = await agent.inspect_session(session.session_id)
        assert session_snapshot["session_id"] == session.session_id
        assert session_snapshot["head_sequence"] > 0
        assert session_snapshot["phase"] == "in_progress"
        main = await session.lane("main")
        research = await session.create_lane("research")
        lanes = await session.list_lanes()
        assert {lane.lane_id for lane in lanes} == {main.lane_id, research.lane_id}
        inspect = await research.inspect()
        assert inspect["name"] == "research"
        assert inspect["history_len"] == 0
        mapping = finstack_ai.MemoryExternalIdentityMap()
        session.bind_external_identity(
            mapping, "slack", "acct", "thread-1", main.lane_id
        )
        assert mapping.resolve("slack", "acct", "thread-1") == (
            session.session_id,
            main.lane_id,
        )
        opened = await agent.open_session(session.session_id, "tenant-a")
        assert opened.session_id == session.session_id
        assert (await opened.lane("research")).lane_id == research.lane_id
        by_id = await opened.lane_by_id(research.lane_id)
        assert by_id.lane_id == research.lane_id

    with _server(_ollama_ndjson(["ok"])) as server:
        asyncio.run(exercise(server))


def test_lane_append_text_does_not_start_a_run() -> None:
    async def exercise(server: object) -> None:
        agent = await _agent(server)  # type: ignore[arg-type]
        session = await agent.create_session("tenant-a")
        main = await session.lane("main")
        entry = await main.append_text("note only")
        inspect = await main.inspect()
        assert inspect["active_run_id"] is None
        assert inspect["leaf_id"] == entry
        assert inspect["history_len"] == 1

    with _server(_ollama_ndjson(["unused"])) as server:
        asyncio.run(exercise(server))


def test_idle_lane_run_returns_a_live_run() -> None:
    async def exercise(server: object) -> None:
        agent = await _agent(server)  # type: ignore[arg-type]
        session = await agent.create_session("tenant-a")
        main = await session.lane("main")
        run = main.run(agent, "say hello")
        assert run.locator.session_id == session.session_id
        assert run.locator.lane_id == main.lane_id
        result = await run.result()
        assert result.text == "hello"
        session_snapshot = await agent.inspect_session(session.session_id)
        assert session_snapshot["phase"] == "completed"
        assert session_snapshot["result_text"] == "hello"

    with _server(_ollama_ndjson(["hello"])) as server:
        asyncio.run(exercise(server))
