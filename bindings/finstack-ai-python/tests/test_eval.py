"""Actual Python evaluation execution, callback cancellation, budgets and rescoring."""

from __future__ import annotations

import asyncio
import inspect
import json
import threading
from dataclasses import replace
from pathlib import Path
from typing import Any

import finstack_ai as ai
import pytest
from finstack_ai import eval as ev


def usage(cost: int = 7, tokens: int = 10) -> ai.Usage:
    return {
        "input_tokens": tokens,
        "output_tokens": 4,
        "total_tokens": tokens + 4,
        "cost": {"unit": "USD", "micros": str(cost), "pricing_policy_version": "v1"},
    }


async def agent(
    callback: ai._finstack_ai.Callback,
    *,
    path: Path | None = None,
    output_type: object = None,
) -> ai.Agent:
    model = ai.PythonModel(
        callback,
        component="test.model.eval",
        provider="scripted",
        model="eval-1",
        context_window_tokens=1_048_576,
    )
    value = await ai.Agent.from_python(
        model,
        sqlite_path=None if path is None else str(path),
        output_type=output_type,
        document_tools=False,
    )
    return await value.with_limits(
        {
            "max_cost": {
                "unit": "USD",
                "micros": "1000000",
                "pricing_policy_version": "v1",
                "unknown_usage": "allow_within_reserved_maximum",
            }
        }
    )


def spec(
    *, scorers: tuple[str, ...] = ("exact_match",), repetitions: int = 1
) -> ev.EvalSpec:
    return ev.EvalSpec(
        "python-eval",
        [ev.TaskSample("one", "question", "answer")],
        [ev.SubjectDecl("baseline")],
        scorers,
        repetitions=repetitions,
    )


async def answer(_: ai.CallbackContext, __: dict[str, Any]) -> ai.ModelOutput:
    return {"text": "answer", "completion_id": "answer", "usage": usage()}


def test_execution_rescore_budget_and_export(tmp_path: Path) -> None:
    async def exercise() -> None:
        calls = 0

        async def model(
            ctx: ai.CallbackContext, request: dict[str, Any]
        ) -> ai.ModelOutput:
            nonlocal calls
            calls += 1
            return await answer(ctx, request)

        subject = await agent(model, path=tmp_path / "journal.sqlite")
        configuration = spec(repetitions=3)
        configuration = replace(
            configuration,
            limits=ev.EvalLimits(
                max_concurrency=1, budget_micros=10, budget_unit="USD"
            ),
        )
        store = ev.SqliteEvalStore(tmp_path / "eval.sqlite")
        binding = ev.SubjectBinding("baseline", subject)
        runner = ev.EvalRunner(configuration, store, [binding], [ev.ExactMatchScorer()])
        first = await runner.run()
        assert calls == 2
        assert first.stop_reason == "eval_budget_exhausted"
        assert first.report["spending"]["total_micros"] == {"USD": "14"}
        assert first.report["counts"]["unattempted"] == 1
        assert first.report["aggregates"][0]["scored_cells"] == 2
        assert (
            len(
                {
                    r[0]["execution"]["session_id"]
                    for r in first.snapshot["reservations"].values()
                }
            )
            == 2
        )
        # A new version changes only scorer behavior, not frozen spec or subject locks.
        rescoring = ev.EvalRunner(
            configuration, store, [binding], [ev.ExactMatchScorer(version=2)]
        )
        result = await rescoring.rescore()
        assert calls == 2
        assert all(
            len(records[0]["scores"]) == 2
            for records in result.snapshot["attempts"].values()
        )
        result.export_jsonl(tmp_path / "attempts.jsonl")
        result.write_summary(tmp_path / "summary.json")
        rows = [
            json.loads(line)
            for line in (tmp_path / "attempts.jsonl").read_text().splitlines()
        ]
        assert len(rows) == 2
        assert rows[0]["usage"]["cost"]["micros"] == "7"
        assert "target" not in rows[0] and "input" not in rows[0]
        assert (
            json.loads((tmp_path / "summary.json").read_text())["counts"]
            == result.report["counts"]
        )
        gate = ev.ThresholdGate(
            "baseline",
            {"scorer": "exact_match", "scorer_version": 2, "name": "exact_match"},
            minimum_mean=1_000_000,
        )
        assert result.evaluate(gate)["incomplete"]
        assert not result.evaluate(gate)["passed"]
        assert store.snapshot().report == result.report
        with pytest.raises(ai.FinstackError) as failure:
            await ev.EvalRunner(
                replace(configuration, repetitions=2),
                store,
                [binding],
                [ev.ExactMatchScorer()],
            ).run()
        assert failure.value.code == "eval_spec_diverged"

    asyncio.run(exercise())


def test_python_preparation_and_score_failures_do_not_repeat_subjects() -> None:
    async def exercise() -> None:
        calls = 0
        preparations = 0

        async def model(
            _: ai.CallbackContext, request: dict[str, Any]
        ) -> ai.ModelOutput:
            nonlocal calls
            calls += 1
            assert "prepared question" in json.dumps(request)
            return {"text": "answer", "completion_id": "custom", "usage": usage()}

        async def prepare(context: ev.PreparationContext) -> ev.PreparedRequest:
            nonlocal preparations
            preparations += 1
            assert context["input"] == "question"
            assert "target" not in context
            return {"input": "prepared question", "max_cycles": 2}

        async def fail(context: ev.ScoreContext) -> list[ev.Score]:
            assert context["output"]["text"] == "answer"
            raise ValueError("SECRET_CALLBACK_ERROR")

        subject = await agent(model)
        store = ev.MemoryEvalStore()
        configuration = spec(scorers=("custom",))
        binding = ev.SubjectBinding("baseline", subject, prepare=prepare)
        runner = ev.EvalRunner(
            configuration, store, [binding], [ev.PythonScorer("custom", 1, fail)]
        )
        result = await runner.run()
        assert result.report["counts"]["completed"] == 1
        assert result.report["counts"]["scoring_failed"] == 1
        assert "SECRET" not in json.dumps(result.snapshot)
        assert (await runner.resume()).snapshot == result.snapshot
        assert calls == preparations == 1

        def repaired(context: ev.ScoreContext) -> list[ev.Score]:
            assert context["output"] is not None
            return [
                {
                    "scorer": "custom",
                    "scorer_version": 2,
                    "name": "grade",
                    "value": 1_000_000,
                    "passed": True,
                    "explanation": None,
                    "metadata": None,
                }
            ]

        rescored = await ev.EvalRunner(
            configuration, store, [binding], [ev.PythonScorer("custom", 2, repaired)]
        ).rescore()
        assert rescored.report["counts"]["scoring_failed"] == 0
        assert calls == preparations == 1
        assert rescored.report["aggregates"][0]["statistics"]["mean"] == "1"

    asyncio.run(exercise())


@pytest.mark.parametrize("phase", ["prepare", "score"])
def test_callback_timeout_cancels_the_actual_asyncio_task(phase: str) -> None:
    async def exercise() -> None:
        entered, cancelled = threading.Event(), threading.Event()

        async def blocked(_: object) -> object:
            entered.set()
            try:
                await asyncio.sleep(60)
            finally:
                cancelled.set()

        subject = await agent(answer)
        configuration = replace(
            spec(scorers=("custom",) if phase == "score" else ("exact_match",)),
            limits=ev.EvalLimits(max_replacement_attempts=0, attempt_timeout_ms=1000),
        )
        binding = ev.SubjectBinding(
            "baseline",
            subject,
            prepare=blocked if phase == "prepare" else None,
            callback_timeout_seconds=0.05,
        )
        scorer = (
            ev.PythonScorer("custom", 1, blocked, callback_timeout_seconds=0.05)
            if phase == "score"
            else ev.ExactMatchScorer()
        )
        result = await ev.EvalRunner(
            configuration, ev.MemoryEvalStore(), [binding], [scorer]
        ).run()
        assert entered.is_set()
        assert await asyncio.to_thread(cancelled.wait, 2), (
            "the actual Python task must observe cancellation"
        )
        if phase == "prepare":
            assert result.report["counts"]["infrastructure_failed"] == 1
        else:
            assert result.report["counts"]["scoring_failed"] == 1
            assert result.report["counts"]["completed"] == 1

    asyncio.run(exercise())


def test_explicit_cancellation_settles_subject_after_python_await_is_cancelled() -> (
    None
):
    async def exercise() -> None:
        entered, cancelled = threading.Event(), threading.Event()

        async def model(ctx: ai.CallbackContext, _: object) -> dict[str, str]:
            entered.set()
            try:
                await ctx.wait_cancelled()
                return {"text": "cancelled", "completion_id": "cancel"}
            finally:
                cancelled.set()

        subject = await agent(model)
        store = ev.MemoryEvalStore()
        configuration = spec()
        binding = ev.SubjectBinding("baseline", subject)
        runner = ev.EvalRunner(configuration, store, [binding], [ev.ExactMatchScorer()])
        task = asyncio.create_task(runner.run())
        assert await asyncio.to_thread(entered.wait, 2)
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        with pytest.raises(ai.FinstackError) as busy:
            await ev.EvalRunner(
                configuration, store, [binding], [ev.ExactMatchScorer()]
            ).resume()
        assert busy.value.code == "eval_runner_busy"
        runner.cancel()
        assert await asyncio.to_thread(cancelled.wait, 2)

        # Await exclusive ownership release, without dispatching another subject.
        async def settled() -> ev.EvalRunReport:
            while True:
                try:
                    return await ev.EvalRunner(
                        configuration, store, [binding], [ev.ExactMatchScorer()]
                    ).resume()
                except ai.FinstackError as error:
                    assert error.code == "eval_runner_busy"
                    await asyncio.sleep(0)

        result = await asyncio.wait_for(settled(), 3)
        assert result.report["counts"]["subject_failed"] == 1

    asyncio.run(exercise())


def test_builtin_configuration_validation_and_ide_surface() -> None:
    ev.IncludesScorer()
    ev.RegexScorer("answer")
    ev.NumericToleranceScorer(ev.ToleranceBands(full_within_ppm=1000, absolute="USD 2"))
    ev.StructuredFieldScorer(
        [ev.FieldSpec("/amount", ev.FieldTolerance("numeric_ppm", 1000))]
    )
    ev.RecordKindsScorer(required=["run_completed"])
    for build in [
        lambda: ev.RegexScorer("["),
        lambda: ev.ExactMatchScorer(version=0),
        lambda: ev.StructuredFieldScorer([]),
        lambda: spec(repetitions=0).digest(),
    ]:
        with pytest.raises(ai.FinstackError):
            build()
    assert "rescore" in inspect.getdoc(ev.EvalRunner)
    assert "subject" in inspect.getdoc(ev.EvalRunner.rescore)
    assert "prepare" in inspect.signature(ev.SubjectBinding).parameters
    assert "max_cost" in ai.RunLimits.__annotations__
    assert all(hasattr(ev, name) for name in ev.__all__)


def test_judge_rescoring_only_calls_graders_and_preserves_failed_subject_output() -> (
    None
):
    async def exercise() -> None:
        subject_calls = grader_calls = 0

        async def model(
            ctx: ai.CallbackContext, request: dict[str, Any]
        ) -> ai.ModelOutput:
            nonlocal subject_calls
            subject_calls += 1
            return await answer(ctx, request)

        async def grade(
            _: ai.CallbackContext, request: dict[str, Any]
        ) -> ai.ModelOutput:
            nonlocal grader_calls
            grader_calls += 1
            assert all(
                tool["model_name"] == "finstack.internal.submit_final_output"
                for tool in request["tools"]
            )
            return {
                "json": {"choice": "correct"},
                "completion_id": "grade",
                "usage": usage(5),
            }

        subject, grader = await agent(model), await agent(grade)
        judge = ev.JudgeScorer(
            grader,
            ev.JudgeRubric(
                "Compare with reference", {"correct": 1_000_000, "incorrect": 0}
            ),
        )
        runner = ev.EvalRunner(
            spec(scorers=("judge",)),
            ev.MemoryEvalStore(),
            [ev.SubjectBinding("baseline", subject)],
            [judge],
        )
        initial = await runner.run()
        assert initial.report["spending"]["total_micros"] == {"USD": "12"}
        rescored = await runner.rescore()
        assert subject_calls == 1 and grader_calls == 2
        assert rescored.report["spending"]["grader_micros"] == {"USD": "10"}
        assert rescored.report["aggregates"][0]["statistics"]["mean"] == "1"

        async def broken(_: ai.CallbackContext, __: object) -> object:
            raise ValueError("unavailable")

        observed: list[object] = []

        def score(context: ev.ScoreContext) -> list[ev.Score]:
            observed.append(context["output"])
            return [
                {
                    "scorer": "custom",
                    "scorer_version": 1,
                    "name": "grade",
                    "value": 0,
                    "passed": False,
                    "explanation": None,
                    "metadata": None,
                }
            ]

        failed = await agent(broken)
        result = await ev.EvalRunner(
            spec(scorers=("custom",)),
            ev.MemoryEvalStore(),
            [ev.SubjectBinding("baseline", failed)],
            [ev.PythonScorer("custom", 1, score)],
        ).run()
        assert observed == [None]
        assert result.report["counts"]["subject_failed"] == 1
        assert result.report["counts"]["scoring_failed"] == 0

    asyncio.run(exercise())


def test_structured_and_numeric_grading_use_rust_arithmetic_and_keep_schema_with_limits() -> (
    None
):
    import pydantic

    class Amount(pydantic.BaseModel):
        amount: str

    async def exercise() -> None:
        async def structured(_: ai.CallbackContext, __: object) -> ai.ModelOutput:
            return {
                "json": {"amount": "USD 1,200,000"},
                "completion_id": "structured",
                "usage": usage(),
            }

        subject = await agent(structured, output_type=pydantic.TypeAdapter(Amount))
        configuration = replace(
            spec(scorers=("structured_field",)),
            tasks=[ev.TaskSample("one", "amount", '{"amount":"$1.2m"}')],
        )
        result = await ev.EvalRunner(
            configuration,
            ev.MemoryEvalStore(),
            [ev.SubjectBinding("baseline", subject)],
            [
                ev.StructuredFieldScorer(
                    [ev.FieldSpec("/amount", ev.FieldTolerance("numeric_ppm", 0))]
                )
            ],
        ).run()
        assert result.report["counts"]["completed"] == 1
        assert all(a["statistics"]["mean"] == "1" for a in result.report["aggregates"])

        async def numeric(_: ai.CallbackContext, __: object) -> ai.ModelOutput:
            return {"text": "119.7 bps", "completion_id": "numeric", "usage": usage()}

        subject = await agent(numeric)
        configuration = replace(
            spec(scorers=("numeric_tolerance",)),
            tasks=[ev.TaskSample("one", "rate", "1.197%")],
        )
        result = await ev.EvalRunner(
            configuration,
            ev.MemoryEvalStore(),
            [ev.SubjectBinding("baseline", subject)],
            [ev.NumericToleranceScorer(ev.ToleranceBands())],
        ).run()
        assert result.report["aggregates"][0]["statistics"]["mean"] == "1"

    asyncio.run(exercise())


@pytest.mark.parametrize("failing", [False, True])
def test_actual_python_execution_matches_rust_fixture_and_rescore(
    failing: bool,
) -> None:
    directory = (
        Path(__file__).resolve().parents[3] / "fixtures/compatibility/eval/parity/v1"
    )
    wire = json.loads((directory / "spec.json").read_text())
    expected = json.loads(
        (directory / ("failed.json" if failing else "completed.json")).read_text()
    )

    async def exercise() -> None:
        calls = {"baseline": 0, "candidate": 0}

        async def build(subject: str) -> ai.Agent:
            async def model(
                _: ai.CallbackContext, request: dict[str, Any]
            ) -> ai.ModelOutput:
                calls[subject] += 1
                candidate = subject == "candidate"
                if failing and candidate and '"two"' in json.dumps(request):
                    raise ValueError("fixture failure")
                return {
                    "text": "answer" if candidate else "wrong",
                    "completion_id": "fixture",
                    "usage": usage(7 if candidate else 10, 8 if candidate else 10),
                }

            return await agent(model)

        configuration = ev.EvalSpec(
            wire["name"],
            [ev.TaskSample(**sample) for sample in wire["tasks"]],
            [ev.SubjectDecl(**subject) for subject in wire["subjects"]],
            wire["scorers"],
            repetitions=wire["repetitions"],
            reducer=ev.RepetitionReducer(**wire["reducer"]),
            limits=ev.EvalLimits(**wire["limits"]),
        )
        bindings = [ev.SubjectBinding(name, await build(name)) for name in calls]
        store = ev.MemoryEvalStore()
        runner = ev.EvalRunner(configuration, store, bindings, [ev.ExactMatchScorer()])
        result = await runner.run()
        normalized = dict(result.report)
        for key in ("spec_digest", "engine_version", "subject_locks"):
            normalized.pop(key)
        assert normalized == expected
        assert result.report["spec_digest"] == configuration.digest()
        result = await ev.EvalRunner(
            configuration, store, bindings, [ev.ExactMatchScorer(version=2)]
        ).rescore()
        normalized = dict(result.report)
        for key in ("spec_digest", "engine_version", "subject_locks"):
            normalized.pop(key)
        for aggregate in normalized["aggregates"]:
            assert aggregate["metric"]["scorer_version"] == 2
            aggregate["metric"]["scorer_version"] = 1
        for comparison in normalized["comparisons"]:
            for metric in comparison["metrics"]:
                metric["metric"]["scorer_version"] = 1
        assert normalized == expected
        assert calls == {"baseline": 4, "candidate": 4}

    asyncio.run(exercise())


def test_fresh_python_processes_resume_and_rescore_without_subject_calls(
    tmp_path: Path,
) -> None:
    import subprocess
    import sys

    script = Path(__file__).with_name("eval_restart_process.py")

    def phase(name: str, expected: int = 0) -> None:
        result = subprocess.run(
            [sys.executable, str(script), name, str(tmp_path)],
            capture_output=True,
            text=True,
            timeout=20,
            check=False,
        )
        assert result.returncode == expected, result.stdout + result.stderr

    phase("run")
    phase("resume")
    phase("rescore")
    (tmp_path / "journal.sqlite").unlink()
    phase("rescore_missing")


def test_fresh_python_process_preserves_an_unresolved_admitted_effect(
    tmp_path: Path,
) -> None:
    import subprocess
    import sys

    script = Path(__file__).with_name("eval_restart_process.py")
    for phase, expected in [("crash", 93), ("uncertain", 0), ("uncertain", 0)]:
        result = subprocess.run(
            [sys.executable, str(script), phase, str(tmp_path)],
            capture_output=True,
            text=True,
            timeout=20,
            check=False,
        )
        assert result.returncode == expected, result.stdout + result.stderr
