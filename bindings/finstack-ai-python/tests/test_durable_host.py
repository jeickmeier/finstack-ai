"""Real process-restart proofs through the Rust-owned Python host API."""

from __future__ import annotations

import sqlite3
import subprocess
import sys
import time
from pathlib import Path

import pytest

SCRIPT = Path(__file__).with_name("durable_host_process.py")


def run_phase(directory: Path, phase: str, expected: int = 0) -> None:
    result = subprocess.run(
        [sys.executable, str(SCRIPT), phase, str(directory)],
        capture_output=True,
        text=True,
        timeout=30,
        check=False,
    )
    assert result.returncode == expected, result.stdout + result.stderr


@pytest.mark.parametrize("decision", ["finish", "reject"])
def test_approval_resumes_in_a_fresh_process(tmp_path: Path, decision: str) -> None:
    run_phase(tmp_path, "park")
    run_phase(tmp_path, decision)
    with sqlite3.connect(tmp_path / "host.sqlite") as connection:
        assert connection.execute(
            "SELECT status FROM finstack_workflow_hitl_inbox"
        ).fetchall() == [("accepted",)]
        assert connection.execute(
            "SELECT count(*) FROM finstack_workflow_worker_wake"
        ).fetchone() == (0,)
        assert connection.execute(
            "SELECT count(*) FROM finstack_workflow_worker_inbox"
        ).fetchone() == (0,)


def test_unresolved_non_idempotent_dispatch_is_never_replaced(tmp_path: Path) -> None:
    run_phase(tmp_path, "park")
    run_phase(tmp_path, "crash_tool", expected=91)
    run_phase(tmp_path, "uncertain")


@pytest.mark.parametrize("fault", ["missing_descriptor", "descriptor_version"])
def test_invalid_recovery_descriptor_fails_before_dispatch(
    tmp_path: Path, fault: str
) -> None:
    run_phase(tmp_path, "park")
    with sqlite3.connect(tmp_path / "host.sqlite") as connection:
        if fault == "missing_descriptor":
            connection.execute("DELETE FROM finstack_workflow_recovery")
        else:
            import json

            descriptor = json.loads(
                connection.execute(
                    "SELECT descriptor FROM finstack_workflow_recovery"
                ).fetchone()[0]
            )
            descriptor["version"] = 999
            connection.execute(
                "UPDATE finstack_workflow_recovery SET descriptor = ?",
                (json.dumps(descriptor).encode(),),
            )
    run_phase(tmp_path, fault)


def test_lost_scheduling_hint_is_reconciled_from_admitted_history(
    tmp_path: Path,
) -> None:
    run_phase(tmp_path, "park")
    # Fault injection removes only a disposable scheduling hint. The journal,
    # accepted authority and immutable host descriptor remain untouched.
    with sqlite3.connect(tmp_path / "host.sqlite") as connection:
        connection.execute("DELETE FROM finstack_workflow_worker_wake")
    run_phase(tmp_path, "finish")


def test_crash_after_completion_replays_cleanup_without_execution(
    tmp_path: Path,
) -> None:
    run_phase(tmp_path, "park")
    run_phase(tmp_path, "crash_settlement", expected=92)
    # The dead process still owns a valid lease. Wait through the configured
    # lease deadline before expecting another process to reclaim its hint.
    with sqlite3.connect(tmp_path / "host.sqlite") as connection:
        expiry = connection.execute(
            "SELECT max(lease_expires_unix_ms) FROM finstack_workflow_worker_wake"
        ).fetchone()[0]
    time.sleep(max(0, expiry / 1000 - time.time()) + 0.02)
    run_phase(tmp_path, "cleanup")
    with sqlite3.connect(tmp_path / "host.sqlite") as connection:
        assert connection.execute(
            "SELECT count(*) FROM finstack_workflow_worker_inbox"
        ).fetchone() == (0,)
        assert connection.execute(
            "SELECT count(*) FROM finstack_workflow_worker_wake"
        ).fetchone() == (0,)


def test_missing_required_artifact_fails_before_model_dispatch(tmp_path: Path) -> None:
    import asyncio
    import shutil
    from typing import Any

    import finstack_ai

    async def exercise() -> None:
        calls = 0

        async def model(
            context: finstack_ai.CallbackContext, request: dict[str, Any]
        ) -> dict[str, str]:
            del context, request
            nonlocal calls
            calls += 1
            return {"text": "done", "completion_id": "artifact-model"}

        artifact_path = tmp_path / "artifacts"
        agent = await finstack_ai.Agent.from_python(
            finstack_ai.PythonModel(
                model,
                component="python.model.host-artifact",
                provider="scripted",
                model="fixture-1",
            ),
            artifact_path=str(artifact_path),
        )
        host = await finstack_ai.DurableHost.open(
            str(tmp_path / "host.sqlite"), {"assistant": agent}
        )
        locator = await host.start(
            "assistant",
            "read attachment",
            attachments=[finstack_ai.Attachment("text/plain", data=b"source evidence")],
        )
        await host.shutdown()
        shutil.rmtree(artifact_path)
        fresh = await finstack_ai.Agent.from_python(
            finstack_ai.PythonModel(
                model,
                component="python.model.host-artifact",
                provider="scripted",
                model="fixture-1",
            ),
            artifact_path=str(artifact_path),
        )
        recovered = await finstack_ai.DurableHost.open(
            str(tmp_path / "host.sqlite"), {"assistant": fresh}
        )
        with pytest.raises(finstack_ai.RuntimeError) as error:
            await recovered.inspect(locator)
        assert error.value.code in {"artifact_not_found", "artifact_unavailable"}
        with pytest.raises(finstack_ai.RuntimeError):
            await recovered.tick()
        assert calls == 0
        await recovered.shutdown()

    asyncio.run(exercise())
