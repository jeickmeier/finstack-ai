---
title: "finstack-ai Product Requirements Document"
subtitle: "Rust-native agent microkernel with first-class Python and WebAssembly bindings"
author: "finstack-ai project"
date: "2026-08-08"
---

# finstack-ai Product Requirements Document

# Document control

| Field | Value |
|---|---|
| Product | `finstack-ai` |
| Document | Product Requirements Document (PRD) |
| Version | 0.7 |
| Status | Pre-implementation product baseline |
| Primary audience | Product owners, framework architects, Rust/Python/WASM engineers, extension authors |
| Related documents | Engineering Standards v0.5; Architecture Specification v0.7; Technical Design v0.11; Implementation Plan v0.11; Security and Threat Model v0.4 |

# Executive summary

`finstack-ai` is a new, open, Rust-native framework for building high-performance AI agents and agent-powered applications. Its foundation is a small **agent microkernel** that owns the universal semantics of an agent run: messages, turns, model requests, tool calls, event ordering, cancellation, limits, durable effect boundaries, and recovery. Everything that represents product policy or external integration—model providers, toolsets, context and memory, persistence, middleware, observability, channels, gateways, workflow systems, and user interfaces—is composed around the kernel through a deliberately small set of extension ports.

The framework will expose the same execution engine through:

- an idiomatic Rust SDK;
- a first-class Python package implemented with PyO3;
- a browser and JavaScript WebAssembly package implemented with `wasm-bindgen`; and
- an optional WIT/Component Model plugin ABI for isolated third-party extensions.

The product is intended to combine the strongest qualities of existing harnesses without reproducing their architectural weaknesses:

- the concise, agent-centered model and extension ergonomics associated with small TypeScript harnesses;
- the typed tools, structured outputs, toolsets, capabilities, and Python developer experience associated with Pydantic-style frameworks;
- the performance, portability, deterministic behavior, and isolation enabled by a Rust-owned execution engine; and
- the modularity of a microkernel without turning every feature into an IPC call or a dynamically loaded plugin.

The standard fast path will statically compose trusted Rust implementations and call them directly. Python and JavaScript callbacks will be supported at coarse-grained boundaries. Independently distributed or untrusted plugins may run in a WASM component or separate process. The framework must remain useful with only one model and no tools, while scaling to durable, multi-session, multi-lane applications.

# 1. Product vision

## 1.1 Vision statement

> Build the smallest trustworthy execution core for AI agents, then make every higher-level capability optional, composable, and equally accessible from Rust, Python, and WebAssembly.

## 1.2 Product promise

A developer should be able to start with a few lines of code:

```rust
let agent = Agent::builder(model)
    .instructions("You are a concise research assistant.")
    .toolset(filesystem)
    .build()?;

let result = agent.run("Review this repository").await?;
```

and retain a clean path to:

- structured tools and outputs;
- streaming and cancellation;
- durable runs and typed human interactions, including approvals;
- multiple model providers;
- Python-defined tools;
- browser execution;
- isolated third-party extensions;
- remote servers and clients; and
- production observability.

The framework must not require a user to adopt a gateway, database, workflow engine, TUI, channel catalogue, or plugin marketplace to use the core agent functionality.

## 1.3 Positioning

`finstack-ai` is a **framework and execution engine**, not a finished assistant application. Products such as coding agents, research assistants, financial analysis agents, document workflows, chat assistants, and long-running automation systems should be built on top of it.

The product is positioned between two common extremes:

- lightweight libraries that are easy to embed but lack durable semantics and cross-language consistency; and
- broad agent platforms that accumulate channels, gateways, schedulers, memory systems, and provider integrations into a large runtime.

`finstack-ai` will provide a strict center and optional edges.

# 2. Problem statement

Current agent frameworks commonly exhibit one or more of the following problems:

1. **The execution semantics are owned by a dynamic host language.** Agent state, streaming, hooks, validation, and tool scheduling are repeatedly interpreted in Python or JavaScript even when most work could remain in native code.
2. **The runtime grows into a monolith.** Providers, channels, databases, dashboards, cron, memory, RAG, hardware, and product-specific CLI concerns become dependencies of the base package.
3. **Extension APIs become overly broad.** A single plugin or capability interface may expose dozens of lifecycle methods, UI concepts, product-specific state, and mutable internal objects.
4. **Dynamic extensibility is confused with modularity.** Every optional feature is made a process, a WASM component, or a runtime-discovered plugin even when a direct native call would be simpler and faster.
5. **Durability is bolted on later.** Runs lack stable effect identifiers and durable boundaries, forcing external workflow engines to wrap an execution model that was not designed for replay or recovery.
6. **Bindings become secondary products.** Python or WebAssembly APIs reimplement semantics, copy large state structures, or cross the language boundary for each stream token.
7. **The smallest build is not actually small.** A “core” package still pulls in every provider SDK, HTTP server, database driver, telemetry exporter, and UI dependency.
8. **Framework behavior differs by frontend.** Rust, Python, JavaScript, CLI, and server modes may order events or handle cancellation differently.

The product must solve these problems through a single Rust-owned semantic core, carefully bounded extension interfaces, and mechanically enforced dependency rules.

# 3. Product principles

## 3.1 Agent-centered microkernel

The kernel is opinionated about the mechanics of one agent run. It is not a generic service container. It owns the canonical model/tool continuation loop and the rules required to keep histories valid and runs recoverable.

## 3.2 Mechanism in the kernel, policy at the edge

The kernel defines what a model request, tool batch, cancellation, approval, limit, and journal transition mean. Extensions decide which model to call, what a tool does, how context is gathered, and which policy is applied.

## 3.3 Direct native calls by default

A microkernel is a responsibility boundary, not a mandate for serialization. Trusted Rust components are resolved once and retained as direct handles. Runtime discovery, WASM, and IPC are optional deployment modes.

## 3.4 One semantic engine across bindings

Rust, Python, JavaScript, browser WASM, servers, and isolated plugins must share the same message model, state transitions, effect identifiers, event order, and recovery rules.

## 3.5 Coarse boundaries

Cross-language and cross-process calls occur at model, toolset, context, middleware-stage, store-batch, or event-batch boundaries. Per-token callbacks across FFI or WASM are not part of the default design.

## 3.6 Capabilities are declarative composition

A capability bundles instructions, toolsets, context providers, middleware, and activation metadata. It is distinct from executable extension code and from an independently distributed plugin package.

## 3.7 Durability starts with effect identity

Every external effect has a stable identifier and a defined requested/completed lifecycle. The framework will not claim exactly-once side effects where that cannot be guaranteed; it will provide idempotency keys and deterministic recovery behavior.

## 3.8 Small core, excellent batteries

The core remains concise. High-quality providers, toolsets, stores, policies, and adapters may be maintained as separate first-party crates and packages.

# 4. Target users and personas

## 4.1 Rust framework integrator

Builds a native application or service and values low overhead, explicit control, safe concurrency, and static composition. Wants to embed an agent without a Python runtime.

**Needs:** idiomatic traits, zero-copy data where practical, predictable async behavior, small dependency graph, deterministic tests, and optional durability.

## 4.2 Python AI application developer

Uses Python for business logic, data science, and rapid iteration. Expects concise decorators, Pydantic-compatible schemas, async support, strong typing, useful errors, and access to Rust-backed providers and tools.

**Needs:** a native wheel, no Rust toolchain for normal installation, Python tools when desired, structured outputs, streaming, and familiar async iteration.

## 4.3 Web and JavaScript developer

Builds browser-based assistants, desktop webviews, or Node applications. Wants a compact WASM package and JavaScript-friendly promises and async iterators.

**Needs:** no server requirement, browser-compatible runtime adapters, TypeScript definitions, event batching, and persistent state handles.

## 4.4 Agent platform engineer

Runs many concurrent sessions and integrates storage, policy, telemetry, gateways, and workflow systems. Values stable execution semantics and recovery.

**Needs:** journal records, limits, cancellation, session/lane routing, backpressure, provider pooling, and conformance tests.

## 4.5 Extension author

Creates a provider, toolset, memory system, middleware policy, store, observer, Python package, or WASM component.

**Needs:** narrow contracts, reference implementations, test harnesses, semantic versioning, and clear security boundaries.

## 4.6 Framework product builder

Builds a coding agent, research product, finance assistant, or internal automation platform and wants to own the user experience rather than inherit one from the framework.

**Needs:** headless core, customizable prompts and capabilities, no mandatory TUI or gateway, and portable session APIs.

# 5. Primary use cases

## UC-01: Minimal native agent

A Rust application creates an agent with one model, sends a prompt, receives a streamed or final response, and uses no tools or persistence.

## UC-02: Tool-using coding agent

An application adds filesystem and shell toolsets, configures parallel or sequential execution, applies an approval policy, streams updates, and persists a session.

## UC-03: Python-defined domain tools

A Python application uses Rust-backed model and session implementations while registering one or more Python async functions as tools with Pydantic-compatible input and output schemas.

## UC-04: Browser agent

A browser application loads the kernel/runtime as WASM, supplies a JavaScript model adapter or calls a remote model service, streams event batches into a UI, and persists state to IndexedDB through a host adapter.

## UC-05: Durable human-in-the-loop workflow

A run pauses on a typed human interaction such as an approval, choice, form, review, or correction; survives process restart; resumes after an authorized external response; and does not repeat completed effects.

## UC-06: High-concurrency agent service

A server hosts thousands of idle sessions and hundreds of active runs, uses provider connection pools, PostgreSQL or another store, and emits OpenTelemetry through an observer extension.

## UC-07: Isolated third-party toolset

A native host loads a WIT-defined WASM component, grants a narrow set of filesystem or network permissions, and exposes its tools to an agent without linking the plugin into the host process.

## UC-08: Multiple lanes within a session

A single session contains independent lanes for a main conversation, external chat threads, or subagent work. Each lane has at most one active operation while lanes may execute concurrently.

## UC-09: External workflow integration

Temporal, Restate, DBOS, or another workflow engine drives the same effect model, supplies durable timers, and persists effect outcomes without replacing the kernel’s agent semantics.

# 6. Goals

## G-01: Concise useful core

A developer can create and run an agent without configuring a plugin manager, database, gateway, graph engine, or workflow system.

## G-02: Native performance

The kernel and standard runtime add negligible overhead relative to provider and tool work, and scale efficiently for local models, fast tools, and high concurrency.

## G-03: First-class Python

The Python API is not a thin demonstration wrapper. It supports the full run lifecycle, Rust-backed components, Python callbacks, structured outputs, Pydantic adapters, streaming, cancellation, and durability.

## G-04: First-class WebAssembly

The core compiles to WebAssembly, exposes idiomatic JavaScript bindings, and maintains the same deterministic traces as native Rust for equivalent inputs.

## G-05: Extensibility without kernel edits

Adding a normal provider, toolset, context source, middleware implementation, journal store, or observer requires no changes to the kernel.

## G-06: Durable by construction

The state model includes stable effect identities, run lineage, generic deferral, typed interactions, journal records, replay rules, and explicit suspension/resumption semantics from the first public release.

## G-07: Safe optional isolation

Untrusted extensions can run behind WIT/WASI or process boundaries with explicit permissions and resource limits, while trusted native code remains fast.

## G-08: Stable semantics

All bindings and runtimes pass common conformance traces for event order, tool scheduling, cancellation, limits, and recovery.

# 7. Non-goals

The initial product is not intended to be:

- a finished coding assistant or chat application;
- a comprehensive gateway, dashboard, or channel server;
- a provider marketplace;
- a generic distributed workflow engine;
- a replacement for Temporal, Restate, DBOS, or Prefect;
- a general-purpose actor runtime;
- a universal plugin package manager;
- an arbitrary DAG engine in the standard run path;
- a vector database or memory product;
- a browser automation framework;
- an MCP server catalogue;
- an attempt to make untrusted native dynamic libraries safe; or
- a claim of exactly-once execution for arbitrary external side effects.

These capabilities may be built as separate products or extensions.

# 8. Product scope and conceptual model

## 8.1 Core concepts

| Concept | Definition |
|---|---|
| Agent | An immutable composition of a model, toolsets, context providers, middleware, limits, capabilities, store, and observers |
| Session | Durable container for conversation entries, lanes, metadata, and run history |
| Lane | Independently advancing position within a session; at most one active operation per lane |
| Run | An accepted unit of agent work from input until completion, failure, cancellation, or suspension |
| Run relation | Durable root/parent/effect lineage connecting a run to delegated, child-agent, or workflow work |
| Turn | One model response plus the tool batch requested by that response |
| Effect | External work requested by the kernel, such as a model call, tool execution, interaction, timer, or persistence barrier |
| Interaction | Durable typed request for human or external input; approval is one standard interaction kind |
| Event | Immutable observation emitted by execution; may be durable or transient |
| Journal record | Durable append-only fact used to reconstruct state and recover work |
| Extension | Executable code that registers one or more implementations with the framework |
| Capability | Declarative bundle of instructions and references to registered components |
| Plugin | Independently distributed extension executed through WASM or process isolation |

## 8.2 Six extension ports

The initial architecture will expose six primary extension ports:

1. `Model`
2. `Toolset`
3. `ContextProvider`
4. `Middleware`
5. `JournalStore`
6. `Observer`

A seventh port requires an architecture decision demonstrating that the behavior cannot be represented cleanly through the existing six or the public session/run API.

## 8.3 Execution modes

| Mode | Intended use | Invocation path |
|---|---|---|
| Native static | Trusted first-party and high-throughput components | Direct Rust trait calls |
| Python/JavaScript callback | Application-defined components | Coarse FFI callback |
| WASM component | Untrusted or independently distributed plugin | WIT canonical ABI |
| External process | Stronger isolation or non-WASM language | Versioned local protocol |

# 9. Functional requirements

## 9.1 Agent microkernel

### FR-KRN-001: Canonical message model

The kernel shall define a provider-neutral message and content model supporting at minimum text, structured JSON, tool calls, tool results, images by reference, files by reference, and provider-specific opaque metadata that can round-trip without being interpreted by unrelated providers.

### FR-KRN-002: Canonical run loop

The kernel shall implement the standard continuation loop:

```text
input → context → model → optional tool batch → model continuation → result
```

It shall guarantee valid ordering and pairing of assistant tool calls and tool results.

### FR-KRN-003: Explicit state machine

Runs shall use the canonical phases `accepted`, `before-run`, `preparing-context`, `before-model`, `awaiting-model`, `after-model`, `before-tool-batch`, `awaiting-tools`, `after-tool-batch`, `before-finalize`, `awaiting-interaction`, `awaiting-external`, `sleeping`, `cancelling`, `suspended`, `completed`, `failed`, and `cancelled`, with only the Technical Design transition table permitted.

### FR-KRN-004: Deterministic decisions

Given the same state and normalized input, `decide` shall produce the same durable record drafts and post-commit actions. Given the same committed batch, `apply` shall produce the same ordered durable-derived public events. Transient progress events are runtime-sequenced and need not reproduce provider timing during replay.

### FR-KRN-005: Stable identifiers

The kernel shall assign stable identifiers for sessions, lanes, runs, turns, model requests, tool batches, tool calls, effects, interactions, and journal records.

### FR-KRN-006: Event ordering

The kernel shall define a binding-independent order for run, turn, message, model, tool, interaction, limit, cancellation, and completion events.

### FR-KRN-007: Transient and durable events

The framework shall distinguish transient progress events from durable records. Token or text deltas shall not be journaled individually by default.

### FR-KRN-008: Limits

The kernel shall enforce configurable limits for model requests, turns, tool calls, parallel tool concurrency, wall time, tokens, cost, output size, context size, retries, and extension-defined counters.

### FR-KRN-009: Cancellation

Cancellation shall be explicit, idempotent, observable, and consistent across Rust, Python, and WASM. Incomplete tool calls shall be reconciled into a valid history before the lane returns to idle.

### FR-KRN-010: Retry semantics

The kernel shall distinguish retryable model failures, retryable tool failures, validation retries directed to the model, and terminal errors. Retry policy shall be configuration rather than provider-specific logic in the kernel.

### FR-KRN-011: Structured output

The framework shall support plain text, raw JSON, schema-validated structured output, and application-defined result decoders. JSON Schema draft 2020-12 shall be the portable schema source of truth; validators may be native to a binding only when they pass the shared validation/retry conformance fixtures.

### FR-KRN-012: Internal tools

The kernel may expose a small number of internal tools required for universal mechanics, such as on-demand capability activation. Product-specific tools shall not live in the kernel.

### FR-KRN-013: Generic deferred effects

Any supported model, tool, interaction, or externally hosted operation shall be able to defer completion while preserving the original `EffectId`, a non-secret external handle, reconciliation policy, expiry/poll metadata, and expected result contract. Deferral shall suspend through generic effect semantics rather than feature-specific run states.

### FR-KRN-014: Run lineage

Every accepted run shall persist an explicit relation containing its root run, optional parent run/effect, relation kind, depth, and optional budget scope. Cancellation, deadlines, principal context, and budget attribution shall propagate according to declared child/delegation policy and remain reconstructable after restart.

### FR-KRN-015: Generalized interactions

The kernel shall define durable, typed interaction request and resolution semantics for approval, choice, form, free text, review, and namespaced custom interactions. Approval-specific APIs may be ergonomic wrappers but shall not create a separate persistence path.

## 9.2 Runtime and effect execution

### FR-RT-001: Runtime driver

A native asynchronous runtime shall execute kernel effects, return normalized results, and publish event batches.

### FR-RT-002: Model streaming

The runtime shall support incremental model text, reasoning, usage, tool-call, and completion events without requiring the kernel to depend on a particular HTTP or provider SDK.

### FR-RT-003: Tool scheduling

The runtime shall support parallel, sequential, and barrier tool execution. Final tool-result messages shall preserve source order even when execution completes out of order.

### FR-RT-004: Backpressure

All internal queues shall be bounded or governed by an explicit policy. Event consumers shall be able to choose blocking, coalescing, drop-progress, or disconnect behavior while durable completion events remain protected.

### FR-RT-005: Stream batching

The runtime shall batch fine-grained progress events for Python, JavaScript, remote clients, and observers according to configurable byte, count, or time thresholds.

### FR-RT-006: Resource reuse

Model providers, HTTP clients, stores, and toolsets shall be reusable across runs and sessions. The runtime shall not construct a new client for every request unless an implementation requires it.

### FR-RT-007: Timers and clocks

Sleep, retry delays, deadlines, and time shall be abstracted so tests and durable workflow integrations can inject deterministic or durable clocks.

### FR-RT-008: External completion routing

The runtime shall accept authenticated external effect completions and interaction resolutions through explicit routers. Duplicate completions shall be idempotent when their identity and digest match; conflicting duplicates shall fail closed and remain auditable.

## 9.3 Extension registration

### FR-EXT-001: Registrar

Extensions shall register implementations through a typed registrar rather than by mutating runtime internals.

### FR-EXT-002: Resolve once

An `Agent` shall resolve named registrations and capability references during construction or run setup. The hot path shall retain direct handles.

### FR-EXT-003: Duplicate handling

Duplicate identifiers shall be rejected by default. Explicit replacement or priority behavior must be declared and observable.

### FR-EXT-004: Namespacing

Extension, capability, tool, and configuration identifiers shall support namespaces to avoid collisions across organizations.

### FR-EXT-005: Lifecycle

Extensions that own long-lived resources shall support initialize, health, and shutdown behavior outside the kernel. Shutdown shall be idempotent and bounded by a deadline.

### FR-EXT-006: Native extension SDK

The Rust SDK shall provide helpers, macros only where justified, mocks, and conformance tests for implementing each port.

## 9.4 Capabilities

### FR-CAP-001: Declarative composition

A capability shall be able to contribute instructions and references to toolsets, context providers, and middleware without implementing execution methods itself.

### FR-CAP-002: Activation

Capabilities shall support always-on, application-activated, and model-activated modes. The MVP shall include durable activation records and the always-on/application modes. The compact model-activated catalog experience may mature after the native developer-preview slice but is required for the 0.1.0 public preview.

### FR-CAP-003: Catalog

On-demand capabilities shall expose compact identifiers and descriptions before activation, without injecting all tools and instructions into the model context.

### FR-CAP-004: Cache stability

Capability activation shall append context changes at safe boundaries where possible and avoid rewriting stable prompt prefixes unnecessarily.

### FR-CAP-005: Serialization

Capabilities composed only of serializable values and registered references shall be expressible in an `AgentSpec`. Capabilities containing host callables may remain code-only.

## 9.5 Model providers

### FR-MDL-001: Provider-neutral contract

The `Model` port shall support request/response streaming, capability discovery, model metadata, usage, provider-specific opaque extensions, and cancellation.

### FR-MDL-002: Model capabilities

A model implementation shall declare supported inputs, structured tool calls, images, reasoning, prompt caching, native tools, continuations, and other normalized capabilities.

### FR-MDL-003: Provider adaptation

Higher-level capabilities shall be able to select provider-native functionality or a local fallback during agent resolution.

### FR-MDL-004: First-party providers

The first stable distribution shall include separate Rust crates for at least one OpenAI-compatible provider, Anthropic-compatible provider, and local/OpenAI-compatible endpoint. Those curated Rust-backed providers shall be bundled into the initial `finstack-ai` Python wheel; provider crates shall not be dependencies of the kernel.

### FR-MDL-005: Test model

The SDK shall include a deterministic fake/scripted model that can emit text, stream chunks, tool calls, errors, deferred responses, and usage for tests.

## 9.6 Toolsets

### FR-TLS-001: Multiple tools per toolset

A `Toolset` shall expose a catalog of related tools and dispatch calls by stable tool identifier.

### FR-TLS-002: Schemas

Each tool shall publish input schema, optional output schema, descriptions, execution mode, side-effect classification, approval metadata, and size limits.

### FR-TLS-003: Validation

Tool inputs shall be validated before execution. Validation failures shall be representable either as terminal framework errors or model-visible retry results according to policy.

### FR-TLS-004: Idempotency

Tool calls shall receive stable effect and call identifiers suitable for idempotency keys. Tools shall be able to declare whether they are read-only, idempotent, retry-safe, or non-repeatable.

### FR-TLS-005: Progress

Tools may emit bounded progress updates, but progress shall not be required for correct completion or recovery.

### FR-TLS-006: First-party toolsets

Initial first-party batteries shall include small filesystem and shell toolsets in separate packages, with path scoping, protected patterns, output limits, and timeouts.

## 9.7 Context providers and memory

### FR-CTX-001: Context contribution

A `ContextProvider` shall receive a bounded context request and return typed context items, references, provenance, and token/byte estimates.

### FR-CTX-002: Context ordering

The framework shall define stable ordering and precedence for system instructions, conversation history, capability instructions, external context, and reminders.

### FR-CTX-003: Budgeting

Context providers shall receive a budget and shall not silently exceed it. After context assembly, the runtime shall invoke configured `before_model` compaction middleware when the candidate request exceeds its policy threshold. If no eligible strategy can satisfy the model budget, the runtime shall return a stable context-budget error rather than silently discard required content.

### FR-CTX-004: Memory as optional policy

Semantic memory and RAG shall be implemented through context providers and/or toolsets, not as mandatory kernel concepts.

## 9.8 Middleware

### FR-MW-001: Limited stages

The stable middleware interface shall expose no more than seven behavior-changing stages in the first major version:

1. before run;
2. prepare context;
3. before model;
4. after model;
5. before tool batch;
6. after tool batch; and
7. before finalize.

### FR-MW-002: Deterministic outcomes

Middleware outcomes that change execution shall be normalized into serializable decisions and recorded when required for recovery.

### FR-MW-003: Ordering

Middleware ordering shall be explicit and simple. The default shall be registration order with optional numeric priority and named before/after constraints resolved once.

### FR-MW-004: No per-token default hooks

The stable middleware ABI shall not include per-token callbacks. Fine-grained stream observation belongs to observers and event consumers.

### FR-MW-005: Approval policy

Human approval shall be expressible as middleware that converts a tool batch or call into a durable interaction whose kind is `approval`.

### FR-MW-006: Pre-terminal verification

`before_finalize` shall run after a candidate result exists and before any terminal record is committed. It may accept completion, request a bounded continuation/retry/interaction, or fail the run. Post-terminal notifications are observer events and cannot change execution.

### FR-MW-007: Context compaction ownership

Compaction that changes the model-visible conversation or context projection shall be implemented as `before_model` middleware. It may use deterministic windowing or model-assisted summarization, but it shall not rewrite or delete canonical conversation history. Its behavior-changing result and any reusable checkpoint shall be normalized, versioned, attributable, and recorded when durability/replay requires it. `ContextProvider` implementations contribute context; observers only observe; neither owns compaction policy.

## 9.9 Sessions, journals, and recovery

### FR-DUR-001: Append-only records

Durable state changes shall be represented as append-only journal records with schema version, sequence number, identifiers, and timestamp.

### FR-DUR-002: Commit before effect

The runtime shall persist an effect-request record before executing a recoverable external effect.

### FR-DUR-003: Completion records

The runtime shall persist normalized effect completion, failure, cancellation, or deferral records before considering the effect settled.

### FR-DUR-004: Recovery

A session shall be reconstructable from a snapshot plus journal tail or from journal records alone.

### FR-DUR-005: At-least-once transparency

Where external systems cannot guarantee exactly-once behavior, recovery shall surface the possibility of re-execution and pass stable idempotency keys to implementations.

### FR-DUR-006: In-memory mode

The framework shall run without a durable store by using an in-memory journal implementation.

### FR-DUR-007: SQLite store

A first-party SQLite journal store shall be available outside the kernel.

### FR-DUR-008: Sessions and lanes

A session shall have a default `main` lane. The data model shall support additional lanes and immutable parent-linked conversation entries, even if some advanced lane operations are phased after the first MVP.

### FR-DUR-009: Single writer per lane

At most one active operation shall write a lane at a time. Different lanes may run concurrently.

### FR-DUR-010: Suspension and resume

Runs shall be able to suspend on an interaction, durable timer, generic deferred effect, external completion, or process shutdown, and later resume from a defined boundary.

### FR-DUR-011: Lineage and outstanding-work recovery

Journal replay shall reconstruct run relations, deferred handles, unresolved interactions, and their cancellation/deadline state without consulting transient process memory.

## 9.10 Observability

### FR-OBS-001: Immutable observer stream

Observers shall receive immutable event batches and shall not be able to change execution.

### FR-OBS-002: Correlation

Every event shall carry sufficient identifiers to correlate session, lane, run, turn, model request, tool call, and effect.

### FR-OBS-003: Standard observer adapters

First-party adapters shall be possible for structured logs, OpenTelemetry, metrics, and test capture without adding those dependencies to the kernel.

### FR-OBS-004: Sensitive data policy

Event payloads shall classify potentially sensitive content. Observers shall support metadata-only or redacted modes.

## 9.11 Rust SDK

### FR-RS-001: Idiomatic builders

The Rust API shall support concise builders and direct trait implementations without requiring macros.

### FR-RS-002: Async runtime separation

The kernel crate shall not depend on Tokio. The standard runtime may use Tokio while alternative runtimes can drive the same kernel.

### FR-RS-003: Typed result decoding

Rust users shall be able to decode structured results through Serde-compatible types while retaining access to raw text or JSON.

### FR-RS-004: Borrowing and ownership

Public types shall favor immutable shared data and `Bytes`-style buffers where useful, while avoiding lifetime-heavy APIs that cannot map cleanly to Python or WASM.

## 9.12 Python binding

### FR-PY-001: Native wheel

The Python package `finstack_ai` shall be distributed as prebuilt wheels for CPython 3.11 through 3.14 and shall not require a Rust compiler for normal installation. The initial release matrix shall use per-version wheels and include a CPython 3.14 free-threaded build where the target platform supports it.

### FR-PY-002: Core classes

The Python API shall expose `Agent`, `AgentSpec`, `Session`, `Lane`, `Run`, `Capability`, model/toolset interfaces, middleware, observers, and event types.

### FR-PY-003: Async and sync APIs

Async APIs shall be primary. A safe synchronous convenience API may be provided where no event loop conflict exists.

### FR-PY-004: Rust-backed fast path

When model, tools, middleware, and store are Rust-backed, the orchestration loop shall remain in Rust and release the GIL during Rust-only work.

### FR-PY-005: Python callbacks

Python models, tools, context providers, and middleware shall be supported through coarse async callbacks. The framework shall document the additional overhead and concurrency behavior.

### FR-PY-006: Event batching

The internal FFI shall transfer batches. The public Python API may expand those batches into individual events for ergonomic iteration.

### FR-PY-007: Pydantic adapter

An optional adapter shall accept Pydantic models, dataclasses, `TypedDict`, and type annotations for tool and output schemas. Schema generation shall occur at registration, not per call.

### FR-PY-008: Exceptions

Rust errors shall map to a documented Python exception hierarchy preserving stable codes, contextual identifiers, retryability, and source chains where safe.

## 9.13 Browser and JavaScript WASM binding

### FR-WASM-001: Core compilation

The deterministic kernel shall compile to `wasm32-unknown-unknown` without Tokio, filesystem, socket, database, or Wasmtime dependencies.

### FR-WASM-002: JavaScript API

The npm package shall expose idiomatic TypeScript definitions, promises, persistent object handles, and async event iteration.

### FR-WASM-003: Host adapters

JavaScript shall be able to supply model, toolset, context, store, and timer adapters through promises or message ports.

### FR-WASM-004: Event batching

WASM-to-JavaScript event delivery shall be batched. The framework shall avoid a boundary crossing for every token.

### FR-WASM-005: Browser storage

A reference IndexedDB journal adapter shall be possible without adding browser storage concepts to the kernel.

### FR-WASM-006: Conformance

Equivalent scripted inputs shall produce the same durable record sequence and normalized event order in native Rust and WASM.

### FR-WASM-007: Optional remote-model adapter

Host interfaces remain the browser contract. The npm package shall additionally provide one tree-shakeable fetch/SSE OpenAI-compatible adapter intended for a same-origin application proxy. It shall not encourage or require embedding provider secrets in browser code.

## 9.14 Isolated plugins

### FR-PLG-001: WIT ABI

An optional host shall define versioned WIT worlds for at least toolsets and context providers. Additional worlds require compatibility and security review.

### FR-PLG-002: Permissions

WASM plugins shall request explicit permissions. The host shall default deny ambient filesystem, environment, network, secrets, and host-memory access.

### FR-PLG-003: Resource limits

The host shall support memory ceilings, fuel or epoch interruption, table/instance limits, call deadlines, and output-size limits.

### FR-PLG-004: External process protocol

A later optional process adapter may use the same semantic interfaces over a versioned local protocol for languages or isolation needs unsuitable for WASM. It shall reuse the remote protocol's bounded frame, envelope, and hello/version-negotiation layer while retaining a distinct plugin message vocabulary.

### FR-PLG-005: No native dynamic library ABI

The project shall not expose an unstable Rust dynamic-library plugin ABI as a primary extension path.

## 9.15 Specifications, configuration, and packaging

### FR-SPEC-001: Immutable `AgentSpec`

An agent’s serializable composition shall be represented by an immutable, versioned `AgentSpec`.

### FR-SPEC-002: Namespaced configuration

Extension-specific configuration shall be namespaced and owned by the extension. The kernel shall not define every provider or tool setting.

### FR-SPEC-003: JSON first

JSON shall be the canonical human-interchange format for specs and schemas. YAML support may be provided as an optional frontend.

### FR-SPEC-004: Lockable resolution

Applications shall be able to persist the resolved component identifiers and versions used to construct an agent.

### FR-SPEC-005: Separate packages

Providers, toolsets, stores, workflow adapters, and plugin hosts shall be separate package artifacts with explicit version and compatibility metadata. Bundled first-party packages follow the lockstep workspace version through pre-1.0; third-party packages are independently versioned, and post-1.0 first-party decoupling requires published engine-compatibility ranges. Cargo feature flags shall not be the primary product packaging mechanism.

# 10. Non-functional requirements

## 10.1 Performance

The following are engineering targets to be measured in versioned published benchmark environments; they are not assumed facts before implementation. Implementation Plan PR-063 owns evidence for NFR-PERF-001 through NFR-PERF-007 and ratifies the 1.0 reference workloads and budgets. Before PR-063 they drive warning/regression reporting; at G8 every applicable target must pass or have an approved unexpired exception. NFR-PERF-007 is a conformance rule rather than a numeric budget.

### NFR-PERF-001: Kernel transition overhead

A small, non-I/O state transition should have a target median below 5 microseconds and p99 below 25 microseconds on the reference desktop environment.

### NFR-PERF-002: Native dispatch

Resolved native model/tool dispatch overhead should be below 10 microseconds excluding implementation work and allocation of user payloads.

### NFR-PERF-003: Binding overhead

A run using exclusively Rust-backed components should complete within 10% of native Rust in the Python binding and within 15% in the browser WASM binding on standardized synthetic workloads, excluding host/network differences.

### NFR-PERF-004: Event throughput

The runtime should sustain at least 100,000 small in-process progress events per second before binding batching, without unbounded queue growth.

### NFR-PERF-005: Idle memory

An empty idle session should target less than 32 KiB of framework-owned memory excluding conversation content, provider clients, store caches, and application data.

### NFR-PERF-006: Minimal startup

The minimal native CLI benchmark should target sub-25-millisecond warm-filesystem startup on the reference desktop environment, with the kernel itself initialized in less than 1 millisecond.

### NFR-PERF-007: No repeated schema compilation

Tool and output schemas shall be normalized and validators prepared at registration or agent construction rather than for every call.

## 10.2 Reliability

### NFR-REL-001

No accepted durable run input may be silently lost after the store confirms acceptance.

### NFR-REL-002

Recovery from any journal prefix produced by the runtime shall yield a valid state or an explicit corruption error.

### NFR-REL-003

Cancellation, shutdown, and consumer disconnection shall not leave invalid tool-call history.

### NFR-REL-004

The runtime shall not use unbounded internal queues.

### NFR-REL-005

All shutdown paths shall be idempotent and safe to invoke after partial initialization.

## 10.3 Portability

### NFR-PORT-001

The kernel shall support current stable Rust on Linux, macOS, Windows, and `wasm32-unknown-unknown`.

### NFR-PORT-002

The standard Python package shall initially target CPython 3.11-3.14 plus free-threaded 3.14t, with wheels for manylinux x86_64/aarch64, macOS arm64, and Windows x64. Classic `abi3` shall not replace this matrix. An `abi3t`/combined stable-ABI strategy may be adopted for Python 3.15+ only after PyO3/maturin support, performance, and the project CI matrix are production-ready.

### NFR-PORT-003

Platform-specific provider, filesystem, sandbox, or service code shall not enter the kernel dependency graph.

## 10.4 Security

The Security and Threat Model v0.4 refines these outcomes into threat assumptions, control obligations, residual risks, and gate evidence. Those controls must remain within the product and architecture boundaries defined by this PRD.

### NFR-SEC-001

Native Rust and Python extensions are trusted code unless explicitly isolated.

### NFR-SEC-002

Isolated plugins shall receive no ambient authority by default.

### NFR-SEC-003

Secrets shall be passed through scoped handles or resolved configuration, not placed in prompts or general event payloads by default.

### NFR-SEC-004

File, network, subprocess, and secret permissions shall be independently scopeable.

### NFR-SEC-005

Observer streams and diagnostic/export projections of journals shall support redaction and metadata-only modes. Authoritative records must retain or securely reference all state required for replay. Version 1 exposes no field-level mutation, redaction, or tombstone operation over authoritative records; deployment-owned whole-session destruction is permitted only after outstanding work and callback credentials are closed/expired and permanently removes resumability. Any future selective authoritative-redaction feature requires an ADR, new versioned record/schema and migration semantics, and updated security/privacy evidence.

## 10.5 Compatibility

### NFR-COMP-001

Public Rust APIs, Python APIs, JavaScript APIs, journal schemas, remote protocols, and WIT packages shall each have explicit compatibility policies.

### NFR-COMP-002

A change to event order, journal meaning, or effect recovery rules requires a versioned architecture decision and conformance fixture update.

### NFR-COMP-003

Unknown fields in durable and remote records shall be handled according to documented forward-compatibility rules rather than silently discarded.

## 10.6 Developer experience

### NFR-DX-001

A minimal working agent should require no more than one model, one constructor/builder, and one `run` call.

### NFR-DX-002

Implementing a basic toolset should require fewer than 100 lines excluding the tools’ business logic and schemas.

### NFR-DX-003

Every extension port shall have a reference implementation, a fake/mock, a conformance suite, and generated API documentation.

### NFR-DX-004

Errors shall include stable codes, relevant identifiers, human-readable context, and remediation guidance where possible.

### NFR-DX-005

Bindings shall remain idiomatic rather than mirroring Rust types mechanically.

# 11. Representative API experience

## 11.1 Rust

```rust
use finstack_ai::{Agent, Capability, Result};
use finstack_ai_provider_openai_compatible::OpenAiCompatible;
use finstack_ai_tools_filesystem::FileSystem;

#[tokio::main]
async fn main() -> Result<()> {
    let coding = Capability::builder("coding")
        .instructions("Inspect before editing; verify after changes.")
        .toolset("filesystem")
        .build();

    let agent = Agent::builder(OpenAiCompatible::from_env()?)
        .register_toolset("filesystem", FileSystem::scoped("."))
        .capability(coding)
        .build()?;

    let result = agent.run("Explain the architecture of this repository").await?;
    println!("{}", result.text()?);
    Ok(())
}
```

## 11.2 Python

```python
from finstack_ai import Agent, Capability
from finstack_ai.providers import OpenAI
from finstack_ai.tools import FileSystem

agent = Agent(
    model=OpenAI.from_env(),
    toolsets={"filesystem": FileSystem(root=".")},
    capabilities=[
        Capability(
            id="coding",
            instructions=["Inspect before editing; verify after changes."],
            toolsets=["filesystem"],
        )
    ],
)

result = await agent.run("Explain the architecture of this repository")
print(result.text)
```

## 11.3 Python-defined tool

```python
from pydantic import BaseModel
from finstack_ai import Agent, tool

class Exposure(BaseModel):
    issuer: str
    market_value: float

@tool
async def portfolio_exposure(issuer: str) -> Exposure:
    return await lookup_exposure(issuer)

agent = Agent(model=model, tools=[portfolio_exposure])
result = await agent.run(
    "What is our exposure to Example Corp?",
    output_type=Exposure,
)
```

## 11.4 JavaScript/WASM

```typescript
import { Agent, Capability } from "@finstack/ai";

const agent = await Agent.create({
  model,
  toolsets: { filesystem },
  capabilities: [
    new Capability({
      id: "coding",
      toolsets: ["filesystem"],
    }),
  ],
});

const run = agent.start("Explain the architecture of this repository");
for await (const batch of run.events()) {
  render(batch);
}
console.log((await run.result()).text);
```

# 12. Distribution and product packaging

## 12.1 Rust crates

The intended public package family includes:

```text
finstack-ai                 # ergonomic facade / SDK
finstack-ai-kernel          # deterministic semantic core
finstack-ai-runtime         # standard native async driver
finstack-ai-protocol        # journal and remote protocol types
finstack-ai-test            # scripted model and conformance utilities
finstack-ai-provider-*      # provider packages
finstack-ai-tools-*         # toolset packages
finstack-ai-store-*         # persistence packages
finstack-ai-workflow-*      # durable workflow adapters
finstack-ai-plugin-host     # optional Wasmtime component host
```

## 12.2 Python packages

```text
finstack-ai                 # finstack_ai plus curated Rust-backed providers
finstack-ai[pydantic]       # optional binding-native validation ergonomics
finstack-ai-provider-*      # reserved for future callback/split providers
```

The initial Python distribution bundles the curated OpenAI-compatible, Anthropic, and local Rust-backed providers in one wheel. Rust crate boundaries remain intact, provider imports are lazy, optional pure-Python dependencies use extras, and CI enforces a wheel-size budget. A separately distributed Rust-backed provider cannot rely on Rust trait ABI compatibility across independent extension modules.

## 12.3 JavaScript packages

```text
@finstack/ai                # WASM kernel/runtime and TypeScript API
@finstack/ai-protocol       # optional pure TypeScript protocol helpers
```

## 12.4 WIT packages

```text
finstack:ai-toolset@0.0.4
finstack:ai-context@0.0.4
```

The plugin-alpha packages track the workspace `0.0.4` release and remain explicitly experimental 0.x interfaces. `@1.0.0` worlds are published only at the framework `1.0.0` gate after compatibility/security evidence. Observer isolation is outside the 1.0 baseline and requires a later ADR rather than a conditional package placeholder.

# 13. Delivery phases

## Phase A: Semantic kernel

- Finalize message, identifier, run-state, run-relation, interaction, event, deferred-effect, and journal models.
- Implement deterministic decision/apply engine.
- Implement scripted traces and property tests.
- Compile the kernel for native Rust and WASM.

## Phase B: Native runtime and SDK

- Tokio-based effect driver.
- One OpenAI-compatible model implementation.
- In-memory journal.
- Minimal toolset.
- Rust builder API.
- Streaming, cancellation, limits, and event batching.

## Phase C: Python and browser bindings

- PyO3 package with Rust-backed fast path.
- Python tool/model adapters.
- Pydantic schema adapter.
- `wasm-bindgen` package with JavaScript host adapters.
- Cross-binding conformance suite.

## Phase D: Durability and batteries

- SQLite journal store.
- Recovery and suspension tests.
- Filesystem and shell toolsets.
- General interaction routing with approval as the first middleware profile.
- Structured log observer.
- Session/lane APIs.

## Phase E: Isolated extension model

- WIT toolset and context worlds.
- Wasmtime host with permissions and limits.
- Reference component.
- Plugin conformance suite.

## Phase F: Server and ecosystem adapters

- Remote session protocol and reference server.
- OpenTelemetry observer.
- Additional providers and stores.
- Workflow integrations.
- Packaging and documentation for third-party extensions.

# 14. MVP definition

The MVP is complete when all of the following are available:

1. A deterministic kernel with a stable trace fixture format.
2. A native runtime that can complete text-only and tool-using runs.
3. Streaming, cancellation, limits, and parallel/sequential tool execution.
4. Stable identifiers, run lineage, generic deferral/interaction records, and an in-memory journal.
5. One native model provider and one native toolset.
6. Rust, Python, and browser WASM APIs using the same kernel.
7. Rust-backed Python and WASM paths that batch events.
8. A scripted model and deterministic conformance tests.
9. A published benchmark suite separating kernel, runtime, FFI, callback, and I/O costs.
10. Documentation that lets a developer create a model, toolset, middleware, and store implementation.

The MVP does not require SQLite, multi-lane execution, a WIT plugin host, remote server, or provider catalogue, but its data model must not prevent those features.

# 15. Release acceptance criteria

## 15.1 Semantic correctness

- All reference traces pass in native Rust and browser WASM.
- Python with Rust-backed components produces the same durable record sequence.
- Tool results remain in source order under parallel execution.
- Cancellation produces valid message history at every tested boundary.
- Every journal prefix produced by fault-injection tests restores or returns a documented corruption error.
- Child/delegated run lineage, outstanding interactions, and deferred effects reconstruct identically across bindings.
- Duplicate external completions are idempotent and conflicting duplicates fail with an auditable error.
- `before_finalize` can prevent terminal commit, while post-terminal observers cannot alter the result.
- `before_model` compaction produces the same model-visible projection across supported bindings, preserves mandatory instructions and tool-call/result validity, and leaves canonical history unchanged.

## 15.2 Performance

- Published benchmarks report native, Python Rust-backed, Python callback, browser WASM, and WASM-plugin paths separately.
- No binding benchmark hides network/model latency inside the framework result.
- Event queues remain bounded under a slow consumer test.
- No schema is regenerated during repeated calls after agent construction.

## 15.3 Packaging

- The kernel dependency audit contains no async runtime, HTTP, database, provider SDK, CLI, telemetry exporter, Python, or Wasmtime dependency.
- Python wheels install and run without a Rust toolchain.
- The npm package runs in a current evergreen browser.
- First-party providers and tools are not linked into the minimal kernel artifact.

## 15.4 Developer experience

- Minimal examples fit on one screen in Rust, Python, and TypeScript.
- Each extension port has a tested example.
- Error codes and event schemas are documented.
- A migration and compatibility policy is published before the first stable journal or WIT format.

# 16. Success metrics

## 16.1 Adoption metrics

- Number of independent applications embedding the Rust SDK.
- Number of active Python projects using Rust-backed components.
- Number of third-party provider/toolset/store packages.
- Percentage of users who use the minimal package without the server or plugin host.

## 16.2 Quality metrics

- Conformance pass rate across bindings.
- Crash-prefix recovery coverage.
- Fuzzing corpus size and discovered invariant violations.
- Mean time to diagnose failures using stable error codes and trace IDs.
- Percentage of public APIs covered by examples and reference docs.

## 16.3 Performance metrics

- Kernel transitions per second.
- Per-run framework CPU and allocation overhead.
- Memory per idle and active session.
- Binding conversion overhead.
- Stream-event throughput under slow and fast consumers.
- Journal replay throughput.

# 17. Risks and mitigations

| Risk | Consequence | Mitigation |
|---|---|---|
| Microkernel becomes a service locator | Complexity without product value | Limit ports to six; resolve once; architecture gate for new ports |
| Middleware surface expands endlessly | ABI instability and unclear behavior | Seven stable stages; immutable inputs by default; normalized outcomes |
| Compaction becomes hidden history mutation | Lost auditability, replay drift, or removed safety context | Own policy in `before_model` middleware; preserve canonical history; record versioned outcomes/checkpoints and protected-content tests |
| Python callbacks dominate execution | Poor performance and GIL contention | Rust-backed batteries; coarse calls; batch events; document callback costs |
| WASM ABI copies large payloads | Latency and memory pressure | Blob references, byte buffers, batching, size limits |
| Durability semantics overreach | False exactly-once guarantees | Explicit at-least-once model; stable idempotency keys; side-effect classification |
| Journal schema freezes too early | Expensive migrations | Version envelope, reserved fields, conformance fixtures, pre-1.0 evolution policy |
| Too many first-party batteries enter core | Monolith reappears | Dependency gates; separate packages; minimal artifact CI |
| Cross-binding APIs diverge | Inconsistent user experience | Shared semantic spec and trace suite; binding-specific ergonomic layer only |
| Provider-specific behavior leaks into kernel | Complexity and frequent core changes | Opaque extensions and normalized model capabilities |
| Plugin security is assumed rather than enforced | Unsafe third-party ecosystem | Default-deny WIT host, explicit permissions, resource limits, signed packages later |
| “Equally fast” is interpreted literally | Unrealistic expectations | Define near-native Rust-backed path; measure callback paths separately |

# 18. Resolved foundational product decisions

These directions are the planning baseline. PR-004 records the corresponding accepted ADRs before Phase 1 implementation; later PRs implement the decisions without reopening them implicitly.

| # | ADR | Decision | Delivery point |
|---|---|---|---|
| 1 | ADR-015 | Use deterministic, versioned CBOR as the canonical journal and protocol envelope; retain lossless JSON/JSONL diagnostic projection. | Profile frozen in PR-004; implemented in PR-039. |
| 2 | ADR-016 | Ship the first SQLite store before public multi-lane concurrency. Lane IDs and immutable parent-linked entries remain in the Phase 1 data model. | PR-040 precedes PR-047. |
| 3 | ADR-017 | Bundle curated Rust-backed providers in the single initial Python wheel while retaining separate Rust crates. | Finalized before PR-027. |
| 4 | ADR-018 | Support CPython 3.11-3.14 with per-version wheels and 3.14t where supported; skip classic `abi3` initially and gate Python 3.15+ `abi3t` adoption on production evidence. | Matrix approved before PR-027. |
| 5 | ADR-019 | Keep browser host interfaces as the contract and ship one optional fetch/SSE OpenAI-compatible adapter for same-origin proxies. | PR-034. |
| 6 | ADR-020 | Put durable activation mechanics plus always/application modes in MVP; complete model-activated catalog UX after MVP and before 0.1.0. | PR-012/PR-022 foundations; PR-032/PR-038 binding UX; PR-048 durability gate. |
| 7 | ADR-021 | Share bounded framing, envelope, and handshake code between remote and process protocols, but keep their message vocabularies distinct. | Generic framing in PR-058; process vocabulary later. |
| 8 | ADR-022 | Use JSON Schema draft 2020-12 as the source of truth, a default precompiled Rust validator, and optional binding-native validators behind shared fixtures. | ADR before PR-012; adapters in PR-031 and peers. |
| 9 | ADR-023 | Use an OpenAI-compatible Chat Completions baseline as the reference network provider; keep the scripted model as the semantic reference and Responses mapping optional. | PR-024. |
| 10 | ADR-024 | Dual-license under MIT OR Apache-2.0, use DCO sign-off, and govern through a named maintainer group plus the existing ADR/RFC process. | License/governance files in PR-001; ADR in PR-004. |

For decision 4, the support floor aligns with the [current CPython lifecycle](https://devguide.python.org/versions/), and the future stable-ABI direction follows [PEP 803](https://peps.python.org/pep-0803/) and the [PyO3 ABI guidance](https://pyo3.rs/main/building-and-distribution). The initial matrix remains version-specific until the project's own performance and compatibility gates pass.

# 19. Requirements traceability summary

| Product area | Requirement families | Primary architecture components |
|---|---|---|
| Agent semantics | FR-KRN | Kernel state, reducer, events, effects |
| Runtime execution | FR-RT | Native runtime, schedulers, batching |
| Modularity | FR-EXT, FR-CAP | Registrar, registry, AgentSpec, capability resolver |
| Models and tools | FR-MDL, FR-TLS | Model and Toolset ports |
| Context and policy | FR-CTX, FR-MW | Context pipeline and middleware chain |
| Durability | FR-DUR | Journal model, store port, recovery engine |
| Observability | FR-OBS | Immutable event batches and observers |
| Rust/Python/WASM | FR-RS, FR-PY, FR-WASM | SDK and binding crates |
| Isolated plugins | FR-PLG | WIT packages and optional host |
| Configuration | FR-SPEC | Versioned AgentSpec and namespaced extension config |

# 20. Glossary

**Agent microkernel:** The minimal Rust component that owns universal agent execution semantics while delegating external work and policy to typed ports.

**Binding fast path:** A Python or JavaScript run using Rust-backed components such that orchestration and most work stay inside Rust/WASM and language crossings occur only at coarse boundaries.

**Capability:** A declarative bundle of instructions and component references, potentially activated on demand.

**Context compaction:** A `before_model` middleware transformation that produces a bounded model-visible projection while preserving canonical conversation history, protected content, attribution, and replay evidence.

**Effect:** External work requested by the kernel and identified durably, such as a model call, tool invocation, interaction, or timer.

**Deferred effect:** An effect that has durably returned a non-secret external handle and will complete later through reconciliation, polling, or an authenticated callback.

**Extension:** Code that registers implementations of one or more framework ports.

**Journal:** Append-only durable record sequence from which session and run state can be reconstructed.

**Interaction:** A durable typed request for human or external input. Approval is the standard boolean/policy interaction profile.

**Lane:** Independently advancing conversation position within a session, with at most one active operation.

**Run relation:** Durable root/parent/effect lineage used for cancellation, deadlines, budget attribution, and audit correlation across nested or delegated runs.

**Observer:** Read-only consumer of immutable event batches.

**Plugin:** An extension distributed and executed through an isolation boundary such as WIT/WASM or a process protocol.

**Toolset:** A cohesive catalog and dispatcher for multiple related tools.

**Transient event:** A progress event useful to live consumers but not required to reconstruct durable state.
