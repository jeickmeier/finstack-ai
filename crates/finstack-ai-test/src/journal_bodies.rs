//! One activated [`RecordBody`] per durable family for journal known-answers.

use std::sync::Arc;

use finstack_ai_kernel::{
    AuthorizationEvidence, BudgetChargeReceipt, BudgetChargeRecorded, BudgetReleaseReceipt,
    BudgetReleaseRequest, BudgetRequest, BudgetReservationReceipt, BudgetReservationReleased,
    BudgetReservationRequested, BudgetReservationSettled, BudgetReserveRequest,
    CancellationInitiator, CancellationReconciled, CancellationRequest, CancellationRequested,
    ChildPlacement, ChildRunLocator, ChildRunPrepared, ComponentId, ComponentRef, ContextPrepared,
    Digest, EffectCancelled, EffectCompleted, EffectDeferred, EffectFailed, EffectInput,
    EffectKind, EffectOutputContract, EffectOutputKind, EffectRequested, ErrorCategory, ErrorCode,
    ErrorDescriptor, ExternalHandleRef, Id, IdTag, InteractionCancelled, InteractionExpired,
    InteractionKind, InteractionRequest, InteractionResolution, JsonSchemaDraft, LaneCreated,
    LaneMoved, Message, Metadata, OperationLocator, OutputConfiguration, OutputValidationFailed,
    PrincipalRef, ProviderIds, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RawJson,
    ReconciliationPolicy, RecordBody, RecordDraft, RecordEnvelope, RetryClassification,
    RetrySafety, RetryScheduled, RunAccepted, RunCompleted, RunFailed, RunLimits,
    RunPropagationPolicy, RunRelation, RunSecurityContext, RunSuspended, SchemaRef, SessionCreated,
    SnapshotWritten, StructuredResultSource, TimerFired, Timestamp, Usage, ValidationIssue,
    Version,
};

use crate::paths::compatibility_fixture;
use crate::public_api_fixture::load_public_api_fixture;

/// Every activated record-body family, in `kind_name` order after construction.
///
/// # Errors
///
/// Returns a construction failure when a sample body is invalid.
#[expect(
    clippy::too_many_lines,
    reason = "the catalog lists every activated RecordBody family in one place"
)]
pub fn all_activated_record_bodies() -> Result<Vec<RecordBody>, String> {
    let mut bodies = vec![
        RecordBody::RunAccepted(sample_run_accepted()?),
        RecordBody::EffectRequested(sample_effect_requested()?),
        RecordBody::EffectDeferred(sample_effect_deferred()?),
        RecordBody::EffectCompleted(sample_effect_completed()?),
        RecordBody::EffectFailed(sample_effect_failed()?),
        RecordBody::EffectCancelled(sample_effect_cancelled()?),
        RecordBody::InteractionRequested(sample_interaction_requested()?),
        RecordBody::InteractionResolved(sample_interaction_resolved()?),
        RecordBody::InteractionExpired(InteractionExpired {
            interaction_id: id(40),
            expired_at: ts(),
        }),
        RecordBody::InteractionCancelled(
            InteractionCancelled::try_new(id(41), None, None, None::<&str>).map_err(err)?,
        ),
        body_from_public_api(
            "public-rust-api/v1/pr009-record/valid--stage-outcome-zero-events.json",
        )?,
        RecordBody::ContextPrepared(sample_context_prepared()?),
        body_from_public_api(
            "public-rust-api/v1/pr009-record/valid--entry-appended-derived-event.json",
        )?,
        body_from_public_api(
            "public-rust-api/v1/pr010-record/valid--tool-batch-opened-zero-events.json",
        )?,
        body_from_public_api(
            "public-rust-api/v1/pr010-record/valid--tool-call-settled-two-events.json",
        )?,
        body_from_public_api(
            "public-rust-api/v1/pr010-record/valid--tool-batch-closed-zero-events.json",
        )?,
        RecordBody::CancellationRequested(CancellationRequested {
            request: CancellationRequest::try_new(
                id(50),
                CancellationInitiator::Deadline,
                None::<&str>,
            )
            .map_err(err)?,
        }),
        RecordBody::CancellationReconciled(CancellationReconciled {
            request_id: id(50),
            completed_effects: Arc::from([]),
            cancelled_effects: Arc::from([]),
            uncertain_effects: Arc::from([]),
        }),
        body_from_public_api("public-rust-api/v1/pr011-record/valid--limit-reached-event.json")?,
        RecordBody::RetryScheduled(
            RetryScheduled::try_new(
                0,
                1,
                RetryClassification::Model,
                "policy-v1",
                id(60),
                ts(),
                ErrorDescriptor::new("model_failed", "failed", ErrorCategory::Model, true)
                    .map_err(err)?,
            )
            .map_err(err)?,
        ),
        RecordBody::TimerFired(TimerFired {
            effect_id: id(61),
            due_at: ts(),
            fired_at: ts(),
        }),
        RecordBody::RunSuspended(RunSuspended {
            reason_code: ErrorCode::new("suspended").map_err(err)?,
            cancellation_request_id: None,
        }),
        RecordBody::RunCompleted(RunCompleted {
            cycle: 0,
            turn_id: id(70),
            model_request_id: id(71),
            effect_id: id(72),
            result_message_id: id(73),
            result_digest: digest(),
        }),
        RecordBody::RunFailed(RunFailed {
            cycle: 0,
            turn_id: None,
            model_request_id: None,
            effect_id: None,
            error: ErrorDescriptor::new(
                "run_failed",
                "failed",
                ErrorCategory::Configuration,
                false,
            )
            .map_err(err)?,
        }),
        body_from_public_api("public-rust-api/v1/pr011-record/valid--run-cancelled-event.json")?,
        RecordBody::OutputConfigured(OutputConfiguration::default()),
        body_from_public_api(
            "public-rust-api/v1/pr012-record/valid--capabilities-zero-events.json",
        )?,
        body_from_public_api(
            "public-rust-api/v1/pr012-record/valid--final-result-zero-events.json",
        )?,
        RecordBody::OutputValidationFailed(sample_output_validation_failed()?),
        body_from_public_api(
            "public-rust-api/v1/pr014-record/valid--external-command-rejected-zero-events.json",
        )?,
        RecordBody::ChildRunPrepared(sample_child_run_prepared()?),
        RecordBody::BudgetReservationRequested(sample_budget_requested()?),
        RecordBody::BudgetReservationSettled(sample_budget_settled()?),
        RecordBody::BudgetChargeRecorded(sample_budget_charged()?),
        RecordBody::BudgetReservationReleased(sample_budget_released()?),
        RecordBody::SessionCreated(SessionCreated::new(Metadata::empty())),
        RecordBody::LaneCreated(LaneCreated::try_new("main").map_err(err)?),
        RecordBody::LaneMoved(LaneMoved::new(id(80))),
        RecordBody::SnapshotWritten(SnapshotWritten::new(1, Digest::raw_json(b"snap"))),
    ];
    bodies.sort_by_key(RecordBody::kind_name);
    Ok(bodies)
}

/// Build a draft for `body` with the required derived-event cardinality.
///
/// # Errors
///
/// Returns a record-construction failure.
pub fn draft_for_body(body: RecordBody, ordinal: u64) -> Result<RecordDraft, String> {
    let count = body.derived_event_count(RECORD_KIND_VERSION).map_err(err)?;
    let events = (0..count)
        .map(|index| {
            let index = u64::try_from(index).map_err(err)?;
            Ok(id(900 + ordinal * 10 + index))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let run_id = if body.is_structural() {
        None
    } else if let RecordBody::RunAccepted(accepted) = &body {
        Some(accepted.run_id())
    } else {
        Some(id(3))
    };
    RecordDraft::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        id(100 + ordinal),
        id(1),
        id(2),
        run_id,
        ts(),
        events,
        body,
    )
    .map_err(err)
}

fn body_from_public_api(relative: &str) -> Result<RecordBody, String> {
    let fixture = load_public_api_fixture(compatibility_fixture(relative)).map_err(err)?;
    let input = fixture
        .input
        .ok_or_else(|| format!("missing input in {relative}"))?;
    let envelope: RecordEnvelope = serde_json::from_value(input).map_err(err)?;
    Ok(envelope.body().clone())
}

fn sample_run_accepted() -> Result<RunAccepted, String> {
    let run = id(3);
    RunAccepted::try_new(
        run,
        RunRelation::root(run).map_err(err)?,
        RunSecurityContext::try_new(
            "tenant-a",
            PrincipalRef::try_new("iss", "sub", Some("tenant-a")).map_err(err)?,
            "oidc",
            "high",
            "policy-1",
            "decision-1",
            None,
        )
        .map_err(err)?,
        None,
        RunLimits::empty(),
        RunPropagationPolicy {
            cancellation: finstack_ai_kernel::CancellationPropagation::Cascade,
            deadline: finstack_ai_kernel::DeadlinePropagation::MinimumOfParentAndChild,
            budget: finstack_ai_kernel::BudgetPropagation::SharedScope,
            principal: finstack_ai_kernel::PrincipalPropagation::Inherit,
        },
        digest(),
        None,
    )
    .map_err(err)
}

fn model_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::ModelResponse,
        schema_version: 1,
        schema_digest: digest(),
    }
}

fn sample_effect_requested() -> Result<EffectRequested, String> {
    EffectRequested::try_new(
        id(10),
        EffectKind::Model,
        None,
        None,
        None,
        model_contract(),
        EffectInput::Model {
            request: RawJson::parse(r#"{"messages":[]}"#).map_err(err)?,
        },
        RetrySafety::SafeToRetry,
        None,
    )
    .map_err(err)
}

fn sample_effect_deferred() -> Result<EffectDeferred, String> {
    Ok(EffectDeferred {
        effect_id: id(11),
        handle: ExternalHandleRef::try_new(
            ComponentId::parse("finstack.provider.demo").map_err(err)?,
            "h1",
            RawJson::parse("{}").map_err(err)?,
        )
        .map_err(err)?,
        reconciliation: ReconciliationPolicy::Poll,
        next_poll_at: None,
        expires_at: None,
        output_contract: model_contract(),
    })
}

fn sample_effect_completed() -> Result<EffectCompleted, String> {
    EffectCompleted::try_new(
        id(12),
        model_contract(),
        RawJson::parse("{}").map_err(err)?,
        None,
        Vec::new(),
        ProviderIds::empty(),
        None::<&str>,
        None,
    )
    .map_err(err)
}

fn sample_effect_failed() -> Result<EffectFailed, String> {
    EffectFailed::try_new(
        id(13),
        model_contract(),
        ErrorDescriptor::new("effect_failed", "failed", ErrorCategory::Model, true).map_err(err)?,
        None,
        None::<&str>,
    )
    .map_err(err)
}

fn sample_effect_cancelled() -> Result<EffectCancelled, String> {
    EffectCancelled::try_new(id(14), model_contract(), None::<&str>, None::<&str>).map_err(err)
}

fn sample_interaction_requested() -> Result<InteractionRequest, String> {
    InteractionRequest::try_new(
        1,
        id(20),
        id(21),
        InteractionKind::Approval,
        vec![],
        RawJson::parse(r#"{"type":"boolean"}"#).map_err(err)?,
        ComponentRef::new(
            ComponentId::parse("finstack.policy.approval").map_err(err)?,
            Some(Version {
                major: 1,
                minor: 0,
                patch: 0,
            }),
        ),
        Version {
            major: 1,
            minor: 0,
            patch: 0,
        },
        None,
        None,
        false,
        Metadata::empty(),
    )
    .map_err(err)
}

fn sample_interaction_resolved() -> Result<InteractionResolution, String> {
    InteractionResolution::try_new(
        id(20),
        "res-1",
        PrincipalRef::try_new("iss", "sub", Some("tenant-a")).map_err(err)?,
        AuthorizationEvidence::try_new("policy-v1", "decision-v1").map_err(err)?,
        RawJson::parse("{}").map_err(err)?,
        None::<&str>,
    )
    .map_err(err)
}

fn sample_context_prepared() -> Result<ContextPrepared, String> {
    let messages: Vec<Message> = Vec::new();
    let canonical = serde_json_canonicalizer::to_vec(&messages).map_err(err)?;
    let digest = Digest::domain_separated("model-context", 1, &canonical).map_err(err)?;
    ContextPrepared::try_new(0, id(30), messages, digest).map_err(err)
}

fn sample_output_validation_failed() -> Result<OutputValidationFailed, String> {
    let value = RawJson::parse(r#"{"answer":1}"#).map_err(err)?;
    Ok(OutputValidationFailed {
        cycle: 0,
        turn_id: id(70),
        model_request_id: id(71),
        effect_id: id(72),
        message_id: id(73),
        schema: SchemaRef {
            draft: JsonSchemaDraft::Draft202012,
            schema_version: 1,
            schema_digest: digest(),
        },
        candidate_digest: value.digest(),
        source: StructuredResultSource::JsonBlock { content_index: 0 },
        issues: Arc::from([ValidationIssue::try_new("", "", None::<&str>, "bad").map_err(err)?]),
        feedback: Arc::from("fix"),
        error: ErrorDescriptor::new(
            "structured_output_validation_failed",
            "structured output did not satisfy the configured schema",
            ErrorCategory::Validation,
            true,
        )
        .map_err(err)?,
        skipped_tool_call_ids: Arc::from([]),
    })
}

fn sample_child_run_prepared() -> Result<ChildRunPrepared, String> {
    let prepared = ChildRunPrepared {
        parent_run_id: id(3),
        parent_effect_id: id(90),
        child: ChildRunLocator {
            operation: OperationLocator::try_new("tenant-a", id(1), id(2), id(4)).map_err(err)?,
            remote: None,
        },
        request_digest: digest(),
        placement: ChildPlacement::CompatibleLaneInParentSession,
        budget_reservation_id: None,
    };
    prepared.validate("tenant-a").map_err(err)?;
    Ok(prepared)
}

fn sample_budget_requested() -> Result<BudgetReservationRequested, String> {
    let amount = BudgetRequest::default();
    let request_digest =
        BudgetReserveRequest::compute_digest(id(200), id(201), id(3), &amount).map_err(err)?;
    Ok(BudgetReservationRequested {
        request: BudgetReserveRequest {
            scope_id: id(200),
            reservation_id: id(201),
            run_id: id(3),
            amount,
            request_digest,
        },
    })
}

fn sample_budget_settled() -> Result<BudgetReservationSettled, String> {
    let amount = BudgetRequest::default();
    let request_digest =
        BudgetReserveRequest::compute_digest(id(200), id(201), id(3), &amount).map_err(err)?;
    Ok(BudgetReservationSettled {
        receipt: BudgetReservationReceipt {
            scope_id: id(200),
            reservation_id: id(201),
            reserved: amount.clone(),
            remaining: amount,
            request_digest,
            receipt_digest: digest(),
        },
    })
}

fn sample_budget_charged() -> Result<BudgetChargeRecorded, String> {
    let usage = Usage::empty();
    let canonical = usage.canonical_bytes().map_err(err)?;
    Ok(BudgetChargeRecorded {
        receipt: BudgetChargeReceipt {
            scope_id: id(200),
            reservation_id: id(201),
            effect_id: id(12),
            charged_usage: usage.clone(),
            cumulative_usage: usage,
            usage_digest: Digest::effect_output(&canonical),
            receipt_digest: digest(),
        },
    })
}

fn sample_budget_released() -> Result<BudgetReservationReleased, String> {
    let request_digest =
        BudgetReleaseRequest::compute_digest(id(200), id(201), id(3)).map_err(err)?;
    Ok(BudgetReservationReleased {
        receipt: BudgetReleaseReceipt {
            scope_id: id(200),
            reservation_id: id(201),
            terminal_run_id: id(3),
            released_unused: BudgetRequest::default(),
            request_digest,
            receipt_digest: digest(),
        },
    })
}

fn id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

fn ts() -> Timestamp {
    Timestamp::from_unix_ms(0).expect("epoch")
}

fn digest() -> Digest {
    Digest::raw_json(b"{}")
}

fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}
