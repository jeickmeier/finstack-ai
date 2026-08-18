"""PR-079 child-run accept and external-completion routing."""

from __future__ import annotations

import asyncio
import json

import pytest

import finstack_ai


def test_start_child_and_complete_external_route() -> None:
    calls = 0

    async def model_callback(
        context: finstack_ai.CallbackContext, request: dict[str, object]
    ) -> dict[str, object]:
        del context, request
        nonlocal calls
        calls += 1
        if calls == 1:
            return {"deferred": "job-1", "completion_id": "python-defer-1"}
        return {"text": "child done", "completion_id": f"python-child-{calls}"}

    async def exercise() -> None:
        model = finstack_ai.PythonModel(
            model_callback,
            component="python.model.child",
            provider="scripted",
            model="preview-1",
        )
        agent = await finstack_ai.Agent.from_python(
            model, child_runs=finstack_ai.ChildRunPolicy.allow(1)
        )
        parent = agent.start("parent work")
        effect_id = await _wait_effect_id(parent)
        child = await parent.start_child(
            agent, "child work", placement="isolated_child_session"
        )
        result = await child.result()
        assert result.text == "child done"
        assert child.locator.run_id != parent.locator.run_id
        assert child.locator.session_id != parent.locator.session_id
        command = {
            "locator": parent.locator.to_dict(),
            "principal": {
                "issuer": "finstack-ai-python",
                "subject": "local-user",
                "tenant_scope": parent.locator.tenant_scope,
            },
            "authorization": {
                "policy_version": "python-policy-v1",
                "decision_id": "python-decision-v1",
            },
            "completion": {
                "effect_id": effect_id,
                "completion_id": "python-ext-1",
                "outcome": {
                    "failed": {
                        "error": {
                            "code": "provider_failed",
                            "message": "provider failed",
                            "category": "model",
                            "retryable": True,
                            "identifiers": {},
                            "safe_details": {},
                        }
                    }
                },
            },
        }
        command = finstack_ai.normalize_prebeta_shape(
            "external_effect_completion", command
        )
        outcome = await parent.complete_external(command)
        assert outcome["status"] in {"committed", "idempotent", "rejected"}

    asyncio.run(exercise())


def test_start_child_fails_closed_when_policy_denies() -> None:
    async def model_callback(
        context: finstack_ai.CallbackContext, request: dict[str, object]
    ) -> dict[str, object]:
        del context, request
        return {"text": "parent done", "completion_id": "python-deny-1"}

    async def exercise() -> None:
        model = finstack_ai.PythonModel(
            model_callback,
            component="python.model.child-deny",
            provider="scripted",
            model="preview-1",
        )
        agent = await finstack_ai.Agent.from_python(model)
        parent = agent.start("parent work")
        with pytest.raises(finstack_ai.FinstackError) as caught:
            await parent.start_child(
                agent, "child work", placement="isolated_child_session"
            )
        assert caught.value.code == "agent_invoke_invalid_acceptance"

    asyncio.run(exercise())


async def _wait_effect_id(run: finstack_ai.Run) -> str:
    async for batch in run.events():
        for event in batch.events():
            if event.kind != "effect_deferred":
                continue
            payload = json.loads(event.to_json())
            effect_id = payload.get("effect_id")
            if isinstance(effect_id, str) and effect_id:
                return effect_id
    raise AssertionError("parent never deferred an effect")
