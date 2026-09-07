"""Offline notebook gates suppress credentials and local server probes."""

from __future__ import annotations

import json
from io import BytesIO

import pytest

import _support


@pytest.mark.parametrize("offline", ["0", "1"])
def test_live_gates_respect_offline_mode(
    monkeypatch: pytest.MonkeyPatch, offline: str
) -> None:
    monkeypatch.setenv("FINSTACK_NOTEBOOK_OFFLINE", offline)
    monkeypatch.setenv("OPENAI_API_KEY", "fixture-key")
    enabled = offline == "0"
    assert _support.live("OPENAI_API_KEY") is enabled
    assert _support.live_value("explicit-fixture", "OPENAI_API_KEY") == (
        "explicit-fixture" if enabled else None
    )
    assert _support.live_value("", "OPENAI_API_KEY") == (
        "fixture-key" if enabled else None
    )
    assert _support.live() is False
    monkeypatch.delenv("OPENAI_API_KEY")
    assert _support.live("OPENAI_API_KEY") is False
    assert _support.live_value("", "OPENAI_API_KEY") is None


@pytest.mark.parametrize("offline", ["0", "1"])
def test_offline_ollama_gate_never_probes_the_server(
    monkeypatch: pytest.MonkeyPatch, offline: str
) -> None:
    monkeypatch.setenv("FINSTACK_NOTEBOOK_OFFLINE", offline)
    requests: list[str] = []

    def urlopen(url: str, *, timeout: float) -> BytesIO:
        assert offline == "0", "offline notebooks must not contact Ollama"
        assert timeout == 0.75
        requests.append(url)
        return BytesIO(json.dumps({"models": [{"name": "fixture-model"}]}).encode())

    monkeypatch.setattr(_support, "urlopen", urlopen)
    enabled = offline == "0"
    assert _support.ollama_installed() == (["fixture-model"] if enabled else None)
    assert _support.ollama_live(model="fixture-model") == (
        "fixture-model" if enabled else None
    )
    assert len(requests) == (2 if enabled else 0)
