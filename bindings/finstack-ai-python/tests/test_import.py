"""Import, metadata, typing, and lazy-provider smoke tests."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import pytest


def test_import_finstack_ai() -> None:
    import finstack_ai

    assert finstack_ai.health() == "ok"
    assert finstack_ai.__version__ == "2.0.0"
    assert finstack_ai.linked_providers() == (
        "openai",
        "anthropic",
        "gemini",
        "ollama",
        "openrouter",
    )
    assert finstack_ai.SqliteDurability.Durable != finstack_ai.SqliteDurability.Relaxed
    assert str(finstack_ai.SqliteDurability.Durable) == "Durable"
    assert str(finstack_ai.SqliteDurability.Relaxed) == "Relaxed"

    metadata = finstack_ai.build_metadata()
    assert metadata["version"] == finstack_ai.__version__
    assert metadata["engine_version"] == finstack_ai.__version__
    assert metadata["implementation"] == "cpython"
    assert isinstance(metadata["free_threaded"], bool)
    assert metadata["provider_quirks_version"] > 0


def test_provider_namespace_is_lazy() -> None:
    import finstack_ai

    assert "finstack_ai.providers" not in sys.modules
    import finstack_ai.providers as providers

    assert providers.openai.is_available()
    assert providers.anthropic.is_available()
    assert providers.gemini.is_available()
    assert providers.ollama.is_available()
    assert finstack_ai.health() == "ok"


def test_typing_files_ship_beside_the_package() -> None:
    import finstack_ai

    package_dir = Path(finstack_ai.__file__).resolve().parent
    assert (package_dir / "py.typed").is_file()
    assert (package_dir / "_finstack_ai.pyi").is_file()


def test_release_module_excludes_non_default_benchmark_fixture() -> None:
    from finstack_ai import _finstack_ai

    assert not hasattr(_finstack_ai, "_benchmark_agent")
    assert not hasattr(_finstack_ai, "_benchmark_native")


def test_import_does_not_start_runtime_threads_or_python_network_calls() -> None:
    script = r"""
import json
import sys
import threading

events = []
def audit(event, args):
    if event.startswith("socket."):
        events.append(event)

sys.addaudithook(audit)
before = {thread.ident for thread in threading.enumerate()}
import finstack_ai
after = [
    {"name": thread.name, "daemon": thread.daemon}
    for thread in threading.enumerate()
    if thread.ident not in before
]
print(json.dumps({
    "events": events,
    "threads": after,
    "health": finstack_ai.health(),
    "pydantic_loaded": "pydantic" in sys.modules,
}))
"""
    completed = subprocess.run(
        [sys.executable, "-I", "-c", script],
        check=True,
        capture_output=True,
        text=True,
    )
    result = json.loads(completed.stdout)
    assert result == {
        "events": [],
        "threads": [],
        "health": "ok",
        "pydantic_loaded": False,
    }


def test_free_threaded_build_handles_concurrent_native_calls() -> None:
    """Exercise the 3.14t module without relying on the compatibility GIL."""
    if os.environ.get("FINSTACK_EXPECT_FREE_THREADED") != "1":
        return

    import finstack_ai

    assert finstack_ai.build_metadata()["free_threaded"] is True
    assert hasattr(sys, "_is_gil_enabled")
    assert sys._is_gil_enabled() is False
    with ThreadPoolExecutor(max_workers=16) as executor:
        results = list(executor.map(lambda _: finstack_ai.health(), range(512)))
    assert results == ["ok"] * 512


def test_anthropic_and_ollama_factories_construct_without_import_side_effects() -> None:
    import asyncio

    import finstack_ai

    async def construct() -> None:
        anthropic = await finstack_ai.Agent.anthropic(
            "http://127.0.0.1:9",
            "fixture-model",
            instruction="Answer concisely.",
        )
        ollama = await finstack_ai.Agent.ollama(
            "http://127.0.0.1:11434",
            "fixture-model",
        )
        assert anthropic.capability_catalog() == []
        assert ollama.capability_catalog() == []

    asyncio.run(construct())


def test_anthropic_http_credentials_fail_closed_without_leaking_the_canary() -> None:
    import asyncio

    import finstack_ai

    canary = "sk-ant-secret-canary-055"

    async def construct() -> None:
        with pytest.raises(finstack_ai.ConfigurationError) as caught:
            await finstack_ai.Agent.anthropic(
                "http://127.0.0.1:9",
                "fixture-model",
                api_key=canary,
            )
        assert caught.value.code == "agent_run_invalid_configuration"
        assert canary not in str(caught.value)
        assert canary not in repr(caught.value)

    asyncio.run(construct())


def test_gemini_and_ollama_factories_construct_without_import_side_effects() -> None:
    import asyncio

    import finstack_ai

    async def construct() -> None:
        gemini = await finstack_ai.Agent.gemini(
            "http://127.0.0.1:9",
            "fixture-model",
            instruction="Answer concisely.",
        )
        ollama = await finstack_ai.Agent.ollama(
            "http://127.0.0.1:11434",
            "fixture-model",
        )
        assert gemini.capability_catalog() == []
        assert ollama.capability_catalog() == []

    asyncio.run(construct())


def test_gemini_http_credentials_fail_closed_without_leaking_the_canary() -> None:
    import asyncio

    import finstack_ai

    canary = "AIza-secret-canary-055"

    async def construct() -> None:
        with pytest.raises(finstack_ai.ConfigurationError) as caught:
            await finstack_ai.Agent.gemini(
                "http://127.0.0.1:9",
                "fixture-model",
                api_key=canary,
            )
        assert caught.value.code == "agent_run_invalid_configuration"
        assert canary not in str(caught.value)
        assert canary not in repr(caught.value)

    asyncio.run(construct())


def test_openai_invalid_model_does_not_leak_the_api_key() -> None:
    import asyncio

    import finstack_ai

    canary = "sk-openai-secret-canary-056"

    async def construct() -> None:
        with pytest.raises(finstack_ai.ConfigurationError) as caught:
            await finstack_ai.Agent.openai(
                "",
                api_key=canary,
            )
        assert caught.value.code == "agent_run_invalid_configuration"
        assert canary not in str(caught.value)
        assert canary not in repr(caught.value)

    asyncio.run(construct())
