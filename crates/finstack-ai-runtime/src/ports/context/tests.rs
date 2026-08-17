use super::port::CONTEXT_STAGE;
use super::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use finstack_ai_kernel::{
    CapabilityId, ComponentId, ComponentInvocation, ContentBlock, Digest, EffectCompleted,
    EffectInput, EffectKind, EffectOutputContract, EffectOutputKind, EffectRequested, EventTag, Id,
    IdTag, InvocationRecovery, LaneTag, Message, MessageRole, Metadata, PipelinePosition,
    PrincipalRef, ProviderIds, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RecordBody,
    RecordEnvelope, RecordTag, RetrySafety, RunTag, Sensitivity, SessionTag, TextBlock, Timestamp,
    Version,
};

use crate::{AuthorizationContext, CancellationSignal, PortFuture, RunCallContext};

fn id<T: IdTag>(value: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    bytes[9..].copy_from_slice(&value.to_be_bytes()[1..]);
    Id::from_bytes(bytes)
}

fn item(priority: i32, source: &str, authority: ContextAuthority) -> ContextItem {
    ContextItem::try_new(
        ContextItemKind::QuotedSource,
        vec![ContentBlock::Text(
            TextBlock::try_new(source).expect("text"),
        )],
        ContextProvenance {
            source_id: Arc::from(source),
            source_ref: None,
            external: true,
        },
        authority,
        priority,
        1,
        Sensitivity::Internal,
        false,
    )
    .expect("item")
}

fn message(ordinal: u64, role: MessageRole, text: &str) -> Message {
    Message::try_new(
        id(ordinal),
        role,
        vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
        Timestamp::from_unix_ms(i64::try_from(ordinal).expect("timestamp")).expect("timestamp"),
        None,
        ProviderIds::empty(),
        Metadata::empty(),
    )
    .expect("message")
}

#[test]
fn explicit_truncation_is_deterministic_and_diagnostic() {
    let contribution = ContextContribution::try_new(
        vec![
            item(1, "low", ContextAuthority::Untrusted),
            item(10, "high", ContextAuthority::Untrusted),
        ],
        None::<&str>,
    )
    .expect("contribution");
    let result = assemble_context(
        vec![RecordedContextContribution {
            component: ComponentId::parse("fixture.context").expect("component"),
            provider_index: 0,
            contribution,
        }],
        ContextBudget {
            max_items: 1,
            max_tokens: 2,
            max_bytes: u64::MAX,
            overflow: ContextOverflowPolicy::TruncateWithDiagnostic,
        },
    )
    .expect("assembly");
    assert_eq!(result.items.len(), 1);
    assert_eq!(result.items[0].priority, 10);
    assert_eq!(result.diagnostics[0].dropped_items, 1);
}

#[test]
fn provider_finish_order_cannot_change_assembly_and_chain_gaps_fail() {
    let first = RecordedContextContribution {
        component: ComponentId::parse("fixture.context-first").expect("component"),
        provider_index: 0,
        contribution: ContextContribution::try_new(
            vec![item(1, "first", ContextAuthority::Untrusted)],
            None::<&str>,
        )
        .expect("contribution"),
    };
    let second = RecordedContextContribution {
        component: ComponentId::parse("fixture.context-second").expect("component"),
        provider_index: 1,
        contribution: ContextContribution::try_new(
            vec![item(100, "second", ContextAuthority::Untrusted)],
            None::<&str>,
        )
        .expect("contribution"),
    };
    let budget = ContextBudget {
        max_items: 8,
        max_tokens: 128,
        max_bytes: u64::MAX,
        overflow: ContextOverflowPolicy::Reject,
    };
    let completion_order =
        assemble_context(vec![second.clone(), first.clone()], budget).expect("completion order");
    let declaration_order =
        assemble_context(vec![first.clone(), second.clone()], budget).expect("declaration");
    assert_eq!(completion_order, declaration_order);
    assert_eq!(
        completion_order.items[0].provenance.source_id.as_ref(),
        "first"
    );

    let mut gap = second;
    gap.provider_index = 2;
    assert_eq!(
        assemble_context(vec![first, gap], budget)
            .expect_err("chain gap")
            .code(),
        CONTEXT_CONFIGURATION_INVALID
    );
}

#[test]
fn authority_projection_has_exact_fixed_groups_and_current_user_once_last() {
    let capability = ContextItem::try_new(
        ContextItemKind::Instruction,
        vec![ContentBlock::Text(
            TextBlock::try_new("capability").expect("text"),
        )],
        ContextProvenance {
            source_id: Arc::from("capability"),
            source_ref: None,
            external: false,
        },
        ContextAuthority::TrustedApplication,
        0,
        1,
        Sensitivity::Internal,
        true,
    )
    .expect("capability");
    let mut reminder = item(0, "reminder", ContextAuthority::TrustedApplication);
    reminder.kind = ContextItemKind::Instruction;
    reminder.provenance.external = false;
    let providers = AssembledContext {
        items: Arc::from([reminder, item(0, "external", ContextAuthority::Untrusted)]),
        diagnostics: Arc::from([]),
        estimated_tokens: 2,
        bytes: 0,
    };
    let projection = assemble_context_projection(ContextProjectionInput {
        system: Arc::from([message(1, MessageRole::System, "system")]),
        developer: Arc::from([message(2, MessageRole::Developer, "developer")]),
        capabilities: Arc::from([CapabilityContext {
            capability_id: CapabilityId::parse("fixture.capability").expect("capability id"),
            instructions: Arc::from([capability]),
        }]),
        providers,
        history: Arc::from([message(3, MessageRole::Assistant, "history")]),
        current_user: message(4, MessageRole::User, "current"),
    })
    .expect("projection");
    let sources = projection
        .iter()
        .map(|item| match item {
            ContextProjectionItem::Message { source, .. }
            | ContextProjectionItem::Context { source, .. } => *source,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        sources,
        vec![
            ContextProjectionSource::System,
            ContextProjectionSource::Developer,
            ContextProjectionSource::Capability,
            ContextProjectionSource::ProviderReminder,
            ContextProjectionSource::ExternalContext,
            ContextProjectionSource::History,
            ContextProjectionSource::CurrentUser,
        ]
    );
}

#[cfg(feature = "native-tokio")]
struct FixtureProvider {
    descriptor: ContextProviderDescriptor,
    calls: Arc<AtomicUsize>,
}

#[cfg(feature = "native-tokio")]
impl ContextProvider for FixtureProvider {
    fn descriptor(&self) -> ContextProviderDescriptor {
        self.descriptor.clone()
    }

    fn collect(
        &self,
        _ctx: ContextCallContext,
        _request: ContextRequest,
    ) -> PortFuture<Result<ContextContribution, ContextError>> {
        self.calls.fetch_add(1, Ordering::AcqRel);
        let contribution = ContextContribution::try_new(
            vec![
                ContextItem::try_new(
                    ContextItemKind::Instruction,
                    vec![ContentBlock::Text(
                        TextBlock::try_new("retrieved system override").expect("text"),
                    )],
                    ContextProvenance {
                        source_id: Arc::from("fixture-source"),
                        source_ref: None,
                        external: true,
                    },
                    ContextAuthority::TrustedApplication,
                    0,
                    4,
                    Sensitivity::Internal,
                    false,
                )
                .expect("item"),
            ],
            None::<&str>,
        )
        .expect("contribution");
        Box::pin(async move { Ok(contribution) })
    }
}

#[cfg(feature = "native-tokio")]
fn descriptor(recovery: InvocationRecovery) -> ContextProviderDescriptor {
    ContextProviderDescriptor {
        invocation: ComponentInvocation {
            component: ComponentId::parse("fixture.context").expect("component"),
            version: Version {
                major: 1,
                minor: 0,
                patch: 0,
            },
            configuration_digest: Digest::raw_json(b"{}"),
            recovery,
        },
        trusted_application_instructions: false,
        metadata: Metadata::empty(),
    }
}

#[cfg(feature = "native-tokio")]
fn request() -> ContextRequest {
    ContextRequest {
        session_id: id::<SessionTag>(1),
        lane_id: id::<LaneTag>(2),
        run_id: id::<RunTag>(3),
        user_input: Arc::from([]),
        recent_history: Arc::from([]),
        budget: ContextBudget {
            max_items: 8,
            max_tokens: 128,
            max_bytes: 16_384,
            overflow: ContextOverflowPolicy::Reject,
        },
        active_capabilities: Arc::from([]),
    }
}

#[cfg(feature = "native-tokio")]
fn run(effect_id: finstack_ai_kernel::EffectId) -> RunCallContext {
    RunCallContext {
        locator: finstack_ai_kernel::OperationLocator::try_new(
            "tenant-a",
            id::<SessionTag>(1),
            id::<LaneTag>(2),
            id::<RunTag>(3),
        )
        .expect("locator"),
        authorization: AuthorizationContext {
            principal: PrincipalRef::try_new("issuer", "subject", Some("tenant-a"))
                .expect("principal"),
            authentication_method: Arc::from("fixture"),
            assurance_level: Arc::from("high"),
            roles: Arc::from([]),
            permitted_scopes: Arc::from([Arc::from("tenant-a")]),
            safe_claims: Metadata::empty(),
            policy_version: Arc::from("v1"),
            decision_id: Arc::from("decision-1"),
        },
        effect_id,
        attempt: 1,
        deadline: None,
        budget_scope_id: None,
        cancellation: CancellationSignal::new(),
    }
}

#[cfg(feature = "native-tokio")]
fn envelope(sequence: u64, body: RecordBody) -> RecordEnvelope {
    let events = (0..body
        .derived_event_count(RECORD_KIND_VERSION)
        .expect("events"))
        .map(|offset| id::<EventTag>(100 + sequence + u64::try_from(offset).expect("offset")))
        .collect();
    RecordEnvelope::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        id::<RecordTag>(10 + sequence),
        id::<SessionTag>(1),
        id::<LaneTag>(2),
        Some(id::<RunTag>(3)),
        sequence,
        Timestamp::from_unix_ms(1_000).expect("timestamp"),
        None,
        Digest::raw_json(b"{}"),
        None,
        Digest::raw_json(b"{}"),
        events,
        body,
    )
    .expect("envelope")
}

#[cfg(feature = "native-tokio")]
fn effect_request(
    effect_id: finstack_ai_kernel::EffectId,
    descriptor: &ContextProviderDescriptor,
    request: &ContextRequest,
) -> EffectRequested {
    EffectRequested::try_new(
        effect_id,
        EffectKind::Context,
        None,
        Some(descriptor.invocation.clone()),
        Some(
            PipelinePosition::try_new(Digest::raw_json(b"chain"), CONTEXT_STAGE, 0)
                .expect("pipeline"),
        ),
        EffectOutputContract {
            kind: EffectOutputKind::ContextContribution,
            schema_version: 1,
            schema_digest: Digest::raw_json(b"context-contribution-v1"),
        },
        EffectInput::Context {
            request: request.to_raw_json().expect("request"),
        },
        RetrySafety::SafeToRetry,
        None,
    )
    .expect("effect")
}

#[cfg(feature = "native-tokio")]
#[tokio::test]
async fn committed_guard_precedes_provider_io_and_untrusted_instruction_is_downgraded() {
    let descriptor = descriptor(InvocationRecovery::RecomputeSafe);
    let request = request();
    let effect_id = id::<finstack_ai_kernel::EffectTag>(4);
    let requested = effect_request(effect_id, &descriptor, &request);
    let committed = envelope(1, RecordBody::EffectRequested(requested.clone()));
    let calls = Arc::new(AtomicUsize::new(0));
    let provider = FixtureProvider {
        descriptor: descriptor.clone(),
        calls: Arc::clone(&calls),
    };
    let context = ContextCallContext {
        run: run(effect_id),
        provider_index: 0,
        chain_digest: Digest::raw_json(b"chain"),
    };
    let wrong = ContextCallContext {
        run: run(id::<finstack_ai_kernel::EffectTag>(99)),
        ..context.clone()
    };
    assert_eq!(
        CommittedContextCall::try_new(&committed, wrong, request.clone(), &descriptor)
            .expect_err("mismatch")
            .code(),
        CONTEXT_COMMIT_REQUIRED
    );
    assert_eq!(calls.load(Ordering::Acquire), 0);

    let contribution =
        CommittedContextCall::try_new(&committed, context, request.clone(), &descriptor)
            .expect("guard")
            .invoke(&provider)
            .await
            .expect("invoke");
    assert_eq!(calls.load(Ordering::Acquire), 1);
    assert_eq!(contribution.items[0].kind, ContextItemKind::QuotedSource);
    assert_eq!(contribution.items[0].authority, ContextAuthority::Untrusted);

    let completed = EffectCompleted::try_new(
        effect_id,
        requested.output_contract().clone(),
        contribution.to_raw_json().expect("output"),
        None,
        Vec::new(),
        finstack_ai_kernel::ProviderIds::empty(),
        None::<&str>,
        None,
    )
    .expect("completion");
    let completed_envelope = envelope(2, RecordBody::EffectCompleted(completed.clone()));
    let recorded = RecordedContextContribution::try_from_records(&committed, &completed_envelope)
        .expect("recorded");
    assert_eq!(recorded.provider_index, 0);
    assert_eq!(
        context_resume_action(&requested, Some(&completed)),
        InvocationResumeAction::UseRecorded
    );
    assert_eq!(
        context_resume_action(&requested, None),
        InvocationResumeAction::Recompute
    );
}
