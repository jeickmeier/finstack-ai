"""P4.1: postgres journal option (`postgres_dsn=`).

Without a live server the tests pin the construction contract: the DSN is
explicit, mutually exclusive with sqlite, and invalid DSNs fail with the
store's stable code. A live round trip runs only when
FINSTACK_TEST_POSTGRES_DSN is set (mirroring the store crate's own
gating).
"""

from __future__ import annotations

import asyncio
import os
from typing import Any

import pytest

import finstack_ai

_LIVE_DSN = os.environ.get("FINSTACK_TEST_POSTGRES_DSN")


def _model() -> finstack_ai.PythonModel:
    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, object]:
        del context, request
        return {"text": "acknowledged", "completion_id": "python-postgres-1"}

    return finstack_ai.PythonModel(
        callback,
        component="python.model.postgres",
        provider="python-fixture",
        model="python-fixture-model",
    )


def test_invalid_dsn_is_a_configuration_error() -> None:
    async def exercise() -> None:
        try:
            await finstack_ai.Agent.from_python(
                _model(), postgres_dsn="not a postgres url"
            )
        except finstack_ai.ConfigurationError as error:
            assert "postgres" in str(error).lower(), str(error)
        else:
            raise AssertionError("expected a configuration error")

    asyncio.run(exercise())


def test_postgres_dsn_mutually_exclusive_with_sqlite(tmp_path: Any) -> None:
    async def exercise() -> None:
        try:
            await finstack_ai.Agent.from_python(
                _model(),
                sqlite_path=str(tmp_path / "journal.sqlite3"),
                postgres_dsn="postgres://localhost/db",
            )
        except finstack_ai.ConfigurationError as error:
            assert "mutually exclusive" in str(error), str(error)
        else:
            raise AssertionError("expected mutual-exclusion rejection")

    asyncio.run(exercise())


@pytest.mark.skipif(_LIVE_DSN is None, reason="FINSTACK_TEST_POSTGRES_DSN not set")
def test_live_postgres_round_trip() -> None:
    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(_model(), postgres_dsn=_LIVE_DSN)
        result = await agent.run("hello")
        assert result.text == "acknowledged"
        opened = await agent.open_session(
            result.session.session_id, result.locator.tenant_scope
        )
        assert opened.session_id == result.session.session_id

    asyncio.run(exercise())
