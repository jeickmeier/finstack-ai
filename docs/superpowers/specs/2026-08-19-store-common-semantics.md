# Spec: Shared Journal-Store Semantics Crate (`finstack-ai-store-common`)

## Problem

`finstack-ai-store-memory` (extensions/stores/finstack-ai-store-memory/src/lib.rs, ~1,240 LOC)
and `finstack-ai-store-sqlite` (extensions/stores/finstack-ai-store-sqlite, ~2,100 LOC)
independently reimplement the same `JournalStore` contract semantics. The
following logic is duplicated — several functions verbatim:

1. **Append admission** — batch-id idempotent replay vs `append_batch_id_reuse`,
   empty-batch rejection, record-id reuse classification
   (`mixed_record_id_reuse` / `mixed_record_batch_reuse` / `record_id_reuse`),
   optimistic sequence check (`Conflict`, `sequence_exhausted`), and
   `StoreLimits` enforcement ordering (sessions → batches → records).
2. **Batch commitment** — `build_committed_batch` (verbatim duplicate).
3. **Snapshot semantics** — `encode_state_request`, `accelerated_from`,
   `outstanding_count`, `tombstone_count` (all verbatim duplicates), plus
   snapshot-write admission (`snapshot_bytes` limit, `snapshot_ahead_of_journal`,
   `snapshot_sequence_regression`) and prune admission
   (`prune_snapshot_not_aligned`) with receipt counting.
4. **Chain-verification** — full-session head verification
   (memory `verify_session` vs sqlite `verify_stored_session`) and tail-window
   verification (memory `loaded_from_batches` vs sqlite `loaded_tail`) sharing
   the gap/split/checksum reason-code triples for `FromSequence` and
   `SnapshotPlusTail` windows.
5. **Scan validation** — `scan_limit_zero`, `scan_limit_exceeded`, start
   normalization, `next_sequence` computation.
6. **Error mapping** — `ProtocolError → StoreError` (duplicate `protocol_error`).

A third backend (Turso/Postgres) would mean a third reimplementation and drift
bugs in exactly this logic.

## Goal

Extract the duplicated *semantic* logic (pure functions over
kernel/protocol/runtime port types — no SQL, no storage plumbing) into a new
crate `finstack-ai-store-common` at `extensions/stores/finstack-ai-store-common`,
and rewire both stores through it. The conformance suites in
`finstack-ai-test` plus each store's existing tests remain the enforcement
mechanism and must pass unchanged (with one deliberate exception below).

## Non-goals

- No generic SQL abstraction, query builder, or shared connection handling.
  `worker.rs`, all SQL, and row↔envelope mapping stay in the sqlite crate.
- No change to the `JournalStore` port or its default impls in
  `finstack-ai-runtime` (runtime cannot depend on `finstack-ai-protocol`).
- No new backend.
- No public API change to either store crate (`MemoryJournalStore`,
  `SqliteJournalStore` signatures unchanged); Python/WASM bindings untouched.

## Behavioral invariants

- Byte-stable idempotency identity: sqlite persists `request_cbor` (CBOR of
  `AppendIdentity`) in the `batches` table and compares it on replay. The
  extracted `AppendIdentity` must encode to identical bytes (same struct shape,
  same field order, same serde derives). Pinned by a golden test written
  against the current code *before* the move.
- Error reason codes and error-check ordering are observable behavior
  (pinned by existing store tests, e.g.
  `batch_and_record_idempotency_precede_sequence_checks`) and must not change,
  **except** two deliberate unifications of tail-window loads
  (`LoadWindow::FromSequence` / `SnapshotPlusTail`), where the stores have
  already drifted (neither behavior is covered by any existing test):
  1. *Hole at the start*: the first available record's sequence is greater
     than the requested start and the start does not fall inside any stored
     batch. Memory today reports the `split` code
     (`load_from_splits_batch` / `snapshot_splits_batch`); sqlite reports the
     `gap` code. Unify to the `gap` code (the record is missing, not a split
     batch). Both are `StoreError::Integrity`, so `StoreError::code()` is
     unchanged.
  2. *Mid-batch start*: the requested start falls strictly inside a stored
     batch (the record at `start - 1` belongs to the same batch). Memory
     errors with the `split` code — except when the start falls inside the
     *last* committed batch, where memory's `position()` search found no
     batch and fell through to the `gap` code; that sub-case also unifies to
     `split`. Sqlite today silently returns a
     reconstructed batch that splits the stored one, violating the port
     contract ("`from_sequence` must land on a batch boundary") and the
     runtime default impl. Unify to the `split` error; this fixes a sqlite
     bug found during this analysis (`loaded_tail`'s split checks are dead
     code because records are fetched with `WHERE sequence >= start`).

## Acceptance

- `cargo test -p finstack-ai-store-common -p finstack-ai-store-memory -p finstack-ai-store-sqlite -p finstack-ai-test` passes.
- `cargo clippy --workspace --all-targets` is clean.
- Neither store crate contains a copy of the extracted functions.
- The extracted pure functions have direct unit tests in
  `finstack-ai-store-common` (this is the new drift-prevention layer: the
  semantics are tested once, as data-in/data-out).
