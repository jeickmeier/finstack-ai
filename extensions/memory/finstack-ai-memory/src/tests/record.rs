use crate::record::*;
use crate::tests::sample_record;
use finstack_ai_kernel::{Timestamp, UNIX_EPOCH};
use std::sync::Arc;

#[test]
fn memory_id_rejects_empty_and_nul() {
    assert!(MemoryId::parse("").is_err());
    assert!(MemoryId::parse("a\0b").is_err());
    assert!(MemoryId::parse("ok-id").is_ok());
}

#[test]
fn memory_id_rejects_too_long() {
    let too_long = "a".repeat(257);
    assert!(MemoryId::parse(&too_long).is_err());
    let max = "a".repeat(256);
    assert!(MemoryId::parse(&max).is_ok());
}

#[test]
fn scope_try_new_rejects_empty_and_nul_tenant() {
    assert!(MemoryScope::try_new("").is_err());
    assert!(MemoryScope::try_new("a\0b").is_err());
    assert!(MemoryScope::try_new("t1").is_ok());
}

#[test]
fn scope_permits_filters_by_optional_fields() {
    let record_scope = MemoryScope::try_new("t1").unwrap().with_user("u1");
    let tenant_only = MemoryScope::try_new("t1").unwrap();
    let wrong_tenant = MemoryScope::try_new("t2").unwrap();
    let matching_user = MemoryScope::try_new("t1").unwrap().with_user("u1");
    let other_user = MemoryScope::try_new("t1").unwrap().with_user("u2");
    assert!(tenant_only.permits(&record_scope));
    assert!(matching_user.permits(&record_scope));
    assert!(!other_user.permits(&record_scope));
    assert!(!wrong_tenant.permits(&record_scope));
}

#[test]
fn scope_permits_filters_by_agent_and_workspace() {
    let record_scope = MemoryScope::try_new("t1")
        .unwrap()
        .with_agent("a1")
        .with_workspace("w1");
    let matching = MemoryScope::try_new("t1")
        .unwrap()
        .with_agent("a1")
        .with_workspace("w1");
    let wrong_agent = MemoryScope::try_new("t1").unwrap().with_agent("a2");
    let wrong_workspace = MemoryScope::try_new("t1").unwrap().with_workspace("w2");
    assert!(matching.permits(&record_scope));
    assert!(!wrong_agent.permits(&record_scope));
    assert!(!wrong_workspace.permits(&record_scope));
}

#[test]
fn record_validation_bounds_confidence_and_preview() {
    let mut record = sample_record("m1", "t1");
    record.provenance.confidence = 101;
    assert!(record.validate().is_err());
    let mut record = sample_record("m1", "t1");
    record.preview = Arc::from("x".repeat(300));
    assert!(record.validate().is_err());
    assert!(sample_record("m1", "t1").validate().is_ok());
}

#[test]
fn record_validation_bounds_keywords() {
    let mut record = sample_record("m1", "t1");
    record.keywords = Arc::from([Arc::<str>::from("")]);
    assert!(record.validate().is_err());

    let mut record = sample_record("m1", "t1");
    record.keywords = Arc::from([Arc::<str>::from("x".repeat(129))]);
    assert!(record.validate().is_err());

    let mut record = sample_record("m1", "t1");
    let too_many: Vec<Arc<str>> = (0..65).map(|i| Arc::from(format!("kw{i}"))).collect();
    record.keywords = Arc::from(too_many);
    assert!(record.validate().is_err());

    let mut record = sample_record("m1", "t1");
    let exactly_64: Vec<Arc<str>> = (0..64).map(|i| Arc::from(format!("kw{i}"))).collect();
    record.keywords = Arc::from(exactly_64);
    assert!(record.validate().is_ok());
}

#[test]
fn system_clock_produces_a_timestamp_at_or_after_epoch() {
    let clock = system_clock();
    let now = clock();
    assert!(now >= UNIX_EPOCH);
}

#[test]
fn memory_error_reason_codes_are_stable() {
    let err = MemoryError::Configuration {
        reason: "memory_configuration_invalid",
    };
    assert!(err.to_string().contains("memory_configuration_invalid"));
    let err = MemoryError::InvalidRecord {
        reason: "memory_record_invalid",
    };
    assert!(err.to_string().contains("memory_record_invalid"));
}

#[test]
fn timestamp_type_is_reachable() {
    let _: Timestamp = UNIX_EPOCH;
}
