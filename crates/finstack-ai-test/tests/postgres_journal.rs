//! Server-gated conformance battery for `PostgresJournalStore` (spec D7/D10).
//!
//! Three proofs, all skipping with a notice (exit 0) when
//! `FINSTACK_PG_TEST_URL` is unset:
//!
//! 1. the shared single-port `JournalStore` conformance case runs against a
//!    real Postgres store, so the port contract is proved by the *same*
//!    battery the memory and sqlite stores answer to;
//! 2. every activated journal-v1 record body (all 40 families) round-trips
//!    through append + load byte-equal, envelope CBOR and payload digests
//!    included;
//! 3. the crash-prefix restart-replay row — drive a run to a pending model
//!    request through the coordinator, drop the coordinator, recover, prune,
//!    recover again — mirroring `crash_prefix/sqlite.rs`'s
//!    `sqlite_v1_opens_prunes_and_process_kill_stays_separate`.
//!
//! The crash-prefix fixtures are reused verbatim by pointing a module at the
//! existing `crash_prefix/helpers/mod.rs`: those helpers are already store
//! agnostic (`accept_run`/`recover`/`write_snapshot` all take
//! `Arc<dyn JournalStore>`), so the postgres row needs no change to the shared
//! crash-prefix module. Only a fraction of the helper catalog is used here,
//! hence the `dead_code` allowance on the module.

#![allow(
    clippy::large_futures,
    reason = "coordinator fixtures are large; boxing would hide the flow"
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use finstack_ai_kernel::{
    AppendBatchTag, AppendRequest, Digest, EffectCancelled, EffectCompleted, EffectFailed,
    EffectInput, EffectKind, EffectOutputContract, EffectOutputKind, EffectRequested,
    ErrorCategory, ErrorDescriptor, ProviderIds, RawJson, RecordBody, RetrySafety, SessionTag,
};
use finstack_ai_runtime::ports::journal::{JournalStore, LoadRequest, PruneRequest, StoreLimits};
use finstack_ai_store_postgres::{PostgresJournalStore, PostgresStoreConfig};
use finstack_ai_test::store_fixtures::{draft, request};
use finstack_ai_test::{
    JournalStoreConformanceCase, LegalRestore, all_activated_record_bodies,
    check_journal_store_conformance, draft_for_body, known_answer_for_body,
    known_answer_for_envelope,
};

#[path = "crash_prefix/helpers/mod.rs"]
#[allow(
    dead_code,
    reason = "only the restart-replay row of the catalog is used here"
)]
mod helpers;

// ---------------------------------------------------------------------------
// Local server gating (kept in this test binary: `finstack-ai-test`'s public
// API must not grow a postgres-shaped surface for one integration suite)
// ---------------------------------------------------------------------------

/// Environment variable naming a live Postgres server, e.g.
/// `postgres://postgres:postgres@localhost:5432/postgres`.
const PG_TEST_URL_VAR: &str = "FINSTACK_PG_TEST_URL";

fn pg_test_url() -> Option<String> {
    std::env::var(PG_TEST_URL_VAR).ok()
}

/// Disposable schema name, unique within this process.
fn fresh_schema_name() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.subsec_nanos())
        .unwrap_or_default();
    format!("fa_test_{pid:x}_{nanos:x}_{counter:x}")
}

fn wide_limits() -> StoreLimits {
    StoreLimits {
        sessions: 64,
        batches_per_session: 512,
        records_per_session: 2_048,
        snapshot_bytes: 1_000_000,
    }
}

async fn open_store(url: &str, schema: &str) -> PostgresJournalStore {
    let mut config = PostgresStoreConfig::new(url, wide_limits());
    config.schema = Arc::from(schema);
    PostgresJournalStore::try_open(config)
        .await
        .expect("open postgres journal store")
}

/// Drop the disposable schema over a throwaway connection.
///
/// Cleanup is explicit (not a `Drop` guard) for the same reason the postgres
/// crate's own test helper spells it out: dropping a schema is an async
/// round-trip, and a failed assertion should surface as itself rather than as
/// a double panic during unwind. A leaked `fa_test_*` schema after a failing
/// test is the accepted, visible trade-off.
async fn drop_schema(url: &str, schema: &str) {
    let (client, connection) = tokio_postgres::connect(url, tokio_postgres::NoTls)
        .await
        .expect("connect for schema cleanup");
    let driver = tokio::spawn(async move {
        let _ = connection.await;
    });
    client
        .batch_execute(&format!("DROP SCHEMA IF EXISTS {schema} CASCADE"))
        .await
        .expect("drop disposable schema");
    drop(client);
    let _ = driver.await;
}

// ---------------------------------------------------------------------------
// (a) Port conformance
// ---------------------------------------------------------------------------

/// The shared `JournalStore` port battery — readiness, atomic append, equal
/// retry idempotency, and load-contains-the-committed-batch — over a real
/// Postgres store.
#[tokio::test]
async fn postgres_journal_store_answers_the_port_conformance_battery() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let schema = fresh_schema_name();
    let store = open_store(&url, &schema).await;

    let committed = check_journal_store_conformance(
        &store,
        JournalStoreConformanceCase {
            request: request(20, 1, 1, vec![draft(10, 1)]),
            expected_first_sequence: 1,
            expected_last_sequence: 1,
        },
    )
    .await
    .expect("postgres journal store conformance");
    assert_eq!(committed.records.len(), 1);
    assert_eq!(committed.first_sequence, 1);
    assert_eq!(committed.last_sequence, 1);

    drop(store);
    drop_schema(&url, &schema).await;
}

// ---------------------------------------------------------------------------
// (b) Journal-v1 corpus
// ---------------------------------------------------------------------------

/// The `EffectOutputContract` an interaction-shaped effect carries.
fn interaction_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::InteractionResolution,
        schema_version: 1,
        schema_digest: Digest::raw_json(b"{}"),
    }
}

/// The partner record an interaction body needs to be *batch*-legal, if any.
///
/// `AppendRequest::try_new` enforces the kernel's interaction pairing rules
/// over the batch: an `InteractionRequested` must be accompanied by the
/// matching `EffectRequested`, and resolved/expired/cancelled interactions
/// must balance against the corresponding interaction-resolution effect
/// outcome. The journal-v1 corpus samples each family in isolation, so four of
/// the forty bodies only become appendable next to a synthesized partner.
/// Those partners ride along in the same batch and round-trip with it; the
/// assertions below still pin the *corpus* body, which is always the batch's
/// first record.
fn interaction_partner(body: &RecordBody, ordinal: u64) -> Option<RecordBody> {
    let effect_id = |offset: u64| helpers::id(700 + ordinal + offset);
    match body {
        RecordBody::InteractionRequested(request) => Some(RecordBody::EffectRequested(
            EffectRequested::try_new(
                request.effect_id(),
                EffectKind::Interaction,
                None,
                None,
                None,
                interaction_contract(),
                EffectInput::Interaction {
                    interaction_id: request.interaction_id(),
                    request_digest: request.request_digest().expect("request digest"),
                },
                RetrySafety::SafeToRetry,
                None,
            )
            .expect("interaction effect partner"),
        )),
        RecordBody::InteractionResolved(_) => Some(RecordBody::EffectCompleted(
            EffectCompleted::try_new(
                effect_id(0),
                interaction_contract(),
                RawJson::parse("{}").expect("output"),
                None,
                Vec::new(),
                ProviderIds::empty(),
                None::<&str>,
                None,
            )
            .expect("interaction completion partner"),
        )),
        RecordBody::InteractionExpired(_) => Some(RecordBody::EffectFailed(
            EffectFailed::try_new(
                effect_id(1),
                interaction_contract(),
                ErrorDescriptor::new(
                    "interaction_expired",
                    "expired",
                    ErrorCategory::Deadline,
                    false,
                )
                .expect("descriptor"),
                None,
                None::<&str>,
            )
            .expect("interaction failure partner"),
        )),
        RecordBody::InteractionCancelled(_) => Some(RecordBody::EffectCancelled(
            EffectCancelled::try_new(
                effect_id(2),
                interaction_contract(),
                None::<&str>,
                None::<&str>,
            )
            .expect("interaction cancellation partner"),
        )),
        _ => None,
    }
}

/// Every activated record-body family survives a Postgres append/load
/// round-trip byte-equal.
///
/// The proof is deliberately stronger than `loaded == appended`: each loaded
/// envelope is re-encoded through the journal known-answer helpers, so the
/// assertion covers the *bytes* (payload CBOR, payload digest, envelope
/// checksum, envelope CBOR) rather than a structural comparison that a lossy
/// column type could still satisfy.
#[tokio::test]
async fn postgres_round_trips_every_journal_v1_record_body_byte_equal() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let schema = fresh_schema_name();
    let store = open_store(&url, &schema).await;

    let bodies = all_activated_record_bodies().expect("activated record bodies");
    assert_eq!(
        bodies.len(),
        40,
        "journal-v1 corpus is the 40 activated families"
    );

    let mut appended = Vec::with_capacity(bodies.len());
    let mut expected_payloads = Vec::with_capacity(bodies.len());
    let mut next_sequence = 1_u64;
    for (index, body) in bodies.into_iter().enumerate() {
        let ordinal = u64::try_from(index).expect("ordinal fits u64") + 1;
        expected_payloads.push(known_answer_for_body(&body).expect("body known answer"));

        let mut drafts = vec![draft_for_body(body.clone(), ordinal).expect("draft for body")];
        if let Some(partner) = interaction_partner(&body, ordinal) {
            drafts.push(draft_for_body(partner, 100 + ordinal).expect("draft for partner"));
        }
        let records = u64::try_from(drafts.len()).expect("batch size fits u64");

        let request = AppendRequest::try_new(
            helpers::id::<AppendBatchTag>(500 + ordinal),
            helpers::id::<SessionTag>(1),
            next_sequence,
            drafts,
        )
        .expect("corpus append request");
        appended.push(store.append(request).await.expect("append corpus body"));
        next_sequence += records;
    }
    let total_records = next_sequence - 1;
    assert_eq!(
        total_records, 44,
        "40 corpus bodies plus the 4 interaction pairing partners"
    );

    let loaded = store
        .load(LoadRequest {
            session_id: helpers::id::<SessionTag>(1),
        })
        .await
        .expect("load corpus session");
    assert_eq!(loaded.head_sequence, total_records);
    assert_eq!(loaded.committed_batches.len(), 40);

    for (index, (committed, batch)) in appended
        .iter()
        .zip(loaded.committed_batches.iter())
        .enumerate()
    {
        assert_eq!(committed, batch, "batch {index} differs after reload");
        let envelope = batch.records.first().expect("one record per corpus batch");
        let origin = committed
            .records
            .first()
            .expect("one record per corpus batch");

        let (payload_digest_hex, payload_cbor_hex) = &expected_payloads[index];
        let (loaded_payload_digest, loaded_checksum, loaded_envelope_cbor) =
            known_answer_for_envelope(envelope).expect("loaded envelope known answer");
        let (origin_payload_digest, origin_checksum, origin_envelope_cbor) =
            known_answer_for_envelope(origin).expect("appended envelope known answer");

        assert_eq!(
            &loaded_payload_digest, payload_digest_hex,
            "batch {index} payload digest drifted from the body known answer"
        );
        assert_eq!(loaded_payload_digest, origin_payload_digest);
        assert_eq!(loaded_checksum, origin_checksum);
        assert_eq!(
            loaded_envelope_cbor, origin_envelope_cbor,
            "batch {index} envelope CBOR is not byte-equal after reload"
        );
        assert_eq!(
            known_answer_for_body(envelope.body())
                .expect("loaded body known answer")
                .1,
            *payload_cbor_hex,
            "batch {index} payload CBOR is not byte-equal after reload"
        );
    }

    drop(store);
    drop_schema(&url, &schema).await;
}

// ---------------------------------------------------------------------------
// (c) Restart-replay (crash-prefix row W2 over postgres)
// ---------------------------------------------------------------------------

/// Postgres mirror of `sqlite_v1_opens_prunes_and_process_kill_stays_separate`:
/// dropping the coordinator mid-run restores to a legal `Retryable` phase, and
/// a snapshot-aligned prune leaves the pending model effect replayable.
#[tokio::test]
async fn postgres_v1_opens_prunes_and_replays_the_pending_model_effect() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let schema = fresh_schema_name();
    let store: Arc<PostgresJournalStore> = Arc::new(open_store(&url, &schema).await);
    let port = Arc::clone(&store) as Arc<dyn JournalStore>;

    let mut coordinator = helpers::accept_run(Arc::clone(&port)).await;
    helpers::drive_to_model_request(&mut coordinator).await;
    helpers::write_snapshot(&port, &coordinator).await;
    drop(coordinator);

    let recovered = helpers::recover(Arc::clone(&port)).await;
    helpers::assert_legal(
        "postgres-W2",
        recovered.state().phase,
        LegalRestore::Retryable,
    );
    drop(recovered);

    store
        .prune(PruneRequest {
            session_id: helpers::id::<SessionTag>(1),
            horizon: helpers::horizon(),
        })
        .await
        .expect("postgres prune");

    let recovered = helpers::recover(Arc::clone(&port)).await;
    assert!(
        recovered.state().pending_model_effect.is_some(),
        "the pruned prefix must still replay the pending model effect"
    );

    drop(recovered);
    drop(port);
    drop(store);
    drop_schema(&url, &schema).await;
}
