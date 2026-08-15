"""Availability probes for curated Rust-backed provider integrations."""

from __future__ import annotations

from .. import linked_providers

__all__ = ["anthropic", "ollama", "openai_compatible"]


class _ProviderAvailability:
    """Availability probe for one linked provider identifier."""

    def __init__(self, linked_name: str) -> None:
        self._linked_name = linked_name

    def is_available(self) -> bool:
        """Return whether this wheel contains the provider implementation.

        Returns:
            ``True`` when the native module reports the linked provider name.
        """

        return self._linked_name in linked_providers()


openai_compatible = _ProviderAvailability("openai-compatible")
anthropic = _ProviderAvailability("anthropic")
ollama = _ProviderAvailability("ollama")
