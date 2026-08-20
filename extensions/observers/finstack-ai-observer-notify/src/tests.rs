use std::sync::Arc;

use finstack_ai_kernel::{
    AssigneeHint, AuthorizationEvidence, ComponentId, ComponentRef, ContentBlock, EffectTag,
    EventTag, Id, IdTag, InteractionCancelled, InteractionExpired, InteractionKind,
    InteractionRequest, InteractionResolution, InteractionTag, LaneTag, Metadata, PrincipalRef,
    QueueDepthWarning, RUN_EVENT_KIND_VERSION, RUN_EVENT_SCHEMA_VERSION, RawJson, RunEvent,
    RunEventBody, RunEventClass, RunTag, Sensitivity, SessionTag, TextBlock, Timestamp, Version,
};
use finstack_ai_runtime::{Observer, ObserverBackpressure, ObserverPayloadMode, SecretString};
use finstack_ai_test::check_observer_conformance;

use super::{
    AssigneeLabel, DeliveryPolicy, InteractionEventKind, NotificationDetail, NotificationSink,
    NotifyObserver,
};

const CANARY: &str = "CANARY_SECRET_VALUE";

fn id<T: IdTag>(value: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    bytes[9..].copy_from_slice(&value.to_be_bytes()[1..]);
    Id::from_bytes(bytes)
}

fn event(kind_body: RunEventBody) -> RunEvent {
    let effect_id = match &kind_body {
        RunEventBody::InteractionRequested(request) => Some(request.effect_id()),
        _ => None,
    };
    if kind_body.kind().class() == RunEventClass::DurableDerived {
        RunEvent::try_durable(
            RUN_EVENT_SCHEMA_VERSION,
            RUN_EVENT_KIND_VERSION,
            id::<EventTag>(10),
            id::<SessionTag>(1),
            id::<LaneTag>(2),
            id::<RunTag>(3),
            None,
            None,
            None,
            effect_id,
            None,
            1,
            1,
            Timestamp::from_unix_ms(1_000).expect("timestamp"),
            Sensitivity::Internal,
            kind_body,
        )
        .expect("event")
    } else {
        RunEvent::try_transient(
            RUN_EVENT_SCHEMA_VERSION,
            RUN_EVENT_KIND_VERSION,
            id::<EventTag>(10),
            id::<SessionTag>(1),
            id::<LaneTag>(2),
            id::<RunTag>(3),
            None,
            None,
            None,
            None,
            None,
            1,
            Timestamp::from_unix_ms(1_000).expect("timestamp"),
            Sensitivity::Internal,
            kind_body,
        )
        .expect("event")
    }
}

fn requested_request(kind: InteractionKind, assignee: Option<AssigneeHint>) -> InteractionRequest {
    let prompt = vec![ContentBlock::Text(
        TextBlock::try_new(format!("please approve {CANARY}")).expect("text"),
    )];
    InteractionRequest::try_new(
        1,
        id::<InteractionTag>(7),
        id::<EffectTag>(8),
        kind,
        prompt,
        RawJson::parse(format!("{{\"marker\":\"{CANARY}\"}}")).expect("schema"),
        ComponentRef::new(
            ComponentId::parse("policy.approval").expect("component"),
            None,
        ),
        Version {
            major: 1,
            minor: 0,
            patch: 0,
        },
        assignee,
        Some(Timestamp::from_unix_ms(9_000).expect("ts")),
        true,
        Metadata::empty(),
    )
    .expect("request")
}

fn requested_body() -> RunEventBody {
    RunEventBody::InteractionRequested(requested_request(
        InteractionKind::Approval,
        Some(AssigneeHint::Role(Arc::from("risk-desk"))),
    ))
}

#[test]
fn projection_whitelists_requested_fields_and_drops_prompt_and_schema() {
    let notification = super::project(&event(requested_body())).expect("projected");
    assert_eq!(notification.event, InteractionEventKind::Requested);
    match &notification.detail {
        NotificationDetail::Requested {
            kind,
            assignee,
            expires_at,
            delegatable,
        } => {
            assert_eq!(kind.as_ref(), "approval");
            assert!(
                matches!(assignee, Some(AssigneeLabel::Role(role)) if role.as_ref() == "risk-desk")
            );
            assert!(expires_at.is_some());
            assert!(delegatable);
        }
        other => panic!("wrong detail: {other:?}"),
    }
    let serialized = serde_json::to_string(&notification).expect("json");
    assert!(!serialized.contains(CANARY));
}

#[test]
fn projection_redacts_resolution_response_and_keeps_labels() {
    let resolution = InteractionResolution::try_new(
        id::<InteractionTag>(7),
        "resolution-1",
        PrincipalRef::try_new("oidc", "reviewer-9", None::<&str>).expect("principal"),
        AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("auth"),
        RawJson::parse(format!("{{\"secret\":\"{CANARY}\"}}")).expect("json"),
        Some("looks-fine"),
    )
    .expect("resolution");
    let notification =
        super::project(&event(RunEventBody::InteractionResolved(resolution))).expect("projected");
    let serialized = serde_json::to_string(&notification).expect("json");
    assert!(serialized.contains("reviewer-9"));
    assert!(serialized.contains("looks-fine"));
    assert!(!serialized.contains(CANARY));
    assert!(!serialized.contains("decision-v1"));
}

#[test]
fn projection_covers_expired_and_cancelled_and_ignores_other_events() {
    let expired = InteractionExpired {
        interaction_id: id::<InteractionTag>(7),
        expired_at: Timestamp::from_unix_ms(9_500).expect("ts"),
    };
    assert!(super::project(&event(RunEventBody::InteractionExpired(expired))).is_some());

    let cancelled =
        InteractionCancelled::try_new(id::<InteractionTag>(7), None, None, Some("timeout"))
            .expect("cancelled");
    let notification =
        super::project(&event(RunEventBody::InteractionCancelled(cancelled))).expect("projected");
    assert!(matches!(
        notification.detail,
        NotificationDetail::Cancelled {
            principal: None,
            reason: Some(ref reason)
        } if reason.as_ref() == "timeout"
    ));

    assert!(
        super::project(&event(RunEventBody::QueueDepthWarning(QueueDepthWarning {
            depth: 3,
            limit: 8
        })))
        .is_none()
    );
}

#[test]
fn custom_kind_label_and_principal_assignee_project_safely() {
    let request = requested_request(
        InteractionKind::Custom {
            name: Arc::from("escalation"),
        },
        Some(AssigneeHint::Principal(
            PrincipalRef::try_new("oidc", "user-1", Some("tenant")).expect("principal"),
        )),
    );
    let notification =
        super::project(&event(RunEventBody::InteractionRequested(request))).expect("projected");
    match &notification.detail {
        NotificationDetail::Requested { kind, assignee, .. } => {
            assert_eq!(kind.as_ref(), "escalation");
            match assignee {
                Some(AssigneeLabel::Principal(principal)) => {
                    assert_eq!(principal.issuer.as_ref(), "oidc");
                    assert_eq!(principal.subject.as_ref(), "user-1");
                }
                other => panic!("wrong assignee: {other:?}"),
            }
        }
        other => panic!("wrong detail: {other:?}"),
    }
    let serialized = serde_json::to_string(&notification).expect("json");
    assert!(!serialized.contains("tenant"));
}

mod tests_support {
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use finstack_ai_runtime::PortFuture;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use crate::{InteractionNotification, NotificationSink, SinkError};

    /// One-shot loopback HTTP server: accepts a single request, captures its
    /// body, answers with the given status and an empty body.
    pub(crate) async fn spawn_loopback_http(
        status: u16,
    ) -> (SocketAddr, Arc<Mutex<Option<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let address = listener.local_addr().expect("addr");
        let slot: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let captured = Arc::clone(&slot);
        tokio::spawn(async move {
            let (mut stream, _peer) = listener.accept().await.expect("accept");
            let mut buffer = Vec::new();
            let mut chunk = [0_u8; 1024];
            let body = loop {
                let read = stream.read(&mut chunk).await.expect("read");
                buffer.extend_from_slice(&chunk[..read]);
                let Some(split) = buffer.windows(4).position(|w| w == b"\r\n\r\n") else {
                    continue;
                };
                let headers = String::from_utf8_lossy(&buffer[..split]).to_ascii_lowercase();
                let length: usize = headers
                    .lines()
                    .find_map(|line| line.strip_prefix("content-length:"))
                    .and_then(|value| value.trim().parse().ok())
                    .unwrap_or(0);
                let body_start = split + 4;
                while buffer.len() < body_start + length {
                    let read = stream.read(&mut chunk).await.expect("read body");
                    buffer.extend_from_slice(&chunk[..read]);
                }
                break String::from_utf8_lossy(&buffer[body_start..body_start + length])
                    .into_owned();
            };
            *captured.lock().expect("lock") = Some(body);
            let response =
                format!("HTTP/1.1 {status} NA\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
            stream.write_all(response.as_bytes()).await.expect("write");
            stream.flush().await.expect("flush");
        });
        (address, slot)
    }

    pub(crate) struct FailingSink {
        attempts: AtomicU64,
    }

    impl FailingSink {
        pub(crate) fn always() -> Self {
            Self {
                attempts: AtomicU64::new(0),
            }
        }

        pub(crate) fn attempts(&self) -> u64 {
            self.attempts.load(Ordering::Relaxed)
        }
    }

    impl NotificationSink for FailingSink {
        fn name(&self) -> &'static str {
            "failing"
        }

        fn deliver(
            &self,
            _notification: InteractionNotification,
        ) -> PortFuture<Result<(), SinkError>> {
            self.attempts.fetch_add(1, Ordering::Relaxed);
            Box::pin(async {
                Err(SinkError::Unavailable {
                    reason: "scripted_failure",
                })
            })
        }
    }

    pub(crate) struct CapturingSink {
        pub(crate) seen: Arc<Mutex<Vec<InteractionNotification>>>,
    }

    impl NotificationSink for CapturingSink {
        fn name(&self) -> &'static str {
            "capturing"
        }

        fn deliver(
            &self,
            notification: InteractionNotification,
        ) -> PortFuture<Result<(), SinkError>> {
            let seen = Arc::clone(&self.seen);
            Box::pin(async move {
                if let Ok(mut guard) = seen.lock() {
                    guard.push(notification);
                }
                Ok(())
            })
        }
    }

    pub(crate) fn capturing_sink() -> (Arc<CapturingSink>, Arc<Mutex<Vec<InteractionNotification>>>)
    {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::new(CapturingSink {
            seen: Arc::clone(&seen),
        });
        (sink, seen)
    }
}

#[tokio::test]
async fn observe_delivers_interaction_events_and_ignores_the_rest() {
    let (sink, seen) = tests_support::capturing_sink();
    let observer = NotifyObserver::try_new(
        sink,
        DeliveryPolicy::default(),
        8,
        ObserverBackpressure::DropProgress,
    )
    .expect("observer");
    observer
        .observe(Arc::from([
            event(requested_body()),
            event(RunEventBody::QueueDepthWarning(QueueDepthWarning {
                depth: 3,
                limit: 8,
            })),
        ]))
        .await
        .expect("observe");
    assert_eq!(seen.lock().expect("lock").len(), 1);
    assert_eq!(observer.delivered(), 1);
    assert_eq!(observer.failed(), 0);
    assert_eq!(observer.dropped(), 0);
}

#[tokio::test]
async fn delivery_retries_then_records_failure_diagnostic() {
    let policy = DeliveryPolicy::try_new(
        std::time::Duration::from_millis(200),
        2,
        std::time::Duration::from_millis(1),
    )
    .expect("policy");
    let sink = Arc::new(tests_support::FailingSink::always());
    let observer = NotifyObserver::try_new(
        Arc::clone(&sink) as Arc<dyn super::NotificationSink>,
        policy,
        8,
        ObserverBackpressure::DropProgress,
    )
    .expect("observer");
    observer
        .observe(Arc::from([event(requested_body())]))
        .await
        .expect("observe");
    assert_eq!(sink.attempts(), 2);
    assert_eq!(observer.failed(), 1);
    assert_eq!(observer.delivered(), 0);
    assert_eq!(
        observer.last_diagnostic().expect("diag").code,
        "notify_delivery_failed"
    );
}

#[tokio::test]
async fn queue_overflow_drops_and_stores_overflow_diagnostic() {
    let (sink, _seen) = tests_support::capturing_sink();
    let observer = NotifyObserver::try_new(
        sink,
        DeliveryPolicy::default(),
        1,
        ObserverBackpressure::DropProgress,
    )
    .expect("observer");
    observer
        .observe(Arc::from([
            event(requested_body()),
            event(requested_body()),
            event(requested_body()),
        ]))
        .await
        .expect("observe");
    assert_eq!(observer.delivered() + observer.dropped(), 3);
    assert!(observer.dropped() > 0);
}

#[test]
fn delivery_policy_clamps_are_enforced() {
    use std::time::Duration;
    assert!(DeliveryPolicy::try_new(Duration::from_millis(500), 0, Duration::ZERO).is_err());
    assert!(DeliveryPolicy::try_new(Duration::from_millis(1), 3, Duration::ZERO).is_err());
    assert!(DeliveryPolicy::try_new(Duration::from_secs(61), 3, Duration::ZERO).is_err());
    assert!(DeliveryPolicy::try_new(Duration::from_secs(5), 6, Duration::ZERO).is_err());
    assert!(DeliveryPolicy::try_new(Duration::from_secs(5), 3, Duration::from_secs(11)).is_err());
    assert!(DeliveryPolicy::try_new(Duration::from_secs(5), 3, Duration::from_millis(500)).is_ok());
}

#[test]
fn webhook_sink_debug_never_leaks_the_url() {
    let sink = super::WebhookSink::try_new(
        SecretString::try_new("https://hooks.example.com/T000/SECRETPART").expect("url"),
        std::time::Duration::from_secs(5),
    )
    .expect("sink");
    let rendered = format!("{sink:?}");
    assert!(rendered.contains("[REDACTED]"));
    assert!(!rendered.contains("SECRETPART"));
    assert!(!rendered.contains("hooks.example.com"));
}

#[test]
fn webhook_sink_rejects_non_http_urls() {
    for bad in ["ftp://x.example/hook", "not a url", "file:///etc/passwd"] {
        assert!(
            super::WebhookSink::try_new(
                SecretString::try_new(bad).expect("secret"),
                std::time::Duration::from_secs(5),
            )
            .is_err()
        );
    }
}

#[tokio::test]
async fn webhook_sink_posts_notification_json_to_loopback() {
    let (address, received) = tests_support::spawn_loopback_http(200).await;
    let sink = super::WebhookSink::try_new(
        SecretString::try_new(format!("http://{address}/hook")).expect("url"),
        std::time::Duration::from_secs(5),
    )
    .expect("sink");
    let notification = super::project(&event(requested_body())).expect("projected");
    sink.deliver(notification).await.expect("delivered");
    let body = received
        .lock()
        .expect("lock")
        .clone()
        .expect("request captured");
    assert!(body.contains("\"event\":\"requested\""));
    assert!(body.contains("\"kind\":\"approval\""));
    assert!(!body.contains(CANARY));
}

#[tokio::test]
async fn webhook_sink_maps_server_errors_to_unavailable() {
    let (address, _received) = tests_support::spawn_loopback_http(500).await;
    let sink = super::WebhookSink::try_new(
        SecretString::try_new(format!("http://{address}/hook")).expect("url"),
        std::time::Duration::from_secs(5),
    )
    .expect("sink");
    let notification = super::project(&event(requested_body())).expect("projected");
    let error = sink.deliver(notification).await.expect_err("must fail");
    assert_eq!(
        error,
        super::SinkError::Unavailable {
            reason: "http_status_error"
        }
    );
}

#[test]
fn slack_text_renders_each_lifecycle_event() {
    let requested = super::project(&event(requested_body())).expect("projected");
    let text = super::slack_text(&requested);
    assert!(text.contains("Interaction requested"));
    assert!(text.contains("approval"));
    assert!(text.contains("risk-desk"));
    assert!(!text.contains(CANARY));

    let resolution = InteractionResolution::try_new(
        id::<InteractionTag>(7),
        "resolution-1",
        PrincipalRef::try_new("oidc", "reviewer-9", None::<&str>).expect("principal"),
        AuthorizationEvidence::try_new("policy-v1", "decision-v1").expect("auth"),
        RawJson::parse(format!("{{\"secret\":\"{CANARY}\"}}")).expect("json"),
        Some("looks-fine"),
    )
    .expect("resolution");
    let resolved =
        super::project(&event(RunEventBody::InteractionResolved(resolution))).expect("projected");
    let text = super::slack_text(&resolved);
    assert!(text.contains("resolved by oidc/reviewer-9"));
    assert!(!text.contains(CANARY));

    let expired = InteractionExpired {
        interaction_id: id::<InteractionTag>(7),
        expired_at: Timestamp::from_unix_ms(9_500).expect("ts"),
    };
    let expired_notification =
        super::project(&event(RunEventBody::InteractionExpired(expired))).expect("projected");
    assert!(super::slack_text(&expired_notification).contains("Interaction expired"));

    let cancelled =
        InteractionCancelled::try_new(id::<InteractionTag>(7), None, None, Some("timeout"))
            .expect("cancelled");
    let cancelled_notification =
        super::project(&event(RunEventBody::InteractionCancelled(cancelled))).expect("projected");
    let text = super::slack_text(&cancelled_notification);
    assert!(text.contains("Interaction cancelled"));
    assert!(text.contains("(timeout)"));
}

#[test]
fn verify_review_interaction_produces_a_review_notification() {
    let request = requested_request(
        InteractionKind::Review,
        Some(AssigneeHint::Role(Arc::from("reviewer"))),
    );
    let notification =
        super::project(&event(RunEventBody::InteractionRequested(request))).expect("projected");
    assert!(matches!(
        notification.detail,
        NotificationDetail::Requested { ref kind, .. } if kind.as_ref() == "review"
    ));
    let text = super::slack_text(&notification);
    assert!(text.contains("review"));
    assert!(!text.contains(CANARY));
}

#[tokio::test]
async fn slack_sink_posts_text_payload() {
    let (address, received) = tests_support::spawn_loopback_http(200).await;
    let sink = super::SlackSink::try_new(
        SecretString::try_new(format!("http://{address}/services/T0/B0/x")).expect("url"),
        std::time::Duration::from_secs(5),
    )
    .expect("sink");
    sink.deliver(super::project(&event(requested_body())).expect("projected"))
        .await
        .expect("delivered");
    let body = received
        .lock()
        .expect("lock")
        .clone()
        .expect("request captured");
    assert!(body.starts_with("{\"text\":"));
    assert!(!body.contains(CANARY));
}

#[test]
fn slack_sink_debug_never_leaks_the_url() {
    let sink = super::SlackSink::try_new(
        SecretString::try_new("https://hooks.slack.com/services/T0/B0/SECRETPART").expect("url"),
        std::time::Duration::from_secs(5),
    )
    .expect("sink");
    let rendered = format!("{sink:?}");
    assert!(rendered.contains("[REDACTED]"));
    assert!(!rendered.contains("SECRETPART"));
    assert!(!rendered.contains("hooks.slack.com"));
}

#[tokio::test]
async fn conformance_accepts_an_empty_batch() {
    let (sink, _seen) = tests_support::capturing_sink();
    let observer = NotifyObserver::try_new(
        sink,
        DeliveryPolicy::default(),
        8,
        ObserverBackpressure::DropProgress,
    )
    .expect("observer");
    assert_eq!(
        observer.descriptor().payload_mode,
        ObserverPayloadMode::Full
    );
    check_observer_conformance(&observer, Arc::from([]))
        .await
        .expect("conformance");
}
