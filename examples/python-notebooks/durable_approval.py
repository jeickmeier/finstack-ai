"""Offline approval recipe: terminate the process while parked, then recover.

Run: ``uv run python examples/python-notebooks/durable_approval.py``.
The two phases share only disk state and this application definition.
"""

from __future__ import annotations

import asyncio
import json
import os
import sqlite3
import subprocess
import sys
import tempfile
from pathlib import Path
from typing import Any

import finstack_ai


async def phase(directory: Path, resume: bool) -> None:
    """Recreate host-supplied callbacks; no live run controller crosses processes."""
    writes = directory / "writes.txt"

    async def model(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context
        if any(
            block["kind"] == "tool_result"
            for message in request["messages"]
            for block in message["content"]
        ):
            return {"text": "approved write complete", "completion_id": "finished"}
        return {
            "completion_id": "request-write",
            "tool_calls": [{"name": "write", "arguments": {"value": 7}}],
        }

    async def tool(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context
        with writes.open("a") as output:
            output.write(f"{request['call']['arguments']['value']}\n")
            output.flush()
            os.fsync(output.fileno())
        return {"output": {"ok": True}}

    tool_spec = {
        "id": "recipe.write",
        "model_name": "write",
        "title": "Write",
        "description": "Write one approved integer to the local recipe file.",
        "input_schema": {
            "type": "object",
            "properties": {"value": {"type": "integer"}},
            "required": ["value"],
            "additionalProperties": False,
        },
        "output_schema": {
            "type": "object",
            "properties": {"ok": {"type": "boolean"}},
            "required": ["ok"],
            "additionalProperties": False,
        },
        "execution": "sequential",
        "side_effect": "non_idempotent_write",
        "retry_safety": "at_most_once",
        "approval": {
            "requirement": "required",
            "reason": "Local file mutation",
            "attributes": {},
        },
        "max_result_bytes": 4096,
        "metadata": {},
    }
    agent = await finstack_ai.Agent.from_python(
        finstack_ai.PythonModel(
            model,
            component="recipe.model.approval",
            provider="offline",
            model="approval",
        ),
        [
            finstack_ai.PythonToolset(
                tool, component="recipe.tools.write", name="write", tools=[tool_spec]
            )
        ],
    )
    host = await finstack_ai.DurableHost.open(
        str(directory / "host.sqlite"), {"approval": agent}
    )
    locator_path = directory / "locator.json"
    if not resume:
        locator = await host.start(
            "approval", "Write seven after approval.", timeout_seconds=120
        )
        locator_path.write_text(json.dumps(locator.to_dict()))
        assert not writes.exists()
        tick = await host.tick()
        assert tick["failures"] == 0 and tick["sessions_reparked"] == 1
        assert len(host.pending()) == 1 and not writes.exists()
        os._exit(0)
    locator = finstack_ai.Locator.from_dict(json.loads(locator_path.read_text()))
    pending = host.pending()[0]
    host.resolve(
        pending["interaction_id"],
        {
            "resolution_id": "local-approval-1",
            "principal": pending["principal"],
            "evidence": pending["evidence"],
            "payload": {"approved": True},
        },
    )
    assert (await host.tick())["failures"] == 0
    state = await host.inspect(locator)
    assert state["terminal"] and state["phase"] == "completed"
    assert writes.read_text().splitlines() == ["7"]
    assert not host.pending()
    assert (await host.tick())["sessions_resumed"] == 0
    await host.shutdown()


def main() -> None:
    """Bound both subprocesses and check authoritative delivery plus cleanup."""
    with tempfile.TemporaryDirectory(prefix="finstack-approval-recipe-") as directory:
        for action in ["park", "resume"]:
            subprocess.run(
                [sys.executable, str(Path(__file__).resolve()), action, directory],
                timeout=30,
                check=True,
            )
        with sqlite3.connect(Path(directory) / "host.sqlite") as connection:
            assert connection.execute(
                "SELECT status FROM finstack_workflow_hitl_inbox"
            ).fetchall() == [("accepted",)]
            assert connection.execute(
                "SELECT count(*) FROM finstack_workflow_worker_wake"
            ).fetchone() == (0,)
            assert connection.execute(
                "SELECT count(*) FROM finstack_workflow_worker_inbox"
            ).fetchone() == (0,)
    print(
        "durable approval: fresh process, one write, Accepted delivery, cleanup verified"
    )


if __name__ == "__main__":
    if len(sys.argv) == 3:
        asyncio.run(phase(Path(sys.argv[2]), sys.argv[1] == "resume"))
    else:
        main()
