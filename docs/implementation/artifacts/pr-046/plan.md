# PR-046 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Branch (when admitted): `codex/pr-046-conversation-tree-main-lane`
Baseline (when admitted): PR-045 review head `8e935847625d48367053c28d08130ecf215a9070`
  (implementation candidate `e58ff33138dd1c4d5c1be9e611575b12cd3a4ac5`)
Plan baseline: documentation pack v0.20 / PLAN-0.18 / Implementation Plan SHA-256
`555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`

This file is the execution contract for PR-046. It does **not** authorize
coding, branch creation, commit, merge, or hosted actions. The open
PR-043, PR-044, and PR-045 envelopes are not reused. `continue` does
not start this PR. Planning does not admit PR-046.

## Admission

- Phase 6 entrance is `Passed` (3/3). Do not re-record `PH6-E-entrance-*`.
- PR-039 is `Done` at `64c54e767f53faac240ab92c19a8447264e82ff8`.
- PR-040 is `Done` at `dbd10d35b223288666b2fdc0e13d03f48b5b97c3`.
- PR-041 is `Done` at `8a84293264291dbe158fae1b4e5dedcc30c74061`.
- PR-042 is `Done` at local merge `4d627711632c771d733b689e1325b2d9679ee317`.
- PR-043 is `In review` on `codex/pr-043-tool-reconciliation` (candidate
  `86f71c8fd47c0c6d721f9ae36df5c0ba90b22025`).
- PR-044 is `In review` on `codex/pr-044-typed-interactions` (candidate
  `e56d6f1d0638986d1201f2b901f372d64d01d062`, evidence
  `a98e230e75dc3e621a55746f19b662cee9a7f2c9`).
- PR-045 is `In review` on `codex/pr-045-durable-cancel-timers` (candidate
  `e58ff33138dd1c4d5c1be9e611575b12cd3a4ac5`, evidence
  `8e935847625d48367053c28d08130ecf215a9070`). PR-043, PR-044, and
  PR-045 stay `In review`.
- Dependencies: PR-039 through PR-045. Do not admit while PR-045 is
  the sole active logical PR.
- ADR-012 stays Accepted. Register currently says Not started /
  Missing; the standalone record already cites Partial PR-014 lane
  identity. This PR advances conversation-tree + main-lane evidence
  to **Partial**. Do **not** mark Implemented (PR-047 concurrent
  lanes and PR-048 crash-prefix remain).
- ADR-016 stays Accepted / In progress / **Partial**. Public
  multi-lane APIs remain PR-047. Do not treat this PR as satisfying
  ADR-016.
- ADR-026 stays Accepted / In progress / **Partial**. This PR adds
  persist/expose/restore of run relations and the parent-effect →
  child-UUIDv7 mapping. Do **not** mark Implemented (PR-047
  propagation and PR-048 crash-prefix remain).
- ADR-013 stays Accepted / Not started / Missing. Do not silently
  mark Implemented or Partial.
- ADR-004 stays Accepted / Partial. Do not mark Implemented.
- ADR-025 / ADR-027 / ADR-033 stay as left by PR-044/PR-045.
- No Implementation Plan section 6.3 ADR trigger applies if the work
  stays inside the existing six ports, existing checksum/encoding
  policy, and at-least-once plus idempotency. Adding the TDD §24.1
  `ConversationEntry` journal family is activating deferred session
  vocabulary, not changing compatibility **policy**. Do not add a
  seventh port. Do not add a `SessionStore` / `Lane` / `Timer` port.
- Do **not** add kernel-state v7. The conversation tree is a
  session-level projection. Do not put entries, leaves, or lane
  names on `KernelState`. Histories with no new control records keep
  their existing `state_version` / `state_hash`.
- Threat Model section 18 is triggered (lineage / journal / session
  structure; primary **TM-15**; SEC-INV-008; secondary TM-12).
  Complete the review before merge. A threat-model review is not an
  ADR and does not stop coding after admit.
- Phase 6 exit “initial multi-lane session semantics” is **partial
  evidence** from this PR (main lane only). Do not infer G5. G5
  still waits for PR-048.

## Acceptance mapping

Five Implementation Plan bullets map 1:1 to A01–A05.

- PR-046-A01: entry parent chains never change after commit.
  After a `ConversationEntry` is committed, a later record that
  reuses the same `EntryId` with a different `parent_id` or body
  fails closed. Equal replay is idempotent. A branch-foundation
  append that points at an older parent does not rewrite that
  parent’s own `parent_id`.
- PR-046-A02: deleting only non-authoritative derived projections
  leaves a valid conversation tree. Drop `SnapshotWritten` /
  snapshot bytes / accelerated restore cache, then recover from the
  journal. The tree, main leaf, operation records, settlement
  identities, and `ChildRunPrepared` mappings remain. Do not treat
  `EntryAppended`, `ToolCallSettled`, `RunAccepted`,
  `ChildRunPrepared`, or conversation entries as disposable logs.
- PR-046-A03: the main lane restores its leaf and active/suspended
  operation. Pattern: create session + `LaneCreated { name: "main" }`,
  append entries, park on a non-terminal phase (`AwaitingInteraction`,
  `AwaitingExternal`, or `Sleeping`), abort the driving task, drop
  the owner, `recover`, respawn. `leaf_id` matches the last committed
  conversation entry. The restored run is the same `RunId` / phase.
  A busy main lane rejects a second root `AcceptRun`.
- PR-046-A04: history extraction preserves tool-call/result
  validity. `extract_history(leaf)` walks `parent_id` from the leaf
  to the root (oldest first). Every `ToolCall` on that path has a
  matching `Tool` result on the same path, or the path is rejected
  as an incomplete pair. Compaction / `ContextPrepared` projections
  are not conversation entries and cannot repair or split pairs.
- PR-046-A05: every restored operation has an unambiguous
  `RunRelation` (root/parent, `parent_effect_id`, kind, depth) and
  budget scope where applicable. The prepared parent-effect →
  child-UUIDv7 mapping and child security/propagation context
  survive recover for (1) `CompatibleLaneInParentSession` and
  (2) `IsolatedChildSession`. Equal retry attaches to the same
  mapped `RunId`. Conflicting remapping fails closed. Lanes are
  not the lineage model.

## Execution envelope

Authorized 2026-08-14 by `implement the plan`.

Local branch and commit are authorized. Merge to `main`, feature-branch
push, hosted PR/merge, npm publish, tag, and G5 inference are not named
and remain prohibited. The open PR-043, PR-044, and PR-045 envelopes
are not reused.

`mode=stacked` (local implementation on the PR-045 review head);
`target` unset; `external actions=none`.

PR-043, PR-044, and PR-045 remain `In review` and are not marked
`Done`. This PR is the active implementation slice; it does not
close or merge those PRs.

Baseline: `8e935847625d48367053c28d08130ecf215a9070`.
Branch: `codex/pr-046-conversation-tree-main-lane`.

## Locked design

Do not invent a seventh port, a `SessionStore` port, a `Lane` port,
`ConversationId`, a second sequence space, kernel-state v7, public
multi-lane APIs, cancel fan-out, cron, or new `RunPhase` values.
Activate TDD §24.1 / Architecture §9.2–9.5 / existing PR-008/PR-013
lineage and PR-039 structural vocabulary.

### 1. Conversation tree is session-level, not run state

`KernelState.messages` stays the run-linear model projection
(`EntryAppended` / `ToolCallSettled`). TDD §24.1 lives beside it:

```rust
pub struct ConversationEntry {
    pub id: EntryId,
    pub parent_id: Option<EntryId>,
    pub lane_id: LaneId,
    pub sequence: u64,
    pub body: EntryBody,
}

pub enum EntryBody {
    Message(Message),
}
```

`EntryBody` is unspecified in TDD; v1 is message-only. Role
(`User` / `Assistant` / `Tool`) distinguishes entries. Do not add
`System` as a required conversation role. Do not put this struct
on `KernelState` or in the `kernel-state` hash.

`sequence` must equal the committing `RecordEnvelope.sequence`.
`lane_id` must equal the envelope `lane_id`. Shared monotonic
sequence is the **session journal sequence** already on the
envelope. Do not invent a per-lane counter.

### 2. New structural journal family, no rewrite of old hashes

User prompts are not canonical today (`ContextPrepared` is a
model-visible projection). A01–A04 require first-class stored
entries, so add:

```text
RecordBody::ConversationEntry(ConversationEntry)
kind_name = "conversation_entry"
derived_event_count = 0
is_structural = true  // omit run_id
```

This does **not** change `EntryAppended`, `LaneMoved`, or v1–v6
hashes. Old journals without conversation entries remain valid:
synthesize a best-effort spine from `EntryAppended` +
`ToolCallSettled` for inspect-only; new sessions write the family.

Do not change `LaneMoved` shape. Do not stuff entries into
`SessionCreated` metadata.

Assistant/tool **operation** records stay as they are. When the
runtime commits an `EntryAppended` or `ToolCallSettled`, the same
batch (or the immediately following structural batch) also commits
`ConversationEntry` + `LaneMoved` so the tree and the run journal
cannot drift. `EntryBody::Message` bytes must equal the sibling
operation message. `EntryId` may share the message UUID bytes
(different tag) so leaf identity is stable.

### 3. Mandatory main lane

On first write to a new session, commit in order:

1. `SessionCreated`
2. `LaneCreated { name: "main" }` on the session’s `LaneId`
3. user `ConversationEntry` (`parent_id = None`) + `LaneMoved`

`main` is the stable application key. `LaneId` is generated once
and reused for that session. `Agent::run` that still allocates a
fresh session per call remains valid; it must persist the three
records above before `AcceptRun`.

Do not add public `create_lane` / `fork` / `list_lanes` /
`navigate` APIs. TDD §24.3 experimental fork is **not** required.
Tests may write a second `LaneCreated` to exercise
`CompatibleLaneInParentSession` without exposing it.

Runtime lane guard: keyed by `(session_id, lane_id)`, at most one
active operation. A second root `AcceptRun` on busy `main` is
rejected. SQLite cross-process sequence conflicts stay PR-040
evidence; do not add a new lock protocol. Concurrent sibling
execution is PR-047.

### 4. Session projection and recover

`CommitCoordinator::recover` today replays the whole session into
one `Kernel`. Structural records are no-ops. A same-session child
`RunAccepted` would break that replay.

Keep one kernel on the coordinator: the **main-lane**
active/suspended (or sole) root operation. Apply rules:

- Structural / `ConversationEntry` / `LaneMoved` / `SnapshotWritten`:
  kernel no-op; session projection applies.
- `record.run_id == accepted.run_id` (or first `RunAccepted` on
  main): existing apply.
- `record.run_id` is a different operation: kernel no-op, but
  `last_applied_sequence` still advances. Session projection
  records the operation summary (`RunAccepted` relation/security
  and later terminal).
- Do not reset kernel-state version when skipping foreign runs.

Session projection (runtime, rebuilt from the journal; not hashed
into `kernel-state`):

```text
lanes: name, leaf_id, created
entries: EntryId -> ConversationEntry (immutable)
operations: RunId -> relation, security, propagation, phase/terminal
child_mappings: (parent_run_id, parent_effect_id) -> ChildRunPrepared
```

Deleting snapshots (A02) uses this rebuild. Do not require a
durable side table.

### 5. History extraction

```text
extract_history(entries, leaf_id) -> Vec<ConversationEntry>
```

Walk `parent_id` until `None`. Reverse to root-first. Detect
cycles and missing parents; fail closed. Then validate tool pairs
on that path: each assistant `ToolCall` has exactly one later
`Tool` result with the same `tool_call_id` on the path. A result
without a call is invalid. Compaction output is out of scope.

Branch foundation (no public fork): a test may append an entry
whose `parent_id` is an older entry, then `LaneMoved` to it. The
abandoned sibling stays in `entries` with its original parent.
`extract_history` from each leaf is a different valid path.

### 6. Run relations and child mapping — persist/recover only

PR-013/PR-022 already own `RunRelation`, `RunAccepted`,
`ChildRunPrepared`, and `ChildRunCoordinator::start_or_attach`.
This PR does **not** redefine them.

Own:

- Rebuild `child_mappings` from every `ChildRunPrepared` in the
  session journal (parent envelopes keep the parent `run_id`).
- Recover `CompatibleLaneInParentSession`: parent session journal
  contains the mapping; child `RunAccepted` may sit on a different
  `lane_id`. Parent kernel skips the child run records. Lookup by
  `(parent_run_id, parent_effect_id)` returns the same child
  `RunId` after recover. Child security/propagation come from the
  child’s `RunAccepted` when that record is in-session, otherwise
  from the prepared locator + parent attenuation already on the
  mapping path.
- Recover `IsolatedChildSession`: mapping lives in the **parent**
  journal; child session recover loads the child’s own
  `RunAccepted.relation` and matches `parent_run_id` /
  `parent_effect_id` / `root_run_id` / depth / budget scope.
- Expose an inspect API on the session projection: each known
  operation’s relation, invocation effect, kind, depth, budget
  scope. Lanes are not lineage.

Do **not** add parent→child cancel fan-out, deadline broadcast,
or a child-run router service. TDD §22.6.3 routing is the
durable mapping + lookup. PR-047 owns lineage-aware propagation
and public multi-lane APIs. PR-048 owns crash-prefix / G5.

`ChildPlacement::RemoteChildSession` stays vocabulary-only.

### 7. Bindings stay locator-shaped

Python/JS `Session` is today’s operation-locator snapshot
(PR-028). Do not redefine it into a conversation handle in this
PR. Record an explicit tracked deferral: binding `Session`/`Lane`
handle parity waits for PR-047 (or a later named PR). Rust SDK
may add inspect helpers used by tests; they are not a new
published binding class.

If a public Rust type name is exported, add public-rust-api
fixtures. Corpus **127 → 128+** (at least
`roundtrip--conversation-entry.json`). Do not rewrite v1–v6
hashes.

## Restore matrix (in-process unless noted)

Not OS kill. Not PR-040 sqlite process-kill. Not PR-048
crash-prefix. Default store: `MemoryJournalStore`. One SQLite
row for A03 leaf/operation restore is allowed (store already
exists) but not required to close A01–A05.

| Row | Scenario | After recover |
| --- | --- | --- |
| 1 Parent immutable | commit entry; attempt rewrite `parent_id` | rejected; original parent remains (A01) |
| 2 Equal replay | same `ConversationEntry` twice | idempotent (A01) |
| 3 Branch foundation | append D with `parent_id = B` after C | B unchanged; history(D)=A-B-D; history(C)=A-B-C (A01) |
| 4 Drop snapshot | write snapshot; delete snapshot/accelerated | tree + leaf + mappings identical (A02) |
| 5 Main leaf + parked run | park non-terminal; drop; recover; respawn | same leaf, same `RunId` / phase (A03) |
| 6 Busy main | second root `AcceptRun` while active | rejected (A03) |
| 7 Tool-pair history | assistant with calls + results | extract keeps pairs; split fixture fails (A04) |
| 8 Compaction is not history | compacted `ContextPrepared` | extract ignores it (A04) |
| 9 Child other lane | `CompatibleLaneInParentSession`; recover | same mapped `RunId`; relation intact (A05) |
| 10 Child session | `IsolatedChildSession`; recover both | mapping + child relation match (A05) |
| 11 Equal child retry | `start_or_attach` after recover | same UUIDv7; no second child (A05) |
| 12 Conflicting remap | different child `RunId` for same effect | fail closed (A05) |

## Implementation pitfalls

1. Do not put the conversation tree on `KernelState` or bump
   `state_version` for structural records.
2. `last_applied_sequence` is the **session** head. Skipping a
   foreign-run record must still advance it, or the next append
   conflicts.
3. Do not `ExecuteEffect` a conversation entry. Do not emit
   derived events for `ConversationEntry`.
4. Do not `ExecuteEffect` the interaction `effect_id` (PR-045).
5. After deny/expire, do not re-request approval (PR-044).
6. `ChildRunPrepared` envelopes keep the **parent** `run_id` so
   they still apply to the parent kernel and
   `child_preparations`.
7. Same-session child `RunAccepted` must not be applied onto the
   parent kernel.
8. Do not treat `LaneId` as lineage. Relation fields stay on
   `RunAccepted`.
9. Do not write public multi-lane APIs or cancel fan-out.
10. Do not require `mise run test-lifecycle` — that task is gone.
11. Workspace sqlite `concurrent_readers_never_observe_a_torn_batch`
    can flake under parallel load; isolate and record honestly.
12. If spawn grows again, re-check clippy `large_futures` on test
    helpers (`Box::pin` as in PR-045).
13. Crash-style tests: `abort()` the driving task, await it, then
    `drop(permit)` / `drop(owner)`.
14. Spawn `event_task.run()` before resume/resolve/cancel `submit`.
15. Pre-046 journals without `SessionCreated` must still load; do
    not fail recover on missing conversation family.

## Files

Create later (implementation / candidate, not this planning step):

- `docs/implementation/artifacts/pr-046/README.md`
- `docs/implementation/artifacts/pr-046/candidate-validation.txt`
- `docs/implementation/artifacts/pr-046/security-review.txt`

Create when implementing:

- `crates/finstack-ai-kernel/src/conversation.rs` — `ConversationEntry`,
  `EntryBody`, parent-immutability + `extract_history` + tool-pair
  validation (I/O-free)
- `crates/finstack-ai-test/tests/conversation.rs` — A01–A05
  restore/history/mapping tests
- `fixtures/compatibility/public-rust-api/v1/kernel-input/` or
  `.../v1/` record fixture `roundtrip--conversation-entry.json`

Modify when implementing:

- `crates/finstack-ai-kernel/src/session.rs` — keep
  `LaneCreated` / `LaneMoved`; do not change their wire shape
- `crates/finstack-ai-kernel/src/records.rs` —
  `RecordBody::ConversationEntry`; `kind_name`; `is_structural`;
  `derived_event_count`
- `crates/finstack-ai-kernel/src/lib.rs` — export conversation types
- `crates/finstack-ai-kernel/src/events.rs` — structural zero-event
  arm
- `crates/finstack-ai-kernel/src/reducer/apply.rs` — kernel no-op
  for `ConversationEntry`; skip foreign `run_id` after accept
- `crates/finstack-ai-kernel/tests/semantic_reference.rs` —
  new kind name
- `crates/finstack-ai-runtime/src/coordinator.rs` — session
  projection rebuild; recover returns projection + main kernel;
  structural commit path (today `commit_composition_records`
  rejects non-child/budget bodies)
- `crates/finstack-ai-runtime/src/task.rs` / `host_task.rs` —
  bootstrap `SessionCreated` + `LaneCreated("main")` + user entry
  + `LaneMoved` before `AcceptRun`; lane guard; skip expire/resume
  rules from PR-045 remain
- `crates/finstack-ai/src/agent.rs` — persist main-lane bootstrap
  instead of generating a bare `AcceptRun`
- `crates/finstack-ai-runtime/src/composition.rs` — after recover,
  `start_or_attach` reads the session mapping (equal retry)
- `crates/finstack-ai-test/src/journal_bodies.rs` — include the
  new body in the exhaustive list
- `crates/finstack-ai-test/tests/public_rust_api.rs` — corpus
  127 → 128+
- `bindings/finstack-ai-python/src/lib.rs` and
  `bindings/finstack-ai-wasm/src/prebeta.rs` — add
  `conversation_entry` to `normalize_prebeta_shape` if that switch
  is exhaustive; do not invent a Python/JS Session handle

Do not modify: `docs/planning/`, WIT, UI, cron/scheduler,
PR-047 public lane APIs, PR-048 crash-prefix / migrators / G5,
IndexedDB v1, SharedArrayBuffer, kernel-state hash fixtures for
v1–v6.

## Proposed tasks

Create ledger rows only at admit. Summaries are
implementation-specific.

1. Tracking — confirm PR-039–PR-045, ADR-012/016/026/013, TM-15,
   exclusions, no branch until PR-045 is no longer the sole
   active PR.
2. Kernel conversation types — failing tests first: parent
   immutability, extract_history, tool-pair validity, structural
   `ConversationEntry` round-trip. No `KernelState` fields.
3. Session projection + recover — rebuild lanes/entries/operations/
   mappings; skip foreign-run apply; `last_applied_sequence` is
   session head; drop-snapshot rebuild (A02).
4. Main-lane bootstrap + guard — `SessionCreated` /
   `LaneCreated("main")` / user entry / `LaneMoved` before
   `AcceptRun`; busy main rejects a second root; A03 park/recover.
5. Sibling commit of tree vs operation records — assistant/tool
   batches also write `ConversationEntry` + `LaneMoved`; bytes
   match; A04.
6. Child mapping restore — rows 9–12; reuse
   `ChildRunCoordinator`; no cancel fan-out; expose relation
   inspect (A05).
7. Validation — A01–A05 fixtures, TM-15 review, graph checks,
   corpus 128+; stop before G5.

## Validation (when authorized)

Focused:

```text
cargo test -p finstack-ai-kernel --offline --locked conversation
cargo test -p finstack-ai-kernel --offline --locked session
cargo test -p finstack-ai-runtime --features native-tokio --offline --locked composition
cargo test -p finstack-ai-test --test conversation --offline --locked
cargo test -p finstack-ai-test --test public_rust_api --offline --locked
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

Do not require `mise run ci`, the full pytest suite, sqlite kill,
or browser tests to close A01–A05. Do not infer G5.

## Explicit exclusions

No seventh port. No `SessionStore` / `Lane` port. No kernel-state
v7. No rewrite of v1–v6 hashes or `LaneMoved` / `EntryAppended`
wire shapes. No public multi-lane create/list/run/cancel APIs
(PR-047). No concurrent sibling execution. No parent→child cancel
or deadline fan-out service. No `RemoteChildSession` dispatch.
No PR-048 crash-prefix, G5, WASM durable restart, migrators, or
snapshot DTO rebuild. No silent ADR-012/016/026 Implemented. No
ADR-013 Partial. Do not reuse the PR-043, PR-044, or PR-045
envelope. Do not admit while PR-045 is the sole active logical PR.
Do not mark PR-043, PR-044, or PR-045 `Done`.
