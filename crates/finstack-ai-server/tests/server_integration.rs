//! Reference server integration coverage.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::{
    AcceptRun, BudgetPropagation, CancelRequested, CancellationInitiator, CancellationPropagation,
    DeadlinePropagation, Digest, EventId, LaneId, Metadata, PrincipalPropagation, PrincipalRef,
    QueueDepthWarning, RunAccepted, RunEvent, RunEventBody, RunId, RunLimits, RunPropagationPolicy,
    RunRelation, RunSecurityContext, Sensitivity, SessionId, UNIX_EPOCH,
};
use finstack_ai_protocol::{
    PROTOCOL_VERSION_V1, RemoteAgentRef, RemoteAuthMethod, RemoteCommand, RemoteCommandPayload,
    RemoteEventView, RemoteLocator, RemoteStartRequest, VersionOffer,
};
use finstack_ai_runtime::audit::{
    SecurityAuditCategory, SecurityAuditError, SecurityAuditEvent, SecurityAuditHealth,
    SecurityAuditReceipt, SecurityAuditSink,
};
use finstack_ai_runtime::ports::PortFuture;

use finstack_ai_server::{
    CreditLimits, ListenAddr, RemoteClient, Server, ServerError, SessionReplica,
    StaticAuthVerifier, TransportKind,
};

const SESSION_ID: &str = "01234567-89ab-7cde-89ab-0123456789ab";
const MISSING_SESSION_ID: &str = "01234567-89ab-7cde-89ab-0123456789ac";
const LANE_ID: &str = "11234567-89ab-7cde-89ab-0123456789ab";
const RUN_ID: &str = "21234567-89ab-7cde-89ab-0123456789ab";

fn locator(value: &str) -> RemoteLocator {
    RemoteLocator::try_new(
        value.parse().expect("session id"),
        Some(LANE_ID.parse().expect("lane id")),
        Some(RUN_ID.parse().expect("run id")),
    )
    .expect("locator")
}

fn start_payload() -> RemoteCommandPayload {
    let locator = locator(SESSION_ID);
    let run_id = locator.run_id().expect("run id");
    let spec_digest = Digest::raw_json(br#"{"agent":"fixture"}"#);
    let accepted = RunAccepted::try_new(
        run_id,
        RunRelation::root(run_id).expect("root"),
        RunSecurityContext::try_new(
            "tenant-a",
            PrincipalRef::try_new("issuer", "subject", Some("tenant-a")).expect("principal"),
            "loopback",
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
            deadline: DeadlinePropagation::MinimumOfParentAndChild,
            budget: BudgetPropagation::SharedScope,
            principal: PrincipalPropagation::Inherit,
        },
        spec_digest,
        None,
    )
    .expect("accepted");
    RemoteCommandPayload::Start(Box::new(
        RemoteStartRequest::try_new(
            AcceptRun {
                session_id: locator.session_id(),
                lane_id: locator.lane_id().expect("lane id"),
                accepted,
            },
            RemoteAgentRef {
                agent_id: finstack_ai_kernel::AgentId::parse("agent.fixture").expect("agent"),
                bundle_id: None,
                spec_digest,
            },
            Vec::new(),
            Metadata::empty(),
            None,
        )
        .expect("start"),
    ))
}

fn cancel_payload() -> RemoteCommandPayload {
    RemoteCommandPayload::Cancel(Box::new(CancelRequested {
        initiator: CancellationInitiator::RuntimeShutdown,
        reason: None,
    }))
}

#[derive(Clone)]
struct RecordingSink {
    events: Arc<Mutex<Vec<SecurityAuditEvent>>>,
    ready: bool,
}

impl RecordingSink {
    fn ready() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
            ready: true,
        }
    }

    fn unhealthy() -> Self {
        Self {
            events: Arc::new(Mutex::new(Vec::new())),
            ready: false,
        }
    }

    fn categories(&self) -> Vec<SecurityAuditCategory> {
        self.events
            .lock()
            .expect("events")
            .iter()
            .map(SecurityAuditEvent::category)
            .collect()
    }

    fn leak_canary(&self, canary: &str) -> bool {
        self.events.lock().expect("events").iter().any(|event| {
            event.event_id().contains(canary)
                || event.reason_code().contains(canary)
                || event
                    .tenant_scope()
                    .is_some_and(|scope| scope.contains(canary))
        })
    }
}

impl SecurityAuditSink for RecordingSink {
    fn record(
        &self,
        event: SecurityAuditEvent,
    ) -> PortFuture<Result<SecurityAuditReceipt, SecurityAuditError>> {
        let events = Arc::clone(&self.events);
        Box::pin(async move {
            let recorded_at = event.timestamp();
            let event_id = std::sync::Arc::<str>::from(event.event_id());
            events.lock().expect("events").push(event);
            Ok(SecurityAuditReceipt {
                event_id,
                recorded_at,
            })
        })
    }

    fn health(&self) -> PortFuture<Result<SecurityAuditHealth, SecurityAuditError>> {
        let ready = self.ready;
        Box::pin(async move { Ok(SecurityAuditHealth { ready }) })
    }
}

fn offer() -> VersionOffer {
    VersionOffer::try_new(
        vec![PROTOCOL_VERSION_V1],
        PROTOCOL_VERSION_V1,
        vec!["auth".into()],
    )
    .expect("offer")
}

fn durable(sequence: u64, kind: &str) -> RemoteEventView {
    let _ = kind;
    let body = RunEventBody::RunSuspended { reason_code: None };
    let mut event_bytes = [0_u8; 16];
    event_bytes[8..].copy_from_slice(&sequence.to_be_bytes());
    RemoteEventView::try_new(
        RunEvent::try_durable(
            1,
            1,
            EventId::from_bytes(event_bytes),
            SESSION_ID.parse::<SessionId>().expect("session id"),
            LANE_ID.parse::<LaneId>().expect("lane id"),
            RUN_ID.parse::<RunId>().expect("run id"),
            None,
            None,
            None,
            None,
            None,
            sequence,
            sequence,
            UNIX_EPOCH,
            Sensitivity::Public,
            body,
        )
        .expect("durable event"),
    )
    .expect("remote event")
}

fn live_event(sequence: u64) -> RemoteEventView {
    let mut event_bytes = [1_u8; 16];
    event_bytes[8..].copy_from_slice(&sequence.to_be_bytes());
    RemoteEventView::try_new(
        RunEvent::try_transient(
            1,
            1,
            EventId::from_bytes(event_bytes),
            SESSION_ID.parse::<SessionId>().expect("session id"),
            LANE_ID.parse::<LaneId>().expect("lane id"),
            RUN_ID.parse::<RunId>().expect("run id"),
            None,
            None,
            None,
            None,
            None,
            sequence,
            UNIX_EPOCH,
            Sensitivity::Public,
            RunEventBody::QueueDepthWarning(QueueDepthWarning { depth: 1, limit: 1 }),
        )
        .expect("transient event"),
    )
    .expect("remote event")
}

async fn ready_server(sink: RecordingSink) -> Server {
    Server::bind(
        ListenAddr::loopback(0),
        Arc::new(StaticAuthVerifier::new("secret", "tenant-a")),
        Some(Arc::new(sink)),
        Duration::from_millis(100),
    )
    .await
    .expect("bind")
}

#[tokio::test]
async fn unhealthy_sink_is_not_ready() {
    let err = Server::bind(
        ListenAddr::loopback(0),
        Arc::new(StaticAuthVerifier::new("secret", "tenant-a")),
        Some(Arc::new(RecordingSink::unhealthy())),
        Duration::from_millis(50),
    )
    .await
    .expect_err("unhealthy");
    assert!(matches!(err, ServerError::AuditNotReady));
}

#[tokio::test]
async fn missing_sink_is_not_ready() {
    let err = Server::bind(
        ListenAddr::loopback(0),
        Arc::new(StaticAuthVerifier::new("secret", "tenant-a")),
        None,
        Duration::from_millis(50),
    )
    .await
    .expect_err("missing");
    assert!(matches!(err, ServerError::AuditNotReady));
}

#[tokio::test]
async fn reconnect_snapshot_tail_barrier_then_live() {
    let server = ready_server(RecordingSink::ready()).await;
    let mut replica = SessionReplica::new(SESSION_ID, "tenant-a");
    replica.append_durable(durable(1, "run_accepted"));
    replica.append_durable(durable(2, "run_completed"));
    server.hub().insert(replica);

    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    let serve = tokio::spawn({
        let server = Arc::new(server);
        async move {
            server
                .serve(server_end, TransportKind::LoopbackPlaintext)
                .await
        }
    });
    let mut client = RemoteClient::new(client_end);
    let view = client
        .reconnect(
            &offer(),
            RemoteAuthMethod::Loopback,
            locator(SESSION_ID),
            "tenant-a",
            Some(0),
        )
        .await
        .expect("reconnect");
    assert!(view.snapshot.is_none());
    assert_eq!(view.barrier, 2);
    assert_eq!(view.tail.len(), 2);
    assert_eq!(view.tail[1].events()[0].kind().kind_name(), "run_suspended");

    serve.abort();
}

#[tokio::test]
async fn live_event_before_barrier_fails_replica() {
    let mut replica = SessionReplica::new(SESSION_ID, "tenant-a");
    let err = replica.queue_live(live_event(1)).expect_err("early live");
    assert!(matches!(err, ServerError::LiveBeforeBarrier));
    replica.release_barrier();
    replica.queue_live(live_event(1)).expect("after barrier");
}

#[tokio::test]
async fn unknown_version_fails_before_session() {
    let server = ready_server(RecordingSink::ready()).await;
    server
        .hub()
        .insert(SessionReplica::new(SESSION_ID, "tenant-a"));
    let (client_end, server_end) = tokio::io::duplex(16 * 1024);
    let serve = tokio::spawn(async move {
        server
            .serve(server_end, TransportKind::LoopbackPlaintext)
            .await
    });
    let mut client = RemoteClient::new(client_end);
    let bad = VersionOffer::try_new(vec![99], 99, vec!["auth".into()]).expect("bad");
    let err = client
        .reconnect(
            &bad,
            RemoteAuthMethod::Loopback,
            locator(SESSION_ID),
            "tenant-a",
            None,
        )
        .await
        .expect_err("version");
    assert!(
        matches!(err, ServerError::Protocol(_) | ServerError::Io(_)),
        "{err:?}"
    );
    serve.abort();
}

#[tokio::test]
async fn bearer_over_plaintext_is_rejected_and_audited() {
    let sink = RecordingSink::ready();
    let server = ready_server(sink.clone()).await;
    server
        .hub()
        .insert(SessionReplica::new(SESSION_ID, "tenant-a"));
    let (client_end, server_end) = tokio::io::duplex(16 * 1024);
    let serve = tokio::spawn(async move {
        server
            .serve(server_end, TransportKind::LoopbackPlaintext)
            .await
    });
    let mut client = RemoteClient::new(client_end);
    let err = client
        .reconnect(
            &offer(),
            RemoteAuthMethod::Bearer {
                token: "CANARY_SECRET_VALUE".into(),
            },
            locator(SESSION_ID),
            "tenant-a",
            None,
        )
        .await
        .expect_err("bearer");
    assert!(matches!(err, ServerError::AuthenticationFailure));
    assert!(
        sink.categories()
            .contains(&SecurityAuditCategory::AuthenticationFailure)
    );
    assert!(!sink.leak_canary("CANARY_SECRET_VALUE"));
    assert!(!sink.leak_canary(SESSION_ID));
    serve.abort();
}

#[tokio::test]
async fn unknown_locator_does_not_reveal_existence() {
    let sink = RecordingSink::ready();
    let server = ready_server(sink.clone()).await;
    let (client_end, server_end) = tokio::io::duplex(16 * 1024);
    let serve = tokio::spawn(async move {
        server
            .serve(server_end, TransportKind::LoopbackPlaintext)
            .await
    });
    let mut client = RemoteClient::new(client_end);
    let err = client
        .reconnect(
            &offer(),
            RemoteAuthMethod::Loopback,
            locator(MISSING_SESSION_ID),
            "tenant-a",
            None,
        )
        .await
        .expect_err("missing");
    assert!(
        matches!(err, ServerError::UnknownLocator | ServerError::Io(_)),
        "{err:?}"
    );
    assert!(
        sink.categories()
            .contains(&SecurityAuditCategory::UnknownLocator)
    );
    assert!(!sink.leak_canary(MISSING_SESSION_ID));
    serve.abort();
}

#[tokio::test]
async fn second_writer_is_busy_without_confirming_session() {
    let sink = RecordingSink::ready();
    let server = Arc::new(ready_server(sink.clone()).await);
    server
        .hub()
        .insert(SessionReplica::new(SESSION_ID, "tenant-a"));
    let (first_client, first_server) = tokio::io::duplex(16 * 1024);
    let serve_first = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            server
                .serve(first_server, TransportKind::LoopbackPlaintext)
                .await
        }
    });
    let mut first = RemoteClient::new(first_client);
    first
        .reconnect(
            &offer(),
            RemoteAuthMethod::Loopback,
            locator(SESSION_ID),
            "tenant-a",
            None,
        )
        .await
        .expect("first writer");

    let (second_client, second_server) = tokio::io::duplex(16 * 1024);
    let serve_second = tokio::spawn({
        let server = Arc::clone(&server);
        async move {
            server
                .serve(second_server, TransportKind::LoopbackPlaintext)
                .await
        }
    });
    let mut second = RemoteClient::new(second_client);
    let err = second
        .reconnect(
            &offer(),
            RemoteAuthMethod::Loopback,
            locator(SESSION_ID),
            "tenant-a",
            None,
        )
        .await
        .expect_err("busy");
    assert!(
        matches!(err, ServerError::UnknownLocator | ServerError::Io(_)),
        "{err:?}"
    );
    serve_first.abort();
    serve_second.abort();
}

#[tokio::test]
async fn command_idempotency_replays_and_conflicts() {
    let server = ready_server(RecordingSink::ready()).await;
    server
        .hub()
        .insert(SessionReplica::new(SESSION_ID, "tenant-a"));
    let (client_end, server_end) = tokio::io::duplex(32 * 1024);
    let serve = tokio::spawn(async move {
        server
            .serve(server_end, TransportKind::LoopbackPlaintext)
            .await
    });
    let mut client = RemoteClient::new(client_end);
    client
        .reconnect(
            &offer(),
            RemoteAuthMethod::Loopback,
            locator(SESSION_ID),
            "tenant-a",
            None,
        )
        .await
        .expect("open");
    let command = RemoteCommand::try_new(
        "0192e0f6-7c3a-7c11-8a4d-2b6e9c1d0a11",
        locator(SESSION_ID),
        "tenant-a",
        0,
        start_payload(),
    )
    .expect("command");
    let first = client.command(command.clone()).await.expect("first");
    assert!(first.accepted());
    let replay = client.command(command).await.expect("replay");
    assert_eq!(first.digest(), replay.digest());
    assert!(replay.accepted());
    let conflict = RemoteCommand::try_new(
        "0192e0f6-7c3a-7c11-8a4d-2b6e9c1d0a11",
        locator(SESSION_ID),
        "tenant-a",
        0,
        cancel_payload(),
    )
    .expect("conflict");
    let err = client.command(conflict).await.expect_err("conflict");
    assert!(matches!(
        err,
        ServerError::IdempotencyConflict | ServerError::Io(_) | ServerError::Protocol(_)
    ));
    serve.abort();
}

#[tokio::test]
async fn command_start_then_cancel_accepts() {
    let server = ready_server(RecordingSink::ready()).await;
    server
        .hub()
        .insert(SessionReplica::new(SESSION_ID, "tenant-a"));
    let (client_end, server_end) = tokio::io::duplex(32 * 1024);
    let serve = tokio::spawn(async move {
        server
            .serve(server_end, TransportKind::LoopbackPlaintext)
            .await
    });
    let mut client = RemoteClient::new(client_end);
    client
        .reconnect(
            &offer(),
            RemoteAuthMethod::Loopback,
            locator(SESSION_ID),
            "tenant-a",
            None,
        )
        .await
        .expect("open");
    let start = RemoteCommand::try_new(
        "0192e0f6-7c3a-7c11-8a4d-2b6e9c1d0a21",
        locator(SESSION_ID),
        "tenant-a",
        0,
        start_payload(),
    )
    .expect("start");
    let first = client.command(start).await.expect("start");
    assert!(first.accepted());
    let cancel = RemoteCommand::try_new(
        "0192e0f6-7c3a-7c11-8a4d-2b6e9c1d0a22",
        locator(SESSION_ID),
        "tenant-a",
        1,
        cancel_payload(),
    )
    .expect("cancel");
    let second = client.command(cancel).await.expect("cancel");
    assert!(second.accepted());
    serve.abort();
}

#[tokio::test]
async fn receipt_cap_fails_closed_on_new_command() {
    let server = ready_server(RecordingSink::ready()).await;
    server
        .hub()
        .insert(SessionReplica::new(SESSION_ID, "tenant-a").with_receipt_cap(1));
    let (client_end, server_end) = tokio::io::duplex(32 * 1024);
    let serve = tokio::spawn(async move {
        server
            .serve(server_end, TransportKind::LoopbackPlaintext)
            .await
    });
    let mut client = RemoteClient::new(client_end);
    client
        .reconnect(
            &offer(),
            RemoteAuthMethod::Loopback,
            locator(SESSION_ID),
            "tenant-a",
            None,
        )
        .await
        .expect("open");
    let first = RemoteCommand::try_new(
        "0192e0f6-7c3a-7c11-8a4d-2b6e9c1d0a31",
        locator(SESSION_ID),
        "tenant-a",
        0,
        start_payload(),
    )
    .expect("first");
    client.command(first).await.expect("accepted");
    let second = RemoteCommand::try_new(
        "0192e0f6-7c3a-7c11-8a4d-2b6e9c1d0a32",
        locator(SESSION_ID),
        "tenant-a",
        1,
        cancel_payload(),
    )
    .expect("second");
    let err = client.command(second).await.expect_err("cap");
    assert!(matches!(
        err,
        ServerError::ReceiptCap | ServerError::Io(_) | ServerError::Protocol(_)
    ));
    serve.abort();
}

#[tokio::test]
async fn slow_client_disconnects_and_keeps_terminal() {
    let mut server = ready_server(RecordingSink::ready()).await;
    server.set_credit_limits(CreditLimits {
        items: 0,
        bytes: 64,
        ack_deadline: Duration::from_millis(20),
    });
    let mut replica = SessionReplica::new(SESSION_ID, "tenant-a");
    replica.append_durable(durable(1, "run_completed"));
    server.hub().insert(replica);
    let (client_end, server_end) = tokio::io::duplex(16 * 1024);
    let serve = tokio::spawn(async move {
        server
            .serve(server_end, TransportKind::LoopbackPlaintext)
            .await
    });
    let mut client = RemoteClient::new(client_end);
    let view = client
        .reconnect(
            &offer(),
            RemoteAuthMethod::Loopback,
            locator(SESSION_ID),
            "tenant-a",
            Some(0),
        )
        .await
        .expect("reconnect");
    assert_eq!(view.barrier, 1);
    let err = serve.await.expect("join").expect_err("credit");
    assert!(matches!(err, ServerError::CreditTimeout));
}

#[test]
fn listen_policy_code_is_stable() {
    assert_eq!(ServerError::ListenInvalid.code(), "server_listen_invalid");
}
