"""Availability metadata for the linked Ollama/local provider path."""

from .. import linked_providers


def is_available() -> bool:
    """Return whether the Ollama/local Rust provider path is linked.

    Returns:
        ``True`` when this wheel contains the OpenAI-compatible implementation
        used by ``Agent.ollama``.
    """

    return "ollama" in linked_providers()
