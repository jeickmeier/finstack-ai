"""Golden-questions conformance for the Python knowledge composition.

The same fixture and the same two assertions as
``apps/finstack-knowledge/src/golden.rs``'s tests: the answer contains
every ``must_contain`` needle, and the observed event kinds are a
superset of ``event_kinds_expected``. Offline: the model is scripted.
"""

from __future__ import annotations

import asyncio
import tempfile
from pathlib import Path

import pytest

from _knowledge import build_knowledge_agent, golden_entries, scripted_model


@pytest.mark.parametrize("entry", golden_entries(), ids=lambda entry: entry["id"])
def test_golden_entry_holds_offline(entry: dict[str, object]) -> None:
    async def exercise() -> tuple[str, list[str]]:
        with tempfile.TemporaryDirectory(prefix="finstack-know-golden-") as tmp:
            agent = await build_knowledge_agent(
                Path(tmp),
                scripted_model(
                    [str(entry["scripted_response"])],
                    component=f"knowledge.model.golden-{entry['id']}",
                ),
            )
            run = agent.start(str(entry["question"]))
            kinds: list[str] = []

            async def collect() -> None:
                async for batch in run.events():
                    kinds.extend(event.kind for event in batch.events())

            result, _ = await asyncio.gather(run.result(), collect())
            return result.text, kinds

    answer, kinds = asyncio.run(exercise())
    for needle in entry["must_contain"]:
        assert str(needle) in answer, (
            f"{entry['id']}: answer missing {needle!r}: {answer}"
        )
    for expected in entry["event_kinds_expected"]:
        assert str(expected) in kinds, (
            f"{entry['id']}: expected event kind {expected}; observed {kinds}"
        )
