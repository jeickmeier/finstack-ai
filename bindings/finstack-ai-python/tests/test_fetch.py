"""Bounded HTTP fetch toolset: JSON-configured construction and tool wiring."""

from __future__ import annotations

import asyncio
import json
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Any

import finstack_ai


def test_http_fetch_toolset_constructs_from_json_with_allowlist() -> None:
    toolset = finstack_ai.HttpFetchToolset('{"allowlist": ["docs.rs"]}')
    assert toolset.component == "finstack.tools.fetch"
    assert toolset.tool_count == 1


def test_http_fetch_toolset_rejects_empty_allowlist() -> None:
    try:
        finstack_ai.HttpFetchToolset('{"allowlist": []}')
    except (TypeError, ValueError):
        return
    raise AssertionError("empty allowlist must be rejected")


def test_http_fetch_toolset_rejects_invalid_json() -> None:
    try:
        finstack_ai.HttpFetchToolset("not json")
    except (TypeError, ValueError):
        return
    raise AssertionError("invalid config json must be rejected")


def test_http_fetch_toolset_rejects_unknown_config_field() -> None:
    try:
        finstack_ai.HttpFetchToolset('{"allowlist": ["docs.rs"], "surprise": true}')
    except (TypeError, ValueError):
        return
    raise AssertionError("unknown config field must be rejected")


def test_http_fetch_toolset_rejects_allow_loopback_http_in_json() -> None:
    # allow_loopback_http must be code-only (the insecure_allow_loopback_http
    # keyword argument), never data-borne. JSON that still carries the old
    # field name must fail like any other unknown key.
    try:
        finstack_ai.HttpFetchToolset(
            '{"allowlist": ["docs.rs"], "allow_loopback_http": true}'
        )
    except (TypeError, ValueError):
        return
    raise AssertionError("allow_loopback_http in JSON must be rejected")


def test_http_fetch_toolset_is_exported_from_finstack_ai() -> None:
    assert "HttpFetchToolset" in finstack_ai.__all__
    assert finstack_ai.HttpFetchToolset is finstack_ai._native.HttpFetchToolset


class _FixtureHandler(BaseHTTPRequestHandler):
    def do_GET(self) -> None:
        body = b"hello from the fetch fixture"
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format: str, *args: object) -> None:
        del format, args


def test_http_fetch_toolset_is_registrable_and_callable_like_other_toolsets() -> None:
    server = HTTPServer(("127.0.0.1", 0), _FixtureHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        port = server.server_address[1]
        url = f"http://127.0.0.1:{port}/"

        model_calls = 0
        second_request: dict[str, Any] = {}

        async def model_callback(
            context: finstack_ai.CallbackContext, request: dict[str, Any]
        ) -> dict[str, Any]:
            del context
            nonlocal model_calls
            model_calls += 1
            if model_calls == 1:
                return {
                    "text": "",
                    "completion_id": "fetch-call-1",
                    "tool_calls": [{"name": "http_fetch", "arguments": {"url": url}}],
                }
            second_request.update(request)
            return {"text": "fetched", "completion_id": "fetch-call-2"}

        async def exercise() -> str:
            toolset = finstack_ai.HttpFetchToolset(
                json.dumps({"allowlist": ["fetch-fixture.invalid"]}),
                insecure_allow_loopback_http=True,
            )
            agent = await finstack_ai.Agent.from_python(
                finstack_ai.PythonModel(
                    model_callback,
                    component="python.model.fetch",
                    provider="python-fixture",
                    model="python-fixture-model",
                ),
                [toolset],
                "Fetch the page when asked.",
            )
            run = agent.start("fetch the fixture page")
            return (await run.result()).text

        assert asyncio.run(exercise()) == "fetched"
        assert model_calls == 2
        assert "hello from the fetch fixture" in json.dumps(second_request)
    finally:
        server.shutdown()
        thread.join(timeout=5)
