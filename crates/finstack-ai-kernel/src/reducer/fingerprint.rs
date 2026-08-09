//! Source-discriminated PR-009 model-settlement fingerprints.

use serde::Serialize;

use super::decision::KernelError;
use super::input::{
    ExternalEffectCompletedInput, ExternalEffectOutcome, ModelSettled, ModelSettlement,
    ReducerStageOutcome, StageSettled,
};
use crate::digest::Digest;
use crate::effects::{
    ComponentInvocation, EffectCompleted, EffectFailed, EffectInput, EffectOutputContract,
    EffectRequested, RetrySafety,
};
use crate::entries::{ContextPrepared, StageCursor, StageDisposition, StageOutcomeRecorded};
use crate::ids::{EffectId, ModelRequestId, TurnId};
use crate::message::Message;
use crate::projection::{
    ArtifactProjection, EffectCompletedProjection, EffectDeferredProjection,
    EffectFailedProjection, ErrorProjection, MessageProjection, UsageProjection,
};
use crate::raw_json::RawJson;
use crate::state::PendingModelEffect;
use crate::time::Timestamp;

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum StageSettlementFingerprintV1<'a> {
    Continue {
        cursor: StageCursor,
    },
    ContextPrepared {
        cursor: StageCursor,
        messages: Vec<MessageProjection<'a>>,
    },
    ModelRequestPrepared(Box<ModelRequestPreparedFingerprintV1<'a>>),
    FinalizeAccepted {
        cursor: StageCursor,
    },
    ContinueModel {
        cursor: StageCursor,
    },
    Fail(Box<StageFailFingerprintV1<'a>>),
}

#[derive(Serialize)]
struct ModelRequestPreparedFingerprintV1<'a> {
    cursor: StageCursor,
    request: &'a RawJson,
    component: Option<&'a ComponentInvocation>,
    output_contract: &'a EffectOutputContract,
    retry_safety: RetrySafety,
    deadline: Option<Timestamp>,
}

#[derive(Serialize)]
struct StageFailFingerprintV1<'a> {
    cursor: StageCursor,
    error: ErrorProjection<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum ModelSettlementFingerprintV1<'a> {
    DirectCompleted(DirectModelCompletedFingerprintV1<'a>),
    DirectFailed(DirectModelFailedFingerprintV1<'a>),
    DirectDeferred(DirectModelDeferredFingerprintV1<'a>),
    ExternalCompleted(ExternalModelCompletedFingerprintV1<'a>),
    ExternalFailed(ExternalModelFailedFingerprintV1<'a>),
}

#[derive(Serialize)]
struct DirectModelCompletedFingerprintV1<'a> {
    turn_id: TurnId,
    model_request_id: ModelRequestId,
    completion: EffectCompletedProjection<'a>,
    assistant_message: MessageProjection<'a>,
}

#[derive(Serialize)]
struct DirectModelFailedFingerprintV1<'a> {
    turn_id: TurnId,
    model_request_id: ModelRequestId,
    failure: EffectFailedProjection<'a>,
}

#[derive(Serialize)]
struct DirectModelDeferredFingerprintV1<'a> {
    turn_id: TurnId,
    model_request_id: ModelRequestId,
    deferred: EffectDeferredProjection<'a>,
}

#[derive(Serialize)]
struct ExternalModelCompletedFingerprintV1<'a> {
    effect_id: EffectId,
    completion_id: &'a str,
    output: &'a RawJson,
    usage: Option<UsageProjection<'a>>,
    artifacts: Vec<ArtifactProjection<'a>>,
    assistant_message: MessageProjection<'a>,
}

#[derive(Serialize)]
struct ExternalModelFailedFingerprintV1<'a> {
    effect_id: EffectId,
    completion_id: &'a str,
    error: ErrorProjection<'a>,
}

pub(super) fn stage_digest(input: &StageSettled) -> Result<Digest, KernelError> {
    let fingerprint = match &input.outcome {
        ReducerStageOutcome::Continue => StageSettlementFingerprintV1::Continue {
            cursor: input.cursor,
        },
        ReducerStageOutcome::ContextPrepared { messages } => {
            StageSettlementFingerprintV1::ContextPrepared {
                cursor: input.cursor,
                messages: messages.iter().map(MessageProjection::from).collect(),
            }
        }
        ReducerStageOutcome::ModelRequestPrepared {
            request,
            component,
            output_contract,
            retry_safety,
            deadline,
        } => StageSettlementFingerprintV1::ModelRequestPrepared(Box::new(
            ModelRequestPreparedFingerprintV1 {
                cursor: input.cursor,
                request,
                component: component.as_ref(),
                output_contract,
                retry_safety: *retry_safety,
                deadline: *deadline,
            },
        )),
        ReducerStageOutcome::FinalizeAccepted => StageSettlementFingerprintV1::FinalizeAccepted {
            cursor: input.cursor,
        },
        ReducerStageOutcome::ContinueModel { .. } => StageSettlementFingerprintV1::ContinueModel {
            cursor: input.cursor,
        },
        ReducerStageOutcome::Fail(error) => {
            StageSettlementFingerprintV1::Fail(Box::new(StageFailFingerprintV1 {
                cursor: input.cursor,
                error: ErrorProjection::from(error),
            }))
        }
    };
    super::canonical_digest("stage-settlement", &fingerprint)
}

pub(super) fn stage_record_digest(
    outcome: &StageOutcomeRecorded,
    sibling: Option<&crate::RecordEnvelope>,
) -> Result<Digest, KernelError> {
    let fingerprint = match &outcome.disposition {
        StageDisposition::Continued => StageSettlementFingerprintV1::Continue {
            cursor: outcome.cursor,
        },
        StageDisposition::ContextPrepared { .. } => {
            let Some(crate::RecordBody::ContextPrepared(ContextPrepared { messages, .. })) =
                sibling.map(crate::RecordEnvelope::body)
            else {
                return Err(KernelError::InvalidRecordOrder);
            };
            StageSettlementFingerprintV1::ContextPrepared {
                cursor: outcome.cursor,
                messages: messages.iter().map(MessageProjection::from).collect(),
            }
        }
        StageDisposition::ModelRequested { .. } => {
            let Some(crate::RecordBody::EffectRequested(requested)) =
                sibling.map(crate::RecordEnvelope::body)
            else {
                return Err(KernelError::InvalidRecordOrder);
            };
            model_request_stage_fingerprint(outcome.cursor, requested)?
        }
        StageDisposition::FinalizeAccepted => StageSettlementFingerprintV1::FinalizeAccepted {
            cursor: outcome.cursor,
        },
        StageDisposition::ContinueModel { next_cycle } => {
            if outcome
                .cursor
                .cycle
                .checked_add(1)
                .is_none_or(|expected| expected != *next_cycle)
            {
                return Err(KernelError::InvalidRecordOrder);
            }
            StageSettlementFingerprintV1::ContinueModel {
                cursor: outcome.cursor,
            }
        }
        StageDisposition::Failed { error } => {
            StageSettlementFingerprintV1::Fail(Box::new(StageFailFingerprintV1 {
                cursor: outcome.cursor,
                error: ErrorProjection::from(error),
            }))
        }
    };
    super::canonical_digest("stage-settlement", &fingerprint)
}

fn model_request_stage_fingerprint(
    cursor: StageCursor,
    requested: &EffectRequested,
) -> Result<StageSettlementFingerprintV1<'_>, KernelError> {
    let EffectInput::Model { request } = requested.input() else {
        return Err(KernelError::InvalidRecordOrder);
    };
    Ok(StageSettlementFingerprintV1::ModelRequestPrepared(
        Box::new(ModelRequestPreparedFingerprintV1 {
            cursor,
            request,
            component: requested.component(),
            output_contract: requested.output_contract(),
            retry_safety: requested.retry_safety(),
            deadline: requested.deadline(),
        }),
    ))
}

pub(super) fn direct_digest(input: &ModelSettled) -> Result<Digest, KernelError> {
    let fingerprint = match &input.outcome {
        ModelSettlement::Completed {
            completion,
            assistant_message,
        } => ModelSettlementFingerprintV1::DirectCompleted(DirectModelCompletedFingerprintV1 {
            turn_id: input.turn_id,
            model_request_id: input.model_request_id,
            completion: EffectCompletedProjection::from(completion),
            assistant_message: MessageProjection::from(assistant_message),
        }),
        ModelSettlement::Failed(failure) => {
            ModelSettlementFingerprintV1::DirectFailed(DirectModelFailedFingerprintV1 {
                turn_id: input.turn_id,
                model_request_id: input.model_request_id,
                failure: EffectFailedProjection::from(failure),
            })
        }
        ModelSettlement::Deferred(deferred) => {
            ModelSettlementFingerprintV1::DirectDeferred(DirectModelDeferredFingerprintV1 {
                turn_id: input.turn_id,
                model_request_id: input.model_request_id,
                deferred: EffectDeferredProjection::from(deferred),
            })
        }
    };
    digest(&fingerprint)
}

pub(super) fn external_digest(input: &ExternalEffectCompletedInput) -> Result<Digest, KernelError> {
    // Failed outcomes always project ExternalFailed so completion-identity
    // classification (TDD §11.3.3 steps 3–4) can run before assistant-presence
    // checks (step 6). Completed outcomes require the assistant in the projection.
    let fingerprint = match (&input.completion.outcome, &input.assistant_message) {
        (
            ExternalEffectOutcome::Completed {
                output,
                usage,
                artifacts,
            },
            Some(assistant_message),
        ) => ModelSettlementFingerprintV1::ExternalCompleted(ExternalModelCompletedFingerprintV1 {
            effect_id: input.completion.effect_id,
            completion_id: &input.completion.completion_id,
            output,
            usage: usage.as_ref().map(UsageProjection::from),
            artifacts: artifacts.iter().map(ArtifactProjection::from).collect(),
            assistant_message: MessageProjection::from(assistant_message),
        }),
        (ExternalEffectOutcome::Completed { .. }, None) => {
            return Err(KernelError::AssistantMessagePresenceMismatch);
        }
        (ExternalEffectOutcome::Failed { error }, _) => {
            ModelSettlementFingerprintV1::ExternalFailed(ExternalModelFailedFingerprintV1 {
                effect_id: input.completion.effect_id,
                completion_id: &input.completion.completion_id,
                error: ErrorProjection::from(error),
            })
        }
    };
    digest(&fingerprint)
}

pub(super) fn completed_record_digest(
    pending: &PendingModelEffect,
    completed: &EffectCompleted,
    assistant_message: &Message,
) -> Result<Digest, KernelError> {
    let fingerprint = if pending.deferred.is_some() {
        if completed.reservation_id().is_some() {
            return Err(KernelError::ModelSettlementMismatch);
        }
        let completion_id = completed
            .completion_id()
            .ok_or(KernelError::ModelSettlementMismatch)?;
        ModelSettlementFingerprintV1::ExternalCompleted(ExternalModelCompletedFingerprintV1 {
            effect_id: completed.effect_id(),
            completion_id,
            output: completed.output(),
            usage: completed.usage().map(UsageProjection::from),
            artifacts: completed
                .artifacts()
                .iter()
                .map(ArtifactProjection::from)
                .collect(),
            assistant_message: MessageProjection::from(assistant_message),
        })
    } else {
        ModelSettlementFingerprintV1::DirectCompleted(DirectModelCompletedFingerprintV1 {
            turn_id: pending.turn_id,
            model_request_id: pending.model_request_id,
            completion: EffectCompletedProjection::from(completed),
            assistant_message: MessageProjection::from(assistant_message),
        })
    };
    digest(&fingerprint)
}

pub(super) fn failed_record_digest(
    pending: &PendingModelEffect,
    failed: &EffectFailed,
) -> Result<Digest, KernelError> {
    let fingerprint = if pending.deferred.is_some() {
        if failed.usage().is_some() {
            return Err(KernelError::ModelSettlementMismatch);
        }
        let completion_id = failed
            .completion_id()
            .ok_or(KernelError::ModelSettlementMismatch)?;
        ModelSettlementFingerprintV1::ExternalFailed(ExternalModelFailedFingerprintV1 {
            effect_id: failed.effect_id(),
            completion_id,
            error: ErrorProjection::from(failed.error()),
        })
    } else {
        ModelSettlementFingerprintV1::DirectFailed(DirectModelFailedFingerprintV1 {
            turn_id: pending.turn_id,
            model_request_id: pending.model_request_id,
            failure: EffectFailedProjection::from(failed),
        })
    };
    digest(&fingerprint)
}

fn digest(fingerprint: &ModelSettlementFingerprintV1<'_>) -> Result<Digest, KernelError> {
    super::canonical_digest("model-settlement", fingerprint)
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let opaque_absent_digest =
            completed_digest(opaque_absent.clone(), Metadata::empty(), vec![]);
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
        error_absent.safe_details =
            Metadata::parse(r#"{"provider_ids":{}}"#).expect("safe details");
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

    fn completed_digest(
        output: RawJson,
        metadata: Metadata,
        artifacts: Vec<ArtifactRef>,
    ) -> Digest {
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
        ErrorDescriptor::new("provider_failed", "failed", ErrorCategory::Model, false)
            .expect("error")
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
}
