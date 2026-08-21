use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::{
    AcceptRun, AllocatedIds, AuthorizationEvidence, BudgetPropagation, CancellationPropagation,
    Digest, ErrorCategory, ErrorDescriptor, ExternalEffectCompletionCommand, ExternalEffectOutcome,
    Id, IdTag, InteractionResolutionCommand, Kernel, KernelInput, Metadata, OperationLocator,
    PrincipalPropagation, PrincipalRef, RawJson, RecordEnvelope, RunAccepted, RunLimits,
    RunPropagationPolicy, RunRelation, RunSecurityContext, Timestamp, TransitionEnv,
};

use super::*;
use crate::{
    JournalStore, LoadRequest, LoadedSession, PortFuture, SecurityAuditCategory,
    SecurityAuditError, SecurityAuditEvent, SecurityAuditGate, SecurityAuditHealth,
    SecurityAuditReceipt, SecurityAuditSink, SnapshotReceipt, SnapshotRequest, StoreError,
    StoreHealth,
};

fn id<T: IdTag>(ordinal: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8..].copy_from_slice(&ordinal.to_be_bytes());
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Id::from_bytes(bytes)
}

fn timestamp(ms: i64) -> Timestamp {
    Timestamp::from_unix_ms(ms).expect("timestamp")
}

fn principal(subject: &str) -> PrincipalRef {
    PrincipalRef::try_new("issuer", subject, Some("tenant-a")).expect("principal")
}

fn authorization() -> AuthorizationEvidence {
    AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("authorization")
}

fn acceptance() -> RunAccepted {
    let run_id = id(3);
    RunAccepted::try_new(
        run_id,
        RunRelation::root(run_id).expect("relation"),
        RunSecurityContext::try_new(
            "tenant-a",
            principal("subject"),
            "oidc",
            "high",
            "policy-v1",
            "decision-v1",
            None,
        )
        .expect("security"),
        None,
        RunLimits::empty(),
        RunPropagationPolicy {
            cancellation: CancellationPropagation::Cascade,
            deadline: finstack_ai_kernel::DeadlinePropagation::MinimumOfParentAndChild,
            budget: BudgetPropagation::SharedScope,
            principal: PrincipalPropagation::Inherit,
        },
        Digest::raw_json(b"agent"),
        None,
    )
    .expect("accepted")
}

fn loaded_session() -> LoadedSession {
    let env = TransitionEnv {
        now: timestamp(1_000),
        ids: AllocatedIds::try_new(
            vec![id(1)],
            vec![id(1)],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![id(1)],
            Vec::new(),
        )
        .expect("ids"),
    };
    let decision = Kernel::default()
        .decide(
            &env,
            KernelInput::AcceptRun(AcceptRun {
                session_id: id(1),
                lane_id: id(2),
                accepted: acceptance(),
            }),
        )
        .expect("decision");
    let records = decision
        .records
        .iter()
        .enumerate()
        .map(|(offset, draft)| {
            let sequence = decision.expected_sequence + u64::try_from(offset).expect("offset");
            RecordEnvelope::try_new(
                draft.format_version(),
                draft.kind_version(),
                draft.record_id(),
                draft.session_id(),
                draft.lane_id(),
                draft.run_id(),
                sequence,
                draft.timestamp(),
                None,
                Digest::raw_json(b"payload"),
                None,
                Digest::raw_json(b"checksum"),
                draft.derived_event_ids().to_vec(),
                draft.body().clone(),
            )
            .expect("envelope")
        })
        .collect::<Vec<_>>();
    let batch = finstack_ai_kernel::CommittedBatch::try_new(
        id(1),
        1,
        u64::try_from(records.len()).expect("count"),
        records,
    )
    .expect("batch");
    LoadedSession {
        session_id: id(1),
        head_sequence: batch.last_sequence,
        head_checksum: batch
            .records
            .last()
            .map(finstack_ai_kernel::RecordEnvelope::checksum),
        metadata: Metadata::empty(),
        committed_batches: Arc::from([batch]),
        snapshot: None,
        accelerated: None,
    }
}

struct StaticStore(LoadedSession);

impl JournalStore for StaticStore {
    fn append(
        &self,
        _request: finstack_ai_kernel::AppendRequest,
    ) -> PortFuture<Result<finstack_ai_kernel::CommittedBatch, StoreError>> {
        Box::pin(async {
            Err(StoreError::Unavailable {
                reason_code: "not_used",
            })
        })
    }

    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        let loaded = if request.session_id == self.0.session_id {
            self.0.clone()
        } else {
            LoadedSession::empty(request.session_id)
        };
        Box::pin(async move { Ok(loaded) })
    }

    fn write_snapshot(
        &self,
        _request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        Box::pin(async {
            Err(StoreError::Unavailable {
                reason_code: "not_used",
            })
        })
    }

    fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
        Box::pin(async {
            Ok(StoreHealth {
                ready: true,
                durable: false,
                detail: Arc::from("test"),
            })
        })
    }
}

struct AuditSink {
    fail: bool,
    events: Mutex<BTreeMap<Arc<str>, SecurityAuditEvent>>,
}

impl AuditSink {
    fn new(fail: bool) -> Self {
        Self {
            fail,
            events: Mutex::new(BTreeMap::new()),
        }
    }
}

impl SecurityAuditSink for AuditSink {
    fn record(
        &self,
        event: SecurityAuditEvent,
    ) -> PortFuture<Result<SecurityAuditReceipt, SecurityAuditError>> {
        if self.fail {
            return Box::pin(async {
                Err(SecurityAuditError::Unavailable {
                    reason_code: "write_failed",
                })
            });
        }
        let event_id = Arc::<str>::from(event.event_id());
        let recorded_at = event.timestamp();
        self.events
            .lock()
            .expect("lock")
            .entry(Arc::clone(&event_id))
            .or_insert(event);
        Box::pin(async move {
            Ok(SecurityAuditReceipt {
                event_id,
                recorded_at,
            })
        })
    }

    fn health(&self) -> PortFuture<Result<SecurityAuditHealth, SecurityAuditError>> {
        Box::pin(async { Ok(SecurityAuditHealth { ready: true }) })
    }
}

fn locator() -> OperationLocator {
    OperationLocator::try_new("tenant-a", id(1), id(2), id(3)).expect("locator")
}

fn completion(subject: &str) -> ExternalEffectCompletionCommand {
    ExternalEffectCompletionCommand::try_new(
        locator(),
        principal(subject),
        authorization(),
        finstack_ai_kernel::ExternalEffectCompletion::try_new(
            id(999),
            "completion-1",
            ExternalEffectOutcome::Failed {
                error: ErrorDescriptor::new(
                    "provider_failed",
                    "provider failed",
                    ErrorCategory::Model,
                    false,
                )
                .expect("error"),
            },
        )
        .expect("completion"),
    )
    .expect("command")
}

fn interaction() -> InteractionResolutionCommand {
    InteractionResolutionCommand::try_new(
        locator(),
        finstack_ai_kernel::InteractionResolution::try_new(
            id(777),
            "resolution-1",
            principal("subject"),
            authorization(),
            RawJson::parse(r#"{"approved":true}"#).expect("response"),
            None::<&str>,
        )
        .expect("resolution"),
    )
    .expect("command")
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("runtime")
}

#[test]
fn fresh_router_loads_only_locator_session_and_audits_unknown_target_idempotently() {
    runtime().block_on(async {
        let store: Arc<dyn JournalStore> = Arc::new(StaticStore(loaded_session()));
        let sink = Arc::new(AuditSink::new(false));
        let gate = SecurityAuditGate::enable(Some(sink.clone()), Duration::from_millis(100))
            .await
            .expect("gate");
        let router = ExternalCompletionRouter::new(store, gate);
        let submitted_at = timestamp(2_000);
        for _ in 0..2 {
            assert_eq!(
                Box::pin(router.route(completion("subject"), submitted_at))
                    .await
                    .expect_err("unknown target"),
                ExternalRouteError::IngressRejected
            );
        }
        let events = sink.events.lock().expect("lock");
        assert_eq!(events.len(), 1);
        let event = events.values().next().expect("event");
        assert_eq!(event.category(), SecurityAuditCategory::UnknownLocator);
        assert_eq!(event.reason_code(), "unknown_target");
        assert!(event.locator_digest().is_some());
        assert!(event.submission_digest().is_some());
    });
}

#[test]
fn principal_mismatch_and_unknown_interaction_are_audited_nonrevealing() {
    runtime().block_on(async {
        let store: Arc<dyn JournalStore> = Arc::new(StaticStore(loaded_session()));
        let sink = Arc::new(AuditSink::new(false));
        let gate = SecurityAuditGate::enable(Some(sink.clone()), Duration::from_millis(100))
            .await
            .expect("gate");
        let completion_router =
            ExternalCompletionRouter::new(Arc::clone(&store), Arc::clone(&gate));
        assert_eq!(
            Box::pin(completion_router.route(completion("different-subject"), timestamp(2_000)),)
                .await
                .expect_err("scope"),
            ExternalRouteError::IngressRejected
        );
        let interaction_router = InteractionRouter::new(store, gate);
        assert_eq!(
            interaction_router
                .route(interaction(), timestamp(2_001))
                .await
                .expect_err("unknown target"),
            ExternalRouteError::IngressRejected
        );
        let events = sink.events.lock().expect("lock");
        assert!(
            events
                .values()
                .any(|event| event.category() == SecurityAuditCategory::ScopeMismatch)
        );
        assert!(events.values().any(|event| {
            event.category() == SecurityAuditCategory::UnknownLocator
                && event.reason_code() == "unknown_target"
        }));
    });
}

#[test]
fn audit_write_failure_rejects_closed_with_same_response() {
    runtime().block_on(async {
        let store: Arc<dyn JournalStore> = Arc::new(StaticStore(loaded_session()));
        let gate = SecurityAuditGate::enable(
            Some(Arc::new(AuditSink::new(true))),
            Duration::from_millis(100),
        )
        .await
        .expect("gate");
        let router = ExternalCompletionRouter::new(store, gate);
        assert_eq!(
            Box::pin(router.route(completion("subject"), timestamp(2_000)))
                .await
                .expect_err("closed"),
            ExternalRouteError::IngressRejected
        );
    });
}
