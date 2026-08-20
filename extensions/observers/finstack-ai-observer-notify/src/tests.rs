use std::sync::Arc;

use finstack_ai_kernel::{
    AssigneeHint, AuthorizationEvidence, ComponentId, ComponentRef, ContentBlock, EffectTag,
    EventTag, Id, IdTag, InteractionCancelled, InteractionExpired, InteractionKind,
    InteractionRequest, InteractionResolution, InteractionTag, LaneTag, Metadata, PrincipalRef,
    QueueDepthWarning, RUN_EVENT_KIND_VERSION, RUN_EVENT_SCHEMA_VERSION, RawJson, RunEvent,
    RunEventBody, RunEventClass, RunTag, Sensitivity, SessionTag, TextBlock, Timestamp, Version,
};
use finstack_ai_runtime::{Observer, ObserverBackpressure, ObserverPayloadMode};
use finstack_ai_test::check_observer_conformance;

use super::{
    AssigneeLabel, DeliveryPolicy, InteractionEventKind, NotificationDetail, NotifyObserver,
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
        ComponentRef::new(ComponentId::parse("policy.approval").expect("component"), None),
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
    let notification = super::project(&event(RunEventBody::InteractionRequested(request)))
        .expect("projected");
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
    use std::sync::{Arc, Mutex};

    use finstack_ai_runtime::PortFuture;

    use crate::{InteractionNotification, NotificationSink, SinkError};

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

    pub(crate) fn capturing_sink()
    -> (Arc<CapturingSink>, Arc<Mutex<Vec<InteractionNotification>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::new(CapturingSink {
            seen: Arc::clone(&seen),
        });
        (sink, seen)
    }
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
