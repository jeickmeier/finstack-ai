"""P3.2: `import_skill_markdown` — SKILL.md to declarative Capability."""

from __future__ import annotations

import asyncio
from typing import Any

import finstack_ai

_SKILL = "---\nname: reviewer\ndescription: Review notes\n---\nCite primary sources.\n"


def test_imported_skill_becomes_a_capability() -> None:
    capability = finstack_ai.import_skill_markdown(_SKILL)
    assert capability.id == "skill.reviewer"
    assert capability.description == "Review notes"
    assert capability.activation == "application"


def test_imported_skill_composes_and_instruction_applies() -> None:
    captured: list[dict[str, Any]] = []

    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, object]:
        del context
        captured.append(request)
        return {"text": "acknowledged", "completion_id": "python-skill-import-1"}

    model = finstack_ai.PythonModel(
        callback,
        component="python.model.skill-import",
        provider="python-fixture",
        model="python-fixture-model",
    )

    async def exercise() -> None:
        capability = finstack_ai.import_skill_markdown(_SKILL)
        agent = await finstack_ai.Agent.from_python(
            model,
            capabilities=[capability],
            active_capabilities=["skill.reviewer"],
        )
        result = await agent.run("review this")
        assert result.text == "acknowledged"
        assert any(
            item["id"] == "skill.reviewer" for item in result.active_capabilities
        )

    asyncio.run(exercise())

    text = "".join(
        block["text"]
        for message in captured[0]["messages"]
        for block in message["content"]
        if block["kind"] == "text"
    )
    assert "Cite primary sources." in text, text


def test_missing_frontmatter_rejected() -> None:
    try:
        finstack_ai.import_skill_markdown("no frontmatter here")
    except ValueError:
        pass
    else:
        raise AssertionError("expected a ValueError for missing frontmatter")
