//! Reference server integration coverage.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai::Session;
use finstack_ai_protocol::{
    PRE_AUTH_FRAME_MAX_BYTES, PROTOCOL_VERSION_V1, RemoteAuthMethod, RemoteCommand,
    RemoteCommandOp, RemoteEventView, RemoteLocator, RemoteSnapshot, VersionOffer,
    decode_frame_len, encode_frame,
};
use finstack_ai_runtime::{
    PortFuture, SecurityAuditCategory, SecurityAuditError, SecurityAuditEvent, SecurityAuditHealth,
    SecurityAuditReceipt, SecurityAuditSink,
};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};

use finstack_ai_server::{
    CreditLimits, ListenAddr, RemoteClient, SERVER_LISTEN_INVALID, Server, ServerError,
    SessionReplica, StaticAuthVerifier, TransportKind,
};

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
    RemoteEventView::new(format!("evt-{sequence}"), kind, Some(sequence), sequence)
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
    let mut replica = SessionReplica::new("sess-1", "tenant-a");
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
            RemoteLocator::new("sess-1", None, None),
            "tenant-a",
            Some(0),
        )
        .await
        .expect("reconnect");
    assert!(view.snapshot.is_some());
    assert_eq!(view.barrier, 2);
    assert_eq!(view.tail.len(), 1);
    assert_eq!(view.tail[0].kind(), "run_completed");

    serve.abort();
}

#[tokio::test]
async fn live_event_before_barrier_fails_replica() {
    let mut replica = SessionReplica::new("sess-1", "tenant-a");
    let err = replica
        .queue_live(RemoteEventView::new("live", "model_text_delta", None, 1))
        .expect_err("early live");
    assert!(matches!(err, ServerError::LiveBeforeBarrier));
    replica.release_barrier();
    replica
        .queue_live(RemoteEventView::new("live", "model_text_delta", None, 1))
        .expect("after barrier");
}

#[tokio::test]
async fn unknown_version_fails_before_session() {
    let server = ready_server(RecordingSink::ready()).await;
    server
        .hub()
        .insert(SessionReplica::new("sess-1", "tenant-a"));
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
            RemoteLocator::new("sess-1", None, None),
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
        .insert(SessionReplica::new("sess-1", "tenant-a"));
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
            RemoteLocator::new("sess-1", None, None),
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
    assert!(!sink.leak_canary("sess-1"));
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
            RemoteLocator::new("missing-session", None, None),
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
    assert!(!sink.leak_canary("missing-session"));
    serve.abort();
}

#[tokio::test]
async fn second_writer_is_busy_without_confirming_session() {
    let sink = RecordingSink::ready();
    let server = Arc::new(ready_server(sink.clone()).await);
    server
        .hub()
        .insert(SessionReplica::new("sess-1", "tenant-a"));
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
            RemoteLocator::new("sess-1", None, None),
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
            RemoteLocator::new("sess-1", None, None),
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
        .insert(SessionReplica::new("sess-1", "tenant-a"));
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
            RemoteLocator::new("sess-1", None, None),
            "tenant-a",
            None,
        )
        .await
        .expect("open");
    let command = RemoteCommand::try_new(
        "0192e0f6-7c3a-7c11-8a4d-2b6e9c1d0a11",
        RemoteLocator::new("sess-1", None, None),
        "tenant-a",
        RemoteCommandOp::Start,
    )
    .expect("command");
    let first = client.command(command.clone()).await.expect("first");
    let replay = client.command(command).await.expect("replay");
    assert_eq!(first.digest(), replay.digest());
    let conflict = RemoteCommand::try_new(
        "0192e0f6-7c3a-7c11-8a4d-2b6e9c1d0a11",
        RemoteLocator::new("sess-1", None, None),
        "tenant-a",
        RemoteCommandOp::Cancel,
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
async fn slow_client_disconnects_and_keeps_terminal() {
    let mut server = ready_server(RecordingSink::ready()).await;
    server.set_credit_limits(CreditLimits {
        items: 0,
        bytes: 64,
        ack_deadline: Duration::from_millis(20),
    });
    let mut replica = SessionReplica::new("sess-1", "tenant-a");
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
            RemoteLocator::new("sess-1", None, None),
            "tenant-a",
            Some(0),
        )
        .await
        .expect("reconnect");
    assert_eq!(view.barrier, 1);
    let err = serve.await.expect("join").expect_err("credit");
    assert!(matches!(err, ServerError::CreditTimeout));
}

#[tokio::test]
async fn oversized_pre_auth_length_fails_before_allocation() {
    let header = u32::try_from(PRE_AUTH_FRAME_MAX_BYTES + 1)
        .expect("fits")
        .to_be_bytes();
    assert!(decode_frame_len(header, PRE_AUTH_FRAME_MAX_BYTES).is_err());
    let frame = encode_frame(&[0x61; 4], PRE_AUTH_FRAME_MAX_BYTES).expect("frame");
    assert_eq!(&frame[..4], &[0, 0, 0, 4]);
}

#[tokio::test]
async fn public_session_projects_to_remote_snapshot() {
    let store = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 4,
            batches_per_session: 8,
            records_per_session: 16,
            snapshot_bytes: 1024,
        })
        .expect("store"),
    );
    let session = Session::create(store, "tenant-a").await.expect("session");
    let snapshot = RemoteSnapshot::new(session.session_id().to_string(), 1);
    assert_eq!(snapshot.session_id(), session.session_id().to_string());
    let encoded = format!("{snapshot:?}");
    assert!(!encoded.contains("rusqlite"));
    assert!(!encoded.contains("page"));
}

#[test]
fn listen_policy_code_is_stable() {
    assert_eq!(SERVER_LISTEN_INVALID, "server_listen_invalid");
    ListenAddr::plaintext_tcp("8.8.8.8:443".parse().expect("addr"))
        .validate()
        .expect_err("plaintext");
}
