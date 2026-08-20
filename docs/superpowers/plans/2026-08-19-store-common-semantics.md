# Shared Journal-Store Semantics (`finstack-ai-store-common`) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract the journal-store semantics duplicated between `finstack-ai-store-memory` and `finstack-ai-store-sqlite` into a new pure-function crate `finstack-ai-store-common`, and rewire both stores through it.

**Architecture:** A new leaf crate at `extensions/stores/finstack-ai-store-common` holds pure functions over `finstack-ai-kernel` / `finstack-ai-protocol` / `finstack-ai-runtime` port types: append admission, batch commitment, snapshot/prune admission, chain-verification of full and tail windows, and scan validation. The stores keep all storage plumbing (memory's `BTreeMap` state, sqlite's worker thread, SQL, and row↔envelope mapping) and delegate every semantic decision to the shared crate. Each store's existing tests plus the `finstack-ai-test` conformance suites are the regression net.

**Tech Stack:** Rust workspace crate; deps `finstack-ai-kernel`, `finstack-ai-protocol`, `finstack-ai-runtime`, `serde`. No rusqlite, no async runtime.

**Spec:** `docs/superpowers/specs/2026-08-19-store-common-semantics.md`

## Global Constraints

- Workspace lints apply: `#![forbid(unsafe_code)]`, deny `clippy::unwrap_used` / `expect_used` / `panic` / `unreachable`, warn `missing_docs` — copy the lint header from `extensions/stores/finstack-ai-store-memory/src/lib.rs:6-25` into the new crate, including the `#![cfg_attr(test, allow(...))]` block.
- `publish = false` and workspace-inherited package fields, matching the other two store crates.
- The CBOR encoding of `AppendIdentity` is a persisted format (sqlite `batches.request_cbor` column). It must be byte-identical before and after the move (Task 2 pins it).
- Error reason codes and check ordering are frozen, except the two tail-window unifications listed in the spec ("Behavioral invariants").
- No public API change to `MemoryJournalStore` or `SqliteJournalStore`; no changes under `bindings/` or `crates/finstack-ai-runtime`.
- Repo commit style: plain imperative sentence, no `feat:`-style prefixes (see `git log`). End every commit message with `Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>`.
- Run commands from the repo root `/Users/jeickmeier/Projects/finstack-ai`.

## File Structure

| File | Responsibility |
|---|---|
| `extensions/stores/finstack-ai-store-common/Cargo.toml` | Crate manifest |
| `extensions/stores/finstack-ai-store-common/src/lib.rs` | Lint header, crate docs, module re-exports |
| `.../src/error.rs` | `protocol_error` mapping |
| `.../src/append.rs` | `AppendIdentity`, reuse classification, sequence check, limits admission, `build_committed_batch` |
| `.../src/snapshot.rs` | Snapshot encode/decode helpers, snapshot & prune admission, prune counts |
| `.../src/window.rs` | `WindowCodes`, tail selection/alignment/verification, full-head verification |
| `.../src/scan.rs` | Scan request validation and paging helpers |
| `.../src/test_support.rs` | `#[cfg(test)]` fixtures (`id`, `draft`, `request`) |
| Modify: `Cargo.toml` (workspace root) | Register member + workspace dep |
| Modify: `extensions/stores/finstack-ai-store-memory/{Cargo.toml,src/lib.rs}` | Rewire memory store |
| Modify: `extensions/stores/finstack-ai-store-sqlite/{Cargo.toml,src/{append.rs,load.rs,store.rs,error.rs,tests.rs}}` | Rewire sqlite store |
| Modify: `CHANGELOG.md` | Entry for the new crate and the sqlite split-batch fix |

---

### Task 1: Scaffold `finstack-ai-store-common` with the error mapping

**Files:**
- Create: `extensions/stores/finstack-ai-store-common/Cargo.toml`
- Create: `extensions/stores/finstack-ai-store-common/src/lib.rs`
- Create: `extensions/stores/finstack-ai-store-common/src/error.rs`
- Modify: `Cargo.toml` (workspace root, members list around line 12 and workspace deps around line 112)

**Interfaces:**
- Produces: crate `finstack_ai_store_common` exporting `pub fn protocol_error(error: ProtocolError) -> StoreError`.

- [ ] **Step 1: Create the manifest**

```toml
[package]
name = "finstack-ai-store-common"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
authors.workspace = true
description = "Shared journal-store semantics for finstack-ai store backends"
publish = false

[dependencies]
finstack-ai-kernel = { workspace = true }
finstack-ai-protocol = { workspace = true }
finstack-ai-runtime = { workspace = true }
serde = { workspace = true }

[lints]
workspace = true
```

- [ ] **Step 2: Register the crate in the workspace root `Cargo.toml`**

Add `"extensions/stores/finstack-ai-store-common",` to `[workspace] members` next to the other two store crates (line ~12), and add to `[workspace.dependencies]` next to the other store entries (line ~112):

```toml
finstack-ai-store-common = { path = "extensions/stores/finstack-ai-store-common", version = "1.0.0" }
```

(Match the exact `version` used by the adjacent `finstack-ai-store-memory` entry.)

- [ ] **Step 3: Write `src/lib.rs` with the lint header and a failing module reference**

```rust
//! Shared journal-store semantics for finstack-ai store backends.
//!
//! Pure functions over the kernel, protocol, and runtime port types. Backends
//! (memory, sqlite, future stores) own all storage plumbing and delegate the
//! store-contract decisions — idempotency, admission, chain verification,
//! windowing — to this crate so the semantics cannot drift between them.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]

mod error;

pub use error::protocol_error;
```

- [ ] **Step 4: Write the failing test inside `src/error.rs`**

```rust
//! Protocol-to-store error mapping shared by all backends.

use finstack_ai_protocol::ProtocolError;
use finstack_ai_runtime::StoreError;

/// Map a protocol failure onto the stable store error surface.
#[must_use]
pub fn protocol_error(error: ProtocolError) -> StoreError {
    match error {
        ProtocolError::LimitExceeded { resource, limit } => {
            StoreError::LimitExceeded { resource, limit }
        }
        ProtocolError::Integrity { reason_code } | ProtocolError::InvalidCbor { reason_code } => {
            StoreError::Integrity { reason_code }
        }
        ProtocolError::Codec { .. } => StoreError::Integrity {
            reason_code: "canonical_codec",
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn protocol_errors_map_to_stable_store_codes() {
        assert!(matches!(
            protocol_error(ProtocolError::LimitExceeded {
                resource: "records",
                limit: 3
            }),
            StoreError::LimitExceeded {
                resource: "records",
                limit: 3
            }
        ));
        assert!(matches!(
            protocol_error(ProtocolError::Integrity {
                reason_code: "chain_broken"
            }),
            StoreError::Integrity {
                reason_code: "chain_broken"
            }
        ));
    }
}
```

Note: if `ProtocolError::Codec`'s payload shape differs from `{ .. }` matching, check its definition in `crates/finstack-ai-protocol` and keep the arm equivalent to the existing copies in `extensions/stores/finstack-ai-store-memory/src/lib.rs:802` and `extensions/stores/finstack-ai-store-sqlite/src/error.rs:54` (they are the source of truth being moved).

- [ ] **Step 5: Run the tests**

Run: `cargo test -p finstack-ai-store-common`
Expected: PASS (write-then-run here; the "failing first" evidence is the compile failure if the module were missing)

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml Cargo.lock extensions/stores/finstack-ai-store-common
git commit -m "Add the finstack-ai-store-common crate with the shared protocol error mapping"
```

---

### Task 2: Pin the sqlite idempotency-identity bytes before any move

The sqlite store persists `request_cbor` (CBOR of `AppendIdentity`) in the `batches` table and compares it byte-for-byte on batch-id replay. This golden test is written against the **current** code and must stay green through Task 7's move.

**Files:**
- Modify: `extensions/stores/finstack-ai-store-sqlite/src/append.rs` (make `request_cbor` reachable from tests: it already is `pub(crate)`-adjacent; expose `pub(crate) fn request_cbor`)
- Modify: `extensions/stores/finstack-ai-store-sqlite/src/tests.rs`

**Interfaces:**
- Consumes: existing test helpers `id`, `draft`, `request` already defined in `src/tests.rs` (same shape as the memory store's helpers).
- Produces: test `append_identity_encoding_is_stable` pinning a hex digest constant.

- [ ] **Step 1: Make `request_cbor` visible to the test module**

In `extensions/stores/finstack-ai-store-sqlite/src/append.rs`, change `fn request_cbor(` to `pub(crate) fn request_cbor(` (line ~142).

- [ ] **Step 2: Write the golden test with a placeholder digest**

Append to `extensions/stores/finstack-ai-store-sqlite/src/tests.rs` (inside the existing test module, reusing its `draft`/`request` helpers):

```rust
#[test]
fn append_identity_encoding_is_stable() {
    // Pins the persisted `batches.request_cbor` encoding. If this test fails,
    // existing databases will mis-detect batch-id replays as
    // `append_batch_id_reuse`. Do not update the constant without a schema
    // migration story.
    let frozen = request(7, 3, 1, vec![draft(70, 3), draft(71, 3)]);
    let bytes = crate::append::request_cbor(&frozen).expect("encode identity");
    let digest = finstack_ai_kernel::Digest::raw_json(&bytes);
    assert_eq!(digest.to_hex(), "REPLACE_WITH_ACTUAL");
}
```

If `Digest` has no `raw_json(&[u8])`/`to_hex` pair with these exact names, use whatever digest/hex helpers the sqlite crate already uses in `src/load.rs` (`digest_from_blob` shows `Digest::from_hex` exists; `finstack_ai_protocol::payload_digest` is an alternative — any deterministic hash-to-hex of `bytes` works). The assertion target is a fixed string constant.

- [ ] **Step 3: Run once to capture the real digest, then paste it in**

Run: `cargo test -p finstack-ai-store-sqlite append_identity_encoding_is_stable`
Expected: FAIL with the assertion printing the actual hex string. Copy that actual value into the constant, replacing `REPLACE_WITH_ACTUAL`.

- [ ] **Step 4: Re-run to verify it passes**

Run: `cargo test -p finstack-ai-store-sqlite append_identity_encoding_is_stable`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add extensions/stores/finstack-ai-store-sqlite
git commit -m "Pin the persisted sqlite append-identity encoding with a golden test"
```

---

### Task 3: Append semantics module

**Files:**
- Create: `extensions/stores/finstack-ai-store-common/src/append.rs`
- Create: `extensions/stores/finstack-ai-store-common/src/test_support.rs`
- Modify: `extensions/stores/finstack-ai-store-common/src/lib.rs`

**Interfaces:**
- Produces (all `pub`, re-exported from `lib.rs`):
  - `struct AppendIdentity { pub batch_id: [u8; 16], pub session_id: [u8; 16], pub expected_sequence: u64, pub draft_cbor: Vec<Vec<u8>> }` with `#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]` — field names, order, and derives copied exactly from `extensions/stores/finstack-ai-store-sqlite/src/append.rs:13-19` (persisted-format invariant).
  - `fn request_identity(request: &AppendRequest) -> Result<AppendIdentity, StoreError>`
  - `fn request_cbor(request: &AppendRequest) -> Result<Vec<u8>, StoreError>`
  - `fn classify_record_reuse(hits: &[AppendBatchId], total_records: usize) -> Result<Option<AppendBatchId>, StoreError>`
  - `fn check_append_sequence(current_head: u64, expected_sequence: u64) -> Result<(), StoreError>`
  - `struct SessionUsage { pub batches: usize, pub records: usize }`
  - `fn admit_append_limits(limits: StoreLimits, sessions: usize, usage: Option<SessionUsage>, incoming_records: usize) -> Result<(), StoreError>`
  - `fn build_committed_batch(request: &AppendRequest, previous_checksum: Option<Digest>) -> Result<CommittedBatch, StoreError>`

- [ ] **Step 1: Create the shared test fixtures**

`src/test_support.rs` — copy the `id`, `draft`, and `request` helper functions verbatim from `extensions/stores/finstack-ai-store-memory/src/lib.rs:844-905` (the `#[cfg(test)] mod tests` helpers), adjusting only imports:

```rust
//! Test-only record/request fixtures shared by the semantics unit tests.

use finstack_ai_kernel::{
    AppendRequest, AuthorizationEvidence, Digest, ExternalCommandKind, ExternalCommandRejected,
    ExternalCommandTarget, Id, IdTag, LaneTag, PrincipalRef, RECORD_FORMAT_VERSION,
    RECORD_KIND_VERSION, RecordBody, RecordDraft, RecordTag, RunTag, SessionTag, Timestamp,
};

pub(crate) fn id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

pub(crate) fn draft(record_ordinal: u64, session_ordinal: u64) -> RecordDraft {
    let principal =
        PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal");
    let authorization =
        AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("authorization");
    let rejection = ExternalCommandRejected::try_new(
        ExternalCommandKind::EffectCompletion,
        format!("completion-{record_ordinal}"),
        ExternalCommandTarget::Effect(id(record_ordinal + 1000)),
        principal,
        authorization,
        "conflicting_completion",
        Digest::raw_json(b"{}"),
        None,
    )
    .expect("rejection");
    RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        id::<RecordTag>(record_ordinal),
        id::<SessionTag>(session_ordinal),
        id::<LaneTag>(session_ordinal + 100),
        Some(id::<RunTag>(session_ordinal + 200)),
        Timestamp::from_unix_ms(i64::try_from(record_ordinal).expect("timestamp"))
            .expect("timestamp"),
        Vec::new(),
        RecordBody::ExternalCommandRejected(rejection),
    )
    .expect("draft")
}

pub(crate) fn request(
    batch_ordinal: u64,
    session_ordinal: u64,
    expected_sequence: u64,
    drafts: Vec<RecordDraft>,
) -> AppendRequest {
    AppendRequest::try_new(
        id(batch_ordinal),
        id::<SessionTag>(session_ordinal),
        expected_sequence,
        drafts,
    )
    .expect("append request")
}
```

Wire it in `lib.rs`: `#[cfg(test)] mod test_support;` and `mod append;` with `pub use append::{AppendIdentity, SessionUsage, admit_append_limits, build_committed_batch, check_append_sequence, classify_record_reuse, request_cbor, request_identity};`.

- [ ] **Step 2: Write the failing tests (inside `src/append.rs`'s `#[cfg(test)] mod tests`)**

```rust
#[cfg(test)]
mod tests {
    use finstack_ai_runtime::StoreLimits;

    use super::*;
    use crate::test_support::{draft, id, request};

    fn limits() -> StoreLimits {
        StoreLimits {
            sessions: 2,
            batches_per_session: 2,
            records_per_session: 3,
            snapshot_bytes: 1024,
        }
    }

    #[test]
    fn record_reuse_classification_matches_store_contract() {
        // No hits: fresh append.
        assert_eq!(classify_record_reuse(&[], 2).expect("fresh"), None);
        // All records previously committed in one batch: replay candidate.
        let batch = id(9);
        assert_eq!(
            classify_record_reuse(&[batch, batch], 2).expect("replay"),
            Some(batch)
        );
        // Partial reuse is corruption.
        assert!(matches!(
            classify_record_reuse(&[batch], 2),
            Err(StoreError::Corruption {
                reason_code: "mixed_record_id_reuse"
            })
        ));
        // Reuse spanning two original batches is corruption.
        assert!(matches!(
            classify_record_reuse(&[batch, id(10)], 2),
            Err(StoreError::Corruption {
                reason_code: "mixed_record_batch_reuse"
            })
        ));
    }

    #[test]
    fn sequence_admission_reports_conflict_and_exhaustion() {
        check_append_sequence(0, 1).expect("genesis");
        check_append_sequence(41, 42).expect("next");
        assert!(matches!(
            check_append_sequence(41, 41),
            Err(StoreError::Conflict {
                expected_sequence: 41,
                actual_next_sequence: 42
            })
        ));
        assert!(matches!(
            check_append_sequence(u64::MAX, 1),
            Err(StoreError::Integrity {
                reason_code: "sequence_exhausted"
            })
        ));
    }

    #[test]
    fn limit_admission_orders_sessions_then_batches_then_records() {
        // New session over the session ceiling.
        assert!(matches!(
            admit_append_limits(limits(), 2, None, 1),
            Err(StoreError::LimitExceeded {
                resource: "sessions",
                limit: 2
            })
        ));
        // Existing session over the batch ceiling.
        let full_batches = SessionUsage {
            batches: 2,
            records: 2,
        };
        assert!(matches!(
            admit_append_limits(limits(), 1, Some(full_batches), 1),
            Err(StoreError::LimitExceeded {
                resource: "batches_per_session",
                limit: 2
            })
        ));
        // Existing session over the record ceiling.
        let full_records = SessionUsage {
            batches: 1,
            records: 3,
        };
        assert!(matches!(
            admit_append_limits(limits(), 1, Some(full_records), 1),
            Err(StoreError::LimitExceeded {
                resource: "records_per_session",
                limit: 3
            })
        ));
        // New session whose first batch already exceeds the record ceiling.
        assert!(matches!(
            admit_append_limits(limits(), 0, None, 4),
            Err(StoreError::LimitExceeded {
                resource: "records_per_session",
                limit: 3
            })
        ));
        admit_append_limits(
            limits(),
            1,
            Some(SessionUsage {
                batches: 1,
                records: 2,
            }),
            1,
        )
        .expect("admitted");
    }

    #[test]
    fn committed_batches_chain_from_the_previous_checksum() {
        let first = build_committed_batch(&request(1, 1, 1, vec![draft(1, 1)]), None)
            .expect("first batch");
        assert_eq!((first.first_sequence, first.last_sequence), (1, 1));
        let prior = first.records[0].checksum();
        let second = build_committed_batch(
            &request(2, 1, 2, vec![draft(2, 1), draft(3, 1)]),
            Some(prior),
        )
        .expect("second batch");
        assert_eq!((second.first_sequence, second.last_sequence), (2, 3));
        assert_eq!(second.records[0].previous_checksum(), Some(prior));
    }

    #[test]
    fn identity_round_trips_and_is_deterministic() {
        let frozen = request(7, 3, 5, vec![draft(70, 3), draft(71, 3)]);
        let identity = request_identity(&frozen).expect("identity");
        assert_eq!(identity.expected_sequence, 5);
        assert_eq!(identity.draft_cbor.len(), 2);
        assert_eq!(
            request_cbor(&frozen).expect("bytes"),
            request_cbor(&frozen).expect("bytes again")
        );
    }
}
```

- [ ] **Step 3: Run to verify the tests fail**

Run: `cargo test -p finstack-ai-store-common`
Expected: FAIL to compile ("cannot find function `classify_record_reuse`" etc.)

- [ ] **Step 4: Implement `src/append.rs`**

The bodies are moves of existing code — `AppendIdentity`/`request_identity`/`request_cbor` from `extensions/stores/finstack-ai-store-sqlite/src/append.rs:13-19,128-144` (fields become `pub` with doc comments), `build_committed_batch` from either store (verbatim duplicate; use the sqlite copy at `append.rs:146-176`, whose `.map_err(protocol_error)` now points at `crate::error::protocol_error`). The three new admission functions codify the shared inline logic:

```rust
//! Append admission and batch-commitment semantics shared by all backends.

use finstack_ai_kernel::{AppendBatchId, AppendRequest, CommittedBatch, Digest};
use finstack_ai_protocol::{commit_records, encode};
use finstack_ai_runtime::{StoreError, StoreLimits};
use serde::{Deserialize, Serialize};

use crate::error::protocol_error;

/// Durable identity of one append request, compared byte-for-byte on replay.
///
/// The CBOR encoding of this struct is a persisted format (sqlite stores it
/// in `batches.request_cbor`). Do not reorder, rename, or retype fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppendIdentity {
    /// Append batch id bytes.
    pub batch_id: [u8; 16],
    /// Session id bytes.
    pub session_id: [u8; 16],
    /// Optimistic next-sequence precondition.
    pub expected_sequence: u64,
    /// Canonical CBOR of each record draft, in request order.
    pub draft_cbor: Vec<Vec<u8>>,
}

/// Compute the durable identity of one append request.
///
/// # Errors
///
/// Returns [`StoreError::Integrity`] when a draft cannot be canonically encoded.
pub fn request_identity(request: &AppendRequest) -> Result<AppendIdentity, StoreError> {
    let draft_cbor = request
        .records()
        .iter()
        .map(|draft| encode(draft).map_err(protocol_error))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AppendIdentity {
        batch_id: request.batch_id().to_bytes(),
        session_id: request.session_id().to_bytes(),
        expected_sequence: request.expected_sequence(),
        draft_cbor,
    })
}

/// Canonical CBOR bytes of [`request_identity`].
///
/// # Errors
///
/// Returns [`StoreError::Integrity`] when encoding fails.
pub fn request_cbor(request: &AppendRequest) -> Result<Vec<u8>, StoreError> {
    encode(&request_identity(request)?).map_err(protocol_error)
}

/// Classify record-id reuse for one append.
///
/// `hits` holds the original batch id of every incoming record that already
/// exists, in any order; `total_records` is the incoming record count.
/// `Ok(None)` means a fresh append. `Ok(Some(batch))` means every record was
/// previously committed in `batch` — the caller must compare identities and
/// either replay or fail with `record_id_reuse`.
///
/// # Errors
///
/// Returns [`StoreError::Corruption`] for partial reuse
/// (`mixed_record_id_reuse`) or reuse spanning batches
/// (`mixed_record_batch_reuse`).
pub fn classify_record_reuse(
    hits: &[AppendBatchId],
    total_records: usize,
) -> Result<Option<AppendBatchId>, StoreError> {
    let Some(first) = hits.first() else {
        return Ok(None);
    };
    if hits.len() != total_records {
        return Err(StoreError::Corruption {
            reason_code: "mixed_record_id_reuse",
        });
    }
    if hits.iter().any(|batch_id| batch_id != first) {
        return Err(StoreError::Corruption {
            reason_code: "mixed_record_batch_reuse",
        });
    }
    Ok(Some(*first))
}

/// Enforce the optimistic append-sequence precondition.
///
/// # Errors
///
/// Returns [`StoreError::Integrity`] (`sequence_exhausted`) when the journal
/// head cannot advance, and [`StoreError::Conflict`] when
/// `expected_sequence` is not the next sequence.
pub fn check_append_sequence(
    current_head: u64,
    expected_sequence: u64,
) -> Result<(), StoreError> {
    let actual_next_sequence = current_head.checked_add(1).ok_or(StoreError::Integrity {
        reason_code: "sequence_exhausted",
    })?;
    if expected_sequence != actual_next_sequence {
        return Err(StoreError::Conflict {
            expected_sequence,
            actual_next_sequence,
        });
    }
    Ok(())
}

/// Current committed footprint of one session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionUsage {
    /// Committed batches in the session.
    pub batches: usize,
    /// Committed records in the session.
    pub records: usize,
}

/// Enforce store resource ceilings for one append, in contract order:
/// sessions, then batches, then records.
///
/// `usage` is `None` for a session that does not exist yet; `sessions` is the
/// current distinct-session count.
///
/// # Errors
///
/// Returns [`StoreError::LimitExceeded`] naming the exhausted resource.
pub fn admit_append_limits(
    limits: StoreLimits,
    sessions: usize,
    usage: Option<SessionUsage>,
    incoming_records: usize,
) -> Result<(), StoreError> {
    match usage {
        None => {
            if sessions >= limits.sessions {
                return Err(StoreError::LimitExceeded {
                    resource: "sessions",
                    limit: limits.sessions,
                });
            }
            if incoming_records > limits.records_per_session {
                return Err(StoreError::LimitExceeded {
                    resource: "records_per_session",
                    limit: limits.records_per_session,
                });
            }
        }
        Some(usage) => {
            if usage.batches >= limits.batches_per_session {
                return Err(StoreError::LimitExceeded {
                    resource: "batches_per_session",
                    limit: limits.batches_per_session,
                });
            }
            if usage.records + incoming_records > limits.records_per_session {
                return Err(StoreError::LimitExceeded {
                    resource: "records_per_session",
                    limit: limits.records_per_session,
                });
            }
        }
    }
    Ok(())
}

/// Seal one frozen request into a committed, checksum-chained batch.
///
/// # Errors
///
/// Returns [`StoreError::Integrity`] when commitment or chaining fails.
pub fn build_committed_batch(
    request: &AppendRequest,
    previous_checksum: Option<Digest>,
) -> Result<CommittedBatch, StoreError> {
    let records = commit_records(
        request.records(),
        request.expected_sequence(),
        previous_checksum,
        None,
    )
    .map_err(protocol_error)?;
    let last_sequence = request
        .expected_sequence()
        .checked_add(
            u64::try_from(records.len() - 1).map_err(|_| StoreError::Integrity {
                reason_code: "record_count_overflow",
            })?,
        )
        .ok_or(StoreError::Integrity {
            reason_code: "sequence_exhausted",
        })?;
    CommittedBatch::try_new(
        request.batch_id(),
        request.expected_sequence(),
        last_sequence,
        records,
    )
    .map_err(|_| StoreError::Integrity {
        reason_code: "committed_batch_invalid",
    })
}
```

- [ ] **Step 5: Run the tests**

Run: `cargo test -p finstack-ai-store-common`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add extensions/stores/finstack-ai-store-common
git commit -m "Add shared append admission and batch-commitment semantics"
```

---

### Task 4: Snapshot and prune semantics module

**Files:**
- Create: `extensions/stores/finstack-ai-store-common/src/snapshot.rs`
- Modify: `extensions/stores/finstack-ai-store-common/src/lib.rs` (add `mod snapshot;` + re-exports)

**Interfaces:**
- Consumes: nothing from earlier tasks (independent module).
- Produces (all `pub`):
  - `fn encode_state_request(request: &StateSnapshotRequest, max_bytes: usize) -> Result<OpaqueSnapshot, StoreError>`
  - `fn accelerated_from(snapshot: &OpaqueSnapshot) -> Option<AcceleratedRestore>`
  - `fn outstanding_count(restored: &AcceleratedRestore) -> u64`
  - `fn tombstone_count(restored: &AcceleratedRestore) -> u64`
  - `fn check_snapshot_size(bytes: usize, limit: usize) -> Result<(), StoreError>`
  - `fn admit_snapshot_sequence(snapshot_sequence: u64, head_sequence: u64, current_snapshot_sequence: Option<u64>) -> Result<(), StoreError>`
  - `fn admit_prune_snapshot(snapshot_sequence: u64, head_sequence: u64) -> Result<(), StoreError>`

- [ ] **Step 1: Write the failing tests (in `src/snapshot.rs`'s test module)**

```rust
#[cfg(test)]
mod tests {
    use finstack_ai_kernel::Digest;
    use finstack_ai_runtime::OpaqueSnapshot;

    use super::*;

    #[test]
    fn snapshot_admission_enforces_size_head_and_regression() {
        check_snapshot_size(3, 3).expect("at limit");
        assert!(matches!(
            check_snapshot_size(4, 3),
            Err(StoreError::LimitExceeded {
                resource: "snapshot_bytes",
                limit: 3
            })
        ));
        admit_snapshot_sequence(2, 5, None).expect("first snapshot");
        admit_snapshot_sequence(3, 5, Some(2)).expect("advance");
        admit_snapshot_sequence(3, 5, Some(3)).expect("same sequence rewrites");
        assert!(matches!(
            admit_snapshot_sequence(6, 5, None),
            Err(StoreError::InvalidRequest {
                reason_code: "snapshot_ahead_of_journal"
            })
        ));
        assert!(matches!(
            admit_snapshot_sequence(2, 5, Some(3)),
            Err(StoreError::InvalidRequest {
                reason_code: "snapshot_sequence_regression"
            })
        ));
    }

    #[test]
    fn prune_admission_requires_an_aligned_covering_snapshot() {
        admit_prune_snapshot(3, 5).expect("covered");
        for (sequence, head) in [(0, 5), (6, 5)] {
            assert!(matches!(
                admit_prune_snapshot(sequence, head),
                Err(StoreError::InvalidRequest {
                    reason_code: "prune_snapshot_not_aligned"
                })
            ));
        }
    }

    #[test]
    fn undecodable_snapshot_bytes_yield_no_accelerated_restore() {
        let snapshot = OpaqueSnapshot::try_new(1, Digest::raw_json(b"{}"), b"junk".as_slice(), 16)
            .expect("snapshot");
        assert!(accelerated_from(&snapshot).is_none());
    }
}
```

(`encode_state_request`, `outstanding_count`, and `tombstone_count` need a real `KernelState`; they are covered end-to-end by both stores' existing snapshot/prune tests, which is sufficient — they are verbatim moves.)

- [ ] **Step 2: Run to verify the tests fail**

Run: `cargo test -p finstack-ai-store-common`
Expected: FAIL to compile

- [ ] **Step 3: Implement `src/snapshot.rs`**

`encode_state_request`, `accelerated_from`, `outstanding_count`, `tombstone_count` are verbatim moves from `extensions/stores/finstack-ai-store-sqlite/src/load.rs:585-635` (identical copies exist in the memory store at `lib.rs:585-685`), with `pub` visibility and doc comments. Add the three admission functions:

```rust
/// Reject snapshot payloads over the configured byte ceiling.
///
/// # Errors
///
/// Returns [`StoreError::LimitExceeded`] (`snapshot_bytes`).
pub fn check_snapshot_size(bytes: usize, limit: usize) -> Result<(), StoreError> {
    if bytes > limit {
        return Err(StoreError::LimitExceeded {
            resource: "snapshot_bytes",
            limit,
        });
    }
    Ok(())
}

/// Enforce snapshot-sequence monotonicity against the journal head.
///
/// # Errors
///
/// Returns [`StoreError::InvalidRequest`] with `snapshot_ahead_of_journal`
/// or `snapshot_sequence_regression`.
pub fn admit_snapshot_sequence(
    snapshot_sequence: u64,
    head_sequence: u64,
    current_snapshot_sequence: Option<u64>,
) -> Result<(), StoreError> {
    if snapshot_sequence > head_sequence {
        return Err(StoreError::InvalidRequest {
            reason_code: "snapshot_ahead_of_journal",
        });
    }
    if current_snapshot_sequence.is_some_and(|current| current > snapshot_sequence) {
        return Err(StoreError::InvalidRequest {
            reason_code: "snapshot_sequence_regression",
        });
    }
    Ok(())
}

/// Require a prune's snapshot to cover a real, committed prefix.
///
/// # Errors
///
/// Returns [`StoreError::InvalidRequest`] (`prune_snapshot_not_aligned`).
pub fn admit_prune_snapshot(
    snapshot_sequence: u64,
    head_sequence: u64,
) -> Result<(), StoreError> {
    if snapshot_sequence == 0 || snapshot_sequence > head_sequence {
        return Err(StoreError::InvalidRequest {
            reason_code: "prune_snapshot_not_aligned",
        });
    }
    Ok(())
}
```

Module imports: `finstack_ai_protocol::{decode_opaque_snapshot, encode_snapshot}`, `finstack_ai_runtime::{AcceleratedRestore, OpaqueSnapshot, StateSnapshotRequest, StoreError}`.

- [ ] **Step 4: Run the tests**

Run: `cargo test -p finstack-ai-store-common`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add extensions/stores/finstack-ai-store-common
git commit -m "Add shared snapshot and prune admission semantics"
```

---

### Task 5: Window and scan semantics module

**Files:**
- Create: `extensions/stores/finstack-ai-store-common/src/window.rs`
- Create: `extensions/stores/finstack-ai-store-common/src/scan.rs`
- Modify: `extensions/stores/finstack-ai-store-common/src/lib.rs` (add modules + re-exports)

**Interfaces:**
- Consumes: `build_committed_batch` (Task 3) inside tests to construct chained batches.
- Produces (all `pub`):
  - `struct WindowCodes { pub gap: &'static str, pub split: &'static str, pub checksum: &'static str }` (`Debug, Clone, Copy, PartialEq, Eq`)
  - `const FROM_SEQUENCE_WINDOW: WindowCodes = WindowCodes { gap: "load_from_sequence_gap", split: "load_from_splits_batch", checksum: "load_from_prior_checksum_mismatch" };`
  - `const SNAPSHOT_WINDOW: WindowCodes = WindowCodes { gap: "snapshot_missing_record", split: "snapshot_splits_batch", checksum: "snapshot_checksum_mismatch" };`
  - `fn select_tail_batches(batches: &[CommittedBatch], start: u64, codes: WindowCodes) -> Result<&[CommittedBatch], StoreError>`
  - `fn check_batch_alignment(start_batch: AppendBatchId, prior_batch: Option<AppendBatchId>, codes: WindowCodes) -> Result<(), StoreError>`
  - `fn verify_tail_records(records: &[RecordEnvelope], start: u64, prior_checksum: Digest, head_sequence: u64, stored_head: Option<Digest>, codes: WindowCodes) -> Result<(), StoreError>`
  - `fn verify_full_head(records: &[RecordEnvelope], stored_head: Option<Digest>) -> Result<Option<Digest>, StoreError>`
  - scan module: `fn validate_scan_limit(limit: u32) -> Result<(), StoreError>`, `const fn scan_start(from_sequence: u64) -> u64`, `fn scan_next_sequence(records: &[RecordEnvelope], has_more: bool) -> Option<u64>`

- [ ] **Step 1: Write the failing tests (in `src/window.rs` and `src/scan.rs` test modules)**

```rust
// window.rs tests
#[cfg(test)]
mod tests {
    use super::*;
    use crate::append::build_committed_batch;
    use crate::test_support::{draft, request};

    fn chained_batches() -> Vec<CommittedBatch> {
        // Batch A: sequences 1-2. Batch B: sequences 3-4.
        let first = build_committed_batch(&request(1, 1, 1, vec![draft(1, 1), draft(2, 1)]), None)
            .expect("first");
        let prior = first.records[1].checksum();
        let second = build_committed_batch(
            &request(2, 1, 3, vec![draft(3, 1), draft(4, 1)]),
            Some(prior),
        )
        .expect("second");
        vec![first, second]
    }

    #[test]
    fn tail_selection_enforces_batch_alignment() {
        let batches = chained_batches();
        // Aligned start returns the suffix.
        let tail = select_tail_batches(&batches, 3, FROM_SEQUENCE_WINDOW).expect("aligned");
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].first_sequence, 3);
        // Start inside batch A is a split.
        assert!(matches!(
            select_tail_batches(&batches, 2, FROM_SEQUENCE_WINDOW),
            Err(StoreError::Integrity {
                reason_code: "load_from_splits_batch"
            })
        ));
        // Start past every batch yields an empty tail (head checks happen later).
        assert!(
            select_tail_batches(&batches, 5, FROM_SEQUENCE_WINDOW)
                .expect("empty tail")
                .is_empty()
        );
    }

    #[test]
    fn tail_verification_checks_gap_checksum_and_head() {
        let batches = chained_batches();
        let head = batches[1].records[1].checksum();
        let prior = batches[0].records[1].checksum();
        let tail: Vec<RecordEnvelope> = batches[1].records.iter().cloned().collect();
        verify_tail_records(&tail, 3, prior, 4, Some(head), FROM_SEQUENCE_WINDOW)
            .expect("verified tail");
        // Empty tail at head+1 needs the stored head to equal the prior checksum.
        verify_tail_records(&[], 5, head, 4, Some(head), FROM_SEQUENCE_WINDOW)
            .expect("empty tail at head");
        assert!(matches!(
            verify_tail_records(&[], 5, prior, 4, Some(head), FROM_SEQUENCE_WINDOW),
            Err(StoreError::Integrity {
                reason_code: "load_from_prior_checksum_mismatch"
            })
        ));
        // Start beyond head+1 is a gap.
        assert!(matches!(
            verify_tail_records(&[], 6, head, 4, Some(head), SNAPSHOT_WINDOW),
            Err(StoreError::Integrity {
                reason_code: "snapshot_missing_record"
            })
        ));
        // First record after a hole is a gap.
        assert!(matches!(
            verify_tail_records(&tail, 2, prior, 4, Some(head), FROM_SEQUENCE_WINDOW),
            Err(StoreError::Integrity {
                reason_code: "load_from_sequence_gap"
            })
        ));
        // Wrong prior checksum is a checksum mismatch.
        assert!(matches!(
            verify_tail_records(&tail, 3, head, 4, Some(head), FROM_SEQUENCE_WINDOW),
            Err(StoreError::Integrity {
                reason_code: "load_from_prior_checksum_mismatch"
            })
        ));
        // Verified chain must land on the stored head.
        assert!(matches!(
            verify_tail_records(&tail, 3, prior, 4, Some(prior), FROM_SEQUENCE_WINDOW),
            Err(StoreError::Integrity {
                reason_code: "head_checksum_mismatch"
            })
        ));
    }

    #[test]
    fn mid_batch_starts_are_rejected_by_alignment() {
        let batches = chained_batches();
        let batch_a = batches[0].batch_id;
        let batch_b = batches[1].batch_id;
        check_batch_alignment(batch_b, Some(batch_a), SNAPSHOT_WINDOW).expect("boundary");
        check_batch_alignment(batch_a, None, SNAPSHOT_WINDOW).expect("genesis");
        assert!(matches!(
            check_batch_alignment(batch_a, Some(batch_a), SNAPSHOT_WINDOW),
            Err(StoreError::Integrity {
                reason_code: "snapshot_splits_batch"
            })
        ));
    }

    #[test]
    fn full_head_verification_accepts_prefixless_journals() {
        let batches = chained_batches();
        let head = batches[1].records[1].checksum();
        let all: Vec<RecordEnvelope> = batches
            .iter()
            .flat_map(|batch| batch.records.iter().cloned())
            .collect();
        assert_eq!(
            verify_full_head(&all, Some(head)).expect("full"),
            Some(head)
        );
        // Pruned journal: records start past sequence 1.
        let tail: Vec<RecordEnvelope> = batches[1].records.iter().cloned().collect();
        assert_eq!(
            verify_full_head(&tail, Some(head)).expect("tail"),
            Some(head)
        );
        assert!(matches!(
            verify_full_head(&all, None),
            Err(StoreError::Integrity {
                reason_code: "head_checksum_mismatch"
            })
        ));
    }
}
```

```rust
// scan.rs tests
#[cfg(test)]
mod tests {
    use finstack_ai_runtime::SCAN_PAGE_MAX_RECORDS;

    use super::*;
    use crate::append::build_committed_batch;
    use crate::test_support::{draft, request};

    #[test]
    fn scan_validation_and_paging_match_the_contract() {
        assert!(matches!(
            validate_scan_limit(0),
            Err(StoreError::InvalidRequest {
                reason_code: "scan_limit_zero"
            })
        ));
        assert!(matches!(
            validate_scan_limit(SCAN_PAGE_MAX_RECORDS + 1),
            Err(StoreError::InvalidRequest {
                reason_code: "scan_limit_exceeded"
            })
        ));
        validate_scan_limit(SCAN_PAGE_MAX_RECORDS).expect("at limit");
        assert_eq!(scan_start(0), 1);
        assert_eq!(scan_start(7), 7);
        let batch = build_committed_batch(&request(1, 1, 1, vec![draft(1, 1)]), None)
            .expect("batch");
        let records: Vec<RecordEnvelope> = batch.records.iter().cloned().collect();
        assert_eq!(scan_next_sequence(&records, true), Some(2));
        assert_eq!(scan_next_sequence(&records, false), None);
        assert_eq!(scan_next_sequence(&[], true), None);
    }
}
```

- [ ] **Step 2: Run to verify the tests fail**

Run: `cargo test -p finstack-ai-store-common`
Expected: FAIL to compile

- [ ] **Step 3: Implement `src/window.rs`**

```rust
//! Tail-window and full-journal chain verification shared by all backends.

use finstack_ai_kernel::{AppendBatchId, CommittedBatch, Digest, RecordEnvelope};
use finstack_ai_protocol::{verify_chain, verify_chain_from};
use finstack_ai_runtime::StoreError;

use crate::error::protocol_error;

/// Stable integrity reason codes for one load window flavor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowCodes {
    /// A required record is missing.
    pub gap: &'static str,
    /// The window start falls inside a committed batch.
    pub split: &'static str,
    /// The tail does not chain from the expected prior checksum.
    pub checksum: &'static str,
}

/// Codes for [`finstack_ai_runtime::LoadWindow::FromSequence`].
pub const FROM_SEQUENCE_WINDOW: WindowCodes = WindowCodes {
    gap: "load_from_sequence_gap",
    split: "load_from_splits_batch",
    checksum: "load_from_prior_checksum_mismatch",
};

/// Codes for [`finstack_ai_runtime::LoadWindow::SnapshotPlusTail`].
pub const SNAPSHOT_WINDOW: WindowCodes = WindowCodes {
    gap: "snapshot_missing_record",
    split: "snapshot_splits_batch",
    checksum: "snapshot_checksum_mismatch",
};

/// Select the batch-aligned tail starting at `start`.
///
/// Returns an empty slice when every batch ends before `start` (the caller's
/// tail verification decides whether that is the empty-at-head case or a gap).
///
/// # Errors
///
/// Returns [`StoreError::Integrity`] with `codes.split` when `start` falls
/// strictly inside a committed batch.
pub fn select_tail_batches(
    batches: &[CommittedBatch],
    start: u64,
    codes: WindowCodes,
) -> Result<&[CommittedBatch], StoreError> {
    let Some(index) = batches
        .iter()
        .position(|batch| batch.first_sequence >= start)
    else {
        if batches
            .last()
            .is_some_and(|batch| batch.last_sequence >= start)
        {
            return Err(StoreError::Integrity {
                reason_code: codes.split,
            });
        }
        return Ok(&[]);
    };
    if batches[index].first_sequence > start
        && index
            .checked_sub(1)
            .and_then(|prior| batches.get(prior))
            .is_some_and(|prior| prior.last_sequence >= start)
    {
        return Err(StoreError::Integrity {
            reason_code: codes.split,
        });
    }
    Ok(&batches[index..])
}

/// Require a window start to sit on a batch boundary.
///
/// `start_batch` is the batch holding the record at the window start;
/// `prior_batch` is the batch holding the record immediately before it, when
/// that record exists.
///
/// # Errors
///
/// Returns [`StoreError::Integrity`] with `codes.split` when both records
/// share a batch.
pub fn check_batch_alignment(
    start_batch: AppendBatchId,
    prior_batch: Option<AppendBatchId>,
    codes: WindowCodes,
) -> Result<(), StoreError> {
    if prior_batch == Some(start_batch) {
        return Err(StoreError::Integrity {
            reason_code: codes.split,
        });
    }
    Ok(())
}

/// Verify a tail window's chain and head against the stored session head.
///
/// `records` holds every committed record at or after `start`, in sequence
/// order. An empty `records` is valid only when `start` is exactly one past
/// the head and the stored head equals `prior_checksum`.
///
/// # Errors
///
/// Returns [`StoreError::Integrity`] with `codes.gap` for missing records,
/// `codes.checksum` for a broken prior link, and `head_checksum_mismatch`
/// when the verified chain does not land on `stored_head`.
pub fn verify_tail_records(
    records: &[RecordEnvelope],
    start: u64,
    prior_checksum: Digest,
    head_sequence: u64,
    stored_head: Option<Digest>,
    codes: WindowCodes,
) -> Result<(), StoreError> {
    if start > head_sequence.saturating_add(1) {
        return Err(StoreError::Integrity {
            reason_code: codes.gap,
        });
    }
    let Some(first) = records.first() else {
        if start != head_sequence.saturating_add(1) {
            return Err(StoreError::Integrity {
                reason_code: codes.gap,
            });
        }
        if stored_head != Some(prior_checksum) {
            return Err(StoreError::Integrity {
                reason_code: codes.checksum,
            });
        }
        return Ok(());
    };
    if first.sequence() != start {
        return Err(StoreError::Integrity {
            reason_code: codes.gap,
        });
    }
    if first.previous_checksum() != Some(prior_checksum) {
        return Err(StoreError::Integrity {
            reason_code: codes.checksum,
        });
    }
    let head =
        verify_chain_from(records, Some(prior_checksum), Some(start)).map_err(protocol_error)?;
    if head != stored_head {
        return Err(StoreError::Integrity {
            reason_code: "head_checksum_mismatch",
        });
    }
    Ok(())
}

/// Verify a full (or prune-truncated) journal and return its verified head.
///
/// # Errors
///
/// Returns [`StoreError::Integrity`] when the chain is broken or the verified
/// head differs from `stored_head` (`head_checksum_mismatch`).
pub fn verify_full_head(
    records: &[RecordEnvelope],
    stored_head: Option<Digest>,
) -> Result<Option<Digest>, StoreError> {
    let head = match records.first() {
        Some(first) if first.sequence() > 1 => {
            verify_chain_from(records, first.previous_checksum(), Some(first.sequence()))
                .map_err(protocol_error)?
        }
        _ => verify_chain(records).map_err(protocol_error)?,
    };
    if head != stored_head {
        return Err(StoreError::Integrity {
            reason_code: "head_checksum_mismatch",
        });
    }
    Ok(head)
}
```

Note on `select_tail_batches`: the "no batch has `first_sequence >= start` but the last batch still covers `start`" branch handles a start inside the final batch; the second split check handles a start inside a middle batch. A start in a hole between batches falls through to the gap path in `verify_tail_records`.

- [ ] **Step 4: Implement `src/scan.rs`**

```rust
//! Scan-request validation and paging semantics shared by all backends.

use finstack_ai_kernel::RecordEnvelope;
use finstack_ai_runtime::{SCAN_PAGE_MAX_RECORDS, StoreError};

/// Validate a scan page size against the port contract.
///
/// # Errors
///
/// Returns [`StoreError::InvalidRequest`] with `scan_limit_zero` or
/// `scan_limit_exceeded`.
pub fn validate_scan_limit(limit: u32) -> Result<(), StoreError> {
    if limit == 0 {
        return Err(StoreError::InvalidRequest {
            reason_code: "scan_limit_zero",
        });
    }
    if limit > SCAN_PAGE_MAX_RECORDS {
        return Err(StoreError::InvalidRequest {
            reason_code: "scan_limit_exceeded",
        });
    }
    Ok(())
}

/// Normalize a scan start: `0` means the first committed record.
#[must_use]
pub const fn scan_start(from_sequence: u64) -> u64 {
    if from_sequence == 0 { 1 } else { from_sequence }
}

/// Next sequence to request after this page, or `None` at the session end.
#[must_use]
pub fn scan_next_sequence(records: &[RecordEnvelope], has_more: bool) -> Option<u64> {
    if !has_more {
        return None;
    }
    records
        .last()
        .and_then(|record| record.sequence().checked_add(1))
}
```

Re-export everything from `lib.rs` (`pub use scan::{scan_next_sequence, scan_start, validate_scan_limit}; pub use window::{FROM_SEQUENCE_WINDOW, SNAPSHOT_WINDOW, WindowCodes, check_batch_alignment, select_tail_batches, verify_full_head, verify_tail_records};`).

- [ ] **Step 5: Run the tests**

Run: `cargo test -p finstack-ai-store-common`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add extensions/stores/finstack-ai-store-common
git commit -m "Add shared window verification and scan semantics"
```

---

### Task 6: Rewire the memory store through `finstack-ai-store-common`

**Files:**
- Modify: `extensions/stores/finstack-ai-store-memory/Cargo.toml`
- Modify: `extensions/stores/finstack-ai-store-memory/src/lib.rs`

**Interfaces:**
- Consumes: everything produced in Tasks 3–5.
- Produces: no public API change; `MemoryJournalStore` behavior identical except the unified tail-window codes (spec "Behavioral invariants").

- [ ] **Step 1: Write the failing behavior-unification test**

Add to the memory store's test module (uses existing helpers):

```rust
#[test]
fn tail_window_reports_gap_for_holes_and_split_for_mid_batch_starts() {
    let store = MemoryJournalStore::try_new(limits()).expect("store");
    let first = block_on(store.append(request(1, 1, 1, vec![draft(1, 1), draft(2, 1)])))
        .expect("append");
    block_on(store.append(request(2, 1, 3, vec![draft(3, 1)]))).expect("append");
    // Mid-batch start (sequence 2 is inside batch 1) is a split.
    assert!(matches!(
        block_on(store.load_from(LoadFromRequest {
            session_id: id::<SessionTag>(1),
            window: LoadWindow::FromSequence {
                from_sequence: 2,
                prior_checksum: first.records[0].checksum(),
            },
        })),
        Err(StoreError::Integrity {
            reason_code: "load_from_splits_batch"
        })
    ));
    // Start past the head is a gap (unified from the old split code).
    assert!(matches!(
        block_on(store.load_from(LoadFromRequest {
            session_id: id::<SessionTag>(1),
            window: LoadWindow::FromSequence {
                from_sequence: 5,
                prior_checksum: first.records[1].checksum(),
            },
        })),
        Err(StoreError::Integrity {
            reason_code: "load_from_sequence_gap"
        })
    ));
}
```

- [ ] **Step 2: Run to verify the new test fails**

Run: `cargo test -p finstack-ai-store-memory tail_window_reports`
Expected: the mid-batch case may already pass (memory already reports split there); the past-head case FAILS if it currently maps elsewhere — record actual behavior. If both already pass, that is acceptable evidence; continue.

- [ ] **Step 3: Add the dependency and rewire**

`Cargo.toml`: add `finstack-ai-store-common = { workspace = true }` to `[dependencies]`.

In `src/lib.rs`:
1. Delete local `build_committed_batch` (lines ~619-649), `encode_state_request` (~585-605), `accelerated_from` (~607-617), `outstanding_count`/`tombstone_count` (~668-685), `protocol_error` (~802-815), `verify_session`'s chain body, and `loaded_from_batches` (~687-763).
2. Import the replacements:

```rust
use finstack_ai_store_common::{
    FROM_SEQUENCE_WINDOW, SNAPSHOT_WINDOW, SessionUsage, WindowCodes, accelerated_from,
    admit_append_limits, admit_prune_snapshot, admit_snapshot_sequence, build_committed_batch,
    check_append_sequence, check_snapshot_size, classify_record_reuse, encode_state_request,
    outstanding_count, protocol_error, scan_next_sequence, scan_start, select_tail_batches,
    tombstone_count, validate_scan_limit, verify_full_head, verify_tail_records,
};
```

3. `append_sync` keeps its batch-id replay block (it compares whole `AppendRequest`s, which is equivalent to identity comparison), then delegates:

```rust
// record-id reuse classification
let hits = request
    .records()
    .iter()
    .filter_map(|record| inner.records_by_id.get(&record.record_id()))
    .map(|entry| entry.batch_id)
    .collect::<Vec<_>>();
if let Some(original_batch_id) = classify_record_reuse(&hits, request.records().len())? {
    let existing = inner
        .batches_by_id
        .get(&original_batch_id)
        .ok_or(StoreError::Integrity {
            reason_code: "missing_record_batch_index",
        })?;
    let exact_records = existing.request.session_id() == request.session_id()
        && existing.request.expected_sequence() == request.expected_sequence()
        && existing.request.records() == request.records();
    if exact_records {
        return Ok(existing.committed.clone());
    }
    return Err(StoreError::Corruption {
        reason_code: "record_id_reuse",
    });
}

// sequence precondition
let current_head = inner
    .sessions
    .get(&request.session_id())
    .map_or(0, |session| session.head_sequence);
check_append_sequence(current_head, request.expected_sequence())?;

// limits
let usage = inner.sessions.get(&request.session_id()).map(|session| SessionUsage {
    batches: session.batches.len(),
    records: session.records,
});
let session_is_new = usage.is_none();
admit_append_limits(self.limits, inner.sessions.len(), usage, request.records().len())?;
```

(The rest of `append_sync` — building and indexing the committed batch — is unchanged; `build_committed_batch` now resolves to the shared crate.)

4. `verify_session` becomes:

```rust
fn verify_session(session: &SessionData) -> Result<(), StoreError> {
    let records = flatten_records(session);
    verify_full_head(&records, session.head_checksum).map(|_| ())
}
```

5. `loaded_from_batches` shrinks to selection + verification + assembly, with `WindowCodes` replacing the three `&'static str` parameters:

```rust
fn loaded_from_batches(
    session_id: SessionId,
    session: &SessionData,
    start: u64,
    prior_checksum: Digest,
    codes: WindowCodes,
) -> Result<LoadedSession, StoreError> {
    let tail = select_tail_batches(&session.batches, start, codes)?;
    let records = tail
        .iter()
        .flat_map(|batch| batch.records.iter().cloned())
        .collect::<Vec<_>>();
    verify_tail_records(
        &records,
        start,
        prior_checksum,
        session.head_sequence,
        session.head_checksum,
        codes,
    )?;
    let snapshot = session.snapshot.clone();
    Ok(LoadedSession {
        session_id,
        head_sequence: session.head_sequence,
        head_checksum: session.head_checksum,
        metadata: session.metadata.clone(),
        committed_batches: tail.to_vec().into(),
        snapshot: snapshot.clone(),
        accelerated: snapshot.as_ref().and_then(accelerated_from),
    })
}
```

Call sites pass `FROM_SEQUENCE_WINDOW` (in `load_from_sequence`) and `SNAPSHOT_WINDOW` (in `load_snapshot_plus_tail`).

6. `scan_sync`: replace the two inline limit checks with `validate_scan_limit(request.limit)?`, the start normalization with `scan_start(request.from_sequence)`, and the `next_sequence` computation with `scan_next_sequence(&records, saw_more)`.
7. `write_snapshot_sync`: replace the size check with `check_snapshot_size(request.snapshot.bytes().len(), self.limits.snapshot_bytes)?` and the two sequence checks with `admit_snapshot_sequence(request.snapshot.sequence(), session.head_sequence, session.snapshot.as_ref().map(OpaqueSnapshot::sequence))?`.
8. `prune_sync`: replace the `snapshot.sequence() == 0 || ... > head` check with `admit_prune_snapshot(snapshot.sequence(), session.head_sequence)?` (the batch-alignment `prune_not_batch_aligned` check and the retain/recount logic stay local).
9. Remove now-unused imports (`commit_records`, `verify_chain`, `verify_chain_from`, `decode_opaque_snapshot`, `encode_snapshot`, `ProtocolError`) and delete the now-dead `RecordIndexEntry.draft` field only if the compiler flags it — otherwise leave.

- [ ] **Step 4: Run the memory store tests**

Run: `cargo test -p finstack-ai-store-memory`
Expected: PASS, including the Step 1 test and every pre-existing test (especially `load_from_sequence_returns_verified_tail_only` and `batch_and_record_idempotency_precede_sequence_checks`).

- [ ] **Step 5: Run the dependent conformance suites**

Run: `cargo test -p finstack-ai-test`
Expected: PASS

- [ ] **Step 6: Commit**

```bash
git add extensions/stores/finstack-ai-store-memory Cargo.lock
git commit -m "Rewire the memory store through finstack-ai-store-common"
```

---

### Task 7: Rewire the sqlite store through `finstack-ai-store-common`

**Files:**
- Modify: `extensions/stores/finstack-ai-store-sqlite/Cargo.toml`
- Modify: `extensions/stores/finstack-ai-store-sqlite/src/append.rs`
- Modify: `extensions/stores/finstack-ai-store-sqlite/src/load.rs`
- Modify: `extensions/stores/finstack-ai-store-sqlite/src/store.rs`
- Modify: `extensions/stores/finstack-ai-store-sqlite/src/error.rs`
- Modify: `extensions/stores/finstack-ai-store-sqlite/src/tests.rs`

**Interfaces:**
- Consumes: Tasks 3–5 exports; Task 2's golden test must stay green untouched.
- Produces: no public API change; one behavior fix — mid-batch tail-window starts now fail with the `split` code instead of silently returning a reconstructed split batch.

- [ ] **Step 1: Write the failing split-batch test**

Add to `src/tests.rs` (mirrors Task 6's test, but sqlite currently *succeeds* on the mid-batch case — that is the bug being fixed):

```rust
#[test]
fn tail_window_rejects_mid_batch_starts() {
    let store = memory_store();
    let first =
        block_on(store.append(request(1, 1, 1, vec![draft(1, 1), draft(2, 1)]))).expect("append");
    block_on(store.append(request(2, 1, 3, vec![draft(3, 1)]))).expect("append");
    assert!(matches!(
        block_on(store.load_from(LoadFromRequest {
            session_id: id::<SessionTag>(1),
            window: LoadWindow::FromSequence {
                from_sequence: 2,
                prior_checksum: first.records[0].checksum(),
            },
        })),
        Err(StoreError::Integrity {
            reason_code: "load_from_splits_batch"
        })
    ));
}
```

(`memory_store()` here is the sqlite test helper that opens a `:memory:` sqlite store — see the existing helpers at the top of `src/tests.rs`; if `LoadFromRequest`/`LoadWindow` are not yet imported there, add them.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p finstack-ai-store-sqlite tail_window_rejects_mid_batch_starts`
Expected: FAIL — the load currently succeeds with a split batch.

- [ ] **Step 3: Add the dependency and rewire**

`Cargo.toml`: add `finstack-ai-store-common = { workspace = true }` to `[dependencies]` (keep `serde` — the binaries still use it; remove it only if the compiler proves it unused).

`src/append.rs`:
1. Delete the local `AppendIdentity`, `request_identity`, `request_cbor`, and `build_committed_batch`; import them from `finstack_ai_store_common` and re-export for the crate: `pub(crate) use finstack_ai_store_common::{AppendIdentity, request_cbor};` (Task 2's golden test path `crate::append::request_cbor` keeps working; `load.rs` keeps decoding `AppendIdentity`).
2. Replace the record-reuse block with `classify_record_reuse`:

```rust
let mut hits = Vec::new();
for record in request.records() {
    if let Some(batch_id) = lookup_record_batch(transaction, record.record_id())? {
        hits.push(batch_id);
    }
}
if let Some(original_batch_id) = classify_record_reuse(&hits, request.records().len())? {
    let existing = load_batch(transaction, original_batch_id)?.ok_or(StoreError::Integrity {
        reason_code: "missing_record_batch_index",
    })?;
    let incoming = request_identity(request)?;
    if existing.identity.session_id == incoming.session_id
        && existing.identity.expected_sequence == incoming.expected_sequence
        && existing.identity.draft_cbor == incoming.draft_cbor
    {
        return Ok(existing.committed);
    }
    return Err(StoreError::Corruption {
        reason_code: "record_id_reuse",
    });
}
```

3. Replace the sequence check with `check_append_sequence(current_head, request.expected_sequence())?` and the limits block with:

```rust
let usage = session.as_ref().map(|row| SessionUsage {
    batches: row.batches,
    records: row.records,
});
let session_is_new = usage.is_none();
admit_append_limits(*limits, count_sessions(transaction)?, usage, request.records().len())?;
```

Note: the current code only calls `count_sessions` when the session is new; keep that behavior (one extra `COUNT(*)` per append is a real cost): call `count_sessions` only when `session_is_new`, passing `usage.map_or_else(|| count_sessions(transaction), |_| Ok(0))`-style, or simpler — keep the small `if session_is_new` guard around a dedicated `sessions`-limit call and pass `sessions: 0` for existing sessions (the sessions arm is only evaluated when `usage.is_none()`, so `admit_append_limits(*limits, if session_is_new { count_sessions(transaction)? } else { 0 }, usage, ...)` is exact and cheap).

`src/load.rs`:
1. Delete `encode_state_request`, `accelerated_from`, `outstanding_count`, `tombstone_count` (lines ~585-635); re-export from the shared crate for the crate-internal callers: `pub(crate) use finstack_ai_store_common::{accelerated_from, encode_state_request, outstanding_count, tombstone_count};`.
2. Rewrite `loaded_tail` to use `verify_tail_records` + `check_batch_alignment`, replacing the three code parameters with `WindowCodes`:

```rust
fn loaded_tail(
    connection: &Connection,
    session_id: SessionId,
    snapshot_bytes: usize,
    stored: &[StoredRecord],
    prior_checksum: Digest,
    start: u64,
    codes: WindowCodes,
) -> Result<LoadedSession, StoreError> {
    let (metadata, head_sequence, snapshot) =
        load_session_extras(connection, session_id, snapshot_bytes)?;
    if let Some(first) = stored.first() {
        let prior_batch = lookup_sequence_batch(connection, session_id, start.saturating_sub(1))?;
        check_batch_alignment(first.batch_id, prior_batch, codes)?;
    }
    let records = stored
        .iter()
        .map(|row| row.envelope.clone())
        .collect::<Vec<_>>();
    let stored_head = session_head_checksum(connection, session_id)?;
    verify_tail_records(&records, start, prior_checksum, head_sequence, stored_head, codes)?;
    let committed_batches = group_batches(stored)?;
    Ok(LoadedSession {
        session_id,
        head_sequence,
        head_checksum: stored_head,
        metadata,
        committed_batches: committed_batches.into(),
        snapshot: snapshot.clone(),
        accelerated: snapshot.as_ref().and_then(accelerated_from),
    })
}
```

Call sites pass `FROM_SEQUENCE_WINDOW` / `SNAPSHOT_WINDOW`. Add the small lookup:

```rust
fn lookup_sequence_batch(
    connection: &Connection,
    session_id: SessionId,
    sequence: u64,
) -> Result<Option<AppendBatchId>, StoreError> {
    if sequence == 0 {
        return Ok(None);
    }
    let bytes = connection
        .query_row(
            "SELECT batch_id FROM records WHERE session_id = ?1 AND sequence = ?2",
            params![
                session_id.as_bytes().as_slice(),
                i64_from_u64(sequence, "sequence")?
            ],
            |row| row.get::<_, Vec<u8>>(0),
        )
        .optional()
        .map_err(map_sqlite_error)?;
    bytes.as_deref().map(id_from_blob).transpose()
}
```

3. `verify_stored_session`: keep the SQL that reads the stored head, then delegate the chain work: `verify_full_head(&records, stored_head)`.
4. `scan_session`: replace the two limit checks with `validate_scan_limit(request.limit)?`, the start normalization with `scan_start(request.from_sequence)`, and the `next_sequence` computation with `scan_next_sequence(&records, has_more)`.

`src/store.rs`:
1. `write_snapshot`: replace the size check with `check_snapshot_size(...)` and the two sequence checks with `admit_snapshot_sequence(request.snapshot.sequence(), session.current_sequence, session.snapshot_sequence)?`.
2. `prune`: replace the `snapshot.sequence() == 0 || ... > current_sequence` check with `admit_prune_snapshot(snapshot.sequence(), session.current_sequence)?` (the SQL batch-alignment count and deletes stay).

`src/error.rs`: delete the local `protocol_error` and re-export: `pub(crate) use finstack_ai_store_common::protocol_error;` (keep the `i64_from_u64` family and `map_sqlite_error` — they are sqlite-specific).

- [ ] **Step 4: Run the sqlite tests**

Run: `cargo test -p finstack-ai-store-sqlite`
Expected: PASS — including Step 1's new test, Task 2's golden `append_identity_encoding_is_stable`, and all pre-existing tests.

- [ ] **Step 5: Run the conformance and integration suites**

Run: `cargo test -p finstack-ai-test`
Expected: PASS (crash-prefix, lanes, compaction, port conformance)

- [ ] **Step 6: Commit**

```bash
git add extensions/stores/finstack-ai-store-sqlite Cargo.lock
git commit -m "Rewire the sqlite store through finstack-ai-store-common and reject mid-batch window starts"
```

---

### Task 8: Workspace verification and changelog

**Files:**
- Modify: `CHANGELOG.md`

**Interfaces:**
- Consumes: all prior tasks.

- [ ] **Step 1: Verify no extracted function survives in either store**

Run: `grep -rn "fn build_committed_batch\|fn accelerated_from\|fn encode_state_request\|fn outstanding_count\|fn tombstone_count\|fn protocol_error" extensions/stores/finstack-ai-store-memory extensions/stores/finstack-ai-store-sqlite`
Expected: no matches (only `use`/re-export lines may mention the names).

- [ ] **Step 2: Full workspace check**

Run: `cargo test --workspace` and `cargo clippy --workspace --all-targets`
Expected: all tests PASS; clippy clean. (The Python/WASM binding crates compile but their behavior is untouched — no store API changed.)

- [ ] **Step 3: Add the changelog entry**

Add under the current unreleased section of `CHANGELOG.md`, matching its existing entry style:

```markdown
- Add `finstack-ai-store-common`: shared journal-store semantics (append
  admission, snapshot/prune admission, chain and window verification, scan
  validation) now used by both the memory and sqlite stores.
- Fix the sqlite store to reject tail-window loads that start mid-batch
  (`load_from_splits_batch` / `snapshot_splits_batch`) instead of returning a
  reconstructed batch that splits a committed one; unify the memory store's
  hole-at-start code to the `gap` reason codes.
```

- [ ] **Step 4: Commit**

```bash
git add CHANGELOG.md
git commit -m "Document the shared store-semantics crate and sqlite window fix"
```

---

## Self-Review Notes

- **Spec coverage:** duplication items 1–6 in the spec map to Tasks 3 (append), 3 (`build_committed_batch`), 4 (snapshot/prune), 5+6+7 (chain/window), 5 (scan), 1 (error mapping). The persisted-format invariant maps to Task 2; the two behavior unifications map to Task 6 Step 1 and Task 7 Steps 1–3; acceptance maps to Task 8.
- **Known judgment calls an executor may hit:**
  - Exact helper names on `Digest` in Task 2 — the plan names the fallback sources.
  - The memory store's `RecordIndexEntry.draft` field is `#[allow(dead_code)]` already; leave it.
  - If `cargo clippy` flags the unused `session_is_new` binding after rewiring (memory Task 6 Step 3.3), it is still needed for the `sessions.entry(...)` insert path — check before deleting.
  - `finstack-ai-runtime` deliberately keeps its own `trim_*` default-impl copies (it cannot depend on `finstack-ai-protocol`); do not try to rewire it.
