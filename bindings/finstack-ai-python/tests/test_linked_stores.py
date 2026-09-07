"""Every provider factory exposes the callback factory's explicit storage choices."""

from __future__ import annotations

import asyncio
from pathlib import Path
from typing import Any

import finstack_ai
import pytest

_FACTORIES = [
    ("openai", ("fixture",), {"api_key": "fixture-secret"}),
    ("openrouter", ("fixture/model",), {"api_key": "fixture-secret"}),
    ("anthropic", ("http://127.0.0.1:1", "fixture"), {}),
    ("gemini", ("http://127.0.0.1:1", "fixture"), {}),
    ("ollama", ("http://127.0.0.1:1", "fixture"), {}),
    (
        "gateway",
        ("http://127.0.0.1:1/v1/responses", "fixture"),
        {
            "wire_protocol": "openai_responses",
            "credential_name": "fixture",
            "hard_input_bytes": 1000000,
        },
    ),
]


@pytest.mark.parametrize(("name", "args", "kwargs"), _FACTORIES)
def test_linked_sqlite_reopens_session(
    name: str, args: tuple[str, ...], kwargs: dict[str, Any], tmp_path: Path
) -> None:
    async def exercise() -> None:
        factory = getattr(finstack_ai.Agent, name)
        options = {
            **kwargs,
            "sqlite_path": str(tmp_path / "journal.sqlite"),
            "artifact_path": str(tmp_path / "artifacts"),
        }
        agent = await factory(*args, **options)
        session = await agent.create_session("linked-storage-tenant")
        reopened = await factory(*args, **options)
        restored = await reopened.open_session(
            session.session_id, "linked-storage-tenant"
        )
        assert restored.session_id == session.session_id

    asyncio.run(exercise())


@pytest.mark.parametrize(("name", "args", "kwargs"), _FACTORIES)
def test_linked_store_combinations_fail_before_connection(
    name: str, args: tuple[str, ...], kwargs: dict[str, Any]
) -> None:
    async def exercise() -> None:
        factory = getattr(finstack_ai.Agent, name)
        with pytest.raises(finstack_ai.ConfigurationError, match="mutually exclusive"):
            await factory(
                *args, **kwargs, sqlite_path="unused.sqlite", postgres_dsn="invalid"
            )
        with pytest.raises(
            finstack_ai.ConfigurationError, match="requires sqlite_path"
        ):
            await factory(
                *args, **kwargs, sqlite_durability=finstack_ai.SqliteDurability.Durable
            )

    asyncio.run(exercise())
