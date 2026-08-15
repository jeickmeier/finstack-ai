# PR-059 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Intended branch (when admitted): `codex/pr-059-workflow-runtime-adapters`
Intended baseline: local `main` at `0a4b477c1eb9b04d2e86294a9c01b36855ddd74c`
Plan baseline: documentation pack v0.20 / PLAN-0.18 / Implementation Plan SHA-256
`555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`

This file is the execution contract for PR-059. The closed PR-054
envelope is not reused. The PR-055–PR-058 envelopes are not reused.
PR-059 is the only active logical PR once admitted. This planning
file does not admit the PR, start Phase 8, or record Phase 8
entrance.

## Execution envelope

Not authorized. Suggested text when the owner is ready:

```
Run PR-059; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

`implement the plan` is enough only if it names that same local-only
integrated envelope **and** the admission checks below are already
true. Do not infer authorization from this planning file, from
`continue`, from Phase 7 `Done`, from G6 `Passed`, or from the
PR-055–PR-058 plans existing.

When authorized, reuse:

```
mode=integrated; target=main
local branch/commit/merge authorized
external actions=none
```

Forbidden: push, hosted PR/merge, npm/pypi publish, crates.io
publish, tag, G5 inference, G7 inference. Do not write `G5-D-*` or
`G7-D-*`. Do not start PR-055–PR-058 or PR-060+. Do not cut or
publish `0.1.0`. Do not bump the lockstep workspace version off
`0.0.4`.

## Admission (when authorized)

Do not admit coding until all of the following are true. Planning
this file does not record them.

### Phase 8 entrance is `Passed` (2/2)

Recorded by PR-055. Current state is `Passed` (2/2).

| Entrance bullet | Current state |
| --- | --- |
| Native preview, Python alpha, WASM alpha, durability beta, plugin alpha | **Satisfiable.** G3/G4/G5/G6 are named (`G5-D-durable-beta-a9568bd869b5`). Do not record `PH8-E-entrance-gates-*` until PR-055 is admitted. |
| Public API change backlog triaged | **Blocked.** `docs/implementation/public-api-change-backlog.md` does not exist. |

Do not infer G5 from Phase 6 `Done`. Do not write the backlog or
`G5-D-*` from this PR. If the owner says `implement the plan` while
entrance is still `0/2`, **stop**.

Phase 8 entrance, once Passed, is recorded by the first admitted
Phase 8 PR. Do not re-record `PH8-E-entrance-*` here.

### PR-055, PR-056, PR-057, and PR-058 are `Done`

All four are planned only and still `Todo`. Implementation Plan
lists them before PR-059. Keep one active logical PR. Do not admit
PR-059 while any predecessor is `Todo` unless the owner explicitly
authorizes parallel Phase 8 work in the same sentence.

PR-059's *code* dependencies are PR-045 (public `ExternalClock`,
same-run cancel of interaction/timer/deferred effects), PR-048
(crash-prefix, SQLite leaf, binding restart traces), and the
existing runtime driver interfaces (`Clock`, `ManualDriveController`,
`ExternalCompletionRouter`, `InteractionRouter`,
`CommitCoordinator::recover_run`, `RunTaskOwner`,
`model_retry_allowed` / `tool_retry_allowed`). Those are already
on `main`.

### Other admission checks

- ADR-013 stays Accepted / Partial. This PR reuses at-least-once
  plus idempotency. Do **not** mark Implemented or claim
  exactly-once external side effects (Implementation Plan §6.3).
- ADR-004 / ADR-008 / ADR-020 stay Partial (G5 still owns
  durability closeout). ADR-014 / ADR-021 stay with PR-058.
- ADR-025 stays Partial / Verified only for the generic deferred
  path already on `main`. Do not reopen it.
- No Implementation Plan section 6.3 ADR trigger applies if the
  work adds no seventh port, no eighth middleware stage, no kernel
  I/O, no native dylib loader, and no stronger-than-at-least-once
  claim. Document ownership in the workflow README, not a new ADR.
- Threat Model section 18 is triggered (workflow / application
  boundary). Primary **TM-19**. Also **TM-10** / **TM-14**
  (retry and timer/cancel already proven; adapters must not bypass
  them) and **SEC-INV-002/003/004/007/008**. Complete the review
  before merge.

## Traceability

Implementation Plan PR-059; PRD UC-09; FR-RT-007; FR-RT-008;
Architecture §2.3, §4.2–4.3, §9.5, §10; TDD §9.1 / future-capabilities
§9.1–9.4 (agent run vs application workflow); ENG-SEM-004.
TM-19.
G7 is Phase 8's gate and is out of scope.

## Acceptance mapping

Four Implementation Plan bullets map 1:1 to A01–A04.

- PR-059-A01: The integration does not reimplement the model/tool
  continuation loop. The adapter drives existing
  `CommitCoordinator` / `RunTaskOwner` post-commit actions. A
  golden scripted model+tool run through the adapter produces the
  same journal record kinds, effect IDs, and `RunEvent` order as
  the same composition without the adapter. No workflow crate
  contains a model-request or tool-batch planner. Architecture
  §4.3 remains: one run follows the canonical machine.
- PR-059-A02: Kernel journal/effect IDs remain authoritative for
  agent semantics. Workflow checkpoints may store
  `(SessionId, LaneId, RunId, last_applied_seq)` as a hint only.
  After worker restart, restore is `JournalStore::load` +
  `recover_run`, not a workflow-owned copy of kernel state. A
  conflicting checkpoint sequence is ignored; the journal wins.
  Deferred completions and interaction resolutions keep the
  original `EffectId` / `InteractionId` (ENG-SEM-004). No
  feature-specific `AwaitingWorkflow` phase.
- PR-059-A03: External retries cannot silently exceed kernel
  policy. Before any workflow-level retry/re-dispatch, the adapter
  calls a runtime helper that composes `RunLimits.max_retries`,
  `RetrySafety`, and `model_retry_allowed` / `tool_retry_allowed`.
  A Temporal-shaped activity retry policy of N attempts against
  `max_retries = 1` executes only the kernel-allowed retries and
  then returns a durable `retry_limit_reached` / deny decision
  without a further `ExecuteEffect`. Conflicting duplicate
  completions stay fail-closed through the existing ingress
  routers.
- PR-059-A04: A typed human interaction, an externally completed
  deferred effect, **or** a long timer survives worker restart in
  the **reference** integration. Prove all three in the local
  harness (one test each is enough; do not pick only the easy
  one). Pattern: commit the wait, abort the driving task, drop
  the owner, reopen the store, `resume`, then resolve/fire. The
  same `EffectId` / `InteractionId` / `due_at` is used. A second
  resume is idempotent.

Principal changes that are not extra acceptance IDs, but are
required to prove the four bullets:

- Public runtime-driver adapter contract (not a seventh port).
- One fully tested in-process reference integration.
- One minimal Temporal-shaped second integration (no live cluster).
- Mapping of workflow retries, durable sleep, callbacks, and
  signals onto kernel effect IDs, generic deferred effects, and
  interactions.
- Deterministic replay tests on the reference harness.

## Locked design

### Layout

Follow Technical Design §2 and PRD §12.1 (`finstack-ai-workflow-*`).
Trusted native leaves stay under `extensions/`. The §2 tree does
not yet name a workflow class; add it as a **sibling** of
providers/toolsets/stores/observers, not a second root and not
under `crates/` or `plugins/`.

```text
crates/finstack-ai-runtime/src/workflow.rs            # contract types
extensions/workflow/README.md                         # ownership boundaries
extensions/workflow/finstack-ai-workflow-local/       # NEW; reference
extensions/workflow/finstack-ai-workflow-temporal/    # NEW; minimal second
examples/durable-interaction/                         # fill TDD placeholder
```

Do not create `finstack-ai-workflow-restate`,
`finstack-ai-workflow-dbos`, or a Temporal-backed `JournalStore`.
Do not invent `examples/workflow/` or `examples/temporal/`.
Do not put Temporal/Restate/DBOS SDKs in workspace dependencies.

Workspace members + `[workspace.dependencies]` at `0.0.4`.
`publish = false` on the example crate.

### Do not add a seventh port

The six ports stay `Model`, `Toolset`, `ContextProvider`,
`Middleware`, `JournalStore`, `Observer`. Workflow engines are
**runtime drivers**, not ports (Architecture §4.2; future-
capabilities §9.1). Do not add `AwaitingWorkflow`, workflow
record families, or a generic DAG in the kernel.

Reuse, do not reshape:

- `Clock` / `ExternalClock` (FR-RT-007; PR-045)
- `ManualDriveController` (pause before dispatch)
- `ExternalCompletionRouter` / `InteractionRouter` (FR-RT-008)
- `CommitCoordinator::recover_run` / `SessionRuntime::open`
  (inspect-not-continue stays; resume is a separate explicit call)
- `RunTaskOwner` respawn (PR-045/PR-048 crash tests)
- `model_retry_allowed` / `tool_retry_allowed` / `max_retries`

`Session::open` remains inspect-not-continue. Do not change that
contract. Resume lives on the workflow driver.

### Runtime contract (`finstack_ai_runtime::workflow`)

Feature-gate on `native-tokio`, same as ingress / manual drive.
No Tokio types in the public enum payloads beyond what those
modules already expose. Suggested surface (names may be
idiomatic-Rust adjusted; the vocabulary is locked):

```text
WorkflowWait::{
  Timer { effect_id, due_at },
  Interaction { interaction_id, request },
  DeferredEffect { effect_id, handle },
  Terminal { .. },
}

WorkflowRetryDecision::{ Allow { remaining }, Deny { code } }

WorkflowDriver
  drive_until_wait() -> WorkflowWait
  resume(store, locator, clock, audit) -> Self
  complete_external(ExternalEffectCompletionCommand) -> ExternalRouteOutcome
  resolve_interaction(InteractionResolutionCommand) -> ...
  retry_decision(effect_id) -> WorkflowRetryDecision
  persist_handoff() -> WorkflowCheckpoint  # hint only
```

`WorkflowCheckpoint` is `(tenant_scope, SessionId, LaneId, RunId,
last_applied_seq)` plus optional non-secret external handles.
It is not kernel state and is not a snapshot substitute
(Architecture §10.5).

`retry_decision` is the only legal path for a workflow engine to
ask "may I re-dispatch this effect?". Codes include
`retry_limit_reached`, `retry_not_safe`, `effect_not_outstanding`,
and existing ingress rejection codes. A `Deny` must not enqueue
`ExecuteEffect`.

Persistence handoff means: the adapter does not continue the
workflow past a wait until the journal commit that requested that
wait has succeeded (commit-before-effect, already true). The
checkpoint is written only after that commit. Do not add a second
authoritative store.

Do not add `temporalio`, `restate-sdk`, rustls, or reqwest to
runtime.

### Reference integration (`finstack-ai-workflow-local`)

Fully tested in-process durable driver. This is the UC-09
reference because a live Temporal/Restate/DBOS cluster is not
feasible under `external actions=none` and the local-only
envelope. "External system test harness where feasible" is this
harness: it supplies Temporal-like primitives (durable sleep,
signals, activity dispatch, worker crash/restart, deterministic
time) without a vendor SDK.

Required behavior:

- Durable sleep parks on a committed timer `EffectId` and
  advances only through `ExternalClock` (no wall-clock
  `tokio::time::sleep` in tests).
- Signals map to `InteractionRouter` or
  `ExternalCompletionRouter` with an explicit locator. No
  journal scan by globally supplied ID (Architecture §10.3).
- Worker restart: drop owner, reopen SQLite (or memory store
  cloned through a crash-prefix helper already used by PR-048),
  `WorkflowDriver::resume`.
- Replay: same journal prefix + same clock + same scripted
  model/tool → same `WorkflowWait` sequence and same traces.
- Tenant scope is taken from the acquired session handle, not
  from a user-selected ownership field (TM-19).

Depend on `finstack-ai-runtime` (native-tokio) and optionally
`finstack-ai-store-sqlite` / `finstack-ai-test`. Do not depend
on the kernel crate directly. Do not depend on the temporal
sibling.

### Minimal second integration (`finstack-ai-workflow-temporal`)

A Temporal-**shaped** mapping crate, not a Temporal Cloud worker.

Lock the vocabulary map in `extensions/workflow/README.md`:

| Temporal-shaped primitive | Kernel / runtime target |
| --- | --- |
| workflow / run ID | `RunId` plus explicit `OperationLocator` |
| activity ID / idempotency key | original `EffectId` |
| `workflow.sleep` / timer | timer effect + `ExternalClock` / `WorkflowWait::Timer` |
| signal | interaction resolution or external completion |
| activity retry policy | `WorkflowDriver::retry_decision` then maybe re-dispatch |
| worker restart | `WorkflowDriver::resume` |
| continue-as-new / child workflow | out of scope (child runs already have lineage; do not wrap them in a second graph) |

Implement a tiny in-memory Temporal-shaped worker (activities,
signals, timers) that drives `WorkflowDriver`. Tests use that
fake. Do **not** add `temporalio`, download a Temporal test
server, or open a network listener.

If a later PR wants a live Temporal SDK, it is a new leaf and a
new envelope. This PR only proves the mapping.

### Example

Fill `examples/durable-interaction/` (already in TDD §2). One
small binary: scripted or calculator model, SQLite journal, local
adapter, one typed interaction that survives a simulated worker
restart, then resolves and completes. `publish = false`.

Do not add workflow crates to `finstack-ai-native-examples`
default dependencies. G-01 stays: the ordinary SDK path does not
require a workflow engine.

### Bindings

No new Python or JavaScript workflow modules. Bindings already
have `open_session` (inspect-not-continue), interaction list/
resolve, and restart traces from PR-048. Those remain the
binding path. Do not add PyO3/wasm-bindgen workflow handles.

### Graph

Forbidden edges:

- kernel / default SDK / wit / wasm / `finstack-ai-native-examples`
  → `finstack-ai-workflow-local`, `finstack-ai-workflow-temporal`,
  `temporalio`, `restate-sdk`
- runtime → workflow leaves, Temporal/Restate/DBOS SDKs, rusqlite
  (runtime may expose the contract only)
- workflow crates → kernel (runtime + public semantic types only)
- workflow-temporal → workflow-local (both depend on the runtime
  contract, not on each other)

Add the two crate names to `tools/wasm_package/check.py`
`FORBIDDEN_WASM` / kernel forbidden set.

Do not restore `tools/architecture/`.
Do not invent `mise run schema-governance`.

### Ownership boundaries (document in README)

```text
workflow engine          business stages, durable sleep, signals,
                         worker identity, retry *intent*

kernel journal           agent-run semantics, effect IDs, phases,
                         settlement fingerprints

runtime driver           commit-before-effect, recover, ingress,
                         retry_decision, clock injection

application              tenant auth, inbox UI, Temporal/Restate
                         cluster, compensation graphs
```

Higher-level engines may compose multiple runs. One run follows
the canonical model/tool machine (Architecture §4.3).

## Tasks (when admitted)

Task IDs are minted at admit, not now. Do not start these until
admission checks pass.

1. Tracking: confirm Phase 8 entrance `Passed` (2/2) and
   PR-055–PR-058 `Done`; open
   `codex/pr-059-workflow-runtime-adapters` from the then-current
   `main` tip. Mark PR-059 `In progress`. Do not re-record
   Phase 8 entrance.
2. Runtime contract module + `retry_decision` + checkpoint hint
   types (A02, A03).
3. Local reference adapter + deterministic replay + clock-injected
   sleep (A01, A02).
4. Worker-restart proofs for interaction, deferred completion, and
   long timer (A04).
5. Temporal-shaped mapping crate + retry-policy exceed test (A03)
   and loop-parity golden (A01).
6. `examples/durable-interaction`, ownership README, TM-19
   cross-tenant negatives, candidate evidence. Stop before
   `G7-D-*`.

## Explicit exclusions

No requirement to support every workflow engine before `0.1.0`.
No live Temporal, Restate, DBOS, or Prefect cluster. No vendor
SDK in workspace.deps. No workflow-backed `JournalStore`. No
kernel workflow graph, seventh port, or `AwaitingWorkflow`. No
exactly-once claim for arbitrary side effects. No child-run
scheduler inside the adapter (lineage already exists). No
Python/JS workflow batteries. No remote-server work (PR-058).
No docs/security-release pack (PR-060). No G5 or G7 decision.
No `0.1.0` bump, publish, or tag.

## Validation

- `cargo tree -p finstack-ai-kernel -p finstack-ai-runtime -p finstack-ai --locked`
  — no `finstack-ai-workflow-local`, no
  `finstack-ai-workflow-temporal`, no `temporalio`, no
  `restate-sdk`
- `cargo tree -p finstack-ai --locked --no-default-features` — same
- `cargo tree -p finstack-ai-native-examples -p finstack-ai-wit --locked`
  — same
- `cargo tree -p finstack-ai-workflow-local --locked` — has
  runtime; no kernel crate, no wasmtime, no temporal sibling
- `cargo tree -p finstack-ai-workflow-temporal --locked` — has
  runtime; no `temporalio`, no kernel crate, no local sibling
- `cargo test -p finstack-ai-runtime --offline --locked workflow`
- `cargo test -p finstack-ai-workflow-local --offline --locked`
- `cargo test -p finstack-ai-workflow-temporal --offline --locked`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `uv run --no-project python tools/wasm_package/check.py graph`
- `mise run check` after the candidate is otherwise green

Do not require `mise run ci`, a Temporal test server, Docker,
Playwright, or Criterion numbers.

## Suggested authorization sentence

When Phase 8 entrance is `Passed` (2/2), PR-055–PR-058 are
`Done`, and the owner is ready:

```
Run PR-059; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

A later, separate sentence is required to record `G5-D-*`, triage
the public API change backlog, admit PR-055–PR-058, or record
`G7-D-*`. Do not infer those from `implement the plan`.
