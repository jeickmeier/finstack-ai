"""P3.4: native `ShellToolset` wrapper (T2, deny-by-default).

An allowlisted program executes and its output reaches the tool result;
a program outside the allowlist fails closed with the crate's stable
policy code.
"""

from __future__ import annotations

import asyncio
from typing import Any

import finstack_ai


def _exec_model(
    argv: list[str], captured: list[dict[str, Any]]
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
                "completion_id": "python-shell-1",
                "tool_calls": [
                    {"name": "shell_exec", "arguments": {"argv": argv, "cwd": None}}
                ],
            }
        return {"text": "done", "completion_id": "python-shell-2"}

    return finstack_ai.PythonModel(
        callback,
        component="python.model.shell",
        provider="python-fixture",
        model="python-fixture-model",
    )


def _resolution(request: dict[str, Any], approved: bool) -> dict[str, Any]:
    return {
        "interaction_id": request["interaction_id"],
        "resolution_id": "python-shell-resolution-1",
        "principal": {
            "issuer": "finstack-ai-python",
            "subject": "local-user",
            "tenant_scope": "python-local",
        },
        "authorization": {
            "policy_version": "python-policy-v1",
            "decision_id": "python-decision-v1",
        },
        "response": {"approved": approved},
    }


async def _wait_for_interaction(run: finstack_ai.Run) -> dict[str, Any]:
    for _ in range(500):
        pending = await run.list_interactions()
        if pending:
            return pending[0]
        await asyncio.sleep(0.01)
    raise AssertionError("timed out waiting for an approval interaction")


async def _run_with_approval(agent: finstack_ai.Agent, prompt: str) -> str:
    run = agent.start(prompt)
    pending = await _wait_for_interaction(run)
    await run.resolve_interaction(_resolution(pending, True))
    return (await run.result()).text


def _tool_result_text(request: dict[str, Any]) -> str:
    return str(
        [
            block
            for message in request["messages"]
            for block in message["content"]
            if block["kind"] == "tool_result"
        ]
    )


def test_allowlisted_program_executes() -> None:
    toolset = finstack_ai.ShellToolset(["/bin/echo"])
    assert toolset.component == "python.tools.shell"

    captured: list[dict[str, Any]] = []

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(
            _exec_model(["/bin/echo", "SHELL_MARKER"], captured), toolsets=[toolset]
        )
        # shell_exec carries ApprovalRequirement.Policy; approve it once.
        assert await _run_with_approval(agent, "echo the marker") == "done"

    asyncio.run(exercise())

    assert "SHELL_MARKER" in _tool_result_text(captured[1])


def test_unlisted_program_fails_closed() -> None:
    captured: list[dict[str, Any]] = []

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(
            _exec_model(["/bin/cat", "/etc/hosts"], captured),
            toolsets=[finstack_ai.ShellToolset(["/bin/echo"])],
        )
        await _run_with_approval(agent, "cat something")

    asyncio.run(exercise())

    result_text = _tool_result_text(captured[1])
    assert "shell_policy_denied" in result_text, result_text


def test_empty_allowlist_rejected() -> None:
    try:
        finstack_ai.ShellToolset([])
    except ValueError as error:
        assert "invalid_shell_allowlist" in str(error)
    else:
        raise AssertionError("expected a ValueError for an empty allowlist")
