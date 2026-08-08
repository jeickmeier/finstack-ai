---
title: "finstack-ai Engineering Standards"
subtitle: "Normative implementation, quality, compatibility, and review rules"
author: "finstack-ai project"
date: "2026-08-08"
---

# finstack-ai Engineering Standards

# Document control

| Field | Value |
|---|---|
| Product | finstack-ai |
| Document | Engineering Standards |
| Version | 0.4 |
| Status | Normative pre-implementation baseline |
| Date | 2026-08-08 |
| Primary audience | Maintainers, contributors, implementation teams, reviewers, and AI coding agents |
| Related documents | Product Requirements Document v0.7; Architecture Specification v0.6; Technical Design v0.8; Implementation Plan v0.8; Security and Threat Model v0.4 |

# 1. Purpose and authority

This document defines repository-wide engineering rules for implementing `finstack-ai`. It owns cross-cutting code, dependency, testing, compatibility, documentation, and review standards. It does not redefine product requirements, architectural boundaries, technical types, or delivery order.

The authority chain in `docs/README.md` applies. If a rule here conflicts with the PRD, an accepted ADR, or the Architecture Specification, implementation stops until the documents are reconciled. The Technical Design selects concrete mechanisms within these standards; the Implementation Plan schedules their delivery.

The words **must**, **must not**, **required**, **should**, and **may** are normative. A deviation from a **must** or **must not** rule requires an accepted ADR or a documented, time-bounded waiver under section 14.

# 2. Governing principles

1. Keep the semantic kernel deterministic, synchronous, I/O-free, and small.
2. Put product capabilities in composable extensions and runtime/application services, not new kernel concepts.
3. Commit recoverable intent before executing or waiting on external work.
4. Maintain one semantic engine and one cross-binding conformance corpus.
5. Resolve and validate configuration once; retain direct native handles on hot paths.
6. Bound queues, payloads, retries, concurrency, and retained state explicitly.
7. Prefer the smallest implementation that satisfies an accepted requirement and current delivery gate.
8. Do not add placeholder frameworks, speculative abstraction layers, or unused extension seams.
9. Treat public schemas and durable semantics as products with compatibility tests.
10. Measure performance at the framework boundary rather than inferring it from external model latency.

# 3. Architecture and dependency standards

## 3.1 Layering

The allowed dependency direction is:

```text
kernel <- runtime + six port contracts <- SDK/bindings/reference applications
   ^                ^
   |                |
protocol codec      leaf providers/tools/observers/plugin hosts
   ^                |
   +---- durable store implementations also depend on protocol codec
```

| ID | Rule | Required evidence |
|---|---|---|
| ENG-ARCH-001 | The kernel must not perform network, filesystem, database, clock, environment, process, or host-language I/O. | Dependency and source-boundary checks in CI. |
| ENG-ARCH-002 | The six primary ports remain `Model`, `Toolset`, `ContextProvider`, `Middleware`, `JournalStore`, and `Observer`. | ADR required for any proposed seventh port. |
| ENG-ARCH-003 | Kernel and runtime code must depend on contracts, never concrete providers, tools, databases, telemetry exporters, or plugin engines. | Package-graph tests and forbidden-import checks. |
| ENG-ARCH-004 | Native extensions resolve once and run through direct typed handles; no runtime registry lookup is permitted inside ordinary model/tool turns. | Construction and hot-path tests or benchmarks. |
| ENG-ARCH-005 | Python, JavaScript, remote, process, and WIT boundaries must remain coarse; per-token cross-boundary callbacks are prohibited by default. | Interface review and crossing-count benchmarks. |
| ENG-ARCH-006 | Feature-specific workflow, memory, RAG, channel, sandbox, subagent, or approval engines must not enter the kernel. | Architecture review and dependency checks. |
| ENG-ARCH-007 | A new abstraction must serve an accepted current requirement and at least one concrete implementation path. | PR traceability and explicit rejected simpler option. |

## 3.2 Package boundaries

- Every package must have one clear responsibility and an identified owner.
- Optional batteries and integrations must be leaf packages or feature-gated adapters.
- Default and minimal feature sets must be tested independently.
- `finstack-ai-runtime` keeps target-neutral port contracts available without Tokio. The facade's default `native-tokio` feature selects the runtime Tokio driver; browser-WASM depends on the facade with default features disabled and the non-default `wasm-host` pass-through feature.
- Functional capability features must be additive. The target-driver selectors `native-tokio` and `wasm-host` are mutually exclusive for browser builds and are enforced by target checks. Enabling one provider or battery must not silently enable unrelated providers, databases, telemetry, Python, Wasmtime, or browser code.
- Cyclic crate dependencies and facade-to-leaf backreferences are prohibited.
- Generated bindings or schemas must be isolated from hand-authored source and reproducible from a documented command.

# 4. Rust implementation standards

## 4.1 Toolchain and language use

- The workspace must declare a stable Rust toolchain and an explicit minimum supported Rust version before the first published crate.
- Stable Rust is the production baseline. Nightly-only tools may support fuzzing, sanitizers, or diagnostics but must not be required by consumers.
- `rustfmt`, workspace Clippy policy, documentation tests, and target checks are merge gates.
- Public crates must set `rust-version`, license, repository, documentation, and feature metadata consistently.

## 4.1.1 Repository toolchain and tasks

- Root `mise.toml` is the canonical pin for contributor and CI tool versions. Do not add `rust-toolchain.toml` or a parallel pin file that can drift from mise.
- Checked-in repository tasks are mise tasks. Documentation, CI, and agents must cite `mise run <task>` once a task exists.
- A Cargo `xtask` crate or equivalent second orchestration layer is prohibited. Prefer direct tool invocations (`cargo`, `uv`, formatters, linters) from thin mise tasks.
- Task scripts must stay sparse and fast. Complex shell or multi-step orchestration is allowed only when a direct one-liner cannot express the required check.
- Python local commands use `uv` under the mise-managed toolchain. Node/npm pins arrive with browser/JavaScript binding work, not earlier.
- Until an explicit MSRV decision is recorded for published crates, the stable Rust version pinned in `mise.toml` is the developer and CI channel.

## 4.2 Safety and panics

- New `unsafe` code requires a dedicated safety rationale, invariant documentation adjacent to the block or module, focused tests, and a named reviewer.
- `unsafe` is prohibited when a safe implementation meets the measured requirement.
- Recoverable input, provider, extension, persistence, protocol, and configuration errors must not panic.
- `unwrap`, `expect`, unreachable assumptions, indexing, and integer conversions are permitted only where the invariant is local, evident, and tested. Public-input paths must return stable errors.
- Drop and cancellation paths must not rely on panicking cleanup.

## 4.3 Errors and diagnostics

- Public errors must carry a stable code, safe human-readable message, relevant stable identifiers, retry classification where applicable, and a source chain for local diagnostics.
- Secrets, raw credentials, protected prompts, and unredacted tool payloads must not enter default error display or telemetry.
- Binding error hierarchies must map from the same framework error code rather than inventing incompatible semantics.
- Expected policy denial, validation retry, cancellation, timeout, and uncertain external outcome must remain distinct.

## 4.4 Ownership, concurrency, and resources

- Authoritative mutable state has one explicit owner. Shared native resources use handles such as `Arc` rather than deep copies.
- Every spawned task must have an owner, cancellation path, shutdown behavior, and join or detach policy.
- Every queue, stream buffer, batch, payload, retry loop, collection, and concurrency pool must have a configured bound or a documented bounded policy.
- Lock scope must not include external I/O or host-language callbacks.
- Time and randomness that affect semantics must enter through explicit inputs. New identifiers follow the accepted UUIDv7 decision.

# 5. Kernel, effects, and durability standards

| ID | Rule |
|---|---|
| ENG-SEM-001 | `decide` must not mutate authoritative state; only committed records may be applied. |
| ENG-SEM-002 | Given the same prior state and normalized input, the kernel must produce the same semantic decision. |
| ENG-SEM-003 | A recoverable external effect must not execute before its request record is durably committed. |
| ENG-SEM-004 | A deferred effect must retain its original `EffectId`; feature-specific completion protocols are prohibited. |
| ENG-SEM-005 | Equivalent duplicate external completions/resolutions are idempotent; conflicting duplicates fail closed and are auditable. |
| ENG-SEM-006 | Every accepted run has explicit root/parent lineage, including root runs. |
| ENG-SEM-007 | Human approval is an interaction profile, not a separate durable state machine. |
| ENG-SEM-008 | `before_finalize` is the last behavior-changing stage; observers cannot alter terminal state. |
| ENG-SEM-009 | Parallel tool execution may finish out of order, but semantic history and final events preserve source order. |
| ENG-SEM-010 | Snapshots are disposable versioned state-CBOR caches; the journal remains authoritative. |
| ENG-SEM-011 | Model-context compaction has one explicit late-tier `before_model` middleware owner per resolved agent, preserves protected content and canonical history, and records versioned evidence/checkpoints when behavior depends on it. |
| ENG-SEM-012 | The only pre-commit storage write permitted is idempotent content-addressed artifact staging: it has no externally visible authority/business effect, is unreachable until a journal reference commits, and has digest verification plus bounded orphan collection. All other recoverable external work follows ENG-SEM-003. |

Every record-changing PR must update reducer transition tests, replay fixtures, schema compatibility fixtures, and crash-prefix coverage where the transition touches durability or external work. Compaction changes additionally require protected-content, tool-pairing, checkpoint invalidation, binding-parity, and canonical-history immutability tests.

# 6. Public API, schema, and compatibility standards

## 6.1 Public contracts

The following are compatibility-controlled contracts:

- Rust public APIs and feature names;
- Python modules, classes, exceptions, and wheel support matrix;
- JavaScript/TypeScript exports and WASM host interfaces;
- `AgentSpec`, bundle, tool, interaction, and structured-output schemas;
- journal records and their meaning;
- event ordering and durable/transient classification;
- remote/process framing and message vocabularies; and
- WIT packages, worlds, resources, and error codes.

## 6.2 Rules

- JSON Schema draft 2020-12 is the portable schema source of truth.
- Public and durable structures must carry explicit format/schema versions where evolution requires independent decoding.
- Typed identifiers serialize as lowercase UUID strings unless an accepted protocol specifies another representation.
- Raw JSON may cross extension boundaries, but it must be size-bounded and validated once at the declared ownership boundary.
- Unknown fields, versions, and enum variants must follow an explicit compatibility policy; silent semantic discard is prohibited.
- Candidate and experimental surfaces must be labeled before public preview. Stability claims begin only at the release gate that names them.
- Breaking journal meaning, event order, effect guarantees, WIT worlds, or remote protocol behavior requires an ADR, migration/compatibility plan, and fixture updates in the same change.

# 7. Cross-language and extension standards

## 7.1 Semantic ownership

Rust owns kernel and runtime semantics. Bindings expose handles and normalized commands; they must not reimplement the continuation loop, recovery rules, interaction state machine, lineage rules, or event ordering.

## 7.2 Python

- Rust-backed paths must release the GIL during Rust-only scheduling, I/O, storage, and waits.
- Python callbacks are explicit trusted-code boundaries and must be cancellable, timeout-bounded, and coarse.
- Schemas and metadata are cached at registration; no per-token or per-call regeneration is allowed on hot paths.
- Python-native validation may replace the Rust adapter only at the Python object boundary and only behind shared conformance fixtures.

## 7.3 Browser WebAssembly and JavaScript

- The kernel must compile without Tokio, native sockets, filesystem assumptions, or provider credentials.
- JavaScript fulfills host effects through bounded adapters; it does not own semantic state.
- Web Workers are the default high-volume topology. Shared-memory threading is opt-in and post-preview unless reconsidered by ADR.
- Browser examples and adapters must never embed provider API keys; use a same-origin or explicitly trusted application proxy.

## 7.4 WIT and isolated extensions

- The initial experimental WIT 0.x interface remains limited to coarse toolset/context calls and final completion/error semantics.
- A component cannot invoke a full agent or create competing run-lineage semantics unless a future ADR expands the ABI.
- Wasmtime, signatures, WASI permissions, and compilation caches remain in optional leaf packages.
- Resource, payload, time, fuel, and instance limits are mandatory for untrusted components.

# 8. Security and privacy standards

The `Security and Threat Model` owns threat assumptions and security control obligations. At minimum:

- native Rust and Python/JavaScript callbacks are trusted unless placed behind an explicit isolation boundary;
- isolated plugins receive no ambient filesystem, network, environment, clock, process, or secret authority;
- secrets travel through scoped references or host resolution, not ordinary prompts, records, events, schemas, or bundle files;
- authentication must finish before remote frames, external completions, or interaction resolutions are trusted;
- authorization decisions must bind the principal, tenant/scope, target session/run/effect/interaction, and permitted action;
- untrusted input must be bounded before allocation and parsed without panics;
- observer payloads and diagnostic/export projections of durable data require redaction and metadata-only modes; v1 authoritative records retain or securely reference replay-required state and expose no field-level redaction/tombstone mutation; whole-session destruction is a deployment retention operation that permanently removes resumability; and
- security-sensitive changes must update the threat model and tests in the same review.

# 9. Dependency and supply-chain standards

- Workspace dependencies and feature policy are centralized. Published crates use compatible version requirements; committed lockfiles pin repository builds.
- Dependencies must be maintained, necessary, license-compatible, and appropriate for all enabled targets.
- A new kernel dependency requires size, transitive graph, portability, maintenance, license, and determinism review.
- `cargo deny` or equivalent policy must check advisories, licenses, sources, and duplicate-risk exceptions in CI.
- Git dependencies, prereleases, forks, and unmaintained packages require an ADR or time-bounded waiver with an exit plan.
- Build scripts, code generators, external binaries, actions, and release tools must be version-pinned; release inputs additionally require checksums or immutable digests where supported.
- Release artifacts include SBOMs, checksums, provenance, and reproducible-build evidence at the gates defined by the Implementation Plan.
- Optional integrations must not enlarge the minimal artifact's dependency graph.

# 10. Testing and verification standards

## 10.1 Required layers

| Layer | Purpose |
|---|---|
| Unit | Local invariants, parsing, policy, and error behavior. |
| Transition table | Every valid and invalid kernel state/input pair. |
| Property | Replay equivalence, ordering, lineage, idempotency, bounds, and serialization properties. |
| Fuzz | Records, schemas, protocol frames, plugin inputs, raw JSON, and recovery sequences. |
| Integration | Port/runtime behavior, cancellation, backpressure, cleanup, and failure mapping. |
| Fault/crash prefix | Every persistent write and recoverable external-effect boundary. |
| Cross-binding conformance | Shared semantic traces through Rust, Python, and browser WASM. |
| Compatibility | Historical public schemas, journals, protocols, and WIT fixtures. |
| Performance | Deterministic synthetic paths with framework costs separated from external latency. |
| Security | Threat-model controls, malformed/adversarial input, permission denial, and secret-redaction checks. |

## 10.2 Evidence rules

- A green aggregate test suite does not replace a focused test proving the changed invariant.
- A fixture must activate the behavior it claims to test; setup-only or dead-path fixtures are rejected.
- Flaky tests must be fixed, quarantined with an owner/expiry, or removed if they test no supported contract. Blind retries are not a fix.
- Live-provider tests supplement scripted deterministic tests and must not gate ordinary offline development unless explicitly selected.
- Tests must not depend on wall-clock races, unordered map iteration, external credentials, or network availability by default.
- Coverage percentages are diagnostic only; invariant and boundary coverage are the acceptance criteria.

# 11. Performance standards

- Performance work starts with a reproducible benchmark and a confirmed hot path.
- Reports separate reducer, scheduling, provider/tool adapter, storage, FFI/host crossing, serialization, and external I/O time.
- Throughput claims must state workload, target, build profile, concurrency, payload sizes, and measurement uncertainty.
- Optimizations must preserve determinism, cancellation, error fidelity, compatibility, and readability unless an ADR accepts a tradeoff.
- Allocation reduction must target measured churn; shared ownership or pooling must not create unbounded retention.
- Warning thresholds may precede stable budgets. Release-blocking budgets begin only after the relevant surface is candidate-stable.

# 12. Documentation standards

- Public APIs require API documentation, failure semantics, and a tested example.
- Every primary document has one declared owner and version. The README authority chain determines conflict resolution, not filename order.
- A change to product semantics, architecture, records, protocols, support policy, or delivery gates must update all affected primary documents, traceability, ADRs, and fixtures in one review unit.
- Documentation must distinguish committed scope, gated future work, examples, and non-goals.
- The Future Capabilities Design Validation is supporting analysis; incorporated requirements live in the authoritative documents.
- Generated reference material must identify its source and regeneration command. Hand edits to generated output are prohibited.
- Links, examples, code blocks, and documented commands must be checked in CI when the relevant tooling exists.

# 13. Pull-request and review standards

Every implementation PR follows the required sections and definition of done in Implementation Plan sections 6.1 and 6.2. In addition:

- the PR must identify the smallest user or architecture outcome it completes;
- unrelated formatting, dependency, generated-file, and semantic changes must be separated when practical;
- reviewers must be able to trace public behavior to requirements and tests without reconstructing intent from chat history;
- kernel semantics require a kernel owner and durability reviewer;
- public binding changes require the binding owner and semantic-conformance evidence;
- persistence/protocol changes require compatibility and migration review;
- security-boundary changes require a security reviewer and threat-model update; and
- benchmark claims require review of fixture activation and measurement method.

# 14. Exceptions and waivers

A waiver is allowed only when it records:

- the exact rule and affected scope;
- why compliance is currently impractical;
- risk and compensating control;
- accountable owner;
- expiry date or delivery gate;
- removal issue; and
- whether public compatibility or security is affected.

Waivers cannot authorize a new primary port, external I/O in the kernel, silent data loss, unbounded resource use, unauthenticated privileged input, or an undocumented public compatibility break. Those require a superseding ADR/design change or are prohibited.

# 15. Gate compliance

| Gate | Standards evidence |
|---|---|
| G0 Foundation | Architecture/dependency checks, toolchain policy, PR template, ADR/waiver process, initial threat model. |
| G1 Kernel Semantics | Transition, property, fuzz, replay, and target-build evidence for all semantic rules. |
| G2 Native Runtime | Commit-before-effect, queue bounds, cancellation ownership, leak/fault evidence. |
| G3 Native Preview | Public API docs, provider/tool conformance, synthetic benchmarks, package metadata. |
| G4 Binding Parity | Shared traces and measured coarse crossing behavior for Python/WASM. |
| G5 Durable Beta | Compatibility/migration fixtures and full crash-prefix evidence. |
| G6 Plugin Alpha | Deny-by-default permissions, limits, signature policy, and hostile-component tests. |
| G7 Public Preview | Published threat model, SBOM/provenance, security process, support scope, and starter validation. |
| G8 1.0 GA | Compatibility freeze, independent security review, enforced budgets, release rehearsal, and waiver closure. |

# 16. Phase 1 readiness checklist

Product implementation beyond the Phase 0 foundation may begin when:

1. the documentation authority chain is accepted;
2. ADR-001 through ADR-037 are recorded and accepted as defined by PR-004;
3. package boundaries and forbidden kernel dependencies have executable check designs;
4. the initial toolchain, MSRV decision point, license, DCO, and dependency policy are owned;
5. semantic, crash-prefix, conformance, security, and benchmark fixture layouts are agreed;
6. the Security and Threat Model is reviewed by the owners of runtime, bindings, durability, and plugins; and
7. exceptions to these standards are either resolved or recorded with owners and expiry gates.
