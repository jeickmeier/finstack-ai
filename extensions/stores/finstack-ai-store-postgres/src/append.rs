//! Multi-writer append: the spec D4 transaction protocol and the spec D5
//! ambiguous-acknowledgement outcome.
//!
//! ## Where the semantics live
//!
//! None of the store-contract *decisions* are made here. Idempotent replay,
//! record-id reuse classification, the sequence precondition, the limit
//! ceilings and the checksum chaining are all
//! [`finstack_ai_store_common`] functions, called in exactly the order the
//! sqlite store calls them in
//! `extensions/stores/finstack-ai-store-sqlite/src/append.rs`:
//!
//! 1. batch-id replay (equal `request_cbor` → replay, otherwise
//!    `Corruption{append_batch_id_reuse}`),
//! 2. empty batch (`InvalidRequest{empty_append_batch}`),
//! 3. record-id reuse (replay when the whole batch matches, otherwise
//!    `Corruption{record_id_reuse}` / the mixed-reuse codes),
//! 4. sequence precondition (`Conflict`),
//! 5. limits, in `sessions` → `batches_per_session` → `records_per_session`
//!    order.
//!
//! This module only supplies the storage plumbing — locking, reads, the
//! `UNNEST` batch insert, the CAS head update, and the counters — so the two
//! backends cannot drift in reason code or check ordering.
//!
//! ## Locking
//!
//! One `READ COMMITTED` transaction per append. `SELECT … FOR UPDATE` on the
//! session row serializes every writer of one session while leaving distinct
//! sessions contention-free; creating a session additionally locks the
//! single `store_totals` row so the `sessions` ceiling is enforced against a
//! stable count.
//!
//! Only the create path takes both locks, and it takes `store_totals`
//! *before* inserting the session row — so a lock cycle would need some
//! other path to insert a session row while holding no `store_totals` lock,
//! and none exists. The `40P01` → `Unavailable{postgres_serialization}`
//! mapping is therefore defense in depth (a deadlock introduced by a future
//! path, or by a lock Postgres takes on this transaction's behalf) rather
//! than the handling of an expected outcome; the caller retries either way.
//!
//! ## Transaction handling
//!
//! Uses `tokio_postgres::Client::transaction()` (which is why [`append`]
//! takes `&mut PooledClient`: `Deref`/`DerefMut` hand out the `&mut Client`
//! it needs) rather than the raw `BEGIN`/`COMMIT` style
//! [`crate::schema::ensure_schema`] uses. `ensure_schema` needs raw
//! statements because it serves concurrent callers over a shared `&Client`;
//! an append owns its checkout exclusively, so the typed transaction — with
//! its guaranteed rollback — is strictly better here. Error paths still
//! `rollback()` explicitly rather than relying on `Transaction::drop`, so
//! the connection is provably out of a transaction before it can be returned
//! to the pool.
//!
//! ## Connection disposition
//!
//! Every error carries a poison flag (see [`Failure`]). Server-reported
//! errors (those with a SQLSTATE, on a still-open connection) leave the
//! connection healthy after the rollback and return it to the pool; errors
//! with no SQLSTATE are IO, protocol, or codec failures on the wire, and
//! errors seen on an already-closed connection are the backend going away
//! under this transaction — both poison the checkout so it is dropped
//! instead of reused (spec D2). A `COMMIT` that fails without a
//! SQLSTATE is the spec D5 ambiguous acknowledgement: the transaction may or
//! may not be durable, so it maps to
//! [`StoreError::AmbiguousAcknowledgement`] and always poisons.

use finstack_ai_kernel::{
    AppendBatchId, AppendRequest, CommittedBatch, Digest, Metadata, RecordEnvelope, SessionId,
};
use finstack_ai_protocol::encode;
use finstack_ai_runtime::{StoreError, StoreLimits};
use finstack_ai_store_common::{
    SessionUsage, admit_append_limits, build_committed_batch, check_append_sequence,
    classify_record_reuse, protocol_error, request_cbor, request_identity,
};
use tokio_postgres::{Client, Transaction};

use crate::error::{Failure, i64_from_u64, u64_from_i64};
use crate::load::{digest_from_bytes, id_from_bytes, load_batch, usize_from_i64};
use crate::pool::PooledClient;

/// Append `request` on `client`, per spec D4.
///
/// The caller keeps ownership of the checkout; connections that must not be
/// reused are marked with [`PooledClient::poison`] here rather than
/// consumed, so a single signature covers both dispositions.
///
/// # Errors
///
/// Returns the store-common admission errors unchanged (`Conflict`,
/// `Corruption`, `InvalidRequest`, `LimitExceeded`),
/// `Integrity{sequence_cas_failed}` if the head moved under the row lock
/// (defense in depth — the lock should make this unreachable),
/// [`StoreError::AmbiguousAcknowledgement`] when the connection dies during
/// `COMMIT` (spec D5), and otherwise the mapped driver error — notably
/// `Unavailable{postgres_serialization}` for serialization failures and
/// deadlocks, which the *caller* retries; this function never retries
/// internally.
pub(crate) async fn append(
    client: &mut PooledClient<Client>,
    request: &AppendRequest,
    limits: &StoreLimits,
) -> Result<CommittedBatch, StoreError> {
    // `client` deref-coerces to the `&mut Client` this needs; the borrow
    // (and the transaction that borrows from it) ends with the statement,
    // before `poison` touches the checkout itself.
    let outcome = append_on_connection(client, request, limits).await;
    match outcome {
        Ok(batch) => Ok(batch),
        Err(failure) => {
            if failure.poison {
                client.poison();
            }
            Err(failure.error)
        }
    }
}

/// Drive one append transaction to `COMMIT` or `ROLLBACK`.
async fn append_on_connection(
    client: &mut Client,
    request: &AppendRequest,
    limits: &StoreLimits,
) -> Result<CommittedBatch, Failure> {
    let transaction = client
        .transaction()
        .await
        .map_err(|error| Failure::from_driver(&error))?;

    let committed = match append_in_transaction(&transaction, request, limits).await {
        Ok(committed) => committed,
        Err(mut failure) => {
            // The rollback must never shadow the original error, but a
            // rollback that could not be delivered leaves the connection
            // possibly still inside a transaction (or dead), so it must not
            // go back to the pool even when the original failure was purely
            // logical.
            if transaction.rollback().await.is_err() {
                failure.poison = true;
            }
            return Err(failure);
        }
    };

    match transaction.commit().await {
        Ok(()) => Ok(committed),
        // Spec D5: no SQLSTATE means the server never reported an outcome,
        // so the commit may or may not be durable. Report it as ambiguous
        // and discard the connection; the caller recovers by retrying the
        // same request, which either replays or commits it fresh.
        Err(error) if error.code().is_none() => Err(Failure {
            error: StoreError::AmbiguousAcknowledgement,
            poison: true,
        }),
        Err(error) => Err(Failure::from_driver(&error)),
    }
}

/// The spec D4 protocol body, inside the transaction.
async fn append_in_transaction(
    transaction: &Transaction<'_>,
    request: &AppendRequest,
    limits: &StoreLimits,
) -> Result<CommittedBatch, Failure> {
    let request_cbor = request_cbor(request)?;
    let mut session = lock_session(transaction, request.session_id()).await?;
    let mut session_created = false;

    // Runs at most twice: the second pass only happens when this writer lost
    // the session-create race, and it starts from a session row that is
    // now present and locked, so it cannot take the create path again.
    loop {
        if let Some(existing) = load_batch(transaction, request.batch_id()).await? {
            return if existing.request_cbor == request_cbor {
                Ok(existing.committed)
            } else {
                Err(StoreError::Corruption {
                    reason_code: "append_batch_id_reuse",
                }
                .into())
            };
        }

        if request.records().is_empty() {
            return Err(StoreError::InvalidRequest {
                reason_code: "empty_append_batch",
            }
            .into());
        }

        if let Some(replayed) = replay_by_record_reuse(transaction, request).await? {
            return Ok(replayed);
        }

        let current_head = session.as_ref().map_or(0, |row| row.current_sequence);
        check_append_sequence(current_head, request.expected_sequence())?;

        if let Some(row) = session.as_ref() {
            admit_append_limits(
                *limits,
                // Unused for an existing session, exactly as sqlite passes
                // 0 here.
                0,
                Some(SessionUsage {
                    batches: row.batch_count,
                    records: row.record_count,
                }),
                request.records().len(),
            )?;
            break;
        }

        let sessions = lock_store_totals(transaction).await?;
        admit_append_limits(*limits, sessions, None, request.records().len())?;
        let created = create_session(transaction, request.session_id()).await?;
        session = lock_session(transaction, request.session_id()).await?;
        if session.is_none() {
            // Unreachable in practice: the row was either inserted here or
            // by the writer that won the race and committed before this
            // re-select. Fail closed rather than loop.
            return Err(StoreError::Integrity {
                reason_code: "missing_session_row",
            }
            .into());
        }
        if created {
            session_created = true;
            break;
        }
        // Lost the race: proceed exactly as if the session had existed all
        // along (spec D4 step 2).
    }

    let session = session.ok_or(StoreError::Integrity {
        reason_code: "missing_session_row",
    })?;
    let committed = build_committed_batch(request, session.head_checksum)?;
    persist_committed_batch(transaction, request, &request_cbor, &committed).await?;
    if session_created {
        bump_session_count(transaction).await?;
    }
    Ok(committed)
}

/// The session row's committed footprint, read under `FOR UPDATE`.
struct SessionRow {
    /// Sequence of the journal head (0 for a session with no records).
    current_sequence: u64,
    /// Checksum of the head record, `None` before the first append.
    head_checksum: Option<Digest>,
    /// Committed batches in this session.
    batch_count: usize,
    /// Committed records in this session.
    record_count: usize,
}

/// Take the per-session write lock, returning the row when it exists.
///
/// This is the serialization point for all writers of one session: two
/// appends to the same session queue here, while appends to different
/// sessions never contend.
async fn lock_session(
    transaction: &Transaction<'_>,
    session_id: SessionId,
) -> Result<Option<SessionRow>, Failure> {
    let row = transaction
        .query_opt(
            "SELECT current_sequence, head_checksum, batch_count, record_count \
             FROM sessions WHERE session_id = $1 FOR UPDATE",
            &[&session_id.as_bytes().as_slice()],
        )
        .await
        .map_err(|error| Failure::from_driver(&error))?;
    let Some(row) = row else {
        return Ok(None);
    };
    let head_checksum: Option<Vec<u8>> = row.get(1);
    Ok(Some(SessionRow {
        current_sequence: u64_from_i64(row.get(0), "current_sequence")?,
        head_checksum: head_checksum
            .as_deref()
            .map(digest_from_bytes)
            .transpose()?,
        batch_count: usize_from_i64(row.get(2), "batch_count")?,
        record_count: usize_from_i64(row.get(3), "record_count")?,
    }))
}

/// Lock the store-wide counters row and return the current session count.
///
/// Held for the rest of the transaction, so the `sessions` ceiling is
/// enforced against a count no concurrent creator can move underneath it.
async fn lock_store_totals(transaction: &Transaction<'_>) -> Result<usize, Failure> {
    let row = transaction
        .query_one(
            "SELECT session_count FROM store_totals WHERE id = TRUE FOR UPDATE",
            &[],
        )
        .await
        .map_err(|error| Failure::from_driver(&error))?;
    Ok(usize_from_i64(row.get(0), "session_count")?)
}

/// Insert the session row, returning `true` when *this* writer created it.
///
/// `ON CONFLICT DO NOTHING` plus the caller's re-`SELECT … FOR UPDATE`
/// closes the create race (spec D4 step 2). A concurrent creator is
/// serialized before that: it must hold the single `store_totals` row lock
/// to reach this statement at all, so two creators queue rather than
/// deadlock, and the loser simply observes the committed row on its
/// re-select. (`40P01` still maps to `Unavailable{postgres_serialization}`
/// as defense in depth — see the module doc comment.)
async fn create_session(
    transaction: &Transaction<'_>,
    session_id: SessionId,
) -> Result<bool, Failure> {
    let inserted = transaction
        .execute(
            "INSERT INTO sessions \
             (session_id, current_sequence, head_checksum, snapshot_sequence, metadata, \
              batch_count, record_count) \
             VALUES ($1, 0, NULL, NULL, $2, 0, 0) \
             ON CONFLICT (session_id) DO NOTHING",
            &[
                &session_id.as_bytes().as_slice(),
                &Metadata::empty().as_bytes(),
            ],
        )
        .await
        .map_err(|error| Failure::from_driver(&error))?;
    Ok(inserted == 1)
}

/// Increment the store-wide session count. Only called by the writer that
/// actually created the session row, under the `store_totals` lock it took
/// in [`lock_store_totals`].
async fn bump_session_count(transaction: &Transaction<'_>) -> Result<(), Failure> {
    transaction
        .execute(
            "UPDATE store_totals SET session_count = session_count + 1 WHERE id = TRUE",
            &[],
        )
        .await
        .map_err(|error| Failure::from_driver(&error))?;
    Ok(())
}

/// Classify record-id reuse and, when the whole request was already
/// committed under a different batch id, replay that batch.
///
/// `Ok(None)` means the records are fresh and the append proceeds.
async fn replay_by_record_reuse(
    transaction: &Transaction<'_>,
    request: &AppendRequest,
) -> Result<Option<CommittedBatch>, Failure> {
    let record_ids = request
        .records()
        .iter()
        .map(|draft| draft.record_id().to_bytes().to_vec())
        .collect::<Vec<_>>();
    // A join against the *request's* array (rather than
    // `WHERE record_id = ANY($1)`) preserves multiplicity: a request that
    // names the same record id twice yields two hits, exactly as sqlite's
    // per-record lookup loop does. `classify_record_reuse` compares the hit
    // count against the request's record count, so collapsing duplicates
    // here would report `mixed_record_id_reuse` where sqlite reports a
    // clean replay.
    let rows = transaction
        .query(
            "SELECT wanted.record_id, records.batch_id \
             FROM UNNEST($1::bytea[]) AS wanted(record_id) \
             JOIN records ON records.record_id = wanted.record_id",
            &[&record_ids],
        )
        .await
        .map_err(|error| Failure::from_driver(&error))?;
    let hits = rows
        .iter()
        .map(|row| {
            let batch_id: Vec<u8> = row.get(1);
            id_from_bytes(&batch_id)
        })
        .collect::<Result<Vec<AppendBatchId>, _>>()?;

    let Some(original_batch_id) = classify_record_reuse(&hits, request.records().len())? else {
        return Ok(None);
    };
    let existing =
        load_batch(transaction, original_batch_id)
            .await?
            .ok_or(StoreError::Integrity {
                reason_code: "missing_record_batch_index",
            })?;
    let incoming = request_identity(request)?;
    if existing.identity.session_id == incoming.session_id
        && existing.identity.expected_sequence == incoming.expected_sequence
        && existing.identity.draft_cbor == incoming.draft_cbor
    {
        return Ok(Some(existing.committed));
    }
    Err(StoreError::Corruption {
        reason_code: "record_id_reuse",
    }
    .into())
}

/// Write the batch row, its records, and the session head/counters.
async fn persist_committed_batch(
    transaction: &Transaction<'_>,
    request: &AppendRequest,
    request_cbor: &[u8],
    committed: &CommittedBatch,
) -> Result<(), Failure> {
    transaction
        .execute(
            "INSERT INTO batches \
             (batch_id, session_id, first_sequence, last_sequence, expected_sequence, \
              request_cbor) \
             VALUES ($1, $2, $3, $4, $5, $6)",
            &[
                &request.batch_id().as_bytes().as_slice(),
                &request.session_id().as_bytes().as_slice(),
                &i64_from_u64(committed.first_sequence, "first_sequence")?,
                &i64_from_u64(committed.last_sequence, "last_sequence")?,
                &i64_from_u64(request.expected_sequence(), "expected_sequence")?,
                &request_cbor,
            ],
        )
        .await
        .map_err(|error| Failure::from_driver(&error))?;

    insert_records(transaction, request, committed).await?;
    update_session_head(transaction, request, committed).await
}

/// Insert every record of the batch in one round trip.
///
/// The column values are transposed into parallel arrays and re-expanded
/// server-side with `UNNEST`, whose multi-array form yields one row per
/// index in array order. Encodings mirror sqlite's `insert_record`
/// column-for-column (ids as 16-byte `BYTEA`, digests as 32-byte `BYTEA`,
/// bodies and derived-event lists as canonical CBOR, timestamps as unix
/// milliseconds, `committed_at` left `NULL`) so a batch written by either
/// backend rehydrates to the same envelopes.
async fn insert_records(
    transaction: &Transaction<'_>,
    request: &AppendRequest,
    committed: &CommittedBatch,
) -> Result<(), Failure> {
    let count = committed.records.len();
    let mut sequences = Vec::with_capacity(count);
    let mut record_ids = Vec::with_capacity(count);
    let mut lane_ids = Vec::with_capacity(count);
    let mut run_ids: Vec<Option<Vec<u8>>> = Vec::with_capacity(count);
    let mut kinds = Vec::with_capacity(count);
    let mut format_versions = Vec::with_capacity(count);
    let mut kind_versions = Vec::with_capacity(count);
    let mut payloads = Vec::with_capacity(count);
    let mut timestamps = Vec::with_capacity(count);
    let mut payload_digests = Vec::with_capacity(count);
    let mut previous_checksums: Vec<Option<Vec<u8>>> = Vec::with_capacity(count);
    let mut checksums = Vec::with_capacity(count);
    let mut derived_event_ids = Vec::with_capacity(count);

    for envelope in committed.records.iter() {
        sequences.push(i64_from_u64(envelope.sequence(), "sequence")?);
        record_ids.push(envelope.record_id().to_bytes().to_vec());
        lane_ids.push(envelope.lane_id().to_bytes().to_vec());
        run_ids.push(envelope.run_id().map(|id| id.to_bytes().to_vec()));
        kinds.push(envelope.body().kind_name().to_owned());
        format_versions.push(i32::from(envelope.format_version()));
        kind_versions.push(i32::from(envelope.kind_version()));
        payloads.push(encode(envelope.body()).map_err(protocol_error)?);
        timestamps.push(envelope.timestamp().as_unix_ms());
        payload_digests.push(envelope.payload_digest().as_bytes().to_vec());
        previous_checksums.push(
            envelope
                .previous_checksum()
                .map(|digest| digest.as_bytes().to_vec()),
        );
        checksums.push(envelope.checksum().as_bytes().to_vec());
        derived_event_ids.push(encode(&envelope.derived_event_ids()).map_err(protocol_error)?);
    }

    transaction
        .execute(
            "INSERT INTO records ( \
                session_id, sequence, record_id, batch_id, lane_id, run_id, kind, \
                format_version, kind_version, payload_cbor, timestamp, committed_at, \
                payload_digest, previous_checksum, envelope_checksum, derived_event_ids) \
             SELECT $1, sequence, record_id, $2, lane_id, run_id, kind, \
                    format_version, kind_version, payload_cbor, timestamp, NULL, \
                    payload_digest, previous_checksum, envelope_checksum, derived_event_ids \
             FROM UNNEST( \
                $3::bigint[], $4::bytea[], $5::bytea[], $6::bytea[], $7::text[], \
                $8::int4[], $9::int4[], $10::bytea[], $11::bigint[], $12::bytea[], \
                $13::bytea[], $14::bytea[], $15::bytea[]) \
             AS unnested( \
                sequence, record_id, lane_id, run_id, kind, format_version, kind_version, \
                payload_cbor, timestamp, payload_digest, previous_checksum, \
                envelope_checksum, derived_event_ids)",
            &[
                &request.session_id().as_bytes().as_slice(),
                &request.batch_id().as_bytes().as_slice(),
                &sequences,
                &record_ids,
                &lane_ids,
                &run_ids,
                &kinds,
                &format_versions,
                &kind_versions,
                &payloads,
                &timestamps,
                &payload_digests,
                &previous_checksums,
                &checksums,
                &derived_event_ids,
            ],
        )
        .await
        .map_err(|error| Failure::from_driver(&error))?;
    Ok(())
}

/// CAS the session head forward and maintain its committed footprint.
///
/// The `WHERE current_sequence = $prev` predicate is defense in depth: the
/// row lock taken in [`lock_session`] already excludes every other writer of
/// this session, so a zero-row update means an invariant broke rather than a
/// lost race.
async fn update_session_head(
    transaction: &Transaction<'_>,
    request: &AppendRequest,
    committed: &CommittedBatch,
) -> Result<(), Failure> {
    let head_checksum = committed
        .records
        .last()
        .map(RecordEnvelope::checksum)
        .ok_or(StoreError::Integrity {
            reason_code: "empty_committed_batch",
        })?;
    let previous_sequence =
        request
            .expected_sequence()
            .checked_sub(1)
            .ok_or(StoreError::Integrity {
                reason_code: "sequence_exhausted",
            })?;
    let record_count =
        i64::try_from(committed.records.len()).map_err(|_| StoreError::Integrity {
            reason_code: "record_count_overflow",
        })?;

    let updated = transaction
        .execute(
            "UPDATE sessions \
             SET current_sequence = $1, head_checksum = $2, \
                 batch_count = batch_count + 1, record_count = record_count + $3 \
             WHERE session_id = $4 AND current_sequence = $5",
            &[
                &i64_from_u64(committed.last_sequence, "last_sequence")?,
                &head_checksum.as_bytes().as_slice(),
                &record_count,
                &request.session_id().as_bytes().as_slice(),
                &i64_from_u64(previous_sequence, "previous_sequence")?,
            ],
        )
        .await
        .map_err(|error| Failure::from_driver(&error))?;
    if updated != 1 {
        return Err(StoreError::Integrity {
            reason_code: "sequence_cas_failed",
        }
        .into());
    }
    Ok(())
}
