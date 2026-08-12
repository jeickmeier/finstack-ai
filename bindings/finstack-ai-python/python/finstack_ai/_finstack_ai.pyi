"""Native Rust-backed finstack-ai control and observation handles."""

from collections.abc import AsyncIterator, Awaitable, Callable
from typing import Any

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

class Session:
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
        """Serialize the identifier snapshot explicitly."""

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
    def session(self) -> Session: ...
    def to_dict(self) -> dict[str, str]:
        """Serialize the terminal result explicitly."""

class Run:
    """Shared control and observation handle for one Rust-owned run."""

    @property
    def session(self) -> Session: ...
    async def result(self) -> RunResult:
        """Wait for the retained terminal result."""
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
        base_url: str, model: str, instruction: str | None = None
    ) -> Agent:
        """Build a keyless Rust-backed OpenAI-compatible agent."""
    @staticmethod
    async def from_python(
        model: PythonModel,
        toolsets: list[PythonToolset] | None = None,
        instruction: str | None = None,
    ) -> Agent:
        """Build an agent from trusted coarse Python callbacks."""
    def start(
        self,
        input: str,
        *,
        timeout_seconds: float = 30.0,
        max_cycles: int = 16,
    ) -> Run:
        """Start a run and return its shared handle immediately."""
    async def run(
        self,
        input: str,
        *,
        timeout_seconds: float = 30.0,
        max_cycles: int = 16,
    ) -> RunResult:
        """Execute one run and await its committed result."""

def health() -> str:
    """Return ``\"ok\"`` without initializing runtime or network resources."""

def build_metadata() -> dict[str, str | bool | int]:
    """Return native build and compatibility metadata."""

def linked_providers() -> tuple[str, ...]:
    """Return curated Rust-backed providers linked into this extension."""

def normalize_prebeta_shape(kind: str, value: dict[str, object]) -> dict[str, object]:
    """Validate one Rust-owned pre-beta lineage or external-command shape."""
