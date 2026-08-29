"""P3.3: native `FileSystemToolset` wrapper (T2 — trusted, not sandboxed).

Reads are confined to the explicit root; an escape attempt fails with the
crate's stable policy code, surfaced in the tool result the model sees.
"""

from __future__ import annotations

import asyncio
from pathlib import Path
from typing import Any

import finstack_ai


def _reading_model(
    path: str, captured: list[dict[str, Any]]
) -> finstack_ai.PythonModel:
    calls = 0

    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context
        nonlocal calls
        calls += 1
        captured.append(request)
        if calls == 1:
            return {
                "text": "",
                "completion_id": "python-fs-1",
                "tool_calls": [
                    {"name": "filesystem_read", "arguments": {"path": path}}
                ],
            }
        return {"text": "done", "completion_id": "python-fs-2"}

    return finstack_ai.PythonModel(
        callback,
        component="python.model.filesystem",
        provider="python-fixture",
        model="python-fixture-model",
    )


def _tool_result_text(request: dict[str, Any]) -> str:
    return str(
        [
            block
            for message in request["messages"]
            for block in message["content"]
            if block["kind"] == "tool_result"
        ]
    )


def test_read_confined_to_root(tmp_path: Path) -> None:
    (tmp_path / "notes.txt").write_text("CONFINED_CONTENT\n")
    toolset = finstack_ai.FileSystemToolset(str(tmp_path))
    assert toolset.component == "python.tools.filesystem"

    captured: list[dict[str, Any]] = []

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(
            _reading_model("notes.txt", captured), toolsets=[toolset]
        )
        result = await agent.run("read my notes")
        assert result.text == "done"

    asyncio.run(exercise())

    assert "CONFINED_CONTENT" in _tool_result_text(captured[1])


def test_escape_attempt_fails_closed(tmp_path: Path) -> None:
    root = tmp_path / "root"
    root.mkdir()
    (tmp_path / "secret.txt").write_text("OUTSIDE_SECRET\n")

    captured: list[dict[str, Any]] = []

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(
            _reading_model("../secret.txt", captured),
            toolsets=[finstack_ai.FileSystemToolset(str(root))],
        )
        await agent.run("read the secret")

    asyncio.run(exercise())

    result_text = _tool_result_text(captured[1])
    assert "OUTSIDE_SECRET" not in result_text
    assert "filesystem_" in result_text, result_text


def test_missing_root_rejected(tmp_path: Path) -> None:
    try:
        finstack_ai.FileSystemToolset(str(tmp_path / "missing"))
    except ValueError:
        pass
    else:
        raise AssertionError("expected a ValueError for a missing root")
