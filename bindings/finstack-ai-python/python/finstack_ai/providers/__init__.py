"""Lazy provider namespace for curated Rust-backed integrations."""

from importlib import import_module
from types import ModuleType
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from . import anthropic as anthropic
    from . import ollama as ollama
    from . import openai_compatible as openai_compatible

__all__ = ["anthropic", "ollama", "openai_compatible"]


def __getattr__(name: str) -> ModuleType:
    """Load a provider facade only when its public name is requested.

    Args:
        name: Provider submodule name.

    Returns:
        The requested provider module.

    Raises:
        AttributeError: If the provider name is unknown.
    """

    if name in {"openai_compatible", "anthropic", "ollama"}:
        module = import_module(f"{__name__}.{name}")
        globals()[name] = module
        return module
    raise AttributeError(f"module {__name__!r} has no attribute {name!r}")
