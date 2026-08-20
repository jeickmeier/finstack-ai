//! Server-gated integration tests for snapshot-aligned prefix prune.
//!
//! Every test skips with a notice (exit 0) when `FINSTACK_PG_TEST_URL` is
//! unset, per the crate's env-gated testing convention (spec D10). Reason
//! codes and the deletion predicate mirror
//! `SqliteJournalStore::prune`/`MemoryJournalStore::prune_sync`
//! (`extensions/stores/finstack-ai-store-sqlite/src/store.rs`,
//! `extensions/stores/finstack-ai-store-memory/src/lib.rs`) exactly: both
//! backends delegate admission and receipt counting to
//! `finstack-ai-store-common`, so a divergence here is a bug in this crate's
//! orchestration, not a legitimate backend difference.

mod helpers;

use std::sync::Arc;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, BudgetPropagation, CancellationPropagation, CompletionIdentity,
    DeadlinePropagation, Digest, EffectTag, InteractionTag, KernelInput, LaneTag,
    ModelSettlementFingerprint, ModelSettlementKind, PrincipalPropagation, PrincipalRef,
    ResolutionIdentity, RunAccepted, RunLimits, RunPropagationPolicy, RunRelation,
    RunSecurityContext, RunTag, SessionTag, Timestamp, TransitionEnv,
};
use finstack_ai_protocol::encode_snapshot;
use finstack_ai_runtime::{
    CommitCoordinator, IdempotencyHorizon, JournalStore, LoadFromRequest, LoadWindow,
    OpaqueSnapshot, PruneRequest, SnapshotRequest, StoreError, StoreLimits,
};
use finstack_ai_store_postgres::{PostgresJournalStore, PostgresStoreConfig};
use finstack_ai_test::store_fixtures::{draft, id, request};

use helpers::{SchemaGuard, connect, disposable_store, fresh_schema_name, pg_test_url};

/// Generous limits: these tests care about prune logic, not ceilings.
fn wide_limits() -> StoreLimits {
    StoreLimits {
        sessions: 1_000,
        batches_per_session: 1_000,
        records_per_session: 1_000,
        snapshot_bytes: 1_000_000,
    }
}

/// Open a store against an explicit schema name, so a second store handle
/// can be opened onto the same schema (for the concurrency test).
async fn open_store(url: &str, schema: &str) -> PostgresJournalStore {
    let mut config = PostgresStoreConfig::new(url, wide_limits());
    config.schema = Arc::from(schema);
    PostgresJournalStore::try_open(config)
        .await
        .expect("try_open store")
}

/// Submit a real `AcceptRun` to session 1 through a [`CommitCoordinator`],
/// leaving the coordinator (and its verified [`finstack_ai_kernel::KernelState`])
/// alive so a caller can read `coordinator.state()` before driving any more
/// history directly through the store. Mirrors
/// `snapshot_scan_metadata.rs::accept_root_run` exactly (same fixture
/// inputs), except it hands back the coordinator instead of dropping it.
async fn accept_root_run(store: &Arc<PostgresJournalStore>) -> CommitCoordinator {
    let mut coordinator = CommitCoordinator::new(Arc::clone(store) as Arc<dyn JournalStore>);
    let run_id = id::<RunTag>(3);
    coordinator
        .submit(
            TransitionEnv {
                now: Timestamp::from_unix_ms(1_000).expect("ts"),
                ids: AllocatedIds::try_new(
                    vec![id(1)],
                    vec![id(1)],
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    vec![id(101)],
                    Vec::new(),
                )
                .expect("ids"),
            },
            KernelInput::AcceptRun(AcceptRun {
                session_id: id::<SessionTag>(1),
                lane_id: id::<LaneTag>(2),
                accepted: RunAccepted::try_new(
                    run_id,
                    RunRelation::root(run_id).expect("relation"),
                    RunSecurityContext::try_new(
                        "tenant-a",
                        PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                            .expect("principal"),
                        "oidc",
                        "high",
                        "policy-v1",
                        "decision-v1",
                        None,
                    )
                    .expect("security"),
                    None,
                    RunLimits::empty(),
                    RunPropagationPolicy {
                        cancellation: CancellationPropagation::Cascade,
                        deadline: DeadlinePropagation::MinimumOfParentAndChild,
                        budget: BudgetPropagation::SharedScope,
                        principal: PrincipalPropagation::Inherit,
                    },
                    Digest::raw_json(br#"{"agent":"fixture"}"#),
                    None,
                )
                .expect("accepted"),
            }),
        )
        .await
        .expect("accept");
    coordinator
}

/// Build a scripted three-batch session and a snapshot aligned on the second
/// batch: batch 1 is a real `AcceptRun` (sequence 1), batches 2 and 3 are
/// plain appended records (sequences 2 and 3). The snapshot at sequence 2
/// carries the coordinator's real (validated) state after `AcceptRun`, with
/// four synthetic tombstone entries added (two completion identities, one
/// resolution identity, one model settlement) so [`PruneReceipt`]'s
/// `retained_tombstones` has a nontrivial, independently-computable value —
/// `finstack_ai_store_common::tombstone_count` sums exactly those four map
/// lengths, so `4` is not a guess, it is what that function computes over
/// this state.
///
/// Returns the store (wrapped for [`CommitCoordinator`]), its schema name,
/// a guard for that schema, and the checksum of the record at sequence 2
/// (the snapshot's head checksum, needed to verify the post-prune tail). The
/// schema name is returned (rather than using [`disposable_store`], which
/// hides it) so a caller can open a second, independent
/// [`PostgresJournalStore`] onto the same schema.
async fn scripted_session_with_aligned_snapshot(
    url: &str,
) -> (Arc<PostgresJournalStore>, String, SchemaGuard, Digest) {
    let schema = fresh_schema_name();
    let store = Arc::new(open_store(url, &schema).await);
    let guard = SchemaGuard::new(schema.clone());

    let coordinator = accept_root_run(&store).await;
    let mut state = coordinator.state().clone();
    drop(coordinator);

    store
        .append(request(2, 1, 2, vec![draft(2, 1)]))
        .await
        .expect("batch two");
    let batch_three = store
        .append(request(3, 1, 3, vec![draft(3, 1)]))
        .await
        .expect("batch three");
    let _ = &batch_three;

    // The record at sequence 2 is what the synthetic snapshot below claims
    // to cover; its checksum is the snapshot's `head_checksum`.
    let head_checksum_at_two = {
        let loaded = store
            .load(finstack_ai_runtime::LoadRequest {
                session_id: id::<SessionTag>(1),
            })
            .await
            .expect("load before snapshot");
        loaded
            .committed_batches
            .iter()
            .flat_map(|batch| batch.records.iter())
            .find(|record| record.sequence() == 2)
            .expect("sequence 2 committed")
            .checksum()
    };

    // Decouple the snapshot's claimed sequence from the coordinator's real
    // `last_applied_sequence` (1, since only `AcceptRun` went through it):
    // `write_snapshot`'s admission only checks the sequence numerically
    // against the journal head, so a state whose `last_applied_sequence` is
    // overridden to match a later batch boundary is exactly what a real
    // `write_state_snapshot` caller would have produced had it recovered
    // through sequence 2.
    state.last_applied_sequence = 2;
    state.state_version = state.state_version.max(6);
    state.completion_identities.insert(
        std::sync::Arc::from("completion-a"),
        CompletionIdentity {
            effect_id: id::<EffectTag>(9_001),
            settlement_digest: Digest::raw_json(b"{}"),
        },
    );
    state.completion_identities.insert(
        std::sync::Arc::from("completion-b"),
        CompletionIdentity {
            effect_id: id::<EffectTag>(9_002),
            settlement_digest: Digest::raw_json(b"{}"),
        },
    );
    state.resolution_identities.insert(
        std::sync::Arc::from("resolution-a"),
        ResolutionIdentity {
            interaction_id: id::<InteractionTag>(9_003),
            settlement_digest: Digest::raw_json(b"{}"),
        },
    );
    state.model_settlements.insert(
        id::<EffectTag>(9_004),
        ModelSettlementFingerprint {
            kind: ModelSettlementKind::Completed,
            digest: Digest::raw_json(b"{}"),
        },
    );

    let (bytes, digest) =
        encode_snapshot(&state, 2, head_checksum_at_two, None, None).expect("encode snapshot");
    let snapshot = OpaqueSnapshot::try_new(2, digest, bytes, wide_limits().snapshot_bytes)
        .expect("opaque snapshot");
    store
        .write_snapshot(SnapshotRequest {
            session_id: id::<SessionTag>(1),
            snapshot,
        })
        .await
        .expect("write snapshot at sequence 2");

    (store, schema, guard, head_checksum_at_two)
}

/// A session with no stored snapshot cannot be pruned.
#[tokio::test]
async fn prune_without_a_snapshot_is_rejected() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, guard) = disposable_store(&url).await;
    store
        .append(request(1, 1, 1, vec![draft(1, 1)]))
        .await
        .expect("append");

    let error = store
        .prune(PruneRequest {
            session_id: id::<SessionTag>(1),
            horizon: IdempotencyHorizon {
                expire_at: Timestamp::from_unix_ms(0).expect("ts"),
            },
        })
        .await
        .expect_err("prune without a snapshot must be rejected");
    assert!(
        matches!(
            error,
            StoreError::InvalidRequest {
                reason_code: "prune_requires_snapshot"
            }
        ),
        "unexpected error: {error:?}"
    );

    guard.cleanup(&connect(&url).await).await;
}

/// An aligned prune deletes the snapshot-covered prefix, decrements the
/// session's counters by exactly what was deleted, and reports receipt
/// counts that match what `finstack_ai_store_common::{outstanding_count,
/// tombstone_count}` compute over the pruned state — the same functions
/// sqlite's and the memory store's `prune` call. A `SnapshotPlusTail` load
/// afterwards returns the retained snapshot plus the retained tail and
/// verifies cleanly.
#[tokio::test]
async fn aligned_prune_deletes_the_prefix_and_a_snapshot_plus_tail_load_verifies() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, _schema, guard, head_checksum_at_two) =
        Box::pin(scripted_session_with_aligned_snapshot(&url)).await;

    let receipt = store
        .prune(PruneRequest {
            session_id: id::<SessionTag>(1),
            horizon: IdempotencyHorizon {
                expire_at: Timestamp::from_unix_ms(0).expect("ts"),
            },
        })
        .await
        .expect("aligned prune succeeds");
    assert_eq!(receipt.pruned_through_sequence, 2);
    assert_eq!(receipt.retained_outstanding, 0);
    assert_eq!(
        receipt.retained_tombstones, 4,
        "two completion identities, one resolution identity, one model settlement"
    );

    let loaded = store
        .load_from(LoadFromRequest {
            session_id: id::<SessionTag>(1),
            window: LoadWindow::SnapshotPlusTail,
        })
        .await
        .expect("snapshot-plus-tail load verifies after prune");
    let accelerated = loaded
        .accelerated
        .expect("the retained snapshot decodes to accelerated state");
    assert_eq!(accelerated.sequence, 2);
    assert_eq!(accelerated.head_checksum, head_checksum_at_two);
    let tail_sequences = loaded
        .committed_batches
        .iter()
        .flat_map(|batch| batch.records.iter())
        .map(finstack_ai_kernel::RecordEnvelope::sequence)
        .collect::<Vec<_>>();
    assert_eq!(
        tail_sequences,
        vec![3],
        "only the record after the snapshot boundary remains in the tail"
    );

    guard.cleanup(&connect(&url).await).await;
}

/// A prune and a concurrent append on the same session serialize through the
/// session row lock rather than deadlocking: both complete, and the journal
/// afterwards reflects both — the pruned prefix is gone and the concurrently
/// appended record is present.
#[tokio::test]
async fn concurrent_prune_and_append_serialize_without_deadlock() {
    let Some(url) = pg_test_url() else {
        eprintln!("skipped: FINSTACK_PG_TEST_URL unset");
        return;
    };
    let (store, schema, guard, _head_checksum_at_two) =
        Box::pin(scripted_session_with_aligned_snapshot(&url)).await;
    // Two independent `PostgresJournalStore`s (two pools, two sets of
    // connections) onto the same schema and session: the prune and the
    // append race each other's session row lock across process-independent
    // handles, exactly as two separate application processes would.
    let second_store = Arc::new(open_store(&url, &schema).await);

    let prune_task = {
        let store = Arc::clone(&store);
        tokio::spawn(async move {
            store
                .prune(PruneRequest {
                    session_id: id::<SessionTag>(1),
                    horizon: IdempotencyHorizon {
                        expire_at: Timestamp::from_unix_ms(0).expect("ts"),
                    },
                })
                .await
        })
    };
    let append_task = {
        let store = Arc::clone(&second_store);
        tokio::spawn(async move { store.append(request(4, 1, 4, vec![draft(4, 1)])).await })
    };

    let (prune_result, append_result) = tokio::join!(prune_task, append_task);
    let receipt = prune_result
        .expect("prune task did not panic")
        .expect("prune completes without deadlocking");
    let committed = append_result
        .expect("append task did not panic")
        .expect("append completes without deadlocking");
    assert_eq!(receipt.pruned_through_sequence, 2);
    assert_eq!(committed.last_sequence, 4);

    let loaded = store
        .load_from(LoadFromRequest {
            session_id: id::<SessionTag>(1),
            window: LoadWindow::SnapshotPlusTail,
        })
        .await
        .expect("post-race snapshot-plus-tail load verifies");
    let tail_sequences = loaded
        .committed_batches
        .iter()
        .flat_map(|batch| batch.records.iter())
        .map(finstack_ai_kernel::RecordEnvelope::sequence)
        .collect::<Vec<_>>();
    assert_eq!(
        tail_sequences,
        vec![3, 4],
        "the pre-race tail record and the concurrently appended one are both present"
    );

    guard.cleanup(&connect(&url).await).await;
}
