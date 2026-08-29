"""P3.6: native `McpToolset` wrapper over a scripted stdio server double.

The double (`mcp_stdio_double.py`) answers `tools/list` with one `echo`
tool and echoes `tools/call` arguments; the toolset freezes the catalog
at construction and round-trips a scripted call.
"""

from __future__ import annotations

import asyncio
import sys
from pathlib import Path
from typing import Any

import finstack_ai

_DOUBLE = str(Path(__file__).resolve().parent / "mcp_stdio_double.py")


async def _stdio_toolset() -> finstack_ai.McpToolset:
    return await finstack_ai.McpToolset.stdio(
        sys.executable,
        [_DOUBLE],
        read_only_tools=["echo"],
    )


def test_catalog_freezes_and_call_round_trips() -> None:
    captured: list[dict[str, Any]] = []
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
                "completion_id": "python-mcp-1",
                "tool_calls": [{"name": "echo", "arguments": {"text": "MCP_MARKER"}}],
            }
        return {"text": "done", "completion_id": "python-mcp-2"}

    model = finstack_ai.PythonModel(
        callback,
        component="python.model.mcp",
        provider="python-fixture",
        model="python-fixture-model",
    )

    async def exercise() -> None:
        toolset = await _stdio_toolset()
        assert toolset.component == "python.tools.mcp"
        agent = await finstack_ai.Agent.from_python(model, toolsets=[toolset])
        result = await agent.run("echo the marker")
        assert result.text == "done"

    asyncio.run(exercise())

    first_tools = {
        tool.get("model_name") or tool.get("name")
        for tool in captured[0].get("tools", [])
    }
    assert "echo" in first_tools, first_tools

    tool_results = str(
        [
            block
            for message in captured[1]["messages"]
            for block in message["content"]
            if block["kind"] == "tool_result"
        ]
    )
    assert "echo: MCP_MARKER" in tool_results, tool_results


def test_unlisted_program_fails_closed(tmp_path: Path) -> None:
    async def exercise() -> None:
        # The factory allowlists only the program it is given; a config
        # with no bound server or a bad program must fail at construction.
        try:
            await finstack_ai.McpToolset.stdio(str(tmp_path / "missing-server"), [])
        except ValueError:
            pass
        else:
            raise AssertionError("expected construction to fail")

    asyncio.run(exercise())
