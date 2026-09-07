"""Fresh-process evaluation and real admitted-effect crash probes for pytest."""

from __future__ import annotations

import asyncio
import json
import os
import sys
from pathlib import Path

from finstack_ai import eval as ev
from test_eval import agent, spec, usage


async def main(phase: str, directory: Path) -> None:
    calls = 0

    async def model(_context: object, _request: object) -> dict[str, object]:
        nonlocal calls
        calls += 1
        if phase == "crash":
            os._exit(93)  # Actual callback after committed EffectRequested.
        assert phase == "run", "resume/rescore must not execute subjects"
        return {"text": "answer", "completion_id": "restart", "usage": usage()}

    subject = await agent(model, path=directory / "journal.sqlite")
    configuration = spec()
    store = ev.SqliteEvalStore(directory / "eval.sqlite")
    runner = ev.EvalRunner(
        configuration,
        store,
        [ev.SubjectBinding("baseline", subject)],
        [ev.ExactMatchScorer(version=2 if phase.startswith("rescore") else 1)],
    )
    if phase.startswith("rescore"):
        result = await runner.rescore()
    elif phase == "run" or phase == "crash":
        result = await runner.run()
    else:
        result = await runner.resume()
    assert calls == int(phase == "run")
    if phase == "uncertain":
        assert result.stop_reason == "eval_attempt_unresolved"
        assert result.report["counts"]["indeterminate"] == 1
        assert result.report["counts"]["attempts"] == 1
    elif phase == "rescore_missing":
        record = next(iter(result.snapshot["attempts"].values()))[0]
        assert record["scores"][-1]["failure_code"] == "eval_rescore_session_missing"
    else:
        assert result.report["counts"]["completed"] == 1
        assert result.report["counts"]["scoring_failed"] == 0
        assert result.report["spending"]["total_micros"] == {"USD": "7"}
    (directory / f"{phase}.json").write_text(json.dumps(result.report))


if __name__ == "__main__":
    asyncio.run(main(sys.argv[1], Path(sys.argv[2])))
