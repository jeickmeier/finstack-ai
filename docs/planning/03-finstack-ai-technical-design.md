---
title: "finstack-ai Technical Design Document"
subtitle: "Implementation-level design for the Rust agent microkernel, runtime, bindings, and extension SDK"
author: "finstack-ai project"
date: "2026-08-09"
---

# finstack-ai Technical Design Document

# Document control

| Field | Value |
|---|---|
| Product | `finstack-ai` |
| Document | Technical Design Document (TDD) |
| Version | 0.15 |
| Status | Implementation baseline |
| Primary language | Rust |
| Bindings | Python/PyO3; JavaScript/WebAssembly; optional WIT Component Model |
| Related documents | Engineering Standards v0.5; Product Requirements Document v0.7; Architecture Specification v0.10; Implementation Plan v0.15; Security and Threat Model v0.5 |

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

```rust
pub struct AllocatedIds {
    pub record_ids: Vec<RecordId>,
    pub event_ids: Vec<EventId>,
    pub effect_ids: Vec<EffectId>,
    pub interaction_ids: Vec<InteractionId>,
    pub message_ids: Vec<MessageId>,
    pub turn_ids: Vec<TurnId>,
    pub model_request_ids: Vec<ModelRequestId>,
    pub tool_batch_ids: Vec<ToolBatchId>,
    pub tool_call_ids: Vec<ToolCallId>,
    pub append_batch_ids: Vec<AppendBatchId>,
    pub cancellation_request_ids: Vec<CancellationRequestId>,
}
```

`AllocatedIds` is a runtime-owned bag of preallocated UUIDv7 values consumed in documented order by `decide`. Empty vectors are valid when a transition allocates no IDs of that kind. Fixtures supply exact vectors; the kernel never generates UUIDv7 values.

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

The domain name is fixed per use (`raw-json`, `record-payload`, `effect-input`, `effect-output`, `blob-content`, `snapshot-state`, `middleware-chain`, `agent-spec`, `model-context`, `stage-settlement`, `model-settlement`, `kernel-state`, or another versioned registry entry). JSON uses the strict RFC 8785 bytes above; durable DTOs use the frozen canonical CBOR profile; blob content uses the exact raw bytes. A digest comparison never mixes domains or schema versions. Cross-language known-answer fixtures include key-order/whitespace-equivalent JSON, duplicate-key rejection, numeric edge cases, Unicode, empty/large blobs, and every durable record family.

`finstack-ai-kernel::digest` owns `Digest`, the domain registry constants, SHA-256 wrapper, and strict canonical-JSON normalization used by semantic DTOs. Runtime and SDK call that module; protocol applies the same type to its canonical-CBOR bytes; leaf adapters do not implement competing hash/JCS rules. PR-006 pins the hash dependency and JSON known-answer corpus. PR-008 freezes the durable digest domain names and schema versions used by record/effect surfaces. PR-009 freezes `model-context`, `stage-settlement`, `model-settlement`, and `kernel-state` at schema version 1 over the JCS projections specified in section 11. PR-039 owns canonical-CBOR encoding of durable record bodies/envelopes, calculation and verification of `record-payload` / envelope checksum digests over those bytes, binary size/depth fixtures, and malicious declared-length enforcement.

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

pub struct Version {
    pub major: u16,
    pub minor: u16,
    pub patch: u16,
}

pub struct ComponentRef {
    pub id: ComponentId,
    pub version: Option<Version>,
}

pub struct MiddlewareRef {
    pub component: ComponentRef,
    pub stage: Option<Arc<str>>,
}

#[serde(rename_all = "snake_case")]
pub enum Sensitivity {
    Public,
    Internal,
    Confidential,
    Secret,
    Credential,
}

pub struct Diagnostic {
    pub code: Arc<str>,
    pub message: Arc<str>,
    pub severity: DiagnosticSeverity,
    pub metadata: Metadata,
}

#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
    Debug,
    Info,
    Warning,
    Error,
}

pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub total_tokens: Option<u64>,
    pub cost: Option<CostAmount>,
    pub extension_counters: BTreeMap<LimitKey, u64>,
}

pub struct CostAmount {
    pub unit: Arc<str>,
    pub micros: u64,
    pub pricing_policy_version: Arc<str>,
}
```

`Version` is a non-negative semantic triple; string SemVer labels are not used on durable fields. `ComponentRef.id` is required; `version` is optional selected-configuration metadata and does not replace resolved-agent lock digests. `MiddlewareRef.stage` names a pipeline stage when present and is bounded by the individual text ceiling in section 6.5. `Sensitivity` classifications match section 31.4. `Diagnostic` values are decision-local and never become `RunEvent` bodies or journal records. `Usage` is the normalized token/cost counter surface for effect completion and budget charge; `CostAmount.micros` follows the same non-negative integer-millionths rules as `CostLimit` in section 22.1 (JSON/JSONL encode as a canonical decimal string). Empty `extension_counters` maps are valid; keys are namespaced `LimitKey` values.

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

`ModelRef.provider` and `ModelRef.model` are non-empty, at most 256 UTF-8 bytes, and must not contain NUL. `thinking_level`, `context_length`, and `fast` are independently optional selected-configuration fields: a model reference may omit all three, carry thinking only, fast only, context length only, or any combination (including thinking and fast together). When present, `context_length` is a positive token budget in the portable exact-JSON integer range (`1..=2^53-1`); larger values are rejected rather than serialized imprecisely for JavaScript consumers. Absent optional fields are omitted from JSON (`skip_serializing_if`). `Message.content` is bounded by the section 6.5 array-item ceiling (4,096). `created_at` is supplied by the runtime environment; the kernel never reads a clock. `Usage` is not a message field; token/cost usage types are defined in section 7.2 and consumed by effect-completion and budget surfaces (PR-008/PR-011).

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
    ExternalEffectCompleted(ExternalEffectCompletedInput),
}

pub struct AcceptRun {
    pub session_id: SessionId,
    pub lane_id: LaneId,
    pub accepted: RunAccepted,
}

pub struct StageSettled {
    pub cursor: StageCursor,
    pub outcome: ReducerStageOutcome,
}

pub enum ReducerStageOutcome {
    Continue,
    ContextPrepared {
        messages: Arc<[Message]>,
    },
    ModelRequestPrepared {
        request: RawJson,
        component: Option<ComponentInvocation>,
        output_contract: EffectOutputContract,
        retry_safety: RetrySafety,
        deadline: Option<Timestamp>,
    },
    FinalizeAccepted,
    ContinueModel {
        reason: Option<Arc<str>>,
    },
    Fail(ErrorDescriptor),
}

pub struct ModelSettled {
    pub turn_id: TurnId,
    pub model_request_id: ModelRequestId,
    pub outcome: ModelSettlement,
}

pub enum ModelSettlement {
    Completed {
        completion: EffectCompleted,
        assistant_message: Message,
    },
    Deferred(EffectDeferred),
    Failed(EffectFailed),
}

pub enum ModelSettlementKind {
    Completed,
    Failed,
}

pub struct ExternalEffectCompletedInput {
    pub completion: ExternalEffectCompletion,
    pub assistant_message: Option<Message>,
}
```

The code block above is the complete concrete PR-009 Rust enum. PR-010 extends it with the tool contract below. The later input **inventory** continues to reserve `CancelRequested` and `TimerFired` (PR-011), capability activation (PR-012), and interaction/reconciliation/resume inputs for their mapped later PRs.

### 11.1.1 PR-010 tool command and policy contract

PR-010 adds these exact externally tagged, `snake_case`, deny-unknown wire shapes:

```rust
pub enum KernelInput {
    // PR-009 variants unchanged
    ToolBatchSettled(ToolBatchSettled),
}

pub enum ReducerStageOutcome {
    // PR-009 variants unchanged
    ToolBatchPrepared {
        calls: Arc<[ToolCallPlan]>,
        continuation: ToolBatchContinuation,
    },
}

pub struct ToolBatchSettled {
    pub tool_batch_id: ToolBatchId,
    pub outcome: ToolSettlement,
}

pub enum ToolSettlement {
    Completed(EffectCompleted),
    Deferred(EffectDeferred),
    Failed(EffectFailed),
}

pub enum ToolExecutionMode {
    Parallel,
    Sequential,
    Barrier,
}

pub enum ToolFailurePolicy {
    ReturnToModel,
    FailRun,
}

pub enum ToolBatchContinuation {
    ContinueModel,
    Finalize,
}

pub struct ValidatedToolCall {
    pub call: ToolCallBlock,
    pub tool_id: ToolId,
    pub component: Option<ComponentInvocation>,
    pub output_contract: EffectOutputContract,
    pub retry_safety: RetrySafety,
    pub deadline: Option<Timestamp>,
    pub execution: ToolExecutionMode,
    pub failure_policy: ToolFailurePolicy,
}

pub struct SyntheticToolClosure {
    pub call: ToolCallBlock,
    pub execution: ToolExecutionMode,
    pub failure_policy: ToolFailurePolicy,
    pub error: ErrorDescriptor,
}

pub enum ToolCallPlan {
    Execute(ValidatedToolCall),
    SyntheticClosure(SyntheticToolClosure),
}
```

`ToolBatchPrepared` is valid only for the exact current `(cycle, BeforeToolBatch)` cursor. Its `calls` array must cover every `ToolCallBlock` in the originating assistant message exactly once and in source order, preserving call ID, tool name, and canonical arguments. Duplicate source IDs return `duplicate_tool_call`; missing, extra, reordered, or mutated plans return `tool_batch_plan_mismatch`. An executable call must use `EffectOutputKind::ToolResult`. A synthetic closure must have a valid framework `ErrorDescriptor`; unknown tools use code `unknown_tool`, category `Tool`, `retryable = false`, and no execution action.

Consecutive `Parallel` calls share a zero-based execution group. Every `Sequential` or `Barrier` call is an exclusive group. Synthetic calls retain their declared mode for deterministic grouping but never produce `EffectRequested` or `ExecuteEffect`. The reducer dispatches only executable calls in the current eligible group. The runtime may execute those actions concurrently; it may not dispatch a later group before every executable call in the current group has reached a terminal settlement. `Barrier` is intentionally an exclusive scheduling boundary in PR-010; broader resource or middleware semantics are not implied.

`ToolBatchSettled` carries exactly one direct settlement. `ExternalEffectCompleted` remains the external settlement command: `assistant_message` must be `None` for every tool outcome. A successful direct or external completion must match the original request and use `EffectOutputKind::ToolResult`; its `EffectCompleted.output` must strictly decode as one `ToolResultBlock` whose `tool_call_id` equals the planned call. Unknown fields or variants fail strict decoding. A framework `EffectFailed` becomes a framework-authored `is_error = true` result under `ReturnToModel`; under `FailRun` it records the same safe closure for the failed call, stops later dispatch, drains already-dispatched calls, closes only undispatched calls with `tool_batch_aborted`, and installs a failed terminal candidate. A deferred call preserves the original `EffectId`, enters `AwaitingExternal`, emits no result, and cannot be synthetically closed while unresolved.

`ToolBatchContinuation::ContinueModel` increments the cycle after batch closure and re-enters `PreparingContext`. `Finalize` enters `BeforeFinalize` with the successful assistant candidate that opened the batch. Tool-result messages remain canonical history but never replace that candidate as the `RunCompleted` result.

The PR-009 stage/outcome matrix is exact:

| Current phase | `StageSettled.cursor.stage` | Allowed `ReducerStageOutcome` |
|---|---|---|
| `BeforeRun` | `BeforeRun` | `Continue`, `Fail` |
| `PreparingContext` | `PrepareContext` | `ContextPrepared`, `Fail` |
| `BeforeModel` | `BeforeModel` | `ModelRequestPrepared`, `Fail` |
| `AfterModel` | `AfterModel` | `Continue`, `Fail` |
| `BeforeToolBatch` | `BeforeToolBatch` | `ToolBatchPrepared`, `Fail` |
| `AfterToolBatch` | `AfterToolBatch` | `Continue`, `Fail` |
| `BeforeFinalize` | `BeforeFinalize` | `FinalizeAccepted`, `ContinueModel`, `Fail` |

For a new settlement, `StageSettled.cursor` must equal the state's exact expected `(cycle, stage)` before the outcome matrix is evaluated; otherwise `stage_cursor_mismatch` is returned. The cursor is part of the `stage-settlement` fingerprint and the durable `StageOutcomeRecorded`, so a delayed prior-cycle input can only classify as an equal post-commit duplicate or a conflict and can never settle the current cycle. Every other stage/outcome pair returns `invalid_phase_input`. PR-009 does not invoke middleware or context providers: these inputs are already-normalized aggregate settlements used to prove reducer semantics. PR-018 later drives the same boundaries from real middleware effects without adding a stage or changing reducer ownership. `ContinueModel` is the generic `before_finalize` continuation: it increments the checked cycle counter and re-enters `PreparingContext`; it is not a retry and does not reuse a prior `TurnId`, `ModelRequestId`, or `EffectId`. PR-011 applies configured turn/model-request limits.

`ModelSettled` applies only to the outstanding direct model effect in `AwaitingModel`. `ExternalEffectCompleted` applies only to the same effect after an equal `EffectDeferred` moved it to `AwaitingExternal`; PR-010 also permits that command for the exact deferred tool call in the active batch. It preserves the original `EffectId` and output contract. A completed external model outcome requires `assistant_message = Some`; every tool outcome and a failed model outcome require `None`. The concrete `ExternalEffectOutcome` still contains only `Completed` and `Failed`; cancellation remains PR-011 scope. An encoded `cancelled` or any other unknown/future outcome variant fails strict decoding as `invalid_input_payload` until its owning PR adds the variant and semantics. A completed model settlement must use `EffectOutputKind::ModelResponse`; its assistant message is role `Assistant`, has provider IDs equal to the completion, and may contain unique tool-call blocks in PR-010. Those call IDs must equal the preallocated `tool_call_ids` queue in source order. Fine-grained model and tool progress events are handled by the runtime event sequencer and are not kernel inputs.

All PR-009 command and record wire forms reject unknown fields, use the shared externally tagged `snake_case` enum representation, and enforce the section 6.5 limits before allocation. Optional fields default only where shown and are omitted on human-readable serialization; no unknown state-bearing member is ignored.

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

The normal path is `Accepted -> BeforeRun -> PreparingContext -> BeforeModel -> AwaitingModel -> AfterModel`, followed by either the tool cycle `BeforeToolBatch -> AwaitingTools -> AfterToolBatch`, which then continues through `PreparingContext` for another model cycle or enters `BeforeFinalize`, or the direct path `BeforeFinalize -> Completed`. Any effect-bearing phase may enter `AwaitingExternal`; middleware/tool policy may enter `AwaitingInteraction`; timers enter `Sleeping`; explicit operator/application suspension enters `Suspended`; cancellation enters `Cancelling` before `Cancelled`. `Completed`, `Failed`, and `Cancelled` are terminal. Every other transition is enumerated in reducer tests; unknown phase/input pairs return `invalid_phase_input`.

PR-010 additionally reaches `BeforeToolBatch`, `AwaitingTools`, and `AfterToolBatch`, and uses `AwaitingExternal` for one or more deferred tool effects. `Accepted` is the deterministic intermediate result of applying `RunAccepted`; successful completion of that committed batch closes to `BeforeRun` before `apply` returns. Interaction, sleeping, cancellation, suspension, and cancelled terminal phases remain frozen for their owning later PRs.

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
        first_transient_sequence: u64,
    ) -> Result<Arc<[RunEvent]>, KernelError>;
}
```

`KernelEvent` in the older signature was an inconsistent alias and is removed. `RunEvent` is the sole runtime event vocabulary from section 20. `decide` does not mutate authoritative state. The runtime-owned event sequencer supplies `first_transient_sequence` as the next unused run-stream sequence. `apply` assigns contiguous values to its derived events in record/ordinal order and rejects overflow as `invalid_input_payload`; transient model progress emitted between commits consumes values from the same sequencer. The event sequence is live delivery metadata, not authoritative kernel state, and remains excluded from `KernelState::state_hash()`.

```rust
pub struct Decision {
    pub expected_sequence: u64,
    pub records: Vec<RecordDraft>,
    pub actions: Vec<PostCommitAction>,
    pub diagnostics: Vec<Diagnostic>,
}
```

`PostCommitAction` may execute only after the associated records are committed and applied.

### 11.3.1 Authoritative state

```rust
pub struct KernelState {
    pub state_version: u16,
    pub last_applied_sequence: u64,
    pub session_id: Option<SessionId>,
    pub lane_id: Option<LaneId>,
    pub accepted: Option<RunAccepted>,
    pub phase: Option<RunPhase>,
    pub cycle: u64,
    pub current_turn: Option<CurrentTurn>,
    pub messages: Arc<[Message]>,
    pub pending_model_effect: Option<PendingModelEffect>,
    pub terminal_candidate: Option<TerminalCandidate>,
    pub stage_settlements: BTreeMap<StageCursor, Digest>,
    pub model_settlements: BTreeMap<EffectId, ModelSettlementFingerprint>,
    pub completion_identities: BTreeMap<Arc<str>, CompletionIdentity>,
    pub terminal: Option<TerminalState>,
}

pub struct StageCursor {
    pub cycle: u64,
    pub stage: Stage,
}

pub struct CurrentTurn {
    pub cycle: u64,
    pub turn_id: TurnId,
    pub context: ContextPrepared,
    pub model_request_id: Option<ModelRequestId>,
    pub effect_id: Option<EffectId>,
    pub final_message_id: Option<MessageId>,
}

pub struct PendingModelEffect {
    pub cycle: u64,
    pub turn_id: TurnId,
    pub model_request_id: ModelRequestId,
    pub requested: EffectRequested,
    pub deferred: Option<EffectDeferred>,
}

pub enum TerminalCandidate {
    Completed {
        cycle: u64,
        turn_id: TurnId,
        model_request_id: ModelRequestId,
        effect_id: EffectId,
        message_id: MessageId,
        result_digest: Digest,
    },
    Failed {
        cycle: u64,
        turn_id: Option<TurnId>,
        model_request_id: Option<ModelRequestId>,
        effect_id: Option<EffectId>,
        error: ErrorDescriptor,
    },
}

pub struct ModelSettlementFingerprint {
    pub kind: ModelSettlementKind,
    pub digest: Digest,
}

pub struct CompletionIdentity {
    pub effect_id: EffectId,
    pub settlement_digest: Digest,
}

pub enum TerminalState {
    Completed(RunCompleted),
    Failed(RunFailed),
}
```

`KernelState::default()` is unaccepted with `state_version = 1`, `last_applied_sequence = 0`, and all optional/collection fields empty. A restore at a nonzero session sequence constructs the same state by applying the complete relevant committed prefix; PR-041 snapshots remain derived caches. `session_id`, `lane_id`, and `RunAccepted.run_id` become immutable when `RunAccepted` applies.

The code block is the complete concrete PR-009 terminal-state vocabulary. PR-009 materializes only `Completed` and `Failed`; PR-011 owns adding `Cancelled(RunCancelled)` to this state enum together with cancellation input, transition, record-application, and compatibility semantics. The later `RunPhase::Cancelled` inventory value is not a PR-009 `TerminalState` payload.

PR-009 applies the existing section 6.5 semantic ceilings as hard authoritative-state capacities: `messages` has at most 4,096 items; each of `stage_settlements`, `model_settlements`, and `completion_identities` has at most 256 entries. PR-010 applies the same 256-entry ceiling independently to `tool_calls` and `tool_settlements`. A replacement/equal duplicate at an existing key does not grow a collection. Before a non-duplicate `decide` or committed-batch `apply` would make any field exceed its ceiling, it returns `state_capacity_exceeded` with the exact field name and leaves state unchanged. Capacity is never reported as `invariant_violation`, and collections are never truncated, evicted, or allowed to grow without bound.

`stage_settlements` is keyed by `(cycle, stage)`, terminal `model_settlements` by `EffectId`, and `completion_identities` by any non-empty settlement `completion_id`; all are derived from committed records and retained so duplicate/conflict classification survives replay. The pending effect retains an optional full `EffectDeferred` value separately because deferral followed by final completion under the same `EffectId` is the intended transition, not a conflict. An equal normalized settlement after its phase advanced returns an empty decision plus a `duplicate_settlement` diagnostic. Completion identity has precedence over the per-effect index: an indexed non-empty `completion_id` with equal `(effect_id, settlement_digest)` is an exact duplicate, while a changed effect or digest returns `conflicting_completion_id` without consulting or returning the per-effect conflict. `conflicting_settlement` is used only for unequal indexed stage, effect, or deferral content not already classified by completion identity. An unknown or non-outstanding effect returns `effect_not_pending`.

### 11.3.2 Settlement fingerprints

#### 11.3.2.1 Stage-settlement fingerprint

`stage-settlement` schema 1 hashes the section 6.4 domain prefix plus RFC 8785 JCS bytes of exactly one externally tagged `snake_case` variant below:

```rust
pub enum StageSettlementFingerprintV1 {
    Continue {
        cursor: StageCursor,
    },
    ContextPrepared {
        cursor: StageCursor,
        messages: Arc<[Message]>,
    },
    ModelRequestPrepared {
        cursor: StageCursor,
        request: RawJson,
        component: Option<ComponentInvocation>,
        output_contract: EffectOutputContract,
        retry_safety: RetrySafety,
        deadline: Option<Timestamp>,
    },
    ToolBatchPrepared {
        cursor: StageCursor,
        calls: Arc<[ToolCallPlan]>,
        continuation: ToolBatchContinuation,
    },
    FinalizeAccepted {
        cursor: StageCursor,
    },
    ContinueModel {
        cursor: StageCursor,
    },
    Fail {
        cursor: StageCursor,
        error: ErrorDescriptor,
    },
}
```

The exact top-level JSON shapes are `{"continue":{...}}`, `{"context_prepared":{...}}`, `{"model_request_prepared":{...}}`, `{"tool_batch_prepared":{...}}`, `{"finalize_accepted":{...}}`, `{"continue_model":{...}}`, or `{"fail":{...}}`. Member names and values are exactly those shown. Every optional nested member, including tool component/deadline and error identifiers, is projected explicitly as a value or JSON `null`. `ContinueModel.reason` is diagnostic, non-semantic input: it is absent from the projection and `StageOutcomeRecorded`, is not replayed, and does not affect equality. Two otherwise equal `ContinueModel` inputs with different reasons therefore produce the same fingerprint and classify as equal. The digest does **not** cover the complete human-readable `StageSettled` input.

Fingerprint construction and replay reconstruction are exact:

| Valid command outcome | Committed record source used by `apply` / replay |
|---|---|
| `Continue` | matching `StageOutcomeRecorded { cursor, disposition: Continued, ... }` |
| `ContextPrepared { messages }` | matching stage record plus required `ContextPrepared`; use its complete `messages`, and validate disposition `turn_id`/`context_digest` against the sibling |
| `ModelRequestPrepared { request, component, output_contract, retry_safety, deadline }` | matching stage record plus required model `EffectRequested`; use `EffectInput::Model.request`, `component`, `output_contract`, `retry_safety`, and `deadline`, and validate disposition turn/request/effect IDs against state and sibling |
| `ToolBatchPrepared { calls, continuation }` | matching stage record plus required `ToolBatchOpened`; use each source-ordered `AssignedToolCall.plan` plus `continuation`, excluding reducer-assigned batch, effect, source-index, and group-index values from this stage fingerprint |
| `FinalizeAccepted` | matching stage record plus its required candidate-matching terminal sibling |
| `ContinueModel { reason: _ }` | matching stage record `ContinueModel { next_cycle }`; validate `next_cycle = checked(cursor.cycle + 1)` but do not project it or any reason |
| `Fail(error)` | matching stage record `Failed { error }` and, for `BeforeFinalize`, its required matching `RunFailed` sibling |

`apply` validates the complete sibling/order/correlation rules, reconstructs this DTO from committed semantic fields, recomputes the digest, and compares it with `StageOutcomeRecorded.settlement_digest` before inserting `stage_settlements` or changing phase. A missing/malformed sibling is `invalid_record_order`; correlation or semantic mismatch uses its dedicated error; a recorded digest unequal to the reconstructed projection is `settlement_digest_mismatch`. Full replay follows the same reconstruction and must produce the same index. No reducer-assigned turn, request, effect, record, or event ID enters this fingerprint.

#### 11.3.2.2 Model-settlement fingerprint

`model-settlement` schema 1 hashes the section 6.4 domain prefix plus RFC 8785 JCS bytes of exactly one externally tagged `snake_case` variant below. The variant is the input family and source discriminator; direct and external settlements with otherwise equal values intentionally hash differently.

```rust
pub enum ModelSettlementFingerprintV1 {
    DirectCompleted(DirectModelCompletedFingerprintV1),
    DirectFailed(DirectModelFailedFingerprintV1),
    DirectDeferred(DirectModelDeferredFingerprintV1),
    ExternalCompleted(ExternalModelCompletedFingerprintV1),
    ExternalFailed(ExternalModelFailedFingerprintV1),
}

pub struct DirectModelCompletedFingerprintV1 {
    pub turn_id: TurnId,
    pub model_request_id: ModelRequestId,
    pub completion: EffectCompleted,
    pub assistant_message: Message,
}

pub struct DirectModelFailedFingerprintV1 {
    pub turn_id: TurnId,
    pub model_request_id: ModelRequestId,
    pub failure: EffectFailed,
}

pub struct DirectModelDeferredFingerprintV1 {
    pub turn_id: TurnId,
    pub model_request_id: ModelRequestId,
    pub deferred: EffectDeferred,
}

pub struct ExternalModelCompletedFingerprintV1 {
    pub effect_id: EffectId,
    pub completion_id: Arc<str>,
    pub output: RawJson,
    pub usage: Option<Usage>,
    pub artifacts: Arc<[ArtifactRef]>,
    pub assistant_message: Message,
}

pub struct ExternalModelFailedFingerprintV1 {
    pub effect_id: EffectId,
    pub completion_id: Arc<str>,
    pub error: ErrorDescriptor,
}
```

The exact top-level JSON shapes are `{"direct_completed":{...}}`, `{"direct_failed":{...}}`, `{"direct_deferred":{...}}`, `{"external_completed":{...}}`, or `{"external_failed":{...}}`. Struct member names are exactly those shown; every member is present; options are explicit JSON `null`; arrays preserve semantic order and are present when empty; IDs/digests/timestamps and nested DTOs use their frozen public JSON field sets. A `RawJson` member is embedded as its parsed semantic JSON value, not as source bytes or a JSON string, before the enclosing object is canonicalized. The completed variants include the full assistant `Message`, including ID, creation time, content, metadata, model, and provider IDs.

For both stage- and model-settlement schema 1, explicit null is recursive. Implementations must construct dedicated fingerprint projection DTOs for every nested struct that has optional fields and serialize every such field as JSON `null` when absent. They must not serialize the ordinary human-readable DTO directly, because that form may omit absent optionals. The semantic type names in the Rust-like definitions identify the exact nested field sets; they do not authorize reuse of omission-bearing Serde settings.

Fingerprint construction from a valid command is exact:

| Kernel input | Fingerprint variant and fields |
|---|---|
| `ModelSettled { turn_id, model_request_id, outcome: Completed { completion, assistant_message } }` | `DirectCompleted` with every supplied field unchanged after `RawJson` normalization |
| `ModelSettled { turn_id, model_request_id, outcome: Failed(failure) }` | `DirectFailed` with the complete supplied `EffectFailed` |
| `ModelSettled { turn_id, model_request_id, outcome: Deferred(deferred) }` | `DirectDeferred` with the complete supplied `EffectDeferred` |
| `ExternalEffectCompletedInput { completion: { effect_id, completion_id, outcome: Completed { output, usage, artifacts } }, assistant_message: Some(message) }` | `ExternalCompleted` with all six shown fields |
| `ExternalEffectCompletedInput { completion: { effect_id, completion_id, outcome: Failed { error } }, assistant_message: None }` | `ExternalFailed` with all three shown fields |

For an external completion, fields absent from the command are excluded rather than synthesized into the fingerprint. `output_contract` is excluded and must match `pending_model_effect.requested.output_contract`; `output_digest` is excluded and must be recomputed from `output`; `usage_digest` is excluded and must be recomputed from `usage`; `EffectCompleted.provider_ids` is excluded because it must equal the included `assistant_message.provider_ids`; and `reservation_id` is excluded and must be `None` in PR-009. The outer turn/model-request/cycle correlations are also excluded because external input carries only `effect_id`; they are validated through the outstanding pending effect. For external failure, the committed `EffectFailed` must additionally have `usage = None` and `usage_digest = None`. Any failure of these equalities is `model_settlement_mismatch`, while malformed sibling shape/order remains `invalid_record_order`.

`apply` and replay reconstruct the same variant before clearing `pending_model_effect`:

| Pre-record state and committed siblings | Reconstructed fingerprint |
|---|---|
| `AwaitingModel`, no deferred value; `EffectCompleted` immediately followed by matching `EntryAppended` | `DirectCompleted` from pending turn/request correlation, the complete `EffectCompleted`, and `EntryAppended.message` |
| `AwaitingModel`, no deferred value; matching `EffectFailed` | `DirectFailed` from pending turn/request correlation and the complete `EffectFailed` |
| `AwaitingModel`, no deferred value; matching `EffectDeferred` | `DirectDeferred` from pending turn/request correlation and the complete `EffectDeferred`; the full value remains pending for later duplicate checks |
| `AwaitingExternal`, deferred value present; `EffectCompleted` immediately followed by matching `EntryAppended` | after the external-field validations above, `ExternalCompleted` from record `effect_id`, required `completion_id`, `output`, `usage`, `artifacts`, and `EntryAppended.message` |
| `AwaitingExternal`, deferred value present; matching `EffectFailed` | after the external-field validations above, `ExternalFailed` from record `effect_id`, required `completion_id`, and `error` |

Replay recreates the same pre-record state from the preceding `EffectRequested` and optional `EffectDeferred`, so source discrimination does not require another durable field. Non-empty usage and artifacts come directly from `EffectCompleted`; the assistant message comes from its required `EntryAppended` sibling. The reconstructed digest is inserted into `model_settlements` and, when present, `completion_identities`; inability to reconstruct the exact command projection or a digest mismatch is `settlement_digest_mismatch`. `ModelSettlementFingerprint { kind, digest }` remains sufficient because `digest` commits to source and every projected field. Deferred duplicate classification recomputes `DirectDeferred` from the retained full pending value, so no additional state field is required.

#### 11.3.2.3 PR-010 tool fingerprints

PR-010 adds three schema-1 domains. All use the section 6.4 prefix plus JCS of dedicated recursive-explicit-null DTOs; ordinary omission-bearing human-readable DTO serialization is forbidden.

`tool-batch-plan` hashes `ToolBatchPlanFingerprintV1 { cycle, turn_id, tool_batch_id, source_message_id, calls, continuation }`, where `calls` is the complete `AssignedToolCall` array including reducer-assigned effect IDs, source/group indexes, and the complete executable or synthetic plan. The digest excludes only `plan_digest` itself.

`tool-settlement` hashes exactly one source-discriminated variant:

```rust
pub enum ToolSettlementFingerprintV1 {
    DirectCompleted { tool_batch_id: ToolBatchId, completion: EffectCompleted },
    DirectFailed { tool_batch_id: ToolBatchId, failure: EffectFailed },
    DirectDeferred { tool_batch_id: ToolBatchId, deferred: EffectDeferred },
    ExternalCompleted {
        tool_batch_id: ToolBatchId,
        effect_id: EffectId,
        completion_id: Arc<str>,
        output: RawJson,
        usage: Option<Usage>,
        artifacts: Arc<[ArtifactRef]>,
    },
    ExternalFailed {
        tool_batch_id: ToolBatchId,
        effect_id: EffectId,
        completion_id: Arc<str>,
        error: ErrorDescriptor,
    },
    Synthetic {
        tool_batch_id: ToolBatchId,
        tool_call_id: ToolCallId,
        effect_id: EffectId,
        result: ToolResultBlock,
        error: ErrorDescriptor,
    },
}
```

Direct and external settlements with otherwise equal values intentionally differ. External projection/exclusion rules match the model external fingerprint: request output contract and recomputed digests are validated rather than projected, reservation is absent, and a tool completion has no assistant message or provider IDs. A synthetic digest commits to the exact model-visible result and safe framework error.

`tool-batch-close` hashes `ToolBatchCloseFingerprintV1 { cycle, turn_id, tool_batch_id, source_message_id, result_message_ids, outcome }` and excludes only `close_digest`. Apply reconstructs all three domains from the committed record and required siblings/state before mutation; any mismatch is `settlement_digest_mismatch`.

### 11.3.3 Post-commit actions and allocated IDs

```rust
pub enum PostCommitAction {
    ExecuteEffect { effect_id: EffectId },
}
```

An `ExecuteEffect` action must have exactly one preceding sibling `EffectRequested` draft with the same ID in the decision. Actions preserve request-record order. A runtime may execute an action only after the complete decision batch commits and `apply` succeeds; an empty or failed append executes none.

`decide` treats every `AllocatedIds` vector as an ordered queue and never generates or derives a UUID. It consumes:

1. `record_ids` in `Decision.records` order;
2. `event_ids` in record order and then the section 20 ordinal order;
3. `effect_ids` in new `EffectRequested` order;
4. `turn_ids` when a successful `PrepareContext` settlement starts a cycle;
5. `model_request_ids` when a successful `BeforeModel` settlement requests a model;
6. `message_ids` when a successful model settlement finalizes the assistant message.

PR-010 consumes the assistant message's preallocated `tool_call_ids` during successful model settlement in source order. `ToolBatchPrepared` consumes one `tool_batch_id` and one `effect_id` per source call, including synthetic calls, then later group dispatch reuses those persisted effect IDs without consuming new ones. Every finalized tool result consumes one `message_id`. PR-010 still consumes no interaction, cancellation-request, or append-batch ID. `append_batch_ids` remains runtime-owned when constructing `AppendRequest`. For a state-changing decision, every kernel-owned queue must contain exactly the IDs required by that transition: shortage returns `allocated_ids_exhausted`, and unused IDs in a kernel-owned queue return `unused_allocated_ids`. Assistant/tool-result semantic checks occur before allocated-ID validation; equality with consumed message/call IDs is checked only after every required queue passes shortage/extra validation.

The 256-record append-batch ceiling is also preflighted before allocated-ID validation. A prepared plan is rejected as `invalid_input_payload { field = "records", reason_code = "too_many_items" }` if its opening decision, the adversarial-order terminal settlement of any execution group (including contiguous result finalization plus next-group dispatch or closure), or any `FailRun` drain/abort closure could exceed 256 records. No accepted plan can later become uncommittable solely because parallel completions arrived in a different order.

`decide` uses this validation order:

1. decode and structurally validate the normalized input;
2. compute its settlement identity/fingerprint without reading or consuming `AllocatedIds`;
3. when the input carries a non-empty `completion_id`, consult `completion_identities` first: an existing equal `(effect_id, settlement_digest)` returns the empty duplicate decision, and an existing unequal tuple returns `conflicting_completion_id` immediately;
4. only when step 3 finds no indexed completion identity, consult the applicable stage, per-effect, or pending-deferral index: equal content returns the empty duplicate decision and unequal content returns `conflicting_settlement`;
5. for either duplicate return, set `expected_sequence = last_applied_sequence + 1` and do not validate or consume any queue in `env.ids`;
6. reject terminal state, stale cursor, wrong phase, wrong outstanding correlation, or invalid semantic payload in the stable order listed in section 11.3.5; assistant presence/role/tool/provider/time checks occur here;
7. preflight prospective state growth in fixed field order `messages`, `stage_settlements`, `model_settlements`, `completion_identities`, `tool_calls`, `tool_settlements`, returning `state_capacity_exceeded` for the first field that would cross its hard ceiling;
8. calculate required IDs and validate every kernel-owned queue for shortages, then extras, in the queue order above; a missing message ID is `allocated_ids_exhausted` even if the supplied message ID could not match;
9. after successful queue validation, require every supplied ID-bearing payload to equal its consumed ID, including assistant `message.id`; mismatch is `assistant_message_mismatch` for that message and `record_identity_mismatch` for other payloads;
10. build the decision.

A pre-commit reevaluation sees no committed settlement index, repeats steps 6–10 against the unchanged state, and therefore requires the same `TransitionEnv` and reproduces the same non-empty records and actions byte-for-byte. A post-commit redelivery takes step 3 or 4; the reused environment may still contain the IDs from the original attempt, but the empty decision neither consumes nor rejects them. This exact-duplicate exception is the only path on which a non-empty kernel-owned queue does not produce `unused_allocated_ids`.

### 11.3.4 Apply validation and phase derivation

`apply` validates the entire batch on a temporary state and swaps it into the kernel only if all records succeed. The batch must be non-empty; `first_sequence` must equal `last_applied_sequence + 1`; `last_sequence` must equal `first_sequence + records.len() - 1`; and every envelope sequence must be contiguous and equal its position. Batch range mismatch is `committed_batch_range_mismatch`; a gap, duplicate, rollback, or overflow is `non_contiguous_record_sequence`. Every record must match the accepted session/lane/run identity (with `RunAccepted` establishing it), and all intra-batch sibling/order constraints below must hold.

Apply precedence is structural batch/range/sequence validation, then record identity/sibling/order and reconstructed fingerprint/digest validation, then temporary-state invariant validation and one checked preflight of the whole batch's net-new state entries in fixed order `messages`, `stage_settlements`, `model_settlements`, `completion_identities`, `tool_calls`, `tool_settlements`, then atomic swap. A batch that would cross a hard capacity returns `state_capacity_exceeded` without mutating authoritative state; a malformed/tampered batch retains its earlier dedicated validation error rather than being masked by capacity.

The PR-009 record-to-state table is normative:

| Committed record | Required prior phase / sibling rule | Applied state |
|---|---|---|
| `RunAccepted` | unaccepted state; only record in its decision | identity fixed; `Accepted`, then deterministic batch-boundary closure to `BeforeRun` |
| `StageOutcomeRecorded(BeforeRun, Continued)` | `BeforeRun` | `PreparingContext` |
| `StageOutcomeRecorded(PrepareContext, ContextPrepared)` | `PreparingContext`; immediately followed by matching `ContextPrepared` | cursor recorded; phase advances when sibling applies |
| `ContextPrepared` | matching prepare-context outcome | current turn/context set; `BeforeModel` |
| `StageOutcomeRecorded(BeforeModel, ModelRequested)` | `BeforeModel`; immediately followed by matching model `EffectRequested` | correlation fixed; phase advances when sibling applies |
| `EffectRequested(Model)` | matching before-model outcome | pending effect set; `AwaitingModel` |
| `EffectDeferred` | `AwaitingModel`; matches pending request | pending effect retained with handle; `AwaitingExternal` |
| `EffectCompleted(Model)` | `AwaitingModel` or `AwaitingExternal`; immediately followed by matching `EntryAppended` | effect settlement and any completion identity indexed; phase advances when sibling applies |
| `EntryAppended` | matching completed effect in the same batch | message appended, candidate set; `AfterModel` |
| `EffectFailed(Model)` | `AwaitingModel` or `AwaitingExternal`; matches pending request | effect settlement and any completion identity indexed; failure candidate set; `BeforeFinalize` |
| `StageOutcomeRecorded(AfterModel, Continued)` | `AfterModel` | preserve completion candidate; `BeforeToolBatch` when the assistant message contains calls, otherwise `BeforeFinalize` |
| non-final `StageOutcomeRecorded(..., Failed)` | matching nonterminal stage | failure candidate set; `BeforeFinalize` |
| `StageOutcomeRecorded(BeforeFinalize, FinalizeAccepted)` | `BeforeFinalize`; immediately followed by terminal record matching the candidate | phase advances when sibling applies |
| `StageOutcomeRecorded(BeforeFinalize, ContinueModel)` | `BeforeFinalize`; completion candidate only | clear candidate/current request, checked `cycle += 1`; `PreparingContext` |
| `StageOutcomeRecorded(BeforeFinalize, Failed)` | `BeforeFinalize`; immediately followed by matching `RunFailed` | phase advances when sibling applies |
| `RunCompleted` | matching finalize-accepted completion candidate | terminal and immutable; `Completed` |
| `RunFailed` | matching finalize-accepted failure candidate or before-finalize failure | terminal and immutable; `Failed` |

The corresponding PR-009 decision/batch shapes are exact:

| Valid input | Ordered record bodies | Actions | Phase after apply |
|---|---|---|---|
| `AcceptRun` | `RunAccepted` | none | `BeforeRun` |
| `StageSettled((cycle, BeforeRun), Continue)` | `StageOutcomeRecorded` | none | `PreparingContext` |
| `StageSettled((cycle, PrepareContext), ContextPrepared)` | `StageOutcomeRecorded`, `ContextPrepared` | none | `BeforeModel` |
| `StageSettled((cycle, BeforeModel), ModelRequestPrepared)` | `StageOutcomeRecorded`, `EffectRequested(Model)` | matching `ExecuteEffect` | `AwaitingModel` |
| `ModelSettled(Completed)` | `EffectCompleted`, `EntryAppended` | none | `AfterModel` |
| `ModelSettled(Deferred)` | `EffectDeferred` | none | `AwaitingExternal` |
| `ModelSettled(Failed)` | `EffectFailed` | none | `BeforeFinalize` |
| `ExternalEffectCompleted` | same durable completed/failed record shape, with distinct external fingerprint variant | none | `AfterModel` / `BeforeFinalize` |
| `StageSettled((cycle, AfterModel), Continue)` | `StageOutcomeRecorded` | none | `BeforeToolBatch` when the assistant message contains calls, otherwise `BeforeFinalize` |
| non-final `StageSettled((cycle, ...), Fail)` | `StageOutcomeRecorded` | none | `BeforeFinalize` |
| `StageSettled((cycle, BeforeFinalize), FinalizeAccepted)` | `StageOutcomeRecorded`, matching `RunCompleted` or `RunFailed` | none | `Completed` / `Failed` |
| `StageSettled((cycle, BeforeFinalize), ContinueModel)` | `StageOutcomeRecorded` | none | `PreparingContext` |
| `StageSettled((cycle, BeforeFinalize), Fail)` | `StageOutcomeRecorded`, `RunFailed` | none | `Failed` |
| exact indexed duplicate settlement | none | none | unchanged |

PR-010 adds these exact decision shapes. `ToolCallSettled*` means the newly contiguous source prefix only; `EffectRequested(next group)*` is present only when that prefix makes the next executable group eligible.

| Valid input | Ordered record bodies | Actions | Phase after apply |
|---|---|---|---|
| `StageSettled((cycle, BeforeToolBatch), ToolBatchPrepared)` | `StageOutcomeRecorded`, `ToolBatchOpened`, `EffectRequested(first executable group)*`, leading synthetic `ToolCallSettled*`, optional `ToolBatchClosed` for an all-synthetic plan | one matching `ExecuteEffect` per first-group request | `AwaitingTools`, or `AfterToolBatch` for an all-synthetic plan |
| `ToolBatchSettled(Deferred)` | `EffectDeferred` | none | `AwaitingExternal` |
| `ToolBatchSettled(Completed/Failed)` | matching `EffectCompleted`/`EffectFailed`, `ToolCallSettled*`, `EffectRequested(next group)*` or optional terminal `ToolBatchClosed` | one matching `ExecuteEffect` per next-group request | `AwaitingTools`, `AwaitingExternal`, or `AfterToolBatch` |
| tool `ExternalEffectCompleted` | matching external `EffectCompleted`/`EffectFailed`, then the same ordered follow-up shape | one matching `ExecuteEffect` per newly eligible next-group request | `AwaitingTools`, `AwaitingExternal`, or `AfterToolBatch` |
| `StageSettled((cycle, AfterToolBatch), Continue)` | `StageOutcomeRecorded` | none | `PreparingContext` with checked `cycle += 1` for `ContinueModel`; `BeforeFinalize` for `Finalize` or `Failed` |
| exact indexed tool settlement/deferral duplicate | none | none | unchanged |

`StageOutcomeRecorded`, `ContextPrepared`, and `EntryAppended` cannot appear without their required sibling records. Record order is semantic and fixed as shown. `RunCompleted` or `RunFailed` is never proposed or applied until `BeforeFinalize` settles. Once terminal, `apply` rejects every later record and `decide` rejects every state-changing input with `terminal_state_immutable`; only an exact already-indexed settlement duplicate may return the empty idempotent decision.

### 11.3.5 Stable reducer errors

```rust
pub enum KernelError {
    InvalidInputPayload { field: &'static str, reason_code: &'static str },
    InvalidRunAcceptance,
    InvalidPhaseInput { phase: Option<RunPhase>, input: &'static str },
    StageCursorMismatch { expected: StageCursor, actual: StageCursor },
    ModelRequestContractMismatch,
    ModelSettlementMismatch,
    DuplicateToolCall,
    ToolBatchPlanMismatch,
    ToolEffectContractMismatch,
    ToolSettlementMismatch,
    ToolResultMismatch,
    AssistantMessagePresenceMismatch,
    AssistantMessageMismatch,
    SettlementDigestMismatch,
    ContextDigestMismatch,
    ConflictingCompletionId,
    CycleOverflow,
    StateCapacityExceeded { field: &'static str },
    AllocatedIdsExhausted { kind: &'static str },
    UnusedAllocatedIds { kind: &'static str },
    CommittedBatchRangeMismatch,
    NonContiguousRecordSequence,
    RecordIdentityMismatch,
    InvalidRecordOrder,
    EffectNotPending { effect_id: EffectId },
    ConflictingSettlement,
    TerminalStateImmutable,
    StateHashFailed,
    InvariantViolation,
}
```

The stable codes are the `snake_case` variant names: `invalid_input_payload`, `invalid_run_acceptance`, `invalid_phase_input`, `stage_cursor_mismatch`, `model_request_contract_mismatch`, `model_settlement_mismatch`, `duplicate_tool_call`, `tool_batch_plan_mismatch`, `tool_effect_contract_mismatch`, `tool_settlement_mismatch`, `tool_result_mismatch`, `assistant_message_presence_mismatch`, `assistant_message_mismatch`, `settlement_digest_mismatch`, `context_digest_mismatch`, `conflicting_completion_id`, `cycle_overflow`, `state_capacity_exceeded`, `allocated_ids_exhausted`, `unused_allocated_ids`, `committed_batch_range_mismatch`, `non_contiguous_record_sequence`, `record_identity_mismatch`, `invalid_record_order`, `effect_not_pending`, `conflicting_settlement`, `terminal_state_immutable`, `state_hash_failed`, and `invariant_violation`.

Mandatory validation maps exactly as follows; implementations must not substitute `invariant_violation` for a rejected public payload or settlement:

| Failure | Stable `KernelError` code |
|---|---|
| strict decode, unknown field, malformed ID/digest/timestamp, empty required string, section 6.5 bound, or invalid enum discriminant | `invalid_input_payload` |
| inconsistent `AcceptRun` identities or invalid accepted payload | `invalid_run_acceptance` |
| no transition for the current phase/input or disallowed stage outcome | `invalid_phase_input` |
| a new `StageSettled.cursor` differs from the exact current `(cycle, stage)` | `stage_cursor_mismatch` |
| model request output kind is not `ModelResponse`, request bytes are invalid, or retry/deadline/component fields violate the request contract | `model_request_contract_mismatch` |
| turn/request/effect correlation, effect kind, output contract, provider IDs, deferral handle, or completed/failed settlement metadata differs from the pending model request | `model_settlement_mismatch` |
| an assistant tool call repeats a source ID within the message or reuses a persistent call ID | `duplicate_tool_call` |
| the prepared plan is missing, extra, reordered, or mutates a source call | `tool_batch_plan_mismatch` |
| an executable tool plan or settlement uses the wrong effect kind/output contract | `tool_effect_contract_mismatch` |
| batch/call/effect/phase/deferral or completion metadata does not match the active tool call | `tool_settlement_mismatch` |
| a successful tool output does not decode exactly to the matching `ToolResultBlock` | `tool_result_mismatch` |
| successful external completion lacks `assistant_message`, or failed outcome supplies one | `assistant_message_presence_mismatch` |
| assistant role, tool-call prohibition, provider IDs, or `created_at` differs from the required finalized message; checked before allocated-ID queues | `assistant_message_mismatch` |
| after successful queue validation, assistant `message.id` differs from the consumed `MessageId` | `assistant_message_mismatch` |
| a committed stage/effect sibling set cannot reproduce its dedicated settlement projection digest or differs from the recorded/indexed digest | `settlement_digest_mismatch` |
| `ContextPrepared.context_digest` does not match canonical prepared messages/context | `context_digest_mismatch` |
| an already-indexed non-empty `completion_id` identifies a different effect or terminal settlement digest; this lookup precedes and short-circuits per-effect conflict lookup | `conflicting_completion_id` |
| checked cycle increment overflows | `cycle_overflow` |
| a non-duplicate decision or valid committed batch would grow one named authoritative collection beyond 4,096 messages or 256 entries | `state_capacity_exceeded` |
| required allocated ID absent / extra ID present on a state-changing path | `allocated_ids_exhausted` / `unused_allocated_ids` |
| committed envelope session/lane/run, record/event ID, sequence-independent correlation, or required transition timestamp differs | `record_identity_mismatch` |
| required sibling is absent, duplicated, mismatched, or out of order | `invalid_record_order` |
| settlement names an unknown or no-longer-pending effect and is not indexed | `effect_not_pending` |
| indexed stage/effect/deferral identity has unequal normalized content and no indexed completion identity already classified the input | `conflicting_settlement` |

Batch-range and sequence failures retain their dedicated codes. `terminal_state_immutable`, state-hash serialization failure, and an unreachable reducer-state contradiction map only to `terminal_state_immutable`, `state_hash_failed`, and `invariant_violation`, respectively. Errors returned by `decide` or `apply` do not mutate state and do not fabricate durable failure records. A normalized `Fail(ErrorDescriptor)` is semantic input, not `KernelError`; it follows the `before_finalize` path and becomes `RunFailed` only after commit.

PR-009 boundary tests must cover one-below-to-limit success, at-limit non-growing duplicate success, at-limit growth failure for each of the four fields, deterministic first-field precedence when one transition would exceed multiple fields, and whole-batch apply overflow with no partial mutation. Private pure-preflight tests may assemble capacity-only states for ceilings that cannot be reached through PR-009's model-only transition graph: the 256-entry stage index is exhausted before the 4,096-message or 256-entry model/completion indexes can be reached. Full decide/apply/replay parity is required for every prefix reachable in the current scope; each later PR that adds transitions capable of independently reaching another ceiling must add the corresponding full-replay boundary proof. Tests must not add a public arbitrary-state restoration constructor merely to synthesize unreachable boundaries. Separate ordering tests must prove semantic assistant failure precedes queue errors, a valid assistant with a missing message ID returns `allocated_ids_exhausted`, and consumed-message-ID mismatch is checked only after queues pass.

### 11.3.6 Canonical state hash

`KernelState::state_hash()` is SHA-256 under domain `kernel-state`, schema version 1, using the section 6.4 prefix encoding. The hash input is RFC 8785 JCS of this exact deny-unknown DTO; the internal maps are never serialized directly:

```rust
pub struct KernelStateHashV1 {
    pub state_version: u16,
    pub last_applied_sequence: u64,
    pub session_id: Option<SessionId>,
    pub lane_id: Option<LaneId>,
    pub accepted: Option<RunAccepted>,
    pub phase: Option<RunPhase>,
    pub cycle: u64,
    pub current_turn: Option<CurrentTurn>,
    pub messages: Arc<[Message]>,
    pub pending_model_effect: Option<PendingModelEffect>,
    pub terminal_candidate: Option<TerminalCandidate>,
    pub stage_settlements: Arc<[StageSettlementHashEntryV1]>,
    pub model_settlements: Arc<[ModelSettlementHashEntryV1]>,
    pub completion_identities: Arc<[CompletionIdentityHashEntryV1]>,
    pub terminal: Option<TerminalState>,
}

pub struct StageSettlementHashEntryV1 {
    pub cycle: u64,
    pub stage: Stage,
    pub settlement_digest: Digest,
}

pub struct ModelSettlementHashEntryV1 {
    pub effect_id: EffectId,
    pub kind: ModelSettlementKind,
    pub settlement_digest: Digest,
}

pub struct CompletionIdentityHashEntryV1 {
    pub completion_id: Arc<str>,
    pub effect_id: EffectId,
    pub settlement_digest: Digest,
}
```

The JSON object has exactly the `KernelStateHashV1` fields above in the shown names. Every field is present: optional values are JSON `null`; collection fields are JSON arrays, including when empty; integers are JSON base-10 numbers; IDs and timestamps use their canonical public string forms; digests are lowercase hexadecimal strings; structs are objects with all fields present; and enums use the shared externally tagged `snake_case` representation. Nested `CurrentTurn`, `PendingModelEffect`, `TerminalCandidate`, `TerminalState`, `RunAccepted`, `Message`, and record payloads use dedicated state-hash projection DTOs with the exact field sets frozen by their definitions and recursive explicit nulls. Implementations must not serialize ordinary human-readable DTOs directly where their Serde form omits absent optionals. The schema-1 `TerminalState` projection accepts only `completed` and `failed`.

Before JCS, `stage_settlements` is projected to entries ordered by ascending numeric `cycle`, then bytewise-ascending canonical `stage` snake-case string; `model_settlements` is ordered by bytewise-ascending canonical lowercase `effect_id`; and `completion_identities` is ordered by bytewise-ascending UTF-8 `completion_id`. Duplicate keys are invalid rather than last-write-wins. This array representation is the only state-hash representation of those maps and avoids non-string JSON keys.

The projection excludes `committed_at`, envelope payload/checksum fields, append-batch IDs, diagnostics, post-commit actions, transient events/sequences, observer state, and runtime caches. `ContextPrepared.context_digest`, settlement fingerprints, completion identities, and terminal result digests remain in the projection alongside the semantic values they validate. Equal valid record prefixes therefore produce equal state hashes independent of model stream chunking, store commit timestamps, or process restart.

#### 11.3.6.1 Conditional kernel-state v2

Every tool-free state remains `state_version = 1`, serializes through `KernelStateHashV1`, and uses the exact existing `kernel-state` schema-1 prefix and hash. Applying the first `EntryAppended` whose assistant message contains a `ToolCallBlock` transactionally upgrades that run to `state_version = 2`; it never rewrites an earlier v1 record or hash.

V2 is SHA-256 under domain `kernel-state`, schema version 2, over a dedicated recursive-explicit-null JCS projection. It contains every v1 field unchanged plus these exact fields after `completion_identities` and before `terminal`:

```rust
pub active_tool_batch: Option<ActiveToolBatch>,
pub tool_calls: Arc<[ToolCallIdentityHashEntryV2]>,
pub tool_settlements: Arc<[ToolSettlementHashEntryV2]>,
pub last_tool_batch: Option<ToolBatchClosed>,

pub struct ToolCallIdentityHashEntryV2 {
    pub tool_call_id: ToolCallId,
    pub cycle: u64,
    pub turn_id: TurnId,
    pub source_message_id: MessageId,
    pub tool_batch_id: Option<ToolBatchId>,
    pub effect_id: Option<EffectId>,
    pub call: ToolCallBlock,
}

pub struct ToolSettlementHashEntryV2 {
    pub effect_id: EffectId,
    pub kind: ToolSettlementKind,
    pub settlement_digest: Digest,
}
```

`ActiveToolBatch` is the exact public state DTO defined by the PR-010 record/state contract below. `tool_calls` sorts by canonical lowercase `tool_call_id`; `tool_settlements` sorts by canonical lowercase `effect_id`. `active_tool_batch.calls` remains assistant source ordered, buffered results retain arrival-derived settlement data, and `result_message_ids` remains finalization ordered. V2 decoding requires `state_version = 2` and all four fields; v1 rejects them as unknown. V2 rejects a missing field, duplicate map key, unsupported state version, impossible cursor/group/status combination, or capacity violation rather than guessing migration state.

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

The v1 maximum `RunRelation.depth` is **16** (`0` for a root). Depth greater than 16 is rejected before commit. Child depth must equal `parent.depth + 1` when a parent relation is present; root relations require `parent_run_id` and `parent_effect_id` both absent; non-root relations require both present and a `root_run_id` that remains stable across the lineage.

`AwaitingExternal` and `AwaitingInteraction` are generic suspension phases. Feature-specific background model, tool, approval, or workflow states are not added to the kernel.

# 12. Journal record design

Public vocabulary for durable truth uses `RecordDraft`, `RecordEnvelope`, and `RecordBody`. Ambiguous Implementation Plan shorthand names `JournalRecord`, `EffectRequest`, and `EffectResult` are non-normative aliases and must not appear in public APIs: map `JournalRecord` → `RecordEnvelope` / `RecordDraft` as appropriate, `EffectRequest` → `EffectRequested`, and `EffectResult` → `EffectCompleted` / `EffectFailed` / `EffectCancelled`.

## 12.1 Envelope and draft

```rust
pub struct RecordDraft {
    pub format_version: u16,
    pub kind_version: u16,
    pub record_id: RecordId,
    pub session_id: SessionId,
    pub lane_id: LaneId,
    pub run_id: Option<RunId>,
    pub timestamp: Timestamp,
    pub derived_event_ids: Arc<[EventId]>,
    pub body: RecordBody,
}

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

`RecordDraft` is the kernel/runtime semantic proposal before store assignment. It carries no sequence, store commit time, payload digest, or checksum. `RecordEnvelope` is the committed durable shape after the store assigns `sequence` (and optional diagnostic `committed_at`) and the protocol codec fills digest/checksum fields. The protocol's canonical-CBOR encoding of a `RecordEnvelope` is the wire/persistence form owned by PR-039; there is no separate public DTO named `CanonicalRecordEnvelope`.

The runtime supplies the semantic `timestamp` and UUIDv7 IDs (including the fixed ordered IDs for durable-derived events) through `TransitionEnv` before decision/commit, and retries preserve them exactly. The store assigns only `sequence` and may add a separate diagnostic `committed_at`; it never rewrites semantic time. `format_version` versions the envelope while `kind_version` versions the selected record body.

`payload_digest` covers canonical-CBOR body bytes under the `record-payload` digest domain. `checksum` covers the canonical envelope fields required for replay—format/kind versions, identifiers, assigned sequence, semantic timestamp, payload digest/body, prior checksum, and derived event IDs—and excludes diagnostic `committed_at`. `previous_checksum` links the session sequence; session metadata and snapshots retain the corresponding head checksum. Loads verify sequence continuity, payload/envelope digests, and the chain before applying records. PR-008 freezes draft/envelope field shapes, versions, ordering, derived-event ID rules, and logical semantic ceilings; PR-039 calculates and verifies digests/checksums and enforces canonical-CBOR byte limits.

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
    ToolCallSettled(ToolCallSettled),
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

Delivery ownership for `RecordBody` variants is staged by logical PR. **PR-008 owns and must materialize** only:

- `RunAccepted`
- `EffectRequested`, `EffectDeferred`, `EffectCompleted`, `EffectFailed`, `EffectCancelled`
- `InteractionRequested`, `InteractionResolved`, `InteractionExpired`, `InteractionCancelled`

**PR-009 additionally owns and materializes**:

- `StageOutcomeRecorded`
- `ContextPrepared`
- `EntryAppended`
- `RunCompleted`
- `RunFailed`

**PR-010 additionally owns and materializes**:

- `ToolBatchOpened`
- `ToolCallSettled`
- `ToolBatchClosed`

All other variants remain reserved in the enum inventory and are owned by later PRs (for example PR-011 limits/cancellation/budget, PR-012 capabilities, and PR-014/PR-039 store/session/lane/snapshot surfaces). An implementation must reject construction of non-owned variants rather than inventing placeholder payloads.

The PR-009 payloads are:

```rust
pub struct StageOutcomeRecorded {
    pub cursor: StageCursor,
    pub disposition: StageDisposition,
    pub settlement_digest: Digest,
}

pub enum StageDisposition {
    Continued,
    ContextPrepared {
        turn_id: TurnId,
        context_digest: Digest,
    },
    ModelRequested {
        turn_id: TurnId,
        model_request_id: ModelRequestId,
        effect_id: EffectId,
    },
    FinalizeAccepted,
    ContinueModel {
        next_cycle: u64,
    },
    Failed {
        error: ErrorDescriptor,
    },
}

pub struct ContextPrepared {
    pub cycle: u64,
    pub turn_id: TurnId,
    pub messages: Arc<[Message]>,
    pub context_digest: Digest,
}

pub struct EntryAppended {
    pub cycle: u64,
    pub turn_id: TurnId,
    pub model_request_id: ModelRequestId,
    pub effect_id: EffectId,
    pub parent_message_id: Option<MessageId>,
    pub message: Message,
}

pub struct RunCompleted {
    pub cycle: u64,
    pub turn_id: TurnId,
    pub model_request_id: ModelRequestId,
    pub effect_id: EffectId,
    pub result_message_id: MessageId,
    pub result_digest: Digest,
}

pub struct RunFailed {
    pub cycle: u64,
    pub turn_id: Option<TurnId>,
    pub model_request_id: Option<ModelRequestId>,
    pub effect_id: Option<EffectId>,
    pub error: ErrorDescriptor,
}
```

`StageOutcomeRecorded.settlement_digest` is the `stage-settlement` schema-1 digest of the dedicated semantic projection in section 11.3.2.1, not of the complete human-readable `StageSettled` input. Required sibling records supply replay fields; reducer-assigned IDs and `ContinueModel.reason` are excluded. Its disposition carries assigned correlations needed for replay validation. `ContextPrepared.context_digest` is the `model-context` v1 digest of the JCS message array and must match `messages`. `EntryAppended.message` must be an assistant message without tool-call blocks in PR-009; `created_at` must equal `TransitionEnv.now`, and its ID must equal the consumed message ID after allocated-ID queues validate. `parent_message_id` is the prior durable message in the model-only linear projection, or `None`. Session/lane conversation-tree `EntryId` and lane-leaf mechanics remain PR-014 scope and are not guessed into this payload.

For successful settlement, `RunCompleted.result_digest` is the matching final `EffectCompleted.output_digest`; `result_message_id` identifies the durable assistant message derived from that normalized output. `RunFailed.error` is the normalized candidate failure accepted by `before_finalize`. Optional correlations are all present for a model failure and may be absent only when an earlier aggregate stage failed before a model request existed.

The exact PR-010 record and replay-state payloads are:

```rust
pub struct AssignedToolCall {
    pub source_index: u32,
    pub group_index: u32,
    pub effect_id: EffectId,
    pub plan: ToolCallPlan,
}

pub struct ToolBatchOpened {
    pub cycle: u64,
    pub turn_id: TurnId,
    pub tool_batch_id: ToolBatchId,
    pub source_message_id: MessageId,
    pub calls: Arc<[AssignedToolCall]>,
    pub continuation: ToolBatchContinuation,
    pub plan_digest: Digest,
}

pub struct ToolCallSettled {
    pub cycle: u64,
    pub turn_id: TurnId,
    pub tool_batch_id: ToolBatchId,
    pub tool_call_id: ToolCallId,
    pub effect_id: EffectId,
    pub message: Message,
    pub settlement_digest: Digest,
    pub synthetic: bool,
    pub error: Option<ErrorDescriptor>,
}

pub enum ToolBatchOutcome {
    ContinueModel,
    Finalize,
    Failed { error: ErrorDescriptor },
}

pub struct ToolBatchClosed {
    pub cycle: u64,
    pub turn_id: TurnId,
    pub tool_batch_id: ToolBatchId,
    pub source_message_id: MessageId,
    pub result_message_ids: Arc<[MessageId]>,
    pub outcome: ToolBatchOutcome,
    pub close_digest: Digest,
}

pub struct ActiveToolBatch {
    pub opened: ToolBatchOpened,
    pub calls: Arc<[ActiveToolCall]>,
    pub current_group: u32,
    pub next_source_index: u32,
    pub result_message_ids: Arc<[MessageId]>,
    pub fatal_error: Option<ErrorDescriptor>,
}

pub struct ActiveToolCall {
    pub assigned: AssignedToolCall,
    pub status: ActiveToolCallStatus,
}

pub enum ActiveToolCallStatus {
    Undispatched,
    Requested { requested: EffectRequested, deferred: Option<EffectDeferred> },
    Buffered {
        result: ToolResultBlock,
        settlement_digest: Digest,
        synthetic: bool,
        error: Option<ErrorDescriptor>,
    },
    Settled {
        result_message_id: MessageId,
        settlement_digest: Digest,
    },
}
```

`ToolBatchOpened.calls` is the complete source-ordered plan with one stable `EffectId` per source call, including synthetic and not-yet-dispatched calls. `source_index` is contiguous from zero. `group_index` is the deterministic partition described in section 11.1.1. `plan_digest` is recomputed from the complete open payload projection before apply mutates state.

`ToolCallSettled.message` is role `Tool`, has exactly one `ToolResultBlock`, no model, empty provider IDs and metadata, and `created_at` equal to its record timestamp. Its result call ID equals the record/planned call ID. `synthetic = false` requires `error = None` and preserves a tool-produced decoded result; `synthetic = true` requires `is_error = true` and `error = Some`. The record finalizes exactly the current `next_source_index`; out-of-order effect records buffer results but cannot produce this record early.

`ToolBatchClosed` is valid only after every source call is `Settled`; its result IDs exactly equal the source-ordered settled messages. `ContinueModel`/`Finalize` must equal the opened continuation. `Failed` requires the batch fatal error. `ToolBatchOpened` and `ToolBatchClosed` derive no public event. `ToolCallSettled` derives ordinal 0 `MessageFinalized` and ordinal 1 `ToolSettled`, both with turn/batch/call/effect correlations and `Internal` sensitivity.

The exact family-discriminated `model-settlement` schema-1 DTOs, external-field exclusions, and command-to-record reconstruction rules are normative in section 11.3.2. In particular, direct and external sources never normalize to one interchangeable shape: source is committed by the fingerprint variant, and non-empty usage, artifacts, and the complete assistant message are reconstructed from `EffectCompleted` plus its required `EntryAppended` sibling.

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

pub enum EffectInput {
    Model {
        request: RawJson,
    },
    Tool {
        call: ToolCallBlock,
    },
    Context {
        request: RawJson,
    },
    Middleware {
        stage: Arc<str>,
        input: RawJson,
    },
    Interaction {
        interaction_id: InteractionId,
        request_digest: Digest,
    },
    Timer {
        due_at: Timestamp,
    },
}

#[serde(rename_all = "snake_case")]
pub enum RetrySafety {
    SafeToRetry,
    IdempotentWithKey,
    AtMostOnce,
    Unknown,
}
```

`EffectInput` must match `EffectRequested.kind`; mismatched combinations are rejected. `EffectInput::Interaction.request_digest` digests the paired `InteractionRequest` body under the `effect-input` domain after RFC 8785 / canonical-CBOR normalization rules for that payload family; the interaction request itself is committed as a sibling `InteractionRequested` record. `input_digest` on `EffectRequested` digests the selected `EffectInput` variant under domain `effect-input`. `RetrySafety` describes external-execution retry policy for the effect; it is distinct from `InvocationRecovery`, which describes whether a component configuration may be recomputed.

Completion records include the normalized output, usage, provider/tool IDs, and retry metadata. Large output needed for replay is staged through `ArtifactStore` and referenced by a digest-bearing `ArtifactRef` before commit; a plain external `BlobRef` is insufficient for authoritative behavior-changing data.

```rust
pub struct EffectCompleted {
    pub effect_id: EffectId,
    pub output_contract: EffectOutputContract,
    pub output: RawJson,
    pub output_digest: Digest,
    pub usage: Option<Usage>,
    pub usage_digest: Option<Digest>,
    pub artifacts: Arc<[ArtifactRef]>,
    pub provider_ids: ProviderIds,
    pub completion_id: Option<Arc<str>>,
    pub reservation_id: Option<BudgetReservationId>,
}

pub struct EffectFailed {
    pub effect_id: EffectId,
    pub output_contract: EffectOutputContract,
    pub error: ErrorDescriptor,
    pub usage: Option<Usage>,
    pub usage_digest: Option<Digest>,
    pub completion_id: Option<Arc<str>>,
}

pub struct EffectCancelled {
    pub effect_id: EffectId,
    pub output_contract: EffectOutputContract,
    pub reason: Option<Arc<str>>,
    pub completion_id: Option<Arc<str>>,
}

pub struct EffectDeferred {
    pub effect_id: EffectId,
    pub handle: ExternalHandleRef,
    pub reconciliation: ReconciliationPolicy,
    pub next_poll_at: Option<Timestamp>,
    pub expires_at: Option<Timestamp>,
    pub output_contract: EffectOutputContract,
}
```

`EffectCompleted.output_contract`, `EffectFailed.output_contract`, and `EffectCancelled.output_contract` must equal the non-optional contract from the originating `EffectRequested` (copied unchanged through any `EffectDeferred`). `output_digest` uses the `effect-output` digest domain. When `usage` is present, `usage_digest` is required and digests the normalized `Usage` value; when `usage` is absent, `usage_digest` must be absent. `artifacts` is empty when no staged artifacts are referenced. Optional `completion_id` supports external idempotency; conflicting reuse with a different normalized outcome fails closed.

```rust
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
}
```

The code block above is the complete concrete PR-009 external outcome enum. `Cancelled` is not a reserved placeholder variant: PR-011 owns adding it and its transition semantics. Until then, strict external-command decoding maps a `cancelled` or any unknown/future discriminant to `invalid_input_payload`; there is no separate typed semantic error for an unmaterialized variant.

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
kernel.apply(committed_batch, next_transient_sequence)
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
```

`ToolBatch` is the runtime-facing executable view. The authoritative `ToolBatchOpened`, `ToolCallSettled`, and `ToolBatchClosed` record shapes are frozen in section 12.2 and are not abbreviated here. `ToolBatchId` is persisted by open/close records and carried by every call event. Each call has its own `ToolCallId` and `EffectId`; deferral, reconciliation, and any later retry keep them frozen.

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

`RunEvent` has exactly two semantic classes: **durable-derived** and **transient**. Public constructors are class-safe and cannot confuse the two. Diagnostics (`Diagnostic` on `Decision`, observer/ops logs, JSONL projections) are a separate non-semantic channel and are never `RunEvent` kinds. Binding-local notifications (language-runtime callbacks, UI widgets, host-only spans) remain binding implementation details outside journal and runtime-event schemas.

```rust
#[serde(rename_all = "snake_case")]
pub enum RunEventClass {
    DurableDerived,
    Transient,
}

#[serde(rename_all = "snake_case")]
pub enum RunEventKind {
    // Durable-derived
    RunAccepted,
    EffectRequested,
    EffectDeferred,
    EffectCompleted,
    EffectFailed,
    EffectCancelled,
    InteractionRequested,
    InteractionResolved,
    InteractionExpired,
    InteractionCancelled,
    MessageFinalized,
    ToolSettled,
    LimitReached,
    RunSuspended,
    RunCompleted,
    RunFailed,
    RunCancelled,
    // Transient
    ModelTextDelta,
    ReasoningDelta,
    ToolProgress,
    QueueDepthWarning,
    ProviderHeartbeat,
}

pub enum RunEventBody {
    // PR-008-owned durable bodies (full payloads)
    RunAccepted(RunAccepted),
    EffectRequested(EffectRequested),
    EffectDeferred(EffectDeferred),
    EffectCompleted(EffectCompleted),
    EffectFailed(EffectFailed),
    EffectCancelled(EffectCancelled),
    InteractionRequested(InteractionRequest),
    InteractionResolved(InteractionResolution),
    InteractionExpired(InteractionExpired),
    InteractionCancelled(InteractionCancelled),
    // PR-009-owned compact public bodies
    MessageFinalized { message_id: MessageId },
    RunCompleted { result_digest: Digest },
    RunFailed { error: ErrorDescriptor },
    // Reserved durable bodies — owning PRs freeze full record/event payloads.
    ToolSettled { tool_call_id: ToolCallId },
    LimitReached { dimension: LimitDimension },
    RunSuspended { reason_code: Option<Arc<str>> },
    RunCancelled { request_id: Option<CancellationRequestId> },
    // Transient bodies
    ModelTextDelta(ModelTextDelta),
    ReasoningDelta(ReasoningDelta),
    ToolProgress(ToolProgress),
    QueueDepthWarning(QueueDepthWarning),
    ProviderHeartbeat(ProviderHeartbeat),
}

pub struct ModelTextDelta {
    pub text: Arc<str>,
}

pub struct ReasoningDelta {
    pub text: Arc<str>,
}

pub struct ToolProgress {
    pub message: Arc<str>,
    pub percent: Option<u8>,
}

pub struct QueueDepthWarning {
    pub depth: u32,
    pub limit: u32,
}

pub struct ProviderHeartbeat {
    pub provider: Arc<str>,
    pub detail: Option<Arc<str>>,
}
```

`RunEvent.kind` and `RunEvent.body` must agree. Durable-derived events require `durable_sequence = Some(source_record_sequence)` and a replay-stable `event_id` taken from the source record's `derived_event_ids`. Transient events require `durable_sequence = None`; their `event_id` values are allocated at emission and are explicitly non-replay-stable. `transient_sequence` is always set and advances for every emitted event on the run stream. Correlation fields (`model_request_id`, `tool_batch_id`, `effect_id`, `tool_call_id`, `turn_id`) are set when applicable to the kind and otherwise `None`. `sensitivity` is mandatory on every event.

Value constructors and deserializers enforce event class, applicable-correlation presence, and sensitivity policy; they do not authenticate caller-supplied identifiers. Authoritative durable model events come only from `Kernel::apply` over committed records and replay-derived pending state. Authoritative transient model events come from the runtime sequencer using the outstanding `PendingModelEffect`. Transport, observer, or binding consumers treat separately decoded events as untrusted until their authenticated source/provenance is established.

For each durable record kind, a versioned ordinal table defines zero or more derived events. Ordinals are dense from zero in table order; `derived_event_ids.len()` must equal the table length for that record kind/version. Replay and every binding reuse those IDs.

### 20.2.1 Derived-event ordinal table (kind_version = 1)

| Record body | Ordinal → `RunEventKind` |
|---|---|
| `RunAccepted` | 0 → `RunAccepted` |
| `EffectRequested` | 0 → `EffectRequested` |
| `EffectDeferred` | 0 → `EffectDeferred` |
| `EffectCompleted` | 0 → `EffectCompleted` |
| `EffectFailed` | 0 → `EffectFailed` |
| `EffectCancelled` | 0 → `EffectCancelled` |
| `InteractionRequested` | 0 → `InteractionRequested` |
| `InteractionResolved` | 0 → `InteractionResolved` |
| `InteractionExpired` | 0 → `InteractionExpired` |
| `InteractionCancelled` | 0 → `InteractionCancelled` |
| `StageOutcomeRecorded` | none |
| `ContextPrepared` | none |
| `EntryAppended` | 0 → `MessageFinalized` |
| `ToolBatchOpened` | none |
| `ToolCallSettled` | 0 → `MessageFinalized`; 1 → `ToolSettled` |
| `ToolBatchClosed` | none |
| `RunCompleted` | 0 → `RunCompleted` |
| `RunFailed` | 0 → `RunFailed` |

PR-008 freezes and implements constructors/fixtures for its rows plus the transient kinds needed to prove class separation (`ModelTextDelta`, `ReasoningDelta`, `ToolProgress`, `QueueDepthWarning`, `ProviderHeartbeat`). PR-009 adds its five rows; PR-010 adds the three tool-record rows. `StageOutcomeRecorded`, `ContextPrepared`, `ToolBatchOpened`, and `ToolBatchClosed` are semantic bookkeeping with no public event and therefore carry empty `derived_event_ids`. Later PRs append rows for their owned record kinds (`LimitReached`, `RunCancelled`, and others) without renumbering existing rows for a given `kind_version`.

For PR-009, `MessageFinalized` takes `turn_id`, `model_request_id`, and `effect_id` from `EntryAppended`; `RunCompleted` and `RunFailed` take their optional/applicable correlations from the terminal payload. Model `EffectRequested`, `EffectDeferred`, `EffectCompleted`, and `EffectFailed` events take the same correlations from the outstanding `PendingModelEffect` established by the matching before-model outcome. Those full model-effect event bodies are `Sensitivity::Confidential`; the compact `MessageFinalized`, `RunCompleted`, and safe-descriptor-only `RunFailed` bodies are `Sensitivity::Internal`. No PR-009 event publishes assistant message content. A later change to these projections or classifications requires compatibility fixtures and threat-model review.

For PR-010, tool `EffectRequested`, `EffectDeferred`, `EffectCompleted`, and `EffectFailed` events take turn/batch/call/effect correlations from the active assigned call and are `Sensitivity::Confidential`. Both events derived from `ToolCallSettled` carry the same correlations and are `Sensitivity::Internal`; `MessageFinalized` contains only its message ID and `ToolSettled` only its call ID. Neither event publishes tool-result content.

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

The final model response yields ordered calls with preallocated unique IDs. `BeforeToolBatch` receives one exact source-ordered `ToolCallPlan` per call. Known tools become validated executable plans; unknown tools become `unknown_tool` synthetic closures. The reducer allocates/persists the batch and every call effect identity before any action, then verifies the schema-1 plan digest during replay.

## 21.2 Execution groups

Rules:

1. Consecutive parallel calls may execute together.
2. A sequential call executes alone and waits for prior work.
3. A barrier waits for all previous calls and blocks subsequent calls until complete.

Only the current group receives `EffectRequested` records/actions. Every direct or external completion is committed immediately in runtime arrival order. Normalized results are buffered in authoritative state; `ToolCallSettled`, tool messages, and their two events are emitted only for the newly contiguous source prefix. A later group is requested only after the current group is terminal. Deferral preserves the call's original identity and blocks closure until its external completion arrives.

Unknown tools and permitted framework failures produce deterministic framework-authored `is_error` results. A `FailRun` call failure prevents later dispatch, drains its already-dispatched group, closes only undispatched calls with `tool_batch_aborted`, and then records a failed batch outcome. PR-010 never emits `EffectCancelled` and never fabricates closure for an active unresolved effect; PR-011 adds those cancellation transitions.
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

`Sensitivity` classifications are the section 7.2 enum (`Public`, `Internal`, `Confidential`, `Secret`, `Credential`). Event subscriptions and observers declare the maximum permitted level and redaction mode.

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
