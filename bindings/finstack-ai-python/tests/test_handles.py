"""Rust-backed async handle, batching, cancellation, and error tests."""

from __future__ import annotations

import asyncio
import json
import threading
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Iterator

import pytest

import finstack_ai


def _ollama_ndjson(parts: list[str]) -> bytes:
    events = [
        json.dumps(
            {"message": {"role": "assistant", "content": part}, "done": False},
            separators=(",", ":"),
        )
        for part in parts
    ]
    events.append(
        json.dumps(
            {
                "message": {"role": "assistant", "content": ""},
                "done": True,
                "done_reason": "stop",
                "prompt_eval_count": 1,
                "eval_count": 1,
            },
            separators=(",", ":"),
        )
    )
    return ("\n".join(events) + "\n").encode()


class _FixtureServer(ThreadingHTTPServer):
    daemon_threads = True

    def __init__(self, body: bytes, status: int, hold_response: bool) -> None:
        super().__init__(("127.0.0.1", 0), _FixtureHandler)
        self.body = body
        self.status = status
        self.hold_response = hold_response
        self.request_started = threading.Event()
        self.release_response = threading.Event()
        self.requests: list[dict[str, object]] = []
        self.requests_changed = threading.Condition()

    @property
    def base_url(self) -> str:
        host, port = self.server_address
        return f"http://{host}:{port}"

    def wait_for_requests(self, count: int, timeout: float) -> bool:
        with self.requests_changed:
            return self.requests_changed.wait_for(
                lambda: len(self.requests) >= count, timeout=timeout
            )


class _FixtureHandler(BaseHTTPRequestHandler):
    server: _FixtureServer

    def do_POST(self) -> None:  # noqa: N802
        content_length = int(self.headers.get("content-length", "0"))
        request = json.loads(self.rfile.read(content_length))
        with self.server.requests_changed:
            self.server.requests.append(request)
            self.server.requests_changed.notify_all()
        self.server.request_started.set()
        if self.server.hold_response:
            self.server.release_response.wait(timeout=5)
        content_type = (
            "application/x-ndjson" if self.server.status == 200 else "application/json"
        )
        try:
            self.send_response(self.server.status)
            self.send_header("content-type", content_type)
            self.send_header("content-length", str(len(self.server.body)))
            self.send_header("connection", "close")
            self.end_headers()
            self.wfile.write(self.server.body)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def log_message(self, format: str, *args: object) -> None:
        del format, args


@contextmanager
def _server(
    body: bytes,
    *,
    status: int = 200,
    hold_response: bool = False,
) -> Iterator[_FixtureServer]:
    server = _FixtureServer(body, status, hold_response)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield server
    finally:
        server.release_response.set()
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)


async def _agent(server: _FixtureServer) -> finstack_ai.Agent:
    return await finstack_ai.Agent.ollama(
        server.base_url,
        "fixture-model",
        "Answer concisely.",
    )


def test_rust_backed_run_batches_events_and_retains_result() -> None:
    async def exercise(
        server: _FixtureServer,
    ) -> tuple[finstack_ai.Run, list[finstack_ai.EventBatch]]:
        agent = await _agent(server)
        run = agent.start("say hello")

        async def collect() -> list[finstack_ai.EventBatch]:
            return [batch async for batch in run.events()]

        first, second, batches = await asyncio.gather(
            run.result(), run.result(), collect()
        )
        assert first.text == "hello world"
        assert second.text == first.text
        assert (
            first.locator.to_dict() == second.locator.to_dict() == run.locator.to_dict()
        )
        assert first.session.session_id == run.session.session_id
        return run, batches

    with _server(_ollama_ndjson(["hello", " ", "w", "o", "r", "l", "d"])) as server:
        run, batches = asyncio.run(exercise(server))

    events = [event for batch in batches for event in batch.events()]
    assert len(batches) < len(events)
    assert [event.transient_sequence for event in events] == sorted(
        event.transient_sequence for event in events
    )
    assert any(event.kind == "model_text_delta" for event in events)
    assert events[-1].kind == "run_completed"
    assert json.loads(batches[-1].to_json())[-1]["kind"] == "run_completed"
    assert batches[-1].to_json_bytes().decode() == batches[-1].to_json()
    assert all(batch.first_sequence <= batch.last_sequence for batch in batches)
    assert all(batch.dropped_progress == 0 for batch in batches)
    assert server.requests[0]["model"] == "fixture-model"
    assert server.requests[0]["stream"] is True

    async def retained_text() -> str:
        return (await run.result()).text

    with ThreadPoolExecutor(max_workers=4) as executor:
        texts = list(executor.map(lambda _: asyncio.run(retained_text()), range(16)))
    assert texts == ["hello world"] * 16


def test_run_bounds_are_canonical_across_python_and_rust() -> None:
    async def exercise(server: _FixtureServer) -> None:
        agent = await _agent(server)
        completed = await agent.run("long deadline", timeout_seconds=100_000)
        assert completed.text == "canonical bounds"
        with pytest.raises(finstack_ai.ConfigurationError) as caught:
            agent.start("invalid cycles", max_cycles=1_025)
        assert caught.value.code == "agent_run_invalid_configuration"

    with _server(_ollama_ndjson(["canonical bounds"])) as server:
        asyncio.run(exercise(server))


def test_independent_rust_runs_reach_io_without_gil_serialization() -> None:
    async def exercise(server: _FixtureServer) -> None:
        agent = await _agent(server)

        async def run(input_text: str) -> str:
            return (await agent.run(input_text)).text

        first = asyncio.create_task(run("first"))
        second = asyncio.create_task(run("second"))
        both_started = await asyncio.to_thread(server.wait_for_requests, 2, 3.0)
        assert both_started
        server.release_response.set()
        assert await asyncio.gather(first, second) == ["concurrent", "concurrent"]

    with _server(_ollama_ndjson(["concurrent"]), hold_response=True) as server:
        asyncio.run(exercise(server))


def test_explicit_cancellation_is_idempotent_and_contextual() -> None:
    async def exercise(server: _FixtureServer) -> None:
        agent = await _agent(server)
        run = agent.start("wait")
        started = await asyncio.to_thread(server.request_started.wait, 3)
        assert started
        await asyncio.gather(run.cancel(), run.cancel())
        with pytest.raises(finstack_ai.CancelledError) as caught:
            await run.result()
        assert caught.value.code == "agent_run_cancelled"
        assert caught.value.retryable is False
        assert caught.value.context == run.locator.to_dict()
        await run.close_events()

    with _server(_ollama_ndjson(["late"]), hold_response=True) as server:
        try:
            asyncio.run(exercise(server))
        finally:
            server.release_response.set()


def test_cancelling_one_result_waiter_does_not_cancel_the_run() -> None:
    async def exercise(server: _FixtureServer) -> None:
        agent = await _agent(server)
        run = agent.start("detach one waiter")
        started = await asyncio.to_thread(server.request_started.wait, 3)
        assert started

        async def wait_for_result() -> finstack_ai.RunResult:
            return await run.result()

        waiter = asyncio.create_task(wait_for_result())
        await asyncio.sleep(0)
        waiter.cancel()
        with pytest.raises(asyncio.CancelledError):
            await waiter
        server.release_response.set()
        result = await run.result()
        assert result.text == "completed"

    with _server(_ollama_ndjson(["completed"]), hold_response=True) as server:
        asyncio.run(exercise(server))


def test_error_hierarchy_preserves_codes_retryability_and_safe_context() -> None:
    async def invalid_configuration() -> None:
        with pytest.raises(finstack_ai.ConfigurationError) as caught:
            await finstack_ai.Agent.ollama("ftp://unsafe.example", "fixture-model")
        assert caught.value.code == "agent_run_invalid_configuration"
        assert caught.value.retryable is False
        assert caught.value.context is None

    async def runtime_failure(server: _FixtureServer) -> None:
        agent = await _agent(server)
        with pytest.raises(finstack_ai.RuntimeError) as caught:
            await agent.run("fail")
        assert caught.value.code == "agent_run_runtime_failure"
        assert caught.value.retryable is False
        assert set(caught.value.context or {}) == {
            "tenant_scope",
            "session_id",
            "lane_id",
            "run_id",
        }

    async def timeout(server: _FixtureServer) -> None:
        agent = await _agent(server)
        with pytest.raises(finstack_ai.TimeoutError) as caught:
            await agent.run("wait", timeout_seconds=0.01)
        assert caught.value.code == "agent_run_timeout"
        assert caught.value.retryable is False
        assert set(caught.value.context or {}) == {
            "tenant_scope",
            "session_id",
            "lane_id",
            "run_id",
        }

    asyncio.run(invalid_configuration())
    with _server(b'{"error":{"message":"fixture failure"}}', status=500) as server:
        asyncio.run(runtime_failure(server))
    with _server(_ollama_ndjson(["late"]), hold_response=True) as server:
        try:
            asyncio.run(timeout(server))
        finally:
            server.release_response.set()
