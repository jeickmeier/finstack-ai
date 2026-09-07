"""Offline persistent recipe using the existing knowledge application definition.

Run from the repository: ``uv run python examples/python-notebooks/persistent_assistant.py``.
The same data directory and session are reopened with fresh component handles.
"""

from __future__ import annotations

import asyncio
import json
import tempfile
import threading
from dataclasses import dataclass
from pathlib import Path
from typing import Any

import finstack_ai
from _knowledge import KnowledgeAgent, build_knowledge_agent

SOURCE = "Policy retention is seven years."


@dataclass
class Answer:
    """The knowledge recipe's Rust-validated result schema."""

    answer: str
    sources: list[str]


async def collect(run: finstack_ai.Run) -> set[str]:
    """Drain the single event consumer independently of result observation."""
    kinds: set[str] = set()
    async for batch in run.events():
        kinds.update(event.kind for event in batch.events())
    return kinds


async def definition(
    data_dir: Path, *, cancel: bool = False
) -> tuple[KnowledgeAgent, threading.Event]:
    """Supply the offline model to the existing shared knowledge composition."""
    entered = threading.Event()

    async def model(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        assert SOURCE in json.dumps(request), (
            "durable document bytes did not reach the model"
        )
        entered.set()
        if cancel:
            await context.wait_cancelled()
            raise asyncio.CancelledError
        return {
            "json": {"answer": "seven years", "sources": ["policy.csv"]},
            "completion_id": "knowledge-answer",
        }

    agent = await build_knowledge_agent(
        data_dir,
        finstack_ai.PythonModel(
            model,
            component="knowledge.model.recipe",
            provider="offline",
            model="knowledge-offline",
            context_window_tokens=131_072,
        ),
        tenant="local",
        output_type=Answer,
    )
    return agent, entered


async def exercise(data_dir: Path) -> None:
    """Verify artifact/history reopening, structured results, streaming and cancellation."""
    first, _ = await definition(data_dir)
    session = await first.agent.create_session("local")
    first, _ = await definition(data_dir)
    lane = await session.lane("main")
    run = lane.run(
        first.agent,
        "Read the attached policy.",
        attachments=[
            finstack_ai.Attachment(
                "text/csv", data=f"policy\n{SOURCE}\n".encode(), name="policy.csv"
            )
        ],
    )
    events = asyncio.create_task(collect(run))
    result = await run.result()
    assert result.output == Answer("seven years", ["policy.csv"])
    assert "run_completed" in await events
    maintenance = await first.maintain(256)
    assert not maintenance["failures"]
    assert maintenance["journal_records"] > 0
    session_id = session.session_id
    del run, lane, session, first

    second, _ = await definition(data_dir)
    session = await second.agent.open_session(session_id, "local")
    lane = await session.lane("main")
    run = lane.run(second.agent, "What was the retention policy?")
    events = asyncio.create_task(collect(run))
    assert (await run.result()).output == Answer("seven years", ["policy.csv"])
    assert "run_completed" in await events
    assert not (await second.maintain(256))["failures"]
    del run, lane, session, second

    third, entered = await definition(data_dir, cancel=True)
    session = await third.agent.open_session(session_id, "local")
    lane = await session.lane("main")
    run = lane.run(third.agent, "Wait for explicit cancellation.")
    events = asyncio.create_task(collect(run))
    assert await asyncio.to_thread(entered.wait, 5)
    await run.cancel()
    try:
        await run.result()
    except finstack_ai.CancelledError:
        pass
    else:
        raise AssertionError("explicit cancellation did not settle")
    assert "run_cancelled" in await events
    assert not (await third.maintain(256))["failures"]


async def main() -> None:
    """Keep demo storage temporary; applications choose and retain their own data directory."""
    with tempfile.TemporaryDirectory(prefix="finstack-knowledge-recipe-") as directory:
        await asyncio.wait_for(exercise(Path(directory)), timeout=30)
    print(
        "persistent knowledge: disk reopen, structured output, events, cancellation verified"
    )


if __name__ == "__main__":
    asyncio.run(main())
