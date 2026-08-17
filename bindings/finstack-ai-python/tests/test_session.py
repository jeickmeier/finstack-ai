"""PR-047 live Session/Lane handles and identity hooks."""

from __future__ import annotations

import asyncio

import finstack_ai

from test_handles import _agent, _ollama_ndjson, _server


def test_session_create_lane_inspect_and_identity_bind() -> None:
    async def exercise(server: object) -> None:
        agent = await _agent(server)  # type: ignore[arg-type]
        session = await agent.create_session("tenant-a")
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

    with _server(_ollama_ndjson(["ok"])) as server:
        asyncio.run(exercise(server))
