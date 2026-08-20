# Postgres journal store — code-review fix batch

Branch `feature/postgres-journal`, applied on top of `320dfdc`. Only files
under `extensions/stores/finstack-ai-store-postgres/` were touched;
store-common, sqlite, and memory are untouched, as instructed.

Finding 6 (verified-head cache extraction to store-common) was explicitly out
of scope and is not addressed here.

---

## Fix 1 — bounded `health()` probe

**Files:** `src/journal_store.rs`

The `SELECT 1` probe issued after a successful checkout was unbounded. A
black-holed connection (network path dropped without an RST, wedged server)
never errors — it simply never answers — so `health()` could hang forever even
though the checkout half was already bounded.

The probe is now wrapped in the same
`tokio::time::timeout(checkout_bound, …)` used for the checkout, where
`checkout_bound` is `PostgresStoreConfig::connect_timeout`. A timeout is
treated identically to a probe error: `ready: false`, and the connection is
`discard()`ed rather than returned to the idle queue — it may still deliver its
`SELECT 1` reply later, so it must never be handed to another caller (spec D2).

The doc comment, which previously said only the checkout was bounded, now
documents both halves and why each needs its own bound.

**Test:** no serverless test is feasible for this arm — reproducing a
black-holed backend needs either a real network fault or a proxy that accepts
and never answers, neither of which the crate's harness has. The existing
serverless `pool::tests::get_times_out_on_an_exhausted_pool` still covers the
checkout half. This fix therefore rests on compile + the existing server-gated
`tests/health.rs` suite (3 tests, green), which proves the happy path did not
regress.

---

## Fix 2 — reject a zero `connect_timeout`

**Files:** `src/config.rs`

`PostgresStoreConfig::validate` gained a guard mirroring sqlite's
`zero_sqlite_busy_timeout` (`extensions/stores/finstack-ai-store-sqlite/src/store.rs`):

```rust
if self.connect_timeout.is_zero() {
    return Err(StoreError::InvalidRequest {
        reason_code: "zero_postgres_connect_timeout",
    });
}
```

It runs after the `pool_size` guard and before `limits.validate()`, so the
existing reason-code ordering is unchanged. The field's doc comment now states
the constraint and why it matters (a zero timeout expires before the connect
future is first polled, so every open would fail as `postgres_unavailable` and
`health()` would report `ready: false` forever).

**Test:** `config::tests::zero_connect_timeout_is_rejected` asserts the exact
reason code.

---

## Fix 3 — scan gap detection at page start (fail-closed)

**Files:** `src/snapshot.rs`, `src/load.rs`

### Prune-boundary semantics (confirmed against `src/prune.rs`)

`prune` deletes `records WHERE sequence < snapshot_sequence` — strictly less
than. So the record *at* `snapshot_sequence` is retained, and **every sequence
`>= snapshot_sequence` must still be stored**. A session whose
`snapshot_sequence` is `NULL` has never been prunable at all, so nothing is
covered. That rule is captured once, in a helper:

```rust
fn pruned_prefix_covers(session: &SessionRow, sequence: u64) -> bool {
    session.snapshot_sequence.is_some_and(|boundary| sequence < boundary)
}
```

Note the comparison: strictly `<`, not `<=` as the brief's first sketch had it.
Because the deletion predicate is strict, the record at the boundary itself
must exist, so a hole *at* `snapshot_sequence` is a genuine gap.

`SessionRow::snapshot_sequence` was widened from private to `pub(crate)` (with
a doc comment naming it the prune boundary) so `snapshot.rs` can read it. It is
already fetched by `load_session_row`, so no extra round trip was added.

### (a) Missing record at the requested start

The old check was dead: the SQL asks for `sequence >= start`, so
`records.first().sequence() < start` can never hold. It is replaced by the
condition that actually detects the hole — `first.sequence() > start` — gated on
`!pruned_prefix_covers(&session, start)`. A gap inside the pruned prefix keeps
the previous (permissive) behavior.

### (b) Missing checkpoint row before the page

`verify_scan_page`'s checkpoint arm previously fell back to the record's own
`previous_checksum()` whenever `load_envelope_checksum(first.sequence() - 1)`
returned `None` — i.e. it anchored the page on a value nothing corroborates.
Now that fallback is taken only when
`pruned_prefix_covers(session, first.sequence() - 1)`; otherwise the scan fails
with `Integrity{scan_checkpoint_mismatch}`.

### Design choice: deliberately stricter than sqlite

sqlite masks both cases. That divergence is intentional and is documented at
`scan`'s doc comment, at `verify_scan_page`, and inline at both check sites:
failing closed on a hole in a live journal is the entire point of chain
verification, and the postgres store has the `snapshot_sequence` column right
there in the row it already read, so it can distinguish "pruned" from "missing"
without extra I/O. No new reason codes were introduced — both outcomes reuse the
existing `scan_sequence_gap` / `scan_checkpoint_mismatch` codes.

**Tests (both server-gated):**

- `tests/snapshot_scan_metadata.rs::scan_from_a_deleted_sequence_is_a_gap` —
  journals 1..=8 in four batches, deletes the row at sequence 4 with raw SQL,
  scans from 4, asserts `Integrity{scan_sequence_gap}`. (Under the old code this
  scan returned `Ok`: the dead check did not fire, and the checkpoint arm's
  fallback let the 5..8 page verify cleanly against the head.)
- `tests/prune.rs::scanning_a_pruned_prefix_still_succeeds` — the twin, opposite
  verdict: after a *real* aligned prune (`pruned_through_sequence == 2`), a scan
  from sequence 1 still succeeds and returns `[2, 3]`. Both the record at 1 and
  its checkpoint row are gone, but both absences sit strictly below the boundary.

---

## Fix 4 — per-connection prepared-statement cache

**Files:** `src/lib.rs`, `src/pool.rs`, `src/session.rs`, `src/load.rs`,
`src/append.rs`, `src/snapshot.rs`, `src/prune.rs`

### Where the cache lives

The brief suggested hanging the cache off `PooledClient`. That alone would have
been wrong: a `PooledClient` is created at checkout and dropped on return, so
the cache would have died every operation and prepared everything afresh. Since
Postgres prepared statements are **session-scoped**, the cache must live with
the *physical connection*.

So the pool's idle queue changed from `Vec<C>` to `Vec<Entry<C>>`, where

```rust
type StatementCache = HashMap<&'static str, Statement>;
struct Entry<C> { client: C, statements: StatementCache }
```

`PooledClient` carries the `StatementCache` while checked out and hands it back
to the `Entry` in `Drop`. A discarded/poisoned checkout drops the cache with the
connection — exactly right, since the server-side statements went away with the
session.

Keyed by the `&'static str` SQL literal rather than by its address: the address
trick relies on the linker not deduplicating equal literals, and hashing ~200
bytes of SQL is free relative to a round trip.

### Preparing on the client, before the transaction

`PooledClient::prepared(&mut self, sql: &'static str)` prepares on the
**client**, never inside a transaction: a `Parse` issued inside a transaction is
rolled back with it, which would leave the cache holding handles the server no
longer knows about. Because a `Transaction` runs on the same connection/session,
client-prepared handles are directly usable inside one.

`Client::transaction()` borrows the client mutably for the transaction's
lifetime, so the cache is unreachable once the transaction is open. Every op
therefore prepares its statements **up front, at the op entry point**, into a
small per-op bundle struct that is threaded down by reference:

| op | bundle | statements |
|---|---|---|
| `append` | `AppendStatements` | 7 |
| `load`/`load_from` | `LoadStatements` | 4 |
| `scan` | `ScanStatements` | 3 |
| `write_snapshot` | `SnapshotStatements` | 3 |
| `write_metadata` | `MetadataStatements` | 2 |
| `prune` | `PruneStatements` | 6 |

This is the least-churn shape that compiles cleanly: the alternative (moving the
cache out of the client borrow so it can be consulted mid-transaction) is
blocked by the rollback-discards-Parse problem above, not just by the borrow
checker.

### SQL literals

`load_records_from`, `load_batch`, and `scan` previously built their SQL with
`format!("SELECT {RECORD_COLUMNS} …")`, which yields a `String` and therefore
cannot be a `&'static str` cache key. `RECORD_COLUMNS` was converted into a
`record_columns!()` `macro_rules!` in `src/lib.rs` (declared before the `mod`
declarations, so textual macro scoping makes it visible crate-wide), and the
three statements are assembled with `concat!` into `&'static str` consts. This
also removes three runtime allocations per load/scan.

### Sites deliberately left direct

The three **session-creation** statements in `append.rs` (`store_totals`
`FOR UPDATE`, the `INSERT … ON CONFLICT DO NOTHING`, the session-count bump) run
at most once per session in the store's lifetime. Preparing them on every
connection would cost more round trips than it ever saves, so they remain inline
SQL. This is documented on `AppendStatements`. Everything else on the hot paths
is cached. There are no dynamic-SQL sites left.

`ensure_schema` (DDL, run once at open over a shared `&Client`) is untouched.

**Test:** `pool::tests::statements_are_cached_per_connection_and_usable_in_transactions`
(server-gated) proves all three properties: (1) a client-prepared statement runs
correctly inside a `Transaction` on the same connection, (2) the cache travels
back to the idle queue with the connection — the second checkout finds it already
populated and re-`prepared()` adds nothing — and (3) a `discard()`ed connection's
replacement starts with an empty cache. A `#[cfg(test)]`-only
`cached_statement_count()` accessor makes the cache observable.

Behavior is otherwise identical: all 44 existing crate tests plus the three
`finstack-ai-test` postgres rows stay green unchanged.

---

## Fix 5 — envelope clones in `load`

**Files:** `src/load.rs`

**One of the two clones was eliminated; the other could not be, and here is
why.**

`verify_full_head` and `verify_tail_records` in
`extensions/stores/finstack-ai-store-common/src/window.rs` both take
`&[RecordEnvelope]`. Changing those signatures is out of scope, and a contiguous
owned slice of envelopes has to exist somewhere for them to borrow — the stored
rows are `StoredRecord { batch_id, envelope }`, not bare envelopes. So the
`envelopes()` clone stays.

The **`group_batches` clone is gone**. It now takes `Vec<StoredRecord>` by value
and *moves* each envelope into the batch it belongs to, accumulating groups in a
single `into_iter()` pass instead of the previous `split_at` + `iter().cloned()`
walk. `loaded_session` correspondingly takes `Vec<StoredRecord>`.

Both call sites (`load_session` and `loaded_tail` — they do share the shape, and
both were fixed) now scope the verification copy in a block so it is **dropped
before** `loaded_session` moves the originals:

```rust
let head_checksum = {
    let records = envelopes(&stored);
    verify_against_cache(&records, &session, cached)?
};
```

Net effect: one owned copy per record alive at a time instead of two, and one
clone performed per record instead of two.

A small `committed_batch()` helper was extracted so the group-flush logic is not
duplicated between the loop body and the final flush.

**Tests:** `tests/load.rs` (9), `tests/severed_commit.rs` (3),
`tests/prune.rs` (4), and the `finstack-ai-test` byte-equality corpus
(`postgres_round_trips_every_journal_v1_record_body_byte_equal`, all 40 families
compared on payload CBOR, payload digest, envelope checksum, and envelope CBOR)
all pass unchanged.

---

## Fix 7 — shared commit/poison scaffolding

**Files:** `src/error.rs` (helpers), `src/append.rs`, `src/prune.rs`,
`src/snapshot.rs`, `src/load.rs`

Two `pub(crate)` helpers in `src/error.rs`:

```rust
pub(crate) fn settle<T, C>(
    outcome: Result<T, Failure>,
    client: &mut PooledClient<C>,
) -> Result<T, StoreError>

pub(crate) async fn commit_or_ambiguous(
    transaction: tokio_postgres::Transaction<'_>,
) -> Result<(), Failure>
```

`settle` is generic over the pooled connection type, matching `PooledClient`'s
own bound, and replaces all **6** copies of the
`if failure.poison { client.poison(); } Err(failure.error)` unwrap (`append`,
`prune`, `write_snapshot`, `write_metadata`, `scan`, `load`).

`commit_or_ambiguous` implements the spec D5 rule and replaces the **4**
write-path copies (`append`, `prune`, `write_snapshot`, `write_metadata`). The
**2 read paths** (`load_on_connection`, `scan_on_connection`) keep their plain
`Failure::from_driver` commit mapping — they intentionally lack the ambiguity
arm, because nothing was written and there is no durability outcome to be
ambiguous about. That reasoning is now written down on
`commit_or_ambiguous`.

No generic async-closure driver was attempted, per the brief. Reason codes and
poison decisions are byte-identical; `tests/severed_commit.rs` (which is the
direct test of the D5 arm) passes unchanged, run twice.

---

## Fix 8 — retry doc accuracy

**Files:** `src/append.rs` (module doc + `append` doc + the `COMMIT` comment),
`src/error.rs` (`DEADLOCK_DETECTED` doc), `src/journal_store.rs`
(`JournalStore::append` doc)

Verified against `crates/finstack-ai-runtime/src/exec/coordinator/submit.rs`:
`CommitCoordinator` retries `Conflict` (once, after a reload) and
`AmbiguousAcknowledgement` (once, by re-appending); every other `StoreError`,
`Unavailable{postgres_serialization}` included, falls through to
`CommitCoordinatorError::Store(error)` and propagates as a hard error — the same
treatment sqlite's `sqlite_busy` gets.

Every "the caller retries" phrasing attached to `postgres_serialization` was
reworded: the store reports these as *transient* `Unavailable` errors, no caller
in this repo retries them, and retry policy is the embedding application's
decision (retrying the same request remains safe under the idempotency
contract). The `AmbiguousAcknowledgement` retry language was left in place — that
one is accurate — but reattributed from "the caller" to "the embedding
application" where it read ambiguously. `grep -n "retr" src/*.rs` shows no
remaining incorrect claim.

---

## Verification

All run from the worktree; exit codes checked directly.

| command | result |
|---|---|
| `cargo test -p finstack-ai-store-postgres` (no env var) | ok — 8 suites, all `test result: ok` |
| `FINSTACK_PG_TEST_URL=… cargo test -p finstack-ai-store-postgres` | ok — 45 + 12 + 3 + 9 + 4 + 3 + 9 passed, 0 failed |
| `FINSTACK_PG_TEST_URL=… cargo test -p finstack-ai-test --test postgres_journal` | ok — 3 passed |
| `cargo clippy -p finstack-ai-store-postgres --all-targets` | clean, 0 warnings |
| `cargo check --workspace` | clean |
| `mise run check-public-api` | exit 0, no baseline churn |
| `cargo test … --test severed_commit` (×2) | ok — 3 passed both runs |

Net test count: +4 (one config unit test, one pool statement-cache test, two
scan gap/prune-boundary integration tests).
