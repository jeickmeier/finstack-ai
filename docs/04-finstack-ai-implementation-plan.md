---
title: "finstack-ai Implementation Plan"
subtitle: "Build phases, pull request sequence, delivery gates, and release roadmap"
version: "0.1"
status: "Draft for implementation planning"
date: "2026-08-08"
related-documents:
  - finstack-ai-PRD-v0.1
  - finstack-ai-Architecture-v0.1
  - finstack-ai-Technical-Design-v0.1
---

# Document control

| Field | Value |
| --- | --- |
| Product | finstack-ai |
| Document | Implementation Plan |
| Version | 0.1 |
| Status | Draft for implementation planning |
| Date | 2026-08-08 |
| Primary audience | Maintainers, implementation team, reviewers, release managers, and AI coding agents |
| Related documents | Product Requirements Document v0.1; Architecture Specification v0.1; Technical Design v0.1 |

# Executive implementation decision

`finstack-ai` will be built as a sequence of small vertical slices around a stable **agent microkernel**. The implementation order intentionally proves universal execution semantics before adding ecosystem breadth. The critical path is:

```text
repository governance
  -> deterministic kernel
  -> native runtime
  -> Rust SDK + one provider + useful tools
  -> native developer preview
```

After the native preview surface is stable, three workstreams may proceed in parallel:

```text
Python bindings       Browser WASM       Durability/recovery
         \                 |                 /
          \                |                /
           -> public semantic parity gates
                        |
                WIT/Wasmtime plugins
                        |
              ecosystem + public preview
                        |
                  1.0 hardening
```

The plan contains **66 logical pull requests** across ten phases. A logical PR may be split when reviewability requires it, but unrelated logical PRs must not be combined merely to reduce the count. The plan favors a continuously usable main branch, cross-language golden traces, and merge gates over a large feature branch or big-bang rewrite.

# 1. Purpose and use of this plan

## 1.1 Purpose

This document translates the PRD, Architecture Specification, and Technical Design into an executable build sequence. It defines:

- phase outcomes and entry/exit gates;
- logical pull requests and their dependencies;
- required tests and acceptance evidence;
- parallel workstreams and the critical path;
- preview and 1.0 release checkpoints;
- scope cut lines when schedule or staffing is constrained; and
- program-level definition of done.

## 1.2 How to use the PR sequence

Each PR entry is a **review unit**, not a promise that the implementation fits in one commit or one calendar week. The author may split a PR when any of the following is true:

- the change exceeds roughly 800-1,200 net hand-written lines;
- generated bindings obscure the human-authored change;
- a schema decision should be reviewed before the implementation;
- the test harness can land independently and reduce risk; or
- a high-risk algorithm benefits from a reference implementation followed by optimization.

A PR should not be combined with the next logical PR when the combination would mix architecture, behavior, packaging, and release concerns. Stacked PRs are permitted, but only the lowest independently reviewable PR should merge first.

# 2. Release targets and product increments

| Checkpoint | After phase | Primary outcome | Compatibility level |
| --- | --- | --- | --- |
| G1 - Kernel Semantics | Phase 1 | Deterministic I/O-free reducer and stable traces | Internal candidate schemas |
| G2 - Native Runtime | Phase 2 | Commit-before-effect runtime and bounded concurrency | Internal runtime contracts |
| 0.0.1-dev | Phase 3 | Rust SDK, real provider, useful toolsets | Developer preview; breaking changes allowed |
| 0.0.2 alpha | Phases 4-5 | Python and browser WASM parity for supported features | Alpha bindings |
| 0.0.3 beta | Phase 6 | SQLite durability, recovery, approvals, lanes | Beta journal/store schemas |
| 0.0.4 plugin alpha | Phase 7 | Permissioned WIT/Wasmtime extensions | Experimental external ABI |
| 0.1.0 public preview | Phase 8 | Ecosystem batteries, server, workflow adapters, docs | Published preview compatibility policy |
| 1.0.0 GA | Phase 9 | Stable contracts, audit, performance and release hardening | SemVer and schema compatibility commitments |

## 2.1 MVP and preview boundary

The **native MVP** is reached at the end of Phase 3. It includes the deterministic kernel, standard runtime, Rust SDK, one OpenAI-compatible provider, a calculator toolset, a constrained filesystem toolset, streaming, tools, limits, cancellation, structured output, in-memory journaling, tests, examples, and native benchmarks.

The **public preview** is reached at the end of Phase 8. It additionally includes first-class Python and browser WASM packages, SQLite durability and recovery, approvals, initial multi-lane sessions, isolated WIT plugins, more providers and batteries, a remote protocol/reference server, one durable-workflow integration, production observers, documentation, provenance, and published compatibility scope.

## 2.2 Explicitly deferred beyond 1.0 unless separately approved

- a public plugin marketplace and automatic update service;
- a broad catalog of chat channels and finished assistant applications;
- native dynamic-library plugins as a stable ABI;
- distributed multi-writer sessions or replicated journal consensus;
- a general workflow graph in the ordinary single-agent hot path;
- per-token Python, JavaScript, process, or WIT middleware callbacks;
- a hosted control plane, dashboard, billing system, or commercial package registry; and
- a claim of exactly-once external side effects.

# 3. Planning assumptions and schedule ranges

The following ranges are planning tools, not delivery commitments. They assume experienced Rust engineering, disciplined use of AI coding tools, automated testing, and prompt review of architectural decisions.

| Phase | Planning range | Parallelization |
| --- | --- | --- |
| 0. Foundation | 1-2 weeks | Mostly sequential |
| 1. Semantic kernel | 4-6 weeks | One core stream; tests may parallelize |
| 2. Native runtime | 4-6 weeks | Model, tools, and event work can partially parallelize |
| 3. Rust SDK/native preview | 3-4 weeks | Provider and tool batteries can parallelize after registry |
| 4. Python | 4-6 weeks | Parallel with Phases 5 and 6 after Phase 3 |
| 5. Browser WASM | 4-6 weeks | Parallel with Phases 4 and 6 after Phase 3 |
| 6. Durability | 6-8 weeks | Store work starts after Phase 2; integration follows Phase 3 |
| 7. Isolated plugins | 4-6 weeks | Starts after port/API candidate freeze |
| 8. Public preview | 4-6 weeks | Ecosystem work can begin earlier in leaf packages |
| 9. 1.0 hardening | 6-10 weeks | Driven by preview feedback and audits |

## 3.1 Staffing scenarios

- **One senior engineer with substantial AI assistance:** approximately 14-20 months to 1.0, with the native developer preview targeted first and some ecosystem scope likely deferred.
- **Three-person core team:** approximately 7-10 months to 1.0. Recommended role hats are kernel/runtime, bindings, and durability/ecosystem, with shared review.
- **Five-person team:** approximately 5-8 months to 1.0 if API ownership is clear. Additional staffing does not safely compress Phase 1 because the semantic kernel is the main coordination bottleneck.

A documentation/security/release contributor can reduce the end-of-program bottleneck even if that role is part-time.

# 4. Delivery principles

## 4.1 Vertical slices before ecosystem breadth

Every major layer must be proven through a runnable vertical slice before additional implementations are added. The first real provider and toolset arrive before a provider catalog. Python and WASM each begin with scripted components before host-language extension breadth.

## 4.2 Commit semantics before effect execution

Recoverable external work must never execute before its request record is durably committed. This invariant is established in Phase 2 and then reused by SQLite, approvals, lanes, workflow adapters, and plugins.

## 4.3 One semantic fixture suite

Rust, Python, browser WASM, persistent stores, and plugins use the same scripted trace corpus. Binding-specific tests are additional; they do not substitute for semantic conformance.

## 4.4 Direct native path by default

The microkernel boundary does not require IPC. Native providers, tools, stores, middleware, and observers resolve once and are retained as direct Rust handles. Python, JavaScript, and WIT crossings are coarse and explicitly benchmarked.

## 4.5 No hidden compatibility commitments

Before public preview, candidate schemas are marked clearly. At public preview, supported contracts and experimental packages are enumerated. At 1.0, only reviewed surfaces receive stability commitments.

## 4.6 Architecture work is code

Dependency checks, schema fixtures, queue bounds, panic policies, compatibility tests, and benchmark metadata are mergeable deliverables. They are not postponed documentation chores.

# 5. Workstreams and ownership

| Workstream | Primary ownership | Core outputs | Key review partner |
| --- | --- | --- | --- |
| Kernel semantics | Core/runtime lead | Domain types, reducer, records, effects, invariants | Durability lead |
| Native runtime | Core/runtime lead | Commit loop, scheduling, streaming, cancellation | Bindings lead |
| SDK/providers/tools | Ecosystem lead | Registrar, AgentSpec, providers, toolsets, examples | Core lead |
| Python | Bindings lead | PyO3 handles, callbacks, Pydantic adapter, wheels | Core lead |
| Browser WASM | Bindings lead or web specialist | wasm-bindgen package, JS adapters, worker, npm | Core lead |
| Durability | Durability/ecosystem lead | JournalStore, SQLite, recovery, approvals, lanes | Core lead |
| Isolated plugins | Runtime/security owner | WIT, Wasmtime host, permissions, guest SDK | Security reviewer |
| Quality/release | Shared or dedicated owner | Conformance, fuzzing, benchmarks, docs, provenance | All maintainers |

## 5.1 Critical-path dependency graph

```text
Phase 0
   |
Phase 1: kernel semantics
   |
Phase 2: native runtime
   |
Phase 3: Rust SDK/native preview
   |-------------------|-------------------|
   v                   v                   v
Phase 4: Python     Phase 5: WASM      Phase 6: durability
   |                   |                   |
   |-------------------|-------------------|
                       |
                Phase 7: plugins
                       |
                Phase 8: preview
                       |
                Phase 9: 1.0
```

Durability store work may begin after Phase 2, and WIT design may begin after the six port traits are candidate-stable. Neither should force changes into the Phase 1 kernel without an ADR.

# 6. Pull request operating model

## 6.1 Required PR sections

Every implementation PR must include:

- linked PRD requirement families and TDD/ADR sections;
- the user-visible or architecture outcome;
- schema/API compatibility impact;
- performance and allocation impact;
- security and trust-boundary impact;
- tests added or updated;
- documentation/examples updated;
- rollback or migration notes when persistent data is affected; and
- explicit out-of-scope items.

## 6.2 Definition of done for an ordinary PR

A PR is mergeable only when:

1. formatting, Clippy, unit/integration tests, architecture checks, and target builds pass;
2. new public behavior has tests at the correct layer;
3. any new record/event/spec/protocol field updates compatibility fixtures;
4. no queue is unbounded and cancellation ownership is documented;
5. public APIs have documentation and at least one example when user-facing;
6. benchmarks are added or updated for hot-path changes;
7. generated files are reproducible and isolated from hand-authored review where practical; and
8. no `unsafe` code is introduced without a dedicated safety rationale and reviewer.

## 6.3 ADR triggers

A separate ADR is required before merging a change that:

- adds a seventh primary extension port;
- adds an eighth normalized middleware stage;
- moves external I/O into the kernel;
- introduces a per-token language/process/plugin callback;
- changes commit-before-effect ordering;
- changes journal, remote protocol, or WIT compatibility policy;
- introduces native dynamic-library loading;
- adds a forbidden kernel dependency exception; or
- claims stronger external-effect guarantees than at-least-once plus idempotency/reconciliation.

# 7. Program gates

| Gate | Minimum evidence |
| --- | --- |
| G0 - Foundation ready | Workspace, architecture checks, CI, ADRs, trace/benchmark fixtures |
| G1 - Kernel semantics | Golden traces, exhaustive transitions, property/fuzz coverage, native/WASM kernel build |
| G2 - Native runtime | Commit-before-effect fault tests, bounded queues, task/cancellation leak checks |
| G3 - Native developer preview | Rust SDK, real provider, useful tools, examples, native benchmarks |
| G4 - Binding parity alpha | Python and browser WASM pass shared traces for supported features |
| G5 - Durable beta | SQLite, recovery matrix, approvals, lanes, migration fixtures |
| G6 - Plugin alpha | WIT conformance, permissions, resource limits, reference components |
| G7 - Public preview | Ecosystem batteries, server, workflow adapter, docs, security/release artifacts |
| G8 - 1.0 GA | Compatibility freeze, audit, performance budgets, release rehearsal and soak |

# 8. Phase 0: Foundation and architecture governance


**Outcome.** A buildable monorepo with automated architecture constraints, cross-target CI, versioning rules, and reusable trace/benchmark infrastructure.


**Planning range.** 1-2 weeks


**Traceability.** NFR-MAINT, NFR-PORT, NFR-SUPPLY; Architecture sections 2, 22; TDD sections 2-4, 32-34.


## Entrance criteria


- Approved PRD, Architecture Specification, and Technical Design v0.1.

- Repository ownership, licensing, and initial maintainer roles agreed.


## Exit criteria


- Every placeholder crate builds on supported targets.

- Forbidden kernel dependencies fail CI.

- Golden trace and benchmark fixture formats are merged.

- All architecture decisions needed for Phase 1 are recorded.


## Pull request sequence


### PR-001 - Create the workspace and package skeleton


**Purpose.** Establish the permanent repository shape without prematurely implementing agent behavior.


**Principal changes.**


- Create the Cargo workspace with `finstack-ai-kernel`, `finstack-ai-runtime`, `finstack-ai-sdk`, and `finstack-ai-protocol`.

- Add placeholder binding packages for Python and browser WASM, plus leaf directories for providers, toolsets, stores, examples, and test fixtures.

- Add license files, contribution guide, coding conventions, `rust-toolchain.toml`, workspace lints, and a minimal project README.

- Adopt lockstep pre-1.0 package versions and a single root changelog.


**Acceptance evidence.**


- `cargo metadata` exposes only the approved dependency direction.

- The workspace builds with default features and with the minimal kernel-only target.

- No provider, database, HTTP, UI, PyO3, wasm-bindgen, or Wasmtime dependency is present in the kernel crate.

- A new contributor can run the documented bootstrap commands from a clean checkout.


**Dependencies.** None.


**Traceability.** Architecture ADR-001, ADR-005, ADR-010; TDD section 2.


**Explicitly excluded.** No public API, state machine, provider, or tool implementation.


### PR-002 - Add architecture and dependency enforcement


**Purpose.** Make the microkernel boundary mechanically enforceable rather than relying on documentation.


**Principal changes.**


- Add an `xtask architecture` command that inspects `cargo metadata` and rejects forbidden direct or transitive dependencies.

- Add source-level guards for concrete provider/tool/store imports in kernel modules and for central provider/tool match registries.

- Create checks for unbounded async channels and public task-local request context.

- Add a review checklist file used by pull request templates.


**Acceptance evidence.**


- A fixture branch that adds `tokio`, `reqwest`, `rusqlite`, `pyo3`, or `wasmtime` to the kernel fails CI with a clear diagnostic.

- A fixture provider and toolset can be added without editing kernel code.

- Architecture checks run in less than one minute on a normal CI runner.

- Exceptions require an ADR identifier and an explicit allowlist entry.


**Dependencies.** PR-001.


**Traceability.** Architecture sections 2.1, 2.2, 22; PRD NFR-MAINT.


**Explicitly excluded.** No semantic lint for state-machine correctness; that arrives with the kernel.


### PR-003 - Establish the cross-platform CI and release build matrix


**Purpose.** Create fast feedback for Rust, Python, and WASM work before feature development begins.


**Principal changes.**


- Add formatting, Clippy, unit-test, documentation, minimal-feature, and architecture jobs for Linux, macOS, and Windows.

- Add `wasm32-unknown-unknown` compilation, headless browser smoke-test placeholders, and Python wheel smoke-test placeholders.

- Add supply-chain checks, license policy, dependency advisories, and reproducible release profiles.

- Configure benchmark artifacts and build metadata retention without making microbenchmarks merge-blocking yet.


**Acceptance evidence.**


- All jobs run on pull requests with path-aware skipping only where safe.

- MSRV, stable, and nightly/fuzz jobs have explicit ownership and cadence.

- A minimal release binary can be produced for Linux, macOS, and Windows.

- CI documents how generated bindings and fixtures are verified.


**Dependencies.** PR-001 and PR-002.


**Traceability.** TDD sections 33-34; PRD NFR-PORT and NFR-SUPPLY.


**Explicitly excluded.** No publication to crates.io, PyPI, or npm.


### PR-004 - Record foundational ADRs and schema governance


**Purpose.** Freeze the decisions that later PRs must not silently reinterpret.


**Principal changes.**


- Convert the Architecture Specification decision summary into versioned ADR files.

- Define ownership and compatibility rules for public Rust APIs, journal records, runtime events, AgentSpec, remote protocol DTOs, and WIT packages.

- Create schema directories, fixture naming conventions, and a change-classification template.

- Define the pre-1.0 breakage policy and the gates for introducing a seventh extension port or an eighth middleware stage.


**Acceptance evidence.**


- ADR-001 through ADR-014 are present and cross-linked to the design documents.

- Every versioned schema family has an owner, compatibility promise, and test location.

- The PR template requires API/schema/performance/security impact statements.

- A schema change without a fixture update fails CI once schemas exist.


**Dependencies.** PR-001.


**Traceability.** Architecture section 25; TDD section 37.


**Explicitly excluded.** No final choice on journal encoding beyond a documented evaluation and decision deadline.


### PR-005 - Create golden trace, conformance, and benchmark harnesses


**Purpose.** Provide one reusable test language for Rust, Python, WASM, recovery, and performance work.


**Principal changes.**


- Define a deterministic scripted input format for model chunks, tool calls, tool results, cancellation, errors, and timers.

- Define the golden trace format for durable records, public events, effects, and final state hashes.

- Add a target-neutral conformance runner with placeholder Rust, Python, and WASM adapters.

- Add Criterion benchmark groups and a machine-readable benchmark metadata format.


**Acceptance evidence.**


- A trivial no-op trace can be loaded, normalized, and compared byte-for-byte.

- Trace fixtures distinguish durable records from transient stream events.

- Benchmark output records compiler, target, commit, feature set, and machine metadata.

- Fixture schemas reject unknown fields and oversized payload declarations.


**Dependencies.** PR-001, PR-003, and PR-004.


**Traceability.** TDD sections 32-33; PRD NFR-TEST and NFR-PERF.


**Explicitly excluded.** No claim of cross-language parity until real bindings exist.


# 9. Phase 1: Semantic agent microkernel


**Outcome.** A deterministic, I/O-free reducer that owns messages, model/tool continuation, limits, cancellation, structured output, journal decisions, and event ordering.


**Planning range.** 4-6 weeks


**Traceability.** FR-KRN, FR-CAP foundations, FR-DUR effect identity; Architecture sections 4, 8-10; TDD sections 5-13.


## Entrance criteria


- Phase 0 gate passed.

- ADR decisions for IDs, raw JSON, event envelopes, and reducer API resolved.


## Exit criteria


- Kernel-only builds for native Rust and browser WASM.

- Golden traces cover model-only, tool, retry, cancellation, limit, and structured-output flows.

- Property and fuzz tests enforce invariants.

- No external effect executes inside the kernel.


## Pull request sequence


### PR-006 - Implement typed identifiers, raw JSON, and stable errors


**Purpose.** Create the low-level value types used by every later schema and binding.


**Principal changes.**


- Implement typed run, session, lane, turn, message, tool-call, effect, event, and record identifiers.

- Implement `RawJson` over shared immutable bytes with validated and unchecked constructors separated.

- Define stable error codes, categories, retryability metadata, and diagnostic context without source-language exceptions.

- Add deterministic ID injection for tests and a production UUIDv7 implementation behind a small clock/random interface.


**Acceptance evidence.**


- All IDs serialize canonically and reject type confusion in Rust APIs.

- Raw JSON can cross clone boundaries without copying the payload.

- Errors round-trip through JSON fixtures with stable codes.

- The kernel remains free of OS randomness, clocks, and async runtime dependencies.


**Dependencies.** Phase 0.


**Traceability.** FR-KRN-005; TDD sections 5-6 and 30.


**Explicitly excluded.** No message semantics or provider-specific error mapping.


### PR-007 - Implement content blocks, messages, and blob references


**Purpose.** Define the canonical data model shared by models, tools, stores, bindings, and protocols.


**Principal changes.**


- Add text, image/blob reference, audio/blob reference, document/blob reference, and structured-data content blocks.

- Define user, assistant, tool-result, and system/instruction message roles with immutable message entries.

- Add tool call and tool result association fields, provider extension payloads, usage placeholders, and timestamps supplied by the environment.

- Define a `BlobRef` that carries identity, media type, length, and optional integrity digest without embedding large payloads.


**Acceptance evidence.**


- Message fixtures serialize identically across native and `wasm32` builds.

- Invalid tool-result associations are rejected.

- Large content is represented by references rather than copied inline.

- Public types have rustdoc examples and schema fixtures.


**Dependencies.** PR-006.


**Traceability.** FR-KRN-001; Architecture sections 9 and 16; TDD section 7.


**Explicitly excluded.** No blob store implementation or model wire adapter.


### PR-008 - Define runtime events, journal records, and effect envelopes


**Purpose.** Separate durable truth, transient progress, and requested external work.


**Principal changes.**


- Create versioned `RunEvent`, `JournalRecord`, `EffectRequest`, and `EffectResult` envelopes.

- Classify each event as durable, transient, diagnostic, or binding-local.

- Define record batches, state version preconditions, effect idempotency keys, and event sequence numbers.

- Add schema fixtures and forward-compatible unknown-field rejection rules for v1.


**Acceptance evidence.**


- Durable and transient event classes cannot be confused through public constructors.

- Every recoverable effect request has a stable `EffectId` and normalized input hash.

- Record ordering is deterministic under an injected transition environment.

- Schema fixture diffs are merge-blocking.


**Dependencies.** PR-006 and PR-007.


**Traceability.** FR-KRN-006, FR-KRN-007; FR-DUR foundations; TDD sections 12 and 20.


**Explicitly excluded.** No store or runtime commit loop.


### PR-009 - Implement the model-only run reducer


**Purpose.** Prove the central decide/apply architecture using the smallest useful agent flow.


**Principal changes.**


- Implement explicit run phases for accepted, preparing, awaiting model, applying model response, completed, failed, and cancelled.

- Add command-level inputs and `decide` output containing record batches, public events, and model effects.

- Add `apply` functions that mutate state only from committed records.

- Support text streaming as transient events and final assistant message commitment as durable state.


**Acceptance evidence.**


- A model-only golden trace produces the same state hash on repeated replay.

- No effect is emitted before the corresponding request record is part of the decision.

- Invalid phase/input combinations return stable errors without panicking.

- Model stream chunk count does not affect the final durable trace.


**Dependencies.** PR-006 through PR-008.


**Traceability.** FR-KRN-002 through FR-KRN-004; TDD section 11.


**Explicitly excluded.** No tools, middleware, context providers, or retries.


### PR-010 - Add tool-call and tool-batch semantics to the reducer


**Purpose.** Extend the canonical loop while keeping scheduling policy explicit and recoverable.


**Principal changes.**


- Add assistant tool-call messages, validated tool-call state, tool-batch creation, and tool-result application.

- Define sequential barriers, parallel groups, source-order finalization, duplicate call handling, and unknown tool behavior.

- Ensure every accepted tool call receives exactly one durable result or synthetic closure result.

- Add continuation rules for returning tool results to the model and terminating all-terminate batches.


**Acceptance evidence.**


- Parallel tools may complete in any runtime order but durable history finalizes in assistant source order.

- Cancellation and failure never leave an unmatched tool call in restored history.

- Duplicate completion for an effect is detected and handled idempotently.

- Golden traces cover mixed sequential/parallel batches and partial failure.


**Dependencies.** PR-009.


**Traceability.** FR-KRN-002, FR-RT-003 semantics, FR-TLS; TDD sections 15 and 21.


**Explicitly excluded.** No actual concurrent executor.


### PR-011 - Add limits, cancellation, deadlines, and retry state


**Purpose.** Make termination semantics part of the kernel rather than runtime-specific behavior.


**Principal changes.**


- Add step, token, cost, tool-call, wall-clock/deadline, and output-size limits.

- Add cancellation intent, cancellation acknowledgement, reconciliation state, and stable cancellation reasons.

- Represent retry decisions, backoff requests, attempt counters, retry budgets, and terminal classification.

- Define precedence when cancellation, limits, model output, and tool completion race.


**Acceptance evidence.**


- Every limit is tested at below, exact, and above-boundary values.

- Cancellation is idempotent and replay-safe from every run phase.

- Retry attempts survive record replay and cannot exceed the configured budget.

- Race-order fixture permutations converge on valid documented outcomes.


**Dependencies.** PR-009 and PR-010.


**Traceability.** FR-KRN-008 through FR-KRN-010; TDD section 22.


**Explicitly excluded.** No Tokio cancellation tokens or real timers.


### PR-012 - Add structured output and internal control tools


**Purpose.** Support typed final results and framework-owned control calls without special-case provider code.


**Principal changes.**


- Define output schema references, structured result candidates, validation outcomes, retry feedback, and final result records.

- Define internal tool identities for final-output submission and optional capability loading.

- Add output end strategies and clear handling of text plus tool calls in one model response.

- Keep schema validation execution outside the kernel while making its result semantics deterministic.


**Acceptance evidence.**


- Structured-output traces cover success, validation retry, exhausted retries, and competing output/tool calls.

- Internal tools are namespaced and cannot collide with application tools.

- The kernel never imports a JSON Schema or Pydantic implementation.

- Plain text remains the zero-configuration default.


**Dependencies.** PR-010 and PR-011.


**Traceability.** FR-KRN-011, FR-KRN-012, FR-CAP foundations; TDD sections 10 and 17.


**Explicitly excluded.** No on-demand capability UX or concrete validator.


### PR-013 - Harden the kernel and pass the semantic gate


**Purpose.** Turn the initial reducer into a stable foundation before runtime and bindings depend on it.


**Principal changes.**


- Complete exhaustive transition tests, property tests, model-based tests, and fuzz targets for records, events, and message application.

- Add invalid-input, oversized-input, unknown-version, and corrupt-replay fixture suites.

- Compile and test the kernel for native and `wasm32-unknown-unknown` targets.

- Publish an internal semantic reference describing every phase, command, record, effect, and invariant.


**Acceptance evidence.**


- All Phase 1 golden traces are stable and reviewed.

- No public kernel function panics on untrusted serialized input.

- Fuzz smoke tests run in CI and longer campaigns run on schedule.

- Architecture gate G1 - Kernel Semantics - is signed off.


**Dependencies.** PR-006 through PR-012.


**Traceability.** All FR-KRN requirements; TDD sections 32 and 36.


**Explicitly excluded.** No compatibility promise beyond fixtures marked `candidate-v1`.


# 10. Phase 2: Native runtime and effect execution


**Outcome.** A Tokio-based runtime that commits records before effects, streams models, schedules tools, applies context and middleware, batches events, and supports bounded cancellation-safe concurrency.


**Planning range.** 4-6 weeks


**Traceability.** FR-RT, FR-MDL, FR-TLS, FR-CTX, FR-MW, FR-OBS; Architecture sections 5-12; TDD sections 13-23.


## Entrance criteria


- Phase 1 semantic gate passed.

- Kernel public types available as a versioned internal crate interface.


## Exit criteria


- Model-only and tool-using runs execute end to end against scripted components.

- Commit-before-effect is verified by fault injection.

- All queues are bounded.

- Runtime cancellation and slow-consumer behavior are tested.


## Pull request sequence


### PR-014 - Implement the runtime commit loop and in-memory journal


**Purpose.** Create the authoritative driver that turns kernel decisions into committed state and external work.


**Principal changes.**


- Implement the decide -> commit -> apply -> dispatch loop with optimistic state versions.

- Add an in-memory `JournalStore` suitable for tests and non-durable runs.

- Define runtime task ownership, run handles, shutdown behavior, and faulted-runtime state.

- Add fault injection before commit, after commit, before dispatch, and after effect completion.


**Acceptance evidence.**


- A recoverable effect is never dispatched when its request batch fails to commit.

- Reapplying committed records reconstructs the same kernel state.

- Store failure faults the affected run predictably and emits diagnostics.

- The no-durability configuration uses the same loop with the in-memory store.


**Dependencies.** Phase 1.


**Traceability.** FR-RT-001; FR-DUR-001 foundations; TDD section 13.


**Explicitly excluded.** No network models, tools, or persistent database.


### PR-015 - Implement the Model port and scripted stream driver


**Purpose.** Define the provider-neutral model contract and prove streaming through the runtime.


**Principal changes.**


- Add the object-safe `Model` trait, request type, model capabilities, model metadata, and normalized stream items.

- Implement a scripted model used by golden traces, retries, suspensions, malformed streams, and cancellation tests.

- Add model resource reuse, connection warmup hooks, and per-request context without provider-specific fields in the kernel.

- Bridge model stream items into transient events and final kernel inputs.


**Acceptance evidence.**


- One-, ten-, hundred-, and thousand-chunk scripted responses produce equivalent durable outcomes.

- Slow and cancelled consumers do not leak model tasks.

- Malformed provider sequences produce stable adapter errors.

- The Model trait is implementable in a leaf crate without kernel edits.


**Dependencies.** PR-014.


**Traceability.** FR-MDL; FR-RT-002 and FR-RT-006; TDD section 14.


**Explicitly excluded.** No real provider or provider routing.


### PR-016 - Implement the Toolset port and scheduler


**Purpose.** Execute model-requested tools through one coarse extension boundary.


**Principal changes.**


- Add `Toolset`, `ToolSpec`, argument validation adapter, tool call context, output streaming, and idempotency metadata.

- Implement sequential and parallel execution groups with bounded concurrency and cancellation token children.

- Add a scripted toolset that supports success, progress, timeout, error, panic containment, and duplicate completion fixtures.

- Return normalized results to the kernel in source-order finalization.


**Acceptance evidence.**


- Scheduler concurrency never exceeds configured limits.

- A panicking native tool is isolated to its call and converted to a stable failure.

- Parallel completion order does not change durable history.

- Tool arguments are validated once at the chosen boundary.


**Dependencies.** PR-014 and PR-015.


**Traceability.** FR-TLS and FR-RT-003; TDD sections 15 and 21.


**Explicitly excluded.** No filesystem or shell implementation.


### PR-017 - Implement the event hub, batching, and backpressure


**Purpose.** Expose responsive streams without making per-token language crossings or unbounded queues part of the design.


**Principal changes.**


- Add bounded subscriber queues, event sequence tracking, lag policy, and subscriber shutdown semantics.

- Implement configurable time/size event coalescing while preserving logical event order.

- Separate observer delivery from interactive run subscribers.

- Add slow-subscriber, disconnected-subscriber, large-stream, and multi-subscriber tests.


**Acceptance evidence.**


- No runtime queue is unbounded.

- Batching changes transport granularity but not logical event order.

- A slow optional observer cannot block the active run indefinitely.

- Dropped transient progress is reported while durable terminal events remain observable through snapshots/results.


**Dependencies.** PR-014 through PR-016.


**Traceability.** FR-RT-004, FR-RT-005; FR-OBS; TDD section 20.


**Explicitly excluded.** No network event protocol.


### PR-018 - Implement ContextProvider, Middleware, and Observer ports


**Purpose.** Complete the six-port composition surface with constrained behavior-changing and read-only extension points.


**Principal changes.**


- Add context provider budgets, deterministic contribution ordering, source attribution, and truncation diagnostics.

- Implement the seven normalized middleware stages and ordering resolution.

- Add immutable observer event batches and a no-op/reference observer.

- Record non-recomputable middleware outcomes when durability mode requires replay-safe behavior.


**Acceptance evidence.**


- Adding context, middleware, or observer fixtures requires no kernel changes.

- Middleware cycles or missing requirements fail at agent resolution, not mid-run.

- Observers cannot mutate execution state through the public API.

- Context budget overrun has a deterministic policy and diagnostic.


**Dependencies.** PR-014 and PR-017.


**Traceability.** FR-CTX, FR-MW, FR-OBS; Architecture sections 6 and 11; TDD sections 16-19.


**Explicitly excluded.** No semantic memory, compaction policy, or telemetry exporter.


### PR-019 - Integrate cancellation, deadlines, retries, and timers


**Purpose.** Connect kernel termination semantics to real async tasks and clocks.


**Principal changes.**


- Create a runtime cancellation-token tree for run, model request, tool batch, and individual tool calls.

- Implement deadline propagation and a clock/timer adapter usable by tests and durable runtimes.

- Implement retry scheduling with jitter supplied outside the kernel and attempts persisted by kernel records.

- Define shutdown grace periods and forced task termination diagnostics.


**Acceptance evidence.**


- Cancellation at every scripted boundary reaches terminal valid state.

- Deadlines are enforced without polling loops.

- Retries resume with the correct attempt count after runtime restart in fixture simulations.

- No background task outlives its owning run after settlement.


**Dependencies.** PR-014 through PR-018.


**Traceability.** FR-KRN-009, FR-KRN-010, FR-RT-007; TDD section 22.


**Explicitly excluded.** No durable database or workflow-engine clock.


### PR-020 - Pass the native runtime fault and concurrency gate


**Purpose.** Stabilize the runtime before adding public SDK and binding commitments.


**Principal changes.**


- Complete runtime integration tests, fault-injection matrices, race tests, leak checks, and concurrency benchmarks.

- Add deterministic manual-drive mode so tests can stop before every external effect.

- Document task ownership, queue bounds, scheduling order, and runtime failure semantics.

- Publish benchmark baselines for reducer cost, stream throughput, tool throughput, and idle-session memory.


**Acceptance evidence.**


- Crash-prefix simulations preserve commit-before-effect invariants.

- Sanitizer/Miri-compatible suites cover unsafe-free core paths where feasible.

- No known task, stream, or channel leak remains in stress tests.

- Architecture gate G2 - Native Runtime - is signed off.


**Dependencies.** PR-014 through PR-019.


**Traceability.** FR-RT completion; NFR-PERF, NFR-REL, NFR-TEST.


**Explicitly excluded.** No public provider-facing product API.


# 11. Phase 3: Rust SDK and native developer preview


**Outcome.** An ergonomic Rust API, declarative capabilities, one production HTTP provider, useful tool batteries, examples, and a benchmarked native developer preview.


**Planning range.** 3-4 weeks


**Traceability.** FR-EXT, FR-CAP, FR-SPEC, representative APIs, distribution; TDD sections 8-10 and 29.


## Entrance criteria


- Phase 2 gate passed.

- Six extension-port traits are candidate-stable for pre-1.0 use.


## Exit criteria


- A new Rust user can build a model-only and tool-using agent in a few lines.

- One real provider and useful tools operate through direct Rust calls.

- Native overhead benchmarks are published.

- Developer preview artifacts are reproducible.


## Pull request sequence


### PR-021 - Implement Registrar, Registry, and resolved-agent construction


**Purpose.** Resolve named extensions once and retain direct handles for the hot path.


**Principal changes.**


- Implement extension registration for models, toolsets, context providers, middleware, stores, and observers.

- Add namespacing, duplicate policy, aliases, lifecycle hooks, and resolution diagnostics.

- Create immutable `ResolvedAgent` and `ResolvedRunPlan` structures.

- Verify that no registry lookup occurs per token or per tool progress event.


**Acceptance evidence.**


- Duplicate and missing-extension diagnostics identify the source registration.

- Resolution is deterministic independent of hash-map iteration order.

- Resolved handles are direct `Arc` trait objects for native components.

- Fixture extensions can be added without editing central lists.


**Dependencies.** Phase 2.


**Traceability.** FR-EXT; Architecture section 7; TDD section 9.


**Explicitly excluded.** No package discovery or dynamic library loading.


### PR-022 - Implement AgentSpec, AgentBuilder, and CapabilitySpec


**Purpose.** Provide concise composition while keeping executable extensions distinct from declarative capabilities.


**Principal changes.**


- Add serializable AgentSpec and ergonomic Rust builder APIs.

- Add CapabilitySpec with instructions, toolsets, context providers, middleware, visibility, and activation metadata.

- Implement spec validation, defaults, version fields, environment-independent resolution, and human-readable diagnostics.

- Add opt-in on-demand capability state without requiring it in the minimal path.


**Acceptance evidence.**


- Equivalent builder and JSON spec inputs resolve to the same agent fingerprint.

- Unknown fields, incompatible versions, duplicate capability IDs, and unresolved references fail before run start.

- The simplest model-only agent requires no capability object.

- Capabilities contain no executable function pointers in the serialized form.


**Dependencies.** PR-021.


**Traceability.** FR-CAP and FR-SPEC; TDD sections 8, 10, and 29.


**Explicitly excluded.** No remote spec registry or visual configuration UI.


### PR-023 - Publish the scripted test kit and extension conformance helpers


**Purpose.** Make provider, toolset, store, middleware, and binding implementations easy to test correctly.


**Principal changes.**


- Expose scripted model/toolset/context/store fixtures from a dedicated test-support crate.

- Add conformance macros/functions for each extension port.

- Add deterministic clocks, ID sources, effect drivers, trace assertions, and slow/failing component helpers.

- Provide example tests that third-party crates can copy without depending on private internals.


**Acceptance evidence.**


- A sample out-of-tree provider and toolset pass conformance tests.

- The test kit has no production dependency from the kernel or runtime.

- Conformance failures explain the violated contract.

- Golden traces can be driven from public SDK APIs.


**Dependencies.** PR-021 and PR-022.


**Traceability.** NFR-TEST; FR-EXT-006; TDD section 32.


**Explicitly excluded.** No certification or marketplace badge.


### PR-024 - Implement the OpenAI-compatible native provider


**Purpose.** Prove the Model port against a real streaming HTTP implementation reusable across many endpoints.


**Principal changes.**


- Implement chat/responses-compatible request mapping, SSE streaming, structured tool calls, usage, errors, timeouts, and cancellation.

- Add configurable base URL, headers, authentication, model metadata, and capability declarations.

- Normalize provider extension fields without leaking wire DTOs into the kernel.

- Add recorded HTTP fixtures and optional live smoke tests.


**Acceptance evidence.**


- Streaming text, tool calls, structured output, retryable errors, and cancellation pass fixtures.

- The provider is entirely omitted from kernel-only and runtime-only builds.

- Secrets are never emitted in diagnostics or traces.

- Warm connection reuse and request overhead are benchmarked.


**Dependencies.** PR-015, PR-021, and PR-022.


**Traceability.** FR-MDL; PRD UC-01 and UC-06.


**Explicitly excluded.** No provider router, OAuth flow, or full model catalog.


### PR-025 - Implement calculator and minimal filesystem toolsets


**Purpose.** Provide useful batteries that demonstrate both pure and resource-scoped tools.


**Principal changes.**


- Add a deterministic calculator toolset for baseline tests and examples.

- Add read, write, edit, list, glob, and content-search filesystem tools scoped to an authorized root.

- Resolve symlinks before authorization; add protected and denied path patterns; bound reads and search output.

- Use blob references or bounded spill files for oversized results.


**Acceptance evidence.**


- Path traversal, symlink escape, protected files, and oversized output tests pass.

- Tool specs are generated once and reused across turns.

- All tool calls carry effect and principal context.

- The calculator path provides a stable high-volume benchmark.


**Dependencies.** PR-016, PR-021, and PR-022.


**Traceability.** FR-TLS; PRD UC-02.


**Explicitly excluded.** No shell execution, Git integration, browser automation, or OS sandbox.


### PR-026 - Ship the native vertical slice and developer preview


**Purpose.** Package the first end-to-end experience and freeze the interfaces needed by bindings.


**Principal changes.**


- Add minimal, coding, and service examples plus a small diagnostic CLI example.

- Publish native benchmarks, binary-size reports, memory profiles, and architecture diagrams.

- Add crates.io publication dry runs, changelog generation, and documentation-site scaffolding.

- Declare the candidate binding surface and defer incompatible changes behind an ADR.


**Acceptance evidence.**


- A clean user project can add the SDK, provider, and toolset crates and complete a run.

- Native model-only and tool-loop examples pass on Linux, macOS, and Windows.

- Performance regressions have initial warning thresholds.

- Release checkpoint `0.0.1-dev` is published or reproducibly staged.


**Dependencies.** PR-021 through PR-025.


**Traceability.** MVP native slice; TDD milestone 3.


**Explicitly excluded.** No Python, WASM, durability database, or plugin ABI promise.


# 12. Phase 4: First-class Python bindings


**Outcome.** A PyO3 package that uses the same Rust engine, preserves the Rust-backed fast path, supports coarse Python callbacks and Pydantic schemas, and passes shared conformance traces.


**Planning range.** 4-6 weeks; may run in parallel with Phases 5 and 6 after Phase 3


**Traceability.** FR-PY, NFR-PERF, NFR-PORT; Architecture section 13; TDD section 25.


## Entrance criteria


- Phase 3 binding surface declared.

- Python package naming and wheel support matrix approved.


## Exit criteria


- Rust-backed Python runs pass the shared trace suite.

- Python callback boundaries are coarse and benchmarked.

- Pydantic inputs/outputs work without placing Pydantic in the Rust core.

- Wheels install without a Rust toolchain on supported platforms.


## Pull request sequence


### PR-027 - Create the PyO3 package and wheel pipeline


**Purpose.** Establish packaging, import, versioning, and platform support before exposing complex APIs.


**Principal changes.**


- Create the `finstack_ai` native module and thin Python package layout.

- Configure maturin, type stub generation, platform wheel builds, source distribution, and local editable development.

- Expose version/build metadata and a minimal health function.

- Add Python linting, typing, test, import-time, and wheel smoke jobs.


**Acceptance evidence.**


- Wheels install and import on the declared CPython/platform matrix.

- Import does not initialize a Tokio runtime or open network resources.

- Package versions align with workspace release metadata.

- No Python toolchain dependency leaks into Rust-only builds.


**Dependencies.** PR-026.


**Traceability.** FR-PY-001 and distribution requirements.


**Explicitly excluded.** No Agent or callback API.


### PR-028 - Expose Agent, Run, Session, Result, and EventBatch handles


**Purpose.** Create an idiomatic async Python control surface while retaining Rust-owned state.


**Principal changes.**


- Expose Python handle classes backed by `Arc` Rust objects rather than copied state.

- Implement async `run`, `start`, `cancel`, `result`, and event-batch iteration.

- Expose immutable snapshots and explicit serialization methods only on request.

- Map stable Rust errors to a documented Python exception hierarchy.


**Acceptance evidence.**


- A model-only scripted run passes from Python without Python callbacks.

- Dropping Python handles cancels or detaches according to documented semantics without leaks.

- Event batching avoids one FFI call per model token.

- Python exceptions preserve stable error codes and context.


**Dependencies.** PR-027 and Phase 3 SDK.


**Traceability.** FR-PY-002 through FR-PY-004; TDD sections 25.2-25.5.


**Explicitly excluded.** No Python-defined model or tool.


### PR-029 - Optimize the Rust-backed Python fast path and GIL behavior


**Purpose.** Ensure Python is a first-class frontend rather than the owner of native orchestration.


**Principal changes.**


- Release the GIL during Rust-only scheduling, HTTP streaming, native tools, storage, and waiting.

- Use shared byte buffers for raw JSON and blob metadata where practical.

- Coalesce events by byte/time thresholds and expose efficient conversion paths.

- Add allocation, FFI-crossing, throughput, and idle-memory benchmarks.


**Acceptance evidence.**


- Rust-backed Python synthetic runs remain within the documented overhead target versus native Rust.

- No callback into Python occurs for each token by default.

- Concurrent Python tasks can await independent Rust runs without serializing on the GIL.

- Benchmark reports separate import, construction, FFI, and external I/O time.


**Dependencies.** PR-028.


**Traceability.** NFR-PERF; Architecture section 13.2; TDD section 25.5.


**Explicitly excluded.** No promise for near-native speed when user code itself runs in Python.


### PR-030 - Add Python model, toolset, context, middleware, and observer adapters


**Purpose.** Allow Python application logic at deliberate coarse extension boundaries.


**Principal changes.**


- Implement Python callback adapters with async/sync support, cancellation signals, timeouts, and error normalization.

- Cache tool metadata and schemas at registration rather than on each call.

- Batch observer events and prohibit per-token middleware hooks in the default API.

- Document thread-safety, reentrancy, state ownership, and callback lifetime.


**Acceptance evidence.**


- One Python model and one Python toolset pass port conformance tests.

- Cancellation reaches Python callbacks and late completion is handled safely.

- Callback exceptions become stable framework errors with sanitized tracebacks.

- A Python callback cannot retain a stale run context after settlement.


**Dependencies.** PR-028 and PR-029.


**Traceability.** FR-PY-005; all six port adapters where supported.


**Explicitly excluded.** No arbitrary Python subclass of the kernel or reducer.


### PR-031 - Add Pydantic-compatible tool and structured-output adapters


**Purpose.** Match the strongest Python developer ergonomics without moving orchestration into Python.


**Principal changes.**


- Accept Pydantic models, dataclasses, TypedDicts, and TypeAdapter-compatible types.

- Generate and cache JSON Schema at registration; validate raw JSON only when crossing into Python objects.

- Provide decorators for tools and concise output-type selection.

- Preserve rich validation errors while mapping them to kernel retry outcomes.


**Acceptance evidence.**


- Tool argument and structured-output examples work with supported Pydantic types.

- Schemas are compiled/generated once per registration unless explicitly refreshed.

- Validation retry traces match native schema-adapter semantics.

- Pydantic remains an optional Python dependency.


**Dependencies.** PR-030.


**Traceability.** FR-PY structured typing; PRD G-03; TDD section 25.7.


**Explicitly excluded.** No requirement that all Python users adopt Pydantic.


### PR-032 - Complete Python conformance, documentation, and alpha release


**Purpose.** Demonstrate semantic parity and make the package usable by external Python developers.


**Principal changes.**


- Run the full applicable golden trace suite through Python APIs.

- Add pytest fixtures, type-check examples, API reference, migration notes, and benchmark reports.

- Add Python starter projects for Rust-backed and Python-callback configurations.

- Publish or stage signed PyPI alpha artifacts with checksums and SBOM references.


**Acceptance evidence.**


- Rust and Python traces match for the supported feature set.

- Wheel smoke tests pass in clean containers without a compiler.

- Public APIs have complete type hints and examples.

- Release checkpoint `0.0.2a` or equivalent is available.


**Dependencies.** PR-027 through PR-031.


**Traceability.** FR-PY completion; TDD milestone 4.


**Explicitly excluded.** No browser WASM or WIT plugin support.


# 13. Phase 5: Browser and JavaScript WebAssembly bindings


**Outcome.** A browser/JavaScript package that compiles the same kernel, drives external effects through host adapters, keeps state behind handles, and passes shared traces in a worker-capable topology.


**Planning range.** 4-6 weeks; may run in parallel with Phases 4 and 6 after Phase 3


**Traceability.** FR-WASM, NFR-PORT, NFR-PERF; Architecture section 14; TDD section 26.


## Entrance criteria


- Phase 3 binding surface declared.

- Browser support baseline and npm package conventions approved.


## Exit criteria


- Browser WASM runs pass applicable shared traces.

- Model/tool/store effects can be fulfilled by JavaScript promises.

- Event batching and worker helpers prevent UI-thread overload.

- npm artifacts include TypeScript definitions and integrity metadata.


## Pull request sequence


### PR-033 - Create the wasm-bindgen package and split runtime


**Purpose.** Compile the deterministic engine to browser WASM while keeping Tokio-only code out.


**Principal changes.**


- Create `finstack-ai-wasm` with wasm-bindgen exports and a host-driven effect executor.

- Define feature and target boundaries between kernel, native runtime, and WASM runtime.

- Add npm packaging, TypeScript definition generation, browser test harness, and bundle-size reporting.

- Expose build/version metadata and a minimal engine health check.


**Acceptance evidence.**


- The package builds for `wasm32-unknown-unknown` without Tokio, native sockets, or filesystem assumptions.

- A no-op scripted trace runs in a headless browser.

- Bundle contents and generated glue are reproducible.

- Kernel architecture checks still pass.


**Dependencies.** PR-026 and PR-013.


**Traceability.** FR-WASM-001; TDD section 26.1.


**Explicitly excluded.** No model/tool host adapters yet.


### PR-034 - Define JavaScript host adapters for models, tools, stores, and clocks


**Purpose.** Map kernel effects to idiomatic JavaScript promises without duplicating agent semantics.


**Principal changes.**


- Define JS/TS interfaces for model streaming, tool calls, context, middleware where supported, journal persistence, blobs, and timers.

- Implement normalization of JS results into kernel inputs and stable error codes.

- Support AbortSignal propagation and bounded stream adapters.

- Avoid exposing mutable kernel internals or entire state snapshots to host callbacks.


**Acceptance evidence.**


- Scripted JS model and tool adapters pass conformance fixtures.

- AbortSignal cancellation closes streams and pending promises predictably.

- Unknown or malformed host responses are rejected with stable diagnostics.

- No per-token callback is required when a host adapter can provide a readable stream/batch.


**Dependencies.** PR-033.


**Traceability.** FR-WASM-002 and FR-WASM-003; TDD section 26.3.


**Explicitly excluded.** No Node-specific filesystem or network implementation.


### PR-035 - Expose Agent, Run, Result, and event-batch JavaScript handles


**Purpose.** Provide an ergonomic TypeScript API with Rust/WASM-owned state.


**Principal changes.**


- Expose resource-style classes and promise-based run methods.

- Implement async event-batch iteration, explicit snapshots, cancellation, and final results.

- Add zero-copy or bounded-copy byte handling where browser APIs permit.

- Generate stable TypeScript declarations and API examples.


**Acceptance evidence.**


- A model-only and tool-using scripted agent run from TypeScript.

- State remains in WASM memory until explicitly serialized.

- Event batching preserves logical sequence and terminal events.

- Dropped handles release resources without leaving active effects.


**Dependencies.** PR-034.


**Traceability.** FR-WASM binding experience; TDD section 26.2.


**Explicitly excluded.** No worker helper or persistent browser store.


### PR-036 - Add Web Worker helpers, backpressure, and cancellation tests


**Purpose.** Make the recommended browser topology safe under high-volume streaming.


**Principal changes.**


- Add a worker wrapper and message protocol for constructing agents, starting runs, receiving batches, and cancelling.

- Implement transfer/batch thresholds and slow-main-thread policies.

- Add worker restart, detached tab, page unload, and cancelled fetch tests.

- Document single-thread defaults and optional future WASM threading requirements.


**Acceptance evidence.**


- A thousand-chunk stream does not lock the UI thread in the reference demo.

- Worker termination leaves a recoverable or clearly cancelled run state.

- Backpressure is bounded and observable.

- Main-thread and worker modes pass the same semantic traces.


**Dependencies.** PR-035.


**Traceability.** FR-RT-004/005 in browser; Architecture section 14.4.


**Explicitly excluded.** No SharedArrayBuffer/threaded WASM requirement.


### PR-037 - Implement IndexedDB reference storage and browser demo


**Purpose.** Demonstrate durable browser-hosted sessions without putting browser APIs in the kernel.


**Principal changes.**


- Add a JavaScript JournalStore adapter backed by IndexedDB.

- Add bounded blob storage/reference handling for browser media.

- Create a browser demo with model adapter configuration, tool example, streaming UI, reload, and resume.

- Add schema upgrade and corrupted-record diagnostics.


**Acceptance evidence.**


- A completed session survives page reload.

- A deliberately interrupted run restores to a documented resumable or terminal state.

- Store writes are ordered and version-checked.

- The demo uses only public package APIs.


**Dependencies.** PR-034 through PR-036 and journal interfaces available from Phase 2.


**Traceability.** FR-WASM and FR-DUR browser slice; Architecture section 14.5.


**Explicitly excluded.** No multi-device synchronization or service-worker execution.


### PR-038 - Complete browser conformance, benchmarks, and npm alpha


**Purpose.** Prove parity and publish a supported JavaScript/WebAssembly artifact.


**Principal changes.**


- Run applicable golden traces in Chromium, Firefox, and WebKit where practical.

- Add bundle-size, startup, reducer, event-throughput, and host-callback benchmarks.

- Document browser security, CORS, credentials, persistence, worker deployment, and compatibility.

- Publish or stage signed npm alpha artifacts with TypeScript declarations and checksums.


**Acceptance evidence.**


- Rust and browser WASM traces match for the supported feature set.

- Performance reports isolate WASM/JS crossing costs.

- The package installs and runs in a clean TypeScript project.

- Release checkpoint `0.0.2-wasm-alpha` or equivalent is available.


**Dependencies.** PR-033 through PR-037.


**Traceability.** FR-WASM completion; TDD milestone 5.


**Explicitly excluded.** No WIT/Wasmtime plugin host.


# 14. Phase 6: Durability, recovery, and lanes


**Outcome.** Persistent journals, snapshots, effect reconciliation, approvals, durable cancellation/retries, conversation trees, and multi-lane sessions with crash-prefix proof.


**Planning range.** 6-8 weeks; store work can begin after Phase 2 and API integration completes after Phase 3


**Traceability.** FR-DUR; PRD G-06 and UC-05/UC-08; Architecture sections 9-10; TDD sections 18, 23-24, 28.


## Entrance criteria


- Phase 2 commit loop stable.

- Journal schema candidate approved.

- Binding teams consume state only through public handles.


## Exit criteria


- SQLite persistence and recovery pass the crash matrix.

- Pending effects reconcile deterministically.

- Approval suspension/resume works through Rust and Python.

- Initial multi-lane session semantics are implemented and documented.


## Pull request sequence


### PR-039 - Finalize JournalStore v1 and canonical record encoding


**Purpose.** Turn the provisional store contract into a versioned persistence boundary.


**Principal changes.**


- Finalize append, load, snapshot, compare-and-append, scan, and metadata operations.

- Choose and implement the canonical journal encoding with strict limits and diagnostic JSON projection.

- Add checksums/state versions and corruption classification.

- Publish compatibility fixtures for every record variant.


**Acceptance evidence.**


- Encoding is deterministic across supported Rust targets.

- Unknown versions and malformed lengths fail before large allocation.

- Store implementations do not redefine record semantics.

- The compatibility test suite runs against historical fixtures.


**Dependencies.** PR-014 and ADR decision from PR-004.


**Traceability.** FR-DUR-001/002; TDD sections 18 and 28.


**Explicitly excluded.** No specific database.


### PR-040 - Implement the SQLite JournalStore


**Purpose.** Provide the default durable local store with atomic append and efficient restore.


**Principal changes.**


- Add SQLite schema, migrations, WAL settings, transaction boundaries, indexes, and connection ownership.

- Implement atomic record batches, optimistic state-version checks, snapshots, metadata, and pruning policy hooks.

- Add busy handling, disk-full, corrupt database, abrupt process exit, and concurrent reader tests.

- Keep the SQLite crate entirely outside the kernel.


**Acceptance evidence.**


- Committed batches are all-or-nothing under injected failures.

- A second writer receives a deterministic conflict/busy result.

- Restore time and storage growth are benchmarked.

- Database migrations are reversible or explicitly one-way with backup guidance.


**Dependencies.** PR-039.


**Traceability.** FR-DUR store requirements; TDD section 18.2.


**Explicitly excluded.** No PostgreSQL or distributed locking.


### PR-041 - Implement snapshots and replay acceleration


**Purpose.** Reduce restore cost without making snapshots authoritative over the journal.


**Principal changes.**


- Define snapshot schema, state hash, journal position, validity rules, and rebuild fallback.

- Create snapshot scheduling outside the kernel decision path.

- Add replay from snapshot plus tail records and automatic invalid snapshot discard.

- Benchmark full replay versus snapshot restore over large sessions.


**Acceptance evidence.**


- Deleting all snapshots leaves a complete recoverable journal.

- A corrupt or mismatched snapshot is ignored safely.

- Snapshot and full replay produce identical state hashes.

- Snapshot creation cannot block active runs beyond configured bounds.


**Dependencies.** PR-039 and PR-040.


**Traceability.** FR-DUR snapshots; Architecture section 10.5; TDD section 23.


**Explicitly excluded.** No compaction that deletes required journal history.


### PR-042 - Implement model-effect reconciliation and suspended model turns


**Purpose.** Define recovery behavior when a process stops around a model request.


**Principal changes.**


- Classify unstarted, in-flight, completed-but-uncommitted, deferred, and non-resumable model effects.

- Add provider reconciliation hooks keyed by effect ID when a provider supports background/deferred responses.

- Implement retry or terminal policies when provider reconciliation is unavailable.

- Persist enough normalized request metadata to reproduce or diagnose the decision.


**Acceptance evidence.**


- Crash tests at every model-effect boundary restore to a documented action.

- A completed committed model response is never requested again.

- Retry uses the original effect/idempotency metadata.

- Unsupported reconciliation fails explicitly rather than pretending exactly-once behavior.


**Dependencies.** PR-039 through PR-041 and Model port.


**Traceability.** FR-DUR recovery; TDD section 23.2.


**Explicitly excluded.** No universal provider stream resumption guarantee.


### PR-043 - Implement tool-effect reconciliation and idempotency policies


**Purpose.** Recover tool batches without duplicating effects silently.


**Principal changes.**


- Classify tool effects by declared idempotency/reconciliation capability.

- Add tool reconciliation hooks, synthetic cancellation/failure results, and manual-resolution state.

- Persist source-order batch metadata and completed subset state.

- Expose effect IDs to tools for application-level idempotency keys.


**Acceptance evidence.**


- Crash tests cover each call before start, during execution, after result, and before result commit.

- Idempotent tools can retry safely with the same effect ID.

- Non-idempotent unknown outcomes suspend for explicit resolution unless policy says otherwise.

- Restored history contains a valid result for every accepted call.


**Dependencies.** PR-039 through PR-041 and Toolset port.


**Traceability.** FR-DUR and FR-TLS idempotency; Architecture section 10.4.


**Explicitly excluded.** No claim of exactly-once external side effects.


### PR-044 - Implement human approval suspension and resume


**Purpose.** Make approval a durable first-class effect rather than a transient UI callback.


**Principal changes.**


- Add approval request, policy context, decision, expiry, delegation, and resolution records.

- Allow middleware/tool policy to suspend before a tool effect begins.

- Expose list/resolve APIs through Rust and Python handles.

- Add approval timeout and cancelled-while-waiting behavior.


**Acceptance evidence.**


- A process can stop after requesting approval and resume after a later decision.

- A denied or expired approval never dispatches the protected tool effect.

- Duplicate decisions are idempotent and audited.

- Approval traces are binding-independent.


**Dependencies.** PR-039 through PR-043 and middleware port.


**Traceability.** PRD UC-05; FR-DUR approval requirements.


**Explicitly excluded.** No end-user approval UI beyond reference CLI/API examples.


### PR-045 - Implement durable cancellation, retries, and timers


**Purpose.** Ensure long waits and interrupted retries survive process restart.


**Principal changes.**


- Persist cancellation intent and reconciliation progress.

- Persist retry schedules, attempt counters, and timer effects.

- Add runtime adapters for native timers and external durable clocks.

- Define restore behavior for overdue timers and cancelled suspended effects.


**Acceptance evidence.**


- A retry scheduled before shutdown fires once after restart according to policy.

- Cancellation during a suspended approval/model/tool state closes predictably.

- Overdue timer replay is deterministic and bounded.

- Timer IDs and schedule records pass compatibility fixtures.


**Dependencies.** PR-039 through PR-044.


**Traceability.** FR-DUR; FR-RT-007; TDD sections 22-23.


**Explicitly excluded.** No general workflow scheduler or cron product.


### PR-046 - Implement the immutable conversation tree and main lane


**Purpose.** Introduce durable session structure without immediately adding broad multi-agent orchestration.


**Principal changes.**


- Store immutable conversation entries with parent links and shared monotonic sequence numbers.

- Create the mandatory `main` lane with a durable leaf and operation ownership.

- Separate conversation entries from operation records and global metadata facts.

- Add branch/navigation foundations and valid-history extraction.


**Acceptance evidence.**


- Entry parent chains never change after commit.

- Deleting operation logs leaves a valid conversation tree.

- The main lane restores its leaf and active/suspended operation correctly.

- History extraction preserves tool-call/result validity.


**Dependencies.** PR-039 through PR-045.


**Traceability.** PRD UC-08 foundation; Architecture section 9; TDD section 24.


**Explicitly excluded.** No parallel lanes yet.


### PR-047 - Add multi-lane session APIs and single-writer concurrency


**Purpose.** Support independent threads and subagent work while retaining one authoritative session writer.


**Principal changes.**


- Add lane create, list, inspect, navigate, run, cancel, suspend, and resume APIs.

- Allow at most one active operation per lane and multiple lanes to execute concurrently.

- Linearize lane mutations while permitting model/tool effects outside the mutation line.

- Expose external identity mapping hooks for channels/threads.


**Acceptance evidence.**


- Two lanes can share a history prefix and diverge without copying entries.

- A busy lane rejects a second operation while sibling lanes continue.

- Interleaved lane records restore deterministically from the shared sequence.

- Concurrency stress tests preserve single-writer invariants.


**Dependencies.** PR-046.


**Traceability.** PRD UC-08; TDD section 24.2.


**Explicitly excluded.** No distributed multi-writer or replicated sessions.


### PR-048 - Complete the crash-prefix matrix, migrations, and durability beta


**Purpose.** Prove that every durable boundary has a recoverable outcome and publish the first durability-ready release.


**Principal changes.**


- Generate crash tests before/after every persistent write and external effect boundary.

- Add journal/store migration tooling, backup/restore commands, corruption reports, and recovery diagnostics.

- Run shared durability traces through Rust, Python, and browser storage where supported.

- Publish operational guidance and benchmark restore/storage behavior.


**Acceptance evidence.**


- Every enumerated crash prefix restores to valid completed, failed, cancelled, suspended, or retryable state.

- Migration fixtures cover every released pre-beta schema.

- Approval and lane scenarios pass restart tests.

- Release checkpoint `0.0.3-beta` and gate G5 - Durable Beta - are approved.


**Dependencies.** PR-039 through PR-047.


**Traceability.** FR-DUR completion; TDD milestone 6.


**Explicitly excluded.** No PostgreSQL, replication, or workflow-engine-specific semantics.


# 15. Phase 7: Isolated WIT/Wasmtime extensions


**Outcome.** A versioned, permissioned Component Model plugin path for independently distributed toolsets and context providers, without imposing Wasmtime on native builds.


**Planning range.** 4-6 weeks; begins after extension-port candidate freeze in Phase 3


**Traceability.** FR-PLG; PRD G-07 and UC-07; Architecture section 15; TDD section 27.


## Entrance criteria


- Phase 3 registrar and port contracts stable.

- Phase 6 record/effect context available.

- WIT versioning ADR approved.


## Exit criteria


- Reference components load and pass conformance.

- Permissions and resource limits are enforced by the host.

- Wasmtime is absent from minimal/native builds unless selected.

- Plugin packaging and compatibility diagnostics are documented.


## Pull request sequence


### PR-049 - Define WIT v1 common types and toolset world


**Purpose.** Create the smallest stable external ABI around coarse toolset calls.


**Principal changes.**


- Define plugin metadata, API version, call context, raw JSON bytes, blob references, tool specs, tool calls, progress batches, results, and errors.

- Define a toolset world with catalog and call operations using component resources where stateful.

- Add generated binding checks and cross-language fixture components.

- Document canonical ABI copying limits and payload-size rules.


**Acceptance evidence.**


- Rust guest and host bindings compile from the checked-in WIT package.

- A reference toolset lists and executes multiple tools.

- Oversized frames/payloads are rejected before allocation.

- The WIT package has explicit compatibility and deprecation rules.


**Dependencies.** PR-021, PR-023, and PR-039.


**Traceability.** FR-PLG toolset surface; TDD sections 27.1-27.3.


**Explicitly excluded.** No model provider world or arbitrary host filesystem access.


### PR-050 - Define context-provider world, manifest, and lifecycle


**Purpose.** Add the second high-value plugin surface plus package metadata and lifecycle rules.


**Principal changes.**


- Define context query, budget, contribution, attribution, and error types.

- Define plugin manifest fields for identity, versions, capabilities, permissions, configuration schema, digest, and signature metadata.

- Define initialize, health, shutdown, and optional warmup lifecycle.

- Map WIT plugins to native Registrar entries through adapters.


**Acceptance evidence.**


- A context component contributes bounded context through the normal pipeline.

- Manifest validation rejects duplicate identities, incompatible interfaces, and undeclared capability worlds.

- Lifecycle timeouts and failures have stable host diagnostics.

- Native agent resolution treats plugin adapters like ordinary port implementations.


**Dependencies.** PR-049.


**Traceability.** FR-PLG; Architecture sections 15.1 and 15.5.


**Explicitly excluded.** No general middleware world until replay and security semantics are proven.


### PR-051 - Implement the optional Wasmtime component host and cache


**Purpose.** Load typed components while keeping engine and compilation cost outside the core distribution.


**Principal changes.**


- Create a leaf plugin-host crate with direct Wasmtime Component Model integration.

- Implement engine configuration, component compilation/cache, instance/store ownership, async calls, and cancellation.

- Adapt toolset/context worlds to native traits.

- Add lazy startup and explicit plugin-host feature/bundle selection.


**Acceptance evidence.**


- Minimal and standard native bundles remain Wasmtime-free unless plugin support is enabled.

- Compiled component cache invalidates on digest, engine, target, or ABI change.

- A plugin trap is contained and converted to a stable plugin error.

- Concurrent plugin calls respect configured instance/store policies.


**Dependencies.** PR-049 and PR-050.


**Traceability.** FR-PLG execution; TDD sections 27.5-27.6.


**Explicitly excluded.** No registry download or marketplace.


### PR-052 - Enforce permissions, resource limits, and signatures


**Purpose.** Make the isolated path meaningfully safer than in-process host-language extensions.


**Principal changes.**


- Implement deny-by-default WASI context, no ambient preopens/network, fuel/epoch limits, memory/table/instance limits, and call deadlines.

- Add permission grants for narrowly scoped host capabilities and per-plugin configuration.

- Implement manifest digest/signature verification and trusted publisher policy modes.

- Record granted permissions and plugin identity in run/effect diagnostics.


**Acceptance evidence.**


- A component cannot access filesystem/network without an explicitly linked capability.

- Resource exhaustion terminates the call without destabilizing the host.

- Strict signature mode rejects unsigned/untrusted packages.

- Permission grants are visible in audit events without exposing secrets.


**Dependencies.** PR-051.


**Traceability.** FR-PLG security; Architecture sections 15.3 and 17.


**Explicitly excluded.** No claim that WASM alone provides complete application-level safety.


### PR-053 - Publish guest SDKs and reference components


**Purpose.** Make the ABI practical for external extension authors.


**Principal changes.**


- Create a Rust guest SDK with generated bindings, ergonomic adapters, schema helpers, logging, and test fixtures.

- Publish reference calculator, filesystem-like sandbox fixture, and context provider components.

- Add component build templates, pinned toolchain instructions, and local host test commands.

- Document version negotiation and migration examples.


**Acceptance evidence.**


- A new component project can build and run from the template.

- Reference components pass native trait and WIT conformance equivalents.

- Generated code versions are pinned and reproducible.

- Examples avoid direct dependence on host-private crates.


**Dependencies.** PR-049 through PR-052.


**Traceability.** FR-PLG authoring; TDD milestone 7.


**Explicitly excluded.** No Python-to-component automatic packaging.


### PR-054 - Complete plugin discovery, conformance, and alpha release


**Purpose.** Provide deterministic local installation/discovery and validate the plugin ecosystem boundary.


**Principal changes.**


- Add lockfile-driven local plugin discovery and explicit enablement.

- Add conformance tests for lifecycle, malformed manifests, permission denial, traps, cancellation, large payloads, and ABI mismatch.

- Separate package installation tooling from runtime loading.

- Publish plugin-host benchmarks and compatibility fixtures.


**Acceptance evidence.**


- Runtime startup consumes a resolved lockfile and performs no registry search/download.

- Ambiguous duplicate packages fail closed.

- Reference plugins pass on supported desktop targets.

- Release checkpoint `0.0.4-plugin-alpha` and gate G6 - Plugin Alpha - are approved.


**Dependencies.** PR-049 through PR-053.


**Traceability.** FR-PLG completion; Architecture ADR-010/011.


**Explicitly excluded.** No public marketplace, automatic updates, or native dynamic-library ABI.


# 16. Phase 8: Ecosystem readiness and public preview


**Outcome.** A credible first-party battery set, remote serving path, workflow adapters, polished documentation, security/release artifacts, and a supported v0.1 public preview.


**Planning range.** 4-6 weeks after core binding/durability/plugin gates; component work may proceed earlier in parallel


**Traceability.** PRD phases D-F, release criteria, success metrics; TDD milestone 8.


## Entrance criteria


- Native preview, Python alpha, WASM alpha, durability beta, and plugin alpha gates passed.

- Public API change backlog triaged.


## Exit criteria


- Core use cases have first-party examples and batteries.

- Remote protocol and reference server are usable.

- Security, benchmark, SBOM, and compatibility reports are published.

- v0.1 public preview acceptance criteria are met.


## Pull request sequence


### PR-055 - Add Anthropic and Ollama/native-local provider packages


**Purpose.** Demonstrate provider diversity across a native cloud API and an OpenAI-compatible/local deployment path.


**Principal changes.**


- Implement Anthropic message/tool/thinking/stream semantics through the Model port.

- Add a supported Ollama/local provider configuration using the compatible adapter or a dedicated leaf implementation where needed.

- Add provider capability negotiation, model metadata refresh hooks, and fixture/live tests.

- Publish provider authoring guidance based on three implementations.


**Acceptance evidence.**


- Text, tool, usage, structured-output, cancellation, and error fixtures pass for each provider.

- Provider-specific reasoning/tool extension fields round-trip safely.

- Users can install only the provider crates they need.

- Cross-provider model traces normalize to the same kernel semantics.


**Dependencies.** PR-024 and stable Model port.


**Traceability.** FR-MDL; ecosystem readiness.


**Explicitly excluded.** No exhaustive provider catalog or central router product.


### PR-056 - Harden filesystem, shell, repository-context, and compaction batteries


**Purpose.** Provide the minimum high-quality components needed for coding and research agent products.


**Principal changes.**


- Harden filesystem operations and add a shell toolset with allow/deny policy, timeouts, output bounds, and optional external sandbox adapter.

- Add repository instruction/context discovery as a ContextProvider.

- Add sliding-window and summarizing compaction middleware with explicit cache-impact diagnostics.

- Add large-tool-output truncation/spill behavior.


**Acceptance evidence.**


- Security tests cover traversal, environment leakage, command policy, timeouts, and output floods.

- Context and compaction changes are attributed and replay-safe.

- Batteries remain separate leaf packages.

- Coding-agent example uses only public components.


**Dependencies.** PR-025, ContextProvider/Middleware ports, and durability where required.


**Traceability.** PRD UC-02 and context requirements.


**Explicitly excluded.** No full coding-agent TUI or browser automation.


### PR-057 - Add observer adapters and production diagnostics


**Purpose.** Make runs inspectable without coupling the kernel to a telemetry vendor.


**Principal changes.**


- Implement structured logging, OpenTelemetry, and Prometheus/reference metrics adapters as separate packages.

- Define redaction policy, correlation fields, span/event mapping, and bounded exporter queues.

- Add runtime status, queue depth, effect latency, store latency, usage, and recovery metrics.

- Publish trace examples and observer conformance tests.


**Acceptance evidence.**


- Observer failure does not change agent semantics.

- Secrets and protected tool payloads are redacted by default.

- Exporter backpressure is bounded and diagnosed.

- The minimal bundle contains no telemetry exporter.


**Dependencies.** PR-018 and stable event envelopes.


**Traceability.** FR-OBS; Architecture section 21.


**Explicitly excluded.** No hosted telemetry service.


### PR-058 - Implement the remote protocol and reference session server


**Purpose.** Provide a clean serving boundary for UIs, gateways, and distributed applications.


**Principal changes.**


- Define a transport-neutral client/session protocol distinct from the plugin ABI.

- Implement framed messages, hello/version negotiation, correlated commands/results, authoritative snapshots, and transient events.

- Add a reference Unix-socket/TCP-local server with authentication hooks and single-writer session routing.

- Add Rust and TypeScript client helpers plus protocol conformance tests.


**Acceptance evidence.**


- Reconnect obtains a fresh authoritative snapshot before live events.

- Unknown protocol versions fail before session acquisition.

- Authentication completes before protocol bytes are trusted.

- Protocol adapters do not expose store-private or kernel-private structures.


**Dependencies.** PR-039, PR-046/047, and stable public events.


**Traceability.** PRD UC-06; Architecture ADR-014; TDD remote protocol open decision.


**Explicitly excluded.** No public cloud control plane, gateway dashboard, or multi-region routing.


### PR-059 - Add external workflow and durable-runtime adapters


**Purpose.** Let Temporal, Restate, DBOS, or similar systems drive the same effects without replacing kernel semantics.


**Principal changes.**


- Define a runtime-driver adapter contract for durable sleep, effect execution, persistence handoff, and run resumption.

- Implement one fully tested reference integration and one minimal second integration or example.

- Map workflow retries/idempotency to kernel effect IDs and document ownership boundaries.

- Add deterministic replay tests using the external system test harness where feasible.


**Acceptance evidence.**


- The integration does not reimplement the model/tool continuation loop.

- Kernel journal/effect IDs remain authoritative for agent semantics.

- External retries cannot silently exceed kernel policy.

- A human wait or long timer survives worker restart in the reference integration.


**Dependencies.** PR-045, PR-048, and runtime driver interfaces.


**Traceability.** PRD UC-09; Architecture section 23.


**Explicitly excluded.** No requirement to support every workflow engine before v0.1.


### PR-060 - Complete documentation, starter repositories, and security/release artifacts


**Purpose.** Make the framework adoptable and independently reviewable.


**Principal changes.**


- Publish concept, Rust, Python, WASM, durability, provider, toolset, plugin, server, and migration guides.

- Create minimal agent, coding agent, Python service, browser worker, durable approval, and WIT plugin starters.

- Generate SBOMs, checksums, provenance, dependency policy reports, threat model, and security response process.

- Run accessibility and documentation-link checks plus fresh-user usability sessions.


**Acceptance evidence.**


- Every public package has a tested quick start.

- Starter repositories pin compatible versions and pass CI.

- Security documentation clearly distinguishes native, Python/JS callback, process, and WASM trust levels.

- Release artifacts are reproducible from tagged source.


**Dependencies.** All prior public surfaces.


**Traceability.** PRD distribution, risks, release criteria; NFR-DOC and NFR-SUPPLY.


**Explicitly excluded.** No commercial support portal or marketplace.


### PR-061 - Cut the v0.1 public preview release


**Purpose.** Consolidate the implementation into the first externally supported preview.


**Principal changes.**


- Resolve all preview-blocking API/schema issues and publish compatibility scope.

- Run full cross-platform, binding, crash, plugin, security, and benchmark release suites.

- Publish crates, wheels, npm/WASM package, WIT source/bindings, protocol fixtures, SBOMs, checksums, and release notes.

- Open the public issue roadmap and establish the deprecation/migration process.


**Acceptance evidence.**


- All PRD public-preview acceptance criteria pass.

- Rust, Python, and WASM common traces are identical for shared features.

- No forbidden kernel dependency or unbounded queue exists.

- Release `0.1.0` and gate G7 - Public Preview - are approved.


**Dependencies.** PR-055 through PR-060 and all previous gates.


**Traceability.** PRD section 15; TDD section 36.


**Explicitly excluded.** No 1.0 compatibility guarantee.


# 17. Phase 9: 1.0 hardening and general availability


**Outcome.** A stable, audited, performance-hardened 1.0 with migration tooling, ecosystem conformance, and explicit long-term compatibility promises.


**Planning range.** 6-10 weeks after v0.1 feedback; planning range only


**Traceability.** PRD long-term success metrics and release criteria; Architecture evolution; all NFR families.


## Entrance criteria


- v0.1 used by external adopters.

- Preview telemetry, issue patterns, API pain points, and migration needs reviewed.


## Exit criteria


- Public contracts frozen under SemVer policy.

- Security and reliability reviews complete.

- Performance budgets enforced.

- 1.0 artifacts and migration guides released.


## Pull request sequence


### PR-062 - Freeze 1.0 public contracts and migration tooling


**Purpose.** Convert preview experience into explicit stability boundaries.


**Principal changes.**


- Review and freeze Rust APIs, AgentSpec, error codes, event/record schemas, Python API, JS API, remote protocol, and WIT v1.

- Add automated compatibility checks, deprecated aliases, migration commands, and fixture converters where promised.

- Document which leaf provider/tool packages may evolve faster than the core.

- Adopt independent versioning only where it reduces ecosystem coupling safely.


**Acceptance evidence.**


- Breaking-change tests detect incompatible public/schema changes.

- Every v0.1 supported project has a documented 1.0 migration path.

- Deprecated APIs carry removal versions.

- Compatibility policy is approved by maintainers.


**Dependencies.** PR-061 and preview feedback.


**Traceability.** NFR-COMPAT; schema governance.


**Explicitly excluded.** No permanent compatibility promise for explicitly experimental packages.


### PR-063 - Harden performance, memory, and startup budgets


**Purpose.** Ensure the architecture delivers measurable benefit rather than only conceptual modularity.


**Principal changes.**


- Optimize reducer allocations, raw JSON handling, stream batching, registry resolution, provider reuse, and store replay.

- Add enforced warning/failure budgets for native, Python fast path, browser WASM, and WIT toolset benchmarks.

- Profile one thousand idle sessions and high-concurrency active sessions.

- Publish representative flamegraphs and tuning guidance.


**Acceptance evidence.**


- No unexplained benchmark regression remains versus v0.1 baselines.

- Python/WASM overhead targets are met for Rust-backed synthetic workloads or documented with approved exceptions.

- Minimal binary and WASM bundle size targets are measured and enforced.

- Memory growth under long streams and repeated runs is bounded.


**Dependencies.** PR-061.


**Traceability.** NFR-PERF and NFR-SCALE; TDD section 33.


**Explicitly excluded.** No optimization that weakens determinism or safety without an ADR.


### PR-064 - Complete reliability, fuzzing, and independent security review


**Purpose.** Raise confidence in untrusted input, recovery, plugin, and binding paths before GA.


**Principal changes.**


- Run extended fuzz campaigns over parsers, records, events, remote frames, WIT inputs, and recovery sequences.

- Complete property/model checking for reducer and lane invariants.

- Commission or conduct an independent security review focused on plugins, filesystem/shell tools, protocols, and secret handling.

- Close or explicitly accept findings with severity and remediation dates.


**Acceptance evidence.**


- No open critical/high finding blocks GA.

- Fuzz corpora and regression cases are checked in where safe.

- Crash/recovery coverage includes every effect category and lane operation.

- Threat model and security advisories are updated.


**Dependencies.** PR-061 and mature feature set.


**Traceability.** NFR-SEC, NFR-REL, NFR-TEST.


**Explicitly excluded.** No guarantee against malicious native in-process extensions.


### PR-065 - Establish ecosystem conformance and release engineering


**Purpose.** Make future releases predictable for first- and third-party implementers.


**Principal changes.**


- Publish provider/toolset/store/plugin conformance suites and compatibility badges/process.

- Finalize multi-package release automation, signing, provenance, rollback, and hotfix procedures.

- Add nightly/canary artifacts and downstream integration tests against starter projects.

- Define support windows and maintenance branches.


**Acceptance evidence.**


- A tagged release can be recreated from source with matching checksums.

- Downstream starter projects test against release candidates automatically.

- Conformance failures identify the violated contract and version.

- Rollback/hotfix rehearsal is completed.


**Dependencies.** PR-062 through PR-064.


**Traceability.** NFR-SUPPLY, NFR-MAINT, ecosystem readiness.


**Explicitly excluded.** No centralized commercial plugin marketplace.


### PR-066 - Ship the 1.0 release candidate and general availability release


**Purpose.** Complete the product build with stable artifacts and long-term operating commitments.


**Principal changes.**


- Run the full release-candidate soak period with external adopters and no unreviewed API changes.

- Publish final crates, wheels, npm/WASM package, WIT packages, server/client packages, fixtures, SBOMs, checksums, and benchmark/security reports.

- Publish 1.0 migration guide, compatibility matrix, support policy, and roadmap.

- Tag and announce GA only after all release gates are signed off.


**Acceptance evidence.**


- All 1.0 acceptance, security, compatibility, and performance gates pass.

- No critical release blocker remains open.

- Documentation and examples reference only released package versions.

- Release `1.0.0` and gate G8 - General Availability - are approved.


**Dependencies.** PR-062 through PR-065.


**Traceability.** All product and technical acceptance criteria.


**Explicitly excluded.** Post-1.0 marketplace, broad channel catalog, and product-specific UIs.


# 18. Cross-phase quality plan

## 18.1 Test layers by phase

- **Kernel unit tests:** exhaustive valid and invalid transitions, record application, limits, cancellation, tool pairing, and structured output.
- **Property tests:** record replay equivalence, event ordering, lane invariants, source-order tool finalization, and serializer round-trips.
- **Fuzz tests:** untrusted records, events, specs, remote frames, WIT payloads, raw JSON, and recovery sequences.
- **Runtime integration tests:** commit faults, task cancellation, backpressure, slow consumers, provider/tool failures, and resource cleanup.
- **Crash-prefix tests:** process stops before and after every persistent write and external-effect boundary.
- **Binding conformance:** the same golden traces through Rust, Python, and browser WASM APIs.
- **Plugin conformance:** lifecycle, permissions, resource limits, traps, oversized payloads, and ABI mismatches.
- **Downstream tests:** starter projects and reference integrations run against release candidates.

## 18.2 Performance program

Performance measurement starts in Phase 0 and becomes release-blocking only after stable baselines exist. Reports must separate:

- deterministic reducer time;
- runtime scheduling and queueing;
- provider/tool adapter overhead;
- storage commit and restore cost;
- Python FFI conversion and callback cost;
- JavaScript/WASM conversion and host-promise cost;
- WIT canonical ABI and Wasmtime overhead; and
- external network/model/tool latency.

Real model latency must never be used to hide framework overhead. Proposed 1.0 budgets are reviewed after Phase 3 measurements rather than hard-coded before evidence exists.

## 18.3 Security program

Security work is continuous:

- dependency and license policy begins in Phase 0;
- filesystem/tool boundaries are tested in Phase 3;
- host-language callback trust is documented in Phases 4-5;
- durable auditability arrives in Phase 6;
- deny-by-default plugin isolation arrives in Phase 7;
- threat model, provenance, SBOM, and response process are public in Phase 8; and
- independent review closes before 1.0.

# 19. Scope cut lines and contingency sequencing

When staffing or schedule is constrained, scope should be cut in the following order while preserving architectural integrity.

## 19.1 Safe cuts before public preview

1. Defer full multi-lane concurrency while retaining lane IDs and the main-lane data model.
2. Ship only one durable workflow integration.
3. Limit WIT v1 to toolsets and context providers.
4. Defer the reference remote server while retaining the protocol schema and in-process APIs.
5. Ship two providers rather than three.
6. Defer shell tools while retaining calculator and constrained filesystem tools.
7. Limit browser persistence to an example adapter rather than a supported package.

## 19.2 Items that must not be cut

- deterministic kernel decisions and record application;
- commit-before-effect ordering;
- stable effect IDs and idempotency metadata;
- bounded queues and tested cancellation;
- direct native handles after one-time resolution;
- shared Rust/Python/WASM trace fixtures for supported behavior;
- explicit trust levels for native, Python/JS, and WASM extensions;
- migration fixtures for every released persistent schema; and
- benchmark separation of framework overhead from external latency.

## 19.3 Fallback release shapes

- **Rust-first preview:** end after Phase 3 if bindings are delayed; do not advertise cross-language parity.
- **Rust/Python preview:** ship Phase 4 before browser WASM if Python demand is materially higher, while keeping WASM kernel tests active.
- **Non-durable alpha:** acceptable only before public preview; public preview must include the effect journal and at least SQLite recovery.
- **No-plugin public preview:** possible if WIT security or tooling is not ready, provided plugin packages are clearly deferred and the native/Python/WASM extension model is complete.

# 20. Risk register for implementation

| Risk | Early warning | Mitigation and decision point |
| --- | --- | --- |
| Kernel grows into a generic service locator | Repeated requests for new ports or provider-specific fields | Enforce six ports; require ADR for new ports; keep channels/gateways outside |
| Reducer becomes a generic workflow graph | Phase logic expressed as arbitrary nodes for ordinary runs | Keep explicit phase functions; place workflows in a separate package |
| Python/WASM APIs force kernel redesign | Bindings require mutable internals or per-token callbacks | Freeze handle/event/effect surface at Phase 3; use coarse adapters |
| Durability arrives too late | Runtime effects lack stable IDs or commit boundaries | Effect IDs in Phase 1; commit loop in Phase 2; SQLite may start in parallel |
| WIT ABI freezes immature extension semantics | Frequent native port changes after WIT work starts | Begin WIT only after Phase 3 candidate freeze; label unsupported worlds experimental |
| Provider work consumes the roadmap | Many provider-specific features requested before semantics stabilize | Implement compatible, Anthropic, and local paths only before preview |
| Benchmark claims become misleading | External model latency dominates reports | Maintain synthetic scripted benchmarks and separate adapter categories |
| Browser support expands into a second runtime | JS reimplements scheduling or history rules | Compile the same kernel; JS only fulfills effects and holds host resources |
| Review throughput becomes the bottleneck | Large stacked PRs and long-lived branches accumulate | Keep logical PRs small, merge test/schema foundations first, maintain main green |
| 1.0 is declared before ecosystem use | APIs freeze without external feedback | Require v0.1 adopter soak and migration feedback before Phase 9 freeze |

# 21. Program completion criteria

The implementation program is complete for 1.0 only when:

1. the kernel has no forbidden dependency and no external I/O;
2. every kernel transition has documented inputs, outputs, records, effects, and invariant tests;
3. recoverable effects commit request records before execution;
4. all runtime and binding queues are bounded;
5. cancellation and crash-prefix tests produce valid restorable states;
6. Rust, Python Rust-backed, and browser WASM pass common trace fixtures for shared behavior;
7. native extensions use direct handles after one-time resolution;
8. Python/JS/WIT callbacks are coarse, cancellable, and benchmarked;
9. public records, events, specs, errors, remote DTOs, and WIT packages have versioned compatibility tests;
10. at least three model paths, useful coding/research batteries, SQLite durability, observers, remote serving, and one workflow integration are documented and tested;
11. security, provenance, SBOM, migration, and release processes have been exercised; and
12. external preview users have validated the migration path to 1.0.

# 22. Traceability summary

| Implementation area | Primary PR range | PRD families | Technical design areas |
| --- | --- | --- | --- |
| Governance and quality infrastructure | PR-001 to PR-005 | NFR-MAINT, NFR-TEST, NFR-SUPPLY | Workspace, CI, testing, benchmarks |
| Semantic kernel | PR-006 to PR-013 | FR-KRN, FR-CAP foundations | IDs, messages, reducer, records, effects |
| Native runtime | PR-014 to PR-020 | FR-RT, FR-MDL, FR-TLS, FR-CTX, FR-MW, FR-OBS | Commit loop and six ports |
| Rust SDK/native MVP | PR-021 to PR-026 | FR-EXT, FR-CAP, FR-SPEC | Registrar, AgentSpec, providers, tools |
| Python | PR-027 to PR-032 | FR-PY | PyO3, callbacks, Pydantic, wheels |
| Browser WASM | PR-033 to PR-038 | FR-WASM | wasm-bindgen, host effects, workers, IndexedDB |
| Durability and lanes | PR-039 to PR-048 | FR-DUR | stores, encoding, recovery, approvals, lanes |
| Isolated plugins | PR-049 to PR-054 | FR-PLG | WIT, Wasmtime, permissions, guest SDK |
| Ecosystem/public preview | PR-055 to PR-061 | Release scope and representative use cases | Providers, batteries, observers, server, workflows |
| 1.0 hardening | PR-062 to PR-066 | All NFR families | Compatibility, performance, security, release |

# 23. First 30 days

The recommended first-month sequence for a new implementation team is:

## Week 1

- Merge PR-001 and PR-002.
- Open PR-003 and PR-004 in parallel.
- Resolve ID representation, journal encoding evaluation criteria, and async trait strategy ADRs.

## Week 2

- Merge PR-003 through PR-005.
- Start PR-006 and PR-007.
- Create the first golden trace fixtures even though the reducer does not yet exist.

## Week 3

- Merge PR-006 and PR-007.
- Implement PR-008 and begin PR-009 behind it.
- Run the kernel on native and browser WASM targets continuously.

## Week 4

- Merge PR-008 and the first model-only slice of PR-009.
- Review state-machine naming and record/effect separation before adding tools.
- Publish an internal demo that drives the reducer manually with scripted model inputs.

The first-month success criterion is not a real provider call. It is a deterministic, replayable model-only run whose records and events are already shaped for the native runtime, Python, WASM, and durability work that follows.
