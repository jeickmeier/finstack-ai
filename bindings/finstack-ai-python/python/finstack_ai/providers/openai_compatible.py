"""Availability metadata for the linked OpenAI-compatible provider."""

from .. import linked_providers


def is_available() -> bool:
    """Return whether the OpenAI-compatible Rust provider is linked.

    Returns:
        ``True`` when this wheel contains the provider implementation.
    """

    return "openai-compatible" in linked_providers()
