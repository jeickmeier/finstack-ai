# PR-048 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Branch (when admitted): `codex/pr-048-crash-prefix-durability`
Baseline (when admitted): PR-047 review head
  `5850a90e957b447731beb6ba3ed28fc01af8eb2d`
  (implementation candidate `14af719a078f1cdf52382e89e0a44837ab207237`)
Plan baseline: documentation pack v0.20 / PLAN-0.18 / Implementation Plan SHA-256
`555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`

This file is the execution contract for PR-048. It does **not** authorize
coding, branch creation, commit, merge, hosted actions, publication, tag,
or a G5 decision. The open PR-043, PR-044, PR-045, PR-046, and PR-047
envelopes are not reused. `continue` does not start this PR. Planning
does not admit PR-048.

## Admission

- Phase 6 entrance is `Passed` (3/3). Do not re-record `PH6-E-entrance-*`.
  Phase 6 exit is `0/4`. This PR produces the exit evidence pack. It does
  not mark Phase 6 `Done`.
- PR-032 is `Done` (Python activation UX). PR-038 is `Done` (JS/WASM
  activation UX and G4). Those are the binding-alpha dependencies.
- PR-039 is `Done` at `64c54e767f53faac240ab92c19a8447264e82ff8`.
- PR-040 is `Done` at `dbd10d35b223288666b2fdc0e13d03f48b5b97c3`.
- PR-041 is `Done` at `8a84293264291dbe158fae1b4e5dedcc30c74061`.
- PR-042 is `Done` at `4d627711632c771d733b689e1325b2d9679ee317`.
- PR-043 is `In review` on `codex/pr-043-tool-reconciliation` (candidate
  `86f71c8fd47c0c6d721f9ae36df5c0ba90b22025`).
- PR-044 is `In review` on `codex/pr-044-typed-interactions` (candidate
  `e56d6f1d0638986d1201f2b901f372d64d01d062`).
- PR-045 is `In review` on `codex/pr-045-durable-cancel-timers` (candidate
  `e58ff33138dd1c4d5c1be9e611575b12cd3a4ac5`).
- PR-046 is `In review` on `codex/pr-046-conversation-tree-main-lane`
  (candidate `dc16907e4e8d2c8a303788dbb9fc04febb1a0572`).
- PR-047 is `In review` on `codex/pr-047-multi-lane-concurrency`
  (candidate `14af719a078f1cdf52382e89e0a44837ab207237`, evidence
  `5850a90e957b447731beb6ba3ed28fc01af8eb2d`). PR-043 through PR-047
  stay `In review`.
- Dependencies: PR-032, PR-038, and PR-039 through PR-047. Do not admit
  while PR-047 is the sole active logical PR.
- G5 is `Not ready`. Workspace versions are `0.0.2`. This PR stages
  `0.0.3` and a G5 readiness pack. It does **not** infer G5, cut the
  checkpoint, publish, or tag.
- ADR-004 / ADR-008 / ADR-012 / ADR-016 / ADR-020 / ADR-026 / ADR-027 /
  ADR-034 / ADR-037 stay Accepted / In progress / **Partial**. This PR
  adds crash-prefix, prune/horizon, activation-restart, and compaction
  recovery evidence. Do **not** mark Implemented (G5 is a named
  decision, not a candidate side effect).
- ADR-013 stays Accepted / Not started / Missing in the register. Do
  not silently mark Implemented. A truthful Partial row is allowed only
  after crash-prefix evidence exists; it is not required at admit.
- ADR-032 stays Implemented for disposable state-CBOR snapshots. Do not
  add a second snapshot DTO or compact projection.
- No Implementation Plan section 6.3 ADR trigger applies if the work
  stays inside the existing six ports, existing checksum/encoding
  policy, and at-least-once plus idempotency. Do not add a seventh
  port. Do not add a `SessionStore` / `Lane` / `Timer` / `Channel`
  port. Do not add a journal family. Do not add kernel-state v7. Do
  not claim exactly-once external effects. Adding optional
  `JournalStore::prune` with default `prune_unsupported` is a v1 store
  method addition, not a compatibility-policy change. A sqlite
  `user_version` bump is allowed only with a one-way migrator and
  fixtures; stop and use change control if the work needs a journal
  compatibility-policy change.
- Threat Model section 18 is triggered (journal, snapshot, effect,
  interaction, lineage, cancellation, retention/prune, browser
  persistence). Primary **TM-12**; also TM-10, TM-11, TM-14, TM-15,
  TM-21; G5 security row (corruption, replay, duplicate
  completion/resolution, backup/restore, migration, cross-scope
  recovery). Complete the review before merge. A threat-model review
  is not an ADR and does not stop coding after admit.
- Phase 6 exit bullets this PR must evidence, without marking the
  phase Done:
  1. SQLite persistence and recovery pass the crash matrix.
  2. Pending effects reconcile deterministically.
  3. Generalized interaction suspension/resume works through Rust and
     Python, with approval as the first supported profile.
  4. Initial multi-lane session semantics are implemented and
     documented (PR-047 candidate plus this PR's lane restart rows).
- Implementation Plan A11 says release checkpoint `0.0.3` and G5 have
  passed. Engineering rules: gates are never inferred; range or PR
  authorization is not a gate decision. Lock the PR-038 reading:
  A11 is the Phase 6 exit review plus G5 readiness pack plus staged
  unpublished `0.0.3` artifacts. Named `G5-D-*` is a separate owner
  action. Do not write `G5-D-*` in the implementation or evidence
  commit.

## Acceptance mapping

Eleven Implementation Plan bullets map 1:1 to A01–A11.

- PR-048-A01: every enumerated crash prefix restores to valid
  `completed`, `failed`, `cancelled`, `suspended`, or retryable
  state, or fails with a documented uncertainty/corruption error
  (TDD §23.4). Default injection is in-process owner drop plus
  `recover` / `recover_run` / `Session::open`. SQLite process-kill
  reuses `sqlite_fault_helper` and is reported separately. Relaxed
  sqlite mode never advertises NFR-REL-001.
- PR-048-A02: migration fixtures cover every released pre-beta
  schema family listed under Locked design §3. Historical journal
  v1 envelopes still decode. sqlite `user_version` 1 still opens.
  A later store version, if introduced, has a one-way migrator and
  a v1→vN fixture. Unknown versions fail closed.
- PR-048-A03: duplicate completion/resolution classification is
  unchanged after restart, snapshot restore, migration, and
  permitted journal pruning. Outstanding callback tokens / unsettled
  targets are never pruned. After the configured horizon, ingress
  tokens are expired and a later command is `expired_locator`
  without a historical-idempotency claim. Indexes already live on
  `KernelState` (`completion_identities`, `resolution_identities`,
  `model_settlements`, `tool_settlements`) and therefore in the
  ADR-032 snapshot envelope. Rebuild them from records when the
  snapshot is discarded. Do not add kernel-state v7.
- PR-048-A04: lineage, interaction, generic deferral, duplicate
  external completion, `before_finalize`, and lane scenarios pass
  restart tests. Equal replay of the interleaved journal yields the
  same mappings, pending work, and terminal/cancel outcomes.
- PR-048-A05: child-invocation crash prefixes before/after
  `ChildRunPrepared` and child `AcceptRun` always attach to one
  mapped UUIDv7 child. Principal/deadline/budget attenuation
  reconstructs from records without process memory.
  `RemoteChildSession` stays vocabulary-only.
- PR-048-A06: budget reservation/charge/release crash prefixes
  reconcile by stable IDs and never double-allocate, double-charge,
  or silently grant on unknown ledger state.
- PR-048-A07: model-activated catalog state survives restart without
  duplicating instructions or rewriting a stable prompt prefix.
  `CapabilitiesActivated` applies only after that record commits.
- PR-048-A08: recorded compaction outcomes replay without rerunning
  summarization. Stale or missing disposable checkpoints rebuild
  from unchanged canonical history.
- PR-048-A09: required compaction projections stored out of line
  survive restart through a verified durable `ArtifactRef`.
  Missing or corrupt required content faults recovery. Only
  explicitly disposable checkpoints may rebuild.
- PR-048-A10: Python and browser adapter surfaces from PR-030 /
  PR-034 / PR-037 pass the same durable restart, migration,
  settlement-idempotency, and interaction/deferred-effect fixtures
  where the target store supports them. Provisional labels remain
  where a target cannot meet the gate. IndexedDB may stay
  `health().durable = false` and `js_indexeddb_experimental` if it
  cannot satisfy NFR-REL-001; it must still pass semantic reload
  fixtures or keep the provisional label honest.
- PR-048-A11: Phase 6 exit review, G5 readiness pack, and staged
  unpublished `0.0.3` artifacts exist. Named `G5-D-*`, checkpoint
  cut, publish, and tag are **not** this PR unless the owner
  separately authorizes them.

## Execution envelope

Authorized 2026-08-14 by `implement the plan`.

Local branch and commit are authorized. Merge to `main`, feature-branch
push, hosted PR/merge, npm/pypi publish, tag, and G5 inference are not
named and remain prohibited. The open PR-043, PR-044, PR-045, PR-046,
and PR-047 envelopes are not reused.

`mode=stacked` (local implementation on the PR-047 review head);
`target` unset; `external actions=none`.

PR-043, PR-044, PR-045, PR-046, and PR-047 remain `In review` and are
not marked `Done`. This PR is the active implementation slice; it does
not close or merge those PRs.

Baseline: `5850a90e957b447731beb6ba3ed28fc01af8eb2d`.
Branch: `codex/pr-048-crash-prefix-durability`.

## Locked design

Do not invent a seventh port, a `SessionStore` port, a `Lane` port,
a `Channel` port, `ConversationId`, a second sequence space,
kernel-state v7, a new journal family, `RemoteChildSession`
dispatch, distributed multi-writer, PostgreSQL, replication,
workflow-engine semantics, SharedArrayBuffer, or exactly-once
external effects. Activate TDD §23.2–23.4, Architecture §10.3–10.5,
TDD §18.2–18.3 / §28.3–28.4, and the existing PR-020 / PR-039–
PR-047 vocabulary.

### 1. Crash means process loss, then recover

TDD §23.4: simulate process loss after every durable append and
before/after every effect result. The restored run must continue
from the next safe boundary, remain explicitly suspended, or fail
with documented uncertainty/corruption.

Default harness (in-process, deterministic, offline):

```text
1. Drive to the named prefix (commit the boundary record).
2. Drop the owner / coordinator / SessionRuntime (no further
   dispatch).
3. Recover with CommitCoordinator::recover / recover_run or
   Session::open on the same store.
4. Assert phase, outstanding effects, settlement/resolution
   classification, child mappings, lane leaves, and the next
   permitted action.
```

Reuse the existing helpers; do not invent a second crash dialect:

- `crates/finstack-ai-test/tests/interaction.rs` `crash_owner`
- `crates/finstack-ai-test/tests/model_port.rs` `crash_before_dispatch`
- `crates/finstack-ai-test/tests/tool_port.rs` `crash_before_tool_dispatch`
- PR-040 `sqlite_fault_helper` for acknowledged sqlite kill only

OS kill and simulated power-loss stay reported separately from
in-process drop (TDD §18.2 / PR-040). Do not treat the flaky
`concurrent_readers_never_observe_a_torn_batch` reader test as an
A01 row.

`CommitCoordinator::recover_run` and `ReplayScope` from PR-047 stay
load-bearing. Conflict reload must keep using `replay_scoped`. A
sibling recover still sets `last_applied_sequence` to the session
head and must not apply another run's `RunAccepted` onto an empty
kernel.

### 2. Enumerated prefix matrix

Generate table-driven tests from this list. Each row names the last
committed record or effect boundary and the legal restored class.
Do not ship a narrative matrix without a failing-then-passing test
per row.

| ID | Boundary | Legal restore |
| --- | --- | --- |
| W1 | after append ACK, before apply | retryable / replay applies |
| W2 | after apply, before snapshot | full replay equals pre-crash |
| W3 | torn or corrupt snapshot | snapshot discarded; journal recovers |
| W4 | after successful snapshot | snapshot-plus-tail equals full replay |
| W5 | after `write_metadata` | metadata never grants authority |
| W6 | after permitted prune | tombstones + outstanding retained |
| D1 | `EffectRequested` committed, not dispatched | retry same identity or documented policy |
| D2 | dispatched, completion uncommitted | reconcile: retryable / still-running / uncertain |
| D3 | provider result before completion commit | same as D2; no second request if completed |
| D4 | `EffectDeferred` committed | `AwaitingExternal`; handle reconstructs |
| D5 | external completion committed | idempotent equal replay |
| D6 | conflicting duplicate after restart | fail closed; audit/reject; run unchanged |
| I1 | interaction requested | `AwaitingInteraction` |
| I2 | interaction resolved | classification unchanged |
| I3 | interaction expired | expired terminal unchanged |
| I4 | cancel while awaiting interaction | cancelled; no fabricated tool success |
| C1 | compaction outcome recorded, summary not durable | rebuild or fault per required/disposable |
| C2 | inline recorded summary | replay without re-summarizing |
| C3 | required `ArtifactRef` persisted | load verifies digest/scope |
| C4 | required artifact missing/corrupt | recovery faults |
| C5 | disposable checkpoint stale/missing | rebuild from canonical history |
| F1 | terminal candidate, before `before_finalize` | stay non-terminal |
| F2 | `before_finalize` decided, terminal uncommitted | retry finalize; no observer mutation |
| F3 | terminal committed | stay terminal |
| L1 | `ChildRunPrepared`, child not accepted | one mapped UUIDv7; retry attaches |
| L2 | child `AcceptRun` committed | same child; no second mapping |
| L3 | parent cancel after mapping | Cascade cancels; detach-preauthorized stays |
| B1 | `BudgetReservationRequested` | no silent grant |
| B2 | `BudgetReservationSettled` | no double-allocate |
| B3 | `BudgetReservationReleased` | release idempotent |
| B4 | charge committed | no double-charge |
| N1 | `LaneCreated` | lane present after `Session::open` |
| N2 | `ConversationEntry` / `LaneMoved` | shared prefix; no copy |
| N3 | sibling `AcceptRun` while other lane active | both reconstruct; busy-lane still holds |
| N4 | cancel fan-out after first child, before second | at-least-once; second cancel idempotent |
| A1 | `CapabilitiesActivated` committed | catalog identical; no duplicate instructions |
| A2 | crash during activation rebuild | apply only the committed activation |

Binding rows (A10) replay a shared subset (D5, D6, I1–I2, N1–N2,
A1) through Python and, where supported, browser storage.

### 3. Released pre-beta schemas (A02)

These are the released pre-beta families. Fixtures must open each
and fail closed on unknown versions.

| Family | Current version | Notes |
| --- | --- | --- |
| Journal envelope / record kind | v1 / 40 families | Do not rewrite known-answers. Family count stays **40** unless a new `RecordBody` is unavoidable; a new family is an ADR-trigger stop. |
| `kernel-state` | v1–v6 | Do not rewrite v1–v6 hashes. No v7. |
| Snapshot envelope | `format_version` 1 | ADR-032 direct state-CBOR. Disposable. |
| sqlite store | `PRAGMA user_version` 1 | Open as-is. Bump only with one-way migrator + fixture. |
| IndexedDB host schema | `INDEXED_DB_SCHEMA_VERSION` 1 | Experimental until A10 says otherwise. |
| Public-rust-api corpus | 131 | Add `pr048-*` subjects only for new public types. |

Do not run `write_journal_v1_fixtures()` without reverting existing
envelopes.

### 4. Settlement index and prune — no second source of truth

TDD §23.3 / Architecture §10.3 already place accepted command
identity and digest on records and on `KernelState`. Snapshots
already encode that `KernelState`. PR-048 proves the rule under
restart, snapshot discard, migration, and prune.

```text
Do:
  - rebuild completion/resolution/model/tool indexes from records
    when the snapshot is absent or invalid
  - keep outstanding targets and their tokens
  - retain terminal settlement / rejection tombstones through the
    application-configured horizon
  - expire post-horizon ingress with expired_locator
  - add optional JournalStore::prune; default prune_unsupported

Do not:
  - add kernel-state v7 fields
  - add a second snapshot DTO
  - persist the host ExternalIdentityMap (PR-047; still host-owned)
  - prune by globally scanning effect IDs
```

Horizon is runtime/application config, not a journal authority
field. Equal bind of horizon config is the caller's problem; the
store enforces "outstanding cannot be pruned" from committed
outstanding-effect / pending-interaction / callback-token state.

Preferred prune shape (extend the existing port; default remains
unsupported):

```rust
pub struct PruneRequest {
    pub session_id: SessionId,
    pub horizon: IdempotencyHorizon,
}

pub struct PruneReceipt {
    pub pruned_through_sequence: u64,
    pub retained_outstanding: u64,
    pub retained_tombstones: u64,
}

impl JournalStore {
    fn prune(&self, _request: PruneRequest)
        -> PortFuture<Result<PruneReceipt, StoreError>>
    {
        // default: InvalidRequest { reason_code: "prune_unsupported" }
    }
}
```

Implement prune on `MemoryJournalStore` and
`SqliteJournalStore`. IndexedDB implements it only if A10 can
prove the same tombstone rule; otherwise leave `prune_unsupported`
and keep the provisional label.

If sqlite needs extra tombstone rows, bump `user_version` to 2 with
a one-way migrator and a v1-file fixture. Prefer retaining existing
`EffectCompleted` / `EffectFailed` / `EffectCancelled` /
`InteractionSettled` / `ExternalCommandRejected` envelopes so no
new `RecordBody` is required.

### 5. Operational tooling is a leaf, not a port

PR-048 asks for migration tooling, backup/restore commands,
corruption reports, and recovery diagnostics. These are leaf
commands over existing APIs:

```text
scan / load / append / write_snapshot / health / prune
protocol::to_diagnostic_jsonl / from_diagnostic_json
verify_chain / decode_snapshot
sqlite file + WAL + SHM copy (already in the crate README)
```

Put sqlite-facing commands next to the store, for example
`extensions/stores/finstack-ai-store-sqlite/src/bin/sqlite_ops.rs`.
Do not add a Cargo `xtask`. Do not add a seventh port. Do not put
backup/migrate on `finstack-ai-kernel`.

Minimum command set:

```text
backup   copy db + wal + shm while the writer is quiet
restore  restore that trio; refuse a partial set
export   JSONL diagnostic projection of one session
import   JSONL → append onto an empty session (support/migrate)
diagnose sequence head, checksum, snapshot validity,
         outstanding effects/interactions, child mappings,
         corruption class
migrate  user_version 0/1 → current; unknown fails closed
```

`sqlite_fault_helper` stays a test helper, not a product command.

### 6. Compaction and artifacts

PR-018 / PR-023 already record outcomes and reject history
mutation. This PR owns end-to-end restart:

- Replay uses the recorded outcome. Do not call the summarizer
  again on a valid recorded result.
- `CompactedSummary::Artifact(ArtifactRef)` is the required
  out-of-line path. Recovery loads the artifact, checks digest and
  `ArtifactScope`, and faults on missing/corrupt required content
  (`artifact_not_found` / `artifact_integrity_failure`).
- `CompactedSummary::Inline` stays valid for small recorded
  summaries.
- Disposable checkpoints may rebuild. Required projections may not.

Do not add an eighth middleware stage. Do not treat compaction as
journal deletion.

### 7. Capability activation checkpoints

`CapabilitiesActivated` is already a journal family. Activation is
applied only after that record commits (A1). A crash during
rebuild (A2) must not emit a second activation, must not duplicate
instructions, and must not rewrite a stable prompt prefix. Reuse
PR-022 / PR-032 / PR-038 catalog rebuild; add crash-prefix fixtures
only.

### 8. Bindings (A10)

Rust remains the semantic owner. Python and JS replay shared
fixtures; they do not reimplement recovery.

- Python: durable restart through the live `Session` handle and a
  store the binding can actually host (scripted/memory always;
  sqlite when the extension is imported). Interaction list/resolve
  and duplicate external completion must match Rust classification
  after process restart.
- JS/WASM: page/worker reload against IndexedDB is the browser
  restart. Keep `health().durable = false` unless the adapter
  meets WAL+FULL-equivalent acknowledgements (it does not today).
  Honest provisional label beats a false durable claim.
- Shared traces: prefer one Rust-owned fixture list consumed by
  Python pytest and JS Playwright. Do not fork semantics.

Pre-1.0 lockstep: if a public recovery/diagnose/prune name is
exported in one binding, export it in the others in the same PR or
record an explicit tracked deferral.

### 9. 0.0.3 staging and G5 readiness (A11)

Follow PR-038, not a silent gate:

- Workspace crate/Python/JS versions may move to `0.0.3` in the
  implementation candidate so staged artifacts match the
  checkpoint name. That is not a cut and not a publish.
- Stage unpublished artifacts (wheels / npm tarball / checksums /
  SBOM) under `docs/implementation/artifacts/pr-048/`.
- Write `phase6-exit-review.txt` and `g5-readiness-review.txt`.
  Result language: `READY FOR NAMED DECISION`; do not write
  `G5-D-*`.
- Operational guidance: sqlite backup/restore, prune/horizon,
  corruption classes, IndexedDB provisional label, at-least-once
  statement.
- Benchmarks: restore vs full replay and storage growth. Warning-
  only unless a named budget already exists. Not PR-063.

A named G5 decision against `main` also needs PR-043–PR-047
integrated. This stacked envelope does not perform that
integration.

## Restore / durability matrix (default MemoryJournalStore)

SQLite is required for A01 store rows, A02 store migration, A03
prune, and Phase 6 exit 1. IndexedDB is A10 only.

| Row | Scenario | After |
| --- | --- | --- |
| 1 Prefix matrix | every ID in §2 | legal class only (A01) |
| 2 Snapshot discard | delete snapshots; recover | same hashes and settlement maps (A03) |
| 3 Snapshot corrupt | flip bytes; recover | snapshot ignored (A03) |
| 4 Prune + tombstone | prune past settled work inside horizon | outstanding kept; duplicate still idempotent (A03) |
| 5 Post-horizon | expire token; late command | `expired_locator`; no historical-idempotency claim (A03) |
| 6 sqlite v1 file | open historical user_version 1 | equal replay or explicit one-way migrate (A02) |
| 7 Journal v1 | historical 40-family fixtures | decode; count stays 40 (A02) |
| 8 Lineage restart | prepared + accepted child; drop | one mapping; attenuation present (A04/A05) |
| 9 Interaction restart | approval pending; drop; resolve | same classification (A04) |
| 10 Deferral restart | `AwaitingExternal`; drop; complete | idempotent equal; conflict fails (A04) |
| 11 Finalize restart | F1–F3 | no observer mutation (A04) |
| 12 Lane restart | N1–N4 on sqlite | leaves and busy-lane reconstruct (A04) |
| 13 Budget | B1–B4 | no double-allocate/charge/silent grant (A06) |
| 14 Activation | A1–A2 | no duplicate instructions (A07) |
| 15 Compaction | C1–C5 | no re-summarize; required artifact faults (A08/A09) |
| 16 Python | shared subset | same classification (A10) |
| 17 Browser | IndexedDB reload or provisional | honest label (A10) |

## Implementation pitfalls

1. Do not infer G5, Phase 6 `Done`, or ADR Implemented.
2. Do not add kernel-state v7 or a second snapshot DTO.
3. Do not add a journal family or rewrite v1–v6 hashes / journal
   known-answers.
4. Do not add a seventh port or put backup/migrate on the kernel.
5. `last_applied_sequence` stays the session head. Keep PR-047
   `recover_run` / `ReplayScope` / scoped conflict reload.
6. One kernel per run. Foreign-run skip stays as PR-047 refined it.
7. Cancel fan-out remains at-least-once plus idempotent
   `CancelRequested`. No exactly-once claim.
8. Do not `ExecuteEffect` a conversation entry, `LaneMoved`, or
   compaction summary.
9. Do not treat `LaneId` as lineage.
10. Outstanding tokens cannot be pruned. Post-horizon is expire,
    not silent drop.
11. IndexedDB `health().durable = true` is a false claim today.
12. Do not require `mise run ci`, full pytest, or every Playwright
    engine to close A01–A09. A10 needs the binding fixtures it
    claims.
13. Isolate `concurrent_readers_never_observe_a_torn_batch` if it
    flakes; it is not an A01 row.
14. Do not mark PR-043–PR-047 `Done`.
15. Do not admit while PR-047 is the sole active logical PR.

## Files

Create at this planning step:

- `docs/implementation/artifacts/pr-048/plan.md` (this file)

Create later (implementation / candidate, not this planning step):

- `docs/implementation/artifacts/pr-048/README.md`
- `docs/implementation/artifacts/pr-048/candidate-validation.txt`
- `docs/implementation/artifacts/pr-048/security-review.txt`
- `docs/implementation/artifacts/pr-048/phase6-exit-review.txt`
- `docs/implementation/artifacts/pr-048/g5-readiness-review.txt`
- staged unpublished `0.0.3` checksums / SBOM under the same tree

Create when implementing:

- `crates/finstack-ai-test/src/crash_prefix.rs` — shared prefix
  driver (drop + recover + legal-class assert)
- `crates/finstack-ai-test/tests/crash_prefix.rs` — A01–A09 matrix
- `crates/finstack-ai-runtime` prune types on the existing journal
  port (`journal.rs`)
- `extensions/stores/finstack-ai-store-sqlite/src/bin/sqlite_ops.rs`
  — backup / restore / export / import / diagnose / migrate
- migration / prune / horizon fixtures under
  `fixtures/compatibility/` (`pr048-*` subjects)
- `bindings/finstack-ai-python/tests/test_durable_restart.py`
- `bindings/finstack-ai-wasm/js/src/durable-restart.test.ts` —
  IndexedDB reload; keep the experimental label honest

Modify when implementing:

- `crates/finstack-ai-runtime/src/journal.rs` — optional `prune`
- `extensions/stores/finstack-ai-store-memory/src/lib.rs` — prune
  + tombstone retain
- `crates/finstack-ai-runtime/src/ingress.rs` — post-horizon
  `expired_locator`
- `crates/finstack-ai-runtime/src/coordinator.rs` — do not change
  `recover_run` / `ReplayScope` unless a matrix row proves it
  wrong; diagnose reads `LoadedSession` and `scan`
- `crates/finstack-ai-runtime/src/middleware.rs` / `artifact.rs` —
  required `ArtifactRef` fault on recover
- `extensions/stores/finstack-ai-store-sqlite/src/lib.rs` — prune,
  diagnose helpers, migrator if `user_version` changes
- `bindings/finstack-ai-python` — restart fixtures; prune/diagnose
  only if exported
- `bindings/finstack-ai-wasm/js/src/adapters/indexeddb.ts` — A10
  reload / migration honesty
- `docs/implementation/delivery-ledger.md` — admit/tracking rows
  only at admit

Do not modify: `docs/planning/`, WIT, UI, cron/scheduler,
`RemoteChildSession` dispatch, SharedArrayBuffer, kernel-state
hash fixtures for v1–v6, existing journal envelope known-answers,
`LaneMoved` / `EntryAppended` / `ConversationEntry` wire shapes,
PR-049+ plugin work.

## Proposed tasks

Create ledger rows only at admit. Summaries are
implementation-specific.

1. Tracking — confirm PR-032/PR-038/PR-039–PR-047, Phase 6
   entrance, ADR-004/008/012/013/016/020/026/027/034/037, TM-12
   and the G5 security row, exclusions, no branch until PR-047 is
   no longer the sole active PR.
2. Crash-prefix harness + A01 matrix — table-driven drop/recover
   for every §2 row; sqlite kill reported separately; no new port.
3. Settlement index, horizon, prune — A03; optional
   `JournalStore::prune`; tombstones through horizon;
   `expired_locator` after; snapshot discard still classifies.
4. Semantic restart cluster — A04/A05/A06 lineage, interaction,
   deferral, duplicate completion, `before_finalize`, lanes,
   child UUIDv7 attach, budget IDs.
5. Activation + compaction recovery — A07/A08/A09; activation only
   at committed checkpoints; recorded outcomes replay; required
   `ArtifactRef` faults; disposable checkpoints rebuild.
6. Migration, backup/restore, diagnostics — A02; JSONL
   export/import; sqlite copy backup; corruption classes;
   historical schema fixtures.
7. Binding durability traces — A10; Python plus browser-where-
   supported; provisional labels stay honest.
8. Ops guidance, benches, `0.0.3` staging, G5 readiness — A11;
   Phase 6 exit review; stop before named G5.

## Validation (when authorized)

Focused:

```text
cargo test -p finstack-ai-test --test crash_prefix --offline --locked
cargo test -p finstack-ai-test --test snapshot_replay --offline --locked
cargo test -p finstack-ai-test --test conversation --offline --locked
cargo test -p finstack-ai-test --test lanes --offline --locked
cargo test -p finstack-ai-test --test interaction --offline --locked
cargo test -p finstack-ai-test --test journal_v1 --offline --locked
cargo test -p finstack-ai-runtime --features native-tokio --offline --locked
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

A10 adds the focused Python restart module and the IndexedDB reload
(or documented provisional) fixture. Do not require `mise run ci`
or the full Playwright matrix to close A01–A09. Do not infer G5.

## Explicit exclusions

No seventh port. No `SessionStore` / `Lane` / `Channel` port. No
kernel-state v7. No new journal family. No rewrite of v1–v6 hashes
or `LaneMoved` / `EntryAppended` / `ConversationEntry` wire
shapes. No PostgreSQL, replication, or workflow-engine semantics.
No distributed multi-writer. No `RemoteChildSession` dispatch. No
exactly-once external-effect claim. No SharedArrayBuffer. No
compact snapshot DTO. No WIT/Wasmtime (PR-049+). No silent
ADR Implemented. No Phase 6 `Done`. No inferred G5, checkpoint
cut, publish, or tag. Do not reuse the PR-043, PR-044, PR-045,
PR-046, or PR-047 envelope. Do not admit while PR-047 is the sole
active logical PR. Do not mark PR-043, PR-044, PR-045, PR-046, or
PR-047 `Done`.
