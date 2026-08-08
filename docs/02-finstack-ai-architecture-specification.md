---
title: "finstack-ai Architecture Specification"
subtitle: "Agent microkernel, extension boundaries, bindings, durability, and deployment model"
author: "Project Draft"
date: "2026-08-07"
---

# Document control

| Field | Value |
|---|---|
| Product | `finstack-ai` |
| Document | Architecture Specification |
| Version | 0.1 |
| Status | Draft for implementation planning |
| Scope | Logical, runtime, data, extension, binding, security, and deployment architecture |
| Related documents | Product Requirements Document; Technical Design Document |

# Executive architecture decision

`finstack-ai` will use an **agent microkernel architecture**.

The microkernel is not a generic plugin operating system. It contains the irreducible semantics required to run an agent consistently: the canonical message model, run and turn state machines, tool-call continuation rules, stable identifiers, event ordering, limits, cancellation, durable effect boundaries, and recovery decisions. It does not perform network, filesystem, database, provider, telemetry, UI, or plugin-discovery work.

External work and policy are supplied through six primary ports:

```text
Model
Toolset
ContextProvider
Middleware
JournalStore
Observer
```

The standard runtime resolves these components once and retains direct handles. Trusted Rust implementations use direct trait calls. Python and JavaScript implementations use coarse-grained adapters. Independently distributed or untrusted extensions use optional WIT/WASM or process isolation.

The same kernel must execute in native Rust and browser WebAssembly. Python is a first-class control and extension surface over the native Rust runtime, not a second implementation of the agent loop.

# 1. Architectural context

## 1.1 System purpose

The system provides a reusable execution foundation for products that need one or more AI agents. It must support minimal embedded use and production service use without requiring the same deployment topology.

Representative products built on the framework include:

- coding agents;
- research assistants;
- financial analysis and portfolio agents;
- document ingestion and review workflows;
- customer or employee assistants;
- browser-based private agents;
- long-running human-in-the-loop processes; and
- multi-channel or multi-agent services.

## 1.2 External actors

```text
+---------------------+       +---------------------+
| Rust application    |       | Python application  |
+----------+----------+       +----------+----------+
           |                             |
           +-------------+---------------+
                         |
                +--------v---------+
                | finstack-ai SDK  |
                +--------+---------+
                         |
                +--------v---------+
                | Agent microkernel|
                +---+----+----+----+
                    |    |    |
         +----------+    |    +----------------+
         |               |                     |
+--------v------+ +------v-------+     +-------v--------+
| Models        | | Toolsets     |     | Stores/Policy  |
+---------------+ +--------------+     +----------------+

Browser/Node applications use the same kernel compiled to WASM.
Remote clients talk to a separate server application, not directly to the kernel.
Isolated plugins are mediated by an optional plugin host.
```

## 1.3 Architectural drivers

The primary drivers, in priority order, are:

1. consistent semantics across Rust, Python, and WASM;
2. a concise and understandable core;
3. low overhead for native and Rust-backed binding paths;
4. extension without kernel modification;
5. deterministic testing and durable recovery;
6. optional isolation for third-party code;
7. small minimal artifacts and dependency graphs;
8. idiomatic developer experience in each language; and
9. long-term compatibility of records and plugin contracts.

# 2. Architecture principles and constraints

## 2.1 Kernel invariants

The following are architecture invariants rather than implementation preferences:

1. The kernel performs no external I/O.
2. The kernel does not depend on an async runtime.
3. The kernel contains no concrete model, tool, database, channel, gateway, telemetry, or UI implementation.
4. The canonical model/tool continuation loop is kernel behavior.
5. Every recoverable external effect has a stable identifier.
6. Durable records are append-only.
7. Observers cannot alter execution.
8. Behavior-changing extension output is normalized before entering kernel state.
9. No unbounded queue exists in the standard runtime.
10. Native components are resolved once and called directly.
11. Bindings do not own independent run state machines.
12. Adding an ordinary implementation of one of the six ports requires zero kernel source changes.

## 2.2 Layer dependency rule

Dependencies point inward:

```text
Applications / products
        |
Bindings and servers
        |
SDK and composition
        |
Runtime and adapters
        |
Kernel
```

The kernel cannot import any outward layer. Provider/tool/store packages depend on the API contracts and runtime adapter interfaces, not vice versa.

## 2.3 Mechanism versus policy

| Mechanism owned centrally | Policy supplied externally |
|---|---|
| A tool batch exists and has an order | Which tools are available |
| A tool call may require approval | Which calls require approval |
| Context has a bounded budget | What external context is retrieved |
| A model request has a stable effect ID | Which provider/model handles it |
| A retry transition is possible | Retry count, delay, and classification |
| A run records usage | Cost tables and spending policy |
| A session may have lanes | How external threads map to lanes |

# 3. Top-level architecture

## 3.1 Logical layers

```text
+------------------------------------------------------------------+
| Applications                                                     |
| CLI | TUI | server | gateway | coding agent | research product   |
+-------------------------------+----------------------------------+
                                |
+-------------------------------v----------------------------------+
| Language and transport surfaces                                  |
| Rust SDK | Python/PyO3 | JS/wasm-bindgen | remote protocol       |
+-------------------------------+----------------------------------+
                                |
+-------------------------------v----------------------------------+
| Composition layer                                                |
| AgentBuilder | AgentSpec | Registrar | Capability resolver        |
+-------------------------------+----------------------------------+
                                |
+-------------------------------v----------------------------------+
| Runtime layer                                                    |
| effect driver | schedulers | batching | adapters | resource pools|
+-------------------------------+----------------------------------+
                                |
+-------------------------------v----------------------------------+
| Agent microkernel                                                |
| state | decisions | records | effects | events | recovery rules   |
+-------------------------------+----------------------------------+
                                |
+-------------------------------v----------------------------------+
| Extension implementations                                       |
| models | toolsets | context | middleware | stores | observers     |
+------------------------------------------------------------------+
```

## 3.2 Deployment-time component modes

The logical architecture does not force one physical topology.

### Native embedded mode

All selected components are linked into one process. Calls use Rust traits and shared handles.

### Python embedded mode

The Rust runtime is loaded as a Python extension module. Rust-backed components remain native. Python-defined components are represented by adapters that re-enter Python only for explicit calls.

### Browser WASM mode

The kernel and a WASM-compatible runtime compile into one WebAssembly module. JavaScript supplies host adapters for model calls, tools, persistence, timers, and UI events.

### Native host with WASM plugins

A native application links the standard runtime and optional Wasmtime host. WIT components provide isolated toolsets or context providers.

### Agent server mode

A separate application owns authentication, routing, client connections, and deployment concerns. It invokes the same SDK and exposes sessions through a transport-neutral protocol.

# 4. Microkernel boundary

## 4.1 Responsibilities inside the kernel

The kernel owns:

- canonical content and message types;
- conversation entry semantics;
- session, lane, run, turn, effect, and call identifiers;
- immutable agent/run policy values used by state decisions;
- state machines for runs and tool batches;
- standard continuation behavior;
- durable record definitions and validation;
- transient and durable event ordering;
- limit counters and terminal limit decisions;
- cancellation reconciliation;
- suspension and resume rules;
- capability activation state;
- recovery from normalized journal records; and
- deterministic decision and apply operations.

## 4.2 Responsibilities outside the kernel

The kernel does not own:

- HTTP or SDK requests;
- provider authentication;
- tool business logic;
- filesystem and process access;
- database drivers or transactions;
- vector retrieval and embeddings;
- retry sleeping or clocks;
- human approval interfaces;
- telemetry export;
- configuration file parsing;
- plugin discovery and installation;
- WIT/WASI runtime setup;
- Python or JavaScript object conversion;
- CLI, TUI, gateway, or channel behavior; or
- workflow engine integration.

## 4.3 Why the default run loop remains in the kernel

The default loop establishes history and recovery invariants that must not vary by extension:

```text
model response with tool calls
    -> record assistant response
    -> request and settle complete tool batch
    -> append one result for every requested call
    -> continue model request unless policy ends the run
```

Allowing separate orchestrator plugins to redefine this machinery would make event order, cancellation, persistence, and binding behavior inconsistent. Higher-level workflow engines may compose multiple runs, but one run follows the canonical machine.

# 5. Core component architecture

## 5.1 `finstack-ai-kernel`

A deterministic library containing no external I/O or async runtime. It exposes domain types and a decision/apply API.

Primary modules:

```text
content
message
ids
agent_spec_core
session
lane
run
turn
tool
model
limits
effects
records
events
reducer
recovery
validation
```

## 5.2 `finstack-ai-runtime`

The standard native runtime. It is responsible for effect execution and coordination.

Primary responsibilities:

- Tokio task management;
- model stream consumption;
- tool scheduling;
- middleware invocation;
- context-provider fan-out;
- journal commit coordination;
- event batching and backpressure;
- timers and deadlines;
- cancellation propagation;
- resource initialization and shutdown; and
- adapter integration.

## 5.3 `finstack-ai-sdk`

The ergonomic composition layer exposed to most Rust users.

Primary responsibilities:

- `AgentBuilder` and `Agent`;
- registries and typed references;
- `CapabilitySpec`;
- serializable `AgentSpec`;
- component resolution;
- default policies;
- result decoding;
- extension registrar;
- test helpers; and
- compatibility facade over kernel/runtime types.

## 5.4 `finstack-ai-protocol`

Versioned types and encodings shared by journal backends and optional remote clients.

It must remain separate from the runtime so stores and clients can work with records without importing Tokio or provider code.

## 5.5 Binding crates

```text
finstack-ai-python
finstack-ai-wasm
```

These crates translate idiomatic host-language APIs to the SDK and retain Rust/WASM handles for long-lived objects.

## 5.6 Optional plugin host

```text
finstack-ai-plugin-host
finstack-ai-wit
```

The host imports WIT-defined components and adapts them to native port traits. Wasmtime is never a kernel dependency.

# 6. The six primary ports

## 6.1 Model

The model port normalizes provider requests and streaming outputs.

Architectural responsibilities:

- advertise model capabilities;
- accept a complete normalized request;
- emit ordered normalized stream items;
- preserve provider-specific opaque continuation data;
- receive stable request/effect identifiers;
- support cancellation where possible; and
- classify failures without deciding framework retry policy.

The model port is deliberately not a generic provider registry. Registries and model selection live in the composition/runtime layers.

## 6.2 Toolset

A toolset owns multiple related tools. It publishes a catalog and dispatches calls.

Architectural responsibilities:

- schema and metadata publication;
- input validation adapter;
- call dispatch;
- progress stream support;
- side-effect and retry-safety declaration;
- output size enforcement; and
- idempotency-key handling.

Toolsets are the normal unit of packaging and permission assignment.

## 6.3 ContextProvider

A context provider contributes external context before model calls.

Examples:

- semantic memory;
- repository metadata;
- retrieved documents;
- user profile;
- current portfolio state;
- system reminders; and
- environment summaries.

The provider receives an explicit budget and returns typed items with provenance and estimates. It does not mutate message history directly.

## 6.4 Middleware

Middleware alters execution at a small set of stable stages. It is used for:

- approvals;
- guardrails;
- request adaptation;
- result transformation;
- tool filtering;
- cost or policy enforcement;
- compaction decisions; and
- application-level validation.

Middleware receives immutable stage inputs and returns a normalized outcome. It cannot retain direct mutable references to kernel state.

## 6.5 JournalStore

A journal store persists batches of versioned records and loads snapshots/tails.

The store owns storage mechanics, not record meaning. The kernel/runtime own sequencing rules and record validation.

Implementations may include:

- in-memory;
- JSONL/file;
- SQLite;
- PostgreSQL;
- IndexedDB; and
- workflow-engine-backed storage.

## 6.6 Observer

Observers receive immutable event batches. They support logging, metrics, tracing, billing capture, audit capture, or test assertions.

Observers are explicitly unable to block or mutate run decisions unless configured as a backpressure-sensitive sink outside the kernel.

# 7. Composition architecture

## 7.1 Registrar

An extension registers concrete implementations through a typed registrar:

```text
register_model
register_toolset
register_context_provider
register_middleware
register_store
register_observer
```

Registrations include:

- namespaced identifier;
- semantic version metadata;
- implementation handle or factory;
- serializable configuration schema reference;
- health/lifecycle metadata; and
- optional aliases.

## 7.2 Agent resolution

Agent construction has two phases.

### Registration phase

Extensions populate registries. No agent-specific selection occurs.

### Resolution phase

`AgentSpec` references are resolved into direct handles and an immutable `ResolvedAgent`.

```text
AgentSpec
  + Registry
  + Environment configuration
  -> validation
  -> capability expansion
  -> middleware ordering
  -> tool catalog assembly
  -> model compatibility checks
  -> ResolvedAgent
```

After resolution, ordinary run execution does not traverse string-based registries.

## 7.3 Capabilities

A capability is declarative. It references registered components rather than embedding arbitrary behavior.

```text
CapabilitySpec
  id
  description
  instructions
  toolset references
  context-provider references
  middleware references
  activation mode
  optional compatibility constraints
```

Capabilities may be:

- always active;
- activated by application API;
- activated by model request through a small internal activation tool; or
- disabled.

## 7.4 Capability activation boundary

Activation changes become durable lane state and are applied at safe run checkpoints. The framework favors append-only changes to preserve prompt-cache stability.

# 8. Run lifecycle architecture

## 8.1 Normal lifecycle

```text
Application starts run
        |
        v
[RunAccepted record committed]
        |
        v
before-run middleware
        |
        v
context providers + prepare-context middleware
        |
        v
[ModelEffectRequested record committed]
        |
        v
model stream -> transient progress events
        |
        v
[ModelEffectCompleted + assistant entry committed]
        |
        +------ no tools ------> output processing -> completed
        |
        v
before-tool middleware / approval
        |
        v
[ToolBatchRequested record committed]
        |
        v
parallel/sequential tool execution
        |
        v
[Tool completion records + result entries committed]
        |
        v
checkpoint -> next model turn or completion
```

## 8.2 Decision/commit/effect cycle

The runtime never executes a recoverable effect solely because the in-memory kernel requested it. It follows this ordering:

1. Kernel evaluates normalized input and proposes durable records plus actions.
2. Runtime validates the proposal against the current state version.
3. Journal store atomically appends the durable record batch.
4. Kernel applies the committed records to in-memory state.
5. Runtime publishes resulting public events.
6. Runtime executes actions whose prerequisite records are committed.
7. Action result returns as normalized input and begins the next cycle.

This avoids mutating authoritative state before the store confirms the transition.

## 8.3 Transient streaming

Provider text/reasoning deltas are transient by default. They are:

- assigned ordering metadata;
- published to live consumers;
- coalesced for bindings;
- optionally observed or captured; and
- discarded on crash.

The final normalized model response is durable. Recovery retries or reconciles an interrupted request according to provider capabilities and run policy.

# 9. State model

## 9.1 Agent state

An `Agent` is immutable after resolution except for explicitly managed resource handles. Run-specific or lane-specific configuration does not mutate the shared agent definition.

## 9.2 Session state

A session contains:

- immutable conversation entries;
- lane definitions and current lane leaves;
- session metadata and labels;
- durable capability activations;
- lane operation journals; and
- optional snapshots.

## 9.3 Conversation entries

Entries use parent identifiers rather than an in-place mutable list. This permits branching without copying history.

```text
entry A -> entry B -> entry C
                 \-> entry D -> entry E
```

A lane points to one leaf. Appending an entry advances only that lane.

## 9.4 Lanes

Every session has `main`. Additional lanes may represent:

- a Slack or email thread;
- a subagent;
- parallel research work;
- a branch/fork; or
- an application-defined work stream.

Rules:

- at most one active operation per lane;
- different lanes may execute concurrently;
- a lane’s config view is derived from entries and durable facts visible on its path;
- lane names are stable application keys; and
- operations on one lane cannot silently move another lane’s leaf.

## 9.5 Run and turn state

A run owns its current phase, limit counters, active effects, pending tool calls, usage, and terminal result. A turn owns one model response and its complete tool batch.

# 10. Journal and recovery architecture

## 10.1 Durable record categories

Records fall into these categories:

- session/lane metadata;
- operation acceptance and completion;
- context preparation decisions;
- model effect request/completion;
- assistant message entries;
- tool batch and call request/completion;
- approval request/decision;
- capability activation;
- timer request/firing;
- cancellation request/reconciliation;
- limit reached;
- run result/failure; and
- snapshot marker.

## 10.2 Stable effect identity

Every external effect receives an `EffectId` derived or allocated before execution. The runtime passes it to the implementation as the primary idempotency key.

## 10.3 Recovery classification

On restore, each requested effect is classified as:

- completed: durable completion exists;
- not started: safe to execute;
- possibly started: implementation-specific reconciliation required;
- retryable: re-execute with same idempotency key;
- suspended: await an external completion or decision;
- non-repeatable uncertainty: stop and require operator/application resolution.

## 10.4 Exactly-once statement

The architecture guarantees exactly-once **record application** within a valid journal sequence. It does not guarantee exactly-once arbitrary external side effects. It supplies stable identifiers and recovery hooks so implementations can provide stronger guarantees where supported.

## 10.5 Snapshots

Snapshots are performance optimizations, not alternate truth. A snapshot includes the last applied record sequence and a checksum/version. The store must still retain enough record history for the configured retention and audit policy.

# 11. Middleware architecture

## 11.1 Stable stages

The first major version supports seven behavior-changing stages:

```text
before_run
prepare_context
after_context / before_model (represented as before_model)
after_model
before_tool_batch
after_tool_batch
after_run
```

For a concise public API, `prepare_context` and `before_model` are separate because context retrieval and final request shaping have different budgets and durability implications.

## 11.2 Ordering

Ordering is resolved once using:

1. fixed priority tier;
2. numeric priority;
3. declared `before`/`after` constraints; and
4. registration order as final tiebreaker.

Cycles are construction errors. Run execution never topologically sorts middleware.

## 11.3 Middleware outcomes

Outcomes are structured values such as:

- continue unchanged;
- replace normalized request/result;
- add instructions/context;
- filter tools;
- request approval;
- request retry;
- end run with result;
- fail run; or
- suspend.

Arbitrary mutation of kernel state is prohibited.

## 11.4 Replay safety

A middleware stage that influences durable behavior must return a serializable outcome. On recovery, the recorded outcome may be reapplied without rerunning middleware unless policy explicitly marks it replay-safe and recomputable.

# 12. Concurrency and scheduling

## 12.1 Concurrency domains

Concurrency is managed at several levels:

- sessions may run concurrently;
- lanes within a session may run concurrently;
- one lane has at most one active operation;
- tool calls within a batch may run concurrently;
- context providers may run concurrently when independent;
- observers may consume asynchronously; and
- model streams are one active stream per model effect.

## 12.2 Tool scheduling

Each tool declares an execution mode:

- `parallel`;
- `sequential`; or
- `barrier`.

A batch is partitioned into safe execution groups while preserving model source order for durable result entries.

## 12.3 Backpressure

Queue categories use different policies:

| Queue | Default policy |
|---|---|
| Durable transition input | Block or fail explicitly; never drop |
| Model stream progress | Coalesce; optionally drop intermediate deltas |
| Tool progress | Coalesce/drop intermediate progress |
| Public completion events | Block within bounded deadline; never silently drop |
| Observer queue | Configurable block, drop-progress, spill, or disconnect |
| Python/WASM event batches | Bounded batches with backpressure signal |

## 12.4 Cancellation propagation

Cancellation flows from application to runtime, then to active model/tool adapters. The kernel records cancellation intent and determines reconciliation. A provider or tool that ignores cancellation remains subject to deadline and result-discard rules.

# 13. Python architecture

## 13.1 Ownership model

Python objects hold opaque references to Rust-owned agents, sessions, lanes, and runs.

```text
Python Agent
    -> Arc<ResolvedAgent>
Python Session
    -> Arc<SessionRuntime>
Python Run
    -> RunHandle + event receiver
```

Python does not receive or reconstruct the entire state machine for ordinary operations.

## 13.2 Rust-backed fast path

For Rust-backed model, toolsets, middleware, and store:

```text
Python call start
    -> enter Rust once
    -> Rust runtime executes run
    -> event batches cross as needed
    -> final result crosses once
```

Rust-only waits and work release the GIL.

## 13.3 Python callback path

A Python implementation is wrapped in a Rust adapter. The adapter:

- acquires the interpreter only for the callback;
- converts one normalized request;
- invokes a sync or async callable;
- converts one result or batch; and
- releases Python before resuming native orchestration.

Callbacks are prohibited at per-token middleware granularity in the standard API.

## 13.4 Pydantic integration

The Pydantic adapter is an outer-layer concern:

- derive schema once;
- cache a `TypeAdapter` or equivalent validator;
- pass canonical JSON Schema to Rust;
- validate raw JSON only at the binding boundary; and
- materialize final Python objects lazily.

The kernel never imports or assumes Pydantic.

## 13.5 Python async integration

The binding provides awaitables and async iterators backed by runtime futures and bounded channels. It must detect misuse from incompatible event-loop contexts and surface a clear error rather than nesting runtimes unsafely.

# 14. Browser and JavaScript WASM architecture

## 14.1 Split runtime

The WASM build includes:

- the same deterministic kernel;
- a single-threaded or host-compatible driver;
- adapters that represent effects as JavaScript promises; and
- batched event export.

It excludes native sockets, filesystem, database drivers, and Wasmtime.

## 14.2 Persistent handles

JavaScript receives object handles rather than serialized state snapshots for each call.

## 14.3 Host effects

JavaScript adapters implement model, toolset, context, store, clock, and observer behavior. A host call receives a normalized payload and returns a promise or stream abstraction.

## 14.4 Worker topology

A recommended browser topology runs the WASM engine in a Web Worker. The UI thread communicates through message batches. This prevents model/tool orchestration and record replay from blocking rendering.

## 14.5 Browser persistence

IndexedDB is implemented as a host `JournalStore` adapter. The kernel remains storage-neutral.

# 15. Isolated plugin architecture

## 15.1 Scope

The first isolated ABI focuses on coarse-grained capabilities with clear security boundaries:

- toolsets; and
- context providers.

Model providers may be added after streaming, credentials, network policy, and cancellation semantics are proven. Middleware is deferred because a broad behavior-changing ABI is harder to stabilize safely.

## 15.2 Host architecture

```text
Native Runtime
    |
WasmToolsetAdapter / WasmContextAdapter
    |
Plugin Host
    |
Wasmtime component + WASI permissions
```

The adapter implements the same native port trait seen by the runtime.

## 15.3 Permissions

Plugin manifests request capabilities such as:

- outbound HTTP to allowed hosts;
- read-only access to selected virtual directories;
- write access to selected virtual directories;
- scoped secret handles;
- clock/random access;
- blob read/write handles; and
- host-provided memory/context services.

No ambient environment or filesystem is inherited.

## 15.4 Payload strategy

WIT records carry stable metadata and handles. Dynamic tool arguments and schemas cross as UTF-8 JSON bytes or blob handles. Large binary content crosses by reference, not embedded copies.

## 15.5 Plugin lifecycle

Plugins support load, initialize, catalog, call, health, and shutdown. Long-lived listener/channel plugins are outside the first ABI and should use a process/application adapter until lifecycle semantics are mature.

# 16. Data architecture

## 16.1 In-memory representation

Native structs are used internally. `Bytes` or shared immutable buffers are favored for raw JSON and media references.

## 16.2 Interchange formats

| Purpose | Format |
|---|---|
| Agent specs and schemas | JSON; optional YAML frontend |
| Tool arguments and structured output | UTF-8 JSON bytes |
| Journal/remote protocol | Versioned CBOR envelope, subject to ADR confirmation |
| Diagnostic export | JSON/JSONL |
| WIT dynamic payloads | UTF-8 JSON bytes plus typed WIT metadata |
| Media and large results | Blob references |

## 16.3 Blob model

Large content is represented as:

```text
BlobRef
  id
  media_type
  length
  digest
  optional name
  optional metadata
```

Blob stores are application/runtime services rather than kernel ports in the initial design. Blob references may be resolved by tools, context providers, or server layers.

# 17. Security architecture

## 17.1 Trust levels

| Extension mode | Trust assumption |
|---|---|
| Native Rust | Fully trusted process code |
| Python callback | Fully trusted process code |
| JavaScript host adapter | Trusted by the containing application |
| WASM component | Untrusted by default; capability scoped |
| External process | Isolated but authenticated and permission scoped |

## 17.2 Security enforcement boundary

The kernel tracks security-relevant metadata and approval state but does not claim to sandbox. Enforcement occurs in:

- tool implementations;
- middleware policy;
- WASI/plugin host;
- process/container boundaries; and
- application authentication/authorization.

## 17.3 Secret handling

Secrets are represented by scoped references where possible. They are resolved inside trusted provider/tool/plugin adapters and are excluded from ordinary event and journal payloads.

## 17.4 Auditability

Security decisions such as approvals, policy denials, permission grants, and plugin identity are durable or observable with stable identifiers and redaction controls.

# 18. Fault model

## 18.1 Kernel errors

Invalid state transitions, corrupt record sequences, identifier mismatches, and invariant failures are terminal for the affected run or session and produce explicit error codes.

## 18.2 External effect failures

Model/tool/context/store/middleware failures are normalized into categories:

- retryable;
- validation/user-correctable;
- permission denied;
- cancelled;
- deadline exceeded;
- unavailable;
- non-repeatable uncertain; and
- terminal.

## 18.3 Store failures

A failed durable append prevents the associated state transition and effect execution. A runtime that cannot confirm store state faults the affected lane or session rather than guessing.

## 18.4 Observer failures

Observers may be configured as best-effort or required. Best-effort observer failure cannot fail a run. Required audit sinks may apply backpressure or fail before execution according to application policy.

## 18.5 Consumer disconnection

A disconnected UI or event consumer does not automatically cancel a run unless the application selected consumer-coupled cancellation. Durable runs continue or suspend independently.

# 19. Observability architecture

## 19.1 Event hierarchy

Events include correlation fields for:

```text
session_id
lane_id
run_id
turn_id
request_id
tool_batch_id
tool_call_id
effect_id
sequence / transient order
```

## 19.2 Separation of events and records

- Records are durable truth.
- Events are public observations derived from records and transient progress.
- Observer-specific spans/metrics are projections.

This prevents telemetry schemas from becoming the persistence schema.

## 19.3 Instrumentation extension

OpenTelemetry and structured logging are separate observer packages. They consume batches and may enrich them with process-level data without modifying kernel events.

# 20. Versioning and compatibility

## 20.1 Rust API

Crates follow semantic versioning. The pre-1.0 phase may evolve quickly, but breaking changes must be documented in release notes and conformance fixtures.

## 20.2 Journal schema

Every record envelope contains format version and record kind version. Unknown optional fields are retained or ignored according to documented rules. Unknown record kinds are not silently skipped when they affect state reconstruction.

## 20.3 Python and JavaScript APIs

Binding APIs are versioned independently but tied to one semantic engine version. Stable error codes and event type names are treated as public compatibility surfaces.

## 20.4 WIT

WIT packages are versioned by major interface directory/package version. Breaking changes create a new world/package version. The host may support multiple major versions through adapters.

## 20.5 AgentSpec

`AgentSpec` includes a schema version and references components by namespaced ID and compatibility constraint. Resolved lock information may capture exact versions.

# 21. Performance architecture

## 21.1 Hot-path design

The hot path uses:

- resolved `Arc` handles;
- native structs;
- shared byte buffers;
- cached schemas;
- bounded channels;
- event coalescing;
- provider/client pooling; and
- no registry lookups per token or tool call beyond local tool dispatch.

## 21.2 Allocation control

The kernel avoids copying complete histories on every turn. Context assembly uses shared entries and views, then materializes provider payloads only when required by the selected model implementation.

## 21.3 Binding control

Python and WASM receive event batches. Raw JSON can be exposed lazily. Large media remains behind blob references.

## 21.4 Measurement boundaries

Benchmarks report separately:

- kernel decision/apply;
- runtime scheduling;
- provider/tool adapter overhead;
- FFI conversion;
- host callback time;
- storage commits;
- remote/WIT serialization; and
- external I/O.

# 22. Dependency governance

## 22.1 Kernel forbidden dependencies

CI rejects direct or transitive kernel dependencies on:

```text
tokio / async-std
reqwest / hyper
rusqlite / sqlx
wasmtime / wasmer
pyo3 / wasm-bindgen
clap / ratatui
opentelemetry exporters
provider SDKs
OS sandbox libraries
```

## 22.2 Architecture tests

Architecture tests verify:

- no concrete provider/tool/store imports in kernel/runtime core modules;
- no central provider or tool match list in the kernel;
- no task-local request context in public contracts;
- no unbounded channels;
- no per-token host-language callback API;
- minimal bundle build remains valid; and
- adding fixture extensions does not modify kernel code.

# 23. Deployment topologies

## 23.1 Minimal embedded

```text
Application binary
  finstack-ai-kernel
  finstack-ai-runtime
  one model
```

## 23.2 Local coding/research agent

```text
Application
  native runtime
  local/remote model
  filesystem + shell toolsets
  SQLite store
  TUI or desktop UI
```

## 23.3 Python service

```text
Python web/service layer
  finstack_ai native module
  Rust-backed providers/tools/store
  selected Python domain tools
```

## 23.4 Browser

```text
Web UI
  Web Worker
    finstack-ai WASM
    JS provider/store adapters
```

## 23.5 Multi-tenant server

```text
Gateway/auth/router process
  Agent service
    session registry
    native runtime
    provider pools
    PostgreSQL store
    observers
```

## 23.6 Isolated plugin host

```text
Agent service
  optional Wasmtime host
    signed/scoped WIT components
```

# 24. Architecture evolution

## 24.1 First implementation slice

The first slice proves the semantic center rather than the ecosystem:

- kernel state and records;
- standard model/tool loop;
- native runtime;
- one provider;
- one toolset;
- in-memory journal;
- Rust/Python/WASM conformance.

## 24.2 Second slice

- SQLite;
- recovery fault matrix;
- approvals;
- filesystem/shell batteries;
- session branching foundation;
- structured observers.

## 24.3 Third slice

- WIT toolset/context plugin host;
- remote protocol;
- additional providers/stores;
- workflow adapters;
- richer capability activation.

# 25. Architecture decision summary

| ADR | Decision |
|---|---|
| ADR-001 | The foundation is an agent microkernel, not a generic plugin host |
| ADR-002 | The canonical model/tool continuation loop is kernel behavior |
| ADR-003 | Kernel decisions are deterministic and external work is represented as effects |
| ADR-004 | Durable transitions commit records before recoverable effects execute |
| ADR-005 | Six primary extension ports are sufficient for the first major version |
| ADR-006 | Trusted native extensions use direct Rust calls after one-time resolution |
| ADR-007 | Python and WASM are bindings to the same engine, not separate runtimes |
| ADR-008 | Capabilities are declarative composition, distinct from executable extensions |
| ADR-009 | Observers are immutable and separate from behavior-changing middleware |
| ADR-010 | WASM isolation is optional and kept outside the kernel |
| ADR-011 | Native dynamic-library plugins are not a supported primary ABI |
| ADR-012 | Sessions use immutable entries and lane identifiers from the initial data model |
| ADR-013 | Exactly-once external side effects are not claimed; stable idempotency keys are provided |
| ADR-014 | Remote client protocol and plugin protocol remain logically distinct |

# 26. Requirements traceability

| Requirement family | Architectural realization |
|---|---|
| FR-KRN | Kernel domain, state machines, reducer, record/event schemas |
| FR-RT | Standard runtime effect driver, schedulers, batching, backpressure |
| FR-EXT | Registrar, registry, resolved agent, extension lifecycle |
| FR-CAP | CapabilitySpec, activation state, resolution checkpoints |
| FR-MDL | Model port and provider packages |
| FR-TLS | Toolset port, scheduler, schemas, idempotency metadata |
| FR-CTX | Context pipeline and explicit budgets |
| FR-MW | Seven-stage normalized middleware chain |
| FR-DUR | Journal, commit-before-effect cycle, recovery, session/lane model |
| FR-OBS | Immutable event batches and observer adapters |
| FR-PY | PyO3 ownership/adapters/batching architecture |
| FR-WASM | Shared kernel, host promises, handles, worker topology |
| FR-PLG | Optional WIT packages and Wasmtime adapter host |
| FR-SPEC | AgentSpec, namespaced configuration, version resolution |

# 27. Architecture review checklist

A change is consistent with this architecture only when the following questions are answered satisfactorily:

1. Does it change universal agent semantics or merely add policy/integration?
2. Can it be implemented through an existing port?
3. Does it introduce external I/O into the kernel?
4. Does it add a language crossing to the hot path?
5. Does it require durable output, and if so, what record captures it?
6. Can recovery determine whether its effects completed?
7. Does it preserve the same behavior in native Rust and WASM?
8. Does it require a new public middleware stage?
9. Does it create an unbounded queue or large payload copy?
10. Can the minimal bundle still omit it completely?
