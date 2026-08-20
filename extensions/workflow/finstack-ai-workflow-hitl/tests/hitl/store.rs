//! `HitlInboxStore` contract exercised against `MemoryHitlStore`.

use std::sync::Arc;

use finstack_ai_kernel::Timestamp;
use finstack_ai_workflow_hitl::{
    HitlError, HitlInboxStore, InteractionRow, InteractionStatus, MemoryHitlStore,
};

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
        status: InteractionStatus::Open,
        resolved_by: None,
        updated_at: ts(requested_ms),
    }
}

/// Exercise the full contract against any `HitlInboxStore` implementation.
/// Extracted as a plain function now so a later SQLite-backed store can reuse
/// it against `&dyn HitlInboxStore` without duplicating the assertions.
pub(crate) fn exercise_hitl_inbox(store: &dyn HitlInboxStore) {
    let a1 = row("tenant-a", "int-a1", 1, 1_000);
    let a2 = row("tenant-a", "int-a2", 2, 2_000);
    let b1 = row("tenant-b", "int-b1", 3, 1_500);
    let b2 = row("tenant-b", "int-b2", 4, 500);

    for r in [&a1, &a2, &b1, &b2] {
        store.upsert(r).expect("upsert");
    }

    // load_open returns only that tenant's Open rows, ordered by
    // requested_at then interaction_id.
    let open_a = store.load_open("tenant-a").expect("load_open a");
    assert_eq!(
        open_a
            .iter()
            .map(|r| r.interaction_id.as_ref())
            .collect::<Vec<_>>(),
        vec!["int-a1", "int-a2"]
    );
    let open_b = store.load_open("tenant-b").expect("load_open b");
    assert_eq!(
        open_b
            .iter()
            .map(|r| r.interaction_id.as_ref())
            .collect::<Vec<_>>(),
        vec!["int-b2", "int-b1"],
        "tenant-b rows ordered by requested_at (500 before 1500)"
    );

    // load round-trips every field.
    let loaded = store
        .load("tenant-a", "int-a1")
        .expect("load")
        .expect("present");
    assert_eq!(loaded, a1);

    // set_status(.., Delivered, Some("alice"), ts) is visible on reload,
    // drops the row from load_open, but keeps it in load_active.
    store
        .set_status(
            "tenant-a",
            "int-a1",
            InteractionStatus::Delivered,
            Some("alice"),
            ts(9_000),
        )
        .expect("set_status");
    let reloaded = store
        .load("tenant-a", "int-a1")
        .expect("load")
        .expect("present");
    assert_eq!(reloaded.status, InteractionStatus::Delivered);
    assert_eq!(reloaded.resolved_by, Some(Arc::from("alice")));
    assert_eq!(reloaded.updated_at, ts(9_000));

    let open_a_after = store.load_open("tenant-a").expect("load_open a after");
    assert_eq!(
        open_a_after
            .iter()
            .map(|r| r.interaction_id.as_ref())
            .collect::<Vec<_>>(),
        vec!["int-a2"],
        "delivered row drops out of load_open"
    );

    let active = store.load_active().expect("load_active");
    let active_ids: Vec<&str> = active.iter().map(|r| r.interaction_id.as_ref()).collect();
    assert!(
        active_ids.contains(&"int-a1"),
        "delivered row stays in load_active"
    );
    assert!(active_ids.contains(&"int-a2"));
    assert!(active_ids.contains(&"int-b1"));
    assert!(active_ids.contains(&"int-b2"));

    // set_status on an unknown id errors unknown_interaction.
    let err = store
        .set_status(
            "tenant-a",
            "does-not-exist",
            InteractionStatus::Closed,
            None,
            ts(10_000),
        )
        .expect_err("unknown interaction");
    assert!(matches!(err, HitlError::UnknownInteraction));
    assert_eq!(err.code(), "unknown_interaction");

    assert_load_open_breaks_ties_by_interaction_id(store);
    assert_expired_and_closed_rows_absent_from_load_active(store);
}

/// Coverage gap (a): two rows with identical `requested_at` and different
/// `interaction_id` must come back from `load_open` in id order.
fn assert_load_open_breaks_ties_by_interaction_id(store: &dyn HitlInboxStore) {
    let c1 = row("tenant-c", "int-c2", 5, 7_000);
    let c2 = row("tenant-c", "int-c1", 6, 7_000);
    for r in [&c1, &c2] {
        store.upsert(r).expect("upsert tie");
    }
    let open_c = store.load_open("tenant-c").expect("load_open c");
    assert_eq!(
        open_c
            .iter()
            .map(|r| r.interaction_id.as_ref())
            .collect::<Vec<_>>(),
        vec!["int-c1", "int-c2"],
        "identical requested_at breaks ties by interaction_id"
    );
}

/// Coverage gap (b): rows set to `Expired` or `Closed` must be absent from
/// `load_active`.
fn assert_expired_and_closed_rows_absent_from_load_active(store: &dyn HitlInboxStore) {
    let d1 = row("tenant-d", "int-d1", 7, 8_000);
    let d2 = row("tenant-d", "int-d2", 8, 8_500);
    for r in [&d1, &d2] {
        store.upsert(r).expect("upsert active gap");
    }
    store
        .set_status(
            "tenant-d",
            "int-d1",
            InteractionStatus::Expired,
            None,
            ts(9_500),
        )
        .expect("set_status expired");
    store
        .set_status(
            "tenant-d",
            "int-d2",
            InteractionStatus::Closed,
            None,
            ts(9_500),
        )
        .expect("set_status closed");
    let active_after = store.load_active().expect("load_active after");
    let active_ids_after: Vec<&str> = active_after
        .iter()
        .map(|r| r.interaction_id.as_ref())
        .collect();
    assert!(
        !active_ids_after.contains(&"int-d1"),
        "expired row absent from load_active"
    );
    assert!(
        !active_ids_after.contains(&"int-d2"),
        "closed row absent from load_active"
    );
}

#[test]
fn memory_store_round_trips_and_orders() {
    exercise_hitl_inbox(&MemoryHitlStore::new());
}

#[test]
fn interaction_status_parse_rejects_unknown_tokens() {
    let err = InteractionStatus::parse("bogus").expect_err("bogus status");
    assert!(matches!(
        err,
        HitlError::StoreIntegrity {
            code: "interaction_status"
        }
    ));
    assert_eq!(err.code(), "interaction_status");
}
