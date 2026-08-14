# PR-047 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Branch (when admitted): `codex/pr-047-multi-lane-concurrency`
Baseline (when admitted): PR-046 review head
  `0303309c2b8ebffc583514d6190d90dc089f06a8`
  (implementation candidate `dc16907e4e8d2c8a303788dbb9fc04febb1a0572`)
Plan baseline: documentation pack v0.20 / PLAN-0.18 / Implementation Plan SHA-256
`555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`

This file is the execution contract for PR-047. It does **not** authorize
coding, branch creation, commit, merge, or hosted actions. The open
PR-043, PR-044, PR-045, and PR-046 envelopes are not reused. `continue`
does not start this PR. Planning does not admit PR-047.

## Admission

- Phase 6 entrance is `Passed` (3/3). Do not re-record `PH6-E-entrance-*`.
- PR-039 is `Done` at `64c54e767f53faac240ab92c19a8447264e82ff8`.
- PR-040 is `Done` at `dbd10d35b223288666b2fdc0e13d03f48b5b97c3`.
  ADR-016 SQLite conflict/recovery evidence is the hard store
  prerequisite.
- PR-041 is `Done` at `8a84293264291dbe158fae1b4e5dedcc30c74061`.
- PR-042 is `Done` at local merge `4d627711632c771d733b689e1325b2d9679ee317`.
- PR-043 is `In review` on `codex/pr-043-tool-reconciliation` (candidate
  `86f71c8fd47c0c6d721f9ae36df5c0ba90b22025`).
- PR-044 is `In review` on `codex/pr-044-typed-interactions` (candidate
  `e56d6f1d0638986d1201f2b901f372d64d01d062`).
- PR-045 is `In review` on `codex/pr-045-durable-cancel-timers` (candidate
  `e58ff33138dd1c4d5c1be9e611575b12cd3a4ac5`).
- PR-046 is `In review` on `codex/pr-046-conversation-tree-main-lane`
  (candidate `dc16907e4e8d2c8a303788dbb9fc04febb1a0572`, evidence
  `0303309c2b8ebffc583514d6190d90dc089f06a8`). PR-043, PR-044, PR-045,
  and PR-046 stay `In review`.
- Dependencies: PR-040, PR-046, and ADR-016. Do not admit while PR-046
  is the sole active logical PR.
- ADR-012 stays Accepted / In progress / **Partial**. This PR adds
  public concurrent multi-lane evidence. Do **not** mark Implemented
  (PR-048 crash-prefix remains).
- ADR-016 stays Accepted / In progress / **Partial**. This PR is the
  public multi-lane successor that ADR-016 gated behind SQLite. Do
  **not** mark Implemented (PR-048 crash-prefix / G5 remain).
- ADR-026 stays Accepted / In progress / **Partial**. This PR adds
  runtime lineage-aware cancel / deadline / depth / budget fan-out.
  Do **not** mark Implemented (PR-048 crash-prefix remains).
- ADR-013 stays Accepted / Not started / Missing. Do not silently
  mark Implemented or Partial.
- ADR-004 stays Accepted / Partial. Do not mark Implemented.
- ADR-025 / ADR-027 / ADR-033 stay as left by PR-044/PR-045.
- No Implementation Plan section 6.3 ADR trigger applies if the work
  stays inside the existing six ports, existing checksum/encoding
  policy, and at-least-once plus idempotency. Do not add a seventh
  port. Do not add a `SessionStore` / `Lane` / `Timer` / `Channel`
  port. Do not add a journal family. Do not add kernel-state v7.
- Threat Model section 18 is triggered (lineage / cancellation /
  journal / session structure; primary **TM-15**; SEC-INV-008;
  secondary TM-12 and TM-19). Complete the review before merge. A
  threat-model review is not an ADR and does not stop coding after
  admit.
- Phase 6 exit “initial multi-lane session semantics” is the
  semantics half of this PR (named lanes, one active operation per
  lane, concurrent siblings, deterministic restore). Do not infer
  G5. G5 still waits for PR-048.
- Implementation Plan §19.1: keep the PR-047 minimum surface.
  Defer convenience navigation and integration breadth (channel
  servers, bookmarks, checkout UX, named checkpoints).

## Acceptance mapping

Five Implementation Plan bullets map 1:1 to A01–A05.

- PR-047-A01: two lanes can share a history prefix and diverge
  without copying entries. `create_lane` plus `navigate` /
  fork-from-leaf points the new lane at an existing `EntryId`.
  `extract_history` from each leaf walks `parent_id` and shares the
  prefix. No entry bytes are duplicated. A later append on lane B
  does not rewrite lane A’s leaf or any shared ancestor’s
  `parent_id`.
- PR-047-A02: a busy lane rejects a second operation while sibling
  lanes continue. Pattern: start a non-terminal run on `main`
  (park on `AwaitingInteraction`, `AwaitingExternal`, or
  `Sleeping`); start a second run on a sibling lane; the sibling
  reaches a later phase; a second `AcceptRun` / `lane.run` on
  `main` is rejected; the sibling is not cancelled or blocked.
- PR-047-A03: interleaved lane records restore deterministically
  from the shared session sequence. Two lanes append
  `ConversationEntry` / `LaneMoved` / operation records in
  overlapping order. `recover` / `Session::open` rebuilds the same
  leaves, entries, and per-lane active/suspended operations.
  `last_applied_sequence` is the session head.
- PR-047-A04: concurrency stress tests preserve single-writer
  invariants. At least one fixture uses
  `finstack-ai-store-sqlite` (ADR-016). Parallel appenders on
  different lanes never tear a batch, never reuse a sequence, and
  restore equal after join. In-process lane guards reject a second
  owner of the same `(session_id, lane_id)`. Store compare-and-
  append remains the cross-process linearizer; do not invent a
  lock protocol.
- PR-047-A05: child-run and lane relationships remain distinct and
  replay to the same result under interleaving. A
  `CompatibleLaneInParentSession` child is a different `RunId` on
  a different `LaneId` in the same session. An `IsolatedChildSession`
  child is a different `SessionId`. Parent cancel walks
  `child_mappings` and submits `CancelRequested { initiator:
  ParentRun { parent_run_id } }` to each child that is not
  detach-preauthorized. Lanes are not lineage. Equal replay of the
  interleaved journal yields the same mappings, relations, and
  terminal/cancel outcomes.

## Execution envelope

Authorized 2026-08-14 by `implement the plan`.

Local branch and commit are authorized. Merge to `main`, feature-branch
push, hosted PR/merge, npm publish, tag, and G5 inference are not named
and remain prohibited. The open PR-043, PR-044, PR-045, and PR-046
envelopes are not reused.

`mode=stacked` (local implementation on the PR-046 review head);
`target` unset; `external actions=none`.

PR-043, PR-044, PR-045, and PR-046 remain `In review` and are not
marked `Done`. This PR is the active implementation slice; it does
not close or merge those PRs.

Baseline: `0303309c2b8ebffc583514d6190d90dc089f06a8`.
Branch: `codex/pr-047-multi-lane-concurrency`.

## Locked design

Do not invent a seventh port, a `SessionStore` port, a `Lane`
port, a `Channel` port, `ConversationId`, a second sequence space,
kernel-state v7, a new journal family, `RemoteChildSession`
dispatch, distributed multi-writer, or new `RunPhase` values.
Activate Architecture §9.2–9.4 / TDD §24.2 / TDD §22.6.3 fan-out
/ existing PR-008/PR-013/PR-039/PR-046 vocabulary.

### 1. SessionRuntime is the session writer, not a port

Architecture §13.1: Python `Session` holds `Arc<SessionRuntime>`.
TDD §25.2 lists `Session` and `Lane` as published classes.

Add `SessionRuntime` in `finstack-ai-runtime` (internal). The SDK
exposes `finstack_ai::Session` and `finstack_ai::Lane` as thin
handles. This is composition, not a seventh port.

```text
SessionRuntime owns:
  store: Arc<dyn JournalStore>
  session_id: SessionId
  projection: SessionProjection          // rebuilt / updated from journal
  guards: (session_id, lane_id) -> owner // TDD §24.2
```

`CommitCoordinator` stays **one kernel = one run**. Concurrent
siblings are N coordinators (or N kernels) sharing one
`SessionRuntime` / one store. Do not put multiple runs on one
`KernelState`. Do not change `kernel-state` hashing.

PR-046 recover rule remains: a coordinator recovered for run R
applies R’s records and treats other `run_id`s as foreign (known
child / child `RunAccepted` skip; unknown foreign still fails
closed). `last_applied_sequence` is still the **session** head.

`Session::open(store, session_id)` rebuilds the projection and
does **not** auto-respawn every non-terminal run. `lane.resume()`
attaches the existing `RunId` / phase (PR-045/PR-046 park+recover).
`lane.run(input)` is a new root on that lane and is rejected when
`active_on_lane` is `Some`.

### 2. Lane guard and single-writer linearization

TDD §24.2: guard keyed by `(session_id, lane_id)`. Only one active
run or structural mutation owns the guard.

PR-047 principal change: linearize **lane mutations** (create,
navigate / `LaneMoved`, `ConversationEntry`, `AcceptRun`,
terminals) while model/tool/timer **effects** run outside the
mutation line.

Linearizers already exist — reuse them:

- in-process: the lane guard;
- cross-process / racing appenders: store compare-and-append plus
  the existing `Conflict` reload path on `CommitCoordinator`.

Do not add a new lock file, advisory-lock table, or `LaneLock`
record. SQLite sequence conflicts are the ADR-016 proof surface
(A04). MemoryJournalStore is enough for A01–A03 and A05.

A second `lane.run` / `AcceptRun` on a guarded lane fails closed
with a stable configuration / busy-lane error. A sibling lane is
not blocked.

### 3. Public Session / Lane APIs — minimum surface

Use the Implementation Plan verbs. Do not invent a parallel
vocabulary (`checkout`, `branch`, `workspace`, `thread`).

```text
Session::create(store, tenant, ...) -> Session
  // SessionCreated + LaneCreated("main") if the session is new.
  // Agent::start that still allocates a fresh session remains valid
  // and must keep the PR-046 bootstrap order.

Session::open(store, session_id) -> Session

Session::create_lane(name, fork) -> Lane
  // LaneCreated { name } on a new LaneId.
  // fork = None (empty leaf) or Some(entry_id) (shared prefix).
  // Some(entry_id) commits LaneMoved to that existing entry.
  // Reject duplicate name, empty name, or a second "main".

Session::list_lanes() -> [Lane]
Session::lane(name | lane_id) -> Lane          // inspect / lookup

Lane::navigate(entry_id)
  // LaneMoved to an existing entry. Does not copy.
  // Does not move any other lane’s leaf.
  // Reject unknown EntryId.

Lane::run(input) -> Run                       // AcceptRun if idle
Lane::cancel()                                // run-level + fan-out
Lane::suspend()                               // stop driver; journal stays
Lane::resume()                                // recover + respawn
Lane::inspect()                               // name, leaf, active RunId, history
```

§19.1 **convenience navigation** is out: no bookmark API, no
named checkpoints, no “checkout” alias, no history UI helpers
beyond `inspect` + existing `extract_history`.

`create_lane(..., Some(entry_id))` **is** the fork. Do not add a
separate `fork()` unless it is a one-line alias of that call; the
plan verbs already include create + navigate.

### 4. Shared prefix without copying

Architecture §9.3 / A01:

```text
entry A -> entry B -> entry C     (lane main)
                 \-> entry D      (lane research)
```

Lane `research` is created with `fork = Some(B)`. Both leaves
share A–B by `parent_id`. New entries on each lane set
`parent_id` to that lane’s current leaf. `maybe_commit_conversation_siblings`
already writes `ConversationEntry` + `LaneMoved` beside
`EntryAppended` / `ToolCallSettled`; keep that hook and apply it
to **every** bootstrapped lane, not only `main`.

Do not clone `ConversationEntry` bodies onto the new lane.

### 5. Binding Session / Lane — Architecture §13.1

PR-046 deferred binding handle parity. This PR owns it.

Today’s Python/JS `Session` is an operation-locator snapshot
(`tenant_scope`, `session_id`, `lane_id`, `run_id`). TDD §25.2
and Architecture §13.1 name `Session` as the live handle.

Lock:

- Rust `finstack_ai::Session` / `Lane` are the live handles.
- Python `Session` becomes `Arc<SessionRuntime>` and a new `Lane`
  class is exported (TDD §25.2).
- JS/WASM `Session` becomes the live handle; add `Lane`.
- Locator fields stay on `Run` (`session_id`, `lane_id`, `run_id`,
  `tenant_scope`) and on callback / error context dicts. That
  snapshot is **not** the Session handle.
- `Run.session` returns the live `Session` handle (Architecture
  13.1). Update `test_handles.py` and JS tests that treated
  `run.session` as a frozen locator (`to_dict()` moves to
  `Run.locator` or the existing error-context dict).
- `Agent.inspectSession` may remain as a convenience DTO; it is
  not a substitute for `Session.open` + `list_lanes`.
- Pre-1.0 lockstep: change Python and JS in the same PR. Do not
  leave a tracked deferral for the other binding.

Do not add `conversation_entry` to `normalize_prebeta_shape`.

### 6. External identity mapping hooks

PR-047: “Expose external identity mapping hooks for channels/threads.”
Future-capabilities §13.2:

```text
(channel account, conversation/thread identity) -> (session_id, lane_id)
```

This is a **host-owned map**, not a kernel record and not a port.
Channels remain applications (PRD non-goal: no gateway product).

```rust
pub struct ExternalIdentityKey {
    pub channel: Arc<str>,
    pub account: Arc<str>,
    pub thread: Arc<str>,
}

pub trait ExternalIdentityMap: Send + Sync {
    fn resolve(&self, key: &ExternalIdentityKey) -> Option<(SessionId, LaneId)>;
    fn bind(
        &self,
        key: ExternalIdentityKey,
        session_id: SessionId,
        lane_id: LaneId,
    ) -> Result<(), IdentityMapError>;
}
```

Ship `MemoryExternalIdentityMap` for tests and as the in-process
hook. Equal bind is idempotent; conflicting bind fails closed
(SEC-INV-004 / TM-19). Do not persist the map in the journal. Do
not put keys on `SessionCreated` metadata as authority. The host
decides durability.

`Session::bind_external_identity(map, key)` / `Session::resolve`
are the SDK hooks. No Slack/email adapter.

### 7. Lineage-aware fan-out — runtime service, not a port

TDD §22.6.3: the kernel owns one run and emits no child-dispatch
action. `Cascade` accepts `ParentRun` from the exact parent.
`DetachOnlyIfPreauthorized` rejects unless the child’s accepted
authorization decision ID is prefixed `detach:`. PR-011 already
proves the normalized child decision. PR-045 already submits
run-level `CancelRequested`. This PR owns **routing**.

```text
SessionRuntime::cancel_lane(lane_id) / cancel_run(run_id)
  1. Submit KernelInput::CancelRequested to that run’s coordinator
     (existing PR-045 path; initiator Principal or RuntimeShutdown
     for the lane API).
  2. Walk projection.child_mappings where parent_run_id == run_id.
  3. For each child:
       IsolatedChildSession -> Session::open(child session) then
         cancel that session’s root / mapped run.
       CompatibleLaneInParentSession -> cancel that sibling lane’s
         run (same SessionRuntime).
       RemoteChildSession -> do not dispatch (vocabulary only).
  4. Child kernel applies ParentRun { parent_run_id } and the
     persisted RunPropagationPolicy. Detached children stay up.
  5. Recurse on the child’s own mappings (depth already on accept).
```

Deadline: children with `DeadlinePropagation::MinimumOfParentAndChild`
receive a `CancelRequested { initiator: Deadline }` or a tighter
persisted due_at through the existing timer path. Do not invent
`TimerId`. Do not `ExecuteEffect` a conversation entry.

Budget: do not add ledger records. Existing reservation /
`ChildRunPrepared.budget_reservation_id` stay. Fan-out does not
silently grant budget. Depth remains an accept-time kernel check.

Do not treat `LaneId` as a cancel target for children. Mapping
key is `(parent_run_id, parent_effect_id)`.

### 8. CompatibleLane child creation uses public lanes

`ChildRunCoordinator::start_or_attach` stays. For
`CompatibleLaneInParentSession`, the caller must have a real
second `LaneId` (PR-046 tests wrote `LaneCreated` by hand). This
PR’s public `create_lane` is that path. Isolated child sessions
still bootstrap their own `main` (PR-046). `RemoteChildSession`
stays vocabulary-only.

### 9. Bindings, fixtures, corpus

- No new `RecordBody` variant. Journal v1 family count stays **40**.
- Do not rewrite existing journal envelope known-answers.
- Do not rewrite v1–v6 kernel-state hashes or `LaneMoved` /
  `EntryAppended` / `ConversationEntry` wire shapes.
- Public Rust types `Session` / `Lane` / `ExternalIdentityKey`
  need public-rust-api fixtures if exported. Corpus **128 → 128+N**
  (count the new subjects; do not guess N until the types exist).
- Subject prefix `pr047-*`. Do not reuse `pr046-record`.

## Restore / concurrency matrix (in-process unless noted)

Not OS kill. Not PR-040 sqlite process-kill. Not PR-048
crash-prefix. Default store: `MemoryJournalStore`. A04 **requires**
one SQLite stress fixture.

| Row | Scenario | After |
| --- | --- | --- |
| 1 Shared prefix | create_lane(fork=B) after A-B-C on main | history(main)=A-B-C; history(research)=A-B; B unchanged (A01) |
| 2 Diverge | append D on research | no copy; main leaf still C (A01) |
| 3 Busy vs sibling | park main; run research; second main run | main rejected; research continues (A02) |
| 4 Interleave restore | overlapping appends; drop; open | same leaves, ops, sequence (A03) |
| 5 SQLite stress | parallel lane appenders | no torn batch; equal restore (A04) |
| 6 Guard | two owners of same lane_id | second fails closed (A04) |
| 7 Compatible child | child on sibling lane; interleave | distinct RunId/LaneId; mapping intact (A05) |
| 8 Isolated child | child other session; parent cancel | child cancelled iff Cascade (A05) |
| 9 Detach | detach-preauthorized child | parent cancel does not cancel child (A05) |
| 10 Identity hook | bind/resolve; conflicting bind | equal idempotent; conflict fails (TM-19) |

## Implementation pitfalls

1. Do not put the tree or lane names on `KernelState`.
2. One kernel per run. Foreign-run skip stays as PR-046 refined it
   (known children / child `RunAccepted` only).
3. `last_applied_sequence` is the session head.
4. Do not `ExecuteEffect` a conversation entry or `LaneMoved`.
5. Keep PR-045 rules (interaction effect, expire-if-due, outstanding
   set) on each run’s coordinator.
6. `ChildRunPrepared` envelopes keep the **parent** `run_id`.
7. Do not treat `LaneId` as lineage.
8. Pre-046 journals without `SessionCreated` must still open.
9. Do not run `write_journal_v1_fixtures()` without reverting
   existing envelopes.
10. Do not mark ADR-012/016/026 Implemented.
11. Do not add `RemoteChildSession` dispatch.
12. Workspace sqlite `concurrent_readers_never_observe_a_torn_batch`
    can flake; isolate and record honestly. A04 is a **different**
    fixture (lane appenders), not that reader test.
13. `maybe_commit_conversation_siblings` must fire for every
    bootstrapped lane, not only `main`.
14. Python/JS `Run.session` meaning changes; update tests in the
    same PR.
15. Cancel fan-out is at-least-once plus idempotent
    `CancelRequested`. No exactly-once claim.

## Files

Create later (implementation / candidate, not this planning step):

- `docs/implementation/artifacts/pr-047/README.md`
- `docs/implementation/artifacts/pr-047/candidate-validation.txt`
- `docs/implementation/artifacts/pr-047/security-review.txt`

Create when implementing:

- `crates/finstack-ai-runtime/src/session.rs` — `SessionRuntime`,
  lane guard, linearized mutation entry, cancel fan-out
- `crates/finstack-ai-runtime/src/identity_map.rs` —
  `ExternalIdentityKey`, `ExternalIdentityMap`,
  `MemoryExternalIdentityMap`
- `crates/finstack-ai/src/session.rs` — public `Session` / `Lane`
- `crates/finstack-ai-test/tests/lanes.rs` — A01–A05
- public-rust-api fixtures under
  `fixtures/compatibility/public-rust-api/v1/pr047-*`

Modify when implementing:

- `crates/finstack-ai-runtime/src/lib.rs` — export session/identity
  types
- `crates/finstack-ai-runtime/src/coordinator.rs` — append/notify
  through `SessionRuntime` when present; keep single-run kernel
- `crates/finstack-ai-runtime/src/composition.rs` — child on a
  public sibling lane; fan-out uses mappings already restored
- `crates/finstack-ai/src/lib.rs` / `agent.rs` — `Agent::start`
  still bootstraps `main`; `Run.session` returns the live handle
- `crates/finstack-ai-kernel/src/conversation.rs` — `lane(name)`
  lookup if missing; no `KernelState` fields
- `crates/finstack-ai-test/tests/public_rust_api.rs` — corpus 128+
- `bindings/finstack-ai-python/src/lib.rs`,
  `python/finstack_ai/__init__.py`, `_finstack_ai.pyi`,
  `tests/test_handles.py` — Session handle + Lane
- `bindings/finstack-ai-wasm/src/agent.rs`,
  `js/src/agent.ts`, `js/src/index.ts` — Session handle + Lane
- `docs/implementation/delivery-ledger.md` — admit/tracking rows
  only at admit

Do not modify: `docs/planning/`, WIT, UI, cron/scheduler,
PR-048 crash-prefix / migrators / G5, IndexedDB schema v1,
SharedArrayBuffer, kernel-state hash fixtures for v1–v6,
existing journal envelope known-answers, `LaneMoved` /
`EntryAppended` / `ConversationEntry` wire shapes.

## Proposed tasks

Create ledger rows only at admit. Summaries are
implementation-specific.

1. Tracking — confirm PR-039–PR-046, ADR-012/016/026/013, TM-15,
   ADR-016 SQLite prerequisite, exclusions, no branch until
   PR-046 is no longer the sole active PR.
2. SessionRuntime + lane guard — failing tests first: guard
   keyed by `(session_id, lane_id)`; structural mutations
   serialize; effects stay outside; projection shared. No new
   port. No `KernelState` fields.
3. Public create / list / inspect / navigate — A01 shared-prefix
   fork without copying; `LaneMoved` only; sibling hook for every
   bootstrapped lane.
4. Concurrent sibling run — A02 busy-lane vs sibling continue;
   A03 interleaved restore from the shared sequence; one kernel
   per run.
5. Lineage fan-out — A05 cancel / deadline / depth / budget
   routing across CompatibleLane and IsolatedChildSession;
   detach-preauthorized stays up; lanes ≠ lineage.
6. Binding Session / Lane + identity hooks — Architecture §13.1
   handles; `MemoryExternalIdentityMap`; Python and JS in the
   same PR; locator stays on `Run`.
7. SQLite single-writer stress + validation — A04; TM-15 review;
   graph checks; corpus 128+N; stop before G5.

## Validation (when authorized)

Focused:

```text
cargo test -p finstack-ai-runtime --features native-tokio --offline --locked session
cargo test -p finstack-ai-runtime --features native-tokio --offline --locked lanes
cargo test -p finstack-ai-test --test lanes --offline --locked
cargo test -p finstack-ai-test --test conversation --offline --locked
cargo test -p finstack-ai-test --test public_rust_api --offline --locked
cargo test -p finstack-ai-store-sqlite --offline --locked -- --test-threads=1
cargo fmt --all --check
cargo clippy --workspace --all-targets --offline --locked -- -D warnings
```

Candidate:

```text
cargo test --workspace --offline --locked
cargo test -p finstack-ai-store-sqlite --offline --locked -- --test-threads=1
cargo test -p finstack-ai-test --test journal_v1 --offline --locked
cargo test -p finstack-ai-test --test public_rust_api --offline --locked
cargo tree -e normal --locked --offline -p finstack-ai-runtime -p finstack-ai -p finstack-ai-kernel
uv run --no-project python tools/wasm_package/check.py graph
```

Focused Python handle tests (`test_handles.py` plus a new
`test_session.py`) are required because `Session` / `Lane` change
in this PR. Do not require `mise run ci`, the full pytest suite,
sqlite kill, or browser tests to close A01–A05. Do not infer G5.

## Explicit exclusions

No seventh port. No `SessionStore` / `Lane` / `Channel` port. No
kernel-state v7. No new journal family. No rewrite of v1–v6
hashes or `LaneMoved` / `EntryAppended` / `ConversationEntry`
wire shapes. No distributed multi-writer or replicated sessions.
No `RemoteChildSession` dispatch. No channel/gateway product. No
convenience navigation beyond create / list / inspect / navigate.
No PR-048 crash-prefix, G5, WASM durable restart, migrators, or
snapshot DTO rebuild. No silent ADR-012/016/026 Implemented. No
ADR-013 Partial. Do not reuse the PR-043, PR-044, PR-045, or
PR-046 envelope. Do not admit while PR-046 is the sole active
logical PR. Do not mark PR-043, PR-044, PR-045, or PR-046 `Done`.
