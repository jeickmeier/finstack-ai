"""P2.1: native `VerifyMiddleware` wrapper.

A pure Python verifier judges the candidate assistant message at
`before_finalize`: accept lands it, reject fails the run with the stable
``verify_rejected`` code, and bounce retries the model with feedback.
"""

from __future__ import annotations

import asyncio
import json
from typing import Any

import finstack_ai


def _model(responses: list[str]) -> finstack_ai.PythonModel:
    queue = list(responses)

    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, object]:
        del context, request
        text = queue.pop(0)
        return {"text": text, "completion_id": f"python-verify-{len(queue)}"}

    return finstack_ai.PythonModel(
        callback,
        component="python.model.verify",
        provider="python-fixture",
        model="python-fixture-model",
    )


def _citation_verifier(message_json: str) -> Any:
    message = json.loads(message_json)
    text = "".join(
        block.get("text", "")
        for block in message.get("content", [])
        if block.get("kind") == "text"
    )
    if "[source]" in text:
        return "accept"
    return {
        "verdict": "reject",
        "findings": [{"kind": "citation", "note": "answer lacks a [source] citation"}],
    }


def _middleware() -> finstack_ai.VerifyMiddleware:
    return finstack_ai.VerifyMiddleware(
        _citation_verifier, verifier_id="citation-check"
    )


def test_accept_lands_the_answer() -> None:
    middleware = _middleware()
    assert middleware.component == "finstack.middleware.verify"

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(
            _model(["cited answer [source]"]), middleware=[middleware]
        )
        result = await agent.run("question")
        assert result.text == "cited answer [source]"

    asyncio.run(exercise())


def test_reject_fails_the_run_with_stable_code() -> None:
    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(
            _model(["uncited answer"]), middleware=[_middleware()]
        )
        try:
            await agent.run("question")
        except finstack_ai.FinstackError as error:
            assert "verify_rejected" in str(error), str(error)
        else:
            raise AssertionError("expected verify_rejected failure")

    asyncio.run(exercise())


def test_raising_verifier_fails_closed() -> None:
    def broken(message_json: str) -> Any:
        raise RuntimeError("verifier bug")

    async def exercise() -> None:
        agent = await finstack_ai.Agent.from_python(
            _model(["any answer"]),
            middleware=[
                finstack_ai.VerifyMiddleware(broken, verifier_id="broken-check")
            ],
        )
        try:
            await agent.run("question")
        except finstack_ai.FinstackError as error:
            assert "verify_rejected" in str(error), str(error)
        else:
            raise AssertionError("expected fail-closed rejection")

    asyncio.run(exercise())


def test_invalid_verifier_id_rejected() -> None:
    try:
        finstack_ai.VerifyMiddleware(_citation_verifier, verifier_id="")
    except ValueError:
        pass
    else:
        raise AssertionError("expected a ValueError for an empty verifier id")
