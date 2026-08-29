use crate::record::*;
use crate::tests::sample_record;
use finstack_ai_kernel::{TIMESTAMP_MAX_MS, Timestamp, UNIX_EPOCH};
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
fn scope_permits_requires_every_dimension_to_match() {
    let record_scope = MemoryScope::try_new("t1")
        .unwrap()
        .try_with_user("u1")
        .unwrap();
    let tenant_only = MemoryScope::try_new("t1").unwrap();
    let wrong_tenant = MemoryScope::try_new("t2").unwrap();
    let matching_user = MemoryScope::try_new("t1")
        .unwrap()
        .try_with_user("u1")
        .unwrap();
    let other_user = MemoryScope::try_new("t1")
        .unwrap()
        .try_with_user("u2")
        .unwrap();
    assert!(!tenant_only.permits(&record_scope));
    assert!(matching_user.permits(&record_scope));
    assert!(!other_user.permits(&record_scope));
    assert!(!wrong_tenant.permits(&record_scope));
}

#[test]
fn scope_permits_filters_by_agent_and_workspace() {
    let record_scope = MemoryScope::try_new("t1")
        .unwrap()
        .try_with_agent("a1")
        .unwrap()
        .try_with_workspace("w1")
        .unwrap();
    let matching = MemoryScope::try_new("t1")
        .unwrap()
        .try_with_agent("a1")
        .unwrap()
        .try_with_workspace("w1")
        .unwrap();
    let wrong_agent = MemoryScope::try_new("t1")
        .unwrap()
        .try_with_agent("a2")
        .unwrap();
    let wrong_workspace = MemoryScope::try_new("t1")
        .unwrap()
        .try_with_workspace("w2")
        .unwrap();
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

#[test]
fn preview_of_respects_the_byte_budget_validate_enforces() {
    // A multi-byte body: truncating by characters would produce a preview
    // that `validate` rejects for exceeding PREVIEW_MAX_BYTES.
    let body = "日本語".repeat(200);
    let preview = preview_of(&body);
    assert!(preview.len() <= PREVIEW_MAX_BYTES);

    let mut record = sample_record("m1", "t1");
    record.preview = preview;
    record.body = MemoryBody::Inline(Arc::from(body.as_str()));
    assert!(record.validate().is_ok());
}

#[test]
fn scope_validation_covers_every_dimension() {
    let overlong = "x".repeat(MEMORY_SCOPE_FIELD_MAX_BYTES + 1);
    assert!(MemoryScope::try_new(&overlong).is_err());

    for invalid in ["", "bad\0value", overlong.as_str()] {
        let scope = MemoryScope::try_new("t1").unwrap().try_with_user(invalid);
        assert_eq!(
            scope,
            Err(MemoryError::InvalidRecord {
                reason: "invalid_scope_dimension"
            })
        );
    }
}

#[test]
fn record_validation_rejects_malformed_text_and_links() {
    let mut record = sample_record("m1", "t1");
    record.preview = Arc::from("bad\0preview");
    assert!(record.validate().is_err());

    let mut record = sample_record("m1", "t1");
    record.keywords = Arc::from([Arc::<str>::from("Alpha"), Arc::from("alpha")]);
    assert_eq!(
        record.validate(),
        Err(MemoryError::InvalidRecord {
            reason: "duplicate_keyword"
        })
    );

    let mut record = sample_record("m1", "t1");
    record.body = MemoryBody::Inline(Arc::from("x".repeat(INLINE_BODY_MAX_BYTES + 1)));
    assert_eq!(
        record.validate(),
        Err(MemoryError::InvalidRecord {
            reason: "invalid_inline_body"
        })
    );

    let mut record = sample_record("m1", "t1");
    record.provenance.source_ref = Some(Arc::from("bad\0source"));
    assert_eq!(
        record.validate(),
        Err(MemoryError::InvalidRecord {
            reason: "invalid_provenance"
        })
    );

    let mut record = sample_record("m1", "t1");
    record.superseded_by = Some(record.id.clone());
    assert_eq!(
        record.validate(),
        Err(MemoryError::InvalidRecord {
            reason: "memory_self_supersession"
        })
    );
}

#[test]
fn record_validation_enforces_time_and_retention_invariants() {
    let mut record = sample_record("m1", "t1");
    record.created_at = Timestamp::from_unix_ms(1).unwrap();
    assert_eq!(
        record.validate(),
        Err(MemoryError::InvalidRecord {
            reason: "last_confirmed_before_created"
        })
    );

    let mut record = sample_record("m1", "t1");
    record.retention = RetentionPolicy::ExpireAfterMs(0);
    assert_eq!(
        record.validate(),
        Err(MemoryError::InvalidRecord {
            reason: "invalid_retention"
        })
    );

    let mut record = sample_record("m1", "t1");
    record.created_at = Timestamp::from_unix_ms(TIMESTAMP_MAX_MS).unwrap();
    record.last_confirmed_at = record.created_at;
    record.retention = RetentionPolicy::ExpireAfterMs(1);
    assert_eq!(
        record.validate(),
        Err(MemoryError::InvalidRecord {
            reason: "retention_overflow"
        })
    );
}

#[test]
fn hard_expiry_is_inclusive_at_the_deadline() {
    let mut record = sample_record("m1", "t1");
    record.retention = RetentionPolicy::ExpireAfterMs(10);
    assert!(!record.is_expired_at(Timestamp::from_unix_ms(9).unwrap()));
    assert!(record.is_expired_at(Timestamp::from_unix_ms(10).unwrap()));
}

#[test]
fn embedding_source_text_is_byte_exact_for_inline_bodies() {
    // Canonical form v1: preview, inline body, and space-joined keywords —
    // exactly the fields full-text search indexes — separated by newlines.
    let record = sample_record("m1", "t1");
    assert_eq!(
        crate::store::embedding_source_text(&record),
        "body text\nbody text\nalpha"
    );

    let mut multi = sample_record("m2", "t1");
    multi.preview = Arc::from("a preview");
    multi.body = MemoryBody::Inline(Arc::from("the inline body"));
    multi.keywords = Arc::from([Arc::<str>::from("alpha"), Arc::<str>::from("beta")]);
    assert_eq!(
        crate::store::embedding_source_text(&multi),
        "a preview\nthe inline body\nalpha beta"
    );

    let mut no_keywords = sample_record("m3", "t1");
    no_keywords.keywords = Arc::from(Vec::<Arc<str>>::new());
    assert_eq!(
        crate::store::embedding_source_text(&no_keywords),
        "body text\nbody text\n"
    );
}

#[tokio::test]
async fn embedding_source_text_for_blob_bodies_covers_preview_and_keywords_only() {
    let record = crate::tests::blob_backed_record("m1", "t1").await;
    assert_eq!(
        crate::store::embedding_source_text(&record),
        "body text\n\nalpha"
    );
}

#[test]
fn embedding_source_digest_is_domain_separated_and_stable() {
    let text = "body text\nbody text\nalpha";
    let digest = crate::store::embedding_source_digest(text).unwrap();
    let expected =
        finstack_ai_kernel::Digest::domain_separated("memory-embed-source", 1, text.as_bytes())
            .unwrap();
    assert_eq!(digest, expected);
    assert_ne!(
        digest,
        crate::store::embedding_source_digest("something else").unwrap()
    );
}
