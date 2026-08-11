"""Typed Python facade for the Rust-owned finstack-ai engine."""

from typing import TypedDict, cast

from . import _finstack_ai as _native

__version__ = _native.__version__


class BuildMetadata(TypedDict):
    """Build and compatibility metadata for the loaded native module."""

    version: str
    engine_version: str
    implementation: str
    free_threaded: bool
    provider_quirks_version: int


def health() -> str:
    """Return ``"ok"`` when the native module loaded successfully.

    Returns:
        The constant health status. This function performs no I/O and does not
        initialize an async runtime.
    """

    return _native.health()


def build_metadata() -> BuildMetadata:
    """Return version, interpreter, and linked-provider build metadata.

    Returns:
        Metadata for the loaded extension and Rust semantic engine.
    """

    return cast(BuildMetadata, _native.build_metadata())


def linked_providers() -> tuple[str, ...]:
    """Return curated Rust-backed providers linked into this wheel.

    Returns:
        Provider identifiers. Provider clients are not constructed by this
        query or during package import.
    """

    return _native.linked_providers()


__all__ = [
    "BuildMetadata",
    "__version__",
    "build_metadata",
    "health",
    "linked_providers",
]
