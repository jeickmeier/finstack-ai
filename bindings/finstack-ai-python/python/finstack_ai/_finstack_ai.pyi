"""Native Rust-backed finstack-ai control and observation handles."""

from collections.abc import AsyncIterator

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
