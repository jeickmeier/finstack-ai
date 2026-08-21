"""Native Rust-backed finstack-ai control and observation handles."""

from collections.abc import AsyncIterator, Awaitable, Callable
from typing import Any, Literal, TypedDict

__version__: str
__engine_version__: str

class FinstackError(Exception):
    """Base error raised by the Rust-owned semantic engine.

    Attributes:
        code: Stable error code string shared with Rust and JavaScript.
        retryable: Whether the caller may retry the same operation.
        context: Optional secret-free locator fields.
    """

    code: str
    retryable: bool
    context: dict[str, str] | None

class ConfigurationError(FinstackError):
    """Invalid immutable agent or run configuration.

    Typical ``code`` is ``agent_run_invalid_configuration``.
    """

class RuntimeError(FinstackError):
    """Rust runtime execution failure.

    Includes callback-context settlement (``python_callback_context_settled``).
    """

class CancelledError(FinstackError):
    """Run reached its durable cancelled terminal state.

    Typical ``code`` is ``agent_run_cancelled``.
    """

class TimeoutError(FinstackError):
    """Run exceeded its configured operational deadline.

    Typical ``code`` is ``agent_run_timeout``.
    """

class CallbackContext:
    """Immutable identity and cancellation view for one callback invocation.

    A context is valid only while its callback is running. Access after the
    callback settles raises :class:`RuntimeError` with code
    ``python_callback_context_settled``.
    """

    @property
    def kind(self) -> str:
        """Port kind for this invocation (``model``, ``toolset``, …)."""
    @property
    def tenant_scope(self) -> str:
        """Host-captured tenant scope."""
    @property
    def session_id(self) -> str:
        """Durable session identity."""
    @property
    def lane_id(self) -> str:
        """Durable lane identity."""
    @property
    def run_id(self) -> str:
        """Durable run identity."""
    @property
    def effect_id(self) -> str:
        """Committed effect identity for this invocation."""
    @property
    def cancelled(self) -> bool:
        """Whether cooperative cancellation has been observed."""
    async def wait_cancelled(self) -> None:
        """Wait until cooperative cancellation reaches this invocation.

        Raises:
            RuntimeError: The context is already settled
                (``python_callback_context_settled``).
        """
    def to_dict(self) -> dict[str, str | bool]:
        """Copy the active invocation context into ordinary Python values.

        Returns:
            Identity and cancellation fields valid at copy time.

        Raises:
            RuntimeError: The context is already settled.
        """

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
    """Instruction-only declarative capability composed by the Rust SDK.

    Python stays instruction-only: ``id``, ``description``,
    ``instructions``, and ``activation``. Native bundle specifications
    may additionally contribute registered Toolset, ContextProvider, and
    Middleware references.
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

class ElicitationToolset:
    """Human-in-the-loop elicitation toolset backed by the Rust implementation.

    ``ask_user=True`` exposes the free-form ``ask_user`` tool; ``tools``
    registers typed per-workflow elicitation tools whose response schemas are
    fixed at registration. Calls park the run as a pending interaction; the
    resolution response becomes the tool result.
    """

    def __init__(
        self,
        *,
        ask_user: bool = False,
        tools: list[dict[str, Any]] | None = None,
    ) -> None: ...
    @property
    def component(self) -> str: ...
    @property
    def tool_count(self) -> int: ...

class MemoryExtension:
    """Native memory composition: one store, one scope, one policy.

    The three accessors return handles accepted directly by the agent
    factories' ``toolsets``, ``context_providers``, and ``observers``
    parameters. All handles share the extension's store, so one extension
    can back several agents.
    """

    @staticmethod
    def in_process(
        *,
        tenant: str = "python-local",
        user: str | None = None,
        agent: str | None = None,
        workspace: str | None = None,
        read: bool = True,
        write: bool = True,
        manage: bool = False,
    ) -> MemoryExtension:
        """Build an extension over a process-local, non-durable store.

        ``tenant`` must equal the tenant scope of the runs that recall from
        it. It defaults to ``"python-local"``, the scope :meth:`Agent.run`
        and :meth:`Agent.start` use; pass the session's tenant scope
        instead when the recalling runs execute on a lane. Recall on a
        mismatched tenant fails the run with
        ``context_contribution_invalid``.
        """

    @staticmethod
    def sqlite(
        *,
        path: str,
        tenant: str = "python-local",
        user: str | None = None,
        agent: str | None = None,
        workspace: str | None = None,
        read: bool = True,
        write: bool = True,
        manage: bool = False,
    ) -> MemoryExtension:
        """Build an extension over a durable SQLite store at ``path``.

        The file and its schema are created on first open.

        ``tenant`` must equal the tenant scope of the runs that recall from
        it. It defaults to ``"python-local"``, the scope :meth:`Agent.run`
        and :meth:`Agent.start` use; pass the session's tenant scope
        instead when the recalling runs execute on a lane. Recall on a
        mismatched tenant fails the run with
        ``context_contribution_invalid``.
        """

    @property
    def tenant(self) -> str: ...
    def context_provider(self, *, max_hits: int | None = None) -> MemoryContextProvider:
        """Return the recall provider handle for ``context_providers``."""

    def toolset(self) -> MemoryToolset:
        """Return the memory toolset handle for ``toolsets``."""

    def observer(self) -> MemoryObserver:
        """Return the capture observer handle for ``observers``."""

class MemoryContextProvider:
    """Recall provider handle produced by :meth:`MemoryExtension.context_provider`."""

    @property
    def component(self) -> str: ...

class MemoryToolset:
    """Memory toolset handle produced by :meth:`MemoryExtension.toolset`.

    The native toolset is materialized at agent-assembly time so it shares
    the agent's artifact store.
    """

    @property
    def component(self) -> str: ...
    @property
    def tool_count(self) -> int: ...

class HttpFetchToolset:
    """Bounded, allowlisted HTTP fetch toolset backed by the Rust implementation.

    Built from a JSON configuration document: ``allowlist`` (required,
    non-empty), and optional ``max_response_bytes``, ``request_timeout_ms``,
    ``max_redirects``, ``per_host_headers``, ``user_agent``. The JSON document
    does NOT accept ``allow_loopback_http`` (an unknown-key error if it does)
    — that privilege is intentionally not data-configurable. Fixtures that
    need it pass the keyword-only ``insecure_allow_loopback_http=True``
    instead; it is applied to the parsed config after the JSON is parsed,
    never read from the JSON payload itself. **Fixtures only. Never enable in
    production.**

    This constructor never attaches an artifact store, so ``mode: "artifact"``
    and any binary (or invalid-UTF-8) response body always fail with
    ``fetch_limit_exceeded`` in v1 — there is no store to stage them to.
    """

    def __init__(
        self, config_json: str, *, insecure_allow_loopback_http: bool = False
    ) -> None: ...
    @property
    def component(self) -> str: ...
    @property
    def tool_count(self) -> int: ...

class E2bSandboxToolset:
    """Composable T4 E2B sandbox toolset for a real model-backed agent."""

    def __init__(
        self,
        api_key: str,
        *,
        endpoint: str | None = None,
        template: str | None = None,
    ) -> None: ...
    @property
    def component(self) -> str: ...
    @property
    def tool_count(self) -> int: ...

class MemoryObserver:
    """Capture observer handle produced by :meth:`MemoryExtension.observer`."""

    @property
    def component(self) -> str: ...

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
    def lane_by_id(self, lane_id: str) -> Awaitable[Lane]:
        """Look up one lane by durable identity.

        Args:
            lane_id: Durable lane identity.

        Returns:
            The live lane handle.

        Raises:
            ConfigurationError: The identity is invalid or the lane does
                not exist.
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
    def append_text(self, text: str) -> Awaitable[str]:
        """Append one user text message on this idle lane.

        This does not start a run.

        Args:
            text: Non-empty user text.

        Returns:
            The durable entry identity.

        Raises:
            ConfigurationError: The lane is busy or the text is invalid.
        """
    def run(
        self,
        agent: Agent,
        input: str,
        *,
        timeout_seconds: float | None = None,
        max_cycles: int = 16,
        max_output_retries: int = 1,
        capability: str | None = None,
        attachments: list[Attachment] | None = None,
    ) -> Run:
        """Start a new root run on this idle lane.

        Dropping the returned handle detaches observation; it does not cancel
        the durable run. Call :meth:`Run.cancel` for explicit cancellation.

        Args:
            agent: Resolved agent that owns the model and journal store.
            input: Non-empty plain-text user input.
            timeout_seconds: Operational deadline in seconds. ``None`` uses
                the agent default. Must be positive when set.
            max_cycles: Maximum model cycles.
            max_output_retries: Maximum structured-output retries.
            capability: Optional model-activated capability id.
            attachments: Optional pre-built attachments, staged and mapped
                to `File` blocks on the run's user message. At most 8.

        Returns:
            A shared :class:`Run` handle.

        Raises:
            ConfigurationError: Input, limits, tenant, ``capability``, or
                ``attachments`` are invalid, or the lane is busy.
            RuntimeError: The background task cannot be accepted.
        """
    def suspend(self) -> Awaitable[None]:
        """Park the in-process driver without dropping the journal.

        The lane's active or suspended run remains durable.
        :meth:`resume` respawns the owner.

        Raises:
            ConfigurationError: The journal cannot be loaded or the lane
                lock is poisoned.
        """
    def resume(self, agent: Agent) -> Awaitable[None]:
        """Recover the parked run and respawn the in-process owner.

        ``Agent.open_session`` still inspects only. Call this after open
        to continue a parked run.

        Args:
            agent: Resolved agent that supplies model and tool ports.

        Raises:
            ConfigurationError: The lane has no suspended run.
            RuntimeError: Restore or spawn fails.
        """

class SqliteDurability:
    """Durability policy applied when the binding opens SQLite.

    ``Durable`` is WAL plus ``synchronous=FULL`` and is the only mode that
    may advertise durable health. ``Relaxed`` is a named non-durable mode
    and must never advertise NFR-REL-001. ``:memory:`` is allowed only
    with ``Relaxed``.
    """

    Durable: SqliteDurability
    Relaxed: SqliteDurability

class ChildRunPolicy:
    """Child-run admission policy frozen into agent composition.

    The default for every factory is :meth:`deny`. Pass
    :meth:`allow` to admit isolated or compatible children up to an
    inclusive depth.
    """

    @staticmethod
    def deny() -> ChildRunPolicy:
        """Reject every child invocation.

        Returns:
            A deny policy. ``Run.start_child`` fails closed.
        """

    @staticmethod
    def allow(max_depth: int) -> ChildRunPolicy:
        """Allow children up to the inclusive ``max_depth``.

        Args:
            max_depth: Inclusive child depth accepted by this agent.

        Returns:
            An allow policy consumed by the agent factories.
        """

class ApprovalGrantMode:
    """How a run parks and releases paid-tool approvals.

    Maps onto Rust ``RunPolicy.approval_grant``. The default for every
    factory is :meth:`per_call`: one park per unpaid Policy or Required
    tool call. :meth:`informed_batch` parks once listing every unpaid
    paid tool. Neither mode relaxes the ``Policy`` approval floor on
    the catalog.

    Examples:
        >>> from finstack_ai import ApprovalGrantMode
        >>> ApprovalGrantMode.per_call() is not None
        True
        >>> ApprovalGrantMode.informed_batch() is not None
        True
    """

    @staticmethod
    def per_call() -> ApprovalGrantMode:
        """Park once per unpaid paid tool call.

        Returns:
            A per-call grant mode consumed by the agent factories as
            ``RunPolicy.approval_grant``.
        """

    @staticmethod
    def informed_batch() -> ApprovalGrantMode:
        """Park once listing every unpaid paid tool call.

        Returns:
            An informed-batch grant mode consumed by the agent factories
            as ``RunPolicy.approval_grant``.
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

class Attachment:
    """One in-memory run attachment staged at submit time.

    Exactly one of ``data``/``path`` is required. A ``path`` is read
    (bounded to 4 MiB) at construction time; its basename becomes the
    default ``name`` when ``name`` is not given explicitly.
    """

    def __init__(
        self,
        media_type: str,
        data: bytes | None = None,
        path: str | None = None,
        name: str | None = None,
    ) -> None:
        """Construct one attachment.

        Args:
            media_type: Blob media type.
            data: In-memory bytes. Mutually exclusive with ``path``.
            path: Local file path read at construction, bounded to 4 MiB.
                Mutually exclusive with ``data``.
            name: Optional display name. Defaults to ``path``'s basename
                when ``path`` is given and ``name`` is omitted.

        Raises:
            ValueError: Neither or both of ``data``/``path`` are given, or
                ``path`` cannot be read, or exceeds 4 MiB.
        """

class ObserverDiagnostic(TypedDict):
    """One stable, redacted observer-delivery diagnostic."""

    code: str
    detail: str

class ObserverDiagnostics(TypedDict):
    """Bounded process-local observer diagnostic snapshot."""

    total: int
    dropped: int
    recent: list[ObserverDiagnostic]

class RunStateSnapshot(TypedDict):
    """Latest confirmed semantic and runtime lifecycle state."""

    revision: int
    journal_sequence: int
    status: Literal["running", "shutting_down", "stopped", "faulted"]
    fault_code: str | None
    phase: str | None
    cycle: int
    prepared_context_messages: list[dict[str, object]]
    committed_run_messages: list[dict[str, object]]
    active_capabilities: list[dict[str, object]]
    resolved_plan_digest: str | None
    pending_interaction: dict[str, object] | None
    validation_failure: dict[str, object] | None
    retry_attempts: int
    terminal: dict[str, object] | None

class Run:
    """Shared control and observation handle for one Rust-owned run."""

    @property
    def session(self) -> Session: ...
    @property
    def locator(self) -> Locator: ...
    async def result(self) -> RunResult:
        """Wait for the retained terminal result.

        Returns:
            An immutable snapshot. Structured ``output`` is present when
            ``output_type`` was configured.

        Raises:
            RuntimeError: The runtime failed after accept.
            CancelledError: The run reached its durable cancelled terminal.
            TimeoutError: The operational deadline elapsed.
        """
    async def live_state(self) -> RunStateSnapshot:
        """Read the latest confirmed run-state snapshot."""
    async def wait_for_live_state(self, revision: int) -> RunStateSnapshot:
        """Wait until the latest-only view advances beyond ``revision``."""
    async def observer_diagnostics(self) -> ObserverDiagnostics:
        """Snapshot bounded, redacted observer-delivery diagnostics.

        The snapshot is process-local and non-semantic. Reading it does not
        affect the journal, kernel state, run result, or best-effort observer
        delivery.

        Returns:
            Total failures, count evicted from the bounded recent list, and
            recent stable ``code``/``detail`` dictionaries in source order.

        Raises:
            RuntimeError: Run startup failed before a runtime handle was
                published.
        """
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
    async def start_child(
        self,
        agent: Agent,
        input: str,
        *,
        placement: str = "isolated_child_session",
        timeout_seconds: float | None = None,
        max_cycles: int = 16,
        max_output_retries: int = 1,
        capability: str | None = None,
        route_endpoint: str | None = None,
        route_service: str | None = None,
        route_id: str | None = None,
        route_token: str | None = None,
    ) -> Run:
        """Prepare and accept one child run through the Rust router.

        Rust commits ``ChildRunPrepared`` then starts the child on the
        frozen locator. This is not a JSON round-trip.

        Args:
            agent: Child agent composition. The parent journal store is
                authoritative; the child's store is not used.
            input: Child user text.
            placement: ``isolated_child_session`` (default),
                ``compatible_lane_in_parent_session``, or
                ``remote_child_session``. Remote placement requires
                ``route_endpoint``, ``route_service``, and ``route_id``.
                Isolated keeps the parent journal operable after accept.
            timeout_seconds: Child operational deadline. ``None`` uses the
                child agent's default.
            max_cycles: Maximum child model cycles.
            max_output_retries: Maximum structured-output retries.
            capability: Optional model-activated child variant.
            route_endpoint: Loopback ``host:port`` or ``unix:/path``.
            route_service: Remote service component id.
            route_id: Opaque non-secret route handle.
            route_token: Optional Bearer token. Never read from env.

        Returns:
            The live child :class:`Run`.

        Raises:
            FinstackError: Prepare or accept failed.
            TypeError: ``placement`` is not a supported value.
        """
    async def complete_external(self, command: dict[str, object]) -> dict[str, str]:
        """Route one authenticated external completion through Rust.

        ``normalize_prebeta_shape("external_effect_completion", command)``
        remains a validator in front of this router.

        Args:
            command: Normalized completion dictionary with ``locator``,
                ``principal``, ``authorization``, and ``completion``.

        Returns:
            A mapping with ``status`` of ``committed``, ``idempotent``,
            or ``rejected``.

        Raises:
            FinstackError: Ingress rejected the command or the locator
                does not match this run.
            TypeError: ``command`` is not a valid completion shape.
        """
    async def cancel(self) -> None:
        """Submit idempotent durable cancellation.

        Dropping this handle does not cancel. This method records intent.

        Raises:
            RuntimeError: Cancellation cannot be recorded.
        """
    def events(self) -> EventBatchIterator:
        """Return the batch-first asynchronous event iterator.

        Returns:
            A single-consumer iterator of :class:`EventBatch` values.
        """
    async def close_events(self) -> None:
        """Close event observation without cancelling execution."""

class Agent:
    """Immutable Rust-owned resolved agent handle."""

    @staticmethod
    async def openai(
        model: str,
        instruction: str | None = None,
        capabilities: list[Capability] | None = None,
        active_capabilities: list[str] | None = None,
        *,
        api_key: str,
        reasoning_effort: str | None = None,
        reasoning_summary: str | None = None,
        media_tools: bool = False,
        openrouter_media_api_key: str | None = None,
        openrouter_media_referer: str | None = None,
        openrouter_media_title: str | None = None,
        toolsets: list[
            PythonToolset
            | ElicitationToolset
            | MemoryToolset
            | HttpFetchToolset
            | E2bSandboxToolset
        ]
        | None = None,
        context_providers: list[PythonContextProvider | MemoryContextProvider]
        | None = None,
        middleware: list[PythonMiddleware] | None = None,
        observers: list[PythonObserver | MemoryObserver] | None = None,
        output_type: Any | None = None,
        child_runs: ChildRunPolicy | None = None,
        approval_grant: ApprovalGrantMode | None = None,
    ) -> Agent:
        """Build a Rust-backed official OpenAI Responses agent.

        The native client posts to ``https://api.openai.com/v1/responses``
        with ``store=false``. Keyword-only ``toolsets``,
        ``context_providers``, ``middleware``, ``observers``, and
        ``output_type`` register the same trusted T2 Python ports as
        :meth:`Agent.from_python`. This factory does not read environment
        variables and does not accept a generic ``base_url``. Output is
        capped at 128,000 tokens while the linked context window is
        1,050,000 tokens.

        Args:
            model: Official OpenAI model name.
            instruction: Optional stable instruction prefix.
            capabilities: Optional declarative capability catalog.
            active_capabilities: Application capability ids to activate.
            api_key: Required Bearer credential. HTTPS is required.
            reasoning_effort: Optional Responses ``reasoning.effort``.
                Allowed values are ``none``, ``minimal``, ``low``,
                ``medium``, ``high``, ``xhigh``, and ``max``. Omit to use the
                provider default.
            reasoning_summary: Optional Responses ``reasoning.summary``.
            media_tools: Register the native OpenAI media toolset
                (``openai_generate_image``, ``openai_generate_speech``,
                ``openai_transcribe_audio``) alongside the model, reusing
                ``api_key``.
            openrouter_media_api_key: Optional explicit OpenRouter API key.
                When set, registers the OpenRouter media-generation toolset
                (image, speech, video, and transcription tools) billed to
                this key, independent of ``api_key``.
            openrouter_media_referer: Optional non-secret ``HTTP-Referer``
                attribution header for the OpenRouter media toolset.
                Requires ``openrouter_media_api_key``.
            openrouter_media_title: Optional non-secret ``X-Title``
                attribution header for the OpenRouter media toolset.
                Requires ``openrouter_media_api_key``.
            toolsets: Optional trusted Python toolset callbacks.
            context_providers: Optional trusted context-provider callbacks.
            middleware: Optional trusted middleware callbacks.
            observers: Optional trusted observer callbacks.
            output_type: Optional Pydantic output type. Lazily requires the
                Pydantic extra.
            child_runs: Optional child-run admission policy. Defaults to
                :meth:`ChildRunPolicy.deny`.
            approval_grant: Optional paid-tool approval grant mode. Defaults
                to :meth:`ApprovalGrantMode.per_call`.

        Returns:
            An immutable Rust-owned agent handle.

        Raises:
            ConfigurationError: The credential, model, capability set, or
                port registration is invalid.
            ValueError: ``openrouter_media_referer`` or
                ``openrouter_media_title`` is set without
                ``openrouter_media_api_key``.
        """
    @staticmethod
    async def openrouter(
        model: str,
        instruction: str | None = None,
        capabilities: list[Capability] | None = None,
        active_capabilities: list[str] | None = None,
        *,
        api_key: str,
        referer: str | None = None,
        title: str | None = None,
        reasoning_effort: str | None = None,
        reasoning_summary: str | None = None,
        media_tools: bool = False,
        toolsets: list[
            PythonToolset
            | ElicitationToolset
            | MemoryToolset
            | HttpFetchToolset
            | E2bSandboxToolset
        ]
        | None = None,
        context_providers: list[PythonContextProvider | MemoryContextProvider]
        | None = None,
        middleware: list[PythonMiddleware] | None = None,
        observers: list[PythonObserver | MemoryObserver] | None = None,
        output_type: Any | None = None,
        child_runs: ChildRunPolicy | None = None,
        approval_grant: ApprovalGrantMode | None = None,
    ) -> Agent:
        """Build a Rust-backed OpenRouter Responses agent.

        The native client posts to
        ``https://openrouter.ai/api/v1/responses`` and omits ``store``
        entirely (the request is stateless; ``store`` is a reserved
        provider-settings field). Keyword-only ``toolsets``,
        ``context_providers``, ``middleware``,
        ``observers``, and ``output_type`` register the same trusted T2
        Python ports as :meth:`Agent.from_python`. This factory does not
        read environment variables and does not accept a generic
        ``base_url``. Output is capped at 128,000 tokens while the linked
        context window is 1,050,000 tokens. This factory does not attach
        a ``MediaResolver``. Vision, file, and audio input require a
        host-built Rust provider with ``with_media_resolver``. ADR-049
        rejected FFI resolvers on linked constructors. ``media_tools``
        registers outbound media-generation tools only.

        Args:
            model: OpenRouter model name.
            instruction: Optional stable instruction prefix.
            capabilities: Optional declarative capability catalog.
            active_capabilities: Application capability ids to activate.
            api_key: Required Bearer credential. HTTPS is required.
            referer: Optional non-secret ``HTTP-Referer`` attribution header.
            title: Optional non-secret ``X-Title`` attribution header.
            reasoning_effort: Optional Responses ``reasoning.effort``.
                Allowed values are ``none``, ``minimal``, ``low``,
                ``medium``, ``high``, ``xhigh``, and ``max``. Omit to use the
                provider default.
            reasoning_summary: Optional Responses ``reasoning.summary``.
            media_tools: Register the OpenRouter media-generation toolset
                (image, speech, video, and transcription tools) alongside
                the model, reusing ``api_key``, ``referer``, and ``title``.
            toolsets: Optional trusted Python toolset callbacks.
            context_providers: Optional trusted context-provider callbacks.
            middleware: Optional trusted middleware callbacks.
            observers: Optional trusted observer callbacks.
            output_type: Optional Pydantic output type. Lazily requires the
                Pydantic extra.
            child_runs: Optional child-run admission policy. Defaults to
                :meth:`ChildRunPolicy.deny`.
            approval_grant: Optional paid-tool approval grant mode. Defaults
                to :meth:`ApprovalGrantMode.per_call`.

        Returns:
            An immutable Rust-owned agent handle.

        Raises:
            ConfigurationError: The credential, model, capability set, or
                port registration is invalid.
        """
    @staticmethod
    async def anthropic(
        base_url: str,
        model: str,
        api_key: str | None = None,
        instruction: str | None = None,
        capabilities: list[Capability] | None = None,
        active_capabilities: list[str] | None = None,
        *,
        openrouter_media_api_key: str | None = None,
        openrouter_media_referer: str | None = None,
        openrouter_media_title: str | None = None,
        toolsets: list[
            PythonToolset
            | ElicitationToolset
            | MemoryToolset
            | HttpFetchToolset
            | E2bSandboxToolset
        ]
        | None = None,
        context_providers: list[PythonContextProvider | MemoryContextProvider]
        | None = None,
        middleware: list[PythonMiddleware] | None = None,
        observers: list[PythonObserver | MemoryObserver] | None = None,
        output_type: Any | None = None,
        child_runs: ChildRunPolicy | None = None,
        approval_grant: ApprovalGrantMode | None = None,
    ) -> Agent:
        """Build a Rust-backed Anthropic Messages agent.

        Construction of the HTTP client happens only in this factory. Importing
        ``finstack_ai`` does not open sockets or start Tokio. The native
        provider is T1. Keyword-only port lists register the same T2 Python
        callbacks as :meth:`Agent.from_python`. This factory does not read
        environment variables. Output is capped at 64,000 tokens, the current
        Claude ceiling, while the linked context window is 1,050,000 tokens.

        Args:
            base_url: Anthropic Messages base URL.
            model: Provider model name.
            api_key: Optional ``x-api-key``. HTTPS is required when set.
                Keyless HTTP loopback is allowed. HTTP plus a key raises
                :class:`ConfigurationError` and does not include the secret
                in ``str`` or ``repr``.
            instruction: Optional stable instruction prefix.
            capabilities: Optional declarative capability catalog.
            active_capabilities: Application capability ids to activate.
            openrouter_media_api_key: Optional explicit OpenRouter API key.
                When set, registers the OpenRouter media-generation toolset
                (image, speech, video, and transcription tools) billed to
                this key.
            openrouter_media_referer: Optional non-secret ``HTTP-Referer``
                attribution header for the OpenRouter media toolset.
                Requires ``openrouter_media_api_key``.
            openrouter_media_title: Optional non-secret ``X-Title``
                attribution header for the OpenRouter media toolset.
                Requires ``openrouter_media_api_key``.
            toolsets: Optional trusted Python toolset callbacks.
            context_providers: Optional trusted context-provider callbacks.
            middleware: Optional trusted middleware callbacks.
            observers: Optional trusted observer callbacks.
            output_type: Optional Pydantic output type. Lazily requires the
                Pydantic extra.
            child_runs: Optional child-run admission policy. Defaults to
                :meth:`ChildRunPolicy.deny`.
            approval_grant: Optional paid-tool approval grant mode. Defaults
                to :meth:`ApprovalGrantMode.per_call`.

        Returns:
            An immutable Rust-owned agent handle.

        Raises:
            ConfigurationError: The endpoint, credential, model, capability
                set, or port registration is invalid.
            ValueError: ``openrouter_media_referer`` or
                ``openrouter_media_title`` is set without
                ``openrouter_media_api_key``.
        """
    @staticmethod
    async def gemini(
        endpoint: str,
        model: str,
        api_key: str | None = None,
        instruction: str | None = None,
        capabilities: list[Capability] | None = None,
        active_capabilities: list[str] | None = None,
        *,
        openrouter_media_api_key: str | None = None,
        openrouter_media_referer: str | None = None,
        openrouter_media_title: str | None = None,
        toolsets: list[
            PythonToolset | ElicitationToolset | HttpFetchToolset | E2bSandboxToolset
        ]
        | None = None,
        context_providers: list[PythonContextProvider] | None = None,
        middleware: list[PythonMiddleware] | None = None,
        observers: list[PythonObserver] | None = None,
        output_type: Any | None = None,
        child_runs: ChildRunPolicy | None = None,
        approval_grant: ApprovalGrantMode | None = None,
    ) -> Agent:
        """Build a Rust-backed Gemini ``generateContent`` agent.

        Construction of the HTTP client happens only in this factory. Importing
        ``finstack_ai`` does not open sockets or start Tokio. Keyword-only port
        lists register the same T2 Python callbacks as
        :meth:`Agent.from_python`. This factory does not read environment
        variables and does not hardcode the Google host: ``endpoint`` is
        passed straight into the provider's ``GeminiConfig::try_new``.

        Args:
            endpoint: Gemini ``generateContent`` base URL (Generative
                Language API).
            model: Provider model name.
            api_key: Optional API key. HTTPS is required when set. Keyless
                HTTP loopback is allowed. HTTP plus a key raises
                :class:`ConfigurationError` and does not include the secret
                in ``str`` or ``repr``.
            instruction: Optional stable instruction prefix.
            capabilities: Optional declarative capability catalog.
            active_capabilities: Application capability ids to activate.
            openrouter_media_api_key: Optional explicit OpenRouter API key.
                When set, registers the OpenRouter media-generation toolset
                (image, speech, video, and transcription tools) billed to
                this key.
            openrouter_media_referer: Optional non-secret ``HTTP-Referer``
                attribution header for the OpenRouter media toolset.
                Requires ``openrouter_media_api_key``.
            openrouter_media_title: Optional non-secret ``X-Title``
                attribution header for the OpenRouter media toolset.
                Requires ``openrouter_media_api_key``.
            toolsets: Optional trusted Python toolset callbacks.
            context_providers: Optional trusted context-provider callbacks.
            middleware: Optional trusted middleware callbacks.
            observers: Optional trusted observer callbacks.
            output_type: Optional Pydantic output type. Lazily requires the
                Pydantic extra.
            child_runs: Optional child-run admission policy. Defaults to
                :meth:`ChildRunPolicy.deny`.
            approval_grant: Optional paid-tool approval grant mode. Defaults
                to :meth:`ApprovalGrantMode.per_call`.

        Returns:
            An immutable Rust-owned agent handle.

        Raises:
            ConfigurationError: The endpoint, credential, model, capability
                set, or port registration is invalid.
            ValueError: ``openrouter_media_referer`` or
                ``openrouter_media_title`` is set without
                ``openrouter_media_api_key``.
        """
    @staticmethod
    async def ollama(
        base_url: str,
        model: str,
        instruction: str | None = None,
        capabilities: list[Capability] | None = None,
        active_capabilities: list[str] | None = None,
        *,
        openrouter_media_api_key: str | None = None,
        openrouter_media_referer: str | None = None,
        openrouter_media_title: str | None = None,
        toolsets: list[
            PythonToolset | ElicitationToolset | MemoryToolset | E2bSandboxToolset
        ]
        | None = None,
        context_providers: list[PythonContextProvider | MemoryContextProvider]
        | None = None,
        middleware: list[PythonMiddleware] | None = None,
        observers: list[PythonObserver | MemoryObserver] | None = None,
        output_type: Any | None = None,
        child_runs: ChildRunPolicy | None = None,
        approval_grant: ApprovalGrantMode | None = None,
    ) -> Agent:
        """Build a keyless Rust-backed native Ollama agent.

        Uses Ollama ``/api/chat``. This factory does not accept an API key.
        Keyword-only port lists register the same T2 Python callbacks as
        :meth:`Agent.from_python`.

        Args:
            base_url: Ollama base URL, typically ``http://127.0.0.1:11434``.
            model: Provider model name.
            instruction: Optional stable instruction prefix.
            capabilities: Optional declarative capability catalog.
            active_capabilities: Application capability ids to activate.
            openrouter_media_api_key: Optional explicit OpenRouter API key.
                When set, registers the OpenRouter media-generation toolset
                (image, speech, video, and transcription tools) billed to
                this key.
            openrouter_media_referer: Optional non-secret ``HTTP-Referer``
                attribution header for the OpenRouter media toolset.
                Requires ``openrouter_media_api_key``.
            openrouter_media_title: Optional non-secret ``X-Title``
                attribution header for the OpenRouter media toolset.
                Requires ``openrouter_media_api_key``.
            toolsets: Optional trusted Python toolset callbacks.
            context_providers: Optional trusted context-provider callbacks.
            middleware: Optional trusted middleware callbacks.
            observers: Optional trusted observer callbacks.
            output_type: Optional Pydantic output type. Lazily requires the
                Pydantic extra.
            child_runs: Optional child-run admission policy. Defaults to
                :meth:`ChildRunPolicy.deny`.
            approval_grant: Optional paid-tool approval grant mode. Defaults
                to :meth:`ApprovalGrantMode.per_call`.

        Returns:
            An immutable Rust-owned agent handle.

        Raises:
            ConfigurationError: The endpoint, model, capability set, or port
                registration is invalid.
            ValueError: ``openrouter_media_referer`` or
                ``openrouter_media_title`` is set without
                ``openrouter_media_api_key``.
        """
    @staticmethod
    async def gateway(
        endpoint: str,
        model: str,
        instruction: str | None = None,
        capabilities: list[Capability] | None = None,
        active_capabilities: list[str] | None = None,
        *,
        wire_protocol: str,
        credential_name: str,
        hard_input_bytes: int | None = None,
        auth: str | None = None,
        api_key: str | None = None,
        toolsets: list[
            PythonToolset
            | ElicitationToolset
            | MemoryToolset
            | HttpFetchToolset
            | E2bSandboxToolset
        ]
        | None = None,
        context_providers: list[PythonContextProvider | MemoryContextProvider]
        | None = None,
        middleware: list[PythonMiddleware] | None = None,
        observers: list[PythonObserver | MemoryObserver] | None = None,
        output_type: Any | None = None,
        child_runs: ChildRunPolicy | None = None,
        approval_grant: ApprovalGrantMode | None = None,
    ) -> Agent:
        """Build a Rust-backed agent that dispatches to a dedicated provider.

        ``Agent.gateway`` stays as a thin dispatcher onto the OpenAI,
        Anthropic, and Ollama crates. This factory does not read
        environment variables. HTTPS is required off loopback and whenever
        ``api_key`` is set. Construction fails without ``hard_input_bytes``.
        ``openai_chat`` is a configuration error.

        Args:
            endpoint: Provider endpoint URL.
            model: Provider model name.
            instruction: Optional stable instruction prefix.
            capabilities: Optional declarative capability catalog.
            active_capabilities: Application capability ids to activate.
            wire_protocol: ``openai_responses``, ``anthropic_messages``,
                ``ollama_chat``, or ``gemini_generate_content``.
            credential_name: Named credential reference, never a secret.
            hard_input_bytes: Required maximum canonical request bytes.
            auth: ``none``, ``bearer``, or ``api_key``. Defaults from
                ``api_key``.
            api_key: Explicit credential. HTTPS is required when set.
            toolsets: Optional trusted Python toolset callbacks.
            context_providers: Optional trusted context-provider callbacks.
            middleware: Optional trusted middleware callbacks.
            observers: Optional trusted observer callbacks.
            output_type: Optional Pydantic output type. Lazily requires the
                Pydantic extra.
            child_runs: Optional child-run admission policy. Defaults to
                :meth:`ChildRunPolicy.deny`.
            approval_grant: Optional paid-tool approval grant mode. Defaults
                to :meth:`ApprovalGrantMode.per_call`.

        Returns:
            An immutable Rust-owned agent handle.

        Raises:
            ConfigurationError: The route, protocol, ``hard_input_bytes``,
                credential pairing, capability set, or port registration is
                invalid.
        """
    @staticmethod
    async def from_python(
        model: PythonModel,
        toolsets: list[
            PythonToolset
            | ElicitationToolset
            | MemoryToolset
            | HttpFetchToolset
            | E2bSandboxToolset
        ]
        | None = None,
        instruction: str | None = None,
        output_type: Any | None = None,
        capabilities: list[Capability] | None = None,
        active_capabilities: list[str] | None = None,
        context_providers: list[PythonContextProvider | MemoryContextProvider]
        | None = None,
        middleware: list[PythonMiddleware] | None = None,
        observers: list[PythonObserver | MemoryObserver] | None = None,
        *,
        child_runs: ChildRunPolicy | None = None,
        approval_grant: ApprovalGrantMode | None = None,
        sqlite_path: str | None = None,
        sqlite_durability: SqliteDurability | None = None,
    ) -> Agent:
        """Build an agent from trusted callbacks and optional Pydantic output type.

        Args:
            model: Trusted Python model callback.
            toolsets: Optional trusted toolset callbacks.
            instruction: Optional model instruction.
            output_type: Optional Pydantic output type.
            capabilities: Optional declarative capabilities.
            active_capabilities: Application-activated capability IDs.
            context_providers: Optional trusted context-provider callbacks.
            middleware: Optional trusted middleware callbacks.
            observers: Optional trusted observer callbacks.
            child_runs: Optional child-run admission policy. Defaults to
                :meth:`ChildRunPolicy.deny`.
            approval_grant: Optional paid-tool approval grant mode. Defaults
                to :meth:`ApprovalGrantMode.per_call`.
            sqlite_path: Optional SQLite file path. ``None`` keeps the
                in-memory journal. ``:memory:`` requires
                :attr:`SqliteDurability.Relaxed`.
            sqlite_durability: Durability policy when ``sqlite_path`` is
                set. Defaults to :attr:`SqliteDurability.Durable`.

        Returns:
            An immutable Rust-owned agent handle.

        Raises:
            ConfigurationError: The callbacks, capability set, or SQLite
                store are invalid.
        """
    def capability_catalog(self) -> list[CapabilityCatalogItem]:
        """Return the bounded model-activated catalog in identity order.

        Returns:
            Compact ``id`` / ``description`` entries. Empty when no ``model``
            capabilities were registered.
        """
    def compact_capability_catalog(self) -> str:
        """Render the compact model-facing catalog without activation.

        Returns:
            ``id: description`` lines under the 8 KiB registration ceiling.
        """
    def read_artifact(self, artifact: dict[str, Any]) -> bytes:
        """Read back the bytes behind an artifact reference a tool returned.

        Toolsets that produce binary output stage it and return a reference
        instead of inlining base64 the model cannot read, so a generated
        image or audio clip arrives as the ``artifact`` field of a tool
        result rather than as data. Pass that value here to get the bytes.

        Args:
            artifact: The ``artifact`` object from a tool result.

        Returns:
            The staged bytes.

        Raises:
            ValueError: If ``artifact`` is not a valid reference, or the
                bytes are no longer present in this agent's store.
        """
    async def re_resolve(self) -> Agent:
        """Compose a new agent from reconstructed catalogs.

        MCP ``list_changed`` does not mutate this lock. In-flight runs keep
        the previous composition. The returned agent has a new lock.

        Returns:
            A new Rust-owned agent handle.

        Raises:
            ConfigurationError: Reconstruction failed or the agent was not
                builder-composed.
        """
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
        timeout_seconds: float | None = None,
        max_cycles: int = 16,
        max_output_retries: int = 1,
        capability: str | None = None,
        attachments: list[Attachment] | None = None,
    ) -> Run:
        """Start a run and return its shared handle immediately.

        Dropping the returned handle detaches observation; it does not cancel
        the durable run. Call :meth:`Run.cancel` for explicit cancellation.

        Args:
            input: Non-empty plain-text user input.
            timeout_seconds: Operational deadline in seconds. ``None`` uses
                the agent default (30, or 120 for
                :meth:`Agent.openai`). Must be positive when set.
            max_cycles: Maximum model cycles.
            max_output_retries: Maximum structured-output retries.
            capability: Optional model-activated capability id. ``None``
                runs this agent; a missing catalog id fails closed. User-input
                word overlap is not used.
            attachments: Optional pre-built attachments, staged and mapped
                to `File` blocks on the run's user message. At most 8.

        Returns:
            A shared :class:`Run` handle.

        Raises:
            ConfigurationError: Input, limits, ``capability``, or
                ``attachments`` are invalid.
            RuntimeError: The background task cannot be accepted.

        Examples:
            >>> import asyncio
            >>> async def demo(agent: Agent) -> str:
            ...     run = agent.start("hello")
            ...     result = await run.result()
            ...     return result.text
        """
    async def run(
        self,
        input: str,
        *,
        timeout_seconds: float | None = None,
        max_cycles: int = 16,
        max_output_retries: int = 1,
        capability: str | None = None,
        attachments: list[Attachment] | None = None,
    ) -> RunResult:
        """Execute one run and await its committed result.

        Equivalent to :meth:`start` followed by :meth:`Run.result`.

        Args:
            input: Non-empty plain-text user input.
            timeout_seconds: Operational deadline in seconds. ``None`` uses
                the agent default (30, or 120 for
                :meth:`Agent.openai`). Must be positive when set.
            max_cycles: Maximum model cycles.
            max_output_retries: Maximum structured-output retries.
            capability: Optional model-activated capability id. ``None``
                runs this agent; a missing catalog id fails closed.
            attachments: Optional pre-built attachments, staged and mapped
                to `File` blocks on the run's user message. At most 8.
                Every element resolves to model-visible extracted Markdown
                via the registered document-ingest middleware/toolset; the
                journaled conversation keeps the original `File` block.

        Returns:
            An immutable terminal snapshot.

        Raises:
            ConfigurationError: Input, limits, ``capability``, or
                ``attachments`` are invalid.
            RuntimeError: The runtime failed after accept.
            CancelledError: The run reached its durable cancelled terminal.
            TimeoutError: The operational deadline elapsed.

        Examples:
            >>> import asyncio
            >>> async def demo(agent: Agent) -> str:
            ...     result = await agent.run("hello")
            ...     return result.text
        """

def health() -> str:
    """Return ``\"ok\"`` without initializing runtime or network resources.

    Returns:
        The literal ``ok``.
    """

def build_metadata() -> dict[str, str | bool | int]:
    """Return native build and compatibility metadata.

    Returns:
        Version, engine, and feature flags. No secrets.
    """

def linked_providers() -> tuple[str, str, str, str, str]:
    """Return curated Rust-backed providers linked into this extension.

    Returns:
        A tuple such as
        ``(\"openai\", \"anthropic\", \"gemini\", \"ollama\", \"openrouter\")``.
    """

def journal_known_answer(kind: str, value: dict[str, object]) -> dict[str, object]:
    """Return payload digest, checksum, and canonical-CBOR hex from Rust.

    Args:
        kind: Known-answer fixture kind owned by the protocol crate.
        value: Binding-neutral mapping for that kind.

    Returns:
        Digest, checksum, and hex fields computed by Rust.

    Raises:
        ConfigurationError: ``kind`` or ``value`` is not a known fixture.
    """

def normalize_prebeta_shape(kind: str, value: dict[str, object]) -> dict[str, object]:
    """Validate one Rust-owned pre-beta lineage or external-command shape.

    This does not route a live agent command.

    Args:
        kind: ``child_lineage``, ``interaction_resolution``, or
            ``external_completion``.
        value: Binding-neutral mapping for that kind.

    Returns:
        The normalized mapping when valid.

    Raises:
        ConfigurationError: The shape is rejected.
        TypeError: ``value`` is not a mapping.
    """

def _normalize_pydantic_schema(schema: dict[str, Any], kind: str) -> dict[str, Any]:
    """Normalize one generated schema into the portable Pydantic subset."""

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

    Runs the same `finstack-ai-tools-document` parser the ingest
    middleware uses, without an `Agent` or `Run`, so a developer can see
    exactly what would be injected for a given file.

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
