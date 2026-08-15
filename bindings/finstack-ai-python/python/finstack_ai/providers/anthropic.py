"""Availability metadata for the linked Anthropic Messages provider."""

from .. import linked_providers


def is_available() -> bool:
    """Return whether the Anthropic Rust provider is linked.

    Returns:
        ``True`` when this wheel contains the provider implementation.
    """

    return "anthropic" in linked_providers()
