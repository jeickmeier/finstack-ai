---
title: "finstack-ai Technical Design Document"
subtitle: "Implementation-level design for the Rust agent microkernel, runtime, bindings, and extension SDK"
author: "Project Draft"
date: "2026-08-07"
---

# Document control

| Field | Value |
|---|---|
| Product | `finstack-ai` |
| Document | Technical Design Document (TDD) |
| Version | 0.1 |
| Status | Draft for implementation |
| Primary language | Rust |
| Bindings | Python/PyO3; JavaScript/WebAssembly; optional WIT Component Model |
| Related documents | Product Requirements Document; Architecture Specification |

# 1. Technical objective

Implement a small deterministic agent engine in Rust that owns the canonical semantics of messages, runs, turns, tool calls, effects, limits, cancellation, journaling, and recovery. Surround that engine with an asynchronous runtime and a concise extension SDK. Expose the same engine through native Rust, Python, and WebAssembly without moving the state machine into the host language.

The implementation must optimize the ordinary fast path:

```text
resolved native model + resolved native toolsets + native store
    -> direct Rust calls
    -> no dynamic discovery during execution
    -> no serialization inside the process
    -> bounded event batching at external boundaries
```

The design also supports slower but more flexible paths:

```text
Python callback adapter
JavaScript promise adapter
WIT/WASM component adapter
external process adapter (future)
```

These paths implement the same logical ports and return normalized values to the same runtime.

# 2. Proposed workspace

```text
finstack-ai/
  Cargo.toml
  rust-toolchain.toml
  deny.toml

  crates/
    finstack-ai-kernel/
      src/
        lib.rs
        ids.rs
        raw_json.rs
        content.rs
        message.rs
        entries.rs
        agent.rs
        capabilities.rs
        limits.rs
        effects.rs
        records.rs
        events.rs
        state/
        reducer/
        recovery/
        validation.rs

    finstack-ai-runtime/
      src/
        lib.rs
        agent.rs
        run_task.rs
        commit.rs
        event_hub.rs
        model_driver.rs
        tool_scheduler.rs
        context_pipeline.rs
        middleware_pipeline.rs
        cancellation.rs
        timers.rs
        shutdown.rs
        adapters/

    finstack-ai-sdk/
      src/
        lib.rs
        builder.rs
        registrar.rs
        registry.rs
        extension.rs
        model.rs
        toolset.rs
        context.rs
        middleware.rs
        store.rs
        observer.rs
        capability.rs
        spec.rs
        result.rs
        error.rs

    finstack-ai-protocol/
      src/
        lib.rs
        envelope.rs
        cbor.rs
        frame.rs
        journal.rs
        remote.rs
        version.rs

    finstack-ai-test/
      src/
        scripted_model.rs
        fake_toolset.rs
        memory_store.rs
        trace_fixture.rs
        manual_clock.rs
        fault_store.rs

  bindings/
    finstack-ai-python/
      Cargo.toml
      pyproject.toml
      src/
      python/finstack_ai/

    finstack-ai-wasm/
      Cargo.toml
      src/
      js/

  plugins/
    finstack-ai-wit/
      wit/v1/
        types.wit
        toolset.wit
        context.wit

    finstack-ai-plugin-host/
      src/

  providers/
    finstack-ai-provider-openai-compatible/
    finstack-ai-provider-anthropic/
    finstack-ai-provider-test/

  toolsets/
    finstack-ai-tools-filesystem/
    finstack-ai-tools-shell/

  stores/
    finstack-ai-store-memory/
    finstack-ai-store-sqlite/

  observers/
    finstack-ai-observer-log/
    finstack-ai-observer-otel/

  examples/
    rust-minimal/
    python-minimal/
    browser-minimal/
    durable-approval/
```

# 3. Crate dependency policy

## 3.1 Dependency graph

```text
finstack-ai-kernel
        ^
        |
finstack-ai-protocol
        ^
        |
finstack-ai-sdk <------ provider/tool/store/observer crates
        ^
        |
finstack-ai-runtime
        ^
        |
Rust facade / Python binding / WASM binding / applications
```

The exact direction between `protocol` and `kernel` may be adjusted to avoid cycles. The preferred arrangement is:

- kernel owns semantic record types;
- protocol owns encodings and transport envelopes for those types; and
- both depend on a tiny shared `finstack-ai-types` crate only if splitting is required.

Do not introduce a miscellaneous utility crate unless at least three independent crates need the same stable abstraction.

## 3.2 Kernel dependency budget

Initial expected direct dependencies:

```text
serde               # derives and versioned data
serde_json           # RawJson validation and diagnostic form
bytes                # shared byte buffers
thiserror            # stable internal errors
uuid                 # typed UUIDv7-compatible identifiers
smallvec             # small fixed-size collections where measured useful
bitflags             # capabilities/flags where appropriate
```

`futures-core` may be avoided because the kernel is synchronous. Every additional direct dependency requires review.

Forbidden in the kernel:

```text
tokio, async-std, smol
reqwest, hyper, rustls
sqlx, rusqlite
pyo3, wasm-bindgen
wasmtime
clap, ratatui
opentelemetry exporters
provider SDKs
```

# 4. Naming and package conventions

- Cargo package names use hyphens, e.g. `finstack-ai-kernel`.
- Rust crate imports use underscores, e.g. `finstack_ai_kernel`.
- Python import is `finstack_ai`.
- npm package is `@finstack/ai`.
- WIT package namespace is `finstack:ai-*`.
- Reserved internal tool IDs use `finstack.internal.*`.
- First-party public component IDs use `finstack.*`.
- Third-party IDs should use reverse-domain or organization namespaces.

# 5. Identifier design

## 5.1 Typed IDs

Use a generic newtype internally and public concrete aliases:

```rust
#[repr(transparent)]
pub struct Id<T> {
    value: uuid::Uuid,
    _marker: core::marker::PhantomData<fn() -> T>,
}

pub type SessionId = Id<SessionTag>;
pub type LaneId = Id<LaneTag>;
pub type RunId = Id<RunTag>;
pub type TurnId = Id<TurnTag>;
pub type EffectId = Id<EffectTag>;
pub type ToolCallId = Id<ToolCallTag>;
pub type RecordId = Id<RecordTag>;
```

Externally, IDs serialize as canonical lowercase UUID strings. UUIDv7 is preferred for new IDs because ordering is useful in logs and storage. Tests use an injected deterministic generator.

## 5.2 Provider identifiers

A provider may supply its own request, message, or tool-call ID. These are stored separately as opaque strings and never replace internal stable IDs.

```rust
pub struct ProviderIds {
    pub request_id: Option<String>,
    pub response_id: Option<String>,
    pub continuation_id: Option<String>,
}
```

## 5.3 Transition environment

Clock and ID generation are nondeterministic effects. The runtime supplies them as normalized transition input:

```rust
pub struct TransitionEnv {
    pub now: Timestamp,
    pub ids: AllocatedIds,
}
```

Test fixtures provide exact timestamps and IDs, making decisions reproducible.

# 6. Raw JSON and byte ownership

## 6.1 `RawJson`

Dynamic tool arguments, structured outputs, provider extensions, and schemas use validated UTF-8 JSON bytes.

```rust
#[derive(Clone)]
pub struct RawJson(bytes::Bytes);

impl RawJson {
    pub fn parse(input: impl Into<Bytes>) -> Result<Self, RawJsonError>;
    pub fn as_bytes(&self) -> &[u8];
    pub fn as_str(&self) -> &str;
}
```

Validation occurs on construction. Parsed `serde_json::Value` is created only when required. This avoids repeated structure/string conversions across providers and bindings.

## 6.2 Shared buffers

Messages, schemas, and results use `Arc<str>`, `Bytes`, or `Arc<[T]>` where sharing is common. Public APIs avoid exposing borrowed lifetimes that cannot map to Python or WASM.

# 7. Content and message model

## 7.1 Content blocks

```rust
pub enum ContentBlock {
    Text(TextBlock),
    Json(JsonBlock),
    Image(MediaRef),
    Audio(MediaRef),
    File(MediaRef),
    ToolCall(ToolCallBlock),
    ToolResult(ToolResultBlock),
    Reasoning(ReasoningBlock),
    Refusal(RefusalBlock),
    Opaque(OpaqueBlock),
}
```

`OpaqueBlock` carries a namespaced media type and bytes/JSON for provider-specific round-tripping. Components that do not understand it must preserve it when the selected provider requires it.

## 7.2 Blob reference

```rust
pub struct BlobRef {
    pub id: Arc<str>,
    pub media_type: Arc<str>,
    pub length: u64,
    pub digest: Option<Arc<str>>,
    pub name: Option<Arc<str>>,
}
```

The core never dereferences a blob. Toolsets, context providers, bindings, or applications do so through their own services.

## 7.3 Message roles

```rust
pub enum MessageRole {
    System,
    Developer,
    User,
    Assistant,
    Tool,
}
```

## 7.4 Model message

```rust
pub struct Message {
    pub id: MessageId,
    pub role: MessageRole,
    pub content: Arc<[ContentBlock]>,
    pub created_at: Timestamp,
    pub model: Option<ModelRef>,
    pub provider_ids: ProviderIds,
    pub metadata: Metadata,
}
```

The kernel validates role/block combinations. A tool-result message must refer to known tool calls and one result must exist for every completed or reconciled call.

# 8. Agent composition types

## 8.1 Serializable `AgentSpec`

```rust
pub struct AgentSpec {
    pub schema_version: u16,
    pub id: AgentId,
    pub model: ComponentRef,
    pub instructions: Vec<InstructionSpec>,
    pub toolsets: Vec<ComponentRef>,
    pub context_providers: Vec<ComponentRef>,
    pub middleware: Vec<MiddlewareRef>,
    pub store: Option<ComponentRef>,
    pub observers: Vec<ComponentRef>,
    pub capabilities: Vec<CapabilitySpec>,
    pub limits: RunLimits,
    pub policy: RunPolicy,
    pub extension_config: BTreeMap<ComponentId, RawJson>,
}
```

The spec is immutable after validation. Callables and runtime handles cannot appear in the serializable representation.

## 8.2 Resolved agent

```rust
pub struct ResolvedAgent {
    pub spec: Arc<AgentSpec>,
    pub model: Arc<dyn Model>,
    pub tools: Arc<ToolRegistry>,
    pub context: Arc<[ResolvedContextProvider]>,
    pub middleware: Arc<[ResolvedMiddleware]>,
    pub store: Arc<dyn JournalStore>,
    pub observers: Arc<[ResolvedObserver]>,
    pub capabilities: Arc<CapabilityRegistry>,
    pub runtime_policy: RuntimePolicy,
}
```

The resolved agent contains no string lookups required for ordinary execution except tool dispatch by model-emitted name. Tool names map to pre-resolved entries in a compact registry.

# 9. Extension registrar

## 9.1 Extension trait

```rust
pub trait Extension: Send + Sync + 'static {
    fn descriptor(&self) -> ExtensionDescriptor;
    fn register(&self, registrar: &mut Registrar) -> Result<(), RegistrationError>;
}
```

## 9.2 Registrar operations

```rust
impl Registrar {
    pub fn model(&mut self, id: ComponentId, model: Arc<dyn Model>) -> Result<()>;
    pub fn toolset(&mut self, id: ComponentId, toolset: Arc<dyn Toolset>) -> Result<()>;
    pub fn context_provider(&mut self, id: ComponentId, provider: Arc<dyn ContextProvider>) -> Result<()>;
    pub fn middleware(&mut self, id: ComponentId, middleware: Arc<dyn Middleware>) -> Result<()>;
    pub fn store(&mut self, id: ComponentId, store: Arc<dyn JournalStore>) -> Result<()>;
    pub fn observer(&mut self, id: ComponentId, observer: Arc<dyn Observer>) -> Result<()>;
}
```

Factories may be added for configuration-dependent initialization. The resolved registry stores either a ready handle or an async factory with declared lifecycle.

## 9.3 Duplicate behavior

Duplicate IDs are errors. Replacement requires explicit `replace(id, expected_owner, new_value)` and is logged in resolution diagnostics.

# 10. Capabilities

## 10.1 Data structure

```rust
pub struct CapabilitySpec {
    pub id: CapabilityId,
    pub description: Arc<str>,
    pub instructions: Arc<[InstructionSpec]>,
    pub toolsets: Arc<[ComponentRef]>,
    pub context_providers: Arc<[ComponentRef]>,
    pub middleware: Arc<[ComponentRef]>,
    pub activation: CapabilityActivation,
}

pub enum CapabilityActivation {
    Always,
    Application,
    Model,
    Disabled,
}
```

## 10.2 Resolution

Capability expansion occurs during agent construction. Always-active components enter the base resolved agent. Deferred components enter a separate catalog with validated references and precomputed tool metadata.

## 10.3 Model activation

The reserved internal tool `finstack.internal.load_capability` accepts one or more capability IDs. The kernel validates requests, records activation, and applies the new instructions/tools/context/middleware at the next safe checkpoint.

The internal tool is implemented by the runtime/kernel adapter and is not dispatched to an external toolset.

# 11. Kernel state machine API

## 11.1 Command-level inputs

The kernel receives normalized commands only at semantic boundaries:

```rust
pub enum KernelInput {
    AcceptRun(AcceptRun),
    StageSettled(StageSettled),
    ModelSettled(ModelSettled),
    ToolBatchSettled(ToolBatchSettled),
    ApprovalSettled(ApprovalSettled),
    TimerFired(TimerFired),
    CancelRequested(CancelRequested),
    EffectReconciled(EffectReconciled),
    ResumeRequested(ResumeRequested),
}
```

Fine-grained model and tool progress events are handled by the runtime event sequencer and are not kernel inputs.

## 11.2 Decision API

```rust
pub struct Kernel {
    state: KernelState,
}

impl Kernel {
    pub fn decide(
        &self,
        env: &TransitionEnv,
        input: KernelInput,
    ) -> Result<Decision, KernelError>;

    pub fn apply(
        &mut self,
        committed: &CommittedBatch,
    ) -> Result<Arc<[KernelEvent]>, KernelError>;
}
```

`decide` does not mutate authoritative state.

```rust
pub struct Decision {
    pub expected_sequence: u64,
    pub records: Vec<RecordDraft>,
    pub actions: Vec<PostCommitAction>,
    pub diagnostics: Vec<Diagnostic>,
}
```

`PostCommitAction` may execute only after the associated records are committed and applied.

## 11.3 Why no generic graph engine

The reducer uses explicit phase-specific functions rather than a generic node graph:

```text
accept_run
settle_before_run
settle_context
settle_before_model
settle_model
settle_after_model
settle_before_tools
settle_tool_batch
settle_after_tools
settle_after_run
```

The standard run path remains visible in code and amenable to exhaustive state tests.

# 12. Journal record design

## 12.1 Envelope

```rust
pub struct RecordEnvelope {
    pub format_version: u16,
    pub record_id: RecordId,
    pub session_id: SessionId,
    pub lane_id: LaneId,
    pub run_id: Option<RunId>,
    pub sequence: u64,
    pub timestamp: Timestamp,
    pub body: RecordBody,
}
```

The store assigns `sequence` and may assign the final timestamp if the runtime uses store-authoritative time. The chosen rule must remain consistent per store.

## 12.2 Record body

Initial variants:

```rust
pub enum RecordBody {
    SessionCreated(SessionCreated),
    LaneCreated(LaneCreated),
    LaneMoved(LaneMoved),
    RunAccepted(RunAccepted),
    StageOutcomeRecorded(StageOutcomeRecorded),
    ContextPrepared(ContextPrepared),
    EffectRequested(EffectRequested),
    EffectCompleted(EffectCompleted),
    EffectFailed(EffectFailed),
    EffectCancelled(EffectCancelled),
    EntryAppended(EntryAppended),
    ToolBatchOpened(ToolBatchOpened),
    ToolBatchClosed(ToolBatchClosed),
    ApprovalRequested(ApprovalRequested),
    ApprovalResolved(ApprovalResolved),
    CapabilityActivated(CapabilityActivated),
    CancellationRequested(CancellationRequested),
    RunSuspended(RunSuspended),
    RunCompleted(RunCompleted),
    RunFailed(RunFailed),
    RunCancelled(RunCancelled),
    SnapshotWritten(SnapshotWritten),
}
```

## 12.3 Effect records

```rust
pub struct EffectRequested {
    pub effect_id: EffectId,
    pub kind: EffectKind,
    pub input: EffectInput,
    pub input_digest: Digest,
    pub retry_safety: RetrySafety,
    pub deadline: Option<Timestamp>,
}

pub enum EffectKind {
    Model,
    Tool,
    Context,
    Middleware,
    Approval,
    Timer,
}
```

Completion records include the normalized output, usage, provider/tool IDs, and retry metadata. Large output is externalized through `BlobRef` before commit.

## 12.4 Atomic batches

A store append accepts a batch and expected previous sequence:

```rust
pub struct AppendRequest {
    pub session_id: SessionId,
    pub expected_sequence: u64,
    pub records: Vec<RecordDraft>,
}

pub struct CommittedBatch {
    pub first_sequence: u64,
    pub last_sequence: u64,
    pub records: Arc<[RecordEnvelope]>,
}
```

The append is atomic. A concurrency mismatch returns `StoreConflict` and no records are appended.

# 13. Runtime commit loop

## 13.1 Main algorithm

```text
receive KernelInput
    |
    v
decision = kernel.decide(state, input)
    |
    +-- no records --> emit diagnostics / schedule actions allowed without commit
    |
    v
store.append(expected_sequence, records)
    |
    +-- conflict --> reload/reconcile lane ownership
    +-- failure  --> fault lane/session; execute no post-commit action
    |
    v
kernel.apply(committed_batch)
    |
    v
publish durable-derived events
    |
    v
execute post-commit actions
    |
    v
normalize action result into next KernelInput
```

## 13.2 State version

The runtime tracks the last applied sequence per session and lane. Kernel decisions include the expected sequence to prevent actions based on stale state.

## 13.3 Non-durable mode

The in-memory store implements the same append API. There is no alternate code path that skips commit sequencing; this keeps behavior consistent.

# 14. Model port design

## 14.1 Trait

```rust
pub type ModelEventStream = Pin<
    Box<dyn futures_core::Stream<Item = Result<ModelStreamItem, ModelError>> + Send>
>;

pub trait Model: Send + Sync + 'static {
    fn descriptor(&self) -> ModelDescriptor;
    fn capabilities(&self, model: &ModelName) -> ModelCapabilities;

    fn request(
        &self,
        ctx: ModelCallContext,
        request: ModelRequest,
    ) -> BoxFuture<'static, Result<ModelEventStream, ModelError>>;

    fn reconcile(
        &self,
        _ctx: ReconcileContext,
        _effect: PendingModelEffect,
    ) -> BoxFuture<'static, Result<ReconcileResult, ModelError>> {
        Box::pin(async { Ok(ReconcileResult::Unknown) })
    }
}
```

A no-GAT boxed future/stream API is selected initially because it maps cleanly to trait objects, Python adapters, and plugin proxies. Performance-critical first-party providers may use internal concrete types behind the trait.

## 14.2 Request

```rust
pub struct ModelRequest {
    pub effect_id: EffectId,
    pub model: ModelName,
    pub messages: Arc<[Message]>,
    pub tools: Arc<[ToolSpec]>,
    pub output: OutputSpec,
    pub settings: ModelSettings,
    pub limits: ModelRequestLimits,
    pub provider_state: Option<OpaqueState>,
}
```

## 14.3 Stream items

```rust
pub enum ModelStreamItem {
    TextDelta(TextDelta),
    ReasoningDelta(ReasoningDelta),
    ToolCallDelta(ToolCallDelta),
    Usage(UsageDelta),
    ProviderEvent(OpaqueProviderEvent),
    Completed(ModelResponse),
}
```

Exactly one `Completed` item is required for a successful stream. The runtime validates stream order and assembles tool calls where the provider emits partial arguments.

## 14.4 Capability model

```rust
pub struct ModelCapabilities {
    pub input: InputCapabilities,
    pub native_tool_calls: bool,
    pub parallel_tool_calls: bool,
    pub structured_output: StructuredOutputCapability,
    pub reasoning: bool,
    pub prompt_cache: bool,
    pub resumable_stream: bool,
    pub idempotent_requests: bool,
    pub native_capabilities: BTreeSet<CapabilityKey>,
}
```

Provider-specific capability detail lives in namespaced metadata rather than kernel enums whenever possible.

# 15. Toolset port design

## 15.1 Trait

```rust
pub type ToolEventStream = Pin<
    Box<dyn Stream<Item = Result<ToolStreamItem, ToolError>> + Send>
>;

pub trait Toolset: Send + Sync + 'static {
    fn descriptor(&self) -> ToolsetDescriptor;
    fn tools(&self) -> Arc<[ToolSpec]>;

    fn call(
        &self,
        ctx: ToolCallContext,
        call: ValidatedToolCall,
    ) -> BoxFuture<'static, Result<ToolEventStream, ToolError>>;

    fn reconcile(
        &self,
        _ctx: ReconcileContext,
        _effect: PendingToolEffect,
    ) -> BoxFuture<'static, Result<ReconcileResult, ToolError>> {
        Box::pin(async { Ok(ReconcileResult::Unknown) })
    }
}
```

## 15.2 Tool metadata

```rust
pub struct ToolSpec {
    pub id: ToolId,
    pub model_name: Arc<str>,
    pub title: Arc<str>,
    pub description: Arc<str>,
    pub input_schema: RawJson,
    pub output_schema: Option<RawJson>,
    pub execution: ToolExecutionMode,
    pub side_effect: SideEffectClass,
    pub retry_safety: RetrySafety,
    pub approval: ApprovalMetadata,
    pub max_result_bytes: u64,
    pub metadata: Metadata,
}
```

`model_name` is the name exposed to the LLM and must be unique in the resolved agent. `ToolId` remains stable across aliases.

## 15.3 Validation

A `ResolvedTool` contains:

- public `ToolSpec`;
- toolset dispatch handle;
- precompiled validator or binding adapter;
- optional output validator; and
- middleware routing metadata.

Validation occurs before `Toolset::call`. Python tools may use a Pydantic adapter; native tools may use Serde or a JSON Schema validator. The kernel receives only normalized validation success/failure.

## 15.4 Tool stream

```rust
pub enum ToolStreamItem {
    Progress(ToolProgress),
    Usage(UsageDelta),
    Completed(ToolResult),
}
```

Progress is transient. Exactly one completion is required on success.

# 16. ContextProvider design

```rust
pub trait ContextProvider: Send + Sync + 'static {
    fn descriptor(&self) -> ContextProviderDescriptor;

    fn collect(
        &self,
        ctx: ContextCallContext,
        request: ContextRequest,
    ) -> BoxFuture<'static, Result<ContextContribution, ContextError>>;
}
```

```rust
pub struct ContextRequest {
    pub session_id: SessionId,
    pub lane_id: LaneId,
    pub run_id: RunId,
    pub user_input: Arc<[ContentBlock]>,
    pub recent_history: Arc<[Message]>,
    pub budget: ContextBudget,
    pub active_capabilities: Arc<[CapabilityId]>,
}

pub struct ContextContribution {
    pub items: Arc<[ContextItem]>,
    pub estimated_tokens: u32,
    pub bytes: u64,
    pub cache_key: Option<Arc<str>>,
}
```

Context item kinds include instruction, quoted source, reference, metadata, and hidden application context. Provenance is mandatory for externally retrieved content.

The runtime can fan out providers concurrently, then order and budget contributions deterministically by resolved provider order and item priority.

# 17. Middleware design

## 17.1 Single invocation interface

```rust
pub trait Middleware: Send + Sync + 'static {
    fn descriptor(&self) -> MiddlewareDescriptor;
    fn stages(&self) -> StageMask;

    fn invoke(
        &self,
        ctx: MiddlewareContext,
        input: StageInput,
    ) -> BoxFuture<'static, Result<StageOutcome, MiddlewareError>>;
}
```

A single interface is easier to proxy through Python and future process boundaries than a trait with many optional methods. The Rust SDK may provide an adapter trait with stage-specific methods for ergonomics.

## 17.2 Stages

```rust
pub enum Stage {
    BeforeRun,
    PrepareContext,
    BeforeModel,
    AfterModel,
    BeforeToolBatch,
    AfterToolBatch,
    AfterRun,
}
```

## 17.3 Outcomes

```rust
pub enum StageOutcome {
    Continue,
    Replace(StageValue),
    AddInstructions(Arc<[InstructionSpec]>),
    AddContext(Arc<[ContextItem]>),
    FilterTools(Arc<[ToolId]>),
    RequestApproval(ApprovalSpec),
    Retry(RetryDirective),
    Suspend(SuspensionSpec),
    Complete(RunOutput),
    Fail(FrameworkError),
}
```

Only outcomes valid for the current stage are accepted. Invalid combinations produce a middleware contract error.

## 17.4 Ordering resolution

```rust
pub struct MiddlewareOrder {
    pub tier: OrderTier,
    pub priority: i32,
    pub before: Arc<[ComponentId]>,
    pub after: Arc<[ComponentId]>,
}
```

The resolver topologically sorts once. A stable sorted list is stored in `ResolvedAgent`.

## 17.5 Durable middleware output

When an outcome affects durable behavior, the runtime writes a `StageOutcomeRecorded` or `EffectCompleted` record before applying it. Middleware descriptor metadata declares whether replay may recompute an outcome; default is recorded output reuse.

# 18. JournalStore design

```rust
pub trait JournalStore: Send + Sync + 'static {
    fn append(
        &self,
        request: AppendRequest,
    ) -> BoxFuture<'static, Result<CommittedBatch, StoreError>>;

    fn load(
        &self,
        request: LoadRequest,
    ) -> BoxFuture<'static, Result<LoadedSession, StoreError>>;

    fn write_snapshot(
        &self,
        request: SnapshotRequest,
    ) -> BoxFuture<'static, Result<SnapshotReceipt, StoreError>>;

    fn health(&self) -> BoxFuture<'static, Result<Health, StoreError>>;
}
```

## 18.1 Store contract

- `append` is atomic per session.
- `expected_sequence` enforces optimistic concurrency.
- duplicate `record_id` appends are idempotent only if payloads match exactly;
- mismatched duplicates are corruption/conflict;
- stores return committed envelopes with assigned sequence;
- loads validate checksums and format versions; and
- store-specific retries occur below the semantic runtime only when they cannot duplicate an append.

## 18.2 SQLite design direction

The first durable store uses tables similar to:

```text
sessions(session_id, current_sequence, snapshot_sequence, metadata)
records(session_id, sequence, record_id, lane_id, run_id, kind, version, payload_cbor, timestamp)
snapshots(session_id, sequence, payload_cbor, digest, timestamp)
```

Append uses one transaction and a compare/update on `current_sequence`.

# 19. Observer design

```rust
pub trait Observer: Send + Sync + 'static {
    fn descriptor(&self) -> ObserverDescriptor;

    fn observe(
        &self,
        batch: Arc<[RunEvent]>,
    ) -> BoxFuture<'static, Result<(), ObserverError>>;
}
```

Observer configuration:

```rust
pub enum ObserverMode {
    BestEffort,
    Required,
}

pub enum ObserverBackpressure {
    Block { timeout: Duration },
    DropProgress,
    Spill,
    Disconnect,
}
```

Best-effort failures produce diagnostics but do not fail the run. Required audit observers can fail a transition before associated effects execute if configured by the application.

# 20. Runtime event model

## 20.1 Event envelope

```rust
pub struct RunEvent {
    pub kind: RunEventKind,
    pub session_id: SessionId,
    pub lane_id: LaneId,
    pub run_id: RunId,
    pub turn_id: Option<TurnId>,
    pub effect_id: Option<EffectId>,
    pub tool_call_id: Option<ToolCallId>,
    pub durable_sequence: Option<u64>,
    pub transient_sequence: u64,
    pub timestamp: Timestamp,
    pub sensitivity: Sensitivity,
    pub body: RunEventBody,
}
```

## 20.2 Durable versus transient

Durable-derived events include accepted, message finalized, tool settled, approval requested/resolved, limit reached, and terminal results.

Transient events include model text delta, reasoning delta, tool progress, queue depth warning, and provider heartbeat.

## 20.3 Event hub

The event hub has one bounded source stream per run and fan-out subscriptions. Each subscriber selects:

- event filter;
- batching thresholds;
- progress coalescing;
- sensitive-data mode; and
- disconnect behavior.

Completion events cannot be silently dropped. A subscriber that cannot keep up is disconnected or receives a compact snapshot/result according to mode.

# 21. Tool scheduling algorithm

## 21.1 Batch construction

The final model response yields ordered calls. Each call is resolved to a tool and annotated with execution mode and policy metadata.

## 21.2 Execution groups

Rules:

1. Consecutive parallel calls may execute together.
2. A sequential call executes alone and waits for prior work.
3. A barrier waits for all previous calls and blocks subsequent calls until complete.
4. Middleware or approval may split a batch further.
5. The runtime enforces global and per-tool concurrency limits.

## 21.3 Completion ordering

Progress/completion events may reflect actual finish order. Durable tool-result entries are appended in model source order after each required preceding result is available. This keeps provider histories stable.

## 21.4 Failure behavior

A tool failure becomes a normalized `ToolResult` when policy permits the model to recover. Framework contract failures, permission failures, and non-repeatable uncertainty may fail or suspend the run instead.

# 22. Cancellation and deadlines

## 22.1 Cancellation token tree

```text
Agent shutdown token
  Session token
    Lane/run token
      Model effect token
      Tool effect tokens
      Context/middleware tokens
```

Cancelling a parent signals descendants. Completion after cancellation is ignored or reconciled according to effect state.

## 22.2 Durable cancellation

A durable lane writes `CancellationRequested` before final reconciliation. The runtime signals active effects, waits up to policy deadline, records settled/cancelled outcomes, synthesizes required tool results, and writes `RunCancelled`.

## 22.3 Consumer cancellation

Dropping a Python/JS run object does not automatically cancel a durable run. The public API provides explicit `cancel()`. Non-durable convenience calls may opt into cancel-on-drop.

# 23. Recovery algorithm

## 23.1 Restore

1. Load latest compatible snapshot.
2. Validate snapshot digest/version.
3. Apply records after snapshot sequence.
4. Validate invariants and open effects.
5. Build `SuspendedOperation` descriptors for incomplete work.
6. Return session in idle, suspended, or faulted state.

## 23.2 Pending effect reconciliation

Each implementation receives pending effect metadata and may return:

```rust
pub enum ReconcileResult {
    Completed(EffectOutput),
    NotStarted,
    StillRunning(DeferredHandle),
    RetrySafe,
    Unknown,
    NonRepeatable,
}
```

Runtime policy maps the result to retry, wait, operator resolution, or failure.

## 23.3 Crash matrix

Tests simulate process loss after every durable append and before/after every effect result. The restored run must either:

- continue from the next safe boundary;
- remain explicitly suspended; or
- fail with a documented uncertainty/corruption error.

# 24. Session and lane implementation

## 24.1 Conversation tree

```rust
pub struct ConversationEntry {
    pub id: EntryId,
    pub parent_id: Option<EntryId>,
    pub lane_id: LaneId,
    pub sequence: u64,
    pub body: EntryBody,
}
```

Entries are immutable. A lane has a current `leaf_id`.

## 24.2 Lane ownership

The runtime uses a lane guard keyed by `(session_id, lane_id)`. Only one active run/structural operation owns the guard. Store sequence checks provide cross-process protection when a server routes incorrectly.

## 24.3 Initial implementation scope

The data model includes lanes from the start. MVP APIs may expose only `main` plus fork/create primitives behind an experimental feature. This avoids a future journal migration from linear histories.

# 25. Python binding design

## 25.1 Toolchain

- PyO3 for native classes and conversion.
- Maturin for builds and wheels.
- `pyo3-async-runtimes` or a small equivalent bridge for awaitables.
- CPython limited ABI considered after measuring compatibility and performance trade-offs.

## 25.2 Python classes

```text
Agent
AgentSpec
Capability
Session
Lane
Run
RunResult
Event / EventBatch
Model
Toolset
ContextProvider
Middleware
JournalStore
Observer
```

Rust-backed concrete classes subclass or satisfy lightweight Python protocols rather than requiring Python inheritance from C extension types.

## 25.3 Handle storage

```rust
#[pyclass]
struct PyAgent {
    inner: Arc<ResolvedAgent>,
}

#[pyclass]
struct PyRun {
    handle: RunHandle,
    events: Mutex<Option<EventSubscription>>,
}
```

## 25.4 Async run

`Agent.run()` returns an awaitable. `Agent.start()` returns `Run` for event iteration and explicit control.

```python
run = agent.start("task")
async for event in run.events():
    ...
result = await run.result()
```

Internally, `events()` receives `EventBatch` values and expands them only if the caller requested individual events.

## 25.5 GIL behavior

- Rust-only construction and validation release the GIL where safe.
- Waiting on provider/tool/store futures does not hold the GIL.
- Python callbacks acquire the GIL only for conversion and callable execution.
- Observer delivery to Python is batched.

## 25.6 Python callback adapters

A Python tool adapter stores:

- callable reference;
- cached input/output schema adapters;
- sync/async classification;
- concurrency policy;
- optional thread-executor policy; and
- source metadata.

The adapter converts raw JSON to Python once, invokes the callable, converts the result once, and returns normalized bytes.

## 25.7 Pydantic adapter

The adapter supports:

```text
BaseModel subclasses
TypeAdapter targets
dataclasses
TypedDict
annotated function signatures
```

Registration flow:

```text
Python type/signature
  -> TypeAdapter cached
  -> JSON Schema generated once
  -> canonical schema bytes registered in Rust
  -> validator handle retained in Python adapter
```

Result flow:

```text
Rust RawJson
  -> Python bytes/string view
  -> TypeAdapter.validate_json
  -> Python object
```

## 25.8 Exception hierarchy

```text
FinstackError
  ConfigurationError
  RegistrationError
  AgentBuildError
  RunError
    ModelError
    ToolError
    MiddlewareError
    ContextError
    StoreError
    CancelledError
    LimitError
    RecoveryError
  ProtocolError
  PluginError
```

Each exception exposes `code`, identifiers, retryable flag, and safe details.

# 26. WebAssembly/JavaScript binding design

## 26.1 Target

`finstack-ai-wasm` targets `wasm32-unknown-unknown` and uses `wasm-bindgen`.

## 26.2 JavaScript classes

```typescript
class Agent {
  static create(spec: AgentOptions): Promise<Agent>;
  start(input: Input): Run;
}

class Run {
  events(options?: EventOptions): AsyncIterable<EventBatch>;
  result(): Promise<RunResult>;
  cancel(reason?: string): Promise<void>;
}
```

## 26.3 Host adapter ABI

JavaScript registers adapters using objects with methods returning promises or async iterables. The WASM wrapper converts these into runtime port proxies.

A model adapter may return a `ReadableStream`, async iterable, or callback-driven stream. The wrapper normalizes it to `ModelStreamItem` batches.

## 26.4 Worker recommendation

The package includes a worker helper that hosts one or more agents in a Web Worker. Main-thread communication uses transferable `ArrayBuffer` batches where possible.

## 26.5 WASM data conversion

- IDs are strings.
- Dynamic JSON is transferred as strings or `Uint8Array`, selected by API.
- Large content uses blob handles.
- Events are arrays of compact objects per batch.
- Internal state never round-trips through JavaScript for each transition.

# 27. WIT plugin design

## 27.1 Package structure

```text
wit/v1/types.wit
wit/v1/toolset.wit
wit/v1/context.wit
```

## 27.2 Common types

```wit
package finstack:ai-types@1.0.0;

interface types {
    record call-context {
        effect-id: string,
        session-id: string,
        lane-id: string,
        run-id: string,
        deadline-unix-ms: option<u64>,
    }

    record blob-ref {
        id: string,
        media-type: string,
        length: u64,
        digest: option<string>,
    }

    record plugin-error {
        code: string,
        message: string,
        retryable: bool,
    }
}
```

## 27.3 Toolset world

```wit
package finstack:ai-toolset@1.0.0;

world toolset-plugin {
    import finstack:ai-host/logging@1.0.0;
    import finstack:ai-host/blobs@1.0.0;

    export toolset;
}

interface toolset {
    record tool-spec {
        id: string,
        model-name: string,
        description: string,
        input-schema-json: list<u8>,
        output-schema-json: option<list<u8>>,
        side-effect: string,
        retry-safety: string,
    }

    record tool-result {
        content-json: list<u8>,
        is-error: bool,
    }

    list-tools: func() -> result<list<tool-spec>, string>;
    call: func(context: call-context, tool-id: string, args-json: list<u8>)
        -> result<tool-result, plugin-error>;
}
```

Streaming progress can be introduced through a WIT resource after validating host/guest async support. The first ABI may keep isolated tool calls coarse and return one result.

## 27.4 Context world

The context plugin receives a budget and request JSON and returns bounded context-item JSON plus blob references. It cannot mutate session history.

## 27.5 Host limits

Per-plugin configuration includes:

```text
max linear memory
max tables/instances
fuel or epoch deadline
call timeout
max output bytes
HTTP host allowlist
filesystem preopens
secret scopes
blob scopes
```

## 27.6 Component cache

The host caches compiled components by digest and runtime version. Stateful plugin instances are pooled only if the world contract declares safe reuse.

# 28. Protocol and encoding

## 28.1 Canonical journal encoding

Provisional decision: use a strict, versioned CBOR subset for durable and remote envelopes, implemented with a maintained Serde-compatible library. Reasons:

- compact binary representation;
- natural byte-string support;
- broad language support;
- deterministic framing; and
- easier evolution than a Rust-specific encoding.

A formal ADR must freeze the supported subset before stable release.

## 28.2 Frame

Remote streams use:

```text
4-byte unsigned big-endian payload length
CBOR envelope payload
```

Maximum frame size is configured and checked before allocation.

## 28.3 Diagnostic JSON

Every envelope and record has a lossless diagnostic JSON projection. JSONL export supports debugging, migrations, and support cases.

## 28.4 Schema evolution

- envelopes have protocol/format version;
- record bodies have kind version;
- additive optional fields are allowed within a major format;
- renamed/removed fields require migration or new kind version;
- unknown state-changing kinds are fatal to replay; and
- unknown diagnostic metadata may be retained/ignored.

# 29. Configuration and spec resolution

## 29.1 JSON spec

`AgentSpec` canonical JSON contains component references and extension-owned config.

```json
{
  "schema_version": 1,
  "id": "research-agent",
  "model": { "id": "finstack.model.openai-compatible", "instance": "default" },
  "toolsets": [
    { "id": "finstack.tools.filesystem", "instance": "workspace" }
  ],
  "capabilities": ["research"],
  "limits": { "max_turns": 12, "max_tool_calls": 50 },
  "extension_config": {
    "finstack.model.openai-compatible/default": {
      "model": "example-model",
      "base_url": "https://example.invalid/v1"
    }
  }
}
```

Secrets are references, not literal values in recommended configurations.

## 29.2 Resolution diagnostics

Agent build returns a structured report containing:

- resolved component IDs and versions;
- aliases used;
- capability expansion;
- tool-name conflicts;
- middleware order;
- provider capability mismatches;
- unused configuration; and
- warnings.

# 30. Error model

## 30.1 Stable code

```rust
pub struct FrameworkError {
    pub code: ErrorCode,
    pub message: Arc<str>,
    pub category: ErrorCategory,
    pub retryable: bool,
    pub identifiers: ErrorIdentifiers,
    pub safe_details: Metadata,
    pub source: Option<Arc<dyn Error + Send + Sync>>,
}
```

Public serialized errors omit unsafe source text unless explicitly enabled.

## 30.2 Categories

```text
configuration
registration
validation
model
tool
context
middleware
store
cancellation
deadline
limit
recovery
corruption
protocol
plugin
internal
```

## 30.3 Panic policy

A panic is never used for user input, provider output, plugin output, or invalid journal data. Unrecoverable internal invariant failures may panic in debug/tests; production boundaries catch and convert where safe. Plugin and Python callback panics/exceptions become component errors.

# 31. Security design

## 31.1 Native code

Native Rust and Python components are trusted. The SDK documentation must state this plainly.

## 31.2 Tool authorization

Tool implementations enforce resource boundaries. Middleware can require approval or deny calls. The runtime includes call identity and principal metadata in `ToolCallContext`.

## 31.3 Principal context

```rust
pub struct Principal {
    pub id: Arc<str>,
    pub tenant: Option<Arc<str>>,
    pub roles: Arc<[Arc<str>]>,
    pub claims: Metadata,
}
```

The principal is application-supplied and propagated explicitly, not through task-local globals.

## 31.4 Sensitive data

`Sensitivity` classifications include public, internal, confidential, secret, and credential. Event subscriptions and observers declare the maximum permitted level and redaction mode.

## 31.5 Plugin manifests

A plugin manifest includes identity, version, WIT world, component digest, requested permissions, resource limits, configuration schema, and optional signature. Signature policy is host configuration, not kernel behavior.

# 32. Testing design

## 32.1 Test pyramid

### Kernel unit tests

Exhaustive phase transitions, invalid inputs, limits, cancellation, tool pairing, and record application.

### Property tests

Generate valid/invalid message histories, effect sequences, tool batches, and cancellation points. Assert invariants and deterministic decisions.

### Fuzzing

Targets include:

- CBOR/JSON parsing;
- journal replay;
- provider stream assembly;
- tool-call delta assembly;
- WIT payload validation; and
- state transition inputs.

### Runtime integration tests

Scripted model, fake toolsets, manual clock, fault store, slow observers, and forced task cancellation.

### Crash-prefix tests

Run a scenario, stop after every append/effect boundary, restore, and compare final durable result.

### Binding conformance tests

The same fixture file drives Rust, Python, and WASM. Compare durable records and normalized events.

### Plugin conformance tests

Reference host invokes compliant and intentionally invalid WIT components, checking limits, permissions, malformed payloads, and shutdown.

## 32.2 Golden trace format

A trace fixture contains:

```text
initial agent spec
initial session records
scripted model/tool/context outcomes
transition timestamps and IDs
expected durable records
expected normalized event order
expected final state/result
```

Fixtures are language-neutral JSON for review, with optional CBOR binary fixtures for protocol compatibility.

## 32.3 Concurrency testing

Use deterministic scheduling where possible and tools such as Loom for synchronization primitives in the runtime. Test simultaneous lane runs, tool batches, cancellation, shutdown, and store conflicts.

## 32.4 Miri and sanitizers

Run Miri on kernel and low-level protocol tests where practical. Native CI includes address/thread sanitizer jobs for selected integration tests.

# 33. Benchmark design

## 33.1 Microbenchmarks

- kernel `decide` and `apply` by transition kind;
- record encode/decode;
- tool registry lookup;
- context assembly;
- event batching;
- message history view construction; and
- snapshot replay.

## 33.2 End-to-end synthetic benchmarks

1. one text-only response;
2. 1/10/100/1,000 model deltas;
3. one fast tool call;
4. 100 fast tool calls;
5. parallel and sequential batches;
6. output validation;
7. cancellation at each boundary;
8. approval suspend/resume;
9. journal restore;
10. 1,000 idle sessions;
11. 100 active sessions; and
12. large result via blob reference.

## 33.3 Binding benchmarks

Report separately:

- native Rust components from Rust;
- native Rust components from Python;
- one Python tool callback;
- Python model stream callback;
- browser WASM with JS adapters;
- native host with WIT toolset.

Do not include real model latency in framework overhead comparisons.

## 33.4 Benchmark artifacts

CI stores machine metadata, compiler version, commit, feature set, raw samples, and flamegraphs for regressions above configured thresholds.

# 34. Build and CI design

## 34.1 Required checks

```text
cargo fmt
cargo clippy with warnings denied
cargo test workspace
architecture dependency tests
minimal-feature build
wasm32 build and browser tests
Python type/tests and wheel smoke tests
npm/TypeScript tests
protocol compatibility fixtures
cargo deny / supply-chain policy
fuzz smoke jobs
benchmark regression check
```

## 34.2 Feature policy

Cargo features are permitted for:

- target-specific implementation choices;
- optional serialization/debug support;
- optional runtime integrations within a leaf package; and
- build-time selection of bundled batteries.

Features are not used as a giant central registry of every provider/channel/tool.

## 34.3 Release artifacts

- crates.io packages;
- Python wheels and source distribution;
- npm WASM package;
- WIT package source and generated bindings;
- protocol schema/fixture archive;
- SBOM and checksums; and
- benchmark report.

# 35. Implementation milestones

## Milestone 1: Domain and reducer

- typed IDs and content/message model;
- run/turn/tool state;
- records and events;
- deterministic `decide/apply` API;
- scripted trace fixtures;
- native and WASM kernel build.

## Milestone 2: Standard runtime

- in-memory store;
- commit loop;
- event hub;
- model driver;
- tool scheduler;
- cancellation and limits;
- scripted model/tool integration.

## Milestone 3: SDK and first provider

- registrar and registry;
- AgentBuilder/AgentSpec;
- one OpenAI-compatible provider;
- minimal filesystem or calculator toolset;
- Rust examples and benchmarks.

## Milestone 4: Python

- PyO3 classes and async bridge;
- Rust-backed fast path;
- Python tool/model adapters;
- Pydantic integration;
- conformance and benchmark suite.

## Milestone 5: Browser WASM

- JS host adapters;
- worker helper;
- event batches and handles;
- IndexedDB reference store;
- browser conformance and benchmarks.

## Milestone 6: Durability

- SQLite store;
- snapshots;
- recovery reconciliation;
- approval suspension;
- crash-prefix matrix;
- initial lane APIs.

## Milestone 7: Isolated plugins

- WIT v1 toolset/context packages;
- Wasmtime host;
- permission and resource limits;
- reference components;
- plugin conformance tests.

## Milestone 8: Ecosystem readiness

- additional providers/toolsets/stores;
- remote protocol and reference server;
- observer adapters;
- workflow integrations;
- stable compatibility policy and migration tooling.

# 36. Definition of done for core technical design

The core implementation is technically ready for public preview when:

1. `finstack-ai-kernel` has no forbidden dependency.
2. Every kernel transition has a documented input, record output, and invariant test.
3. The native runtime never executes a recoverable effect before its request record is committed.
4. All queues are bounded and tested with slow consumers.
5. Parallel tools finalize history in source order.
6. Cancellation and crash-prefix tests produce valid restored states.
7. Rust, Python Rust-backed, and browser WASM pass common trace fixtures.
8. Benchmarks isolate framework overhead from model/network latency.
9. Adding a fixture provider/toolset/store requires no kernel changes.
10. Public errors, records, events, and specs have versioned schemas.

# 37. Open technical decisions requiring ADRs

| Decision | Options to evaluate |
|---|---|
| Journal encoding | strict CBOR versus JSONL-first with later binary encoding |
| ID representation | UUIDv7 versus ULID or internal 128-bit type with UUID projection |
| Async trait strategy | boxed futures/streams versus GAT-based traits plus object-safe adapters |
| Validation core | JSON Schema engine, Serde-generated validators, binding-owned validation, or hybrid |
| Python ABI | per-version CPython wheels versus stable limited ABI where feasible |
| WASM threading | single-thread default versus optional threads with cross-origin isolation |
| Snapshot format | direct state CBOR versus replay-optimized compact projection |
| Model interruption | retry-only baseline versus provider reconciliation interface in MVP |
| Middleware replay | always record outcomes versus declarative recompute-safe middleware |
| Remote protocol | reuse journal record envelopes versus separate client DTOs and adapters |
| Plugin streaming | synchronous first WIT call versus resource-based async stream in v1 |
| Blob service | application-owned only versus a seventh formal runtime service after MVP |

# 38. Technical traceability matrix

| PRD family | Technical sections |
|---|---|
| FR-KRN | 5-13, 20-24 |
| FR-RT | 13-23 |
| FR-EXT | 8-10 |
| FR-CAP | 10 |
| FR-MDL | 14 |
| FR-TLS | 15, 21 |
| FR-CTX | 16 |
| FR-MW | 17 |
| FR-DUR | 12-13, 18, 23-24 |
| FR-OBS | 19-20 |
| FR-RS | 2-4, 8-19 |
| FR-PY | 25 |
| FR-WASM | 26 |
| FR-PLG | 27, 31 |
| FR-SPEC | 8, 10, 29 |
| NFR performance | 6, 20-21, 25-28, 33 |
| NFR reliability | 12-13, 22-24, 32 |
| NFR compatibility | 28-29, 34 |
