"""P1.3: native `RepositoryContextProvider` wrapper.

The provider reads allowlisted instruction files under one explicit root
and contributes them as context items, so a capture model must see the
file text in the model-visible request. Registration identity is the
crate's declared ``finstack.context.repository`` v0.0.4.
"""

from __future__ import annotations

import asyncio
from pathlib import Path
from typing import Any

import finstack_ai


def _capture_model(captured: list[dict[str, Any]]) -> finstack_ai.PythonModel:
    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, object]:
        del context
        captured.append(request)
        return {"text": "acknowledged", "completion_id": "python-repository-1"}

    return finstack_ai.PythonModel(
        callback,
        component="python.model.repository",
        provider="python-fixture",
        model="python-fixture-model",
    )


def _all_text(request: dict[str, Any]) -> str:
    return "".join(
        block["text"]
        for message in request["messages"]
        for block in message["content"]
        if block["kind"] == "text"
    )


def test_default_allowlist_contributes_agents_md(tmp_path: Path) -> None:
    (tmp_path / "AGENTS.md").write_text("Repository rule: cite every claim.\n")

    provider = finstack_ai.RepositoryContextProvider(str(tmp_path))
    assert provider.component == "finstack.context.repository"

    captured: list[dict[str, Any]] = []

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(
            _capture_model(captured),
            context_providers=[provider],
        )
        result = await agent.run("What are the rules?")
        assert result.text == "acknowledged"

    asyncio.run(exercise())

    assert "Repository rule: cite every claim." in _all_text(captured[0])


def test_explicit_allowlist_reads_only_named_files(tmp_path: Path) -> None:
    (tmp_path / "docs.md").write_text("Included documentation line.\n")
    (tmp_path / "secret.md").write_text("EXCLUDED_MARKER\n")

    captured: list[dict[str, Any]] = []

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(
            _capture_model(captured),
            context_providers=[
                finstack_ai.RepositoryContextProvider(
                    str(tmp_path), allowlist=["docs.md"]
                )
            ],
        )
        await agent.run("What is documented?")

    asyncio.run(exercise())

    text = _all_text(captured[0])
    assert "Included documentation line." in text
    assert "EXCLUDED_MARKER" not in text


def test_traversing_allowlist_name_rejected(tmp_path: Path) -> None:
    try:
        finstack_ai.RepositoryContextProvider(str(tmp_path), allowlist=["../escape.md"])
    except ValueError:
        pass
    else:
        raise AssertionError("expected a ValueError for a traversing name")


def test_nonexistent_root_rejected(tmp_path: Path) -> None:
    try:
        finstack_ai.RepositoryContextProvider(str(tmp_path / "missing"))
    except ValueError:
        pass
    else:
        raise AssertionError("expected a ValueError for a missing root")
