"""Resolve-once Python service starter: health, catalog, one request."""

from __future__ import annotations

import asyncio
from typing import Any

import finstack_ai


async def model(
    context: finstack_ai.CallbackContext, request: dict[str, Any]
) -> dict[str, Any]:
    """Return one deterministic offline model completion."""
    assert context.kind == "model"
    assert request["model"] == "service-model"
    return {"text": "service ready", "completion_id": "service-1"}


async def main() -> None:
    """Resolve once, print health, and handle one offline request."""
    print(finstack_ai.health())
    callback = finstack_ai.PythonModel(
        model,
        component="starter.model.service",
        provider="starter",
        model="service-model",
    )
    agent = await finstack_ai.Agent.from_python(
        callback,
        instruction="Reply with the scripted completion.",
    )
    print(agent.compact_capability_catalog())
    result = await agent.run("Handle this request.")
    print(result.text)


if __name__ == "__main__":
    asyncio.run(main())
