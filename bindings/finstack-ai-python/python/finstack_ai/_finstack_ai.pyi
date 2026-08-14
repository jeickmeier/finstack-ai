"""Native Rust-backed finstack-ai control and observation handles."""

from collections.abc import AsyncIterator, Awaitable, Callable
from typing import Any, Literal, TypedDict

__version__: str
__engine_version__: str

class FinstackError(Exception):
    """Base error raised by the Rust-owned semantic engine."""

    code: str
    retryable: bool
    context: dict[str, str] | None

class ConfigurationError(FinstackError):
    """Invalid immutable agent or run configuration."""

class RuntimeError(FinstackError):
    """Rust runtime execution failure."""

class CancelledError(FinstackError):
    """Run reached its durable cancelled terminal state."""

class TimeoutError(FinstackError):
    """Run exceeded its configured operational deadline."""

class CallbackContext:
    """Immutable identity and cancellation view for one callback invocation.

    A context is valid only while its callback is running. Access after the
    callback settles raises :class:`RuntimeError` with code
    ``python_callback_context_settled``.
    """

    @property
    def kind(self) -> str: ...
    @property
    def tenant_scope(self) -> str: ...
    @property
    def session_id(self) -> str: ...
    @property
    def lane_id(self) -> str: ...
    @property
    def run_id(self) -> str: ...
    @property
    def effect_id(self) -> str: ...
    @property
    def cancelled(self) -> bool: ...
    async def wait_cancelled(self) -> None:
        """Wait until cooperative cancellation reaches this invocation."""
    def to_dict(self) -> dict[str, str | bool]:
        """Copy the active invocation context into ordinary Python values."""

Callback = Callable[
    [CallbackContext, dict[str, Any]], dict[str, Any] | Awaitable[dict[str, Any]]
]

CapabilityActivation = Literal["always", "application", "model", "disabled"]

class CapabilityCatalogItem(TypedDict):
    """One compact model-visible capability entry."""

    id: str
    description: str

class ActiveCapability(TypedDict):
    """One Rust-owned capability activation committed for a run."""

    id: str
    source: Literal["always", "application", "model"]

class Capability:
    """Bounded declarative capability composed by the Rust SDK.

    Python capabilities in the alpha surface contribute instructions. Native
    bundle specifications may additionally contribute registered Toolset,
    ContextProvider, and Middleware references.
    """

    def __init__(
        self,
        id: str,
        description: str,
        instructions: list[str],
        *,
        activation: CapabilityActivation = "application",
    ) -> None: ...
    @property
    def id(self) -> str: ...
    @property
    def description(self) -> str: ...
    @property
    def activation(self) -> CapabilityActivation: ...

class PythonModel:
    """Trusted coarse Python implementation of the Rust Model port.

    The callback receives one normalized model draft and returns a mapping
    containing ``text``, ``completion_id``, and optional ``tool_calls``.
    Python callbacks run in-process and therefore have access to process memory.
    """

    def __init__(
        self,
        callback: Callback,
        *,
        component: str,
        provider: str,
        model: str,
        callback_timeout_seconds: float = 30.0,
        hard_input_bytes: int = 1_048_576,
        context_window_tokens: int = 8_192,
        max_output_tokens: int = 1_024,
    ) -> None: ...
    @property
    def component(self) -> str: ...
    @property
    def model(self) -> str: ...

class PythonToolset:
    """Trusted coarse Python implementation of the Rust Toolset port.

    Tool descriptors are normalized and cached at construction. The callback
    receives one validated tool call and returns ``output`` plus optional
    ``is_error``. Python callbacks run in-process and may access process memory.
    """

    def __init__(
        self,
        callback: Callback,
        *,
        component: str,
        name: str,
        tools: list[dict[str, Any]],
        callback_timeout_seconds: float = 30.0,
    ) -> None: ...
    @property
    def component(self) -> str: ...
    @property
    def tool_count(self) -> int: ...

class PythonContextProvider:
    """Trusted coarse Python implementation of the ContextProvider port.

    The callback receives a bounded normalized request and must return a
    normalized context contribution. Callbacks run in-process with host
    authority and are non-repeatable by default.
    """

    def __init__(
        self,
        callback: Callback,
        *,
        component: str,
        callback_timeout_seconds: float = 30.0,
        trusted_application_instructions: bool = False,
    ) -> None: ...
    @property
    def component(self) -> str: ...

class PythonMiddleware:
    """Trusted coarse Python implementation of selected middleware stages.

    Stage names are one or more of ``before_run``, ``prepare_context``,
    ``before_model``, ``after_model``, ``before_tool_batch``,
    ``after_tool_batch``, and ``before_finalize``. There is no per-token hook.
    """

    def __init__(
        self,
        callback: Callback,
        *,
        component: str,
        stages: list[str],
        priority: int = 0,
        callback_timeout_seconds: float = 30.0,
    ) -> None: ...
    @property
    def component(self) -> str: ...

ObserverCallback = Callable[[list[dict[str, Any]]], None | Awaitable[None]]

class PythonObserver:
    """Trusted read-only batched Python observer.

    ``payload_mode`` is ``metadata_only``, ``redacted``, or ``full``. Even
    ``full`` excludes credential-class event bodies. The observer receives
    logical batches rather than per-token callbacks.
    """

    def __init__(
        self,
        callback: ObserverCallback,
        *,
        component: str,
        payload_mode: str = "metadata_only",
        callback_timeout_seconds: float = 30.0,
    ) -> None: ...
    @property
    def component(self) -> str: ...

class Locator:
    """Immutable identifiers for one accepted operation."""

    @property
    def tenant_scope(self) -> str: ...
    @property
    def session_id(self) -> str: ...
    @property
    def lane_id(self) -> str: ...
    @property
    def run_id(self) -> str: ...
    def to_dict(self) -> dict[str, str]:
        """Serialize the identifier snapshot explicitly.

        Returns:
            ``tenant_scope``, ``session_id``, ``lane_id``, and ``run_id``.
        """

class Session:
    """Live handle for one journaled session."""

    @property
    def tenant_scope(self) -> str: ...
    @property
    def session_id(self) -> str: ...
    def create_lane(self, name: str, fork: str | None = None) -> Awaitable[Lane]:
        """Create a named lane, optionally forking from an existing entry.

        Args:
            name: Application lane name. ``main`` is reserved for bootstrap.
            fork: Optional existing entry identity to share without copying.

        Returns:
            The new live lane handle.

        Raises:
            ConfigurationError: The name is invalid, duplicated, or the fork
                entry is unknown.
        """
    def list_lanes(self) -> Awaitable[list[Lane]]:
        """List restored lanes.

        Returns:
            Live handles for every lane in the session.

        Raises:
            ConfigurationError: The session journal cannot be loaded.
        """
    def lane(self, name: str) -> Awaitable[Lane]:
        """Look up one lane by application name.

        Args:
            name: Application lane name.

        Returns:
            The live lane handle.

        Raises:
            ConfigurationError: The named lane does not exist.
        """
    def bind_external_identity(
        self,
        map: MemoryExternalIdentityMap,
        channel: str,
        account: str,
        thread: str,
        lane_id: str,
    ) -> None:
        """Bind a host-owned external identity to one lane.

        Args:
            map: In-process identity map owned by the host.
            channel: Channel or adapter name.
            account: Account identity on that channel.
            thread: Conversation or thread identity.
            lane_id: Durable lane identity in this session.

        Raises:
            ConfigurationError: The key is invalid, the lane is unknown, or
                the key is already bound to a different session lane.
        """
    @staticmethod
    def resolve_external_identity(
        map: MemoryExternalIdentityMap,
        channel: str,
        account: str,
        thread: str,
    ) -> tuple[str, str] | None:
        """Resolve a host-owned external identity key.

        Args:
            map: In-process identity map owned by the host.
            channel: Channel or adapter name.
            account: Account identity on that channel.
            thread: Conversation or thread identity.

        Returns:
            ``(session_id, lane_id)`` when the key is bound, otherwise
            ``None``.
        """

class Lane:
    """Live handle for one lane in a session."""

    @property
    def lane_id(self) -> str: ...
    @property
    def session(self) -> Session: ...
    def navigate(self, entry_id: str) -> Awaitable[None]:
        """Point this idle lane at an existing entry without copying.

        Args:
            entry_id: Existing conversation entry identity.

        Raises:
            ConfigurationError: The entry is unknown or the lane is busy.
        """
    def inspect(self) -> Awaitable[dict[str, object]]:
        """Inspect name, leaf, active run, and history length.

        Returns:
            A mapping with ``lane_id``, ``name``, ``leaf_id``,
            ``active_run_id``, and ``history_len``.

        Raises:
            ConfigurationError: The lane cannot be inspected.
        """

class MemoryExternalIdentityMap:
    """In-process external identity map."""

    def __init__(self) -> None: ...
    def resolve(
        self, channel: str, account: str, thread: str
    ) -> tuple[str, str] | None:
        """Resolve one previously bound key.

        Args:
            channel: Channel or adapter name.
            account: Account identity on that channel.
            thread: Conversation or thread identity.

        Returns:
            ``(session_id, lane_id)`` when the key is bound, otherwise
            ``None``.

        Raises:
            ConfigurationError: The key is empty, oversized, or contains NUL.
        """

class Event:
    """Immutable runtime event snapshot."""

    @property
    def kind(self) -> str: ...
    @property
    def event_class(self) -> str: ...
    @property
    def transient_sequence(self) -> int: ...
    @property
    def durable_sequence(self) -> int | None: ...
    def to_json(self) -> str:
        """Serialize the complete event explicitly."""

class EventBatch:
    """Immutable bounded transport batch."""

    @property
    def first_sequence(self) -> int: ...
    @property
    def last_sequence(self) -> int: ...
    @property
    def dropped_progress(self) -> int: ...
    def __len__(self) -> int: ...
    def events(self) -> list[Event]:
        """Expand the batch into individual immutable event snapshots."""
    def to_json(self) -> str:
        """Serialize the complete batch explicitly."""
    def to_json_bytes(self) -> bytes:
        """Serialize once and copy directly into Python bytes.

        Returns:
            UTF-8 JSON for every event in this transport batch.
        """

class EventBatchIterator(AsyncIterator[EventBatch]):
    """Single-consumer asynchronous batch iterator."""

    def __aiter__(self) -> EventBatchIterator: ...
    async def __anext__(self) -> EventBatch: ...

class RunResult:
    """Immutable successful terminal result snapshot."""

    @property
    def text(self) -> str: ...
    @property
    def output(self) -> Any | None:
        """Typed structured output selected by ``output_type``."""
    @property
    def retry_attempts(self) -> int:
        """Durable retry attempts consumed by this run."""
    @property
    def active_capabilities(self) -> list[ActiveCapability]:
        """Complete Rust-owned activation set committed for this run."""
    @property
    def trace(self) -> list[str]:
        """Stable Rust-owned committed record-kind trace in journal order."""
    @property
    def locator(self) -> Locator: ...
    @property
    def session(self) -> Locator: ...
    def to_dict(self) -> dict[str, str]:
        """Serialize the terminal result explicitly."""

class Run:
    """Shared control and observation handle for one Rust-owned run."""

    @property
    def session(self) -> Session: ...
    @property
    def locator(self) -> Locator: ...
    async def result(self) -> RunResult:
        """Wait for the retained terminal result."""
    async def list_interactions(self) -> list[dict[str, object]]:
        """List the outstanding typed interaction for this run.

        Rust owns routing. The result is the persisted request envelope
        (0 or 1 item), not a Python-owned queue.

        Returns:
            Zero or one interaction-request dictionaries for this run's
            locator.

        Raises:
            FinstackError: The authenticated locator cannot be listed.
        """
    async def resolve_interaction(self, resolution: dict[str, object]) -> None:
        """Resolve the outstanding interaction through the live run.

        A schema-valid approval denial is ``{"approved": false}``. The
        live worker coordinator stays authoritative; do not route a
        second coordinator against a live journal.

        Args:
            resolution: Binding-neutral resolution dictionary with
                ``interaction_id``, ``resolution_id``, ``principal``,
                ``authorization``, and ``response``.

        Raises:
            FinstackError: The handle is unavailable or the settlement
                is rejected as conflicting, expired, or unauthorized.
            TypeError: ``resolution`` is not a valid resolution shape.
        """
    async def cancel(self) -> None:
        """Submit idempotent durable cancellation."""
    def events(self) -> EventBatchIterator:
        """Return the batch-first asynchronous event iterator."""
    async def close_events(self) -> None:
        """Close event observation without cancelling execution."""

class Agent:
    """Immutable Rust-owned resolved agent handle."""

    @staticmethod
    async def openai_compatible(
        base_url: str,
        model: str,
        instruction: str | None = None,
        capabilities: list[Capability] | None = None,
        active_capabilities: list[str] | None = None,
    ) -> Agent:
        """Build a keyless Rust-backed OpenAI-compatible agent."""
    @staticmethod
    async def from_python(
        model: PythonModel,
        toolsets: list[PythonToolset] | None = None,
        instruction: str | None = None,
        output_type: Any | None = None,
        capabilities: list[Capability] | None = None,
        active_capabilities: list[str] | None = None,
    ) -> Agent:
        """Build an agent from trusted callbacks and optional Pydantic output type."""
    def capability_catalog(self) -> list[CapabilityCatalogItem]:
        """Return the bounded model-activated catalog in identity order."""
    def compact_capability_catalog(self) -> str:
        """Render the compact model-facing catalog without activation."""
    def create_session(self, tenant_scope: str = "default") -> Awaitable[Session]:
        """Create a live session on this agent's journal store.

        Args:
            tenant_scope: Tenant scope captured by the host.

        Returns:
            A live session handle with a bootstrapped ``main`` lane.

        Raises:
            ConfigurationError: The session cannot be created.
        """
    def open_session(self, session_id: str, tenant_scope: str) -> Awaitable[Session]:
        """Open an existing session without respawning parked runs.

        Args:
            session_id: Durable session identity.
            tenant_scope: Tenant scope captured by the host.

        Returns:
            A live session handle rebuilt from the journal.

        Raises:
            ConfigurationError: The session id is invalid or the journal
                cannot be replayed.
        """
    def start(
        self,
        input: str,
        *,
        timeout_seconds: float = 30.0,
        max_cycles: int = 16,
        max_output_retries: int = 1,
    ) -> Run:
        """Start a run and return its shared handle immediately."""
    async def run(
        self,
        input: str,
        *,
        timeout_seconds: float = 30.0,
        max_cycles: int = 16,
        max_output_retries: int = 1,
    ) -> RunResult:
        """Execute one run and await its committed result."""

def health() -> str:
    """Return ``\"ok\"`` without initializing runtime or network resources."""

def build_metadata() -> dict[str, str | bool | int]:
    """Return native build and compatibility metadata."""

def linked_providers() -> tuple[str, ...]:
    """Return curated Rust-backed providers linked into this extension."""

def journal_known_answer(kind: str, value: dict[str, object]) -> dict[str, object]:
    """Return payload digest, checksum, and canonical-CBOR hex from Rust."""

def normalize_prebeta_shape(kind: str, value: dict[str, object]) -> dict[str, object]:
    """Validate one Rust-owned pre-beta lineage or external-command shape."""

def _normalize_pydantic_schema(schema: dict[str, Any], kind: str) -> dict[str, Any]:
    """Normalize one generated schema into the portable Pydantic subset."""
