"""Offline supervisor with two specialists and Rust-owned child execution.

Run: ``uv run python examples/python-notebooks/supervisor.py``.
"""

from __future__ import annotations

import asyncio
import concurrent.futures
import threading
from collections.abc import Awaitable, Callable
from typing import Any

import finstack_ai


async def drain(run: finstack_ai.Run) -> None:
    """Keep event delivery independent from waiting for results."""
    async for _ in run.events():
        pass


async def definition(
    name: str,
    answer: str | None,
    *,
    cancel: bool,
    summary: concurrent.futures.Future[str],
    observer: Callable[[list[dict[str, Any]]], Awaitable[None]],
) -> tuple[finstack_ai.Agent, threading.Event]:
    """Build one offline agent; ``answer=None`` is the supervisor awaiting ``summary``."""
    entered = threading.Event()

    async def model(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del request
        entered.set()
        if cancel:
            await context.wait_cancelled()
            raise asyncio.CancelledError
        text = await asyncio.wrap_future(summary) if answer is None else answer
        return {"text": text, "completion_id": name}

    agent = await finstack_ai.Agent.from_python(
        finstack_ai.PythonModel(
            model,
            component=f"recipe.model.{name}",
            provider="offline",
            model="specialist",
        ),
        child_runs=finstack_ai.ChildRunPolicy.allow(1)
        if answer is None
        else finstack_ai.ChildRunPolicy.deny(),
        observers=[
            finstack_ai.PythonObserver(
                observer, component="recipe.observer.lineage", payload_mode="full"
            )
        ],
    )
    return agent, entered


async def expect_cancelled(runs: list[finstack_ai.Run]) -> None:
    """Require every run to settle as cancelled."""
    for run in runs:
        try:
            await run.result()
        except finstack_ai.CancelledError:
            pass
        else:
            raise AssertionError("parent cancellation did not settle every child")


async def expect_combined(
    parent: finstack_ai.Run,
    children: list[finstack_ai.Run],
    summary: concurrent.futures.Future[str],
) -> None:
    """Require both specialist results, then the parent's combined summary."""
    results = await asyncio.gather(*(child.result() for child in children))
    assert [result.text for result in results] == [
        "retention: seven years",
        "risk: annual review",
    ]
    combined = "; ".join(result.text for result in results)
    summary.set_result(combined)
    assert (await parent.result()).text == combined


def assert_lineage(
    events: list[dict[str, Any]],
    parent: finstack_ai.Run,
    children: list[finstack_ai.Run],
) -> None:
    """Require committed parent/child lineage for every admitted child."""
    accepted = {
        event["run_id"]: event["body"]["run_accepted"]
        for event in events
        if "run_accepted" in (event.get("body") or {})
    }
    assert len(accepted) == 3
    for child in children:
        relation = accepted[child.locator.run_id]["relation"]
        assert relation["parent_run_id"] == parent.locator.run_id
        assert relation["root_run_id"] == parent.locator.run_id
        assert relation["depth"] == 1 and relation["parent_effect_id"]
        assert child.locator.session_id != parent.locator.session_id
        print(f"lineage: {parent.locator.run_id} -> {child.locator.run_id}")


async def exercise(cancel: bool) -> None:
    """Collect two results, or cancel all admitted children through the parent."""
    events: list[dict[str, Any]] = []
    summary: concurrent.futures.Future[str] = concurrent.futures.Future()

    async def observer(batch: list[dict[str, Any]]) -> None:
        events.extend(batch)

    supervisor, started = await definition(
        "supervisor", None, cancel=cancel, summary=summary, observer=observer
    )
    parent = supervisor.start(
        "Collect two specialist results.", timeout_seconds=10, max_cycles=1
    )
    streams = [asyncio.create_task(drain(parent))]
    assert await asyncio.to_thread(started.wait, 5)
    children = []
    for name, answer in [
        ("policy", "retention: seven years"),
        ("risk", "risk: annual review"),
    ]:
        specialist, entered = await definition(
            name, answer, cancel=cancel, summary=summary, observer=observer
        )
        child = await parent.start_child(
            specialist,
            f"Assess {name}.",
            placement="isolated_child_session",
            timeout_seconds=10,
            max_cycles=1,
        )
        streams.append(asyncio.create_task(drain(child)))
        children.append(child)
        assert await asyncio.to_thread(entered.wait, 5)

    if cancel:
        await parent.cancel()
        await expect_cancelled([parent, *children])
    else:
        await expect_combined(parent, children, summary)
    await asyncio.gather(*streams)
    assert_lineage(events, parent, children)


async def main() -> None:
    """Use finite local fixtures and join all run/event tasks before exit."""
    await asyncio.wait_for(exercise(False), timeout=20)
    await asyncio.wait_for(exercise(True), timeout=20)
    print(
        "supervisor: two results, committed lineage, bounded children, cancellation verified"
    )


if __name__ == "__main__":
    asyncio.run(main())
