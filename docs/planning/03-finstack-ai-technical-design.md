---
title: "finstack-ai Technical Design Document"
subtitle: "Implementation-level design for the Rust agent microkernel, runtime, bindings, and extension SDK"
author: "finstack-ai project"
date: "2026-08-08"
---

# finstack-ai Technical Design Document

# Document control

| Field | Value |
|---|---|
| Product | `finstack-ai` |
| Document | Technical Design Document (TDD) |
| Version | 0.12 |
| Status | Implementation baseline |
| Primary language | Rust |
| Bindings | Python/PyO3; JavaScript/WebAssembly; optional WIT Component Model |
| Related documents | Engineering Standards v0.5; Product Requirements Document v0.7; Architecture Specification v0.7; Implementation Plan v0.12; Security and Threat Model v0.4 |

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

# 2. Workspace

```text
finstack-ai/
  Cargo.toml
  mise.toml                 # sole toolchain pin + repository tasks; no rust-toolchain.toml
  deny.toml
  licenses/
    LICENSE-MIT
    LICENSE-APACHE

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
        ports/
          mod.rs
          model.rs
          toolset.rs
          context.rs
          middleware.rs
          store.rs
          observer.rs
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
        external_completion.rs
        interactions.rs
        services/
          mod.rs
          agent_invoker.rs
          budget.rs
          artifact.rs
          security_audit.rs
        adapters/

      # Cargo features:
      # default = []
      # native-tokio = Tokio driver/task/timer/shutdown adapters
      # wasm-host = local futures/streams and browser host driver; excludes Tokio

    finstack-ai/                  # package finstack-ai; library finstack_ai
      # Cargo features:
      # default = ["native-tokio"]
      # native-tokio = ["finstack-ai-runtime/native-tokio"]
      # wasm-host = ["finstack-ai-runtime/wasm-host"]
      src/
        lib.rs
        builder.rs
        registrar.rs
        registry.rs
        extension.rs
        capability.rs
        agent_catalog.rs
        bundle.rs
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

  extensions/                 # trusted native leaf batteries (in-process port implementations)
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

  plugins/                    # optional isolated WIT/Wasmtime path (not native batteries)
    finstack-ai-wit/
      wit/v0.0.4/
        finstack-ai-types/types.wit
        finstack-ai-host/host.wit
        finstack-ai-toolset/toolset.wit
        finstack-ai-context/context.wit

    finstack-ai-plugin-host/
      src/

  examples/
    rust-minimal/
    python-minimal/
    browser-minimal/
    durable-interaction/
```

Root `mise.toml` pins contributor/CI tools and hosts repository tasks. Do not add `rust-toolchain.toml`. Node/npm pins arrive with browser/JavaScript binding packages, not in the initial root pin set.

Native providers, toolsets, stores, and observers live under `extensions/` so the repository root keeps a clear core-versus-addon split. `plugins/` remains reserved for the optional isolated WIT/Wasmtime host and worlds; do not place trusted in-process batteries there.

# 3. Crate dependency policy

## 3.1 Dependency graph

```text
                         finstack-ai-kernel
                           ^           ^
                           |           |
              finstack-ai-runtime   finstack-ai-protocol
                           ^           ^
                           |           |
                       finstack-ai     |
                           ^           |
                           |           |
       Rust facade / Python / WASM   remote server/client

provider/tool/observer crates -> runtime port contracts only
durable store crates -> runtime store contract + protocol codec
```

The dependency direction is fixed:

- the kernel owns public semantic IDs, records, events, and effect types;
- the runtime depends on the kernel and owns the six port traits/effect execution;
- the SDK depends on runtime/kernel, owns composition, and re-exports the port contracts with ergonomic adapters;
- protocol depends on the kernel's public semantic DTOs only where journal encoding requires them, and otherwise owns independent framing/remote/process DTOs;
- server/client adapters depend on protocol plus SDK/runtime as required; and
- persistent store implementations may depend on runtime port contracts plus protocol codec, while the in-memory store need not; and
- no kernel, runtime, or SDK crate depends on protocol merely to communicate in-process.

There is no baseline `finstack-ai-types` crate. If a cycle appears, move transport mapping outward into the protocol/server adapter rather than moving kernel-private structures outward or adding a miscellaneous shared crate.

Do not introduce a miscellaneous utility crate unless at least three independent crates need the same stable abstraction.

## 3.2 Target-specific async object bounds

The six logical port method/data contracts are identical across targets, but their executor bounds are target-correct:

```rust
#[cfg(not(target_arch = "wasm32"))]
pub trait PortObject: Send + Sync + 'static {}
#[cfg(not(target_arch = "wasm32"))]
impl<T: Send + Sync + 'static> PortObject for T {}

#[cfg(target_arch = "wasm32")]
pub trait PortObject: 'static {}
#[cfg(target_arch = "wasm32")]
impl<T: 'static> PortObject for T {}

#[cfg(not(target_arch = "wasm32"))]
pub type PortFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;
#[cfg(target_arch = "wasm32")]
pub type PortFuture<T> = Pin<Box<dyn Future<Output = T> + 'static>>;

#[cfg(not(target_arch = "wasm32"))]
pub type PortStream<T> = Pin<Box<dyn Stream<Item = T> + Send + 'static>>;
#[cfg(target_arch = "wasm32")]
pub type PortStream<T> = Pin<Box<dyn Stream<Item = T> + 'static>>;
```

Native runtime handles therefore remain `Send + Sync` and may cross Tokio worker threads. The browser-WASM runtime owns local JavaScript promise/stream proxies on one engine/worker thread and never claims they are thread-safe. Threaded WASM requires separate post-preview contracts/evidence under ADR-031. Conditional bounds do not change method names, normalized values, record/event semantics, or conformance fixtures.

`finstack-ai-runtime` has no default driver. The `finstack-ai` facade depends on it with `default-features = false` and exposes two pass-through driver features: default `native-tokio = ["finstack-ai-runtime/native-tokio"]` and non-default `wasm-host = ["finstack-ai-runtime/wasm-host"]`. `finstack-ai-wasm` depends on the facade with `default-features = false, features = ["wasm-host"]`; it does not bypass the public composition layer. Provider/tool/observer leaves that need only contracts depend on runtime with defaults disabled and no driver. CI rejects Tokio/native adapter dependencies or both drivers in the resolved `wasm32-unknown-unknown` graph. Kernel-only and contract-only builds enable neither driver feature.

## 3.3 Kernel dependency budget

Initial expected direct dependencies:

```text
serde               # derives and versioned data
serde_json           # RawJson validation and diagnostic form
bytes                # shared byte buffers
sha2                 # canonical SHA-256 Digest implementation
thiserror            # stable internal errors
uuid                 # typed UUIDv7-compatible identifiers
```

The kernel does not depend on `futures-core`; it is synchronous. `smallvec`, `bitflags`, and similar convenience/optimization crates are not baseline dependencies and may be added only when concrete code or measurements justify them. Every additional direct dependency requires review.

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

Globally registered/exported `ComponentId`, `ToolId`, and packaged `CapabilityId` values must be namespaced. `AgentId`/`BundleId` and capability keys local to one locked bundle may use bounded local aliases matching `[a-z][a-z0-9._-]{0,127}`; resolution expands them to the owning bundle namespace in `ResolvedAgentLock`. Builder strings such as `"filesystem"` are instance aliases resolved to a namespaced component/tool ID and are never persisted as the global identity.

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
pub type MessageId = Id<MessageTag>;
pub type EntryId = Id<EntryTag>;
pub type ModelRequestId = Id<ModelRequestTag>;
pub type ToolBatchId = Id<ToolBatchTag>;
pub type EffectId = Id<EffectTag>;
pub type ToolCallId = Id<ToolCallTag>;
pub type InteractionId = Id<InteractionTag>;
pub type BudgetScopeId = Id<BudgetScopeTag>;
pub type BudgetReservationId = Id<BudgetReservationTag>;
pub type CancellationRequestId = Id<CancellationRequestTag>;
pub type EventId = Id<EventTag>;
pub type RecordId = Id<RecordTag>;
pub type AppendBatchId = Id<AppendBatchTag>;
pub type ArtifactId = Id<ArtifactTag>;
```

Externally, IDs serialize as canonical lowercase UUID strings. New IDs use UUIDv7 because ordering is useful in logs and storage. Tests use an injected deterministic generator.

Human-selected names are a separate validated family, serialized as strings rather than UUIDs:

```rust
#[repr(transparent)]
pub struct Key<T>(Arc<str>);

pub type AgentId = Key<AgentTag>;
pub type BundleId = Key<BundleTag>;
pub type ComponentId = Key<ComponentTag>;
pub type CapabilityId = Key<CapabilityTag>;
pub type ToolId = Key<ToolTag>;
```

Keys use the namespace rules in section 4, are length/character bounded, and remain stable across process runs. `ToolCallId` identifies one invocation; `ToolId` names a registered tool. ADR-029 applies to allocated runtime entity IDs, not these configuration/package keys.

## 5.2 Provider identifiers

A provider may supply its own request, message, or tool-call ID. These are stored separately as opaque strings and never replace internal stable IDs.

```rust
pub struct ProviderIds {
    pub request_id: Option<Arc<str>>,
    pub response_id: Option<Arc<str>>,
    pub continuation_id: Option<Arc<str>>,
}
```

Provider ID strings are opaque shared buffers. Each present string is bounded by the individual text/byte-string ceiling in section 6.5. Empty strings are rejected.

## 5.3 Transition environment

Clock and ID generation are nondeterministic effects. The runtime supplies them as normalized transition input:

```rust
pub struct TransitionEnv {
    pub now: Timestamp,
    pub ids: AllocatedIds,
}
```

Test fixtures provide exact timestamps and IDs, making decisions reproducible.

## 5.4 Time and duration

```rust
pub struct Timestamp(i64); // Unix epoch milliseconds, UTC
pub struct Duration(u64);  // elapsed milliseconds
```

`Timestamp` is signed milliseconds since 1970-01-01T00:00:00Z, restricted to UTC years 0001 through 9999. Canonical CBOR uses the integer; JSON/diagnostic text uses an exact RFC 3339 UTC value with millisecond precision; Python uses an aware UTC datetime/int conversion and JavaScript uses an integer `number` (the supported range is within exact IEEE-754 integer range). Leap seconds are not representable and follow Unix-time mapping. `Duration` is non-negative milliseconds with checked arithmetic and configured maxima.

Durable deadlines/timer due-times are wall-clock `Timestamp` values. The runtime converts remaining time to a monotonic clock for in-process waiting so wall-clock adjustments do not extend active waits; monotonic instants are never serialized. On restart it recomputes remaining time from the persisted wall deadline, fires overdue timers once through the durable timer effect, and diagnoses/clamps backward-clock anomalies according to the persisted original maximum duration. Semantic/commit timestamps and elapsed durations are distinct fields.

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

`RawJson::parse` uses a bounded strict parser: duplicate object keys, trailing data, excessive depth/size, and non-JSON numeric values are rejected. For serialized input, the source span is length-checked before parsing and both the source bytes and resulting RFC 8785 canonical bytes must fit the section 6.5 ceiling. Programmatic construction must satisfy the same canonical-byte ceiling. Original validated bytes may be retained for provider round-tripping, but equality/idempotency digests use RFC 8785 JSON Canonicalization Scheme bytes. Unicode strings are not normalized beyond JSON decoding, so visually similar but byte-distinct code points remain distinct.

The portable JCS numeric domain is finite IEEE-754 binary64. Canonicalization parses and serializes numbers under RFC 8785/ECMAScript rules; overflow is rejected. Schemas must encode exact integers outside `-(2^53-1)..=(2^53-1)`, precision-sensitive decimals, and monetary quantities as strings or typed non-JSON fields rather than JSON numbers. Durable typed CBOR DTOs may use their declared wider integer types. Cross-language fixtures cover the safe-integer boundaries, `i64`/`u64` string encodings, exponent normalization, rounding-equivalent lexical forms, negative zero, underflow/overflow, and non-finite rejection.

## 6.2 `Metadata`

Public and durable metadata uses one bounded opaque object rather than unconstrained language maps:

```rust
#[repr(transparent)]
pub struct Metadata(RawJson);
```

`Metadata::parse` requires a top-level JSON object and `{}` is the default. It inherits strict `RawJson` validation. Equality and semantic digests use JCS canonical bytes; canonical CBOR carries those JCS bytes as a byte string, while human and diagnostic JSON emits the object itself. Every unknown member is preserved during round-trip even when a consumer ignores it. Metadata is included in the enclosing object's digest and never grants authority; a member that changes behavior requires an explicitly versioned enclosing contract.

V1 metadata is limited to 64 KiB each for its serialized source span and canonical JCS bytes, 64 top-level members, 128 UTF-8 bytes per key, and depth 16. The source-span limit is checked before parsing; programmatic construction enforces the canonical-byte limit. Keys are namespaced outside a field owner's documented stable keys. `ErrorDescriptor.safe_details`, `AuthorizationContext.safe_claims`, and artifact metadata are non-secret. Child-run metadata cannot change principal, tenant, deadline, placement, or budget. Provider message metadata is exposed only to its owning adapter unless a versioned portable member says otherwise.

## 6.3 Shared buffers

Messages, schemas, and results use `Arc<str>`, `Bytes`, or `Arc<[T]>` where sharing is common. Public APIs avoid exposing borrowed lifetimes that cannot map to Python or WASM.

## 6.4 Digest contract

```rust
pub struct Digest([u8; 32]);
```

The v1 digest algorithm is SHA-256, serialized as lowercase 64-character hexadecimal. Every digest is domain-separated over:

```text
"finstack-ai" NUL domain-name NUL schema-version-u32-be NUL canonical-bytes
```

The domain name is fixed per use (`raw-json`, `record-payload`, `effect-input`, `effect-output`, `blob-content`, `snapshot-state`, `middleware-chain`, `agent-spec`, or another versioned registry entry). JSON uses the strict RFC 8785 bytes above; durable DTOs use the frozen canonical CBOR profile; blob content uses the exact raw bytes. A digest comparison never mixes domains or schema versions. Cross-language known-answer fixtures include key-order/whitespace-equivalent JSON, duplicate-key rejection, numeric edge cases, Unicode, empty/large blobs, and every durable record family.

`finstack-ai-kernel::digest` owns `Digest`, the domain registry, SHA-256 wrapper, and strict canonical-JSON normalization used by semantic DTOs. Runtime and SDK call that module; protocol applies the same type to its canonical-CBOR bytes; leaf adapters do not implement competing hash/JCS rules. PR-003/PR-006 pin/audit the hash dependency and known-answer corpus.

## 6.5 V1 semantic payload bounds

The following are default and hard v1 schema ceilings. Deployments may configure lower creation or ingress limits, but replay decoders must continue accepting every valid committed v1 value up to these ceilings.

| Dimension | V1 ceiling |
|---|---:|
| Canonical record envelope | 8 MiB |
| Atomic append batch | 16 MiB and 256 records |
| Individual text or byte string | 4 MiB |
| Array items | 4,096 |
| Map entries | 256 |
| Canonical-CBOR nesting depth | 32 |
| `RawJson` value | 1 MiB and depth 32 |
| `Metadata` value | 64 KiB, 64 top-level members, 128-byte keys, depth 16 |

The canonical record-envelope size is the byte length of the complete canonical-CBOR `RecordEnvelope`. The atomic append-batch byte size is the checked sum of its committed canonical record-envelope lengths, excluding transport framing, database pages, transaction metadata, and other backend overhead; it must satisfy both the 16 MiB sum and independent 256-record cap. No separate canonical batch encoding is implied.

Constructors and decoders check the applicable byte, item, and depth limits before allocation and return a stable error; they never truncate semantic input. For `RawJson` and `Metadata` parsed from serialized input, both the source span and canonical JCS bytes must fit their respective byte ceiling. Replay-required content larger than these ceilings uses a scoped `ArtifactRef`. Protocol, plugin, provider, tool, and deployment limits may be stricter; the 16 KiB pre-authentication frame ceiling remains independent. Raising a hard schema ceiling after release is a compatibility change with new boundary fixtures.

# 7. Content and message model

## 7.1 Content blocks

The v1 shipping `ContentBlock` set is:

```rust
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContentBlock {
    Text(TextBlock),
    Json(JsonBlock),
    Image(MediaRef),
    Audio(MediaRef),
    File(MediaRef),
    ToolCall(ToolCallBlock),
    ToolResult(ToolResultBlock),
    Opaque(OpaqueBlock),
}
```

`Reasoning` and `Refusal` variants are deferred until a consumer PR freezes their payload DTOs; adding them later is an additive pre-1.0 enum extension with fixtures. Human JSON uses an explicit `kind` discriminator with snake_case variant names. Unknown `kind` values and unknown object members are rejected. Nested `ToolCall`/`ToolResult` blocks inside a `ToolResultBlock` payload are rejected.

```rust
pub struct TextBlock {
    pub text: Arc<str>,
}

pub struct JsonBlock {
    pub value: RawJson,
}

pub struct MediaRef {
    pub blob: BlobRef,
}

pub struct ToolCallBlock {
    pub tool_call_id: ToolCallId,
    pub tool_name: Arc<str>,
    pub arguments: RawJson,
}

pub struct ToolResultBlock {
    pub tool_call_id: ToolCallId,
    pub content: Arc<[ContentBlock]>,
    pub is_error: bool,
}

pub enum OpaquePayload {
    Bytes(Bytes),
    Json(RawJson),
}

pub struct OpaqueBlock {
    pub media_type: Arc<str>,
    pub payload: OpaquePayload,
}
```

`TextBlock.text`, opaque byte payloads, and other individual text/byte strings are bounded by the section 6.5 4 MiB ceiling. `JsonBlock.value` and `ToolCallBlock.arguments` use `RawJson` ceilings. `tool_name` and opaque/media type strings are non-empty, at most 256 UTF-8 bytes, and must not contain NUL. `OpaqueBlock` carries a namespaced media type and bytes/JSON for provider-specific round-tripping. Components that do not understand it must preserve it when the selected provider requires it. Media variants carry only a `BlobRef`; constructors and decoders reject any inline payload member.

## 7.2 Blob reference

```rust
pub struct BlobRef {
    pub id: Arc<str>,
    pub media_type: Arc<str>,
    pub length: u64,
    pub digest: Option<Digest>,
    pub name: Option<Arc<str>>,
}
```

`BlobRef` does not carry metadata. Bounded non-secret metadata for durable artifacts belongs on `ArtifactRef`. `id` and `media_type` are non-empty and at most 256 UTF-8 bytes without NUL; optional `name` uses the same bound when present. Optional integrity digests for blob bytes use the `blob-content` digest domain over the exact raw bytes. The core never dereferences a blob. Toolsets, context providers, bindings, or applications do so through their own services.

Artifact, external-handle, and identity references use small transport-safe records rather than application objects:

```rust
pub struct ArtifactRef {
    pub id: ArtifactId,
    pub kind: Arc<str>,
    pub blob: BlobRef,
    pub content_digest: Digest,
    pub scope_digest: Digest,
    pub metadata: Metadata,
}

pub struct ExternalHandleRef {
    pub provider: ComponentId,
    pub handle: Arc<str>,
    pub reconciliation_metadata: RawJson,
}

pub struct PrincipalRef {
    pub issuer: Arc<str>,
    pub subject: Arc<str>,
    pub tenant_scope: Option<Arc<str>>,
}

pub enum AssigneeHint {
    Principal(PrincipalRef),
    Role(Arc<str>),
    Queue(Arc<str>),
}
```

`ArtifactRef` content is always reached through its `BlobRef`; the metadata cannot carry secret material. `ExternalHandleRef` contains non-secret provider identifiers and reconciliation metadata only. `PrincipalRef` is an authenticated identity reference, not a bearer credential, and `AssigneeHint` never grants authorization by itself.

```rust
pub struct AuthorizationEvidence {
    pub policy_version: Arc<str>,
    pub decision_id: Arc<str>,
}
```

## 7.3 Message roles

```rust
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    System,
    Developer,
    User,
    Assistant,
    Tool,
}
```

Role/block validity matrix (v1):

| Role | Allowed blocks |
|---|---|
| `System`, `Developer` | `Text`, `Json`, `Opaque` |
| `User` | `Text`, `Json`, `Image`, `Audio`, `File`, `Opaque` |
| `Assistant` | `Text`, `Json`, `Image`, `Audio`, `File`, `ToolCall`, `Opaque` |
| `Tool` | `ToolResult` only; at least one `ToolResult` required |

## 7.4 Model message

```rust
pub enum ThinkingLevel {
    Low,
    Medium,
    High,
}

pub struct ModelRef {
    pub provider: Arc<str>,
    pub model: Arc<str>,
    pub thinking_level: Option<ThinkingLevel>,
    pub context_length: Option<u64>,
    pub fast: Option<bool>,
}

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

`ModelRef.provider` and `ModelRef.model` are non-empty, at most 256 UTF-8 bytes, and must not contain NUL. `thinking_level`, `context_length`, and `fast` are independently optional selected-configuration fields: a model reference may omit all three, carry thinking only, fast only, context length only, or any combination (including thinking and fast together). When present, `context_length` is a positive token budget in the portable exact-JSON integer range (`1..=2^53-1`); larger values are rejected rather than serialized imprecisely for JavaScript consumers. Absent optional fields are omitted from JSON (`skip_serializing_if`). `Message.content` is bounded by the section 6.5 array-item ceiling (4,096). `created_at` is supplied by the runtime environment; the kernel never reads a clock. `Usage` is not a message field in PR-007; token/cost usage types remain owned by budget and effect-completion surfaces (PR-008/PR-011) and are deferred until those consumers freeze them.

Constructors and deserializers validate the role/block matrix and reject invalid combinations with stable validation error codes. Tool-association validation in the message model is pure and structural:

- every `ToolResultBlock.tool_call_id` in a `Tool` message must be unique within that message;
- when a caller supplies an optional known-call set, every result `tool_call_id` must be present in that set;
- missing/unknown/duplicate associations and wrong-role tool blocks are rejected.

Run-state pairing—pending-call tracking, exactly-once settlement, source-order finalization, synthetic closure, cancellation, and recovery—belongs to the reducer (PR-010) and is out of scope for the message DTO layer.

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
    pub capabilities: Vec<CapabilityRef>,
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
    pub lock: Arc<ResolvedAgentLock>,
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

## 8.3 Bundle and resolution lock

```rust
pub struct BundleSpec {
    pub schema_version: u16,
    pub id: BundleId,
    pub version: Version,
    pub agents: Vec<AgentSpec>,
    pub capabilities: Vec<CapabilitySpec>,
    pub requirements: Vec<BundleRequirement>,
    pub conflicts: Vec<BundleConflict>,
    pub defaults: BundleDefaults,
    pub config_schema: Option<SchemaRef>,
    pub compatibility: CompatibilityRequirements,
}

pub struct ResolvedAgentLock {
    pub schema_version: u16,
    pub engine_version: Version,
    pub agent_spec_digest: Digest,
    pub bundle: Option<LockedBundle>,
    pub components: Vec<LockedComponent>,
    pub capabilities: Vec<LockedCapability>,
    pub effective_config_digest: Digest,
    pub middleware_chain_digest: Digest,
    pub schema_digests: Vec<Digest>,
}
```

`BundleRequirement` supports only a required component with compatible major/range, an optional component, a required capability, a finite one-of component list, a required host feature, or a minimum framework-contract version. `BundleConflict` names incompatible component/capability IDs or host features. The runtime does not solve package installation: installation produces an exact package/component lock, registrations must match it, and `BundleResolver` validates/expands it into immutable `ResolvedAgent` values.

Configuration precedence is fixed, lowest to highest:

```text
component defaults
  < bundle defaults
  < application configuration
  < agent-specific configuration
  < run overrides explicitly allowlisted by policy
```

Unknown keys, conflicts, unresolved alternatives, incompatible versions/features, or a non-allowlisted run override fail before run acceptance. Secrets are references resolved by the host and never embedded in a bundle, spec, effective-config digest input, or lock export.

Every successful resolution emits a canonical, versioned `ResolvedAgentLock`. Applications can export and later re-import it; reconstruction fails closed when an exact component/version/schema/config selection is absent or incompatible. The lock contains identifiers, versions, compatibility selections, and digests but no executable handles, credentials, or environment-specific secret values. `AgentSpec.capabilities` contains explicit `CapabilityRef` objects; capability definitions live in the same bundle or a registered, locked catalog, so JSON and Rust use one shape rather than mixing embedded specs and bare strings.

## 8.4 Rust typed result decoding

```rust
impl RunResult {
    pub fn decode<T: serde::de::DeserializeOwned>(&self) -> Result<T, ResultDecodeError>;
}
```

`decode` is available only for a structured result whose committed output-contract/schema digest matches the resolved output specification. It deserializes the already validated `RawJson` bytes with Serde, reports stable path-aware errors, and never reruns the model or mutates the run. Custom decoders register against the same schema/output-contract digest and conformance fixtures.

# 9. Extension registrar

## 9.1 Extension trait

```rust
pub trait Extension: PortObject {
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

The registrar accepts either a ready implementation handle or a typed async factory for configuration-dependent initialization. Agent construction validates configuration and invokes each selected factory once under a construction deadline/cancellation context. A factory returns a ready handle plus declared shutdown ownership; failures occur before the run starts. `ResolvedAgent` stores ready handles only, so no factory lookup or initialization occurs on the run hot path.

## 9.3 Duplicate behavior

Duplicate IDs are errors. Replacement requires explicit `replace(id, expected_owner, new_value)` and is logged in resolution diagnostics.

## 9.4 Non-kernel composition services

The SDK/runtime defines three reusable services that are not primary kernel ports:

- `BundleResolver` validates a `BundleSpec`, expands dependencies/conflicts/defaults against the registrar, and produces immutable resolved component references before agent construction completes.
- `BudgetLedger` optionally reserves and aggregates usage across `BudgetScopeId` values for parent/child/delegated runs. The kernel still enforces per-run limits and records usage; pricing, shared allocation, and account policy remain application-owned.
- `ArtifactStore` stores/loads scoped large content and metadata addressed by `BlobRef`/`ArtifactRef`. Authorization, retention, encryption, and backend selection remain application/runtime responsibilities.

These service handles live in construction/run services and are never called by the kernel. Bundle/budget/artifact services are optional unless resolved composition requires them. `SecurityAuditSink` is different: it is mandatory whenever remote/external completion or interaction ingress is enabled and may be omitted only in pure embedded configurations with no such ingress. `ResolvedAgent`/server construction contains validated direct handles or an explicit absent state; lookup is not repeated inside progress paths.

`BundleResolver` and `AgentCatalog` are SDK composition services. `AgentInvoker`, `BudgetLedger`, `ArtifactStore`, and `SecurityAuditSink` are runtime service contracts so the SDK and leaf adapters depend inward without a cycle.

```rust
pub trait SecurityAuditSink: PortObject {
    fn record(
        &self,
        event: SecurityAuditEvent,
    ) -> PortFuture<Result<SecurityAuditReceipt, SecurityAuditError>>;
}

pub struct SecurityAuditEvent {
    pub event_id: EventId,
    pub occurred_at: Timestamp,
    pub tenant_scope: Option<Arc<str>>,
    pub principal: Option<PrincipalRef>,
    pub authorization: Option<AuthorizationEvidence>,
    pub category: Arc<str>,
    pub reason_code: Arc<str>,
    pub locator_digest: Option<Digest>,
    pub submitted_digest: Option<Digest>,
}
```

Ingress allocates a stable event ID and retries the same normalized event idempotently. The sink has a bounded call deadline/queue, returns a durable receipt or error, and never receives bearer tokens, raw credentials, submitted content, or an existence-revealing locator. For unauthenticated, scope-mismatched, malformed-token, or unknown-locator commands, a receipt is required before the rejection response; timeout/failure rejects closed and raises an operational health signal. Known authorized conflicts additionally use the journal `ExternalCommandRejected` record as authoritative run-adjacent evidence.

```rust
pub trait AgentInvoker: PortObject {
    fn start_or_attach(
        &self,
        ctx: ChildRunContext,
        request: ChildRunRequest,
    ) -> PortFuture<Result<ChildRunHandle, AgentInvokeError>>;
}

pub struct ChildRunContext {
    pub parent: OperationLocator,
    pub parent_effect_id: EffectId,
    pub authorization: AuthorizationContext,
}

pub struct ChildRunRequest {
    pub agent: AgentRef,
    pub input: Arc<[ContentBlock]>,
    pub placement: ChildPlacement,
    pub requested_deadline: Option<Timestamp>,
    pub requested_budget: BudgetRequest,
    pub delegation_id: Option<Arc<str>>,
    pub metadata: Metadata,
}

pub enum ChildPlacement {
    CompatibleLaneInParentSession,
    IsolatedChildSession,
    RemoteChildSession,
}

pub struct ChildRunLocator {
    pub operation: OperationLocator,
    pub remote: Option<RemoteRouteRef>,
}

pub struct RemoteRouteRef {
    pub service: ComponentRef,
    pub route: ExternalHandleRef,
}

pub struct ChildRunHandle {
    pub locator: ChildRunLocator,
    pub relation_digest: Digest,
}
```

`start_or_attach` is a durable idempotent handshake, not an ordinary fire-and-forget call. Before any child execution, the parent runtime allocates all required UUIDv7 session/lane/run IDs and any non-secret remote route, then commits the complete `ChildRunLocator` in `ChildRunPrepared` through the parent commit coordinator. Same-session placement freezes the exact new lane; isolated placement freezes the child session/main-lane; remote placement freezes the service plus opaque route and remote operation IDs. The store enforces one mapping per `(parent_run_id, parent_effect_id)` for `ChildAgent` invocation. A racing/retried equal digest returns the committed mapping; a different digest is a durable conflict/audit error. The child side then accepts exactly that locator/`RunId` idempotently: equal `RunAccepted` content attaches to the existing child, while different content fails closed. A crash between parent preparation and child acceptance simply retries the second step. No child `RunId` is derived from an `EffectId`, and recovery never scans globally by `RunId`.

The default placement is `CompatibleLaneInParentSession` only when tenant, store, retention, permission, and agent policies match; otherwise resolution selects `IsolatedChildSession`. Remote placement requires the remote protocol and uses the same prepared mapping. Recovery reconnects to a completed/active/suspended child by the stored ID or performs the missing acceptance. Product-specific delegation/fan-out policy remains middleware/application code.

```rust
pub trait BudgetLedger: PortObject {
    fn reserve(&self, request: BudgetReserveRequest)
        -> PortFuture<Result<BudgetReservationReceipt, BudgetError>>;
    fn reconcile(&self, scope_id: BudgetScopeId, reservation_id: BudgetReservationId)
        -> PortFuture<Result<BudgetReservationState, BudgetError>>;
    fn charge(&self, request: BudgetChargeRequest)
        -> PortFuture<Result<BudgetChargeReceipt, BudgetError>>;
    fn release(&self, request: BudgetReleaseRequest)
        -> PortFuture<Result<BudgetReleaseReceipt, BudgetError>>;
}

pub struct BudgetRequest {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cost: Option<CostLimit>,
    pub extension_counters: BTreeMap<LimitKey, u64>,
}

pub struct BudgetReserveRequest {
    pub scope_id: BudgetScopeId,
    pub reservation_id: BudgetReservationId,
    pub run_id: RunId,
    pub amount: BudgetRequest,
    pub request_digest: Digest,
}

pub struct BudgetChargeRequest {
    pub scope_id: BudgetScopeId,
    pub reservation_id: BudgetReservationId,
    pub effect_id: EffectId,
    pub usage: Usage,
    pub usage_digest: Digest,
}

pub struct BudgetReleaseRequest {
    pub scope_id: BudgetScopeId,
    pub reservation_id: BudgetReservationId,
    pub terminal_run_id: RunId,
    pub request_digest: Digest,
}

pub struct BudgetReservationReceipt {
    pub scope_id: BudgetScopeId,
    pub reservation_id: BudgetReservationId,
    pub reserved: BudgetRequest,
    pub remaining: BudgetRequest,
    pub request_digest: Digest,
    pub receipt_digest: Digest,
}

pub struct BudgetChargeReceipt {
    pub scope_id: BudgetScopeId,
    pub reservation_id: BudgetReservationId,
    pub effect_id: EffectId,
    pub charged_usage: Usage,
    pub cumulative_usage: Usage,
    pub usage_digest: Digest,
    pub receipt_digest: Digest,
}

pub struct BudgetReleaseReceipt {
    pub scope_id: BudgetScopeId,
    pub reservation_id: BudgetReservationId,
    pub terminal_run_id: RunId,
    pub released_unused: BudgetRequest,
    pub request_digest: Digest,
    pub receipt_digest: Digest,
}

pub enum BudgetReservationState {
    Reserved(BudgetReservationReceipt),
    Released(BudgetReleaseReceipt),
    NotFound,
    Unknown,
}

pub struct BudgetReservationRequested {
    pub request: BudgetReserveRequest,
}

pub struct BudgetReservationSettled {
    pub receipt: BudgetReservationReceipt,
}

pub struct BudgetChargeRecorded {
    pub receipt: BudgetChargeReceipt,
}

pub struct BudgetReservationReleased {
    pub receipt: BudgetReleaseReceipt,
}
```

Ledger operations are idempotent on `(BudgetScopeId, BudgetReservationId, operation kind)` and bind a normalized request digest; equal retries return the original receipt and conflicting reuse fails closed. When shared/child budgeting is enabled, `ChildRunPrepared` includes a reservation ID and amount. The parent atomically records `BudgetReservationRequested` with preparation, calls `reserve`, then commits `BudgetReservationSettled` before child `RunAccepted`; recovery reconciles the same reservation. `EffectCompleted` commits the reservation ID, normalized usage, and usage digest before the runtime calls `charge`; `BudgetChargeRecorded` commits the returned receipt. A terminal run record similarly commits release intent before `release`, followed by `BudgetReservationReleased`. Recovery finds any committed charge/release intent without its receipt and reconciles or retries the same idempotent operation. Charges are keyed by the settled `EffectId`, monotonic, and never double-applied. Release is idempotent and does not erase charges. Ledger unavailable/unknown outcomes suspend or fail before child/effect dispatch according to locked policy; they never silently grant budget. Per-run kernel limits work without a ledger.

```rust
pub struct ArtifactScope {
    pub tenant_scope: Arc<str>,
    pub session_id: SessionId,
    pub run_id: Option<RunId>,
    pub sensitivity: Sensitivity,
}

pub struct ArtifactMetadata {
    pub kind: Arc<str>,
    pub media_type: Arc<str>,
    pub name: Option<Arc<str>>,
    pub attributes: Metadata,
}

pub trait ArtifactStore: PortObject {
    fn stage_put(
        &self,
        scope: ArtifactScope,
        content: Bytes,
        metadata: ArtifactMetadata,
    ) -> PortFuture<Result<ArtifactRef, ArtifactError>>;

    fn get(
        &self,
        scope: ArtifactScope,
        artifact: ArtifactRef,
    ) -> PortFuture<Result<Bytes, ArtifactError>>;
}
```

For any artifact referenced by a durable behavior-changing record, `stage_put` must durably store the exact bytes and return a content-addressed reference before the journal batch is appended. The runtime verifies length, SHA-256 `content_digest`, and scope binding. A failed journal append may leave an unreferenced staged object; stores garbage-collect such orphans only after a configured grace period. Journal retention pins referenced content for at least the recoverability/audit period. A missing, wrong-scope, or digest-mismatched required artifact is an integrity/recovery error and is never silently recomputed. Only explicitly disposable derived caches such as compatible compaction checkpoints may be discarded and rebuilt.

`stage_put` maps `ArtifactMetadata.kind` verbatim to `ArtifactRef.kind`, `media_type` and `name` to the returned `BlobRef`, and `attributes` to `ArtifactRef.metadata`. It sets `BlobRef.length` from the exact stored bytes and `BlobRef.digest` to the same SHA-256 digest carried by `ArtifactRef.content_digest`. Every field is preserved within the section 6.5 bounds; the store rejects an invalid or oversized value instead of truncating, rewriting, or dropping it. Metadata attributes remain non-secret and non-authoritative: they cannot widen the supplied scope or grant read/write authority.

This is the narrow ENG-SEM-012 preparatory-write exception: the key is the content/domain digest, repeated writes are byte-identical and idempotent, staged content is not discoverable or authoritative before a journal reference, no permission/business action occurs, and both staging bytes/concurrency plus orphan age/total storage are bounded. Disk-full or staging failure prevents the referencing record from committing. All other service/provider work requires committed intent first.

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

At that checkpoint the runtime builds a new immutable `ResolvedRunPlan` from the locked base plus the complete active-capability set. It reruns tool-name, component/version, middleware-order, unique-compactor, post-compactor-mutation, budget, and permission validation before committing activation. The plan carries new component-chain/config/profile digests. Duplicate compaction ownership or any invalid chain rejects activation without partial state change; a changed digest invalidates incompatible compaction checkpoints. Runs never splice a capability into an already executing chain.

## 10.4 Delivery stages

The MVP implements `Always` and `Application`, reserves the `Model` enum value/internal tool identity, and makes activation a durable reducer transition. The 0.0.2-0.0.3 window adds compact catalog rendering, append-only prompt context changes, and activation heuristics across bindings. Those UX/policy pieces must pass real-provider, prompt-cache, and restart tests before the 0.1.0 public preview.

# 11. Kernel state machine API

## 11.1 Command-level inputs

The kernel receives normalized commands only at semantic boundaries:

```rust
pub enum KernelInput {
    AcceptRun(AcceptRun),
    StageSettled(StageSettled),
    ModelSettled(ModelSettled),
    ToolBatchSettled(ToolBatchSettled),
    InteractionSettled(InteractionResolution),
    ExternalEffectCompleted(ExternalEffectCompletion),
    TimerFired(TimerFired),
    CancelRequested(CancellationRequest),
    EffectReconciled(EffectReconciled),
    ResumeRequested(ResumeRequested),
}
```

Fine-grained model and tool progress events are handled by the runtime event sequencer and are not kernel inputs.

## 11.2 Canonical run phases

```rust
pub enum RunPhase {
    Accepted,
    BeforeRun,
    PreparingContext,
    BeforeModel,
    AwaitingModel,
    AfterModel,
    BeforeToolBatch,
    AwaitingTools,
    AfterToolBatch,
    BeforeFinalize,
    AwaitingInteraction,
    AwaitingExternal,
    Sleeping,
    Cancelling,
    Suspended,
    Completed,
    Failed,
    Cancelled,
}
```

The normal path is `Accepted -> BeforeRun -> PreparingContext -> BeforeModel -> AwaitingModel -> AfterModel`, followed by either the tool cycle `BeforeToolBatch -> AwaitingTools -> AfterToolBatch -> BeforeModel` or `BeforeFinalize -> Completed`. Any effect-bearing phase may enter `AwaitingExternal`; middleware/tool policy may enter `AwaitingInteraction`; timers enter `Sleeping`; explicit operator/application suspension enters `Suspended`; cancellation enters `Cancelling` before `Cancelled`. `Completed`, `Failed`, and `Cancelled` are terminal. Every other transition is enumerated in reducer tests; unknown phase/input pairs return `invalid_phase_input`.

## 11.3 Decision API

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

## 11.4 Why no generic graph engine

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
settle_before_finalize
```

The standard run path remains visible in code and amenable to exhaustive state tests.

## 11.5 Run lineage and generalized suspension

```rust
pub struct RunRelation {
    pub root_run_id: RunId,
    pub parent_run_id: Option<RunId>,
    pub parent_effect_id: Option<EffectId>,
    pub kind: RunRelationKind,
    pub depth: u16,
    pub budget_scope_id: Option<BudgetScopeId>,
    pub external_work_ref: Option<Arc<str>>,
}

pub enum RunRelationKind {
    Root,
    ChildAgent,
    DelegatedAgent,
    WorkflowStep,
}

pub struct RunAccepted {
    pub run_id: RunId,
    pub relation: RunRelation,
    pub security: RunSecurityContext,
    pub effective_deadline: Option<Timestamp>,
    pub limits: RunLimits,
    pub propagation: RunPropagationPolicy,
    pub resolved_agent_lock_digest: Digest,
}

pub struct RunSecurityContext {
    pub tenant_scope: Arc<str>,
    pub principal: PrincipalRef,
    pub authentication_method: Arc<str>,
    pub assurance_level: Arc<str>,
    pub authorization_policy_version: Arc<str>,
    pub authorization_decision_id: Arc<str>,
    pub delegated_from: Option<PrincipalRef>,
}

pub struct RunPropagationPolicy {
    pub cancellation: CancellationPropagation,
    pub deadline: DeadlinePropagation,
    pub budget: BudgetPropagation,
    pub principal: PrincipalPropagation,
}

pub enum CancellationPropagation {
    Cascade,
    DetachOnlyIfPreauthorized,
}

pub enum DeadlinePropagation {
    MinimumOfParentAndChild,
}

pub enum BudgetPropagation {
    SharedScope,
    ReservedChildAllocation,
}

pub enum PrincipalPropagation {
    Inherit,
    AttenuatedDelegation,
}

pub struct ChildRunPrepared {
    pub parent_run_id: RunId,
    pub parent_effect_id: EffectId,
    pub child: ChildRunLocator,
    pub request_digest: Digest,
    pub placement: ChildPlacement,
    pub budget_reservation_id: Option<BudgetReservationId>,
}
```

`RunAccepted` always stores relation, security, effective deadline/limits, propagation policy, and resolved-agent lock evidence; root runs use `RunRelationKind::Root`. The kernel validates relation shape, depth, deadline/budget monotonicity, and declared propagation but does not invoke child agents or aggregate budgets. A child inherits the tenant scope, cannot exceed the parent deadline or reserved budget, and either retains the parent principal or uses an explicitly authorized delegated principal whose scopes/roles are attenuated and whose decision ID is persisted. Tenant changes require a separately authenticated boundary. `AgentCatalog`, `AgentInvoker`, and runtime/application policy own invocation services.

`AwaitingExternal` and `AwaitingInteraction` are generic suspension phases. Feature-specific background model, tool, approval, or workflow states are not added to the kernel.

# 12. Journal record design

## 12.1 Envelope

```rust
pub struct RecordEnvelope {
    pub format_version: u16,
    pub kind_version: u16,
    pub record_id: RecordId,
    pub session_id: SessionId,
    pub lane_id: LaneId,
    pub run_id: Option<RunId>,
    pub sequence: u64,
    pub timestamp: Timestamp,
    pub committed_at: Option<Timestamp>,
    pub payload_digest: Digest,
    pub previous_checksum: Option<Digest>,
    pub checksum: Digest,
    pub derived_event_ids: Arc<[EventId]>,
    pub body: RecordBody,
}
```

The runtime supplies the semantic `timestamp` and UUIDv7 IDs (including the fixed ordered IDs for durable-derived events) through `TransitionEnv` before decision/commit, and retries preserve them exactly. The store assigns only `sequence` and may add a separate diagnostic `committed_at`; it never rewrites semantic time. `format_version` versions the envelope while `kind_version` versions the selected record body.

`payload_digest` covers canonical body bytes. `checksum` covers the canonical envelope fields required for replay—format/kind versions, identifiers, assigned sequence, semantic timestamp, payload digest/body, prior checksum, and derived event IDs—and excludes diagnostic `committed_at`. `previous_checksum` links the session sequence; session metadata and snapshots retain the corresponding head checksum. Loads verify sequence continuity, payload/envelope digests, and the chain before applying records.

## 12.2 Record body

Initial variants:

```rust
pub enum RecordBody {
    SessionCreated(SessionCreated),
    LaneCreated(LaneCreated),
    LaneMoved(LaneMoved),
    RunAccepted(RunAccepted),
    ChildRunPrepared(ChildRunPrepared),
    BudgetReservationRequested(BudgetReservationRequested),
    BudgetReservationSettled(BudgetReservationSettled),
    BudgetChargeRecorded(BudgetChargeRecorded),
    BudgetReservationReleased(BudgetReservationReleased),
    StageOutcomeRecorded(StageOutcomeRecorded),
    ContextPrepared(ContextPrepared),
    EffectRequested(EffectRequested),
    EffectDeferred(EffectDeferred),
    EffectCompleted(EffectCompleted),
    EffectFailed(EffectFailed),
    EffectCancelled(EffectCancelled),
    EntryAppended(EntryAppended),
    ToolBatchOpened(ToolBatchOpened),
    ToolBatchClosed(ToolBatchClosed),
    InteractionRequested(InteractionRequest),
    InteractionResolved(InteractionResolution),
    InteractionExpired(InteractionExpired),
    InteractionCancelled(InteractionCancelled),
    ExternalCommandRejected(ExternalCommandRejected),
    CapabilityActivated(CapabilityActivated),
    CancellationRequested(CancellationRequested),
    CancellationReconciled(CancellationReconciled),
    LimitReached(LimitReached),
    TimerFired(TimerFired),
    RunSuspended(RunSuspended),
    RunCompleted(RunCompleted),
    RunFailed(RunFailed),
    RunCancelled(RunCancelled),
    SnapshotWritten(SnapshotWritten),
}
```

Control-path payloads are explicit and versioned rather than inferred from runtime state:

```rust
pub struct CancellationRequest {
    pub request_id: CancellationRequestId,
    pub initiator: CancellationInitiator,
    pub reason: Option<Arc<str>>,
}

pub enum CancellationInitiator {
    Principal {
        principal: PrincipalRef,
        authorization: AuthorizationEvidence,
    },
    ParentRun { parent_run_id: RunId },
    Deadline,
    RuntimeShutdown,
}

pub struct CancellationRequested {
    pub request: CancellationRequest,
}

pub struct CancellationReconciled {
    pub request_id: CancellationRequestId,
    pub completed_effects: Arc<[EffectId]>,
    pub cancelled_effects: Arc<[EffectId]>,
    pub uncertain_effects: Arc<[EffectId]>,
}

pub struct LimitReached {
    pub dimension: LimitDimension,
    pub observed: LimitValue,
    pub maximum: LimitValue,
    pub usage_digest: Digest,
}

pub enum LimitDimension {
    ModelRequests,
    Turns,
    ToolCalls,
    ParallelTools,
    InputTokens,
    OutputTokens,
    ContextBytes,
    OutputBytes,
    Retries,
    WallTime,
    Cost,
    Extension(LimitKey),
}

pub enum LimitValue {
    Count(u64),
    Bytes(u64),
    Duration(Duration),
    Cost {
        unit: Arc<str>,
        micros: u64,
        pricing_policy_version: Arc<str>,
    },
}

pub struct TimerFired {
    pub effect_id: EffectId,
    pub due_at: Timestamp,
    pub fired_at: Timestamp,
}
```

All arrays, maps, strings, raw JSON, metadata, records, and append batches above use the v1 ceilings in section 6.5. Repeating an equal cancellation request or timer firing is idempotent; conflicting reuse of an ID is an invariant error. `CancellationReconciled.uncertain_effects` forces `Suspended` rather than a fabricated `RunCancelled`. `LimitReached` requires matching value variants for its dimension and is committed before limit-driven suspension or terminalization.

## 12.3 Effect records

```rust
pub struct EffectRequested {
    pub effect_id: EffectId,
    pub kind: EffectKind,
    pub relation: Option<EffectRelation>,
    pub component: Option<ComponentInvocation>,
    pub pipeline: Option<PipelinePosition>,
    pub output_contract: EffectOutputContract,
    pub input: EffectInput,
    pub input_digest: Digest,
    pub retry_safety: RetrySafety,
    pub deadline: Option<Timestamp>,
}

pub struct ComponentInvocation {
    pub component: ComponentId,
    pub version: Version,
    pub configuration_digest: Digest,
    pub recovery: InvocationRecovery,
}

pub struct PipelinePosition {
    pub chain_digest: Digest,
    pub stage: Arc<str>,
    pub index: u32,
}

pub enum InvocationRecovery {
    RecomputeSafe,
    Reconcile,
    NonRepeatable,
}

pub struct EffectRelation {
    pub parent_effect_id: EffectId,
    pub purpose: EffectPurpose,
}

pub enum EffectPurpose {
    CompactionSummary { middleware_component_id: ComponentId },
}

pub enum EffectKind {
    Model,
    Tool,
    Context,
    Middleware,
    Interaction,
    Timer,
}
```

Completion records include the normalized output, usage, provider/tool IDs, and retry metadata. Large output needed for replay is staged through `ArtifactStore` and referenced by a digest-bearing `ArtifactRef` before commit; a plain external `BlobRef` is insufficient for authoritative behavior-changing data.

```rust
pub struct EffectDeferred {
    pub effect_id: EffectId,
    pub handle: ExternalHandleRef,
    pub reconciliation: ReconciliationPolicy,
    pub next_poll_at: Option<Timestamp>,
    pub expires_at: Option<Timestamp>,
    pub output_contract: EffectOutputContract,
}

pub struct EffectOutputContract {
    pub kind: EffectOutputKind,
    pub schema_version: u16,
    pub schema_digest: Digest,
}

pub enum EffectOutputKind {
    ModelResponse,
    ToolResult,
    ContextContribution,
    MiddlewareOutcome,
    InteractionResolution,
    TimerFiring,
    ArtifactReceipt,
    Custom(Key<EffectOutputTag>),
}

pub enum ReconciliationPolicy {
    CallbackOnly,
    Poll,
    CallbackOrPoll,
    ExternalWorkflow,
}

pub struct ExternalEffectCompletion {
    pub effect_id: EffectId,
    pub completion_id: Arc<str>,
    pub outcome: ExternalEffectOutcome,
}

pub enum ExternalEffectOutcome {
    Completed {
        output: RawJson,
        usage: Option<Usage>,
        artifacts: Arc<[ArtifactRef]>,
    },
    Failed { error: ErrorDescriptor },
    Cancelled { reason: Option<Arc<str>> },
}
```

External completion and interaction commands carry an explicit durable locator:

```rust
pub struct OperationLocator {
    pub tenant_scope: Arc<str>,
    pub session_id: SessionId,
    pub lane_id: LaneId,
    pub run_id: RunId,
}

pub struct ExternalEffectCompletionCommand {
    pub locator: OperationLocator,
    pub principal: PrincipalRef,
    pub authorization: AuthorizationEvidence,
    pub completion: ExternalEffectCompletion,
}

pub struct InteractionResolutionCommand {
    pub locator: OperationLocator,
    pub resolution: InteractionResolution,
}

pub struct ExternalCommandRejected {
    pub command_kind: ExternalCommandKind,
    pub command_id: Arc<str>,
    pub target: ExternalCommandTarget,
    pub principal: PrincipalRef,
    pub authorization: AuthorizationEvidence,
    pub reason_code: Arc<str>,
    pub submitted_digest: Digest,
    pub accepted_digest: Option<Digest>,
}

pub enum ExternalCommandKind {
    EffectCompletion,
    InteractionResolution,
}

pub enum ExternalCommandTarget {
    Effect(EffectId),
    Interaction(InteractionId),
}
```

The ingress adapter authenticates the caller and binds any signed opaque callback token to the complete locator before invoking the runtime router. The router never scans journals by `effect_id` or `interaction_id`; it validates tenant scope, loads the named session, verifies lane/run/target identity and authorization, then validates/decodes the outcome against the non-optional kind/version/schema digest copied from `EffectRequested` into `EffectDeferred` before proposing typed kernel input. A matching command identity plus normalized payload digest is idempotent. Even a tool without an application output schema uses the versioned framework `ToolResult` envelope schema (with a permissive content member), so the outer contract is never absent.

## 12.4 Interaction records

```rust
pub struct InteractionRequest {
    pub request_version: u16,
    pub interaction_id: InteractionId,
    pub effect_id: EffectId,
    pub kind: InteractionKind,
    pub prompt: Arc<[ContentBlock]>,
    pub prompt_digest: Digest,
    pub response_schema: RawJson,
    pub response_schema_digest: Digest,
    pub policy_component: ComponentRef,
    pub policy_version: Version,
    pub assignee_hint: Option<AssigneeHint>,
    pub expires_at: Option<Timestamp>,
    pub delegatable: bool,
    pub metadata: Metadata,
}

pub enum InteractionKind {
    Approval,
    Choice,
    Form,
    FreeText,
    Review,
    Correction,
    Custom(Arc<str>),
}

pub struct InteractionResolution {
    pub interaction_id: InteractionId,
    pub resolution_id: Arc<str>,
    pub principal: PrincipalRef,
    pub authorization: AuthorizationEvidence,
    pub response: RawJson,
    pub comment: Option<Arc<str>>,
}

pub struct InteractionExpired {
    pub interaction_id: InteractionId,
    pub expired_at: Timestamp,
}

pub struct InteractionCancelled {
    pub interaction_id: InteractionId,
    pub principal: Option<PrincipalRef>,
    pub authorization: Option<AuthorizationEvidence>,
    pub reason: Option<Arc<str>>,
}
```

For a principal-initiated cancellation, `principal` and `authorization` are both present and must match the authorized command. Framework expiry, parent/run cancellation, and shutdown use neither; one without the other is invalid.

The runtime validates a resolution against the recorded schema and authorization policy before proposing `InteractionResolved`. Duplicate `resolution_id` values with the same normalized response digest are idempotent; conflicting or late resolutions fail closed and are audited. Approval helpers construct `InteractionKind::Approval`; there are no approval-only durable records.

An interaction uses one generic effect lifecycle. The request batch atomically commits `EffectRequested { kind: Interaction, effect_id }` and `InteractionRequested` with the same ID; either without the other is corruption. A valid resolution atomically commits `InteractionResolved` and the matching `EffectCompleted`; expiry/failure/cancellation atomically commits the corresponding interaction terminal record plus `EffectFailed`/`EffectCancelled`. Denial is a schema-valid `InteractionResolved` approval response, not an `InteractionRejected` record. The persisted request/policy versions and prompt/schema digests remain authoritative across deployment upgrades; an incompatible resolver suspends for migration/operator action rather than guessing or cancelling silently.

For an authenticated command whose locator and target are known, a conflicting duplicate or invalid late command appends an `ExternalCommandRejected` record containing command kind/identity, target ID, principal, stable reason code, submitted digest, and the prior accepted digest when disclosure policy permits. Applying this record changes no run state but provides durable audit evidence. Authentication failures, scope mismatches, malformed tokens, and unknown locators are written only to the required security-audit sink to avoid journal probing and existence disclosure; they never reach the kernel. If required audit recording fails, the command fails closed.

## 12.5 Atomic batches

A store append accepts a batch and expected previous sequence:

```rust
pub struct AppendRequest {
    pub batch_id: AppendBatchId,
    pub session_id: SessionId,
    pub expected_sequence: u64,
    pub records: Vec<RecordDraft>,
}

pub struct CommittedBatch {
    pub batch_id: AppendBatchId,
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
pub type ModelEventStream = PortStream<Result<ModelStreamItem, ModelError>>;

pub trait Model: PortObject {
    fn descriptor(&self) -> ModelDescriptor;
    fn capabilities(&self, model: &ModelName) -> ModelCapabilities;

    fn request(
        &self,
        ctx: ModelCallContext,
        request: ModelRequest,
    ) -> PortFuture<Result<ModelEventStream, ModelError>>;

    fn reconcile(
        &self,
        _ctx: ReconcileContext,
        _effect: PendingModelEffect,
    ) -> PortFuture<Result<ReconcileResult, ModelError>> {
        Box::pin(async { Ok(ReconcileResult::Unknown) })
    }
}
```

A no-GAT boxed future/stream API is selected initially because it maps cleanly to trait objects, Python adapters, and plugin proxies. Performance-critical first-party providers may use internal concrete types behind the trait.

## 14.2 Request

```rust
pub struct ModelRequest {
    pub request_id: ModelRequestId,
    pub effect_id: EffectId,
    pub model: ModelName,
    pub messages: Arc<[Message]>,
    pub tools: Arc<[ToolSpec]>,
    pub output: OutputSpec,
    pub settings: ModelSettings,
    pub limits: ModelRequestLimits,
    pub provider_state: Option<OpaqueState>,
}

pub struct ModelRequestDraft {
    pub model: ModelName,
    pub messages: Arc<[Message]>,
    pub tools: Arc<[ToolSpec]>,
    pub output: OutputSpec,
    pub settings: ModelSettings,
    pub limits: ModelRequestLimits,
}
```

`ModelRequestId` correlates one logical model turn request across records/events/bindings; `EffectId` identifies its committed external execution/idempotency lifecycle. A retry/reconciliation reuses both IDs and frozen request bytes. A later continuation after tools allocates a new pair.

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
    pub context_profile: ModelContextProfile,
    pub native_tool_calls: bool,
    pub parallel_tool_calls: bool,
    pub structured_output: StructuredOutputCapability,
    pub reasoning: bool,
    pub prompt_cache: bool,
    pub resumable_stream: bool,
    pub idempotent_requests: bool,
    pub native_capabilities: BTreeSet<CapabilityKey>,
}

pub struct ModelContextProfile {
    pub model: ModelName,
    pub hard_input_bytes: u64,
    pub context_window_tokens: u64,
    pub max_output_tokens: u64,
    pub reserved_output_tokens: u64,
    pub provider_overhead_tokens: u64,
    pub estimator: TokenEstimatorRef,
}

pub struct TokenEstimatorRef {
    pub id: Arc<str>,
    pub version: Arc<str>,
    pub source: TokenEstimatorSource,
}

pub enum TokenEstimatorSource {
    ProviderTokenizer,
    ProjectExact,
    ConservativeUpperBound,
}
```

The provider/model descriptor supplies hard ceilings and its estimator identity/version. `AgentSpec` may only reduce input/output budgets or increase reserved/provider-overhead safety margins; an explicitly allowlisted run override may reduce them again, never exceed provider ceilings. Resolution computes and locks the effective profile and its digest. If no exact tokenizer exists, the provider supplies a named/versioned conservative estimator with a documented upper-bound rule; an unknown estimator cannot be treated as exact. Hard byte and token validation always runs after compaction and before the model effect commit.

Provider-specific capability detail lives in namespaced metadata rather than kernel enums whenever possible.

## 14.5 Reference implementations

`ScriptedModel` is the semantic reference and drives all deterministic conformance fixtures. The reference network provider is OpenAI-compatible with Chat Completions as the required baseline. Responses API mapping is an optional adapter extension. A versioned quirks table captures endpoint deviations, and Anthropic fixtures act as the early check that `Model` remains provider-neutral.

# 15. Toolset port design

```rust
pub struct ToolBatch {
    pub tool_batch_id: ToolBatchId,
    pub turn_id: TurnId,
    pub calls: Arc<[ValidatedToolCall]>,
}

pub struct ToolBatchOpened {
    pub tool_batch_id: ToolBatchId,
    pub turn_id: TurnId,
    pub source_order_call_ids: Arc<[ToolCallId]>,
}
```

`ToolBatchId` is persisted by `ToolBatchOpened`/`ToolBatchClosed` and carried by every call event. Each call has its own `ToolCallId` and `EffectId`; retries keep them frozen.

## 15.1 Trait

```rust
pub type ToolEventStream = PortStream<Result<ToolStreamItem, ToolError>>;

pub trait Toolset: PortObject {
    fn descriptor(&self) -> ToolsetDescriptor;
    fn tools(&self) -> Arc<[ToolSpec]>;

    fn call(
        &self,
        ctx: ToolCallContext,
        call: ValidatedToolCall,
    ) -> PortFuture<Result<ToolEventStream, ToolError>>;

    fn reconcile(
        &self,
        _ctx: ReconcileContext,
        _effect: PendingToolEffect,
    ) -> PortFuture<Result<ReconcileResult, ToolError>> {
        Box::pin(async { Ok(ReconcileResult::Unknown) })
    }
}
```

## 15.2 Tool metadata

```rust
pub enum ApprovalRequirement {
    Policy,
    Required,
    NotRequired,
}

pub struct ApprovalMetadata {
    pub requirement: ApprovalRequirement,
    pub reason: Option<Arc<str>>,
    pub attributes: Metadata,
}

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

`model_name` is the name exposed to the LLM and must be unique in the resolved agent. `ToolId` remains stable across aliases. `ApprovalMetadata` is a bounded policy hint, not an authorization grant: `Required` cannot be bypassed, while `Policy` and `NotRequired` remain subject to stricter host or middleware policy.

## 15.3 Validation

A `ResolvedTool` contains:

- public `ToolSpec`;
- toolset dispatch handle;
- precompiled validator or binding adapter;
- optional output validator; and
- middleware routing metadata.

JSON Schema draft 2020-12 is the canonical schema representation. Registration normalizes each schema once and compiles the default Rust validator used by native and browser WASM paths. External references must resolve from an explicit offline registry or application-supplied retriever; validation cannot depend on ambient filesystem or network access. The initial portability subset is pinned by conformance fixtures and must intersect the strict structured-output subsets supported by reference providers.

The default Rust/WASM validator is the `jsonschema` crate (the 0.40 line at design freeze), configured explicitly for draft 2020-12. Validators are compiled once at registration and reused. Reference resolution uses a host-supplied offline retriever; default/minimal builds perform no implicit network retrieval. Python tools may retain a cached Pydantic validator, and Rust result decoders may use Serde, but binding-native validators must pass the same success/failure path and model-retry fixtures. The kernel receives only normalized validation success/failure and imports no validator. Replacing `jsonschema` as the baseline requires an ADR with native/WASM conformance, size, maintenance, and performance evidence rather than an undocumented fallback.

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
pub trait ContextProvider: PortObject {
    fn descriptor(&self) -> ContextProviderDescriptor;

    fn collect(
        &self,
        ctx: ContextCallContext,
        request: ContextRequest,
    ) -> PortFuture<Result<ContextContribution, ContextError>>;

    fn reconcile(
        &self,
        _ctx: ReconcileContext,
        _effect: PendingContextEffect,
    ) -> PortFuture<Result<ReconcileResult, ContextError>> {
        Box::pin(async { Ok(ReconcileResult::Unknown) })
    }
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

Every provider descriptor declares whether it is authorized to contribute trusted application instructions. That grant is made at agent resolution and included in the lock; without it, any returned `Instruction` is downgraded to quoted untrusted context and cannot acquire System/Developer authority. Retrieved/model-generated material is untrusted regardless of its embedded wording.

The assembled model-context order and authority precedence are fixed:

1. resolved system policy/instructions, in declared order;
2. resolved developer/application instructions;
3. active capability instructions in locked activation order;
4. trusted application reminders from authorized providers, sorted by locked provider order then item priority/source order;
5. all other external context as delimited quoted/reference material, sorted by locked provider order then item priority/source order;
6. canonical conversation history in parent-chain chronological order; and
7. the current user request exactly once as the final user-authority item.

Higher-authority groups cannot be overridden by lower groups. Stable source order breaks equal-priority ties; hash-map iteration never participates. The runtime may fan out providers concurrently but assembles only after all accepted outcomes are normalized and budgeted. Shared traces cover malicious retrieved “system” text, equal-priority ties, provider finish-order permutations, and capability activation.

Each context collection is an `EffectRequested(Context)` with locked component/version/configuration, chain digest/index, input digest, retry policy, and deadline committed before `collect` runs. Its normalized contribution or failure is committed before assembly. Recovery reuses completed output, reruns only `RecomputeSafe` calls with the same `EffectId`, invokes `reconcile` when declared, and suspends/fails explicitly for unresolved non-repeatable calls. The resume cursor is derived from committed chain positions, never process memory.

# 17. Middleware design

## 17.1 Single invocation interface

```rust
pub trait Middleware: PortObject {
    fn descriptor(&self) -> MiddlewareDescriptor;
    fn stages(&self) -> StageMask;

    fn invoke(
        &self,
        ctx: MiddlewareContext,
        input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>>;

    fn reconcile(
        &self,
        _ctx: ReconcileContext,
        _effect: PendingMiddlewareEffect,
    ) -> PortFuture<Result<ReconcileResult, MiddlewareError>> {
        Box::pin(async { Ok(ReconcileResult::Unknown) })
    }
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
    BeforeFinalize,
}
```

## 17.3 Outcomes

```rust
pub enum StageOutcome {
    Continue,
    Replace(StageValue),
    AddInstructions(Arc<[InstructionSpec]>),
    AddContext(Arc<[ContextItem]>),
    CompactContext(CompactionResult),
    RequestCompactionModel(CompactionModelRequest),
    FilterTools(Arc<[ToolId]>),
    RequestInteraction(InteractionSpec),
    Retry(RetryDirective),
    Suspend(SuspensionSpec),
    Complete(RunOutput),
    Fail(ErrorDescriptor),
}
```

The stage/outcome matrix is fixed (`yes` means permitted before any narrower descriptor-tier rule):

| Outcome | BeforeRun | PrepareContext | BeforeModel | AfterModel | BeforeToolBatch | AfterToolBatch | BeforeFinalize |
|---|---:|---:|---:|---:|---:|---:|---:|
| `Continue` / `Fail` / `Suspend` / `RequestInteraction` | yes | yes | yes | yes | yes | yes | yes |
| `Replace` | yes | yes | yes | yes | yes | yes | no |
| `AddInstructions` / `AddContext` | no | yes | yes | no | no | no | no |
| `FilterTools` | no | no | yes | no | yes | no | no |
| `CompactContext` / `RequestCompactionModel` | no | no | compactor only | no | no | no | no |
| `Retry` | no | no | no | yes | no | yes | yes |
| `Complete` | yes | no | no | yes | no | yes | yes |

An invalid combination returns stable `middleware_outcome_not_allowed` and is never coerced. Post-compaction validators are narrower: only `Continue`, `Fail`, `Suspend`, or `RequestInteraction`. `BeforeFinalize` follows the table and cannot replace the candidate result or mutate context.

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

The `BeforeModel` tiers are ordered as request/policy shaping, context mutation, context compaction, then post-compaction validation. A descriptor that declares the context-compactor role is unique per resolved agent. Resolution fails if more than one component declares it, or if ordering constraints place a context-mutating middleware after it. Post-compaction validators may continue, fail, suspend, or request interaction but cannot return `Replace`, `AddInstructions`, `AddContext`, or `CompactContext`.

## 17.5 Durable middleware output

Every middleware component invocation commits `EffectRequested(Middleware)` with locked component/version/configuration, stage, middleware-chain digest/index, input digest, retry policy, and deadline before `invoke` runs. The normalized result atomically commits `EffectCompleted` plus `StageOutcomeRecorded` before application. Recorded output reuse is the default. A descriptor may declare `RecomputeSafe` only when deterministic, side-effect-free, independent of unavailable external state, and covered by replay conformance tests; otherwise it declares reconciliation support or non-repeatable behavior. Recovery derives the next chain cursor from committed positions, reuses completed outcomes, calls `reconcile` where declared, reruns only recompute-safe invocations with the same `EffectId`, and suspends/fails unresolved non-repeatable work.

`BeforeFinalize` receives a candidate terminal result before `RunCompleted` or another terminal record is committed. It may continue, request bounded retry/continuation/interaction, suspend, or fail. Terminal notification is observer-only and cannot change state.

## 17.6 Compaction middleware contract

Compaction is a `BeforeModel` middleware specialization. Its `MiddlewareDescriptor` declares the unique context-compactor role and a strategy/configuration identity; this is ordering/validation metadata, not a seventh port or new stage. The runtime supplies the fully assembled candidate `ModelRequest`, the provider/model input budget, reserved output budget, protected item set, token-estimation metadata, and the latest compatible checkpoint for that middleware component when one exists.

```rust
pub struct CompactionResult {
    pub replacement_messages: Arc<[Message]>,
    pub derived_summaries: Arc<[ContextItem]>,
    pub evidence: CompactionEvidence,
    pub checkpoint: Option<CompactionCheckpoint>,
}

pub struct CompactionModelRequest {
    pub model: ComponentRef,
    pub request: ModelRequestDraft,
    pub budget_scope_id: BudgetScopeId,
    pub source_sensitivity: Sensitivity,
    pub residency_policy_digest: Digest,
    pub resume_state: RawJson,
}

pub struct CompactionEvidence {
    pub strategy_id: Arc<str>,
    pub strategy_version: u32,
    pub configuration_digest: Digest,
    pub model_context_profile_digest: Digest,
    pub source_digest: Digest,
    pub protected_item_set_digest: Digest,
    pub covered_entry_ids: Arc<[EntryId]>,
    pub retained_entry_ids: Arc<[EntryId]>,
    pub projection_digest: Digest,
    pub estimated_tokens_before: u64,
    pub estimated_tokens_after: u64,
    pub summary_digest: Option<Digest>,
    pub cache_impact: PromptCacheImpact,
}

pub struct CompactionCheckpoint {
    pub component_id: ComponentId,
    pub strategy_id: Arc<str>,
    pub strategy_version: u32,
    pub configuration_digest: Digest,
    pub model_context_profile_digest: Digest,
    pub covered_through_entry_id: EntryId,
    pub source_digest: Digest,
    pub summary: CompactedSummary,
    pub summary_digest: Digest,
    pub sensitivity: Sensitivity,
}

pub enum CompactedSummary {
    Inline(Arc<[ContextItem]>),
    Artifact(ArtifactRef),
}

pub enum PromptCacheImpact {
    StablePrefixPreserved,
    MutableSuffixChanged,
    CacheInvalidated,
}
```

The replacement is a model-visible projection only. Canonical `ConversationEntry` values, parent links, tool calls/results, and journal records are never edited or deleted. The runtime rejects a compaction result that:

- omits protected system/developer/capability instructions, the active user request, required policy context, or pinned items;
- splits a tool call from its result or produces invalid role/source ordering;
- exceeds the hard model-input budget after reserved output and provider overhead are applied;
- lacks consistent source, retained-entry, strategy, configuration, or digest evidence; or
- lowers sensitivity/provenance classification without an explicit permitted transformation.

Every protected System/Developer/capability message must be present with the same ID and canonical bytes; a compactor cannot paraphrase or regenerate it. Synthesized summaries are `ContextItem::DerivedSummary` values with covered-entry IDs, provenance, sensitivity, and a derived/untrusted authority class. They cannot create System/Developer/User messages or otherwise elevate authority, even if their text contains instruction-like language.

The ordinary `StageOutcomeRecorded` path persists behavior-changing evidence and the exact normalized replacement projection either inline or through a required durable `ArtifactRef` staged before the outcome commit. The optional reusable checkpoint may be inline, a durable artifact, or an explicitly disposable cache reference. The main `EffectRequested(Model)` record contains the final request actually sent. On replay, recorded output is reused; a missing/corrupt required projection is an integrity failure, never a reason to rerun summarization. On a later turn, a checkpoint is accepted only when its component, strategy/version, configuration digest, model-context profile, covered-history digest, and sensitivity policy match; otherwise it is ignored and compaction rebuilds from canonical history.

Deterministic windowing can complete inside the middleware invocation. Model-assisted middleware must return `RequestCompactionModel`; it cannot call a provider directly. The runtime then:

1. validates that the secondary model is authorized for the full source sensitivity, tenant, residency, and egress scope;
2. allocates a child `ModelRequestId`/`EffectId` related to the committed middleware effect with `EffectPurpose::CompactionSummary`;
3. commits that `EffectRequested(Model)` before dispatch;
4. settles or reconciles the child through the ordinary model-effect lifecycle and records its usage against the explicit budget scope; and
5. resumes the same middleware invocation identity with the normalized child result and opaque bounded `resume_state`, then commits the final parent outcome.

A crash at any step reuses/reconciles the child effect and resume cursor; no provider call is unrecorded. The child inherits cancellation/deadline/principal context. It must not recursively invoke the same agent or compaction chain, and maximum compaction-effect depth is one.

Threshold and hysteresis settings prevent compaction on every turn. Prompt-cache-aware strategies preserve the stable instruction prefix and compact only eligible mutable history. If compaction fails or cannot meet the hard budget without protected-content loss, the normalized outcome is a stable context-budget failure unless the application configured a tested deterministic fallback.

# 18. JournalStore design

```rust
pub trait JournalStore: PortObject {
    fn append(
        &self,
        request: AppendRequest,
    ) -> PortFuture<Result<CommittedBatch, StoreError>>;

    fn load(
        &self,
        request: LoadRequest,
    ) -> PortFuture<Result<LoadedSession, StoreError>>;

    fn write_snapshot(
        &self,
        request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>>;

    fn health(&self) -> PortFuture<Result<Health, StoreError>>;
}
```

## 18.1 Store contract

- `append` is atomic per session.
- `batch_id`, record IDs, semantic timestamps, payloads, and their digest are allocated/frozen before the first append attempt and reused byte-for-byte on retry.
- durable-derived event IDs are allocated/frozen with their source record; replay reuses them by documented event ordinal, while transient progress events receive runtime UUIDv7 IDs and have only transient-order guarantees.
- A store checks for an existing `batch_id` before applying the `expected_sequence` precondition: an exact request digest returns the original committed receipt, while reuse with different content is corruption.
- Only a previously unseen batch evaluates `expected_sequence` for optimistic concurrency; a mismatch returns `StoreConflict` without appending.
- duplicate `record_id` appends are idempotent only if payloads match exactly;
- mismatched duplicates are corruption/conflict;
- stores return committed envelopes with assigned sequence;
- loads validate checksums and format versions; and
- after an ambiguous transport/store acknowledgement, the runtime retries the same frozen batch and accepts only the original receipt or an explicit conflict/corruption result; and
- store-specific retries occur below the semantic runtime only when they preserve this batch identity contract.

## 18.2 SQLite design direction

The first durable store uses tables similar to:

```text
sessions(session_id, current_sequence, snapshot_sequence, metadata)
records(session_id, sequence, record_id, lane_id, run_id, kind, version, payload_cbor, timestamp)
snapshots(session_id, sequence, payload_cbor, digest, timestamp)
```

The actual schema also stores batch identity, format/kind versions, semantic timestamp, optional commit timestamp, payload digest, previous/head checksum, envelope checksum, and derived event IDs. Append uses one transaction and a compare/update on `current_sequence` and current head checksum.

The supported durable mode uses WAL plus `synchronous=FULL` (and platform full-fsync controls where SQLite/OS expose them). An append acknowledgement is returned only after the committing SQLite transaction reports success under that mode. Startup verifies the WAL/database and checksum head before serving the session. `synchronous=NORMAL/OFF`, in-memory filesystems, or storage that does not honor flush ordering may be exposed only as explicitly named relaxed/non-durable modes; they cannot satisfy NFR-REL-001 and must surface that fact in configuration/health metadata. Tests distinguish process-kill recovery from power-loss/storage-fault expectations and document unavoidable hardware/filesystem assumptions.

## 18.3 Snapshot representation

The initial snapshot is a direct, versioned CBOR projection of kernel/session state plus its journal sequence and digest. It is a disposable replay cache rather than a second semantic model. A replay-optimized compact format requires new benchmark evidence and an ADR; corrupt, unknown, or stale snapshots are discarded and rebuilt from records.

# 19. Observer design

```rust
pub trait Observer: PortObject {
    fn descriptor(&self) -> ObserverDescriptor;

    fn observe(
        &self,
        batch: Arc<[RunEvent]>,
    ) -> PortFuture<Result<(), ObserverError>>;
}
```

Observer delivery configuration is operational only:

```rust
pub enum ObserverBackpressure {
    BlockBounded { timeout: Duration },
    DropProgress,
    Disconnect,
}
```

Observer failures produce diagnostics but never change a run decision, prevent an effect, or alter a terminal result. Unbounded/spill-to-disk observer queues are not a v1 mode; an observer that needs durable export owns a separately bounded external pipeline. Correctness-critical audit facts are journal records. The ingress `SecurityAuditSink` used for rejected unauthenticated/unknown external commands is a separate runtime/server security service, not an `Observer`; it may fail that command closed because no run transition has been accepted.

# 20. Runtime event model

## 20.1 Event envelope

```rust
pub struct RunEvent {
    pub schema_version: u16,
    pub kind_version: u16,
    pub event_id: EventId,
    pub kind: RunEventKind,
    pub session_id: SessionId,
    pub lane_id: LaneId,
    pub run_id: RunId,
    pub turn_id: Option<TurnId>,
    pub model_request_id: Option<ModelRequestId>,
    pub tool_batch_id: Option<ToolBatchId>,
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

Durable-derived events include accepted, message finalized, tool settled, interaction requested/resolved (including the approval profile), limit reached, and terminal results.

For each durable record kind, a versioned table defines zero or more derived event ordinals. Their UUIDv7 `event_id` values are allocated in the record draft and persisted in `derived_event_ids`, so replay and every binding reproduce the same IDs. Transient event IDs are allocated at emission and are not replay-stable.

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

## 22.1 Limits, usage, and cost

```rust
pub struct RunLimits {
    pub max_model_requests: Option<u64>,
    pub max_turns: Option<u64>,
    pub max_tool_calls: Option<u64>,
    pub max_parallel_tools: Option<u32>,
    pub max_input_tokens: Option<u64>,
    pub max_output_tokens: Option<u64>,
    pub max_context_bytes: Option<u64>,
    pub max_output_bytes: Option<u64>,
    pub max_retries: Option<u32>,
    pub max_wall_time: Option<Duration>,
    pub max_cost: Option<CostLimit>,
    pub extension_counters: BTreeMap<LimitKey, u64>,
}

pub struct CostLimit {
    pub unit: Arc<str>,
    pub micros: u64,
    pub pricing_policy_version: Arc<str>,
    pub unknown_usage: UnknownUsagePolicy,
}

pub enum UnknownUsagePolicy {
    FailClosed,
    SuspendForDecision,
    AllowWithinReservedMaximum,
}
```

Cost uses non-negative `u64` integer millionths of an ISO-4217 currency or a namespaced application credit unit; floating-point money is prohibited. The v1 maximum is `u64::MAX` and all aggregation is checked. If an aggregate overflows, the kernel commits terminal `RunFailed` with `ErrorDescriptor.code = cost_overflow`, `category = limit`, and `retryable = false`; it never wraps, emits `LimitReached`, or fabricates a `LimitValue::Cost.observed` value. JSON/JSONL and JavaScript-facing schemas encode `micros` as a canonical decimal string; Python may expose an exact integer. The pricing-policy version and provider/model price inputs used for each charge are recorded with normalized usage. Counter increments are monotonic checked integers; overflow is a `counter_overflow` failure, never wraparound. Extension counter keys are namespaced, registered with their maxima during agent resolution, capped at 32 keys per resolved agent in v1, and incremented only through normalized kernel input. `EffectCompleted` usage drives aggregation; crossing a representable hard bound commits `LimitReached` before terminalization or suspension according to the declared unknown-usage policy.

## 22.2 Cancellation token tree

```text
Agent shutdown token
  Session token
    Lane/run token
      Model effect token
      Tool effect tokens
      Context/middleware tokens
```

Cancelling a parent signals descendants. Completion after cancellation is ignored or reconciled according to effect state.

## 22.3 Durable cancellation

A durable lane writes `CancellationRequested` before final reconciliation. The runtime signals active effects, waits up to policy deadline, records settled/cancelled outcomes, synthesizes required tool results, and writes `RunCancelled`.

## 22.4 Consumer cancellation

Dropping a Python/JS run object does not automatically cancel a durable run. The public API provides explicit `cancel()`. Non-durable convenience calls may opt into cancel-on-drop.

## 22.5 Terminal race precedence

The lane's committed journal order, not task wake order, decides races. Optimistic append permits only one next batch; losers reload and reclassify their input.

| Earlier committed condition | Later input | Required result |
|---|---|---|
| `RunCompleted`, `RunFailed`, or `RunCancelled` | any state-changing input | Terminal record wins; reject/audit the late input without changing state. |
| `CancellationRequested` | ordinary model/tool/context/middleware completion | Enter/remain `Cancelling`; use completion only as reconciliation evidence and do not resume ordinary progress. |
| `CancellationRequested` | non-repeatable/unknown external outcome | Preserve explicit uncertainty in `Suspended`; do not fabricate successful cancellation until reconciled/operator-resolved. |
| `LimitReached` | ordinary completion/retry | Limit outcome wins; no further effect dispatch. |
| Effect completion and terminal result committed | later cancellation/limit observation | Existing completion/terminal result wins; later request is idempotent/no-op with diagnostic. |
| Same normalized decision observes multiple uncommitted causes | invariant/security contract failure, hard limit, explicit cancellation, ordinary completion/retry | Apply the first applicable cause in that order. |

Once cancellation controls a tool batch, the runtime reconciles active calls and commits exactly one real result or framework-authored `Cancelled` closure for every accepted call before `RunCancelled`, preserving source order and marking the closure as synthetic with no tool output/success claim. Fabricating a successful/tool-produced result is prohibited. A late equivalent external command remains idempotent under the settlement index; a conflicting one follows the durable rejection rules.

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

`StillRunning` must be backed by a committed `EffectDeferred` record before the runtime waits, polls, or accepts a callback. External completion is checked against the outstanding effect, cancellation/deadline state, completion identity, expected schema, and principal policy before it becomes kernel input.

## 23.3 Settlement identity retention

`EffectCompleted`/`EffectFailed`/`EffectCancelled` and `InteractionResolved`/terminal interaction records retain the accepted command identity and normalized payload digest. Kernel snapshots include a compact settlement index keyed by `(OperationLocator, target ID)` so duplicate classification survives restart without rescanning the full journal. The index is derived from records and must match full replay.

An outstanding callback token and its settlement tombstone cannot be pruned. After terminal settlement, the application-configured idempotency horizon must be at least as long as callback-token validity and the advertised recovery/audit retention; journal pruning retains a versioned settlement tombstone through that horizon. After the horizon, ingress tokens are expired/revoked and a later command is rejected/audited as `expired_locator` without claiming historical idempotency. Snapshot, pruning, export, and migration fixtures preserve this rule.

## 23.4 Crash matrix

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

The data model includes lanes from the start. MVP APIs expose `main`; experimental fork/create primitives may exist without a concurrency promise. The first durable store and main-lane crash-prefix matrix ship before public concurrent multi-lane APIs. This avoids a future journal migration from linear histories while ensuring lane ownership is validated against SQLite sequence conflicts before PR-047.

# 25. Python binding design

## 25.1 Toolchain

- PyO3 for native classes and conversion.
- Maturin for builds and wheels.
- `pyo3-async-runtimes` as the baseline bridge for Python awaitables and the native Tokio runtime.
- CPython 3.11-3.14 per-version wheels for the initial GIL-enabled matrix.
- A version-specific CPython 3.14t wheel where the target platform supports free-threading.
- No classic `abi3` launch wheel; evaluate Python 3.15+ `abi3t` or combined stable-ABI wheels only after production CI, compatibility, and performance gates pass.

The required launch platforms are manylinux x86_64/aarch64, macOS arm64, and Windows x64. Module initialization must explicitly declare and test free-threaded safety; long Rust work detaches from the interpreter and shared Python-facing state does not rely on the GIL for synchronization.

The bridge dependency does not itself establish CPython 3.14t safety; the project's callback, cancellation, handle, shutdown, and concurrent-access tests own that evidence. Replacing the baseline bridge requires an ADR/dependency review with supported-Python, cancellation, runtime-ownership, free-threaded, maintenance, and performance evidence.

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

## 25.8 Distribution composition

One `finstack-ai` wheel contains the binding and curated Rust-backed OpenAI-compatible, Anthropic, and local providers. Their Rust crates remain independently packaged leaf crates but follow the lockstep workspace version through pre-1.0; version decoupling after 1.0 requires a published compatibility range and conformance evidence. They are linked into the same extension module to avoid relying on an unstable Rust ABI across separate wheels. Provider submodules import lazily, Pydantic remains an extra, and wheel size is a release budget.

## 25.9 Exception hierarchy

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

## 26.4 Worker topology

The package includes a worker helper that hosts one or more agents in a Web Worker. Main-thread communication uses transferable `ArrayBuffer` batches where possible.

## 26.5 WASM data conversion

- IDs are strings.
- Dynamic JSON is transferred as strings or `Uint8Array`, selected by API.
- Large content uses blob handles.
- Events are arrays of compact objects per batch.
- Internal state never round-trips through JavaScript for each transition.

## 26.6 Optional OpenAI-compatible adapter

The npm package exports a tree-shakeable `@finstack/ai/adapters/openai-compatible` module that implements the model host ABI using browser `fetch` and SSE. Its default configuration targets a same-origin proxy URL. It owns no kernel semantics, includes `AbortSignal` propagation and CORS diagnostics, and documents that provider credentials must not be embedded in shipped browser code.

# 27. WIT plugin design

## 27.1 Package structure

```text
wit/v0.0.4/finstack-ai-types/types.wit
wit/v0.0.4/finstack-ai-host/host.wit
wit/v0.0.4/finstack-ai-toolset/toolset.wit
wit/v0.0.4/finstack-ai-context/context.wit
```

Plugin alpha publishes experimental `@0.0.4` packages in lockstep with the workspace release. Later breaking pre-1.0 changes increment the appropriate package/world version and require adapters/fixtures. `@1.0.0` is created only at the framework 1.0 compatibility gate; the host may support multiple majors through explicit adapters.

Each listed subdirectory is a separate single-package WIT root. Binding generation builds `ai-types` first, then `ai-host`, then the guest packages; a pinned dependency-fetch/build tool materializes the standard `deps/` view for each dependent root and a checked lock records exact package versions and digests. CI rebuilds from clean roots, rejects undeclared/mismatched dependencies, and verifies generated guest/host bindings. A directory never contains two `package` declarations.

## 27.2 Common types

```wit
package finstack:ai-types@0.0.4;

interface types {
    record call-context {
        effect-id: string,
        session-id: string,
        lane-id: string,
        run-id: string,
        tenant-scope: string,
        principal-issuer: string,
        principal-subject: string,
        authorization-decision-id: string,
        permitted-scopes: list<string>,
        budget-scope-id: option<string>,
        deadline-unix-ms: option<s64>,
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

## 27.3 Host capability interfaces

The host package is a versioned dependency of the guest packages, not an ambient WASI grant:

```wit
package finstack:ai-host@0.0.4;

interface logging {
    use finstack:ai-types/types@0.0.4.{call-context, plugin-error};

    enum level { trace, debug, info, warn, error }
    log: func(context: call-context, level: level, message: string)
        -> result<_, plugin-error>;
}

interface blobs {
    use finstack:ai-types/types@0.0.4.{call-context, blob-ref, plugin-error};

    read: func(context: call-context, reference: blob-ref, offset: u64, max-bytes: u64)
        -> result<list<u8>, plugin-error>;
}
```

The host authorizes every blob read against the sanitized call context and configured per-call byte ceiling. Linking either interface grants only those functions; it does not grant ambient filesystem, network, environment, clock, or secret access.

## 27.4 Toolset world

```wit
package finstack:ai-toolset@0.0.4;

world toolset-plugin {
    import finstack:ai-host/logging@0.0.4;
    import finstack:ai-host/blobs@0.0.4;

    export toolset;
}

interface toolset {
    use finstack:ai-types/types@0.0.4.{call-context, plugin-error};

    record tool-spec {
        id: string,
        model-name: string,
        title: string,
        description: string,
        input-schema-json: list<u8>,
        output-schema-json: option<list<u8>>,
        execution-mode: string,
        side-effect: string,
        retry-safety: string,
        approval-policy-json: list<u8>,
        max-result-bytes: u64,
        metadata-json: list<u8>,
    }

    record tool-catalog {
        digest: string,
        tools: list<tool-spec>,
    }

    record tool-result {
        content-json: list<u8>,
        is-error: bool,
    }

    list-tools: func() -> result<tool-catalog, plugin-error>;
    call: func(context: call-context, tool-id: string, args-json: list<u8>)
        -> result<tool-result, plugin-error>;
}
```

The host validates the immutable catalog digest, schemas, execution mode, approval metadata, and output ceiling at registration. It enforces the minimum of host/run/tool limits and performs authorization/interaction policy before invoking the guest; plugin metadata never grants permission. `call-context` carries only a sanitized identity/scope projection and no credential/full claims.

The 0.x/initial 1.0 tool world is coarse and returns exactly one result/error. It has no progress batch or resumable resource stream. Resource-based streaming is deferred until host/guest async support, cancellation, resource cleanup, and canonical-ABI overhead are validated without changing kernel semantics.

## 27.5 Context world

```wit
package finstack:ai-context@0.0.4;

world context-plugin {
    import finstack:ai-host/logging@0.0.4;
    import finstack:ai-host/blobs@0.0.4;

    export context-provider;
}

interface context-provider {
    use finstack:ai-types/types@0.0.4.{call-context, blob-ref, plugin-error};

    record context-budget {
        max-tokens: u64,
        max-bytes: u64,
        max-items: u32,
    }

    record context-query {
        context: call-context,
        request-json: list<u8>,
        budget: context-budget,
    }

    record context-item {
        item-json: list<u8>,
        blobs: list<blob-ref>,
        estimated-tokens: u64,
        bytes: u64,
    }

    collect: func(query: context-query)
        -> result<list<context-item>, plugin-error>;
}
```

The host validates each item against the native context schema, provenance/authority rules, and the stricter of provider/run/plugin budgets. The plugin cannot mutate session history, elevate retrieved content to trusted instructions, or use a private suspension protocol.

## 27.6 Host limits

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

## 27.7 Component cache

The host caches compiled components by digest and runtime version. Stateful plugin instances are pooled only if the world contract declares safe reuse.

# 28. Protocol and encoding

## 28.1 Canonical journal encoding

Use `ciborium` behind a project-owned codec wrapper for the strict, versioned deterministic CBOR profile used by durable, remote, and process envelopes. The project wrapper validates the value tree, recursively orders every map using RFC 8949 deterministic key ordering (using `ciborium::value::CanonicalValue` or equivalent comparison), selects permitted canonical numeric representations, and only then calls the library encoder. The design does not assume that `ciborium::ser::into_writer` is an end-to-end canonical serializer. The frozen profile requires:

- definite-length items;
- shortest-form integer and length encodings;
- integers representable directly by CBOR major types 0 and 1 only; bignum tags 2 and 3 are rejected in v1;
- deterministic map-key ordering;
- finite floats only, encoded in the shortest IEEE-754 width that preserves the value exactly; negative zero is preserved as distinct from positive zero; non-finite values are rejected;
- no duplicate map keys; and
- schema-level size/depth limits before allocation.

Reasons:

- compact binary representation;
- natural byte-string support;
- broad language support;
- deterministic framing; and
- easier evolution than a Rust-specific encoding.

ADR-015 freezes the profile in PR-004. PR-039 implements it and publishes binary compatibility fixtures, including nested maps built in different insertion orders, integer/float width boundaries through `u64::MAX`, negative zero, and rejection of bignum tags, duplicate keys, and non-finite values.

## 28.2 Frame

Remote and external-process streams use the same generic frame/envelope/handshake layer:

```text
4-byte unsigned big-endian payload length
CBOR envelope payload
```

Maximum frame size is configured and checked before allocation.

The envelope identifies its payload schema family. Remote session DTOs and plugin/process messages remain distinct enums with separate versions and conformance fixtures; sharing framing does not merge their semantics.

Before authentication, the remote server accepts only bounded `ClientHello`, `ServerHello`, `Authenticate`, `AuthResult`, and `Close` messages. The pre-auth frame ceiling is 16 KiB, compression is disabled, strings/collections/depth are separately bounded, parsing/auth have deadlines, and attempts are capped; the 4-byte length is treated as untrusted and rejected before allocation when over the ceiling. No session ID, command, snapshot, or target lookup is processed pre-auth.

The reference server listens on a Unix socket or loopback TCP by default. Non-loopback TCP requires explicit enablement plus TLS 1.3 or later and configured authentication; bearer credentials are never accepted over plaintext. Hello negotiation selects the highest mutually supported version at or above both peers' configured minimum/downgrade floor. Unknown payload families, missing mandatory security features, and downgrade below the floor close the connection without fallback.

After authentication, every state-changing command carries a UUIDv7 command ID, authenticated tenant scope, explicit session/lane/run locator where applicable, expected durable cursor when relevant, and normalized digest. Equal command ID/digest retries return the original response through the configured idempotency horizon; conflicting reuse fails closed and is audited. Authorization is rechecked on every session/target operation.

Reconnect ordering is:

```text
authenticate
  -> open-session(last-known-durable-sequence)
  -> authoritative snapshot at sequence S (or explicit no-snapshot)
  -> verified durable tail S+1..B
  -> sync barrier B
  -> live durable/transient event batches after B
```

No live event may overtake the barrier. Transient progress before reconnect is not replay-guaranteed; the snapshot/tail and terminal status are authoritative. Client/server flow control uses bounded byte/item credit windows and acknowledged batch cursors. A slow client pauses within deadlines and then disconnects/resumes from its durable cursor; durable completion is never silently dropped. Per-connection/session queues, outstanding commands, snapshot size, and tail batch size are bounded.

## 28.3 Diagnostic JSON

Every envelope and record has a lossless diagnostic JSON projection. JSONL export supports debugging, migrations, and support cases. A typed `micros` value is always an unsigned base-10 string with no leading zero except `"0"`; decoders reject JSON numbers, signs, overflow, and non-canonical leading zeros.

## 28.4 Schema evolution

| Schema family | Unknown-field behavior | Evolution rule |
|---|---|---|
| `AgentSpec`, `BundleSpec`, locks, component config | Reject by default; extension-owned config may allow only fields its own versioned schema declares. | Add fields with schema-version/default semantics; rename/remove requires migration/version bump. |
| Durable record envelope/body/snapshot | Envelope unknowns and body fields are accepted only when the exact schema version marks them semantically ignorable; preserve canonical opaque bytes/value for re-export. Unknown state-bearing fields/kinds are fatal before apply. | Envelope has format version; body has kind version; semantic addition/rename/removal requires a new kind/version and migration fixture. |
| Inbound remote/process commands and authentication messages | Reject unknown fields and unknown variants before state lookup. | Explicit protocol/command version negotiation; no permissive command decoding. |
| Outbound remote events/responses | Accept only schema-declared ignorable optionals; retain opaque values where proxy/re-export is supported. | Additive optionals within negotiated version; semantic changes require new version. |
| WIT records/worlds | Structural interface is exact; unknown fields do not exist in a compiled world. | Breaking/additive interface changes follow the package/world version policy and adapter matrix. |
| `Metadata` objects | Preserve every unknown member during round-trip; consumers may ignore members they do not own, but metadata never grants authority. | Add opaque members within the enclosing field contract; a behavior-changing member requires an enclosing schema/version change and fixtures. |
| Diagnostic metadata / JSONL projections | Retain when round-tripping; consumers may ignore documented diagnostic keys. | Must not influence replay semantics. |
| Golden trace/compatibility fixtures | Reject unknown fields. | Fixture schema version changes explicitly with a converter or regenerated reviewed corpus. |

No decoder silently discards an unknown value that could affect state, authorization, idempotency, ordering, or recovery.

## 28.5 Redaction boundary

Authoritative journal records retain or securely reference the complete state needed for replay. Metadata-only/redacted modes apply to observers, diagnostic JSON/JSONL, support bundles, and exports. Version 1 has no field-level mutation, redaction, or tombstone operation over authoritative records. A deployment may destroy an entire session/store unit only after outstanding effects/interactions and callback credentials are closed or expired; destruction permanently removes replay/resume and must be reported as such. Selective authoritative redaction requires a future ADR, new versioned record/schema and migration rules, and updated security/privacy evidence. Model-context compaction is never deletion.

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
  "capabilities": [
    { "id": "research", "version": "^1" }
  ],
  "limits": { "max_turns": 12, "max_tool_calls": 50 },
  "extension_config": {
    "finstack.model.openai-compatible/default": {
      "model": "example-model",
      "base_url": "https://example.invalid/v1"
    }
  }
}
```

Secrets are references, not literal values in supported configurations.

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
    pub descriptor: ErrorDescriptor,
    pub source: Option<Arc<dyn Error + Send + Sync>>,
}

pub struct ErrorDescriptor {
    pub code: ErrorCode,
    pub message: Arc<str>,
    pub category: ErrorCategory,
    pub retryable: bool,
    pub identifiers: ErrorIdentifiers,
    pub safe_details: Metadata,
}
```

Ports may construct runtime `FrameworkError` values with a source chain for local diagnostics. Before any kernel input, record, event, retry decision, digest, binding, WIT, or remote boundary, the runtime normalizes it to the source-free, serializable `ErrorDescriptor`. Source objects/text are diagnostic-only, never hashed or persisted, and are exposed only through an explicit local debug policy with redaction. Durable/public equality and compatibility use the descriptor alone.

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

This section implements the control obligations in Security and Threat Model v0.4. Control ownership, tests, and residual risks must remain traceable to that register as the implementation evolves.

## 31.1 Native code

Native Rust and Python components are trusted. The SDK documentation must state this plainly.

## 31.2 Tool authorization

Tool implementations enforce resource boundaries. Middleware can require a typed interaction—using the approval profile where appropriate—or deny calls. The runtime includes call identity and principal metadata in `ToolCallContext`.

## 31.3 Principal context

```rust
pub struct AuthorizationContext {
    pub principal: PrincipalRef,
    pub authentication_method: Arc<str>,
    pub assurance_level: Arc<str>,
    pub roles: Arc<[Arc<str>]>,
    pub permitted_scopes: Arc<[Arc<str>]>,
    pub safe_claims: Metadata,
    pub policy_version: Arc<str>,
    pub decision_id: Arc<str>,
}
```

Ingress constructs `AuthorizationContext` only after authentication/authorization. `PrincipalRef` is the durable audit-safe subset stored in records; the full context is propagated explicitly to runtime services and coarse port-call contexts, never through task-local globals. Tools receive the principal, tenant, permitted resource scopes, and policy decision reference required for enforcement. Interactions and external completions persist `PrincipalRef` plus the decision reference. Child runs persist an attenuated `RunSecurityContext`. WIT receives a sanitized principal/tenant/scope projection with no bearer credential or unrestricted claims.

```rust
pub struct RunCallContext {
    pub locator: OperationLocator,
    pub authorization: AuthorizationContext,
    pub effect_id: EffectId,
    pub attempt: u32,
    pub deadline: Option<Timestamp>,
    pub budget_scope_id: Option<BudgetScopeId>,
    pub cancellation: CancellationSignal,
}

pub struct ModelCallContext {
    pub run: RunCallContext,
    pub request_id: ModelRequestId,
}

pub struct ToolCallContext {
    pub run: RunCallContext,
    pub tool_batch_id: ToolBatchId,
    pub tool_call_id: ToolCallId,
}

pub struct ContextCallContext {
    pub run: RunCallContext,
    pub pipeline: PipelinePosition,
}

pub struct MiddlewareContext {
    pub run: RunCallContext,
    pub pipeline: PipelinePosition,
}

pub struct ReconcileContext {
    pub run: RunCallContext,
    pub original_input_digest: Digest,
}
```

These immutable contexts are the only source of runtime identity, authorization, deadline, budget, cancellation, attempt, and idempotency data at port boundaries. Native adapters receive the full authorized projection; WIT receives the sanitized subset defined in section 27; remote/process adapters bind it to authenticated locators. Reconciliation reuses the original identifiers/evidence.

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
- generalized interaction suspension, with approval as the first profile;
- crash-prefix matrix;
- initial lane APIs.

## Milestone 7: Isolated plugins

- initial experimental WIT toolset/context packages;
- Wasmtime host;
- permission and resource limits;
- reference components;
- plugin conformance tests.

## Milestone 8: Ecosystem readiness

- additional providers/toolsets/stores;
- deterministic and model-assisted `BeforeModel` compaction middleware batteries;
- remote protocol and reference server;
- observer adapters;
- workflow integrations;
- stable compatibility policy and migration tooling.

# 36. Definition of done for core technical design

The core implementation is technically ready for public preview when:

1. `finstack-ai-kernel` has no forbidden dependency.
2. Every kernel transition has a documented input, record output, and invariant test.
3. The native runtime never executes a recoverable effect or begins an external wait before its request/deferral record is committed.
4. All queues are bounded and tested with slow consumers.
5. Parallel tools finalize history in source order.
6. Cancellation and crash-prefix tests produce valid restored states.
7. Rust, Python Rust-backed, and browser WASM pass common trace fixtures.
8. Benchmarks isolate framework overhead from model/network latency.
9. Adding a fixture provider/toolset/store requires no kernel changes.
10. Public errors, records, events, and specs have versioned schemas.
11. Run lineage, generic deferral, and interactions survive crash-prefix restoration across supported bindings.
12. `before_finalize` is the last behavior-changing stage and no terminal observer can mutate execution.
13. `BeforeModel` compaction preserves canonical history/protected content, produces binding-identical valid requests, and safely rebuilds invalid derived checkpoints.

# 37. Technical decision status

All pre-implementation technical decisions are resolved. Changes to these directions require the cited ADR and evidence gate rather than silently reopening the option during implementation.

| ADR | Decision | Reconsideration gate |
|---|---|---|
| ADR-029 | Allocated runtime entity IDs use typed lowercase UUID strings and UUIDv7; human-selected configuration/package keys use validated namespaced strings. | Only before PR-006 schema fixtures freeze. |
| ADR-030 | Object-safe boxed futures/streams for public extension traits; concrete internal fast paths remain allowed. | Phase 3 benchmarks must show material dispatch cost before changing the public ABI. |
| ADR-031 | Single-threaded browser WASM with Web Workers by default; no SharedArrayBuffer requirement. | Post-preview opt-in only, after cross-origin isolation and conformance evidence. |
| ADR-032 | Direct versioned kernel-state CBOR snapshots; snapshots remain disposable derived caches. | PR-041 benchmarks may propose a new ADR for a compact projection. |
| ADR-033 | MVP interruption uses retry, suspension, or explicit uncertainty; the Model trait reserves optional reconciliation implemented in Phase 6. | PR-042 provider evidence determines per-provider support, not the port shape. |
| ADR-034 | Record state-changing middleware outcomes by default; recompute only when explicitly declared safe and fixture-proven. | PR-018 descriptor/conformance review. |
| ADR-035 | WIT plugin alpha uses experimental @0.x tool/context packages and one coarse completion; @1.0.0 worlds freeze only at the framework `1.0.0` gate, with resource streaming deferred. | Post-plugin-alpha evidence on async support, cleanup, overhead, and compatibility. |
| ADR-036 | Blob storage is application/runtime-owned through 1.0; no seventh kernel port. | Post-1.0 usage evidence and a new architecture ADR. |
| ADR-037 | Model-context compaction uses `BeforeModel` middleware, never mutates canonical history, and stores versioned outcomes/checkpoints as derived data. | Before public preview only if PR-018/PR-056 conformance evidence proves the existing stage/outcome contract cannot preserve required semantics. |

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
