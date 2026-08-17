use super::super::input::{
    ExternalEffectCompletedInput, ExternalEffectOutcome, ModelSettled, ModelSettlement,
    ToolSettlement,
};
use super::*;
use crate::digest::Digest;
use crate::effects::{EffectCompleted, EffectFailed, EffectOutputContract};
use crate::ids::{EffectId, ModelRequestId, ToolBatchId, ToolCallId, TurnId};
use crate::message::Message;
use crate::raw_json::RawJson;
use crate::time::Timestamp;
use crate::tools::{ToolBatchContinuation, ToolBatchOutcome};
use std::sync::Arc;

use crate::effects::{EffectDeferred, EffectOutputKind, ReconciliationPolicy};
use crate::error::{ErrorCategory, ErrorDescriptor};
use crate::ids::{ArtifactId, ComponentId, MessageId};
use crate::message::{MessageRole, ProviderIds};
use crate::raw_json::Metadata;
use crate::refs::{ExternalHandleRef, Usage};
use crate::{ArtifactRef, BlobRef, ContentBlock, TextBlock};

#[test]
fn all_model_settlement_families_match_explicit_null_known_answers() {
    let completed = ModelSettled {
        turn_id: turn_id(),
        model_request_id: request_id(),
        outcome: ModelSettlement::Completed {
            completion: completed_effect(),
            assistant_message: assistant_message(),
        },
    };
    let failed = ModelSettled {
        turn_id: turn_id(),
        model_request_id: request_id(),
        outcome: ModelSettlement::Failed(failed_effect(None)),
    };
    let deferred = ModelSettled {
        turn_id: turn_id(),
        model_request_id: request_id(),
        outcome: ModelSettlement::Deferred(deferred_effect()),
    };
    let external_completed = ExternalEffectCompletedInput {
        completion: super::super::input::ExternalEffectCompletion {
            effect_id: effect_id(),
            completion_id: Arc::from("external-completion"),
            outcome: ExternalEffectOutcome::Completed {
                output: RawJson::parse(r#"{"text":"hello"}"#).expect("output"),
                usage: Some(Usage::empty()),
                artifacts: Arc::from([]),
            },
        },
        assistant_message: Some(assistant_message()),
    };
    let external_failed = ExternalEffectCompletedInput {
        completion: super::super::input::ExternalEffectCompletion {
            effect_id: effect_id(),
            completion_id: Arc::from("external-failure"),
            outcome: ExternalEffectOutcome::Failed {
                error: failure_error(),
            },
        },
        assistant_message: None,
    };

    let actual = [
        direct_digest(&completed).expect("completed").to_hex(),
        direct_digest(&failed).expect("failed").to_hex(),
        direct_digest(&deferred).expect("deferred").to_hex(),
        external_digest(&external_completed)
            .expect("external completed")
            .to_hex(),
        external_digest(&external_failed)
            .expect("external failed")
            .to_hex(),
    ];
    assert_eq!(
        actual,
        [
            "a58098266092b6c6fd944a1315e78da8b15cc73f47896a930625a05fdd17fb66",
            "a2213be113b6ec43f4ae61c7cc6287dd56af0657578eea9d24f8d8d6f8eed862",
            "1e6a0f2ee48d94cb157ff6dd45cb8dd3742712b6efc0e4aacf16c90c2b71fdaa",
            "be24d437c38cdef5f4c3ab28c6ca1bd68e56307b62b149c238ddcaf448865b72",
            "0f6d26e1e4c7aabe6ed3679e2812abcd6e037db35945858cc0581a9758f5508d",
        ]
    );
}

#[test]
fn tool_fingerprint_domains_and_source_variants_are_stable_and_distinct() {
    let batch = ToolBatchId::parse("01234567-89ab-7cde-89ab-0123456789b0").expect("tool batch");
    let direct_completed =
        direct_tool_digest(batch, &ToolSettlement::Completed(completed_effect()))
            .expect("direct completed");
    assert_eq!(
        direct_completed,
        direct_tool_digest(batch, &ToolSettlement::Completed(completed_effect()))
            .expect("repeated direct completed")
    );
    let direct_failed = direct_tool_digest(
        batch,
        &ToolSettlement::Failed(failed_effect(Some("direct-failure"))),
    )
    .expect("direct failed");
    let direct_deferred = direct_tool_digest(batch, &ToolSettlement::Deferred(deferred_effect()))
        .expect("direct deferred");
    let external_completed_input = ExternalEffectCompletedInput {
        completion: super::super::input::ExternalEffectCompletion {
            effect_id: effect_id(),
            completion_id: Arc::from("direct-completion"),
            outcome: ExternalEffectOutcome::Completed {
                output: RawJson::parse(r#"{"text":"hello"}"#).expect("output"),
                usage: None,
                artifacts: Arc::from([]),
            },
        },
        assistant_message: None,
    };
    let external_completed =
        external_tool_digest(batch, &external_completed_input).expect("external completed");
    let external_failed = external_tool_digest(
        batch,
        &ExternalEffectCompletedInput {
            completion: super::super::input::ExternalEffectCompletion {
                effect_id: effect_id(),
                completion_id: Arc::from("direct-failure"),
                outcome: ExternalEffectOutcome::Failed {
                    error: failure_error(),
                },
            },
            assistant_message: None,
        },
    )
    .expect("external failed");
    let tool_call_id =
        ToolCallId::parse("01234567-89ab-7cde-89ab-0123456789b1").expect("tool call");
    let synthetic_error =
        ErrorDescriptor::new("unknown_tool", "unknown tool", ErrorCategory::Tool, false)
            .expect("synthetic error");
    let synthetic_result = crate::ToolResultBlock::try_new(
        tool_call_id,
        vec![ContentBlock::Text(
            TextBlock::try_new("unknown tool").expect("result text"),
        )],
        true,
    )
    .expect("synthetic result");
    let synthetic = synthetic_tool_digest(
        batch,
        tool_call_id,
        effect_id(),
        &synthetic_result,
        &synthetic_error,
    )
    .expect("synthetic");
    let plan = tool_batch_plan_digest(
        7,
        turn_id(),
        batch,
        *assistant_message().id(),
        &[],
        ToolBatchContinuation::Finalize,
    )
    .expect("plan");
    let close = tool_batch_close_digest(
        7,
        turn_id(),
        batch,
        *assistant_message().id(),
        &[],
        &ToolBatchOutcome::Finalize,
    )
    .expect("close");
    let distinct = [
        direct_completed,
        direct_failed,
        direct_deferred,
        external_completed,
        external_failed,
        synthetic,
        plan,
        close,
    ]
    .into_iter()
    .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(distinct.len(), 8);
}

const OPAQUE_OUTPUT_ABSENT: &str =
    "a782b5b04c7c2ccceebaa2d7521bcd074bcfa6b1ac65497fa0defd3685c3ccdb";
const OPAQUE_OUTPUT_EXPLICIT_NULL: &str =
    "0d269028aab76646df000c88a83923a76542804cafba0581861427edfdf3f92c";
const MESSAGE_METADATA_ABSENT: &str =
    "0aef0c2831804cc36f3168d9beff5f89e1bba771a0b570aa1681d1abbf268235";
const MESSAGE_METADATA_EXPLICIT_NULL: &str =
    "a939b8786c85fbd5a52c15788dab3f9168159f43fe31145c22121330664c32e8";
const ERROR_SAFE_DETAILS_ABSENT: &str =
    "dd6417daa0a9e055f445c925357994e939bcf3936fbbb50e8ea8112409a71556";
const ERROR_SAFE_DETAILS_EXPLICIT_NULL: &str =
    "db3ed3d3acf0e244e5c34fc041bbb3151d41f6a7fc1f64b1155bd1b4fede4492";
const ARTIFACT_METADATA_ABSENT: &str =
    "0a96c718c72dc143d64776a529370d68be1cfcae28539a5e109aaa28e7892f88";
const ARTIFACT_METADATA_EXPLICIT_NULL: &str =
    "f4051dabe48adf42cddd2738d758116d9c7fcf2a0556b0dc0916cd8c009ed51e";

#[test]
fn opaque_output_absent_and_null_shapes_have_fixed_distinct_fingerprints() {
    let opaque_absent = RawJson::parse(r#"{"provider_ids":{}}"#).expect("opaque absent fields");
    let opaque_null = RawJson::parse(
        r#"{"provider_ids":{"continuation_id":null,"request_id":null,"response_id":null}}"#,
    )
    .expect("opaque explicit nulls");
    let opaque_absent_digest = completed_digest(opaque_absent.clone(), Metadata::empty(), vec![]);
    let opaque_null_digest = completed_digest(opaque_null.clone(), Metadata::empty(), vec![]);
    assert_known_pair(
        "opaque output",
        opaque_absent_digest,
        opaque_null_digest,
        OPAQUE_OUTPUT_ABSENT,
        OPAQUE_OUTPUT_EXPLICIT_NULL,
    );
}

#[test]
fn message_metadata_absent_and_null_shapes_have_fixed_distinct_fingerprints() {
    let metadata_absent_digest = completed_digest(
        RawJson::parse("{}").expect("output"),
        Metadata::parse(
            r#"{"content":[],"created_at":"2025-01-01T00:00:00Z","id":"opaque","role":"user"}"#,
        )
        .expect("metadata"),
        vec![],
    );
    let metadata_null_digest =
        completed_digest(
            RawJson::parse("{}").expect("output"),
            Metadata::parse(
                r#"{"content":[],"created_at":"2025-01-01T00:00:00Z","id":"opaque","model":null,"role":"user"}"#,
            )
            .expect("metadata"),
            vec![],
        );
    assert_known_pair(
        "message metadata",
        metadata_absent_digest,
        metadata_null_digest,
        MESSAGE_METADATA_ABSENT,
        MESSAGE_METADATA_EXPLICIT_NULL,
    );
}

#[test]
fn error_safe_details_absent_and_null_shapes_have_fixed_distinct_fingerprints() {
    let mut error_absent = failure_error();
    error_absent.safe_details = Metadata::parse(r#"{"provider_ids":{}}"#).expect("safe details");
    let mut error_null = failure_error();
    error_null.safe_details = Metadata::parse(
        r#"{"provider_ids":{"continuation_id":null,"request_id":null,"response_id":null}}"#,
    )
    .expect("safe details");
    let error_absent_digest = direct_digest(&ModelSettled {
        turn_id: turn_id(),
        model_request_id: request_id(),
        outcome: ModelSettlement::Failed(failed_effect_with_error(error_absent)),
    })
    .expect("failure");
    let error_null_digest = direct_digest(&ModelSettled {
        turn_id: turn_id(),
        model_request_id: request_id(),
        outcome: ModelSettlement::Failed(failed_effect_with_error(error_null)),
    })
    .expect("failure");
    assert_known_pair(
        "error safe_details",
        error_absent_digest,
        error_null_digest,
        ERROR_SAFE_DETAILS_ABSENT,
        ERROR_SAFE_DETAILS_EXPLICIT_NULL,
    );
}

#[test]
fn artifact_metadata_absent_and_null_shapes_have_fixed_distinct_fingerprints() {
    let artifact_absent =
        artifact(Metadata::parse(r#"{"provider_ids":{}}"#).expect("artifact metadata"));
    let artifact_null = artifact(
        Metadata::parse(
            r#"{"provider_ids":{"continuation_id":null,"request_id":null,"response_id":null}}"#,
        )
        .expect("artifact metadata"),
    );
    let artifact_absent_digest = completed_digest(
        RawJson::parse("{}").expect("output"),
        Metadata::empty(),
        vec![artifact_absent],
    );
    let artifact_null_digest = completed_digest(
        RawJson::parse("{}").expect("output"),
        Metadata::empty(),
        vec![artifact_null],
    );
    assert_known_pair(
        "artifact metadata",
        artifact_absent_digest,
        artifact_null_digest,
        ARTIFACT_METADATA_ABSENT,
        ARTIFACT_METADATA_EXPLICIT_NULL,
    );
}

#[test]
fn semantically_equal_canonical_raw_json_has_equal_fingerprints() {
    let canonical_a = RawJson::parse(r#"{"a":1,"b":2}"#).expect("canonical a");
    let canonical_b = RawJson::parse(r#"{"b":2,"a":1}"#).expect("canonical b");
    assert_eq!(
        completed_digest(canonical_a, Metadata::empty(), vec![]),
        completed_digest(canonical_b, Metadata::empty(), vec![])
    );
}

fn assert_known_pair(
    label: &str,
    absent: Digest,
    explicit_null: Digest,
    expected_absent: &str,
    expected_explicit_null: &str,
) {
    assert_ne!(absent, explicit_null, "{label} relational distinction");
    assert_eq!(
        absent.to_hex(),
        expected_absent,
        "{label} absent schema-1 fingerprint"
    );
    assert_eq!(
        explicit_null.to_hex(),
        expected_explicit_null,
        "{label} explicit-null schema-1 fingerprint"
    );
}

fn completed_effect() -> EffectCompleted {
    EffectCompleted::try_new(
        effect_id(),
        output_contract(),
        RawJson::parse(r#"{"text":"hello"}"#).expect("output"),
        None,
        vec![],
        ProviderIds::empty(),
        Some("direct-completion"),
        None,
    )
    .expect("completion")
}

fn failed_effect(completion_id: Option<&str>) -> EffectFailed {
    EffectFailed::try_new(
        effect_id(),
        output_contract(),
        failure_error(),
        None,
        completion_id,
    )
    .expect("failure")
}

fn failed_effect_with_error(error: ErrorDescriptor) -> EffectFailed {
    EffectFailed::try_new(effect_id(), output_contract(), error, None, None::<&str>)
        .expect("failure")
}

fn completed_digest(output: RawJson, metadata: Metadata, artifacts: Vec<ArtifactRef>) -> Digest {
    let completion = EffectCompleted::try_new(
        effect_id(),
        output_contract(),
        output,
        None,
        artifacts,
        ProviderIds::empty(),
        Some("direct-completion"),
        None,
    )
    .expect("completion");
    direct_digest(&ModelSettled {
        turn_id: turn_id(),
        model_request_id: request_id(),
        outcome: ModelSettlement::Completed {
            completion,
            assistant_message: message_with_metadata(metadata),
        },
    })
    .expect("digest")
}

fn artifact(metadata: Metadata) -> ArtifactRef {
    let digest = Digest::blob_content(b"artifact");
    ArtifactRef::try_new(
        ArtifactId::parse("01234567-89ab-7cde-89ab-0123456789af").expect("artifact"),
        "test",
        BlobRef::try_new("blob", "application/json", 8, None, None::<&str>).expect("blob"),
        digest,
        digest,
        metadata,
    )
    .expect("artifact")
}

fn deferred_effect() -> EffectDeferred {
    EffectDeferred {
        effect_id: effect_id(),
        handle: ExternalHandleRef::try_new(
            ComponentId::parse("finstack.provider.test").expect("component"),
            "job-1",
            RawJson::parse("{}").expect("metadata"),
        )
        .expect("handle"),
        reconciliation: ReconciliationPolicy::CallbackOnly,
        next_poll_at: None,
        expires_at: None,
        output_contract: output_contract(),
    }
}

fn assistant_message() -> Message {
    message_with_metadata(Metadata::empty())
}

fn message_with_metadata(metadata: Metadata) -> Message {
    Message::try_new(
        MessageId::parse("01234567-89ab-7cde-89ab-0123456789ad").expect("message"),
        MessageRole::Assistant,
        vec![ContentBlock::Text(
            TextBlock::try_new("hello").expect("text"),
        )],
        Timestamp::from_unix_ms(1_000).expect("timestamp"),
        None,
        ProviderIds::empty(),
        metadata,
    )
    .expect("message")
}

fn output_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::ModelResponse,
        schema_version: 1,
        schema_digest: Digest::raw_json(br#"{"type":"model"}"#),
    }
}

fn failure_error() -> ErrorDescriptor {
    ErrorDescriptor::new("provider_failed", "failed", ErrorCategory::Model, false).expect("error")
}

fn effect_id() -> EffectId {
    EffectId::parse("01234567-89ab-7cde-89ab-0123456789ac").expect("effect")
}

fn turn_id() -> TurnId {
    TurnId::parse("01234567-89ab-7cde-89ab-0123456789aa").expect("turn")
}

fn request_id() -> ModelRequestId {
    ModelRequestId::parse("01234567-89ab-7cde-89ab-0123456789ab").expect("request")
}
