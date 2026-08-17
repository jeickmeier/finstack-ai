"""Live-gate helpers for the Python learning notebooks.

The binding does not read environment variables. Notebooks resolve
``api_key=`` from a top-of-notebook assignment via ``live_value()``,
then the environment. This module does not load ``.env`` files.
"""

from __future__ import annotations

import json
import os
from urllib.error import URLError
from urllib.request import urlopen

DEFAULT_OLLAMA_MODEL = "gemma4:26b"


def live(*env_names: str) -> bool:
    """Return whether gated live provider cells may run.

    True when every named environment variable is set to a non-empty
    value. Callers that need a local server use :func:`ollama_live`
    instead of an extra flag.

    Args:
        *env_names: Environment variable names that must be present.

    Returns:
        ``True`` when every named variable is set and non-empty.
        ``False`` when ``env_names`` is empty or any name is missing.
    """

    if not env_names:
        return False
    return all(bool(os.environ.get(name)) for name in env_names)


def live_value(explicit: str, *env_names: str) -> str | None:
    """Return a notebook value, else the first non-empty env var.

    Args:
        explicit: Value from a notebook assignment. Empty strings fall
            through to ``env_names``.
        *env_names: Environment variable names to try in order.

    Returns:
        The first non-empty value, or ``None`` when ``explicit`` and
        every named variable are empty or unset. The value is never
        printed.
    """

    if explicit:
        return explicit
    for name in env_names:
        value = os.environ.get(name)
        if value:
            return value
    return None


def ollama_preferred_model(model: str | None = None) -> str:
    """Return the notebook Ollama model name.

    Args:
        model: Explicit model name. Empty values fall through.

    Returns:
        ``model`` when set, otherwise ``OLLAMA_MODEL`` when set,
        otherwise ``gemma4:26b``.
    """

    return model or os.environ.get("OLLAMA_MODEL") or DEFAULT_OLLAMA_MODEL


def ollama_installed(base_url: str = "http://127.0.0.1:11434") -> list[str] | None:
    """Return installed Ollama model names from ``/api/tags``.

    Args:
        base_url: Ollama base URL.

    Returns:
        Installed names, possibly empty, or ``None`` when the server is
        unreachable or the response is not a tags listing.
    """

    url = base_url.rstrip("/") + "/api/tags"
    try:
        with urlopen(url, timeout=0.75) as response:
            payload = json.loads(response.read().decode("utf-8"))
    except (OSError, URLError, TimeoutError, json.JSONDecodeError, ValueError):
        return None
    models = payload.get("models") if isinstance(payload, dict) else None
    if not isinstance(models, list):
        return None
    named: list[str] = []
    for item in models:
        if not isinstance(item, dict):
            continue
        name = item.get("name") or item.get("model")
        if isinstance(name, str) and name:
            named.append(name)
    return named


def ollama_live(
    base_url: str = "http://127.0.0.1:11434",
    model: str | None = None,
) -> str | None:
    """Return the preferred model when Ollama lists it.

    The preferred name is ``model``, else ``OLLAMA_MODEL``, else
    ``gemma4:26b``. This does not pick another installed model.

    Args:
        base_url: Ollama base URL.
        model: Explicit model name. Empty values fall through.

    Returns:
        The preferred model name, or ``None`` when the server is
        unreachable or that model is not installed.
    """

    preferred = ollama_preferred_model(model)
    installed = ollama_installed(base_url)
    if installed is None or preferred not in installed:
        return None
    return preferred


def secret(name: str) -> str:
    """Return one environment value without printing it.

    Args:
        name: Environment variable name.

    Returns:
        The non-empty value.

    Raises:
        KeyError: The name is missing or empty. The message is the name
            only; the value is never interpolated or printed.
    """

    value = os.environ.get(name)
    if not value:
        raise KeyError(name)
    return value
