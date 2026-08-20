"""`Agent.read_artifact` resolves a tool's staged output back into bytes.

A tool that produces more bytes than it may put in a result stages them
and returns an `ArtifactRef` instead, so the bytes never reach the model
and, without this accessor, never reach Python either. The OpenRouter
media tools do this for every generated image and audio clip; notebook
`examples/python-notebooks/09_openrouter.ipynb` exercises that live path
end to end, which needs network and a funded key. These tests cover the
two failure modes that do not.
"""

from __future__ import annotations

import asyncio

import pytest

import finstack_ai

DIGEST = "0" * 64

# The shape a staged tool artifact actually has, taken from an
# `openrouter_generate_image` result, with a digest nothing was ever
# staged under.
ABSENT_ARTIFACT = {
    "id": "00000000-0000-7000-8000-000000000000",
    "kind": "tool-output",
    "blob": {
        "id": DIGEST,
        "digest": DIGEST,
        "length": 8,
        "media_type": "image/jpeg",
        "name": "openrouter-image",
    },
    "content_digest": DIGEST,
    "scope_digest": DIGEST,
    "metadata": {},
}


async def _agent() -> finstack_ai.Agent:
    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, object]
    ) -> dict[str, object]:
        del context, request
        return {"text": "ok", "completion_id": "python-read-artifact-1"}

    model = finstack_ai.PythonModel(
        callback,
        component="python.model.read-artifact",
        provider="python-fixture",
        model="python-fixture-model",
    )
    return await finstack_ai.Agent.from_python(model)


def test_read_artifact_rejects_a_value_that_is_not_a_reference() -> None:
    """A dict that is not an `ArtifactRef` fails before any store lookup."""

    async def exercise() -> None:
        agent = await _agent()
        with pytest.raises(ValueError, match="invalid artifact reference"):
            agent.read_artifact({"id": "not-a-reference"})

    asyncio.run(exercise())


def test_read_artifact_reports_a_reference_this_agent_never_staged() -> None:
    """A well-formed reference still fails when the store has no bytes.

    Artifacts live in the agent that staged them, so a reference carried
    over from another agent or a previous process resolves to nothing.
    """

    async def exercise() -> None:
        agent = await _agent()
        with pytest.raises(ValueError, match="artifact read failed"):
            agent.read_artifact(ABSENT_ARTIFACT)

    asyncio.run(exercise())
