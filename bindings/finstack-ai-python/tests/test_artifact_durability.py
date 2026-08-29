"""P0.1: durable artifact store option (``artifact_path=``).

`DocumentIngestMiddleware` re-resolves a session's attachment bytes on
every later turn (the journal keeps ``File`` blocks canonical and only the
model-visible request is rewritten), so an agent built over a persisted
``sqlite_path`` journal can only continue a session with attachments when
the artifact store outlives the process. ``artifact_path=`` swaps the
default process-local `InProcessArtifactStore` for the filesystem-backed
`LocalArtifactStore` — the same fix the knowledge CLI ships
(`apps/finstack-knowledge/src/compose.rs::open_artifact_store`).

A second `Agent` instance in this process has a *fresh* default store, so
building "agent B" here is behaviorally identical to a process restart.
"""

from __future__ import annotations

import asyncio
from pathlib import Path
from typing import Any

import finstack_ai

_CSV = b"quarter,revenue\nQ1,1250\nQ2,1600\n"


def _capture_model(
    component: str, captured: list[dict[str, Any]]
) -> finstack_ai.PythonModel:
    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, object]:
        del context
        captured.append(request)
        return {"text": "acknowledged", "completion_id": f"{component}-1"}

    return finstack_ai.PythonModel(
        callback,
        component=component,
        provider="python-fixture",
        model="python-fixture-model",
    )


async def _first_turn(journal: Path, artifacts: Path | None) -> tuple[str, str]:
    """Run turn one with a CSV attachment; return (session_id, tenant)."""
    agent = await finstack_ai.Agent.from_python(
        _capture_model("python.model.durability-a", []),
        sqlite_path=str(journal),
        artifact_path=None if artifacts is None else str(artifacts),
    )
    result = await agent.run(
        "Summarize the attached file.",
        attachments=[
            finstack_ai.Attachment(data=_CSV, media_type="text/csv", name="revenue.csv")
        ],
    )
    assert result.text == "acknowledged"
    return result.session.session_id, result.locator.tenant_scope


def test_artifact_path_survives_agent_restart(tmp_path: Path) -> None:
    """A fresh agent over the same paths re-resolves historical attachments."""
    journal = tmp_path / "journal.sqlite3"
    artifacts = tmp_path / "artifacts"

    async def exercise() -> None:
        session_id, tenant = await _first_turn(journal, artifacts)

        captured: list[dict[str, Any]] = []
        agent_b = await finstack_ai.Agent.from_python(
            _capture_model("python.model.durability-b", captured),
            sqlite_path=str(journal),
            artifact_path=str(artifacts),
        )
        opened = await agent_b.open_session(session_id, tenant)
        lane = await opened.lane("main")
        run = lane.run(agent_b, "And what was Q2 revenue?")
        result = await run.result()
        assert result.text == "acknowledged"

        # The follow-up model request still carries the converted Markdown
        # (re-resolved from the durable store) and never a raw File block.
        assert len(captured) == 1
        blocks = [
            block
            for message in captured[0]["messages"]
            if message["role"] == "user"
            for block in message["content"]
        ]
        assert not any(block["kind"] == "file" for block in blocks), blocks
        text = "".join(block["text"] for block in blocks if block["kind"] == "text")
        assert "Q1" in text and "Q2" in text, text

    asyncio.run(exercise())


def test_default_in_process_store_cannot_cross_agents(tmp_path: Path) -> None:
    """Without ``artifact_path`` the limitation is explicit, not silent.

    Agent B's fresh in-process store cannot resolve the journaled
    attachment, so the follow-up turn fails instead of silently dropping
    the document.
    """
    journal = tmp_path / "journal.sqlite3"

    async def exercise() -> None:
        session_id, tenant = await _first_turn(journal, None)

        agent_b = await finstack_ai.Agent.from_python(
            _capture_model("python.model.durability-c", []),
            sqlite_path=str(journal),
        )
        opened = await agent_b.open_session(session_id, tenant)
        lane = await opened.lane("main")
        run = lane.run(agent_b, "And what was Q2 revenue?")
        try:
            await run.result()
        except Exception as error:  # noqa: BLE001 - binding maps to its own error type
            message = str(error).lower()
            assert "artifact" in message or "middleware" in message, message
        else:
            raise AssertionError(
                "expected the follow-up turn to fail without artifact_path"
            )

    asyncio.run(exercise())


def test_invalid_artifact_path_is_a_configuration_error(tmp_path: Path) -> None:
    """A file (not a directory) as ``artifact_path`` fails at build time."""
    blocker = tmp_path / "not-a-dir"
    blocker.write_bytes(b"x")

    async def exercise() -> None:
        try:
            await finstack_ai.Agent.from_python(
                _capture_model("python.model.durability-d", []),
                artifact_path=str(blocker),
            )
        except Exception as error:  # noqa: BLE001 - binding maps to its own error type
            assert "root_dir" in str(error) or "artifact" in str(error).lower(), str(
                error
            )
        else:
            raise AssertionError("expected a configuration error")

    asyncio.run(exercise())
