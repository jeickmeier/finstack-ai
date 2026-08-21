//! `HitlInboxStore` contract exercised against `SqliteHitlStore`, plus
//! sqlite-specific persistence and co-location checks.

use std::sync::Arc;

use finstack_ai_kernel::{AuthorizationEvidence, PrincipalRef, Timestamp};
use finstack_ai_workflow_hitl::{
    HitlInboxStore, InteractionRow, InteractionStatus, InteractionTransition, SqliteHitlStore,
};
use finstack_ai_workflow_worker::SqliteWorkerStore;

use crate::store::exercise_hitl_inbox;

fn ts(ms: i64) -> Timestamp {
    Timestamp::from_unix_ms(ms).expect("timestamp")
}

fn id<T: finstack_ai_kernel::IdTag>(ordinal: u64) -> finstack_ai_kernel::Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    finstack_ai_kernel::Id::from_bytes(bytes)
}

fn row(tenant: &str, interaction: &str, session: u64, requested_ms: i64) -> InteractionRow {
    InteractionRow {
        tenant_scope: Arc::from(tenant),
        session_id: id(session),
        lane_id: id(2),
        run_id: id(3),
        interaction_id: Arc::from(interaction),
        kind: Arc::from("approval"),
        requested_at: ts(requested_ms),
        expires_at: None,
        request: Arc::from(&br#"{"prompt":"approve?"}"#[..]),
        accepted_principal: PrincipalRef::try_new("issuer", "subject", Some(tenant))
            .expect("principal"),
        accepted_evidence: AuthorizationEvidence::try_new("policy-v1", "decision-v1")
            .expect("evidence"),
        status: InteractionStatus::Open,
        resolved_by: None,
        outcome_code: None,
        updated_at: ts(requested_ms),
    }
}

#[test]
fn sqlite_store_round_trips_and_orders() {
    let dir = tempfile::tempdir().expect("dir");
    let store = SqliteHitlStore::open(dir.path().join("hitl.sqlite")).expect("open");
    exercise_hitl_inbox(&store);
}

#[test]
fn rows_and_statuses_survive_reopen() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("hitl.sqlite");

    {
        let store = SqliteHitlStore::open(&path).expect("open first");
        store
            .upsert(&row("tenant-a", "int-a1", 1, 1_000))
            .expect("upsert");
        store
            .transition(
                "tenant-a",
                "int-a1",
                InteractionTransition {
                    expected: InteractionStatus::Open,
                    next: InteractionStatus::Buffered,
                    resolved_by: Some("alice"),
                    outcome_code: Some("buffered"),
                    updated_at: ts(2_000),
                },
            )
            .expect("set_status");
    }

    let reopened = SqliteHitlStore::open(&path).expect("reopen");
    let loaded = reopened
        .load("tenant-a", "int-a1")
        .expect("load")
        .expect("present after reopen");
    assert_eq!(loaded.status, InteractionStatus::Buffered);
    assert_eq!(loaded.resolved_by, Some(Arc::from("alice")));
    assert_eq!(loaded.updated_at, ts(2_000));
}

#[test]
fn worker_and_hitl_stores_share_one_file() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("shared.sqlite");

    let worker_store = SqliteWorkerStore::open(&path).expect("worker open");
    let hitl_store = SqliteHitlStore::open(&path).expect("hitl open");

    hitl_store
        .upsert(&row("tenant-a", "int-a1", 1, 1_000))
        .expect("hitl upsert");
    assert!(
        hitl_store
            .load("tenant-a", "int-a1")
            .expect("hitl load")
            .is_some()
    );

    // The worker store still owns and operates on its own tables in the
    // same file.
    assert_eq!(worker_store.path(), path);
    assert_eq!(hitl_store.path(), path);
}

#[test]
fn unversioned_hitl_table_requires_a_fresh_adapter_database() {
    let dir = tempfile::tempdir().expect("dir");
    let path = dir.path().join("legacy.sqlite");
    let legacy = rusqlite::Connection::open(&path).expect("legacy");
    legacy
        .execute_batch(
            "CREATE TABLE finstack_workflow_hitl_inbox (
                tenant_scope TEXT NOT NULL,
                interaction_id TEXT NOT NULL,
                PRIMARY KEY (tenant_scope, interaction_id)
            );",
        )
        .expect("schema");
    drop(legacy);

    let Err(error) = SqliteHitlStore::open(&path) else {
        panic!("unversioned schema must fail closed");
    };
    assert_eq!(error.code(), "hitl_schema_reset_required");
}
