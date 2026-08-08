---
title: "finstack-ai Implementation Plan"
subtitle: "Build phases, pull request sequence, delivery gates, and release roadmap"
author: "finstack-ai project"
date: "2026-08-08"
---

# finstack-ai Implementation Plan

# Document control

| Field | Value |
| --- | --- |
| Product | finstack-ai |
| Document | Implementation Plan |
| Version | 0.11 |
| Status | Implementation baseline |
| Date | 2026-08-08 |
| Primary audience | Maintainers, implementation team, reviewers, release managers, and AI coding agents |
| Related documents | Engineering Standards v0.5; Product Requirements Document v0.7; Architecture Specification v0.7; Technical Design v0.11; Security and Threat Model v0.4 |

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
- preview and `1.0.0` release checkpoints;
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
| 0.0.3 beta | Phase 6 | SQLite durability, deferred effects, interactions, lineage, lanes | Beta journal/store schemas |
| 0.0.4 plugin alpha | Phase 7 | Permissioned WIT/Wasmtime extensions | Experimental external ABI |
| 0.1.0 public preview | Phase 8 | Ecosystem batteries, server, workflow adapters, docs | Published preview compatibility policy |
| 1.0.0 GA | Phase 9 | Stable contracts, audit, performance and release hardening | SemVer and schema compatibility commitments |

## 2.1 MVP and preview boundary

The **native developer-preview slice** is reached at the end of Phase 3. It includes the deterministic kernel, standard runtime, Rust SDK, one OpenAI-compatible provider, a calculator toolset, a constrained filesystem toolset, streaming, tools, limits, cancellation, structured output, run lineage, generic deferral/interaction semantics, `before_finalize`, in-memory journaling, tests, examples, and native benchmarks. It is not the product MVP defined by PRD section 14.

The **product MVP** is reached at the end of Phase 5/G4, after the Python and browser WASM packages, shared-kernel/cross-binding conformance, binding event batching, binding/callback benchmark categories, and extension-author documentation complete every PRD section 14 criterion. It may remain an alpha and does not include SQLite, public multi-lane APIs, WIT plugins, or the remote server.

The **public preview** is reached at the end of Phase 8. It additionally includes first-class Python and browser WASM packages, SQLite durability and recovery, typed interaction routing with approval as the first profile, externally completed effects, initial multi-lane sessions, the complete model-activated capability catalog experience, isolated WIT plugins, more providers and batteries, a remote protocol/reference server, one durable-workflow integration, production observers, documentation, provenance, and published compatibility scope.

## 2.2 Deferred beyond the 1.0 baseline

These items are outside the accepted 1.0 scope. Adding one requires a later versioned PRD/plan amendment and, where it changes architecture or compatibility, an accepted ADR.

- a public plugin marketplace and automatic update service;
- a broad catalog of chat channels and finished assistant applications;
- native dynamic-library plugins as a stable ABI;
- distributed multi-writer sessions or replicated journal consensus;
- a general workflow graph in the ordinary single-agent hot path;
- per-token Python, JavaScript, process, or WIT middleware callbacks;
- a hosted control plane, dashboard, billing system, or commercial package registry; and
- a claim of exactly-once external side effects.

# 3. Planning assumptions and schedule ranges

The following ranges are elapsed workstream estimates, not engineer-weeks or delivery commitments. They assume experienced Rust engineering, disciplined use of AI coding tools, automated testing, and prompt review of architectural decisions. With Phases 4-7 overlapping only where their entrance criteria permit, the stated dependency graph has an approximate 28-42 week critical path before schedule contingency; calendar forecasts must not undercut that path.

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

- **One senior engineer with substantial AI assistance:** approximately 14-20 months to `1.0.0`, with the native developer preview targeted first and some ecosystem scope likely deferred.
- **Three-person core team:** approximately 8-11 months to `1.0.0`. Role ownership is kernel/runtime, bindings, and durability/ecosystem, with shared review.
- **Five-person team:** approximately 7-10 months to `1.0.0` if API ownership is clear. Additional staffing does not safely compress Phase 1 because the semantic kernel is the main coordination bottleneck.

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
| Durability | Durability/ecosystem lead | JournalStore, SQLite, recovery, deferred effects, interactions, lineage, lanes | Core lead |
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
   |                   |                   | \
   |                   |                   |  +-- after PR-039 + Phase 3 port freeze
   |                   |                   |                   |
   |                   |                   |             Phase 7: plugins
   |                   |                   |                   |
   |-------------------|-------------------|-------------------|
                                       Phase 8: preview
                                               |
                                        Phase 9: 1.0
```

Durability store work may begin after Phase 2. WIT design may begin after the six port traits are candidate-stable, and Phase 7 implementation may overlap the tail of Phase 6 once PR-039 provides the required record/effect context. Phase 8 still waits for the Phase 4-7 gates. Neither workstream should force changes into the Phase 1 kernel without an ADR.

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
| G0 - Foundation ready | Workspace, reviewed Engineering Standards and Threat Model, architecture checks, CI, ADRs, security/trace/benchmark fixtures |
| G1 - Kernel semantics | Golden traces, exhaustive transitions, property/fuzz coverage, native/WASM kernel build |
| G2 - Native runtime | Commit-before-effect fault tests, bounded queues, task/cancellation leak checks |
| G3 - Native developer preview | Rust SDK, real provider, useful tools, examples, native benchmarks |
| G4 - Binding parity alpha | Python and browser WASM pass shared traces for supported features |
| G5 - Durable beta | SQLite, recovery matrix, deferred effects, interactions, lineage, lanes, migration fixtures |
| G6 - Plugin alpha | WIT conformance, permissions, resource limits, reference components |
| G7 - Public preview | Ecosystem batteries, server, workflow adapter, docs, implemented-control threat-model review, and security/release artifacts |
| G8 - 1.0 GA | Compatibility freeze, audit, performance budgets, release rehearsal and soak |

# 8. Phase 0: Foundation and architecture governance

**Outcome.** A buildable monorepo governed by reviewed engineering/security baselines, automated architecture constraints, cross-target CI, versioning rules, and reusable security/trace/benchmark infrastructure.

**Planning range.** 1-2 weeks

**Traceability.** Engineering Standards; Security and Threat Model; PRD NFR-PORT, NFR-SEC, NFR-COMP, NFR-DX, and release criteria; Architecture sections 2, 17, 22; TDD sections 2-4, 31-34.

## Entrance criteria

- Versioned, internally reconciled Engineering Standards, PRD, Architecture Specification, Technical Design, and Security and Threat Model baselines are present. Phase 0 establishes G0; its named passing decision is required before Phase 1 implementation work merges.

## Exit criteria

- Every placeholder crate builds on supported targets.

- Forbidden kernel dependencies fail CI.

- Golden trace and benchmark fixture formats are merged.

- All architecture decisions needed for Phase 1 are recorded.

- Every G0 engineering/security rule has an owner and planned automated, review, or gate evidence.

## Pull request sequence

### PR-001 - Create the workspace and package skeleton

**Purpose.** Establish the permanent repository shape without prematurely implementing agent behavior.

**Principal changes.**

- Create the Cargo workspace with `finstack-ai-kernel`, `finstack-ai-runtime`, public SDK/facade package `finstack-ai` (library `finstack_ai`), and `finstack-ai-protocol`; do not create a competing `finstack-ai-sdk` package.

- Add placeholder binding packages for Python and browser WASM, plus `extensions/` leaf directories for providers, toolsets, stores, and observers; keep isolated WIT/Wasmtime under `plugins/`; add `examples/` and test-fixture directories.

- Add canonical `licenses/LICENSE-MIT` and `licenses/LICENSE-APACHE` files, dual-license metadata (`MIT OR Apache-2.0`), DCO sign-off instructions, `GOVERNANCE.md`, `SECURITY.md`, contribution guide, coding conventions, workspace lints, and a minimal project README. Keep root `mise.toml` as the sole toolchain pin and task entrypoint; do not add `rust-toolchain.toml` or a Cargo `xtask` crate.

- Adopt the Engineering Standards and Security and Threat Model as repository-governed inputs; link their exception, review-trigger, and evidence rules from contribution templates.

- Adopt one lockstep pre-1.0 version for the semantic core crates, Python/JavaScript binding distributions, and bundled first-party leaf crates, plus a single root changelog; any post-1.0 decoupling requires explicit engine-compatibility ranges and shared conformance evidence.

**Acceptance evidence.**

- `cargo metadata` exposes only the approved dependency direction.

- The workspace builds with default features and with the minimal kernel-only target.

- No provider, database, HTTP, UI, PyO3, wasm-bindgen, or Wasmtime dependency is present in the kernel crate.

- A new contributor can run the documented bootstrap commands from a clean checkout: install mise, then `mise install` and `mise run doctor`.

- Contribution and release ownership are explicit before the first external contribution is accepted.

- `SECURITY.md` provides a private reporting path, supported-version placeholder, and named response owner consistent with the Threat Model.

**Dependencies.** None.

**Traceability.** Engineering Standards sections 1, 12-16; Security and Threat Model sections 13-14; Architecture ADR-001, ADR-005, ADR-010; TDD section 2.

**Explicitly excluded.** No public API, state machine, provider, or tool implementation.

### PR-002 - Add architecture and dependency enforcement

**Purpose.** Make the microkernel boundary mechanically enforceable rather than relying on documentation.

**Principal changes.**

- Add a thin mise task (for example `mise run architecture`) that inspects `cargo metadata` and rejects forbidden direct or transitive dependencies. Prefer a direct tool invocation; do not introduce a Cargo `xtask` crate or other second orchestration layer.

- Add source-level guards for concrete provider/tool/store imports in kernel modules and for central provider/tool match registries.

- Enforce the fixed crate direction `kernel <- runtime (including the six port contracts) <- SDK <- bindings/applications`; require leaf providers/tools/observers to implement runtime-owned contracts, allow persistent store leaves to depend on the runtime store contract plus protocol codec, keep protocol parallel/outward with only public kernel DTO dependencies, and reject kernel/runtime/SDK dependencies on protocol for in-process execution.

- Enforce runtime/facade feature separation: runtime has no default driver; the facade depends on runtime with defaults disabled; facade `native-tokio` (facade default) and `wasm-host` pass through to the corresponding runtime driver; contract-only leaves enable neither; browser WASM depends on the facade with defaults disabled and only `wasm-host`; and its graph contains no Tokio/native-I/O dependency.

- Create checks for unbounded async channels and public task-local request context.

- Add a review checklist file used by pull request templates.

- Map executable architecture checks to `ENG-ARCH-*` and `ENG-SEM-*` rules so failures identify the governing standard.

**Acceptance evidence.**

- A fixture branch that adds `tokio`, `reqwest`, `rusqlite`, `pyo3`, or `wasmtime` to the kernel fails CI with a clear diagnostic.

- A fixture provider and toolset can be added without editing kernel code.

- A compile-only fixture proves the runtime invokes each runtime-owned port trait while the SDK and leaf implementation crates depend only in the allowed direction; no SDK/runtime cycle is possible.

- Native `Send + Sync` and browser-local port/future/stream compile fixtures pass with the same normalized method/data contracts, and the resolved WASM dependency graph contains no Tokio.

- Architecture checks run in less than one minute on a normal CI runner.

- Exceptions require an ADR identifier and an explicit allowlist entry.

- A time-bounded waiver fixture proves owner, scope, risk, compensating control, expiry, and removal issue are required.

**Dependencies.** PR-001.

**Traceability.** Engineering Standards sections 3 and 14; Architecture sections 2.1, 2.2, 22; PRD NFR-DX and NFR-COMP.

**Explicitly excluded.** No semantic lint for state-machine correctness; that arrives with the kernel.

### PR-003 - Establish the cross-platform CI and release build matrix

**Purpose.** Create fast feedback for Rust, Python, and WASM work before feature development begins.

**Principal changes.**

- Install and activate mise in CI so jobs use the same pinned tools and task names as local docs (`mise run …`).

- Add formatting, Clippy, unit-test, documentation, minimal-feature, and architecture jobs for Linux, macOS, and Windows.

- Add `wasm32-unknown-unknown` compilation, headless browser smoke-test placeholders, and Python wheel smoke-test placeholders.

- Reserve Python jobs for CPython 3.11-3.14, version-specific 3.14t, manylinux x86_64/aarch64, macOS arm64, and Windows x64; the expensive full wheel matrix may remain scheduled until PR-027.

- Add supply-chain checks, license policy, dependency advisories, and reproducible release profiles.

- Add secret scanning, canary-redaction fixtures, and placeholders/ownership for parser fuzzing and security boundary tests without treating placeholders as passing evidence.

- Configure benchmark artifacts and build metadata retention without making microbenchmarks merge-blocking yet.

**Acceptance evidence.**

- All jobs run on pull requests with path-aware skipping only where safe.

- MSRV, stable, and nightly/fuzz jobs have explicit ownership and cadence.

- A minimal release binary can be produced for Linux, macOS, and Windows.

- CI documents how generated bindings and fixtures are verified.

- Security checks report the Threat Model control or engineering rule they verify and fail on detected credential material in committed examples/fixtures.

**Dependencies.** PR-001 and PR-002.

**Traceability.** Engineering Standards sections 8-10; Security and Threat Model sections 10, 12-13; TDD sections 33-34; PRD NFR-PORT, NFR-SEC, and release criteria.

**Explicitly excluded.** No publication to crates.io, PyPI, or npm.

### PR-004 - Record foundational ADRs and schema governance

**Purpose.** Freeze the decisions that later PRs must not silently reinterpret.

**Principal changes.**

- Convert the Architecture Specification decision summary into versioned ADR files.

- Record ADR-015 through ADR-037, including canonical encoding, future-capability refinements, compaction ownership, closed technical implementation choices, and their evidence/reconsideration gates.

- Define ownership and compatibility rules for public Rust APIs, journal records, runtime events, AgentSpec, remote protocol DTOs, and WIT packages.

- Create schema directories, fixture naming conventions, and a change-classification template.

- Define the pre-1.0 breakage policy and the gates for introducing a seventh extension port or an eighth middleware stage.

- Cross-link every accepted ADR that changes a security boundary to the Threat Model review trigger and affected control IDs.

**Acceptance evidence.**

- ADR-001 through ADR-037 are present and cross-linked to the design documents.

- Every versioned schema family has an owner, compatibility promise, and test location.

- The PR template requires API/schema/performance/security impact statements.

- A schema change without a fixture update fails CI once schemas exist.

- An ADR that changes a listed trust boundary cannot close without updating the Threat Model or documenting why no threat/control changes.

**Dependencies.** PR-001.

**Traceability.** Architecture section 25; TDD section 37.

**Explicitly excluded.** No implementation of canonical encoding, provider adapters, validators, or binding packages; this PR freezes their constraints and owners.

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

- Gate G0 - Foundation Ready - has a named passing decision recorded after the Phase 0 exit evidence is complete.

**Dependencies.** PR-001, PR-003, and PR-004.

**Traceability.** Engineering Standards section 10; TDD sections 32-33; PRD NFR-PERF, NFR-REL, and NFR-COMP.

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

- Implement typed session, lane, run, turn, message, model-request, tool-batch, tool-call, effect, interaction, event, budget-scope, and record identifiers.

- Implement `RawJson` over shared immutable bytes with validated and unchecked constructors separated.

- Implement the bounded top-level-object `Metadata` newtype, preservation/canonicalization rules, and v1 `RawJson`/metadata limits.

- Define stable error codes, categories, retryability metadata, and diagnostic context without source-language exceptions.

- Add deterministic ID injection for tests and a production UUIDv7 implementation behind a small clock/random interface.

- Implement canonical UTC Unix-epoch-millisecond `Timestamp` and non-negative millisecond `Duration` types with checked range/arithmetic and cross-language conversions.

**Acceptance evidence.**

- All IDs serialize canonically and reject type confusion in Rust APIs.

- Raw JSON and metadata cross clone boundaries without copying payload bytes; fixtures enforce exact and one-over source-span and canonical-JCS byte ceilings plus member/key/depth ceilings, and preserve unknown metadata members.

- Native/Python/JavaScript/CBOR/JSON fixtures agree on timestamp/duration range, precision, UTC/RFC-3339 mapping, leap-second policy, and overflow rejection.

- Errors round-trip through JSON fixtures with stable codes.

- The kernel remains free of OS randomness, clocks, and async runtime dependencies.

**Dependencies.** Phase 0.

**Traceability.** FR-KRN-005; TDD sections 5-6 and 30.

**Explicitly excluded.** No message semantics or provider-specific error mapping.

### PR-007 - Implement content blocks, messages, and blob references

**Purpose.** Define the canonical data model shared by models, tools, stores, bindings, and protocols.

**Principal changes.**

- Add v1 content blocks: text, structured JSON, image/audio/file by `BlobRef`, tool call, tool result, and provider-opaque payloads (`OpaqueBlock`).

- Define `MessageRole` values `system`, `developer`, `user`, `assistant`, and `tool` with immutable `Message` values and the TDD §7 role/block matrix.

- Add tool-call and tool-result association fields, provider extension payloads (`OpaqueBlock` / `ProviderIds` / `Metadata`), `ModelRef`, and environment-supplied timestamps.

- Define a `BlobRef` that carries identity, media type, length, optional name, and optional integrity digest without embedding large payloads or metadata.

- Implement pure structural tool-association validation (uniqueness; optional caller-supplied known-call set). Run-state pairing remains PR-010.

**Acceptance evidence.**

- Message fixtures serialize to frozen expected bytes on native builds, and `finstack-ai-kernel` type-checks for `wasm32-unknown-unknown` (A01 evidence is compile-plus-deterministic-serialization; executed WASM fixture runners are not required for PR-007).

- Invalid tool-result associations are rejected.

- Large content is represented by references rather than copied inline.

- Public types have rustdoc examples and schema fixtures.

**Dependencies.** PR-006.

**Traceability.** FR-KRN-001; Architecture sections 9 and 16; TDD section 7.

**Explicitly excluded.** No blob store implementation or model wire adapter. No `Usage` type on messages (deferred to PR-008/PR-011 consumers). No `Reasoning`/`Refusal` content-block variants (deferred until a consumer freezes payload DTOs). No reducer/run-state tool pairing (PR-010).

### PR-008 - Define runtime events, journal records, and effect envelopes

**Purpose.** Separate durable truth, transient progress, and requested external work.

**Principal changes.**

- Create versioned `RunEvent`, `JournalRecord`, `EffectRequest`, and `EffectResult` envelopes.

- Classify each event as durable, transient, diagnostic, or binding-local.

- Define record batches, state version preconditions, effect idempotency keys, and event sequence numbers.

- Freeze SHA-256 domain-separated digest semantics, strict RFC 8785 normalization for `RawJson`, canonical-CBOR record digests, runtime-owned semantic timestamps, and separate optional store commit timestamps.

- Keep durable wall-clock due/deadline values distinct from non-serialized monotonic elapsed time.

- Add `RunRelation`, `EffectDeferred`, generalized interaction request/resolution/expiry/cancellation records, and non-secret external-handle metadata.

- Require every effect/deferral to carry a non-optional versioned output kind/schema digest that survives unchanged through external completion.

- Define `RunAccepted` with durable principal/tenant/authentication-policy evidence, effective deadline/limits, propagation policy, and resolved-agent lock digest.

- Add the TDD per-family compatibility matrix: strict AgentSpec/config and inbound-command rejection, schema-declared/preserved ignorable durable optionals, fatal unknown state-bearing data, exact WIT worlds, and retain-or-ignore diagnostic metadata.

- Freeze the v1 semantic payload ceilings for record envelopes, append batches, strings/bytes, collections, nesting, raw JSON, and metadata; oversized semantic input fails before allocation and is never truncated.

**Acceptance evidence.**

- Durable and transient event classes cannot be confused through public constructors.

- Every recoverable effect request has a stable `EffectId` and normalized input hash.

- Every event carries schema/kind versions and the applicable model-request/tool-batch correlation IDs; every record carries envelope/kind versions and a cross-language payload digest.

- Durable-derived UUIDv7 event IDs are allocated with and persisted on their source record by a versioned ordinal mapping; replay reuses them, while transient event IDs are explicitly non-replay-stable.

- Every `RunAccepted` fixture has valid root/parent/effect lineage, and approval uses the interaction schema rather than dedicated records.

- Restart fixtures reconstruct the initiating security context and prove child principal scopes, deadlines, and budgets can only be attenuated.

- Record ordering is deterministic under an injected transition environment.

- Schema fixture diffs are merge-blocking; constructors and schema fixtures prove exact-limit and one-over-limit logical record-count, collection-item, raw-JSON/metadata-byte, and depth ceilings. Canonical-CBOR envelope/batch byte limits and malicious declared-length enforcement belong to PR-039.

**Dependencies.** PR-006 and PR-007.

**Traceability.** FR-KRN-006, FR-KRN-007; FR-DUR foundations; TDD sections 6.5, 12, and 20.

**Explicitly excluded.** No store or runtime commit loop.

### PR-009 - Implement the model-only run reducer

**Purpose.** Prove the central decide/apply architecture using the smallest useful agent flow.

**Principal changes.**

- Implement the exact TDD `RunPhase` enum and transition table: accepted, before-run, preparing-context, before-model, awaiting-model, after-model, before-tool-batch, awaiting-tools, after-tool-batch, before-finalize, awaiting-interaction, awaiting-external, sleeping, cancelling, suspended, completed, failed, and cancelled.

- Add command-level inputs and the exact TDD `Decision` output: record drafts, post-commit actions/effects, and diagnostics. Durable public events are derived only by `apply` after commit; transient progress is sequenced by the runtime.

- Add `apply` functions that mutate state only from committed records.

- Support text streaming as transient events and final assistant message commitment as durable state.

- Support generic effect deferral/resumption and require `before_finalize` settlement before terminal commit.

**Acceptance evidence.**

- A model-only golden trace produces the same state hash on repeated replay.

- No effect is emitted before the corresponding request record is part of the decision.

- Invalid phase/input combinations return stable errors without panicking.

- Model stream chunk count does not affect the final durable trace.

- A deferred model completion and a `before_finalize` continuation replay identically without feature-specific reducer branches.

**Dependencies.** PR-006 through PR-008.

**Traceability.** FR-KRN-002 through FR-KRN-004; TDD section 11.

**Explicitly excluded.** No tools, middleware, context providers, or retries.

### PR-010 - Add tool-call and tool-batch semantics to the reducer

**Purpose.** Extend the canonical loop while keeping scheduling policy explicit and recoverable.

**Principal changes.**

- Add assistant tool-call messages, validated tool-call state, tool-batch creation, and tool-result application.

- Define sequential barriers, parallel groups, source-order finalization, duplicate call handling, and unknown tool behavior.

- Ensure every accepted tool call receives exactly one durable result or synthetic closure result.

- Add continuation rules for returning tool results to the model and for the explicit policy outcome where a settled batch terminates the run instead of continuing.

- Allow tool effects to defer under the same `EffectDeferred`/external-completion path used by models; unresolved calls remain suspended rather than receiving fabricated completion.

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

- Add model-request, turn, tool-call/concurrency, input/output token, context/output byte, retry, wall-clock/deadline, checked `u64` integer-micro-unit cost, and bounded namespaced extension-counter limits.

- Add cancellation intent, cancellation acknowledgement, reconciliation state, and stable cancellation reasons.

- Represent retry decisions, backoff requests, attempt counters, retry budgets, and terminal classification.

- Implement the TDD terminal-race precedence table; implementation work must not redefine ordering ad hoc.

- Define lineage-based cancellation/deadline/budget propagation and cancellation/expiry behavior for deferred effects and interactions.

- Implement the fixed `RunPropagationPolicy` rules and persist authorization decision references; do not infer propagation from transient application state.

**Acceptance evidence.**

- Every limit is tested at below, exact, and above-boundary values.

- Cost fixtures use integer millionths plus pricing-policy version and cover `0`, `2^53-1`, `2^53`, `u64::MAX`, canonical JSON-string round-trip, and unknown usage policies. Checked aggregate overflow commits terminal `RunFailed` with code `cost_overflow`, category `limit`, and `retryable = false`, without `LimitReached` or a fabricated observed cost; extension counters are registered/bounded, monotonic, replay-stable, and fail on overflow.

- Cancellation is idempotent and replay-safe from every run phase.

- Retry attempts survive record replay and cannot exceed the configured budget.

- Race-order fixture permutations converge on valid documented outcomes.

- Cancelling a root/parent run produces deterministic descendant, interaction, and deferred-effect outcomes under each declared propagation policy.

**Dependencies.** PR-009 and PR-010.

**Traceability.** FR-KRN-008 through FR-KRN-010; TDD section 22.

**Explicitly excluded.** No Tokio cancellation tokens or real timers.

### PR-012 - Add structured output and internal control tools

**Purpose.** Support typed final results and framework-owned control calls without special-case provider code.

**Principal changes.**

- Define JSON Schema draft 2020-12 output references, structured result candidates, normalized validation outcomes, retry feedback, and final result records.

- Define internal tool identities for final-output submission and `finstack.internal.load_capability`, plus durable activation records/reducer semantics.

- Add output end strategies and clear handling of text plus tool calls in one model response.

- Keep schema validation execution outside the kernel while making its result semantics deterministic.

**Acceptance evidence.**

- Structured-output traces cover success, validation retry, exhausted retries, and competing output/tool calls.

- Internal tools are namespaced and cannot collide with application tools.

- The kernel never imports a JSON Schema or Pydantic implementation.

- Shared traces define validator-independent success, failure, and model-visible retry semantics.

- Plain text remains the zero-configuration default.

**Dependencies.** PR-004/ADR-020/ADR-022, PR-010, and PR-011.

**Traceability.** FR-KRN-011, FR-KRN-012, FR-CAP foundations; TDD sections 10 and 17.

**Explicitly excluded.** No compact model-activated catalog UX or concrete validator; `Always`/`Application` mechanics and the reserved `Model` path are sufficient here.

### PR-013 - Harden the kernel and pass the semantic gate

**Purpose.** Turn the initial reducer into a stable foundation before runtime and bindings depend on it.

**Principal changes.**

- Complete exhaustive transition tests, property tests, model-based tests, and fuzz targets for records, events, and message application.

- Add invalid-input, oversized-input, unknown-version, and corrupt-replay fixture suites.

- Add lineage, deferred-completion, typed-interaction, and `before_finalize` property/fuzz fixtures, including idempotent and conflicting duplicate completions.

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

- Add `ExternalCompletionRouter` and `InteractionRouter` command paths whose authenticated commands carry tenant scope plus an explicit session/lane/run/target locator; bind signed opaque callback tokens at ingress, prohibit global target-ID journal scans, and perform schema/authorization validation plus deterministic duplicate handling before kernel input.

- Validate and decode every callback against the outstanding effect's kind/version/schema digest before constructing typed kernel input; absent or mismatched contracts fail closed.

- Add the required security-audit sink path and the non-state-changing durable rejection record for known authorized targets.

- Define bounded/idempotent `SecurityAuditSink` event/receipt semantics with stable IDs, redacted locator/payload digests, deadlines, health reporting, and no raw token/content fields; require it whenever external ingress is enabled.

- Add fault injection before commit, after commit, before dispatch, and after effect completion.

**Acceptance evidence.**

- A recoverable effect is never dispatched when its request batch fails to commit.

- Reapplying committed records reconstructs the same kernel state.

- Store failure faults the affected run predictably and emits diagnostics.

- The no-durability configuration uses the same loop with the in-memory store.

- Identical external completions are idempotent; known-target conflicts fail closed and append a durable rejection record without changing run state, while authentication/scope/unknown-locator failures are captured by the required security-audit sink without revealing target existence.

- Restart tests route completion and interaction commands directly from their locators without an in-memory index or a journal scan, and required-audit failure rejects the command.

- Pure embedded mode can omit the sink only because it exposes no external ingress; enabling remote completion/interaction/server ingress without a healthy sink fails construction/readiness.

**Dependencies.** Phase 1.

**Traceability.** FR-RT-001; FR-DUR-001 foundations; TDD section 13.

**Explicitly excluded.** No network models, tools, or persistent database.

### PR-015 - Implement the Model port and scripted stream driver

**Purpose.** Define the provider-neutral model contract and prove streaming through the runtime.

**Principal changes.**

- Add the object-safe `Model` trait, request type, model capabilities, model metadata, and normalized stream items.

- Define/lock `ModelContextProfile` per resolved provider/model: hard input bytes, context/output token ceilings, reserved output/provider overhead, and exact or conservative estimator identity/version with override precedence that can only tighten ceilings.

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

- Implement the default Rust JSON Schema draft 2020-12 validator with `jsonschema` 0.40, compiled once per registration and usable in native/WASM builds with host-supplied offline reference resolution and no implicit network fetch.

- Implement sequential and parallel execution groups with bounded concurrency and cancellation token children.

- Add a scripted toolset that supports success, progress, timeout, error, panic containment, and duplicate completion fixtures.

- Return normalized results to the kernel in source-order finalization.

**Acceptance evidence.**

- Scheduler concurrency never exceeds configured limits.

- A panicking native tool is isolated to its call and converted to a stable failure.

- Parallel completion order does not change durable history.

- Tool arguments are validated once at the chosen boundary; approval fixtures prove `Required` cannot be weakened, stricter host or middleware policy overrides `Policy` and `NotRequired`, and approval or general metadata attributes grant no authority.

- The default Rust validator and any fixture validator produce identical normalized retry outcomes for the portable schema subset.

**Dependencies.** PR-004/ADR-022, PR-014, and PR-015.

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

- Implement the seven normalized middleware stages with `before_finalize` as the last behavior-changing stage and ordering resolution.

- Add immutable observer event batches and a no-op/reference observer.

- Record non-recomputable middleware outcomes when durability mode requires replay-safe behavior.

- Add `RequestInteraction` outcomes with approval as the first standard profile; post-terminal notifications remain observer-only.

- Define `before_model` `CompactContext` as a normalized middleware outcome with a unique context-compactor descriptor role, late-stage ordering rules, protected-item validation, strategy/configuration/source evidence, token estimates, prompt-cache impact, and an optional versioned derived checkpoint.

- Commit each context/middleware invocation with stable component/version/config, chain fingerprint/index, input digest, recovery class, and deadline before calling it; add reconcile hooks and resume the chain only from committed cursors.

- Add `RequestCompactionModel` as the only model-assisted path: commit/reconcile a related Model subeffect, authorize it for source sensitivity/residency/egress, record budgeted usage, then resume the same middleware identity.

**Acceptance evidence.**

- Adding context, middleware, or observer fixtures requires no kernel changes.

- Middleware cycles or missing requirements fail at agent resolution, not mid-run.

- Observers cannot mutate execution state through the public API.

- Context budget overrun has a deterministic policy and diagnostic.

- `before_finalize` can request bounded continuation/interaction before terminal commit, and no observer can change a committed terminal result.

- A fixture compaction middleware can replace only the model-visible projection, cannot remove protected content or split tool-call/result pairs, and never changes canonical conversation entries.

- Duplicate compaction owners or context-mutating middleware ordered after compaction fail agent resolution with a precise diagnostic.

- Crash-prefix fixtures at every context/middleware component and compaction child-model boundary reuse committed outcomes/cursors, never perform unrecorded provider I/O, and suspend explicit non-repeatable uncertainty.

**Dependencies.** PR-014 and PR-017.

**Traceability.** FR-CTX, FR-MW, FR-OBS; Architecture sections 6 and 11, especially 11.5; TDD sections 16-19, especially 17.6.

**Explicitly excluded.** No semantic memory, first-party compaction strategy/battery, or telemetry exporter.

### PR-019 - Integrate cancellation, deadlines, retries, and timers

**Purpose.** Connect kernel termination semantics to real async tasks and clocks.

**Principal changes.**

- Create a runtime cancellation-token tree for run, model request, tool batch, and individual tool calls.

- Implement deadline propagation and a clock/timer adapter usable by tests and durable runtimes.

- Convert persisted wall deadlines to monotonic waits per process; add forward/backward wall-jump fixtures so adjustments cannot silently extend an active deadline.

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

**Traceability.** FR-RT completion; NFR-PERF and NFR-REL; Engineering Standards section 10.

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

- Support ready handles and typed async construction factories; invoke selected factories once under construction cancellation/deadline, surface failures before run start, and retain only ready handles in `ResolvedAgent`.

- Create immutable `ResolvedAgent` and `ResolvedRunPlan` structures.

- Verify that no registry lookup occurs per token or per tool progress event.

**Acceptance evidence.**

- Duplicate and missing-extension diagnostics identify the source registration.

- Resolution is deterministic independent of hash-map iteration order.

- Resolved handles are direct `Arc` trait objects for native components.

- Factory-backed and ready-handle registrations resolve to the same immutable hot-path shape, and no factory runs after the first run is accepted.

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

- Define the canonical `BundleSpec` finite requirement/conflict model and fixed configuration precedence; consume exact installation locks rather than adding a general package solver.

- Emit/export/import a credential-free `ResolvedAgentLock` with engine, exact component/capability versions, compatibility selections, effective config, middleware-chain, and schema digests.

- Add Rust `RunResult::decode<T: DeserializeOwned>` tied to the committed structured-output schema digest, with stable path-aware decode errors and no rerun/mutation.

- Add non-kernel `BundleSpec`/`BundleResolver` composition, `AgentCatalog`/`AgentInvoker` references for child/delegated runs, optional `BudgetLedger` aggregation, and scoped `ArtifactStore` service handles without putting their policy in the kernel.

- Define `BudgetLedger` reserve/reconcile/charge/release receipts with reservation/effect idempotency keys, request digests, fail-closed unknown outcomes, and journaled ordering before child/effect dispatch.

- Implement the child invocation handshake: allocate/commit one complete same-session, isolated-session, or remote child locator (all UUIDv7 session/lane/run IDs plus non-secret remote route where applicable) per `(parent_run_id, parent_effect_id)` before child acceptance, then idempotently start-or-attach the exact child; never derive/scan by run ID.

- Ship `Always` and `Application` activation modes; retain the reserved `Model` mode and prevalidated compact catalog metadata without requiring either in the minimal path.

- Rebuild and validate an immutable resolved run plan at activation checkpoints; reject duplicate compactors/post-compactor mutation and invalidate checkpoints when chain/config/profile digests change.

**Acceptance evidence.**

- Equivalent builder and JSON spec inputs resolve to the same agent fingerprint.

- Rust and JSON use explicit `CapabilityRef` objects; every reference resolves to a locked bundle/catalog definition.

- Unknown fields, incompatible versions, duplicate capability IDs, and unresolved references fail before run start.

- The simplest model-only agent requires no capability object.

- Capabilities contain no executable function pointers in the serialized form.

- Bundle conflicts and unresolved agent references fail during resolution, and child-run relation policy is explicit before invocation.

- Crash/race fixtures around parent preparation and child acceptance create exactly one complete child locator/run for an equal request across every placement and durably reject a conflicting digest without global lookup.

- A resolved lock round-trips exactly and reconstructs the same fingerprint; missing/incompatible versions, schemas, or allowed config selections fail before a run, and exported locks contain no secrets.

- Typed Rust result decoding passes success/type/path/error fixtures against the same validated bytes and schema digest used by Python/WASM parity tests.

- Bundles that require budget aggregation or artifacts fail construction when the corresponding scoped service is absent; minimal agents require neither service.

- Crash/race fixtures cannot double-reserve/charge or start a child before its required reservation settles; equal ledger retries return the original receipt and conflicting digests fail closed.

- Artifact fixtures prove write-before-journal ordering, scope authorization, content-digest verification, orphan cleanup after failed appends, retention pinning, and integrity failure for a missing/corrupt referenced artifact. They also prove exact `ArtifactMetadata`-to-`ArtifactRef`/`BlobRef` preservation, section 6.5 boundary rejection without truncation, and that attributes cannot grant authority.

**Dependencies.** PR-021.

**Traceability.** FR-CAP and FR-SPEC; TDD sections 6.5, 8, 9, 10, and 29.

**Explicitly excluded.** No remote spec registry, visual configuration UI, or final model-activated catalog rendering/heuristics.

### PR-023 - Publish the scripted test kit and extension conformance helpers

**Purpose.** Make provider, toolset, store, middleware, and binding implementations easy to test correctly.

**Principal changes.**

- Expose scripted model/toolset/context/store fixtures from a dedicated test-support crate.

- Add conformance macros/functions for each extension port.

- Add deterministic clocks, ID sources, effect drivers, trace assertions, and slow/failing component helpers.

- Add helpers and golden scenarios for child-run lineage, deferred external completion, typed interaction schemas, duplicate completion, `before_finalize` continuation, and protected replay-safe compaction projections/checkpoints.

- Provide example tests that third-party crates can copy without depending on private internals.

**Acceptance evidence.**

- A sample out-of-tree provider and toolset pass conformance tests.

- The test kit has no production dependency from the kernel or runtime.

- Conformance failures explain the violated contract.

- Golden traces can be driven from public SDK APIs.

- Compaction conformance proves canonical-history immutability, protected-item retention, tool-pair validity, checkpoint invalidation, hard-budget enforcement, and identical shared projections across bindings.

**Dependencies.** PR-021 and PR-022.

**Traceability.** Engineering Standards section 10; FR-EXT-006; TDD section 32.

**Explicitly excluded.** No certification or marketplace badge.

### PR-024 - Implement the OpenAI-compatible native provider

**Purpose.** Prove the Model port against a real streaming HTTP implementation reusable across many endpoints.

**Principal changes.**

- Implement the Chat Completions baseline, SSE streaming, structured tool calls, usage, errors, timeouts, and cancellation; keep Responses API mapping as an optional extension.

- Add configurable base URL, headers, authentication, model metadata, and capability declarations.

- Normalize provider extension fields without leaking wire DTOs into the kernel.

- Maintain a versioned endpoint quirks table for OpenAI, Azure-style endpoints, vLLM, Ollama, LM Studio, and gateways without claiming every endpoint is identical.

- Add recorded HTTP fixtures and optional live smoke tests.

**Acceptance evidence.**

- Streaming text, tool calls, structured output, retryable errors, and cancellation pass fixtures.

- The provider is entirely omitted from kernel-only and runtime-only builds.

- Secrets are never emitted in diagnostics or traces.

- Warm connection reuse and request overhead are benchmarked.

- The same provider package can run recorded, local keyless compatibility fixtures; the scripted model remains the semantic conformance reference.

**Dependencies.** PR-004/ADR-023, PR-015, PR-021, and PR-022.

**Traceability.** FR-MDL; PRD UC-01 and UC-06.

**Explicitly excluded.** No provider router, OAuth flow, or full model catalog.

### PR-025 - Implement calculator and minimal filesystem toolsets

**Purpose.** Provide useful batteries that demonstrate both pure and resource-scoped tools.

**Principal changes.**

- Add a deterministic calculator toolset for baseline tests and examples.

- Add read, write, edit, list, glob, and content-search filesystem tools scoped to an authorized root.

- Use handle-relative directory operations with no-follow/openat-style protections (or an equivalent platform capability API), authorize the opened handle/object rather than a pre-open canonical path, and enforce protected/denied patterns plus read/search bounds.

- Stage oversized results through the scoped digest-verifying `ArtifactStore`; do not create ad hoc untracked spill files.

**Acceptance evidence.**

- Path traversal, symlink escape, protected files, and oversized output tests pass.

- Rename/symlink-swap race fixtures between authorization and open/read/write cannot escape the root on any supported platform; unsupported safe primitives fail closed.

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

- Publish the composition crate as Cargo package `finstack-ai` / library `finstack_ai`; examples and leaf-crate names use the canonical `finstack-ai-provider-openai-compatible` and `finstack-ai-tools-filesystem` packages.

- Publish native benchmarks, binary-size reports, memory profiles, and architecture diagrams.

- Add crates.io publication dry runs, changelog generation, and documentation-site scaffolding.

- Declare the candidate binding surface and defer incompatible changes behind an ADR.

**Acceptance evidence.**

- A clean user project can add the SDK, provider, and toolset crates and complete a run.

- Native model-only and tool-loop examples pass on Linux, macOS, and Windows.

- Performance regressions have initial warning thresholds.

- Release checkpoint `0.0.1-dev` is published or reproducibly staged.

- Gate G3 - Native Developer Preview - has a named passing decision recorded after the Phase 3 exit evidence is complete.

**Dependencies.** PR-021 through PR-025.

**Traceability.** MVP native slice; TDD milestone 3.

**Explicitly excluded.** No Python, WASM, durability database, or plugin ABI promise.

# 12. Phase 4: First-class Python bindings

**Outcome.** A PyO3 package that uses the same Rust engine, preserves the Rust-backed fast path, supports coarse Python callbacks and Pydantic schemas, and passes shared conformance traces.

**Planning range.** 4-6 weeks; may run in parallel with Phases 5 and 6 after Phase 3

**Traceability.** FR-PY, NFR-PERF, NFR-PORT; Architecture section 13; TDD section 25.

## Entrance criteria

- Phase 3 binding surface declared.

- Python package naming and the CPython 3.11-3.14/3.14t per-version wheel matrix approved under ADR-017/ADR-018.

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

- Establish a single-extension-module composition for curated Rust-backed providers, initially link the available OpenAI-compatible provider, and use lazy Python submodule imports plus a wheel-size budget. PR-055 adds Anthropic/local implementations to the same wheel.

- Set published package metadata to Python `>=3.11`; build per-version wheels for CPython 3.11-3.14 plus version-specific 3.14t on manylinux x86_64/aarch64, macOS arm64, and Windows x64 where supported.

- Expose version/build metadata and a minimal health function.

- Add Python linting, typing, test, import-time, and wheel smoke jobs.

**Acceptance evidence.**

- Wheels install and import on the declared CPython/platform matrix, including a free-threaded concurrency smoke test for 3.14t.

- Import does not initialize a Tokio runtime or open network resources.

- Package versions align with workspace release metadata.

- No Python toolchain dependency leaks into Rust-only builds.

- The release does not depend on classic `abi3`; an `abi3t` experiment cannot replace a per-version wheel until Python 3.15+ and project performance/conformance gates pass.

**Dependencies.** PR-004/ADR-017/ADR-018 and PR-026.

**Traceability.** FR-PY-001 and distribution requirements.

**Explicitly excluded.** No Agent or callback API, separate Rust-backed provider wheels, or stable-ABI launch promise.

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

- Expose coarse child-run handles, interaction request/resolution, and authenticated external-completion APIs without Python reimplementing routing semantics.

- Treat lineage/interaction/external-completion binding shapes as pre-beta against scripted/in-memory semantics; do not claim durable restart support in this PR.

**Acceptance evidence.**

- One Python model and one Python toolset pass port conformance tests.

- Cancellation reaches Python callbacks and late completion is handled safely.

- Callback exceptions become stable framework errors with sanitized tracebacks.

- A Python callback cannot retain a stale run context after settlement.

- Python lineage/interactions/deferred-completion traces match Rust for the Phase 4 in-memory/scripted surface. PR-048 is the blocking durable restart, pruning, and duplicate-completion parity gate.

**Dependencies.** PR-028 and PR-029.

**Traceability.** FR-PY-005; all six port adapters where supported.

**Explicitly excluded.** No arbitrary Python subclass of the kernel or reducer.

### PR-031 - Add Pydantic-compatible tool and structured-output adapters

**Purpose.** Match the strongest Python developer ergonomics without moving orchestration into Python.

**Principal changes.**

- Accept Pydantic models, dataclasses, TypedDicts, and TypeAdapter-compatible types.

- Generate and normalize JSON Schema draft 2020-12 at registration; validate raw JSON with the cached Pydantic adapter only when crossing into Python objects.

- Provide decorators for tools and concise output-type selection.

- Preserve rich validation errors while mapping them to kernel retry outcomes.

**Acceptance evidence.**

- Tool argument and structured-output examples work with supported Pydantic types.

- Schemas are compiled/generated once per registration unless explicitly refreshed.

- Validation retry traces match native schema-adapter semantics.

- Every supported Pydantic shape emits a schema inside the portable provider/native/WASM subset or fails registration with a precise unsupported-keyword diagnostic.

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

- Complete compact capability catalog rendering and activation heuristics in the shared SDK/runtime as an independently reviewable split if needed, then expose all three activation modes idiomatically in Python.

- Publish or stage signed PyPI alpha artifacts with checksums and SBOM references.

**Acceptance evidence.**

- Rust and Python traces match for the supported feature set.

- Wheel smoke tests pass in clean containers without a compiler.

- Public APIs have complete type hints and examples.

- `Always`, `Application`, and `Model` activation pass Python/shared traces without rewriting the stable prompt prefix.

- The Python half of the exact `0.0.2 alpha` checkpoint is release-candidate ready; the checkpoint is cut only after Phase 5 also passes.

**Dependencies.** PR-004/ADR-020 and PR-027 through PR-031.

**Traceability.** FR-PY completion, FR-CAP model-activated UX; TDD sections 10.4 and 25.

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

- Compile the runtime-owned logical port contracts with native `Send + Sync`/sendable futures and wasm-local object/future/stream aliases; select facade `wasm-host` with defaults disabled and verify the pass-through runtime feature.

- Add npm packaging, TypeScript definition generation, browser test harness, and bundle-size reporting.

- Expose build/version metadata and a minimal engine health check.

**Acceptance evidence.**

- The package builds for `wasm32-unknown-unknown` without Tokio, native sockets, or filesystem assumptions.

- Compile fixtures implement every logical port with a non-`Send` JavaScript promise/stream proxy on WASM and with `Send + Sync` handles on native, without changing DTOs or method semantics.

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

- Keep all JavaScript adapter objects on the owning worker/local executor; no unsafe `Send`/`Sync` assertions or cross-worker JS handles are permitted.

- Support AbortSignal propagation and bounded stream adapters.

- Publish a tree-shakeable `@finstack/ai/adapters/openai-compatible` fetch/SSE adapter implementing the model host interface, defaulted to a same-origin proxy URL.

- Avoid exposing mutable kernel internals or entire state snapshots to host callbacks.

- Expose coarse child-run handles, interaction request/resolution, and external-completion adapters through the same normalized runtime commands.

- Mark durability-dependent adapter APIs pre-beta and validate them here only against scripted/in-memory semantics; PR-048 owns cross-binding durable acceptance.

**Acceptance evidence.**

- Scripted JS model and tool adapters pass conformance fixtures.

- AbortSignal cancellation closes streams and pending promises predictably.

- Unknown or malformed host responses are rejected with stable diagnostics.

- No per-token callback is required when a host adapter can provide a readable stream/batch.

- The optional remote adapter passes streaming/cancellation fixtures and its examples contain no browser-embedded provider secret.

- JavaScript lineage/interactions/deferred-completion traces match native semantics without per-progress-event callbacks.

- No Phase 5 acceptance claim implies SQLite/IndexedDB crash durability; those cases become release-blocking in PR-048 after JournalStore v1 freezes.

**Dependencies.** PR-004/ADR-019 and PR-033.

**Traceability.** FR-WASM-002 and FR-WASM-003; TDD section 26.3.

**Explicitly excluded.** No Node-specific filesystem/network implementation and no network dependency in the kernel/WASM core.

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

**Purpose.** Make the default Web Worker browser topology safe under high-volume streaming.

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

**Purpose.** Demonstrate experimental browser persistence without putting browser APIs in the kernel, before JournalStore v1 freezes.

**Principal changes.**

- Add a JavaScript JournalStore adapter backed by IndexedDB.

- Add bounded blob storage/reference handling for browser media.

- Create a browser demo with model adapter configuration, tool example, streaming UI, reload, and resume.

- Add schema upgrade and corrupted-record diagnostics.

- Label the adapter/schema provisional; PR-048 must migrate and re-run it against PR-039 JournalStore v1 before any supported durability claim.

**Acceptance evidence.**

- A completed session survives page reload.

- A deliberately interrupted scripted run restores to a documented provisional state; this is not the durable-beta acceptance matrix.

- Store writes are ordered and version-checked.

- The demo uses only public package APIs.

- Package/readme metadata identifies browser persistence as experimental until PR-048 revalidation.

**Dependencies.** PR-034 through PR-036 and journal interfaces available from Phase 2.

**Traceability.** FR-WASM and FR-DUR browser slice; Architecture section 14.5.

**Explicitly excluded.** No multi-device synchronization or service-worker execution.

### PR-038 - Complete browser conformance, benchmarks, and npm alpha

**Purpose.** Prove parity and publish a supported JavaScript/WebAssembly artifact.

**Principal changes.**

- Run applicable golden traces in Chromium, Firefox, and WebKit where practical.

- Add bundle-size, startup, reducer, event-throughput, and host-callback benchmarks.

- Document browser security, CORS, credentials, persistence, worker deployment, and compatibility.

- Document the same-origin proxy pattern and explicitly reject shipping provider API keys in browser bundles.

- Expose the compact model-activated capability catalog through the same host-independent SDK semantics used by Rust/Python.

- Publish or stage signed npm alpha artifacts with TypeScript declarations and checksums.

**Acceptance evidence.**

- Rust and browser WASM traces match for the supported feature set.

- Performance reports isolate WASM/JS crossing costs.

- The package installs and runs in a clean TypeScript project.

- All three activation modes pass browser/shared traces, including cache-stable context append behavior.

- The exact cross-binding `0.0.2 alpha` checkpoint is available after both Phase 4 and Phase 5 acceptance evidence passes.

- Gate G4 - Binding Parity Alpha - has a named passing decision recorded only after both Phase 4 and Phase 5 exit and logical-PR evidence sets are complete.

**Dependencies.** PR-004/ADR-020, the independently reviewable shared activation-UX slice described in PR-032, and PR-033 through PR-037.

**Traceability.** FR-WASM completion, FR-CAP model-activated UX; TDD sections 10.4 and 26.

**Explicitly excluded.** No WIT/Wasmtime plugin host.

# 14. Phase 6: Durability, recovery, and lanes

**Outcome.** Persistent journals, direct state-CBOR snapshots, generic deferred-effect reconciliation, generalized interactions, durable cancellation/retries, explicit run lineage, conversation trees, and multi-lane sessions with crash-prefix proof.

**Planning range.** 6-8 weeks; store work can begin after Phase 2 and API integration completes after Phase 3

**Traceability.** FR-DUR; PRD G-06 and UC-05/UC-08; Architecture sections 9-10; TDD sections 18, 23-24, 28.

## Entrance criteria

- Phase 2 commit loop stable.

- Journal schema candidate approved.

- Binding teams consume state only through public handles.

## Exit criteria

- SQLite persistence and recovery pass the crash matrix.

- Pending effects reconcile deterministically.

- Generalized interaction suspension/resume works through Rust and Python, with approval as the first supported profile.

- Initial multi-lane session semantics are implemented and documented.

## Pull request sequence

### PR-039 - Finalize JournalStore v1 and canonical record encoding

**Purpose.** Turn the provisional store contract into a versioned persistence boundary.

**Principal changes.**

- Finalize append, load, snapshot, compare-and-append, scan, and metadata operations.

- Define append-batch identity and ambiguous-acknowledgement recovery: retries reuse frozen batch/record IDs, semantic timestamps, payloads, and digests; stores return the original receipt for an exact committed batch before evaluating expected sequence and classify mismatched reuse as corruption.

- Implement ADR-015's deterministic CBOR profile with `ciborium` behind the project codec wrapper: validate and recursively sort map keys using RFC 8949 ordering before library encoding; enforce definite lengths, shortest lossless major-type integer/finite-float encodings, rejection of bignum tags 2/3, preservation of negative zero, duplicate-key/non-finite-float rejection, strict v1 depth/size/item limits, and lossless JSON/JSONL diagnostic projection. Do not rely on the library writer alone for canonicalization.

- Add checksums/state versions and corruption classification.

- Add per-record payload/envelope checksums and a session checksum chain/head covering all replay-semantic fields while excluding diagnostic store commit time.

- Publish compatibility fixtures for every record variant.

- Freeze the v1 lineage, generic `EffectDeferred`, interaction, and `before_finalize` record variants together with the rest of the semantic journal.

**Acceptance evidence.**

- Encoding is deterministic across supported Rust targets.

- Binary fixtures are byte-identical across native targets, and their JSON projection round-trips without semantic loss, including canonical decimal-string `micros` values.

- Nested maps created in different insertion orders encode identically; fixtures cover `0`, `2^53-1`, `2^53`, and `u64::MAX`, bignum-tag/overflow rejection, float-width boundaries, negative zero, and non-finite rejection.

- Exact-limit and one-over-limit canonical envelope byte, checked sum-of-envelope batch byte, batch-record-count, collection-item, and nesting-depth fixtures are enforced during decode; backend/transport overhead is excluded, and unknown versions or malicious declared lengths fail before allocation.

- Reordered, truncated, payload-modified, or wrong-head journal fixtures fail checksum/sequence verification before record application.

- Store implementations do not redefine record semantics.

- SQLite and other persistent stores consume the shared protocol codec rather than duplicating canonical-CBOR rules; runtime/SDK dependency graphs remain protocol-free.

- The compatibility test suite runs against historical fixtures.

- Fault injection after durable commit but before acknowledgement proves retry returns the original receipt exactly once; concurrent new batches still receive deterministic sequence conflicts.

**Dependencies.** PR-014 and ADR-015 from PR-004.

**Traceability.** FR-DUR-001/002; TDD sections 6.5, 18, and 28.

**Explicitly excluded.** No specific database.

### PR-040 - Implement the SQLite JournalStore

**Purpose.** Provide the default durable local store with atomic append and efficient restore.

**Principal changes.**

- Add SQLite schema, migrations, WAL settings, transaction boundaries, indexes, and connection ownership.

- Make WAL + `synchronous=FULL` (plus supported platform full-fsync controls) the acknowledged durable mode; label NORMAL/OFF or non-flush-guaranteeing storage as relaxed/non-durable in config and health output.

- Implement atomic record batches, optimistic state-version checks, snapshots, metadata, and pruning policy hooks.

- Add busy handling, disk-full, corrupt database, abrupt process exit, and concurrent reader tests.

- Keep the SQLite crate entirely outside the kernel.

**Acceptance evidence.**

- Committed batches are all-or-nothing under injected failures.

- Acknowledged durable-mode commits survive the supported power-loss/storage-fault harness assumptions; process-kill and simulated power-loss tests are reported separately, and relaxed mode never advertises NFR-REL-001.

- A second writer receives a deterministic conflict/busy result.

- Restore time and storage growth are benchmarked.

- Database migrations are reversible or explicitly one-way with backup guidance.

**Dependencies.** PR-039.

**Traceability.** FR-DUR store requirements; TDD section 18.2.

**Explicitly excluded.** No PostgreSQL or distributed locking.

### PR-041 - Implement snapshots and replay acceleration

**Purpose.** Reduce restore cost without making snapshots authoritative over the journal.

**Principal changes.**

- Implement ADR-032's direct versioned state-CBOR snapshot envelope with state hash, journal position, validity rules, and rebuild fallback; do not introduce a second handwritten snapshot DTO model.

- Create snapshot scheduling outside the kernel decision path.

- Add replay from snapshot plus tail records and automatic invalid snapshot discard.

- Benchmark full replay versus snapshot restore over large sessions.

**Acceptance evidence.**

- Deleting all snapshots leaves a complete recoverable journal.

- A corrupt or mismatched snapshot is ignored safely.

- Snapshot and full replay produce identical state hashes.

- Snapshot creation cannot block active runs beyond configured bounds.

**Dependencies.** PR-039 and PR-040.

**Traceability.** FR-DUR snapshots; Architecture section 10.5; TDD section 18.3.

**Explicitly excluded.** No compaction that deletes required journal history.

### PR-042 - Implement model-effect reconciliation and suspended model turns

**Purpose.** Define recovery behavior when a process stops around a model request.

**Principal changes.**

- Drive unstarted, in-flight, completed-but-uncommitted, deferred, and non-resumable model states through the shared deferred-effect state machine.

- Add provider reconciliation hooks keyed by the original effect ID and its non-secret external handle when a provider supports background/deferred responses.

- Implement retry or terminal policies when provider reconciliation is unavailable.

- Persist enough normalized request metadata to reproduce or diagnose the decision.

**Acceptance evidence.**

- Crash tests at every model-effect boundary restore to a documented action.

- A completed committed model response is never requested again.

- Retry uses the original effect/idempotency metadata.

- Unsupported reconciliation fails explicitly rather than pretending exactly-once behavior.

- Duplicate external completion for the same effect/result is idempotent; a conflicting completion fails closed and is audited.

**Dependencies.** PR-039 through PR-041 and Model port.

**Traceability.** FR-DUR recovery; TDD section 23.2.

**Explicitly excluded.** No universal provider stream resumption guarantee.

### PR-043 - Implement tool-effect reconciliation and idempotency policies

**Purpose.** Recover tool batches without duplicating effects silently.

**Principal changes.**

- Drive tool waits and unknown outcomes through the same deferred-effect state machine used by model and application effects, classified by declared idempotency/reconciliation capability.

- Add tool reconciliation hooks, synthetic cancellation/failure results, and manual-resolution state.

- Persist source-order batch metadata and completed subset state.

- Expose effect IDs to tools for application-level idempotency keys.

**Acceptance evidence.**

- Crash tests cover each call before start, during execution, after result, and before result commit.

- Idempotent tools can retry safely with the same effect ID.

- Non-idempotent unknown outcomes suspend for explicit resolution unless policy says otherwise.

- Restored history contains a valid result for every accepted call.

- Tool-specific code does not introduce a parallel suspension or completion protocol.

**Dependencies.** PR-039 through PR-041 and Toolset port.

**Traceability.** FR-DUR and FR-TLS idempotency; Architecture section 10.4.

**Explicitly excluded.** No claim of exactly-once external side effects.

### PR-044 - Implement generalized interactions and the approval profile

**Purpose.** Make typed human interaction a durable kernel concept, with approval delivered as the first policy profile rather than as a separate persistence subsystem.

**Principal changes.**

- Add versioned interaction request/resolution records for approval, choice, form, free text, review, correction, and custom extension kinds.

- Define assignee hints, schema/prompt references, expiry, delegation, cancellation, and idempotent resolution semantics.

- Implement approval as an interaction wrapper and allow middleware/tool policy to suspend before a protected effect begins.

- Expose authenticated list/resolve APIs through Rust and Python handles using the shared interaction router.

- Add approval timeout and cancelled-while-waiting behavior.

**Acceptance evidence.**

- A process can stop after requesting any supported interaction and resume after a later valid resolution.

- A denied or expired approval never dispatches the protected tool effect.

- Duplicate equivalent resolutions are idempotent; conflicting resolutions fail closed and are audited.

- Approval, choice, and review traces are binding-independent and use the same persisted interaction envelope.

**Dependencies.** PR-039 through PR-043 and middleware port.

**Traceability.** PRD UC-05; FR-KRN-015; FR-DUR interaction requirements.

**Explicitly excluded.** No end-user interaction UI beyond reference CLI/API examples.

### PR-045 - Implement durable cancellation, retries, and timers

**Purpose.** Ensure long waits and interrupted retries survive process restart.

**Principal changes.**

- Persist cancellation intent and reconciliation progress.

- Persist retry schedules, attempt counters, and timer effects.

- Add runtime adapters for native timers and external durable clocks.

- Define restore behavior for overdue timers and cancelled deferred effects or interactions.

- Recompute monotonic waits from persisted wall due-times on restart, fire overdue timers through one idempotent durable timer completion, and diagnose/clamp backward-clock anomalies.

**Acceptance evidence.**

- A retry scheduled before shutdown fires once after restart according to policy.

- Cancellation during a suspended interaction or deferred model/tool/application effect closes predictably.

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

- Persist and expose run relations for the root operation and any child/delegated runs without treating lanes as the lineage model.

- Persist/recover the prepared parent-effect to child-UUIDv7 mapping and child security/propagation context across same-session lanes and child sessions.

**Acceptance evidence.**

- Entry parent chains never change after commit.

- Deleting only non-authoritative derived projections leaves a valid conversation tree; operation/journal records required for recovery, audit, settlement idempotency, or retention policy are never treated as disposable logs.

- The main lane restores its leaf and active/suspended operation correctly.

- History extraction preserves tool-call/result validity.

- Every restored operation has an unambiguous root/parent relation, invocation effect, kind, depth, and budget scope where applicable.

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

- Propagate lineage-aware cancellation, deadlines, depth, and budget policy across child runs while retaining independent run records.

**Acceptance evidence.**

- Two lanes can share a history prefix and diverge without copying entries.

- A busy lane rejects a second operation while sibling lanes continue.

- Interleaved lane records restore deterministically from the shared sequence.

- Concurrency stress tests preserve single-writer invariants.

- Child-run and lane relationships remain distinct and replay to the same result under interleaving.

**Dependencies.** PR-040, PR-046, and ADR-016. SQLite conflict/recovery evidence is a hard prerequisite, even if in-memory lane tests were prototyped earlier.

**Traceability.** PRD UC-08; TDD section 24.2.

**Explicitly excluded.** No distributed multi-writer or replicated sessions.

### PR-048 - Complete the crash-prefix matrix, migrations, and durability beta

**Purpose.** Prove that every durable boundary has a recoverable outcome and publish the first durability-ready release.

**Principal changes.**

- Generate crash tests before/after every persistent write, deferred-effect transition, interaction transition, compaction middleware/summary/checkpoint transition, `before_finalize` decision, and external-effect boundary.

- Add journal/store migration tooling, backup/restore commands, corruption reports, and recovery diagnostics.

- Persist/rebuild the external-command settlement identity/digest index in snapshots and retain tombstones through the callback-token/idempotency horizon across pruning and migration.

- Run shared durability traces through Rust, Python, and browser storage where supported.

- Add restart/crash-prefix fixtures for application/model capability activation and prove activation is applied only at safe checkpoints.

- Publish operational guidance and benchmark restore/storage behavior.

**Acceptance evidence.**

- Every enumerated crash prefix restores to valid completed, failed, cancelled, suspended, or retryable state.

- Migration fixtures cover every released pre-beta schema.

- Duplicate completion/resolution classification is unchanged after restart, snapshot restore, migration, and permitted journal pruning; outstanding tokens are never pruned and post-horizon tokens are explicitly expired.

- Lineage, interaction, generic deferral, duplicate external completion, `before_finalize`, and lane scenarios pass restart tests.

- Child-invocation crash prefixes before/after preparation and acceptance always attach to one mapped UUIDv7 child; principal/deadline/budget attenuation reconstructs without process memory.

- Budget reservation/charge/release crash prefixes reconcile by stable IDs and never double-allocate, double-charge, or silently grant on unknown ledger state.

- Model-activated catalog state survives restart without duplicating instructions or rewriting a stable prompt prefix.

- Recorded compaction outcomes replay without rerunning summarization, while stale/missing checkpoints rebuild from unchanged canonical history.

- Required compaction projections stored out of line survive restart through a verified durable `ArtifactRef`; missing/corrupt required content faults recovery, while only explicitly disposable checkpoints may rebuild.

- Python and browser adapter surfaces from PR-030/PR-034/PR-037 pass the same durable restart, migration, settlement-idempotency, and interaction/deferred-effect fixtures where the target store supports them; provisional labels remain where a target cannot meet the gate.

- Release checkpoint `0.0.3` (beta) and gate G5 - Durable Beta - have passed.

**Dependencies.** PR-032/PR-038 for the binding-alpha activation UX, and PR-039 through PR-047 for durability. Binding-specific durability cases may merge conditionally, but the 0.0.3 beta gate requires the complete matrix.

**Traceability.** FR-DUR completion; TDD milestone 6.

**Explicitly excluded.** No PostgreSQL, replication, or workflow-engine-specific semantics.

# 15. Phase 7: Isolated WIT/Wasmtime extensions

**Outcome.** A versioned, permissioned Component Model plugin path for independently distributed toolsets and context providers, without imposing Wasmtime on native builds.

**Planning range.** 4-6 weeks; begins after extension-port candidate freeze in Phase 3

**Traceability.** FR-PLG; PRD G-07 and UC-07; Architecture section 15; TDD section 27.

## Entrance criteria

- Phase 3 registrar and port contracts stable.

- Phase 6 record/effect context available.

- ADR-035's experimental 0.x-to-1.0 WIT versioning and coarse-completion policy accepted in the baseline.

## Exit criteria

- Reference components load and pass conformance.

- Permissions and resource limits are enforced by the host.

- Wasmtime is absent from minimal/native builds unless selected.

- Plugin packaging and compatibility diagnostics are documented.

## Pull request sequence

### PR-049 - Define the initial WIT common types and toolset world

**Purpose.** Create the smallest experimental pre-1.0 external ABI around coarse toolset calls.

**Principal changes.**

- Define plugin metadata, 0.x API version, sanitized principal/tenant/scope/budget call context, raw JSON bytes, blob references, parity-complete tool specs, tool calls, one final result, and errors.

- Define a toolset world with catalog and call operations using component resources where stateful.

- Define the versioned `finstack:ai-host` logging/blob interfaces and explicit `finstack:ai-types` imports used by guest packages; linking them grants no ambient WASI authority.

- Add generated binding checks and cross-language fixture components.

- Document canonical ABI copying limits and payload-size rules.

- Keep the initial WIT world at ADR-035's coarse completion boundary: a component invocation returns one final result/error and exposes no progress/resumable kernel effect stream.

**Acceptance evidence.**

- Rust guest and host bindings compile from the checked-in WIT package.

- A reference toolset lists and executes multiple tools.

- Catalog digest, execution mode, approval policy, result ceiling, side-effect/retry metadata, and sanitized authorization context map to native semantics; omitted/invalid security metadata fails registration rather than receiving permissive defaults.

- Oversized frames/payloads are rejected before allocation.

- The WIT package has explicit compatibility and deprecation rules.

- Plugin alpha publishes @0.x packages only; @1.0.0 generation is blocked until the framework `1.0.0` gate.

- No initial WIT component world can invoke a full nested agent or persist its own competing run-lineage semantics.

**Dependencies.** PR-021, PR-023, and PR-039.

**Traceability.** FR-PLG toolset surface; TDD sections 27.1-27.4.

**Explicitly excluded.** No model provider world or arbitrary host filesystem access.

### PR-050 - Define context-provider world, manifest, and lifecycle

**Purpose.** Add the second high-value plugin surface plus package metadata and lifecycle rules.

**Principal changes.**

- Define context query, budget, contribution, attribution, and error types.

- Define plugin manifest fields for identity, versions, capabilities, permissions, configuration schema, digest, and signature metadata.

- Define initialize, health, shutdown, and optional warmup lifecycle.

- Map WIT plugins to native Registrar entries through adapters.

- Keep context-provider calls coarse and bounded; they cannot suspend a run with a plugin-private interaction or deferral protocol.

**Acceptance evidence.**

- A context component contributes bounded context through the normal pipeline.

- Manifest validation rejects duplicate identities, incompatible interfaces, and undeclared capability worlds.

- Lifecycle timeouts and failures have stable host diagnostics.

- Native agent resolution treats plugin adapters like ordinary port implementations.

- WIT lifecycle and context calls cannot invoke a full nested agent; host-mediated child runs remain an SDK/runtime concern.

**Dependencies.** PR-049.

**Traceability.** FR-PLG; Architecture sections 15.1 and 15.5; TDD section 27.5.

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

**Traceability.** FR-PLG execution; TDD sections 27.6-27.7.

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

- Release checkpoint `0.0.4` (plugin alpha) and gate G6 - Plugin Alpha - have passed.

**Dependencies.** PR-049 through PR-053.

**Traceability.** FR-PLG completion; Architecture ADR-010/011.

**Explicitly excluded.** No public marketplace, automatic updates, or native dynamic-library ABI.

# 16. Phase 8: Ecosystem readiness and public preview

**Outcome.** A credible first-party battery set, remote serving path, workflow adapters, polished documentation, security/release artifacts, and a supported `0.1.0` public preview.

**Planning range.** 4-6 weeks after core binding/durability/plugin gates; component work may proceed earlier in parallel

**Traceability.** PRD phases D-F, release criteria, success metrics; TDD milestone 8.

## Entrance criteria

- Native preview, Python alpha, WASM alpha, durability beta, and plugin alpha gates passed.

- Public API change backlog triaged.

## Exit criteria

- Core use cases have first-party examples and batteries.

- Remote protocol and reference server are usable.

- Security, benchmark, SBOM, and compatibility reports are published.

- `0.1.0` public-preview acceptance criteria are met.

## Pull request sequence

### PR-055 - Add Anthropic and Ollama/native-local provider packages

**Purpose.** Demonstrate provider diversity across a native cloud API and an OpenAI-compatible/local deployment path without making the Model port provider-shaped.

**Principal changes.**

- Implement Anthropic message/tool/thinking/stream semantics through the Model port.

- Add a supported Ollama/local provider configuration using the compatible adapter or a dedicated leaf implementation where needed.

- Add provider capability negotiation, model metadata refresh hooks, and fixture/live tests.

- Publish provider authoring guidance based on three implementations.

- Validate the already-shipped model-activated capability catalog against OpenAI-compatible and Anthropic prompt/tool/cache behavior.

- Add both providers to the curated Rust-backed Python wheel through lazy submodules while retaining separate installable Rust crates.

**Acceptance evidence.**

- Text, tool, usage, structured-output, cancellation, and error fixtures pass for each provider.

- Provider-specific reasoning/tool extension fields round-trip safely.

- Rust users can install only the provider crates they need; Python users receive the curated providers in the single supported wheel without eager provider initialization.

- Provider diversity does not change shared activation traces, and repeated activation does not rewrite the stable prompt prefix.

- Cross-provider model traces normalize to the same kernel semantics.

**Dependencies.** PR-004/ADR-023, PR-024, PR-032/PR-038, and stable Model port.

**Traceability.** FR-MDL, FR-CAP; ecosystem readiness.

**Explicitly excluded.** No exhaustive provider catalog or central router product.

### PR-056 - Harden filesystem, shell, repository-context, and compaction batteries

**Purpose.** Provide the minimum high-quality components needed for coding and research agent products.

**Principal changes.**

- Harden filesystem operations and add a shell toolset with allow/deny policy, timeouts, output bounds, and optional external sandbox adapter.

- Add repository instruction/context discovery as a ContextProvider.

- Add `before_model` sliding-window, large-tool-output, and model-assisted summarizing compaction middleware using the normalized PR-018 contract, with thresholds/hysteresis, protected-item rules, explicit budget/failure behavior, and prompt-cache-impact diagnostics.

- Add reference memory and retrieval/context-provider patterns, plus a verification middleware example that uses `before_finalize` to continue, request an interaction, fail, or accept the terminal result.

- Add large-tool-output truncation/spill behavior that preserves tool-call/result pairing, provenance, sensitivity, and inspectable artifact references.

**Acceptance evidence.**

- Security tests cover traversal, environment leakage, command policy, timeouts, and output floods.

- Context and compaction changes are attributed and replay-safe.

- Compaction never mutates canonical history; deterministic and summarizing strategies produce valid bounded requests, invalidate incompatible checkpoints, preserve protected content, and fail safely when no valid projection fits.

- Batteries remain separate leaf packages.

- Coding-agent example uses only public components.

- Reference memory/retrieval patterns use public ContextProvider and artifact/blob boundaries, and the verifier does not mutate state after the terminal record.

- Model-assisted summarization uses an explicit model/budget scope, cannot recursively invoke the same compaction chain, and has cancellation/recovery tests.

- Protected instructions remain byte/ID-identical; generated summaries remain provenance-bearing unprivileged context, and unauthorized sensitivity/residency/egress to a secondary model fails before dispatch.

**Dependencies.** PR-025, ContextProvider/Middleware ports, and durability where required.

**Traceability.** PRD UC-02, FR-CTX-003, and FR-MW-007; Architecture section 11.5; TDD section 17.6; Security Threat TM-21.

**Explicitly excluded.** No full coding-agent TUI or browser automation.

### PR-057 - Add observer adapters and production diagnostics

**Purpose.** Make runs inspectable without coupling the kernel to a telemetry vendor.

**Principal changes.**

- Implement structured logging, OpenTelemetry, and Prometheus/reference metrics adapters as separate packages.

- Define redaction and metadata-only policies for observer streams, diagnostic JSON/JSONL, support bundles, and journal exports, plus correlation fields, span/event mapping, and bounded exporter queues.

- Add runtime status, queue depth, effect latency, store latency, usage, and recovery metrics.

- Add content-redacted compaction metrics/events for trigger reason, strategy/version, estimated tokens before/after, checkpoint hit/miss/invalidation, summary usage/latency, failure/fallback, and prompt-cache impact.

- Publish trace examples and observer conformance tests.

**Acceptance evidence.**

- Observer failure does not change agent semantics.

- Secrets and protected tool payloads are redacted by default from observers and diagnostic/export projections; metadata-only modes preserve identifiers needed for safe operations without exposing content.

- Exporter backpressure is bounded and diagnosed.

- The minimal bundle contains no telemetry exporter.

- Compaction diagnostics expose no compacted source/summary content by default and observer failure cannot alter compaction behavior.

**Dependencies.** PR-018 and stable event envelopes.

**Traceability.** FR-OBS and FR-MW-007; Architecture sections 11.5 and 21; Security Threats TM-04, TM-17, and TM-21.

**Explicitly excluded.** No hosted telemetry service.

### PR-058 - Implement the remote protocol and reference session server

**Purpose.** Provide a clean serving boundary for UIs, gateways, and distributed applications.

**Principal changes.**

- Define a transport-neutral client/session protocol distinct from the plugin ABI.

- Implement the reusable 4-byte length prefix, deterministic CBOR envelope, hello/version negotiation, and pre-allocation limits generically over a payload schema family; layer correlated remote commands/results, authoritative snapshots, and transient events on top.

- Reserve a distinct external-process/plugin payload family that reuses framing and handshake code without reusing remote session message enums.

- Add a reference Unix-socket/TCP-local server with authentication hooks and single-writer session routing.

- Define the bounded pre-auth vocabulary/16-KiB ceiling, parse/auth deadlines and attempt caps, loopback/Unix-socket default, TLS 1.3+ and authentication requirement for non-loopback TCP, and downgrade floor.

- Add authenticated command IDs/digests and idempotency receipts, authorization per locator, reconnect snapshot-tail-barrier-live ordering, and bounded credit-window flow control.

- Add Rust and TypeScript client helpers plus protocol conformance tests.

- Require the bounded `SecurityAuditSink` in reference-server readiness and exercise malformed/unknown/scope-mismatch audit receipts without leaking target existence.

**Acceptance evidence.**

- Reconnect obtains a fresh authoritative snapshot before live events.

- Unknown protocol versions fail before session acquisition.

- Only the bounded hello/auth/close vocabulary is parsed before authentication; no session/target lookup occurs, oversized/malformed input fails before allocation, and all post-auth commands are scoped/authorized.

- The server refuses non-loopback plaintext operation and downgrade below policy; reconnect never interleaves live events before the snapshot/tail barrier, and slow clients resume from a durable cursor without dropping terminal completion.

- Protocol adapters do not expose store-private or kernel-private structures.

- Frame/handshake fuzz and size-limit tests are shared by remote and process payload fixtures; vocabulary compatibility tests remain separate.

**Dependencies.** PR-039, PR-046/047, and stable public events.

**Traceability.** PRD UC-06; Architecture ADR-014 and ADR-021; TDD section 28.2.

**Explicitly excluded.** No public cloud control plane, gateway dashboard, or multi-region routing.

### PR-059 - Add external workflow and durable-runtime adapters

**Purpose.** Let Temporal, Restate, DBOS, or similar systems drive the same effects without replacing kernel semantics.

**Principal changes.**

- Define a runtime-driver adapter contract for durable sleep, effect execution, persistence handoff, and run resumption.

- Implement one fully tested reference integration and one minimal second integration or example.

- Map workflow retries/idempotency, long waits, callbacks, and signals to kernel effect IDs, generic deferred effects, and interactions; document ownership boundaries.

- Add deterministic replay tests using the external system test harness where feasible.

**Acceptance evidence.**

- The integration does not reimplement the model/tool continuation loop.

- Kernel journal/effect IDs remain authoritative for agent semantics.

- External retries cannot silently exceed kernel policy.

- A typed human interaction, externally completed deferred effect, or long timer survives worker restart in the reference integration.

**Dependencies.** PR-045, PR-048, and runtime driver interfaces.

**Traceability.** PRD UC-09; Architecture sections 2.3, 9.5, and 10.

**Explicitly excluded.** No requirement to support every workflow engine before `0.1.0`.

### PR-060 - Complete documentation, starter repositories, and security/release artifacts

**Purpose.** Make the framework adoptable and independently reviewable.

**Principal changes.**

- Publish concept, Rust, Python, WASM, durability, provider, toolset, plugin, server, and migration guides.

- Create minimal agent, coding agent, Python service, browser worker, durable interaction/approval, and WIT plugin starters.

- Refresh the pre-implementation Threat Model with implemented-control evidence, residual risks, and deployment guidance; generate SBOMs, checksums, provenance, dependency policy reports, and the public security response process.

- Publish governance/RFC guidance and verify package metadata, source headers where used, and release artifacts consistently declare `MIT OR Apache-2.0` and DCO contribution policy.

- Run accessibility and documentation-link checks plus fresh-user usability sessions.

**Acceptance evidence.**

- Every public package has a tested quick start.

- Starter repositories pin compatible versions and pass CI.

- Security documentation clearly distinguishes native, Python/JS callback, process, and WASM trust levels.

- Release artifacts are reproducible from tagged source.

- License, DCO, maintainer ownership, ADR, and public RFC links are reachable from every contribution entry point.

- Every Threat Model control required at G7 links to passing evidence, an explicitly accepted residual risk, or a blocking issue.

**Dependencies.** All prior public surfaces.

**Traceability.** Engineering Standards; Security and Threat Model sections 12-14; PRD distribution, risks, release criteria, NFR-DX, and NFR-SEC.

**Explicitly excluded.** No commercial support portal or marketplace.

### PR-061 - Cut the `0.1.0` public preview release

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

- All three capability activation modes pass shared public-preview traces; model activation remains append-only at safe checkpoints.

- Release `0.1.0` and gate G7 - Public Preview - are approved.

**Dependencies.** PR-055 through PR-060 and all previous gates.

**Traceability.** PRD section 15; TDD section 36.

**Explicitly excluded.** No 1.0 compatibility guarantee.

# 17. Phase 9: 1.0 hardening and general availability

**Outcome.** A stable, audited, performance-hardened 1.0 with migration tooling, ecosystem conformance, and explicit long-term compatibility promises.

**Planning range.** 6-10 weeks after `0.1.0` feedback; planning range only

**Traceability.** PRD long-term success metrics and release criteria; Architecture evolution; all NFR families.

## Entrance criteria

- `0.1.0` used by external adopters.

- Preview telemetry, issue patterns, API pain points, and migration needs reviewed.

## Exit criteria

- Public contracts frozen under SemVer policy.

- Security and reliability reviews complete.

- Performance budgets enforced.

- `1.0.0` artifacts and migration guides released.

## Pull request sequence

### PR-062 - Freeze 1.0 public contracts and migration tooling

**Purpose.** Convert preview experience into explicit stability boundaries.

**Principal changes.**

- Review and freeze Rust APIs, AgentSpec, error codes, event/record schemas, Python API, JS API, remote protocol, and the WIT 1.0 interface.

- Add automated compatibility checks, deprecated aliases, migration commands, and fixture converters where promised.

- Document which leaf provider/tool packages may evolve faster than the core.

- Adopt independent versioning only where it reduces ecosystem coupling safely.

**Acceptance evidence.**

- Breaking-change tests detect incompatible public/schema changes.

- Every `0.1.0` supported project has a documented `1.0.0` migration path.

- Deprecated APIs carry removal versions.

- Compatibility policy is approved by maintainers.

**Dependencies.** PR-061 and preview feedback.

**Traceability.** NFR-COMP; schema governance.

**Explicitly excluded.** No permanent compatibility promise for explicitly experimental packages.

### PR-063 - Harden performance, memory, and startup budgets

**Purpose.** Ensure the architecture delivers measurable benefit rather than only conceptual modularity.

**Principal changes.**

- Optimize reducer allocations, raw JSON handling, stream batching, registry resolution, provider reuse, and store replay.

- Measure and enforce the numeric NFR-PERF-001 through NFR-PERF-006 targets on versioned published reference workloads/environments for native, Python fast path, browser WASM, and applicable WIT benchmarks; enforce NFR-PERF-007 as a steady-state conformance rule against repeated schema/validator compilation.

- Profile one thousand idle sessions and high-concurrency active sessions.

- Publish representative flamegraphs and tuning guidance.

**Acceptance evidence.**

- No unexplained benchmark regression remains versus `0.1.0` baselines.

- Published evidence covers NFR-PERF-001 through NFR-PERF-007; every applicable 1.0 target or budget passes on its reference workload/environment or has an approved unexpired exception, and repeated schema/validator compilation fails conformance.

- Minimal binary and WASM bundle size targets are measured and enforced.

- Memory growth under long streams and repeated runs is bounded.

**Dependencies.** PR-061.

**Traceability.** NFR-PERF-001 through NFR-PERF-007 and NFR-REL; TDD section 33.

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

**Traceability.** NFR-SEC and NFR-REL; Engineering Standards section 10.

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

**Traceability.** Engineering Standards sections 9 and 12-13; PRD release criteria and ecosystem readiness.

**Explicitly excluded.** No centralized commercial plugin marketplace.

### PR-066 - Ship the `1.0.0` release candidate and general availability release

**Purpose.** Complete the product build with stable artifacts and long-term operating commitments.

**Principal changes.**

- Run the full release-candidate soak period with external adopters and no unreviewed API changes.

- Publish final crates, wheels, npm/WASM package, WIT packages, server/client packages, fixtures, SBOMs, checksums, and benchmark/security reports.

- Publish the `1.0.0` migration guide, compatibility matrix, support policy, and roadmap.

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
- **Property tests:** record replay equivalence, event ordering, lane invariants, source-order tool finalization, compaction protected-item/tool-pair preservation, checkpoint invalidation, and serializer round-trips.
- **Fuzz tests:** untrusted records, events, specs, remote frames, WIT payloads, raw JSON, and recovery sequences.
- **Runtime integration tests:** commit faults, task cancellation, backpressure, slow consumers, provider/tool failures, compaction thresholds/hard budgets/fallbacks, and resource cleanup.
- **Crash-prefix tests:** process stops before and after every persistent write, compaction middleware/summary/checkpoint boundary, and external-effect boundary.
- **Binding conformance:** the same golden traces, including compacted model-visible projections, through Rust, Python, and browser WASM APIs.
- **Plugin conformance:** lifecycle, permissions, resource limits, traps, oversized payloads, and ABI mismatches.
- **Security verification:** threat-control fixtures, authorization negatives, hostile/malformed input, secret redaction, boundary limits, and insecure-example scanning.
- **Downstream tests:** starter projects and reference integrations run against release candidates.

## 18.2 Performance program

Performance measurement starts in Phase 0 and becomes release-blocking only after stable baselines exist. Reports must separate:

- deterministic reducer time;
- runtime scheduling and queueing;
- provider/tool adapter overhead;
- storage commit and restore cost;
- deterministic/model-assisted compaction, checkpoint reuse, and token/cache impact;
- Python FFI conversion and callback cost;
- JavaScript/WASM conversion and host-promise cost;
- WIT canonical ABI and Wasmtime overhead; and
- external network/model/tool latency.

Real model latency must never be used to hide framework overhead. The PRD values are engineering targets from the start, not assumed measurements. Before PR-063, benchmark method, reference-environment metadata, regression reporting, and comparison against those targets are normative, but the numeric targets are warning evidence rather than release-blocking budgets. PR-063 uses the Phase 3-and-later corpus to ratify the reference workloads/environments and activate the NFR-PERF-001 through NFR-PERF-006 targets as 1.0 warning/failure budgets; any unmet applicable target requires an approved unexpired exception. NFR-PERF-007 is a conformance rule rather than a numeric budget.

## 18.3 Security program

Security work is continuous:

- the pre-implementation Threat Model, security ownership/reporting path, dependency/license policy, secret scanning, and control-to-evidence plan begin in Phase 0;
- filesystem/tool boundaries are tested in Phase 3;
- host-language callback trust is documented in Phases 4-5;
- durable auditability arrives in Phase 6;
- deny-by-default plugin isolation arrives in Phase 7;
- the Threat Model is refreshed against implemented controls and published with provenance, SBOM, deployment guidance, and the response process in Phase 8; and
- independent review closes before `1.0.0`.

# 19. Scope control and contingency sequencing

When staffing or schedule is constrained, scope should be cut in the following order while preserving architectural integrity.

## 19.1 Safe reductions that preserve public-preview scope

1. Limit the initial multi-lane surface to the PR-047 minimum—independent named lanes, one active operation per lane, concurrent sibling execution, and deterministic restore—while deferring convenience navigation and integration breadth.
2. Ship only one durable workflow integration.
3. Keep the initial WIT interface limited to toolsets and context providers.
4. Limit browser persistence to an example adapter rather than a supported package.

## 19.2 Items that must not be cut

- deterministic kernel decisions and record application;
- commit-before-effect ordering;
- stable effect IDs and idempotency metadata;
- bounded queues and tested cancellation;
- direct native handles after one-time resolution;
- shared Rust/Python/WASM trace fixtures for supported behavior;
- explicit trust levels for native, Python/JS, and WASM extensions;
- migration fixtures for every released persistent schema;
- an explicit `before_model` compaction contract that preserves canonical history and includes one deterministic safe strategy;
- explicit run lineage, generic deferred effects, generalized interactions, and a pre-terminal `before_finalize` behavior stage; and
- benchmark separation of framework overhead from external latency.

## 19.3 Pre-preview fallback release shapes

- **Rust-first `0.0.x` developer release:** end after Phase 3 if bindings are delayed; do not advertise cross-language parity.
- **Rust/Python `0.0.x` alpha:** ship Phase 4 before browser WASM if Python demand is materially higher, while keeping WASM kernel tests active.
- **Non-durable `0.0.x` alpha:** acceptable only before public preview; public preview includes the effect journal and at least SQLite recovery.
- **Plugin-delayed `0.0.x` alpha:** continue native/Python/WASM adoption work if WIT security or tooling is not ready, but do not label the release `0.1.0` or pass G7.
- **Provider-reduced `0.0.x` alpha:** ship two model paths while the third is incomplete, but do not label the release `0.1.0` or pass G7; changing the three-path public-preview scope requires a versioned PRD/plan amendment and applicable ADR reconciliation.
- **Shell-delayed `0.0.x` alpha:** retain calculator and constrained filesystem tools while shell hardening is incomplete, but do not label the release `0.1.0` or pass G7; removing shell from the accepted baseline requires a versioned PRD/plan amendment.

The `0.1.0` public-preview scope in section 2.1 and the G7/Phase 8 exit criteria remain fixed: isolated WIT plugins and a usable remote protocol/reference server are required. Replacing that scope requires a versioned PRD and Implementation Plan amendment with updated compatibility promises and gate evidence; contingency sequencing alone cannot do so.

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
| Compaction silently becomes history mutation | Replay/audit drift, broken tool pairing, or removed policy context | Keep compaction in `before_model` middleware; protect required items; record versioned evidence/checkpoints; fail safely when no valid projection fits |
| Review throughput becomes the bottleneck | Large stacked PRs and long-lived branches accumulate | Keep logical PRs small, merge test/schema foundations first, maintain main green |
| `1.0.0` is declared before ecosystem use | APIs freeze without external feedback | Require `0.1.0` adopter soak and migration feedback before Phase 9 freeze |

# 21. Program completion criteria

The implementation program is complete for 1.0 only when:

1. the kernel has no forbidden dependency and no external I/O;
2. every kernel transition has documented inputs, outputs, records, effects, and invariant tests;
3. recoverable effects commit request or deferral records before execution or external waiting, preserve the same effect ID through completion, and reject conflicting duplicate completions;
4. all runtime and binding queues are bounded;
5. cancellation and crash-prefix tests produce valid restorable states across lineage, deferred effects, interactions, and `before_finalize` decisions;
6. Rust, Python Rust-backed, and browser WASM pass common trace fixtures for shared behavior;
7. native extensions use direct handles after one-time resolution;
8. Python/JS/WIT callbacks are coarse, cancellable, and benchmarked;
9. public records, events, specs, errors, run relations, interactions, deferred-effect DTOs, remote DTOs, and WIT packages have versioned compatibility tests;
10. at least three model paths, safe context-compaction batteries, useful coding/research batteries, SQLite durability, observers, remote serving, and one workflow integration are documented and tested;
11. security, provenance, SBOM, migration, and release processes have been exercised; and
12. external preview users have validated the migration path to `1.0.0`.

# 22. Traceability summary

PRD lettered phases are capability groupings; the numbered phases and logical PRs in this plan own execution order. The Technical Design milestones are implementation summaries, not additional gates.

| PRD capability group | Implementation Plan delivery | TDD milestone(s) | Primary gate(s) |
| --- | --- | --- | --- |
| A - Semantic kernel | Phase 1, PR-006 through PR-013; Phase 0 is the governance prerequisite | 1 | G0, G1 |
| B - Native runtime and SDK | Phases 2-3, PR-014 through PR-026 | 2-3 | G2, G3 |
| C - Python and browser bindings | Phases 4-5, PR-027 through PR-038 | 4-5 | G4 |
| D - Durability and batteries | PR-025 foundation; Phase 6, PR-039 through PR-048; Phase 8, PR-056 through PR-057 | 3, 6, 8 | G3, G5, G7 |
| E - Isolated extension model | Phase 7, PR-049 through PR-054 | 7 | G6 |
| F - Server and ecosystem adapters | Phase 8, PR-055 through PR-061 | 8 | G7 |
| Cross-cutting GA hardening | Phase 9, PR-062 through PR-066 | Definition of done and all milestones | G8 |

A family traceability reference such as `NFR-PERF` expands to every numbered requirement in that family unless the entry names a narrower range. This convention avoids duplicating requirement prose while preserving ownership.

| Implementation area | Primary PR range | PRD families | Technical design areas |
| --- | --- | --- | --- |
| Governance and quality infrastructure | PR-001 to PR-005 | NFR-PORT, NFR-SEC, NFR-COMP, NFR-DX, release criteria | Engineering Standards, Threat Model, workspace, CI, testing, benchmarks |
| Semantic kernel | PR-006 to PR-013 | FR-KRN, FR-CAP foundations | IDs, messages, reducer, records, effects |
| Native runtime | PR-014 to PR-020 | FR-RT, FR-MDL, FR-TLS, FR-CTX, FR-MW, FR-OBS | Commit loop and six ports |
| Rust SDK/native MVP | PR-021 to PR-026 | FR-EXT, FR-CAP, FR-SPEC | Registrar, AgentSpec, providers, tools |
| Python | PR-027 to PR-032 | FR-PY | PyO3, callbacks, Pydantic, wheels |
| Browser WASM | PR-033 to PR-038 | FR-WASM | wasm-bindgen, host effects, workers, IndexedDB |
| Durability and lanes | PR-039 to PR-048 | FR-DUR | stores, encoding, recovery, deferred effects, interactions, lineage, lanes |
| Isolated plugins | PR-049 to PR-054 | FR-PLG | WIT, Wasmtime, permissions, guest SDK |
| Ecosystem/public preview | PR-055 to PR-061 | Release scope and representative use cases | Providers, compaction/context batteries, observers, server, workflows |
| 1.0 hardening | PR-062 to PR-066 | All NFR families | Compatibility, performance, security, release |

# 23. First 30 days

The baseline first-month sequence for a new implementation team is:

## Week 1

- Merge PR-001 and PR-002.
- Open PR-003 and PR-004 in parallel.
- Record and ratify the resolved ID representation, deterministic journal encoding profile, and async trait strategy ADRs together with the remaining Phase 0 decisions.

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
