use std::fmt::Debug;
use std::sync::Arc;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, BudgetPropagation, CancellationPropagation, CommittedBatch,
    ComponentId, ContentBlock, ContextPrepared, DeadlinePropagation, Decision, Digest,
    EffectCompleted, EffectDeferred, EffectKind, EffectOutputContract, EffectOutputKind,
    EntryAppended, ErrorCategory, ErrorDescriptor, ExternalEffectCompletedInput,
    ExternalEffectCompletion, ExternalEffectOutcome, ExternalHandleRef, Id, IdTag, Kernel,
    KernelError, KernelInput, Message, MessageRole, Metadata, ModelSettled, ModelSettlement,
    ModelSettlementKind, ModelTextDelta, PostCommitAction, PrincipalPropagation, PrincipalRef,
    ProviderIds, RECORD_FORMAT_VERSION, RECORD_KIND_VERSION, RawJson, ReconciliationPolicy,
    RecordBody, RecordDraft, RecordEnvelope, ReducerStageOutcome, RetrySafety, RunAccepted,
    RunCompleted, RunEvent, RunEventKind, RunFailed, RunLimits, RunPhase, RunPropagationPolicy,
    RunRelation, RunSecurityContext, Sensitivity, SessionId, Stage, StageCursor, StageDisposition,
    StageOutcomeRecorded, TerminalCandidate, TerminalState, TextBlock, Timestamp, TransitionEnv,
};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const SESSION: u64 = 1;
const LANE: u64 = 2;
const RUN: u64 = 3;
const USER_MESSAGE: u64 = 4;
const TURN_ONE: u64 = 101;
const MODEL_REQUEST_ONE: u64 = 102;
const EFFECT_ONE: u64 = 103;
const FINAL_MESSAGE_ONE: u64 = 104;
const TURN_TWO: u64 = 201;
const MODEL_REQUEST_TWO: u64 = 202;
const EFFECT_TWO: u64 = 203;
const FINAL_MESSAGE_TWO: u64 = 204;

#[derive(Default)]
struct Harness {
    kernel: Kernel,
    batches: Vec<CommittedBatch>,
    events: Vec<RunEvent>,
}

impl Harness {
    fn apply_input(&mut self, env: TransitionEnv, input: KernelInput) -> Decision {
        let before = self.kernel.state().clone();
        let before_hash = before.state_hash().expect("pre-decision state hash");
        let decision = self.kernel.decide(&env, input).expect("valid decision");
        drop(env.ids);
        assert_eq!(self.kernel.state(), &before, "decide mutated state");
        assert_eq!(
            self.kernel.state().state_hash().expect("state hash"),
            before_hash,
            "decide altered state hash"
        );

        let expected_sequence =
            i64::try_from(decision.expected_sequence).expect("fixture sequence fits i64");
        let batch = commit_decision(&decision, 20_000 + expected_sequence);
        let first_transient_sequence =
            u64::try_from(self.events.len()).expect("event sequence fits u64");
        let events = self
            .kernel
            .apply(&batch, first_transient_sequence)
            .expect("valid committed batch");
        self.events.extend(events.iter().cloned());
        self.batches.push(batch);
        decision
    }
}

fn id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[..6].copy_from_slice(&[0x01, 0x23, 0x45, 0x67, 0x89, 0xab]);
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

fn timestamp(unix_ms: i64) -> Timestamp {
    Timestamp::from_unix_ms(unix_ms).expect("fixed timestamp")
}

fn transition_env(
    now_ms: i64,
    record_ids: &[u64],
    event_ids: &[u64],
    effect_ids: &[u64],
    turn_ids: &[u64],
    model_request_ids: &[u64],
    message_ids: &[u64],
) -> TransitionEnv {
    TransitionEnv {
        now: timestamp(now_ms),
        ids: AllocatedIds::try_new(
            record_ids
                .iter()
                .copied()
                .map(id::<finstack_ai_kernel::RecordTag>)
                .collect(),
            event_ids
                .iter()
                .copied()
                .map(id::<finstack_ai_kernel::EventTag>)
                .collect(),
            effect_ids
                .iter()
                .copied()
                .map(id::<finstack_ai_kernel::EffectTag>)
                .collect(),
            vec![],
            message_ids
                .iter()
                .copied()
                .map(id::<finstack_ai_kernel::MessageTag>)
                .collect(),
            turn_ids
                .iter()
                .copied()
                .map(id::<finstack_ai_kernel::TurnTag>)
                .collect(),
            model_request_ids
                .iter()
                .copied()
                .map(id::<finstack_ai_kernel::ModelRequestTag>)
                .collect(),
            vec![],
            vec![],
            vec![],
            vec![],
        )
        .expect("bounded fixed allocated ids"),
    }
}

fn empty_env(now_ms: i64) -> TransitionEnv {
    transition_env(now_ms, &[], &[], &[], &[], &[], &[])
}

fn root_acceptance() -> RunAccepted {
    let run_id = id::<finstack_ai_kernel::RunTag>(RUN);
    RunAccepted::try_new(
        run_id,
        RunRelation::root(run_id).expect("root relation"),
        RunSecurityContext::try_new(
            "tenant-a",
            PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a")).expect("principal"),
            "oidc",
            "high",
            "policy-v1",
            "decision-v1",
            None,
        )
        .expect("security context"),
        None,
        RunLimits::empty(),
        RunPropagationPolicy {
            cancellation: CancellationPropagation::Cascade,
            deadline: DeadlinePropagation::MinimumOfParentAndChild,
            budget: BudgetPropagation::SharedScope,
            principal: PrincipalPropagation::Inherit,
        },
        Digest::raw_json(br#"{"agent":"fixture"}"#),
        None,
    )
    .expect("root acceptance")
}

fn accept_input() -> KernelInput {
    KernelInput::AcceptRun(AcceptRun {
        session_id: id::<finstack_ai_kernel::SessionTag>(SESSION),
        lane_id: id::<finstack_ai_kernel::LaneTag>(LANE),
        accepted: root_acceptance(),
    })
}

fn provider_ids() -> ProviderIds {
    ProviderIds::try_new(
        Some("provider-request-1"),
        Some("provider-response-1"),
        None::<&str>,
    )
    .expect("provider ids")
}

fn context_messages() -> Vec<Message> {
    vec![
        Message::try_new(
            id::<finstack_ai_kernel::MessageTag>(USER_MESSAGE),
            MessageRole::User,
            vec![ContentBlock::Text(
                TextBlock::try_new("Say hello.").expect("user text"),
            )],
            timestamp(900),
            None,
            ProviderIds::empty(),
            Metadata::empty(),
        )
        .expect("user message"),
    ]
}

fn assistant_message(message_ordinal: u64, now_ms: i64, text: &str) -> Message {
    Message::try_new(
        id::<finstack_ai_kernel::MessageTag>(message_ordinal),
        MessageRole::Assistant,
        vec![ContentBlock::Text(
            TextBlock::try_new(text).expect("assistant text"),
        )],
        timestamp(now_ms),
        None,
        provider_ids(),
        Metadata::empty(),
    )
    .expect("assistant message")
}

fn output_contract() -> EffectOutputContract {
    EffectOutputContract {
        kind: EffectOutputKind::ModelResponse,
        schema_version: 1,
        schema_digest: Digest::raw_json(br#"{"type":"model_response"}"#),
    }
}

fn completed_effect(effect_ordinal: u64, completion_id: &str, text: &str) -> EffectCompleted {
    EffectCompleted::try_new(
        id::<finstack_ai_kernel::EffectTag>(effect_ordinal),
        output_contract(),
        RawJson::parse(json!({ "text": text }).to_string()).expect("model output"),
        None,
        vec![],
        provider_ids(),
        Some(completion_id),
        None,
    )
    .expect("completed model effect")
}

fn completed_input(
    turn_ordinal: u64,
    request_ordinal: u64,
    effect_ordinal: u64,
    message_ordinal: u64,
    now_ms: i64,
    completion_id: &str,
    text: &str,
) -> KernelInput {
    KernelInput::ModelSettled(ModelSettled {
        turn_id: id::<finstack_ai_kernel::TurnTag>(turn_ordinal),
        model_request_id: id::<finstack_ai_kernel::ModelRequestTag>(request_ordinal),
        outcome: ModelSettlement::Completed {
            completion: completed_effect(effect_ordinal, completion_id, text),
            assistant_message: assistant_message(message_ordinal, now_ms, text),
        },
    })
}

fn failed_input(
    turn_ordinal: u64,
    request_ordinal: u64,
    effect_ordinal: u64,
    completion_id: &str,
) -> KernelInput {
    KernelInput::ModelSettled(ModelSettled {
        turn_id: id::<finstack_ai_kernel::TurnTag>(turn_ordinal),
        model_request_id: id::<finstack_ai_kernel::ModelRequestTag>(request_ordinal),
        outcome: ModelSettlement::Failed(
            finstack_ai_kernel::EffectFailed::try_new(
                id::<finstack_ai_kernel::EffectTag>(effect_ordinal),
                output_contract(),
                fixture_error("provider_failed"),
                None,
                Some(completion_id),
            )
            .expect("failed model effect"),
        ),
    })
}

fn deferred_effect(effect_ordinal: u64, handle: &str) -> EffectDeferred {
    EffectDeferred {
        effect_id: id::<finstack_ai_kernel::EffectTag>(effect_ordinal),
        handle: ExternalHandleRef::try_new(
            ComponentId::parse("finstack.provider.fixture").expect("provider component"),
            handle,
            RawJson::parse(r#"{"region":"test"}"#).expect("reconciliation metadata"),
        )
        .expect("external handle"),
        reconciliation: ReconciliationPolicy::CallbackOrPoll,
        next_poll_at: Some(timestamp(5_000)),
        expires_at: Some(timestamp(10_000)),
        output_contract: output_contract(),
    }
}

fn deferred_input(
    turn_ordinal: u64,
    request_ordinal: u64,
    effect_ordinal: u64,
    handle: &str,
) -> KernelInput {
    KernelInput::ModelSettled(ModelSettled {
        turn_id: id::<finstack_ai_kernel::TurnTag>(turn_ordinal),
        model_request_id: id::<finstack_ai_kernel::ModelRequestTag>(request_ordinal),
        outcome: ModelSettlement::Deferred(deferred_effect(effect_ordinal, handle)),
    })
}

fn external_completed_input(
    effect_ordinal: u64,
    message_ordinal: u64,
    now_ms: i64,
    completion_id: &str,
    text: &str,
) -> KernelInput {
    KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
        completion: ExternalEffectCompletion {
            effect_id: id::<finstack_ai_kernel::EffectTag>(effect_ordinal),
            completion_id: Arc::from(completion_id),
            outcome: ExternalEffectOutcome::Completed {
                output: RawJson::parse(json!({ "text": text }).to_string())
                    .expect("external model output"),
                usage: None,
                artifacts: Arc::from([]),
            },
        },
        assistant_message: Some(assistant_message(message_ordinal, now_ms, text)),
    })
}

fn external_failed_input(effect_ordinal: u64, completion_id: &str, code: &str) -> KernelInput {
    KernelInput::ExternalEffectCompleted(ExternalEffectCompletedInput {
        completion: ExternalEffectCompletion {
            effect_id: id::<finstack_ai_kernel::EffectTag>(effect_ordinal),
            completion_id: Arc::from(completion_id),
            outcome: ExternalEffectOutcome::Failed {
                error: fixture_error(code),
            },
        },
        assistant_message: None,
    })
}

fn fixture_error(code: &str) -> ErrorDescriptor {
    ErrorDescriptor::new(code, "fixture failure", ErrorCategory::Model, false)
        .expect("error descriptor")
}

fn stage_input(cycle: u64, stage: Stage, outcome: ReducerStageOutcome) -> KernelInput {
    KernelInput::StageSettled(finstack_ai_kernel::StageSettled {
        cursor: StageCursor { cycle, stage },
        outcome,
    })
}

fn accept(harness: &mut Harness) -> Decision {
    harness.apply_input(
        transition_env(1_000, &[1], &[1], &[], &[], &[], &[]),
        accept_input(),
    )
}

fn settle_before_run(harness: &mut Harness) -> Decision {
    harness.apply_input(
        transition_env(1_100, &[2], &[], &[], &[], &[], &[]),
        stage_input(0, Stage::BeforeRun, ReducerStageOutcome::Continue),
    )
}

fn prepare_context(harness: &mut Harness, cycle: u64, second_cycle: bool) -> Decision {
    let (records, turn) = if second_cycle {
        (&[13, 14][..], TURN_TWO)
    } else {
        (&[3, 4][..], TURN_ONE)
    };
    harness.apply_input(
        transition_env(
            if second_cycle { 2_100 } else { 1_200 },
            records,
            &[],
            &[],
            &[turn],
            &[],
            &[],
        ),
        stage_input(
            cycle,
            Stage::PrepareContext,
            ReducerStageOutcome::ContextPrepared {
                messages: Arc::from(context_messages()),
            },
        ),
    )
}

fn request_model(harness: &mut Harness, cycle: u64, second_cycle: bool) -> Decision {
    let (records, event, effect, request) = if second_cycle {
        (&[15, 16][..], 6, EFFECT_TWO, MODEL_REQUEST_TWO)
    } else {
        (&[5, 6][..], 2, EFFECT_ONE, MODEL_REQUEST_ONE)
    };
    harness.apply_input(
        transition_env(
            if second_cycle { 2_200 } else { 1_300 },
            records,
            &[event],
            &[effect],
            &[],
            &[request],
            &[],
        ),
        stage_input(
            cycle,
            Stage::BeforeModel,
            ReducerStageOutcome::ModelRequestPrepared {
                request: RawJson::parse(r#"{"messages":[{"role":"user","text":"Say hello."}]}"#)
                    .expect("model request"),
                component: None,
                output_contract: output_contract(),
                retry_safety: RetrySafety::SafeToRetry,
                deadline: Some(timestamp(8_000)),
            },
        ),
    )
}

fn complete_model(harness: &mut Harness, second_cycle: bool, completion_id: &str) -> Decision {
    let (records, events, turn, request, effect, message, now_ms) = if second_cycle {
        (
            &[17, 18][..],
            &[7, 8][..],
            TURN_TWO,
            MODEL_REQUEST_TWO,
            EFFECT_TWO,
            FINAL_MESSAGE_TWO,
            2_300,
        )
    } else {
        (
            &[7, 8][..],
            &[3, 4][..],
            TURN_ONE,
            MODEL_REQUEST_ONE,
            EFFECT_ONE,
            FINAL_MESSAGE_ONE,
            1_400,
        )
    };
    harness.apply_input(
        transition_env(now_ms, records, events, &[], &[], &[], &[message]),
        completed_input(
            turn,
            request,
            effect,
            message,
            now_ms,
            completion_id,
            "hello",
        ),
    )
}

fn settle_after_model(harness: &mut Harness, cycle: u64, second_cycle: bool) -> Decision {
    harness.apply_input(
        transition_env(
            if second_cycle { 2_400 } else { 1_500 },
            if second_cycle { &[19] } else { &[9] },
            &[],
            &[],
            &[],
            &[],
            &[],
        ),
        stage_input(cycle, Stage::AfterModel, ReducerStageOutcome::Continue),
    )
}

fn finalize(harness: &mut Harness, cycle: u64, second_cycle: bool) -> Decision {
    harness.apply_input(
        transition_env(
            if second_cycle { 2_500 } else { 1_600 },
            if second_cycle { &[20, 21] } else { &[10, 11] },
            if second_cycle { &[9] } else { &[5] },
            &[],
            &[],
            &[],
            &[],
        ),
        stage_input(
            cycle,
            Stage::BeforeFinalize,
            ReducerStageOutcome::FinalizeAccepted,
        ),
    )
}

fn drive_to_awaiting_model() -> Harness {
    let mut harness = Harness::default();
    accept(&mut harness);
    settle_before_run(&mut harness);
    prepare_context(&mut harness, 0, false);
    request_model(&mut harness, 0, false);
    harness
}

fn drive_to_after_model() -> Harness {
    let mut harness = drive_to_awaiting_model();
    complete_model(&mut harness, false, "completion-1");
    harness
}

fn drive_to_before_finalize() -> Harness {
    let mut harness = drive_to_after_model();
    settle_after_model(&mut harness, 0, false);
    harness
}

fn drive_to_completed() -> Harness {
    let mut harness = drive_to_before_finalize();
    finalize(&mut harness, 0, false);
    harness
}

fn commit_decision(decision: &Decision, committed_at_ms: i64) -> CommittedBatch {
    commit_records(
        decision.expected_sequence,
        &decision.records,
        None,
        IdentityOverride::default(),
        committed_at_ms,
    )
}

#[derive(Clone, Copy, Default)]
struct IdentityOverride {
    session: Option<SessionId>,
    lane: Option<finstack_ai_kernel::LaneId>,
    run: Option<finstack_ai_kernel::RunId>,
}

fn commit_records(
    first_sequence: u64,
    drafts: &[RecordDraft],
    sequences: Option<&[u64]>,
    identity: IdentityOverride,
    committed_at_ms: i64,
) -> CommittedBatch {
    assert!(!drafts.is_empty(), "committed batches are non-empty");
    let records = drafts
        .iter()
        .enumerate()
        .map(|(index, draft)| {
            let offset = u64::try_from(index).expect("fixture index fits u64");
            let sequence = sequences.map_or(first_sequence + offset, |values| values[index]);
            RecordEnvelope::try_new(
                draft.format_version(),
                draft.kind_version(),
                draft.record_id(),
                identity.session.unwrap_or_else(|| draft.session_id()),
                identity.lane.unwrap_or_else(|| draft.lane_id()),
                identity.run.or_else(|| draft.run_id()),
                sequence,
                draft.timestamp(),
                Some(timestamp(committed_at_ms)),
                Digest::raw_json(format!("payload-{sequence}").as_bytes()),
                None,
                Digest::raw_json(format!("checksum-{sequence}").as_bytes()),
                draft.derived_event_ids().to_vec(),
                draft.body().clone(),
            )
            .expect("committed envelope")
        })
        .collect::<Vec<_>>();
    CommittedBatch {
        batch_id: id::<finstack_ai_kernel::AppendBatchTag>(10_000 + first_sequence),
        first_sequence,
        last_sequence: first_sequence + records.len() as u64 - 1,
        records: records.into(),
    }
}

fn replay(batches: &[CommittedBatch]) -> Kernel {
    let mut kernel = Kernel::default();
    let mut next_transient_sequence = 0_u64;
    for batch in batches {
        let events = kernel
            .apply(batch, next_transient_sequence)
            .expect("replay committed batch");
        next_transient_sequence = next_transient_sequence
            .checked_add(u64::try_from(events.len()).expect("event count fits u64"))
            .expect("event sequence overflow");
    }
    kernel
}

fn decision_body_names(decision: &Decision) -> Vec<&'static str> {
    decision
        .records
        .iter()
        .map(|record| record.body().kind_name())
        .collect()
}

type ExpectedEvent = (
    RunEventKind,
    u64,
    u64,
    Option<u64>,
    Option<u64>,
    Option<u64>,
    Sensitivity,
);

fn assert_exact_event_trace(harness: &Harness, expected: &[ExpectedEvent]) {
    assert_eq!(harness.events.len(), expected.len(), "event count");
    for (
        transient_sequence,
        (event, (kind, event_id, sequence, turn, request, effect, sensitivity)),
    ) in harness.events.iter().zip(expected).enumerate()
    {
        assert_eq!(event.kind(), *kind);
        assert_eq!(
            event.event_id(),
            id::<finstack_ai_kernel::EventTag>(*event_id)
        );
        assert_eq!(event.durable_sequence(), Some(*sequence));
        assert_eq!(
            event.transient_sequence(),
            u64::try_from(transient_sequence).expect("event sequence fits u64")
        );
        assert_eq!(event.turn_id(), turn.map(id::<finstack_ai_kernel::TurnTag>));
        assert_eq!(
            event.model_request_id(),
            request.map(id::<finstack_ai_kernel::ModelRequestTag>)
        );
        assert_eq!(
            event.effect_id(),
            effect.map(id::<finstack_ai_kernel::EffectTag>)
        );
        assert_eq!(event.tool_batch_id(), None);
        assert_eq!(event.tool_call_id(), None);
        assert_eq!(event.sensitivity(), *sensitivity);
    }
    assert!(
        harness
            .batches
            .iter()
            .flat_map(|batch| batch.records.iter())
            .filter(|record| matches!(
                record.body(),
                RecordBody::StageOutcomeRecorded(_) | RecordBody::ContextPrepared(_)
            ))
            .all(|record| record.derived_event_ids().is_empty()),
        "bookkeeping records must derive zero events"
    );
}

fn direct_success_events() -> Vec<ExpectedEvent> {
    vec![
        (
            RunEventKind::RunAccepted,
            1,
            1,
            None,
            None,
            None,
            Sensitivity::Internal,
        ),
        model_event(
            RunEventKind::EffectRequested,
            2,
            6,
            Sensitivity::Confidential,
        ),
        model_event(
            RunEventKind::EffectCompleted,
            3,
            7,
            Sensitivity::Confidential,
        ),
        model_event(RunEventKind::MessageFinalized, 4, 8, Sensitivity::Internal),
        model_event(RunEventKind::RunCompleted, 5, 11, Sensitivity::Internal),
    ]
}

fn external_success_events() -> Vec<ExpectedEvent> {
    vec![
        (
            RunEventKind::RunAccepted,
            1,
            1,
            None,
            None,
            None,
            Sensitivity::Internal,
        ),
        model_event(
            RunEventKind::EffectRequested,
            2,
            6,
            Sensitivity::Confidential,
        ),
        model_event(
            RunEventKind::EffectDeferred,
            3,
            7,
            Sensitivity::Confidential,
        ),
        model_event(
            RunEventKind::EffectCompleted,
            4,
            8,
            Sensitivity::Confidential,
        ),
        model_event(RunEventKind::MessageFinalized, 5, 9, Sensitivity::Internal),
    ]
}

fn direct_failure_events() -> Vec<ExpectedEvent> {
    vec![
        (
            RunEventKind::RunAccepted,
            1,
            1,
            None,
            None,
            None,
            Sensitivity::Internal,
        ),
        model_event(
            RunEventKind::EffectRequested,
            2,
            6,
            Sensitivity::Confidential,
        ),
        model_event(RunEventKind::EffectFailed, 3, 7, Sensitivity::Confidential),
        model_event(RunEventKind::RunFailed, 4, 9, Sensitivity::Internal),
    ]
}

fn external_failure_events() -> Vec<ExpectedEvent> {
    vec![
        (
            RunEventKind::RunAccepted,
            1,
            1,
            None,
            None,
            None,
            Sensitivity::Internal,
        ),
        model_event(
            RunEventKind::EffectRequested,
            2,
            6,
            Sensitivity::Confidential,
        ),
        model_event(
            RunEventKind::EffectDeferred,
            3,
            7,
            Sensitivity::Confidential,
        ),
        model_event(RunEventKind::EffectFailed, 4, 8, Sensitivity::Confidential),
    ]
}

const fn model_event(
    kind: RunEventKind,
    event_id: u64,
    sequence: u64,
    sensitivity: Sensitivity,
) -> ExpectedEvent {
    (
        kind,
        event_id,
        sequence,
        Some(TURN_ONE),
        Some(MODEL_REQUEST_ONE),
        Some(EFFECT_ONE),
        sensitivity,
    )
}

fn assert_error_code<T: Debug>(result: Result<T, KernelError>, expected: &str) {
    let error = result.expect_err(expected);
    assert_eq!(error.code(), expected);
}

fn assert_apply_rejected_without_mutation(
    kernel: &mut Kernel,
    batch: &CommittedBatch,
    expected: &str,
) {
    let before = kernel.state().clone();
    let before_hash = before.state_hash().expect("state hash");
    assert_error_code(kernel.apply(batch, 0), expected);
    assert_eq!(kernel.state(), &before);
    assert_eq!(
        kernel.state().state_hash().expect("state hash"),
        before_hash
    );
}

fn assert_json_round_trip_and_unknown_fields<T>(value: &T)
where
    T: Serialize + DeserializeOwned + PartialEq + Debug,
{
    let encoded = serde_json::to_value(value).expect("serialize public value");
    let decoded: T = serde_json::from_value(encoded.clone()).expect("strict JSON round trip");
    assert_eq!(&decoded, value);

    let mut with_unknown = encoded;
    insert_unknown_field(&mut with_unknown);
    assert!(
        serde_json::from_value::<T>(with_unknown).is_err(),
        "{} accepted an unknown field",
        std::any::type_name::<T>()
    );
}

fn assert_json_round_trip<T>(value: &T)
where
    T: Serialize + DeserializeOwned + PartialEq + Debug,
{
    let encoded = serde_json::to_value(value).expect("serialize public value");
    let decoded: T = serde_json::from_value(encoded).expect("strict JSON round trip");
    assert_eq!(&decoded, value);
}

fn insert_unknown_field(value: &mut Value) {
    let object = value.as_object_mut().expect("public wire object");
    if object.len() == 1 {
        let inner = object.values_mut().next().expect("enum value");
        if let Some(inner_object) = inner.as_object_mut() {
            inner_object.insert("unknown_contract_field".to_owned(), Value::Bool(true));
            return;
        }
    }
    object.insert("unknown_contract_field".to_owned(), Value::Bool(true));
}

fn find_body<T>(batches: &[CommittedBatch], extract: impl Fn(&RecordBody) -> Option<T>) -> T {
    batches
        .iter()
        .flat_map(|batch| batch.records.iter())
        .find_map(|record| extract(record.body()))
        .expect("record body in trace")
}

fn canonical_digest(domain: &str, value: &Value) -> Digest {
    let canonical = serde_json_canonicalizer::to_string(value).expect("canonical JSON");
    Digest::domain_separated(domain, 1, canonical.as_bytes()).expect("fixed digest domain")
}

fn input_payload(input: &KernelInput, variant: &str) -> Value {
    serde_json::to_value(input)
        .expect("serialize kernel input")
        .as_object()
        .and_then(|object| object.get(variant))
        .cloned()
        .expect("expected input variant")
}

#[path = "apply_and_json.rs"]
mod apply_and_json;
#[path = "matrix.rs"]
mod matrix;
#[path = "settlements.rs"]
mod settlements;
#[path = "successful.rs"]
mod successful;
#[path = "tool_batches.rs"]
mod tool_batches;
