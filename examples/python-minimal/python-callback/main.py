"""Minimal trusted Python-callback finstack-ai starter."""

from __future__ import annotations

import asyncio
from typing import Any

import finstack_ai


async def model(
    context: finstack_ai.CallbackContext, request: dict[str, Any]
) -> dict[str, Any]:
    """Return one deterministic offline model completion."""
    assert context.kind == "model"
    assert request["model"] == "starter-model"
    return {"text": "hello from the Rust-owned run loop", "completion_id": "starter-1"}


async def main() -> None:
    """Build and execute one offline callback-backed agent."""
    callback = finstack_ai.PythonModel(
        model,
        component="starter.model.python",
        provider="starter",
        model="starter-model",
    )
    agent = await finstack_ai.Agent.from_python(
        callback,
        instruction="Reply with the scripted completion.",
    )
    result = await agent.run("hello")
    assert "run_accepted" in result.trace
    print(result.text)


if __name__ == "__main__":
    asyncio.run(main())
