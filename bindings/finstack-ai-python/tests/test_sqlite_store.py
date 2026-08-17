"""PR-078 SQLite durability, migration, and settlement-idempotency fixtures."""

from __future__ import annotations

import asyncio
import json
import sqlite3
from pathlib import Path
from typing import Any

import pytest

import finstack_ai

from test_interactions import _resolution, _wait_for_interaction, _write_tool

_REPO_ROOT = Path(__file__).resolve().parents[3]
_FIXTURES = _REPO_ROOT / "fixtures/compatibility/python-sqlite/v1"


def _load(relative: str) -> dict[str, Any]:
    return json.loads((_FIXTURES / relative).read_text())


async def _callback_agent(
    *,
    sqlite_path: str,
    durability: finstack_ai.SqliteDurability,
    text: str = "ok",
) -> finstack_ai.Agent:
    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context, request
        return {"text": text, "completion_id": "python-sqlite-1"}

    return await finstack_ai.Agent.from_python(
        finstack_ai.PythonModel(
            callback,
            component="python.model.sqlite",
            provider="python-fixture",
            model="python-fixture-model",
        ),
        instruction="Answer concisely.",
        sqlite_path=sqlite_path,
        sqlite_durability=durability,
    )


def test_sqlite_durability_is_importable() -> None:
    assert finstack_ai.SqliteDurability.Durable != finstack_ai.SqliteDurability.Relaxed
    assert str(finstack_ai.SqliteDurability.Durable) == "Durable"
    assert str(finstack_ai.SqliteDurability.Relaxed) == "Relaxed"


def test_migration_opens_user_version_zero_as_schema_v1(tmp_path: Path) -> None:
    fixture = _load("migration/valid--open-user-version-1.json")
    path = tmp_path / "journal.sqlite"
    connection = sqlite3.connect(path)
    version = connection.execute("PRAGMA user_version").fetchone()[0]
    connection.close()
    assert version == fixture["start_user_version"]

    async def exercise() -> None:
        agent = await _callback_agent(
            sqlite_path=str(path),
            durability=finstack_ai.SqliteDurability.Durable,
        )
        session = await agent.create_session("tenant-a")
        assert session.session_id

    asyncio.run(exercise())
    connection = sqlite3.connect(path)
    version = connection.execute("PRAGMA user_version").fetchone()[0]
    connection.close()
    assert version == fixture["schema_user_version"]


def test_migration_rejects_unsupported_user_version(tmp_path: Path) -> None:
    fixture = _load("migration/invalid--unsupported-user-version.json")
    path = tmp_path / "journal.sqlite"
    connection = sqlite3.connect(path)
    connection.execute(f"PRAGMA user_version = {fixture['user_version']}")
    connection.commit()
    connection.close()

    async def exercise() -> None:
        with pytest.raises(finstack_ai.ConfigurationError) as caught:
            await _callback_agent(
                sqlite_path=str(path),
                durability=finstack_ai.SqliteDurability.Durable,
            )
        assert caught.value.code == fixture["expect_error_code"]
        assert fixture["expect_reason"] in str(caught.value)

    asyncio.run(exercise())


def test_settlement_survives_sqlite_reopen(tmp_path: Path) -> None:
    fixture = _load("settlement/valid--completed-run-restart.json")
    path = tmp_path / "journal.sqlite"
    durability = finstack_ai.SqliteDurability.Relaxed

    async def exercise() -> None:
        agent = await _callback_agent(
            sqlite_path=str(path),
            durability=durability,
            text="hello",
        )
        result = await agent.run("say hello")
        assert result.text == "hello"
        session_id = result.session.session_id
        tenant = result.locator.tenant_scope
        del agent
        reopened_agent = await _callback_agent(
            sqlite_path=str(path),
            durability=durability,
        )
        opened = await reopened_agent.open_session(session_id, tenant)
        inspect = await (await opened.lane("main")).inspect()
        assert inspect["active_run_id"] is None
        assert opened.session_id == session_id
        assert fixture["expect"] == "session_survives_without_active_run"

    asyncio.run(exercise())


def test_duplicate_interaction_resolution_is_idempotent() -> None:
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
                "completion_id": "python-sqlite-write-1",
                "tool_calls": [{"name": "write", "arguments": {"value": 1}}],
            }
        return {"text": "write complete", "completion_id": "python-sqlite-write-2"}

    async def tool_callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context
        return {"output": {"ok": True, "value": request["call"]["arguments"]["value"]}}

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(
            finstack_ai.PythonModel(
                model_callback,
                component="python.model.sqlite-write",
                provider="python-fixture",
                model="python-fixture-model",
            ),
            [
                finstack_ai.PythonToolset(
                    tool_callback,
                    component="python.toolset.sqlite-write",
                    name="python-sqlite-write-tools",
                    tools=[_write_tool()],
                )
            ],
            "Use tools when needed.",
        )
        run = agent.start("write 1")
        pending = await _wait_for_interaction(run)
        resolution = _resolution(pending, True)
        await run.resolve_interaction(resolution)
        await run.resolve_interaction(resolution)
        assert (await run.result()).text == "write complete"

    asyncio.run(exercise())
