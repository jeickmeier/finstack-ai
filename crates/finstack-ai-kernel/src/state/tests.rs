use super::hash_projection;
use super::*;
use crate::content::{ContentBlock, ToolCallBlock};
use crate::reducer::KernelError;

mod validate_tool_state_tests {
    use std::sync::Arc;

    use super::*;
    use crate::conversation::{Message, MessageRole, ProviderIds};
    use crate::primitives::MessageId;
    use crate::primitives::{Metadata, RawJson};
    use crate::records::tools::ToolCallIdentity;

    /// Reference implementation of the authorship check: the nested scan the
    /// indexed version replaced. Any input the two disagree on is a regression
    /// in a fail-closed boundary, so the equivalence is asserted directly.
    fn authored_by_nested_scan(state: &KernelState, identity: &ToolCallIdentity) -> bool {
        state.messages.iter().any(|message| {
            *message.id() == identity.source_message_id
                && message.role() == MessageRole::Assistant
                && message.content().iter().any(
                    |block| matches!(block, ContentBlock::ToolCall(call) if call == &identity.call),
                )
        })
    }

    fn id<T: crate::IdTag>(ordinal: u64) -> crate::Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        crate::Id::from_bytes(bytes)
    }

    fn tool_call(ordinal: u64, name: &str) -> ToolCallBlock {
        ToolCallBlock::try_new(
            id::<crate::ToolCallTag>(ordinal),
            name,
            RawJson::parse(format!(r#"{{"ordinal":{ordinal}}}"#)).expect("arguments"),
        )
        .expect("tool call")
    }

    fn message(message_id: MessageId, role: MessageRole, blocks: Vec<ContentBlock>) -> Message {
        Message::try_new(
            message_id,
            role,
            blocks,
            crate::Timestamp::from_unix_ms(1_000).expect("timestamp"),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("message")
    }

    /// Builds a state whose single tool call is authored by its source message.
    fn authored_state() -> (KernelState, ToolCallIdentity) {
        let source_message_id = id::<crate::MessageTag>(1);
        let call = tool_call(2, "lookup_price");
        let identity = ToolCallIdentity {
            cycle: 0,
            turn_id: id::<crate::TurnTag>(3),
            source_message_id,
            tool_batch_id: None,
            effect_id: None,
            call: call.clone(),
        };
        let state = KernelState {
            state_version: 2,
            messages: Arc::new(vec![message(
                source_message_id,
                MessageRole::Assistant,
                vec![ContentBlock::ToolCall(call.clone())],
            )]),
            tool_calls: [(*call.tool_call_id(), identity.clone())]
                .into_iter()
                .collect(),
            ..KernelState::default()
        };
        (state, identity)
    }

    #[test]
    fn indexed_authorship_matches_the_nested_scan_across_mutations() {
        let (base, identity) = authored_state();

        // Authored: both forms agree it is present.
        assert!(authored_by_nested_scan(&base, &identity));
        assert_eq!(base.validate_tool_state(), Ok(()));

        // Wrong source message id.
        let mut wrong_source = identity.clone();
        wrong_source.source_message_id = id::<crate::MessageTag>(99);
        let mut state = base.clone();
        state.tool_calls = [(*wrong_source.call.tool_call_id(), wrong_source.clone())]
            .into_iter()
            .collect();
        assert!(!authored_by_nested_scan(&state, &wrong_source));
        assert!(state.validate_tool_state().is_err());

        // Author message exists but carries a different call payload.
        let mut different_args = base.clone();
        different_args.messages = Arc::new(vec![message(
            identity.source_message_id,
            MessageRole::Assistant,
            vec![ContentBlock::ToolCall(tool_call(2, "different_tool"))],
        )]);
        assert!(!authored_by_nested_scan(&different_args, &identity));
        assert!(different_args.validate_tool_state().is_err());

        // A non-assistant author is unrepresentable: `Message::try_new` rejects
        // a tool-role message carrying a tool-call block, so the role filter in
        // both forms can only ever see assistant authorship.
        assert!(
            Message::try_new(
                identity.source_message_id,
                MessageRole::Tool,
                vec![ContentBlock::ToolCall(identity.call.clone())],
                crate::Timestamp::from_unix_ms(1_000).expect("timestamp"),
                None,
                ProviderIds::empty(),
                Metadata::empty(),
            )
            .is_err()
        );

        // Same id authored by a non-source message must not satisfy the check.
        let mut other_author = base.clone();
        other_author.messages = Arc::new(vec![message(
            id::<crate::MessageTag>(42),
            MessageRole::Assistant,
            vec![ContentBlock::ToolCall(identity.call.clone())],
        )]);
        assert!(!authored_by_nested_scan(&other_author, &identity));
        assert!(other_author.validate_tool_state().is_err());
    }

    #[test]
    fn duplicate_effect_assignments_are_still_rejected() {
        let (mut state, identity) = authored_state();
        let second_call = tool_call(7, "lookup_quote");
        let effect_id = id::<crate::EffectTag>(11);
        let batch_id = id::<crate::ToolBatchTag>(12);

        let mut first = identity.clone();
        first.effect_id = Some(effect_id);
        first.tool_batch_id = Some(batch_id);
        let mut second = ToolCallIdentity {
            call: second_call.clone(),
            ..identity.clone()
        };
        second.effect_id = Some(effect_id);
        second.tool_batch_id = Some(batch_id);

        state.messages = Arc::new(vec![message(
            identity.source_message_id,
            MessageRole::Assistant,
            vec![
                ContentBlock::ToolCall(identity.call.clone()),
                ContentBlock::ToolCall(second_call.clone()),
            ],
        )]);
        state.tool_calls = [
            (*first.call.tool_call_id(), first),
            (*second.call.tool_call_id(), second),
        ]
        .into_iter()
        .collect();

        assert_eq!(
            state.validate_tool_state(),
            Err(KernelError::InvalidInputPayload {
                field: "tool_state",
                reason_code: "inconsistent",
            }),
            "the same effect id assigned to two calls must stay rejected"
        );
    }
}

mod validate_structured_output_tests {
    use std::sync::Arc;

    use super::*;
    use crate::primitives::{CapabilityId, ErrorCategory, ErrorDescriptor};
    use crate::records::policy::{
        ActiveCapability, CapabilityActivationSource, FinalResultRecorded, JsonSchemaDraft,
        OutputConfiguration, OutputEndStrategy, OutputSpec, OutputValidationFailed, SchemaRef,
        StructuredResultSource,
    };

    fn id<T: crate::IdTag>(ordinal: u64) -> crate::Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        crate::Id::from_bytes(bytes)
    }

    fn accepted_state(state_version: u16) -> KernelState {
        let timestamp = crate::Timestamp::from_unix_ms(1_000).expect("timestamp");
        let run_id = id::<crate::RunTag>(12);
        let accepted = crate::RunAccepted::try_new(
            run_id,
            crate::RunRelation::root(run_id).expect("root relation"),
            crate::RunSecurityContext::try_new(
                "tenant-a",
                crate::PrincipalRef::try_new("issuer", "subject", Some("tenant-a"))
                    .expect("principal"),
                "oidc",
                "high",
                "policy-v1",
                "decision-v1",
                None,
            )
            .expect("security"),
            None,
            crate::RunLimits::empty(),
            crate::RunPropagationPolicy {
                cancellation: crate::CancellationPropagation::Cascade,
                deadline: crate::DeadlinePropagation::MinimumOfParentAndChild,
                budget: crate::BudgetPropagation::ReservedChildAllocation,
                principal: crate::PrincipalPropagation::Inherit,
            },
            crate::Digest::raw_json(b"agent-lock"),
            None,
        )
        .expect("accepted");
        KernelState {
            state_version,
            session_id: Some(id::<crate::SessionTag>(10)),
            lane_id: Some(id::<crate::LaneTag>(11)),
            accepted: Some(accepted),
            accepted_at: Some(timestamp),
            ..KernelState::default()
        }
    }

    fn dummy_schema() -> SchemaRef {
        SchemaRef {
            draft: JsonSchemaDraft::Draft202012,
            schema_version: 1,
            schema_digest: crate::Digest::raw_json(b"{}"),
        }
    }

    fn dummy_final_result() -> FinalResultRecorded {
        let value = crate::RawJson::parse("{}").expect("value");
        FinalResultRecorded {
            cycle: 0,
            turn_id: id::<crate::TurnTag>(1),
            model_request_id: id::<crate::ModelRequestTag>(2),
            effect_id: id::<crate::EffectTag>(3),
            message_id: id::<crate::MessageTag>(4),
            schema: dummy_schema(),
            value_digest: value.digest(),
            value,
            source: StructuredResultSource::JsonBlock { content_index: 0 },
            end_strategy: OutputEndStrategy::Exhaustive,
            skipped_tool_call_ids: Arc::from([]),
        }
    }

    fn dummy_validation_failure() -> OutputValidationFailed {
        OutputValidationFailed {
            cycle: 0,
            turn_id: id::<crate::TurnTag>(1),
            model_request_id: id::<crate::ModelRequestTag>(2),
            effect_id: id::<crate::EffectTag>(3),
            message_id: id::<crate::MessageTag>(4),
            schema: dummy_schema(),
            candidate_digest: crate::Digest::raw_json(b"{}"),
            source: StructuredResultSource::JsonBlock { content_index: 0 },
            issues: Arc::from([]),
            feedback: Arc::from("invalid"),
            error: ErrorDescriptor::new(
                "invalid_input",
                "payload rejected",
                ErrorCategory::Validation,
                false,
            )
            .expect("error"),
            skipped_tool_call_ids: Arc::from([]),
        }
    }

    #[test]
    fn structured_output_rejections_apply_at_versions_4_5_and_6() {
        for version in [4_u16, 5, 6] {
            let mut invalid_configuration = accepted_state(version);
            invalid_configuration.output_configuration = Some(OutputConfiguration {
                output: OutputSpec::JsonSchema {
                    schema: SchemaRef {
                        draft: JsonSchemaDraft::Draft202012,
                        schema_version: 0,
                        schema_digest: crate::Digest::raw_json(b"{}"),
                    },
                },
                end_strategy: OutputEndStrategy::Exhaustive,
            });
            assert_eq!(
                invalid_configuration.validate(),
                Err(KernelError::InvalidInputPayload {
                    field: "output_configuration",
                    reason_code: "invalid",
                }),
                "invalid output_configuration at v{version}"
            );

            let mut invalid_capabilities = accepted_state(version);
            invalid_capabilities.active_capabilities = Arc::from([
                ActiveCapability {
                    capability_id: CapabilityId::parse("cap.z").expect("later"),
                    source: CapabilityActivationSource::Always,
                },
                ActiveCapability {
                    capability_id: CapabilityId::parse("cap.a").expect("earlier"),
                    source: CapabilityActivationSource::Always,
                },
            ]);
            assert_eq!(
                invalid_capabilities.validate(),
                Err(KernelError::InvalidInputPayload {
                    field: "active_capabilities",
                    reason_code: "invalid_capability_set",
                }),
                "invalid capability set at v{version}"
            );

            let mut conflicting = accepted_state(version);
            conflicting.final_result = Some(dummy_final_result());
            conflicting.validation_failure = Some(dummy_validation_failure());
            assert_eq!(
                conflicting.validate(),
                Err(KernelError::InvalidInputPayload {
                    field: "structured_output",
                    reason_code: "conflicting_outcomes",
                }),
                "conflicting structured outcomes at v{version}"
            );
        }
    }
}

mod validate_control_state_tests {
    use std::sync::Arc;

    use super::*;
    use crate::primitives::{ErrorCategory, ErrorCode, ErrorDescriptor};
    use crate::records::lifecycle::{RunCancelled, RunCompleted, RunFailed, RunSuspended};
    use crate::records::run::{CancellationInitiator, CancellationRequest};
    use crate::state::CancellationState;

    fn id<T: crate::IdTag>(ordinal: u64) -> crate::Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        crate::Id::from_bytes(bytes)
    }

    fn accepted_v3() -> KernelState {
        let timestamp = crate::Timestamp::from_unix_ms(1_000).expect("timestamp");
        let run_id = id::<crate::RunTag>(12);
        let accepted = crate::RunAccepted::try_new(
            run_id,
            crate::RunRelation::root(run_id).expect("root relation"),
            crate::RunSecurityContext::try_new(
                "tenant-a",
                crate::PrincipalRef::try_new("issuer", "subject", Some("tenant-a"))
                    .expect("principal"),
                "oidc",
                "high",
                "policy-v1",
                "decision-v1",
                None,
            )
            .expect("security"),
            None,
            crate::RunLimits::empty(),
            crate::RunPropagationPolicy {
                cancellation: crate::CancellationPropagation::Cascade,
                deadline: crate::DeadlinePropagation::MinimumOfParentAndChild,
                budget: crate::BudgetPropagation::ReservedChildAllocation,
                principal: crate::PrincipalPropagation::Inherit,
            },
            crate::Digest::raw_json(b"agent-lock"),
            None,
        )
        .expect("accepted");
        KernelState {
            state_version: 3,
            session_id: Some(id::<crate::SessionTag>(10)),
            lane_id: Some(id::<crate::LaneTag>(11)),
            accepted: Some(accepted),
            accepted_at: Some(timestamp),
            phase: Some(RunPhase::BeforeFinalize),
            ..KernelState::default()
        }
    }

    fn completed_payload() -> RunCompleted {
        RunCompleted {
            cycle: 0,
            turn_id: id::<crate::TurnTag>(1),
            model_request_id: id::<crate::ModelRequestTag>(2),
            effect_id: id::<crate::EffectTag>(3),
            result_message_id: id::<crate::MessageTag>(4),
            result_digest: crate::Digest::raw_json(b"result"),
        }
    }

    fn failed_payload() -> RunFailed {
        RunFailed {
            cycle: 0,
            turn_id: None,
            model_request_id: None,
            effect_id: None,
            error: ErrorDescriptor::new(
                "limit_reached",
                "configured run limit reached",
                ErrorCategory::Limit,
                false,
            )
            .expect("error"),
        }
    }

    fn cancelled_payload() -> RunCancelled {
        RunCancelled {
            request_id: id::<crate::CancellationRequestTag>(20),
            reason_code: ErrorCode::new("cancelled").expect("reason"),
        }
    }

    fn cancellation_state() -> CancellationState {
        CancellationState {
            request: CancellationRequest::try_new(
                id::<crate::CancellationRequestTag>(20),
                CancellationInitiator::Deadline,
                None::<&str>,
            )
            .expect("request"),
            prior_phase: RunPhase::BeforeFinalize,
            completed_effects: Arc::from([]),
            cancelled_effects: Arc::from([]),
            uncertain_effects: Arc::from([]),
            outstanding_effects: Arc::from([]),
        }
    }

    #[test]
    fn terminal_phase_without_payload_is_rejected_at_every_state_version() {
        for (phase, payload) in [
            (
                RunPhase::Completed,
                TerminalState::Completed(completed_payload()),
            ),
            (RunPhase::Failed, TerminalState::Failed(failed_payload())),
            (
                RunPhase::Cancelled,
                TerminalState::Cancelled(cancelled_payload()),
            ),
        ] {
            let mut paired = accepted_v3();
            if matches!(phase, RunPhase::Cancelled) {
                paired.cancellation = Some(cancellation_state());
            }
            paired.phase = Some(phase);
            paired.terminal = Some(payload);
            assert_eq!(
                paired.validate(),
                Ok(()),
                "apply-paired {phase:?} must remain legal"
            );

            let mut unpaired = paired.clone();
            unpaired.terminal = None;
            assert_eq!(
                unpaired.validate(),
                Err(KernelError::InvalidInputPayload {
                    field: "terminal",
                    reason_code: "inconsistent_phase",
                }),
                "terminal phase {phase:?} without payload is unproducible"
            );
        }

        let mut v1_completed = KernelState {
            phase: Some(RunPhase::Completed),
            terminal: Some(TerminalState::Completed(completed_payload())),
            ..KernelState::default()
        };
        assert_eq!(v1_completed.validate(), Ok(()));
        v1_completed.terminal = None;
        assert_eq!(
            v1_completed.validate(),
            Err(KernelError::InvalidInputPayload {
                field: "terminal",
                reason_code: "inconsistent_phase",
            })
        );
    }

    #[test]
    fn cancelling_or_cancelled_without_cancellation_is_rejected() {
        for phase in [RunPhase::Cancelling, RunPhase::Cancelled] {
            let mut paired = accepted_v3();
            paired.phase = Some(phase);
            paired.cancellation = Some(cancellation_state());
            if phase == RunPhase::Cancelled {
                paired.terminal = Some(TerminalState::Cancelled(cancelled_payload()));
            }
            assert_eq!(paired.validate(), Ok(()), "apply-paired {phase:?}");

            let mut unpaired = paired.clone();
            unpaired.cancellation = None;
            assert_eq!(
                unpaired.validate(),
                Err(KernelError::InvalidInputPayload {
                    field: "cancellation",
                    reason_code: "inconsistent",
                }),
                "{phase:?} without cancellation is unproducible"
            );
        }
    }

    #[test]
    fn suspended_without_suspension_is_rejected() {
        let mut paired = accepted_v3();
        paired.phase = Some(RunPhase::Suspended);
        paired.suspension = Some(RunSuspended {
            reason_code: ErrorCode::new("unknown_cost_usage").expect("reason"),
            cancellation_request_id: None,
        });
        assert_eq!(paired.validate(), Ok(()));

        let mut unpaired = paired.clone();
        unpaired.suspension = None;
        assert_eq!(
            unpaired.validate(),
            Err(KernelError::InvalidInputPayload {
                field: "suspension",
                reason_code: "inconsistent",
            })
        );
    }

    #[test]
    fn already_enforced_validate_gaps_still_reject() {
        let mut pending_without_phase = accepted_v3();
        pending_without_phase.state_version = 6;
        pending_without_phase.pending_interaction = Some(crate::state::PendingInteraction {
            request: crate::effects::InteractionRequest::try_new(
                1,
                id::<crate::InteractionTag>(20),
                id::<crate::EffectTag>(21),
                crate::effects::InteractionKind::Approval,
                vec![],
                crate::RawJson::parse("{}").expect("schema"),
                crate::ComponentRef::new(
                    crate::ComponentId::parse("policy.approval").expect("component"),
                    None,
                ),
                crate::Version {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
                None,
                None,
                false,
                crate::Metadata::empty(),
            )
            .expect("request"),
            prior_phase: RunPhase::BeforeFinalize,
            cursor: crate::records::lifecycle::StageCursor {
                cycle: 0,
                stage: crate::records::lifecycle::Stage::BeforeFinalize,
            },
        });
        assert_eq!(
            pending_without_phase.validate(),
            Err(KernelError::InvalidInputPayload {
                field: "pending_interaction",
                reason_code: "inconsistent",
            })
        );

        let mut accepted_at_mismatch = accepted_v3();
        accepted_at_mismatch.accepted_at = None;
        assert_eq!(
            accepted_at_mismatch.validate(),
            Err(KernelError::InvalidInputPayload {
                field: "accepted_at",
                reason_code: "inconsistent",
            })
        );
    }
}

mod v1_v2_snapshot_sidecar_tests {
    use super::*;
    use crate::records::policy::LimitUsage;

    fn id<T: crate::IdTag>(ordinal: u64) -> crate::Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        crate::Id::from_bytes(bytes)
    }

    fn accepted_v1_after_prepare() -> KernelState {
        let timestamp = crate::Timestamp::from_unix_ms(1_000).expect("timestamp");
        let run_id = id::<crate::RunTag>(12);
        let accepted = crate::RunAccepted::try_new(
            run_id,
            crate::RunRelation::root(run_id).expect("root relation"),
            crate::RunSecurityContext::try_new(
                "tenant-a",
                crate::PrincipalRef::try_new("issuer", "subject", Some("tenant-a"))
                    .expect("principal"),
                "oidc",
                "high",
                "policy-v1",
                "decision-v1",
                None,
            )
            .expect("security"),
            None,
            crate::RunLimits::empty(),
            crate::RunPropagationPolicy {
                cancellation: crate::CancellationPropagation::Cascade,
                deadline: crate::DeadlinePropagation::MinimumOfParentAndChild,
                budget: crate::BudgetPropagation::ReservedChildAllocation,
                principal: crate::PrincipalPropagation::Inherit,
            },
            crate::Digest::raw_json(b"agent-lock"),
            None,
        )
        .expect("accepted");
        KernelState {
            state_version: 1,
            session_id: Some(id::<crate::SessionTag>(10)),
            lane_id: Some(id::<crate::LaneTag>(11)),
            accepted: Some(accepted),
            accepted_at: Some(timestamp),
            phase: Some(RunPhase::BeforeModel),
            limit_usage: LimitUsage {
                turns: 1,
                context_bytes: 32,
                ..LimitUsage::default()
            },
            ..KernelState::default()
        }
    }

    #[test]
    fn v1_snapshot_sidecars_round_trip_without_changing_hash() {
        let state = accepted_v1_after_prepare();
        let before_hash = state.state_hash().expect("v1 hash");
        let json = serde_json::to_value(&state).expect("serialize v1");
        assert!(json.get("accepted_at").is_none());
        assert!(json.get("limit_usage").is_none());
        assert!(json.get("snapshot_accepted_at").is_some());
        assert!(json.get("snapshot_limit_usage").is_some());
        let restored: KernelState = serde_json::from_value(json).expect("restore v1");
        assert_eq!(restored.accepted_at, state.accepted_at);
        assert_eq!(restored.limit_usage, state.limit_usage);
        assert_eq!(restored.state_hash().expect("restored hash"), before_hash);
    }

    #[test]
    fn v1_snapshot_rejects_v3_control_field_names() {
        let state = accepted_v1_after_prepare();
        let mut with_accepted_at = serde_json::to_value(&state).expect("serialize v1");
        with_accepted_at["accepted_at"] = serde_json::json!(1_000);
        serde_json::from_value::<KernelState>(with_accepted_at)
            .expect_err("v1 must reject accepted_at");

        let mut with_cancellation = serde_json::to_value(&state).expect("serialize v1");
        with_cancellation["cancellation"] = serde_json::Value::Null;
        serde_json::from_value::<KernelState>(with_cancellation)
            .expect_err("v1 must reject cancellation");
    }

    #[test]
    fn old_v1_snapshots_without_sidecars_restore_defaults() {
        let json = serde_json::to_value(KernelState::default()).expect("default v1");
        assert!(json.get("snapshot_accepted_at").is_none());
        assert!(json.get("snapshot_limit_usage").is_none());
        let restored: KernelState = serde_json::from_value(json).expect("restore default");
        assert!(restored.accepted_at.is_none());
        assert_eq!(restored.limit_usage, LimitUsage::default());
    }
}

mod completion_identity_hash_entry_decode_bounds_tests {
    use serde_json::{Value, json};

    use super::super::types::CompletionIdentityHashEntryV1;
    use crate::content::LABEL_MAX_BYTES;

    fn id<T: crate::IdTag>(ordinal: u64) -> crate::Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        crate::Id::from_bytes(bytes)
    }

    fn completion_entry(completion_id: &str) -> Value {
        json!({
            "completion_id": completion_id,
            "effect_id": id::<crate::EffectTag>(1),
            "settlement_digest": crate::Digest::raw_json(b"settlement"),
        })
    }

    #[test]
    fn completion_identity_hash_entry_enforces_label_bounds_and_unknown_fields() {
        assert!(
            serde_json::from_value::<CompletionIdentityHashEntryV1>(completion_entry(
                &"x".repeat(LABEL_MAX_BYTES)
            ))
            .is_ok()
        );
        assert!(
            serde_json::from_value::<CompletionIdentityHashEntryV1>(completion_entry(
                &"x".repeat(LABEL_MAX_BYTES + 1)
            ))
            .is_err()
        );
        let mut unknown = completion_entry("valid");
        unknown
            .as_object_mut()
            .expect("completion entry")
            .insert("future".to_owned(), Value::Bool(true));
        assert!(serde_json::from_value::<CompletionIdentityHashEntryV1>(unknown).is_err());
    }
}

mod kernel_state_eq_tests {
    use super::*;

    #[test]
    fn eq_is_wire_json_not_state_hash_projection() {
        let left = KernelState::default();
        let right = KernelState::default();
        let left_wire = serde_json_canonicalizer::to_vec(&left).expect("left wire");
        let right_wire = serde_json_canonicalizer::to_vec(&right).expect("right wire");
        assert_eq!(left_wire, right_wire);
        assert_eq!(left, right);

        let hash_projection = hash_projection::KernelStateHashV1::from_state(&left);
        let hash_json =
            serde_json_canonicalizer::to_vec(&hash_projection).expect("hash projection");
        assert_ne!(
            left_wire, hash_json,
            "wire Serialize and hash projection must stay distinct"
        );

        let mut writer = crate::primitives::DigestWriter::new("kernel-state", 1).expect("writer");
        serde_json_canonicalizer::to_writer(&hash_projection, &mut writer).expect("hash write");
        let (digest, _) = writer.finish();
        assert_eq!(left.state_hash().expect("hash"), digest);
        assert_eq!(
            left.state_hash().expect("hash"),
            right.state_hash().expect("hash")
        );
    }
}

mod hash_projection_explicit_null_tests {
    use std::sync::Arc;

    use super::*;
    use crate::effects::{InteractionKind, InteractionRequest};
    use crate::primitives::{
        ComponentId, ComponentRef, ErrorCategory, ErrorDescriptor, Usage, Version,
    };
    use crate::records::lifecycle::{Stage, StageCursor};
    use crate::records::policy::{
        BudgetChargeReceipt, BudgetRequest, BudgetReserveRequest, JsonSchemaDraft, LimitDimension,
        LimitReached, LimitUsage, LimitValue, OutputConfiguration, OutputValidationFailed,
        SchemaRef, StructuredResultSource, ValidationIssue,
    };
    use crate::records::run::{
        ChildPlacement, ChildRunLocator, ChildRunPrepared, OperationLocator,
    };

    fn id<T: crate::IdTag>(ordinal: u64) -> crate::Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        crate::Id::from_bytes(bytes)
    }

    fn v4_json(state: &KernelState) -> serde_json::Value {
        serde_json::to_value(hash_projection::KernelStateHashV4::from_state(state))
            .expect("v4 json")
    }

    fn v5_json(state: &KernelState) -> serde_json::Value {
        serde_json::to_value(hash_projection::KernelStateHashV5::from_state(state))
            .expect("v5 json")
    }

    fn v6_json(state: &KernelState) -> serde_json::Value {
        serde_json::to_value(hash_projection::KernelStateHashV6::from_state(state))
            .expect("v6 json")
    }

    fn dummy_schema() -> SchemaRef {
        SchemaRef {
            draft: JsonSchemaDraft::Draft202012,
            schema_version: 1,
            schema_digest: crate::Digest::raw_json(b"{}"),
        }
    }

    fn last_limit_without_cost() -> LimitReached {
        let usage = LimitUsage::default();
        LimitReached {
            dimension: LimitDimension::Turns,
            observed: LimitValue::Count(1),
            maximum: LimitValue::Count(0),
            usage_digest: usage.digest().expect("usage digest"),
            usage,
        }
    }

    #[test]
    fn v4_absent_control_fields_serialize_as_null() {
        let json = v4_json(&KernelState::default());
        assert_eq!(json["last_limit"], serde_json::Value::Null);
        assert_eq!(json["output_configuration"], serde_json::Value::Null);
        assert_eq!(json["final_result"], serde_json::Value::Null);
        assert_eq!(json["validation_failure"], serde_json::Value::Null);
    }

    #[test]
    fn v3_last_limit_still_omits_absent_usage_optionals() {
        let mut state = KernelState {
            state_version: 3,
            last_limit: Some(last_limit_without_cost()),
            ..KernelState::default()
        };
        let json = serde_json::to_value(hash_projection::KernelStateHashV3::from_state(&state))
            .expect("v3 json");
        let usage = json["last_limit"]["usage"]
            .as_object()
            .expect("raw last_limit usage");
        assert!(
            !usage.contains_key("cost"),
            "v3 must keep the human-readable LimitUsage omission"
        );
        assert!(
            !usage.contains_key("extension_counters"),
            "v3 must keep the human-readable LimitUsage omission"
        );
        state.state_version = 4;
        let v4_json = v4_json(&state);
        let v4_usage = v4_json["last_limit"]["usage"]
            .as_object()
            .expect("projected last_limit usage");
        assert_eq!(v4_usage["cost"], serde_json::Value::Null);
        assert_eq!(v4_usage["extension_counters"], serde_json::json!([]));
    }

    #[test]
    fn validation_failure_projects_error_identifiers_as_explicit_nulls() {
        let state = KernelState {
            validation_failure: Some(OutputValidationFailed {
                cycle: 0,
                turn_id: id::<crate::TurnTag>(1),
                model_request_id: id::<crate::ModelRequestTag>(2),
                effect_id: id::<crate::EffectTag>(3),
                message_id: id::<crate::MessageTag>(4),
                schema: dummy_schema(),
                candidate_digest: crate::Digest::raw_json(b"{}"),
                source: StructuredResultSource::JsonBlock { content_index: 0 },
                issues: Arc::from([ValidationIssue::try_new(
                    "/",
                    "/properties/name",
                    None::<&str>,
                    "missing name",
                )
                .expect("issue")]),
                feedback: Arc::from("invalid"),
                error: ErrorDescriptor::new(
                    "structured_output_validation_failed",
                    "structured output did not satisfy the configured schema",
                    ErrorCategory::Validation,
                    true,
                )
                .expect("error"),
                skipped_tool_call_ids: Arc::from([]),
            }),
            ..KernelState::default()
        };
        let failure = &v4_json(&state)["validation_failure"];
        assert_eq!(failure["issues"][0]["keyword"], serde_json::Value::Null);
        let identifiers = failure["error"]["identifiers"]
            .as_object()
            .expect("error identifiers");
        for key in [
            "session_id",
            "lane_id",
            "run_id",
            "turn_id",
            "message_id",
            "model_request_id",
            "tool_batch_id",
            "tool_call_id",
            "effect_id",
            "interaction_id",
            "event_id",
            "budget_scope_id",
            "budget_reservation_id",
            "cancellation_request_id",
            "record_id",
            "append_batch_id",
            "artifact_id",
        ] {
            assert_eq!(
                identifiers[key],
                serde_json::Value::Null,
                "{key} must serialize as null"
            );
        }
    }

    #[test]
    fn v5_child_and_budget_optionals_serialize_as_null() {
        let parent_effect_id = id::<crate::EffectTag>(3);
        let reservation_id = id::<crate::BudgetReservationTag>(15);
        let mut state = KernelState::default();
        state.child_preparations.insert(
            parent_effect_id,
            ChildRunPrepared {
                parent_run_id: id::<crate::RunTag>(12),
                parent_effect_id,
                child: ChildRunLocator {
                    operation: OperationLocator::try_new(
                        "tenant-a",
                        id::<crate::SessionTag>(10),
                        id::<crate::LaneTag>(11),
                        id::<crate::RunTag>(13),
                    )
                    .expect("locator"),
                    remote: None,
                },
                request_digest: crate::Digest::raw_json(b"child-request"),
                placement: ChildPlacement::CompatibleLaneInParentSession,
                budget_reservation_id: None,
            },
        );
        state.budget_reservations.insert(
            reservation_id,
            BudgetReservationReplay {
                request: BudgetReserveRequest {
                    scope_id: id::<crate::BudgetScopeTag>(14),
                    reservation_id,
                    run_id: id::<crate::RunTag>(13),
                    amount: BudgetRequest::default(),
                    request_digest: crate::Digest::raw_json(b"reserve"),
                },
                settlement: None,
                release: None,
            },
        );
        state.budget_charges.insert(
            id::<crate::EffectTag>(4),
            BudgetChargeReceipt {
                scope_id: id::<crate::BudgetScopeTag>(14),
                reservation_id,
                effect_id: id::<crate::EffectTag>(4),
                charged_usage: Usage::empty(),
                cumulative_usage: Usage::empty(),
                usage_digest: crate::Digest::raw_json(b"usage"),
                receipt_digest: crate::Digest::raw_json(b"charge"),
            },
        );

        let json = v5_json(&state);
        let child = &json["child_preparations"][0];
        assert_eq!(child["budget_reservation_id"], serde_json::Value::Null);
        assert_eq!(child["child"]["remote"], serde_json::Value::Null);

        let reservation = &json["budget_reservations"][0];
        assert_eq!(reservation["settlement"], serde_json::Value::Null);
        assert_eq!(reservation["release"], serde_json::Value::Null);
        let amount = &reservation["request"]["amount"];
        assert_eq!(amount["input_tokens"], serde_json::Value::Null);
        assert_eq!(amount["output_tokens"], serde_json::Value::Null);
        assert_eq!(amount["cost"], serde_json::Value::Null);
        assert_eq!(amount["extension_counters"], serde_json::json!({}));

        let charge = &json["budget_charges"][0];
        for usage_key in ["charged_usage", "cumulative_usage"] {
            let usage = &charge[usage_key];
            assert_eq!(usage["input_tokens"], serde_json::Value::Null);
            assert_eq!(usage["output_tokens"], serde_json::Value::Null);
            assert_eq!(usage["total_tokens"], serde_json::Value::Null);
            assert_eq!(usage["cost"], serde_json::Value::Null);
            assert_eq!(usage["extension_counters"], serde_json::json!({}));
        }
    }

    #[test]
    fn v6_interaction_optionals_serialize_as_null() {
        let state = KernelState {
            output_configuration: Some(OutputConfiguration::default()),
            pending_interaction: Some(PendingInteraction {
                request: InteractionRequest::try_new(
                    1,
                    id::<crate::InteractionTag>(20),
                    id::<crate::EffectTag>(21),
                    InteractionKind::Approval,
                    vec![],
                    crate::RawJson::parse("{}").expect("schema"),
                    ComponentRef::new(
                        ComponentId::parse("policy.approval").expect("component"),
                        None,
                    ),
                    Version {
                        major: 1,
                        minor: 0,
                        patch: 0,
                    },
                    None,
                    None,
                    false,
                    crate::Metadata::empty(),
                )
                .expect("request"),
                prior_phase: RunPhase::BeforeFinalize,
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::BeforeFinalize,
                },
            }),
            last_interaction_terminal: Some(InteractionTerminal {
                interaction_id: id::<crate::InteractionTag>(20),
                kind: InteractionKind::Approval,
                cursor: StageCursor {
                    cycle: 0,
                    stage: Stage::BeforeFinalize,
                },
                outcome: InteractionTerminalOutcome::Granted,
            }),
            ..KernelState::default()
        };

        let json = v6_json(&state);
        assert!(json["output_configuration"].is_object());
        let request = &json["pending_interaction"]["request"];
        assert_eq!(request["assignee_hint"], serde_json::Value::Null);
        assert_eq!(request["expires_at"], serde_json::Value::Null);
        assert_eq!(
            request["policy_component"]["version"],
            serde_json::Value::Null
        );
        assert!(json["last_interaction_terminal"].is_object());
    }
}
