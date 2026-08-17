use std::sync::Arc;

use super::*;
use crate::content::BlobRef;
use crate::digest::Digest;
use crate::error::ErrorDescriptor;
use crate::ids::{ArtifactId, ComponentId, EffectId};
use crate::raw_json::{Metadata, RawJson};
use crate::refs::{ArtifactRef, ExternalHandleRef};

#[test]
fn effect_input_external_tag_round_trips_raw_json() {
    let input = EffectInput::Model {
        request: RawJson::parse(r#"{"a":1}"#).expect("json"),
    };
    let json = serde_json::to_string(&input).expect("ser");
    assert_eq!(json, r#"{"model":{"request":{"a":1}}}"#);
    let round: EffectInput = serde_json::from_str(&json).expect("de");
    assert_eq!(round, input);
    assert_eq!(
        input.digest().expect("digest").to_hex(),
        "c59ec0a2d3310a96d14147f8ea38ec979fe0623a5aa3ad6f15981ae26e67eab7"
    );
}

#[test]
fn effect_request_digests_input() {
    let effect_id = EffectId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("id");
    let input = EffectInput::Model {
        request: RawJson::parse(r#"{"b":1,"a":2}"#).expect("json"),
    };
    let contract = EffectOutputContract {
        kind: EffectOutputKind::ModelResponse,
        schema_version: 1,
        schema_digest: Digest::raw_json(br#"{"schema":1}"#),
    };
    let requested = EffectRequested::try_new(
        effect_id,
        EffectKind::Model,
        None,
        None,
        None,
        contract,
        input,
        RetrySafety::SafeToRetry,
        None,
    )
    .expect("requested");
    assert_eq!(
        requested.input_digest(),
        requested.input().digest().expect("digest")
    );
    assert!(requested.relation().is_none());
    assert!(requested.component().is_none());
    assert!(requested.pipeline().is_none());
    assert!(requested.deadline().is_none());
    assert!(
        EffectRequested::try_new(
            effect_id,
            EffectKind::Tool,
            None,
            None,
            None,
            requested.output_contract().clone(),
            EffectInput::Model {
                request: RawJson::parse("{}").expect("json"),
            },
            RetrySafety::Unknown,
            None,
        )
        .is_err()
    );
}

#[test]
fn interaction_cancellation_deserialization_enforces_authorization_pair() {
    let input = r#"{
        "interaction_id":"01234567-89ab-7cde-89ab-0123456789ab",
        "principal":{
            "issuer":"https://issuer.example",
            "subject":"user-1",
            "tenant_scope":"tenant-a"
        }
    }"#;
    let error = serde_json::from_str::<InteractionCancelled>(input)
        .expect_err("one-sided authorization must fail");
    assert!(error.to_string().contains("requires both principal"));
}

#[test]
fn effect_completion_rejects_oversized_artifact_array() {
    let artifact = ArtifactRef::try_new(
        ArtifactId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("artifact"),
        "model-output",
        BlobRef::try_new("blob-1", "application/json", 2, None, None::<&str>).expect("blob"),
        Digest::raw_json(br"{}"),
        Digest::raw_json(br"scope"),
        Metadata::empty(),
    )
    .expect("artifact");
    let error = EffectCompleted::try_new(
        EffectId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("effect"),
        EffectOutputContract {
            kind: EffectOutputKind::ModelResponse,
            schema_version: 1,
            schema_digest: Digest::raw_json(br"schema"),
        },
        RawJson::parse("{}").expect("output"),
        None,
        vec![artifact; crate::content::CONTENT_MAX_ITEMS + 1],
        crate::message::ProviderIds::empty(),
        None::<&str>,
        None,
    )
    .expect_err("oversized artifacts");
    assert_eq!(error.code(), "too_many_items");
}

#[test]
fn failed_effect_rejects_programmatically_invalid_descriptor() {
    let effect_id = EffectId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("effect");
    let mut error = ErrorDescriptor::new(
        "provider_failed",
        "failed",
        crate::error::ErrorCategory::Model,
        false,
    )
    .expect("descriptor");
    error.message = Arc::from("x".repeat(crate::content::TEXT_MAX_BYTES + 1));
    let result = EffectFailed::try_new(
        effect_id,
        EffectOutputContract {
            kind: EffectOutputKind::ModelResponse,
            schema_version: 1,
            schema_digest: Digest::raw_json(b"schema"),
        },
        error,
        None,
        None::<&str>,
    );
    assert!(result.is_err());
}

#[test]
fn effect_settlements_must_preserve_originating_output_contract() {
    let effect_id = EffectId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("effect");
    let requested = EffectRequested::try_new(
        effect_id,
        EffectKind::Model,
        None,
        None,
        None,
        EffectOutputContract {
            kind: EffectOutputKind::ModelResponse,
            schema_version: 1,
            schema_digest: Digest::raw_json(br"model-response"),
        },
        EffectInput::Model {
            request: RawJson::parse("{}").expect("input"),
        },
        RetrySafety::SafeToRetry,
        None,
    )
    .expect("request");
    let wrong_contract = EffectOutputContract {
        kind: EffectOutputKind::ToolResult,
        schema_version: 1,
        schema_digest: Digest::raw_json(br"tool-result"),
    };
    let completed = EffectCompleted::try_new(
        effect_id,
        wrong_contract.clone(),
        RawJson::parse("{}").expect("output"),
        None,
        vec![],
        crate::message::ProviderIds::empty(),
        None::<&str>,
        None,
    )
    .expect("completion");
    assert_eq!(completed.output().as_str(), "{}");
    assert!(completed.usage().is_none());
    assert!(completed.artifacts().is_empty());
    assert!(completed.completion_id().is_none());
    assert_eq!(
        completed
            .validate_against(&requested)
            .expect_err("contract mismatch")
            .code(),
        "effect_settlement_mismatch"
    );

    let deferred = EffectDeferred {
        effect_id,
        handle: ExternalHandleRef::try_new(
            ComponentId::parse("finstack.provider.example").expect("component"),
            "handle-1",
            RawJson::parse("{}").expect("metadata"),
        )
        .expect("handle"),
        reconciliation: ReconciliationPolicy::CallbackOnly,
        next_poll_at: None,
        expires_at: None,
        output_contract: wrong_contract,
    };
    assert_eq!(
        deferred
            .validate_against(&requested)
            .expect_err("contract mismatch")
            .code(),
        "effect_settlement_mismatch"
    );
}
