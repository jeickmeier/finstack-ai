---
title: "finstack-ai Architecture Specification"
subtitle: "Agent microkernel, extension boundaries, bindings, durability, and deployment model"
author: "finstack-ai project"
date: "2026-08-08"
---

# finstack-ai Architecture Specification

# Document control

| Field | Value |
|---|---|
| Product | `finstack-ai` |
| Document | Architecture Specification |
| Version | 0.6 |
| Status | Pre-implementation architecture baseline |
| Scope | Logical, runtime, data, extension, binding, security, and deployment architecture |
| Related documents | Engineering Standards v0.4; Product Requirements Document v0.7; Technical Design v0.8; Implementation Plan v0.8; Security and Threat Model v0.4 |

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
9. long-term compatibility of records and plugin contracts;
10. durable lineage and generic suspension across nested or externally completed work.

# 2. Architecture principles and constraints

## 2.1 Kernel invariants

The following are architecture invariants rather than implementation preferences:

1. The kernel performs no external I/O.
2. The kernel does not depend on an async runtime.
3. The kernel contains no concrete model, tool, database, channel, gateway, telemetry, or UI implementation.
4. The canonical model/tool continuation loop is kernel behavior.
5. Every recoverable external effect has a stable identifier.
6. Durable records are append-only.
7. Every accepted run records explicit root/parent relation metadata.
8. Deferred work preserves its original `EffectId`; feature-specific background-job state machines are prohibited.
9. Human/external input uses generalized typed interactions; approval is a profile, not a separate durable mechanism.
10. No terminal run record is committed before `before_finalize` settles.
11. Context compaction changes only the model-visible projection through `before_model` middleware; canonical conversation history remains immutable.
12. Observers cannot alter execution.
13. Behavior-changing extension output is normalized before entering kernel state.
14. No unbounded queue exists in the standard runtime.
15. Native components are resolved once and called directly.
16. Bindings do not own independent run state machines.
17. Adding an ordinary implementation of one of the six ports requires zero kernel source changes.

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

The kernel and a WASM-compatible runtime compile into one WebAssembly module. JavaScript supplies host adapters for model calls, tools, persistence, timers, and UI events. The package also offers an optional OpenAI-compatible fetch/SSE adapter, but host interfaces remain the contract.

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
- run lineage and relation validation;
- generic effect deferral and external-completion decisions;
- typed interaction request/resolution state;
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

The crate has a target-neutral contract/core surface and no default driver feature. Native facade builds enable the `native-tokio` driver. Browser-WASM builds select the local `wasm-host` surface with facade defaults disabled, so Tokio and native-only I/O adapters never enter the WASM dependency graph. This is one logical runtime contract and semantic engine, not a second JavaScript run loop.

Primary responsibilities:

- ownership of the six object-safe port contracts used for effect execution;
- Tokio task management;
- model stream consumption;
- tool scheduling;
- middleware invocation;
- context-provider fan-out;
- journal commit coordination;
- event batching and backpressure;
- timers and deadlines;
- cancellation propagation;
- external-effect completion and interaction routing;
- optional budget-scope aggregation through an application-supplied `BudgetLedger`;
- scoped artifact/blob routing through an application-supplied `ArtifactStore` service;
- resource initialization and shutdown; and
- adapter integration.

## 5.3 `finstack-ai` (SDK/facade crate)

The ergonomic composition layer exposed to most Rust users. Its Cargo package is `finstack-ai`, its Rust library/import name is `finstack_ai`, and there is no separate public `finstack-ai-sdk` package.

The facade owns target-driver feature selection and declares its runtime dependency with `default-features = false`. Its default `native-tokio` feature passes through to `finstack-ai-runtime/native-tokio`; its non-default `wasm-host` feature passes through to `finstack-ai-runtime/wasm-host`. Browser bindings depend on the facade with default features disabled and `wasm-host` enabled. Target checks reject native Tokio/I/O dependencies or simultaneous native/host drivers in the browser-WASM graph.

Primary responsibilities:

- `AgentBuilder` and `Agent`;
- ergonomic adapters and public re-exports of runtime port contracts;
- registries and typed references;
- `CapabilitySpec`;
- `AgentCatalog`, `AgentInvoker`, `BundleSpec`, and `BundleResolver` composition services outside the kernel;
- serializable `AgentSpec`;
- component resolution;
- default policies;
- result decoding;
- extension registrar;
- test helpers; and
- compatibility facade over kernel/runtime types.

## 5.4 `finstack-ai-protocol`

This crate owns the project codec, framing/handshake primitives, diagnostic journal encoding, and distinct versioned DTO families for remote sessions and external processes. It may depend on public kernel semantic DTOs where journal encoding requires them; remote/process DTOs map at outward adapters and never become kernel or in-process runtime contracts.

It remains separate from the runtime so codec tooling and clients do not import Tokio or provider code. The runtime, SDK, and in-memory store do not depend on protocol merely to exchange typed Rust values. A persistent store such as SQLite may depend outward on both the runtime-owned `JournalStore` contract and the protocol codec to encode/verify the single canonical journal format; this leaf dependency does not flow back into runtime or SDK.

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

The scripted model is the semantic reference for deterministic conformance. The reference network implementation is OpenAI-compatible with Chat Completions as its required baseline; Responses-style mapping is optional. Provider quirks stay in a versioned adapter table rather than changing the port contract.

## 6.2 Toolset

A toolset owns multiple related tools. It publishes a catalog and dispatches calls.

Architectural responsibilities:

- schema and metadata publication;
- input validation adapter backed by canonical JSON Schema draft 2020-12;
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

- typed interaction and approval policies;
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

Observers are explicitly unable to block or mutate run decisions. Bounded delivery backpressure may delay/disconnect an observer subscription but cannot gate semantic execution; correctness-critical audit facts use journal/runtime security paths instead.

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
  -> exact version/config/schema lock emission
  -> ResolvedAgent
```

After resolution, ordinary run execution does not traverse string-based registries.

Every successful resolution emits a versioned `ResolvedAgentLock` containing the semantic-engine version, exact component/capability selections, source spec/bundle and effective-configuration digests, middleware-chain digest, and relevant schema digests. It is credential-free, exportable, and required for exact reconstruction. Missing or incompatible locked selections fail before run acceptance. `BundleSpec` adds only finite requirements/alternatives/conflicts and fixed configuration layering; package installation/resolution remains outside the runtime hot path.

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

The MVP implements the activation record/reducer mechanics and exposes `Always` and `Application` modes. Compact catalog rendering, activation heuristics, and the complete `Model` mode are post-MVP policy/UX work, but must ship before the 0.1.0 public preview.

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
[EffectRequested(Model) record committed]
        |
        v
model stream -> transient progress events
        |
        v
[EffectCompleted(Model) + EntryAppended(assistant) committed]
        |
        +------ no tools ------> output processing -> before_finalize
        |
        v
before-tool middleware / approval
        |
        v
[ToolBatchOpened + EffectRequested(Tool) records committed]
        |
        v
parallel/sequential tool execution
        |
        v
[EffectCompleted/Failed/Cancelled(Tool) + result entries + ToolBatchClosed committed]
        |
        v
checkpoint -> next model turn or before_finalize
                                      |
                                      v
                       [terminal record committed]
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

The initial data model includes lane IDs and immutable parent links. Public concurrent multi-lane APIs arrive only after the SQLite store so cross-process sequence conflicts and crash recovery can be validated against a durable implementation.

## 9.5 Run and turn state

A run owns its current phase, explicit root/parent/effect relation, limit counters, active/deferred effects, unresolved interactions, pending tool calls, usage, and terminal result. A turn owns one model response and its complete tool batch.

Run relation metadata is immutable after acceptance. It provides root and parent correlation, relation kind, depth, optional budget scope, and external-work reference. It does not make child-run scheduling a kernel responsibility; `AgentInvoker` and application/runtime services own invocation and fan-out policy.

Every `RunAccepted` also persists an audit-safe initiating principal/tenant reference, authentication method/assurance, authorization policy version and decision ID, effective deadline/limits, propagation policy, and resolved-agent lock digest. Child contexts may only attenuate principal scopes, deadlines, and budget; tenant changes require a new authenticated boundary.

Child invocation allocates and commits a complete child locator—tenant/session/lane/run plus remote service/opaque route when applicable—in a unique parent mapping keyed by `(parent_run_id, parent_effect_id)` before child acceptance. Retried equal requests attach to that mapping; conflicting requests fail closed. The child accepts the mapped locator/`RunId` idempotently, so a crash between journals cannot create duplicate children, no ID is derived from an effect, and recovery never requires a global run-ID scan.

# 10. Journal and recovery architecture

## 10.1 Durable record categories

Records fall into these categories:

- session/lane metadata;
- operation acceptance and completion;
- context preparation decisions;
- model effect request/completion;
- assistant message entries;
- tool batch and call request/completion;
- effect deferral and external completion;
- typed interaction request/resolution/expiry/cancellation;
- run relation/lineage;
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

A deferred effect always retains the original `EffectId` and a non-secret external handle. Every external-completion or interaction-resolution command reaches the runtime with an authenticated tenant scope and explicit `(SessionId, LaneId, RunId, EffectId|InteractionId)` locator; signed opaque callback tokens are resolved to that locator at ingress. Routers never scan journals by globally supplied effect/interaction ID. They load the named session and validate scope, outstanding target, command identity, output digest/schema, principal, deadline, and cancellation state before proposing completion records.

Identical duplicates are idempotent. For a known authorized locator, a conflicting duplicate or invalid late command appends a durable rejection/audit record that does not change run state. Authentication failures, scope mismatches, malformed callback tokens, and unknown locators go only to the required security-audit sink to prevent journal probing or existence disclosure; failure of required audit recording fails the command closed.

Accepted command identity/digest settlements remain derivable from journal records and snapshots. Outstanding targets cannot be pruned; terminal settlement tombstones are retained for at least the callback-token/idempotency horizon. After that declared horizon, callback tokens are expired and late commands are rejected/audited without a claim of indefinite duplicate recognition.

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
before_model
after_model
before_tool_batch
after_tool_batch
before_finalize
```

For a concise public API, `prepare_context` and `before_model` are separate because context retrieval and final request shaping have different budgets and durability implications.

`before_finalize` receives a candidate terminal result before any terminal record is committed. It may accept completion, request bounded continuation/retry/interaction, suspend, or fail. Post-terminal notifications are immutable observer events; there is no behavior-changing `after_run` stage.

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
- request a typed interaction, including approval;
- request retry;
- end run with result;
- fail run; or
- suspend.

Arbitrary mutation of kernel state is prohibited.

## 11.4 Replay safety

A middleware stage that influences durable behavior must return a serializable outcome. On recovery, the recorded outcome may be reapplied without rerunning middleware unless policy explicitly marks it replay-safe and recomputable.

## 11.5 Context compaction ownership

Context compaction is model-request policy and therefore belongs to middleware at `before_model`. It does not belong to the kernel, because compaction strategy is application/model policy; to `ContextProvider`, because providers contribute context rather than rewrite the assembled request; to `Model`, because providers must receive an already bounded request; or to `Observer`, because compaction changes behavior.

The pipeline is:

```text
canonical immutable conversation/history
  -> prepare_context providers contribute budgeted items
  -> runtime assembles candidate model context and reserves output budget
  -> before_model compaction middleware
       continue unchanged when below threshold
       or replace the model-visible projection with a compacted projection
  -> validate hard model/context limits
  -> commit/dispatch model effect
```

A compaction strategy may use deterministic windowing, truncation of explicitly eligible large tool output, or model-assisted summarization. It must preserve:

- system/developer and active capability instructions;
- the current user request and active turn;
- unresolved interactions and policy/safety context;
- pinned context and application-declared non-compactable items;
- valid tool-call/result pairing and source order; and
- sensitivity, provenance, and source attribution for retained or summarized material.

Each resolved agent has at most one active middleware component declaring the context-compactor role; windowing, tool-output, summarizing, or future semantic approaches are strategies owned by that component rather than competing compaction middleware. Resolution fails on duplicate compaction owners. The compactor occupies a late `before_model` tier after all instruction/context/request-shaping middleware. Middleware after it may validate, deny, suspend, or request interaction, but may not add/replace/compact model context. The runtime applies final hard-limit validation after the chain.

Compaction never rewrites, deletes, or replaces canonical conversation entries. It produces a derived model-visible projection plus evidence identifying the middleware component/version, strategy/configuration digest, source range/digest, retained and summarized ranges, token estimates before/after, and cache impact.

Behavior-changing compaction output is recorded through the ordinary durable middleware-outcome path. An optional incremental checkpoint is a versioned derived cache keyed by the middleware/strategy/configuration, model-context profile, and covered-history digest. The runtime may supply the latest compatible checkpoint to the middleware on later turns; missing, stale, corrupt, or incompatible checkpoints are discarded and rebuilt from canonical history. No compaction-specific kernel state machine or eighth middleware stage is introduced.

Model-assisted summarization is a separately requested Model subeffect linked to the already committed middleware effect. The middleware returns a normalized subeffect request rather than calling a provider; the runtime commits, executes/reconciles, records usage, and resumes the same middleware cursor before the main model request is committed. The secondary model must be authorized for the full source sensitivity, tenant, residency, and egress policy. Protected messages remain ID/byte-identical and generated summaries are unprivileged derived context, never System/Developer authority. If protected content cannot fit, summarization fails, or the final projection still exceeds the locked model-context profile, the runtime returns a stable context-budget error or an explicitly configured safe fallback; it never silently removes protected content.

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
| Observer queue | Configurable bounded block, drop-progress, or disconnect; no v1 spill queue |
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

JSON Schema draft 2020-12 is the source of truth generated and normalized at registration. A default Rust validator is compiled once and used for native/WASM baseline behavior. Pydantic or another binding-native validator may materialize host objects at the binding boundary only when shared fixtures prove identical success, failure, and retry semantics.

## 13.5 Python async integration

The binding provides awaitables and async iterators backed by runtime futures and bounded channels. It must detect misuse from incompatible event-loop contexts and surface a clear error rather than nesting runtimes unsafely.

## 13.6 Python distribution and ABI

The initial `finstack-ai` wheel bundles the curated Rust-backed OpenAI-compatible, Anthropic, and local providers while their Rust crates remain separate. Imports are lazy, optional pure-Python dependencies are extras, and CI enforces a wheel-size budget.

The initial compatibility matrix is CPython 3.11-3.14 with per-version wheels plus 3.14t where supported. Classic `abi3` is intentionally not the launch strategy. Python 3.15+ `abi3t` or combined stable-ABI wheels may replace part of the matrix only after PyO3/maturin, performance, and project conformance gates pass.

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

The default browser topology runs the WASM engine in a Web Worker. The UI thread communicates through message batches. This prevents model/tool orchestration and record replay from blocking rendering. Main-thread execution is supported only as an explicitly documented host-compatible mode with the same bounds and conformance tests; it is not the production default.

## 14.5 Browser persistence

IndexedDB is implemented as a host `JournalStore` adapter. The kernel remains storage-neutral.

## 14.6 Optional remote-model adapter

`@finstack/ai/adapters/openai-compatible` is a tree-shakeable fetch/SSE implementation of the model host interface. It defaults to a same-origin application proxy, documents CORS behavior, and never treats browser-embedded provider API keys as an acceptable production configuration. The adapter is a battery, not a kernel network dependency.

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

## 15.6 External process protocol

The remote client protocol and external plugin protocol use distinct payload vocabularies and compatibility policies. Both reuse the same bounded 4-byte length prefix, versioned CBOR envelope, hello/version negotiation, and pre-allocation size checks from `finstack-ai-protocol`.

# 16. Data architecture

## 16.1 In-memory representation

Native structs are used internally. `Bytes` or shared immutable buffers are favored for raw JSON and media references.

## 16.2 Interchange formats

| Purpose | Format |
|---|---|
| Agent specs and schemas | JSON; optional YAML frontend |
| Tool arguments and structured output | UTF-8 JSON bytes |
| Journal/remote/process framing | Deterministic versioned CBOR envelope |
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

The reusable non-kernel service name is `ArtifactStore`: it stages and retrieves tenant/session-scoped, digest-bearing artifact content behind `ArtifactRef` values. Authoritative journal references are committed only after the bytes are durable; a failed journal append can leave a collectable orphan, while missing or digest-mismatched referenced content is an integrity failure. Retention of referenced content must cover the configured recovery/audit period. Plain `BlobRef` values may point to external or ephemeral media but cannot carry behavior-changing replay state without an `ArtifactRef` integrity/scope wrapper.

`BudgetLedger` aggregates reservations and usage across related run budget scopes. Neither service is a seventh primary port: the kernel never invokes them, applications may omit or replace them, and their policy does not alter universal reducer semantics.

# 17. Security architecture

The Security and Threat Model v0.4 is the authoritative threat/control register for these boundaries. This section owns the system placement of those controls; the Technical Design owns their concrete implementation.

## 17.1 Trust levels

| Extension mode | Trust assumption |
|---|---|
| Native Rust | Fully trusted process code |
| Python callback | Fully trusted process code |
| JavaScript host adapter | Trusted by the containing application |
| WASM component | Untrusted by default; capability scoped |
| External process | Isolated but authenticated and permission scoped |

## 17.2 Security enforcement boundary

The kernel tracks security-relevant metadata and interaction state but does not claim to sandbox. Enforcement occurs in:

- tool implementations;
- middleware policy;
- WASI/plugin host;
- process/container boundaries; and
- application authentication/authorization.

## 17.3 Secret handling

Secrets are represented by scoped references where possible. They are resolved inside trusted provider/tool/plugin adapters and are excluded from ordinary event and journal payloads.

## 17.4 Auditability

Security decisions such as typed interaction approvals, policy denials, permission grants, and plugin identity are durable or observable with stable identifiers and redaction controls.

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

Observer delivery may use bounded blocking, progress dropping, or disconnect policies, but observer failure cannot change a run decision, prevent an effect, or alter a terminal result. Version 1 has no observer spill queue. Correctness-critical audit facts are committed journal records. A server/runtime security-audit sink may fail an unauthenticated or unknown-target external command closed before it becomes kernel input; that ingress service is not an `Observer` and does not retroactively change accepted execution.

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

Every record envelope contains format version and record kind version. Durable fields are additive within a kind version only when its schema marks them semantically ignorable and decoders preserve their canonical bytes/opaque value for re-export; an unknown state-bearing field or record kind is fatal to replay until a supported version/migration exists. Strict configuration and inbound command schemas reject unknown fields. Diagnostic metadata may be retained or ignored as declared. The Technical Design compatibility matrix is authoritative per schema family.

Semantic timestamps and record/batch IDs are supplied by the normalized transition environment and remain frozen across retries; stores may add a separate commit timestamp but cannot rewrite semantic time. Append batches have stable identities. On an ambiguous acknowledgement, exact replay of a previously committed batch returns its original receipt before optimistic-sequence checks, while content mismatch is corruption.

## 20.3 Python and JavaScript APIs

Through pre-1.0, the semantic core crates and Rust/Python/JavaScript binding distributions use one lockstep workspace release version and compatibility matrix. After 1.0, a binding distribution may release independently only when it declares the compatible semantic-engine range and passes the shared fixtures. Stable error codes and event type names are public compatibility surfaces in every release mode.

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

## 22.3 License and project governance

The project is distributed under `MIT OR Apache-2.0`. Contributions use Developer Certificate of Origin sign-off rather than a contributor license agreement. A named maintainer group owns releases and security response; ADRs are binding for architecture, and a public RFC process is required for ecosystem-facing contract changes such as journal schemas, event order, WIT worlds, and remote protocols.

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
- generalized interactions, with approval as the first profile;
- filesystem/shell batteries;
- main-lane/session branching foundation;
- model-activated capability catalog UX in the binding-alpha/durability-beta window;
- structured observers.

## 24.3 Third slice

- concurrent multi-lane APIs after SQLite recovery is proven;
- WIT toolset/context plugin host;
- remote protocol;
- additional providers/stores;
- workflow adapters;
- provider-diversity validation of model-activated capabilities.

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
| ADR-015 | Deterministic versioned CBOR is canonical; JSON/JSONL is diagnostic |
| ADR-016 | SQLite durability precedes concurrent multi-lane APIs |
| ADR-017 | Curated Rust-backed providers ship in one initial Python wheel |
| ADR-018 | Python launches with CPython 3.11-3.14 per-version wheels plus 3.14t |
| ADR-019 | Browser host interfaces remain canonical and an optional fetch/SSE adapter ships as a battery |
| ADR-020 | Capability mechanics ship in MVP; model-activated catalog UX is required by public preview |
| ADR-021 | Remote and process protocols share framing/handshake code but not message vocabularies |
| ADR-022 | JSON Schema 2020-12 is the schema source of truth with conformant native/binding validators |
| ADR-023 | The network reference provider is OpenAI-compatible Chat Completions; the scripted model remains semantic reference |
| ADR-024 | The project uses MIT OR Apache-2.0, DCO, named maintainers, and ADR/RFC governance |
| ADR-025 | Effects may defer generically and later complete under the original `EffectId` |
| ADR-026 | Every run persists explicit root/parent/effect lineage |
| ADR-027 | Approval is a standard profile of generalized typed interactions |
| ADR-028 | `before_finalize` is the final behavior-changing middleware stage; post-run handling is observation only |
| ADR-029 | Allocated runtime entity IDs use typed UUID values serialized as lowercase UUID strings and new values use UUIDv7; human-selected agent/component/capability/tool/bundle keys remain validated namespaced strings |
| ADR-030 | Object-safe boxed futures/streams are the initial public extension ABI; concrete implementations may optimize internally |
| ADR-031 | Browser WASM is single-threaded/worker-based by default; threaded WASM is a post-preview opt-in requiring separate evidence |
| ADR-032 | Initial snapshots are direct versioned kernel-state CBOR projections and remain disposable derived caches |
| ADR-033 | MVP interruption uses retry/suspend/explicit uncertainty while the Model port reserves optional reconciliation implemented with durability |
| ADR-034 | State-changing middleware outcomes are recorded by default; only explicitly recompute-safe outcomes may rerun during replay |
| ADR-035 | WIT plugin alpha uses experimental @0.x tool/context packages with one coarse completion; @1.0.0 worlds freeze only at the framework `1.0.0` gate and resource-based streaming remains deferred |
| ADR-036 | Blob storage remains an application/runtime service through 1.0 and does not become a seventh kernel port |
| ADR-037 | Model-context compaction is `before_model` middleware; it preserves canonical history and records only a versioned derived projection/checkpoint |

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
| FR-MW | Seven-stage normalized middleware chain, including `before_model` compaction and `before_finalize` verification |
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
11. If it changes model-visible context, is it explicit `before_model` middleware that preserves canonical history, protected content, provenance, and replay evidence?
