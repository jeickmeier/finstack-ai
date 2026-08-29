"""Typed Python facade for the Rust-owned finstack-ai engine."""

from typing import Literal as _Literal
from typing import NotRequired as _NotRequired
from typing import TypedDict, cast

from . import _finstack_ai as _native
from ._finstack_ai import (
    Agent,
    ApprovalGrantMode,
    Attachment,
    BillingObserver,
    CalculatorToolset,
    CallbackContext,
    CancelledError,
    Capability,
    ChildRunPolicy,
    CompactionMiddleware,
    ConfigurationError,
    E2bSandboxToolset,
    ElicitationToolset,
    Event,
    EventBatch,
    EventBatchIterator,
    FileSystemToolset,
    FinstackError,
    HttpFetchToolset,
    InstructionsMiddleware,
    HistoryCachePolicy,
    Lane,
    LogObserver,
    Locator,
    McpToolset,
    MemoryContextProvider,
    MemoryExtension,
    MemoryExternalIdentityMap,
    MemoryObserver,
    MemoryToolset,
    MetricsObserver,
    NotifyObserver,
    OtelObserver,
    PythonContextProvider,
    PythonMiddleware,
    PythonModel,
    PythonObserver,
    PythonToolset,
    RedactionMiddleware,
    RepositoryContextProvider,
    Run,
    RunResult,
    RuntimeError,
    Session,
    ShellToolset,
    SkillsToolset,
    SqliteDurability,
    TimeoutError,
    ToolPolicyMiddleware,
    VerifyMiddleware,
)
from ._pydantic import PydanticTool, pydantic_toolset, tool

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

    Examples:
        >>> health()
        'ok'
    """

    return _native.health()


def build_metadata() -> BuildMetadata:
    """Return version, interpreter, and linked-provider build metadata.

    Returns:
        Metadata for the loaded extension and Rust semantic engine.

    Examples:
        >>> metadata = build_metadata()
        >>> {"version", "engine_version"} <= metadata.keys()
        True
    """

    return cast(BuildMetadata, _native.build_metadata())


def linked_providers() -> tuple[str, ...]:
    """Return curated Rust-backed providers linked into this wheel.

    Returns:
        Provider identifiers. Provider clients are not constructed by this
        query or during package import.

    Examples:
        >>> isinstance(linked_providers(), tuple)
        True
    """

    return _native.linked_providers()


def journal_known_answer(kind: str, value: dict[str, object]) -> dict[str, object]:
    """Return payload digest, checksum, and canonical-CBOR hex from Rust.

    ``kind`` is ``record_body`` or ``record_envelope``. This helper does not
    implement CBOR in Python; it calls the one Rust engine.

    Args:
        kind: Known-answer family.
        value: Diagnostic JSON mapping of the body or envelope.

    Returns:
        A mapping with ``payload_digest``, optional ``checksum``, and
        ``cbor_hex``.

    Raises:
        TypeError: If ``kind`` or ``value`` is invalid.
    """

    return _native.journal_known_answer(kind, value)


def normalize_prebeta_shape(kind: str, value: dict[str, object]) -> dict[str, object]:
    """Validate a pre-beta Rust-owned lineage or external-command shape.

    Supported kinds are ``child_run_prepared``, ``interaction_resolution``,
    and ``external_effect_completion``. This helper only validates the
    shape. ``Run.start_child`` and ``Run.complete_external`` route those
    two kinds through the Rust engine.

    Args:
        kind: Stable shape family.
        value: Candidate normalized mapping.

    Returns:
        The Rust-validated normalized mapping.

    Raises:
        TypeError: If the kind or shape is invalid.
    """

    return _native.normalize_prebeta_shape(kind, value)


SessionInspectPhase = _Literal[
    "empty",
    "in_progress",
    "completed",
    "failed",
    "cancelled",
]


class SessionInspectSnapshot(TypedDict):
    """Replay-derived provisional session inspection snapshot."""

    session_id: str
    head_sequence: int
    phase: SessionInspectPhase
    result_text: _NotRequired[str]
    last_record_kind: _NotRequired[str]


class ParsedDocument(TypedDict):
    """Detailed result of a debug document parse."""

    markdown: str
    format: str
    page_count: int | None
    classification: str | None
    requires_ocr: bool
    truncated: bool


def parse_document_markdown(
    media_type: str, data: bytes | None = None, path: str | None = None
) -> str:
    """Debug helper: parse a document and return only its Markdown.

    Runs the same parser the document-ingest middleware uses, without
    constructing an ``Agent`` or ``Run``, so a developer can see exactly
    what would be injected for a given file.

    Args:
        media_type: Declared media type; a hint, not authoritative.
        data: In-memory bytes. Mutually exclusive with ``path``.
        path: Local file path read at call time, bounded to 4 MiB.
            Mutually exclusive with ``data``.

    Returns:
        The parsed GitHub-Flavored Markdown (possibly truncated).

    Raises:
        ValueError: Neither or both of ``data``/``path`` are given,
            ``path`` cannot be read or exceeds 4 MiB, or parsing fails
            with a stable ``document_*`` error code.
    """

    return _native.parse_document_markdown(media_type, data, path)


def parse_document(
    media_type: str, data: bytes | None = None, path: str | None = None
) -> ParsedDocument:
    """Debug helper: parse a document and return the full detailed result.

    Args:
        media_type: Declared media type; a hint, not authoritative.
        data: In-memory bytes. Mutually exclusive with ``path``.
        path: Local file path read at call time, bounded to 4 MiB.
            Mutually exclusive with ``data``.

    Returns:
        ``markdown``, ``format``, ``page_count``, ``classification``,
        ``requires_ocr``, and ``truncated``.

    Raises:
        ValueError: Neither or both of ``data``/``path`` are given,
            ``path`` cannot be read or exceeds 4 MiB, or parsing fails
            with a stable ``document_*`` error code.
    """

    return _native.parse_document(media_type, data, path)


def import_skill_markdown(source: str) -> Capability:
    """Parse one ``SKILL.md`` document into a declarative :class:`Capability`.

    The document is frontmatter (``name``, optional ``description``) plus a
    Markdown body that becomes the capability's instruction. Scripts and
    references are unsupported. The imported capability activates as
    ``application``.

    Args:
        source: The complete ``SKILL.md`` text.

    Returns:
        The parsed capability, ready for any agent factory's
        ``capabilities=[...]``.

    Raises:
        ValueError: The frontmatter or body is invalid.
    """

    return _native.import_skill_markdown(source)


__all__ = [
    "Agent",
    "HistoryCachePolicy",
    "ApprovalGrantMode",
    "Attachment",
    "BillingObserver",
    "BuildMetadata",
    "CalculatorToolset",
    "CallbackContext",
    "CancelledError",
    "Capability",
    "ChildRunPolicy",
    "CompactionMiddleware",
    "ConfigurationError",
    "E2bSandboxToolset",
    "ElicitationToolset",
    "Event",
    "EventBatch",
    "EventBatchIterator",
    "FileSystemToolset",
    "FinstackError",
    "HttpFetchToolset",
    "InstructionsMiddleware",
    "Lane",
    "LogObserver",
    "Locator",
    "McpToolset",
    "MemoryContextProvider",
    "MemoryExtension",
    "MemoryExternalIdentityMap",
    "MemoryObserver",
    "MemoryToolset",
    "MetricsObserver",
    "NotifyObserver",
    "OtelObserver",
    "ParsedDocument",
    "PydanticTool",
    "PythonContextProvider",
    "PythonMiddleware",
    "PythonModel",
    "PythonObserver",
    "PythonToolset",
    "RedactionMiddleware",
    "RepositoryContextProvider",
    "Run",
    "RunResult",
    "RuntimeError",
    "Session",
    "SessionInspectPhase",
    "SessionInspectSnapshot",
    "ShellToolset",
    "SkillsToolset",
    "SqliteDurability",
    "TimeoutError",
    "ToolPolicyMiddleware",
    "VerifyMiddleware",
    "__version__",
    "build_metadata",
    "health",
    "import_skill_markdown",
    "journal_known_answer",
    "linked_providers",
    "normalize_prebeta_shape",
    "parse_document",
    "parse_document_markdown",
    "pydantic_toolset",
    "tool",
]
