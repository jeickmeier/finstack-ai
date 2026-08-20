"""Approval grant mode accepted by Python agent factories."""

from __future__ import annotations

import asyncio

import finstack_ai


def test_approval_grant_mode_is_accepted_by_from_python() -> None:
    assert finstack_ai.ApprovalGrantMode.per_call() is not None
    assert finstack_ai.ApprovalGrantMode.informed_batch() is not None

    async def model_callback(
        context: finstack_ai.CallbackContext, request: dict[str, object]
    ) -> dict[str, object]:
        del context, request
        return {"text": "ok", "completion_id": "python-approval-grant-1"}

    async def exercise() -> None:
        model = finstack_ai.PythonModel(
            model_callback,
            component="python.model.approval-grant",
            provider="scripted",
            model="preview-1",
        )
        agent = await finstack_ai.Agent.from_python(
            model,
            approval_grant=finstack_ai.ApprovalGrantMode.informed_batch(),
        )
        assert agent.capability_catalog() == []

    asyncio.run(exercise())
