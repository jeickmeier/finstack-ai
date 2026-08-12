"""Typed Python facade for the Rust-owned finstack-ai engine."""

from typing import TypedDict, cast

from . import _finstack_ai as _native
from ._finstack_ai import (
    Agent,
    CallbackContext,
    CancelledError,
    ConfigurationError,
    Event,
    EventBatch,
    EventBatchIterator,
    FinstackError,
    PythonContextProvider,
    PythonMiddleware,
    PythonModel,
    PythonObserver,
    PythonToolset,
    Run,
    RunResult,
    RuntimeError,
    Session,
    TimeoutError,
)

__version__ = _native.__version__


class BuildMetadata(TypedDict):
    """Build and compatibility metadata for the loaded native module."""

    version: str
    engine_version: str
    implementation: str
    free_threaded: bool
    provider_quirks_version: int


def health() -> str:
    """Return ``\"ok\"`` when the native module loaded successfully.

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


def normalize_prebeta_shape(kind: str, value: dict[str, object]) -> dict[str, object]:
    """Validate a pre-beta Rust-owned lineage or external-command shape.

    Supported kinds are ``child_run_prepared``, ``interaction_resolution``,
    and ``external_effect_completion``. This data-only API does not route a
    command or claim durable restart support; PR-048 owns that beta gate.

    Args:
        kind: Stable shape family.
        value: Candidate normalized mapping.

    Returns:
        The Rust-validated normalized mapping.

    Raises:
        TypeError: If the kind or shape is invalid.
    """

    return _native.normalize_prebeta_shape(kind, value)


__all__ = [
    "Agent",
    "BuildMetadata",
    "CallbackContext",
    "CancelledError",
    "ConfigurationError",
    "Event",
    "EventBatch",
    "EventBatchIterator",
    "FinstackError",
    "PythonContextProvider",
    "PythonMiddleware",
    "PythonModel",
    "PythonObserver",
    "PythonToolset",
    "Run",
    "RunResult",
    "RuntimeError",
    "Session",
    "TimeoutError",
    "__version__",
    "build_metadata",
    "health",
    "linked_providers",
    "normalize_prebeta_shape",
]
