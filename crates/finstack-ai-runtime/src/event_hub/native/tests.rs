use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::{
    EffectTag, EventTag, Id, IdTag, LaneTag, ModelRequestTag, ModelTextDelta, QueueDepthWarning,
    RUN_EVENT_KIND_VERSION, RUN_EVENT_SCHEMA_VERSION, RunEvent, RunEventBody, RunEventKind, RunTag,
    Sensitivity, SessionTag, Timestamp, TurnTag,
};

use super::super::{
    EventHubConfig, EventLagPolicy, EventSubscriptionCloseReason, EventSubscriptionConfig,
    EventSubscriptionError, RuntimeEventPublisher,
};
use super::{EventHubHandle, EventSubscription, event_hub};
use crate::{EventBatchConfig, EventFilter, ProgressCoalescing};

fn id<T: IdTag>(value: u64) -> Id<T> {
    let mut bytes = [0_u8; 16];
    bytes[6] = 0x70;
    bytes[8] = 0x80;
    bytes[9..].copy_from_slice(&value.to_be_bytes()[1..]);
    Id::from_bytes(bytes)
}

fn progress(sequence: u64) -> RunEvent {
    RunEvent::try_transient(
        RUN_EVENT_SCHEMA_VERSION,
        RUN_EVENT_KIND_VERSION,
        id::<EventTag>(1_000 + sequence),
        id::<SessionTag>(1),
        id::<LaneTag>(2),
        id::<RunTag>(3),
        None,
        None,
        None,
        None,
        None,
        sequence,
        Timestamp::from_unix_ms(1_000).expect("timestamp"),
        Sensitivity::Internal,
        RunEventBody::QueueDepthWarning(QueueDepthWarning {
            depth: u32::try_from(sequence).unwrap_or(u32::MAX),
            limit: u32::MAX,
        }),
    )
    .expect("progress event")
}

fn confidential_progress(sequence: u64) -> RunEvent {
    confidential_progress_with_text(sequence, "delta")
}

fn confidential_progress_with_text(sequence: u64, text: &str) -> RunEvent {
    RunEvent::try_transient(
        RUN_EVENT_SCHEMA_VERSION,
        RUN_EVENT_KIND_VERSION,
        id::<EventTag>(1_000 + sequence),
        id::<SessionTag>(1),
        id::<LaneTag>(2),
        id::<RunTag>(3),
        Some(id::<TurnTag>(4)),
        Some(id::<ModelRequestTag>(5)),
        None,
        Some(id::<EffectTag>(6)),
        None,
        sequence,
        Timestamp::from_unix_ms(1_000).expect("timestamp"),
        Sensitivity::Confidential,
        RunEventBody::ModelTextDelta(ModelTextDelta::try_new(text).expect("delta")),
    )
    .expect("confidential progress event")
}

fn durable(sequence: u64) -> RunEvent {
    RunEvent::try_durable(
        RUN_EVENT_SCHEMA_VERSION,
        RUN_EVENT_KIND_VERSION,
        id::<EventTag>(1_000 + sequence),
        id::<SessionTag>(1),
        id::<LaneTag>(2),
        id::<RunTag>(3),
        None,
        None,
        None,
        None,
        None,
        sequence + 1,
        sequence,
        Timestamp::from_unix_ms(1_000).expect("timestamp"),
        Sensitivity::Internal,
        RunEventBody::RunSuspended { reason_code: None },
    )
    .expect("durable event")
}

fn terminal(sequence: u64) -> RunEvent {
    RunEvent::try_durable(
        RUN_EVENT_SCHEMA_VERSION,
        RUN_EVENT_KIND_VERSION,
        id::<EventTag>(1_000 + sequence),
        id::<SessionTag>(1),
        id::<LaneTag>(2),
        id::<RunTag>(3),
        None,
        None,
        None,
        None,
        None,
        sequence + 1,
        sequence,
        Timestamp::from_unix_ms(1_000).expect("timestamp"),
        Sensitivity::Internal,
        RunEventBody::RunCancelled { request_id: None },
    )
    .expect("terminal event")
}

fn subscription_config() -> EventSubscriptionConfig {
    EventSubscriptionConfig {
        queue_capacity: 8,
        filter: EventFilter {
            include_durable: true,
            include_transient: true,
            kinds: Arc::from([]),
            max_sensitivity: Sensitivity::Credential,
        },
        batching: EventBatchConfig {
            flush_count: 8,
            flush_bytes: 64 * 1_024,
            flush_interval: Duration::from_secs(1),
        },
        progress_coalescing: ProgressCoalescing::Enabled,
        lag_policy: EventLagPolicy::DropProgress {
            durable_timeout: Duration::from_millis(10),
        },
    }
}

fn spawn_hub(max_subscribers: usize) -> (EventHubHandle, tokio::task::JoinHandle<()>) {
    let (hub, task) = event_hub(EventHubConfig {
        source_capacity: 8,
        max_subscribers,
    })
    .expect("hub");
    (hub, tokio::spawn(task.run()))
}

async fn publish(hub: &EventHubHandle, events: Vec<RunEvent>) {
    RuntimeEventPublisher::publish(hub, events.into())
        .await
        .expect("publish");
}

async fn close_hub(hub: &EventHubHandle, task: tokio::task::JoinHandle<()>) {
    hub.close().await;
    task.await.expect("hub task");
}

async fn wait_for_reason(subscription: &EventSubscription, reason: EventSubscriptionCloseReason) {
    for _ in 0..100 {
        if subscription.status().close_reason == Some(reason) {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("subscription did not close with {reason:?}");
}

#[tokio::test]
async fn configuration_and_registration_bounds_are_rejected() {
    assert_eq!(
        EventHubConfig {
            source_capacity: 0,
            max_subscribers: 1,
        }
        .validate(),
        Err(EventSubscriptionError::InvalidConfiguration)
    );
    assert_eq!(
        EventHubConfig {
            source_capacity: 1,
            max_subscribers: 0,
        }
        .validate(),
        Err(EventSubscriptionError::InvalidConfiguration)
    );
    let mut invalid = subscription_config();
    invalid.queue_capacity = 0;
    assert_eq!(
        invalid.validate(),
        Err(EventSubscriptionError::InvalidConfiguration)
    );
    for mutate in [
        |config: &mut EventSubscriptionConfig| config.batching.flush_count = 0,
        |config: &mut EventSubscriptionConfig| config.batching.flush_bytes = 0,
        |config: &mut EventSubscriptionConfig| {
            config.batching.flush_interval = Duration::ZERO;
        },
    ] {
        let mut config = subscription_config();
        mutate(&mut config);
        assert_eq!(
            config.validate(),
            Err(EventSubscriptionError::InvalidConfiguration)
        );
    }
    for policy in [
        EventLagPolicy::BlockBounded {
            timeout: Duration::ZERO,
        },
        EventLagPolicy::DropProgress {
            durable_timeout: Duration::ZERO,
        },
    ] {
        let mut config = subscription_config();
        config.lag_policy = policy;
        assert_eq!(
            config.validate(),
            Err(EventSubscriptionError::InvalidConfiguration)
        );
    }
    let mut invalid_filter = subscription_config();
    invalid_filter.filter.include_durable = false;
    invalid_filter.filter.include_transient = false;
    assert_eq!(
        invalid_filter.validate(),
        Err(EventSubscriptionError::InvalidConfiguration)
    );
    let mut duplicate_filter = subscription_config();
    duplicate_filter.filter.kinds = Arc::from([
        RunEventKind::QueueDepthWarning,
        RunEventKind::QueueDepthWarning,
    ]);
    assert_eq!(
        duplicate_filter.validate(),
        Err(EventSubscriptionError::InvalidConfiguration)
    );

    let (hub, task) = spawn_hub(1);
    let mut first = hub
        .subscribe_interactive(subscription_config())
        .await
        .expect("first subscription");
    assert!(matches!(
        hub.subscribe_interactive(subscription_config()).await,
        Err(EventSubscriptionError::CapacityExhausted)
    ));
    first.close();
    let _replacement = hub
        .subscribe_interactive(subscription_config())
        .await
        .expect("replacement subscription");
    publish(&hub, vec![progress(0)]).await;
    let sequence_error = RuntimeEventPublisher::publish(&hub, Arc::from([progress(2)]))
        .await
        .expect_err("source sequence gap");
    assert_eq!(sequence_error.code, "event_sequence_mismatch");
    publish(&hub, vec![progress(1)]).await;
    close_hub(&hub, task).await;
}

#[test]
fn json_byte_len_matches_to_vec() {
    let event = progress(0);
    assert_eq!(
        super::super::json_byte_len(&event).expect("count"),
        serde_json::to_vec(&event).expect("vec").len()
    );
}

#[tokio::test(start_paused = true)]
async fn count_byte_and_timer_thresholds_flush_without_reordering() {
    let (hub, task) = spawn_hub(3);

    let mut count_config = subscription_config();
    count_config.batching.flush_count = 2;
    let mut by_count = hub
        .subscribe_interactive(count_config)
        .await
        .expect("count subscription");

    let first = progress(0);
    let second = progress(1);
    let mut byte_config = subscription_config();
    byte_config.batching.flush_count = 100;
    byte_config.batching.flush_bytes = serde_json::to_vec(&first).expect("serialize").len()
        + serde_json::to_vec(&second).expect("serialize").len();
    let mut by_bytes = hub
        .subscribe_interactive(byte_config)
        .await
        .expect("byte subscription");

    let mut timer_config = subscription_config();
    timer_config.batching.flush_count = 100;
    timer_config.batching.flush_bytes = usize::MAX;
    timer_config.batching.flush_interval = Duration::from_millis(50);
    let mut by_timer = hub
        .subscribe_interactive(timer_config)
        .await
        .expect("timer subscription");

    publish(&hub, vec![first.clone(), second.clone()]).await;
    let count_batch = by_count.next_batch().await.expect("count batch");
    assert_eq!(count_batch.events(), [first.clone(), second.clone()]);
    let byte_batch = by_bytes.next_batch().await.expect("byte batch");
    assert_eq!(byte_batch.events(), [first.clone(), second.clone()]);

    tokio::time::advance(Duration::from_millis(49)).await;
    assert!(by_timer.receiver.try_recv().is_err());
    tokio::time::advance(Duration::from_millis(1)).await;
    let timer_batch = by_timer.next_batch().await.expect("timer batch");
    assert_eq!(timer_batch.events(), [first, second]);
    close_hub(&hub, task).await;
}

#[tokio::test(start_paused = true)]
async fn oversized_durable_boundary_and_terminal_events_flush_immediately() {
    let (hub, task) = spawn_hub(1);
    let mut config = subscription_config();
    config.batching.flush_count = 100;
    config.batching.flush_bytes = serde_json::to_vec(&progress(0)).expect("serialize").len() + 1;
    config.batching.flush_interval = Duration::from_mins(1);
    let mut subscription = hub
        .subscribe_interactive(config)
        .await
        .expect("subscription");

    publish(
        &hub,
        vec![
            progress(0),
            confidential_progress_with_text(1, &"x".repeat(1_024)),
            progress(2),
            durable(3),
            terminal(4),
        ],
    )
    .await;
    for expected in 0..5 {
        let batch = subscription.next_batch().await.expect("immediate batch");
        assert_eq!(batch.events().len(), 1);
        assert_eq!(batch.first_sequence(), expected);
        assert_eq!(batch.last_sequence(), expected);
    }
    close_hub(&hub, task).await;
}

#[tokio::test]
async fn filtering_and_multi_subscriber_delivery_preserve_identity_and_order() {
    let (hub, task) = spawn_hub(2);
    let mut all_config = subscription_config();
    all_config.batching.flush_count = 16;
    let mut all = hub
        .subscribe_interactive(all_config)
        .await
        .expect("all subscription");

    let mut filtered_config = subscription_config();
    filtered_config.filter.max_sensitivity = Sensitivity::Internal;
    filtered_config.filter.kinds =
        Arc::from([RunEventKind::QueueDepthWarning, RunEventKind::RunSuspended]);
    let mut filtered = hub
        .subscribe_interactive(filtered_config)
        .await
        .expect("filtered subscription");

    let source = vec![progress(0), confidential_progress(1), durable(2)];
    publish(&hub, source.clone()).await;

    let mut flattened = Vec::new();
    while flattened.len() < source.len() {
        flattened.extend_from_slice(all.next_batch().await.expect("all batch").events());
    }
    assert_eq!(flattened, source);

    let first = filtered.next_batch().await.expect("filtered progress");
    let second = filtered.next_batch().await.expect("filtered durable");
    assert_eq!(first.events(), [progress(0)]);
    assert_eq!(second.events(), [durable(2)]);
    assert_eq!(first.dropped_progress(), 0);
    assert_eq!(second.dropped_progress(), 0);
    close_hub(&hub, task).await;
}

#[tokio::test(start_paused = true)]
async fn every_lag_policy_is_bounded_and_durable_loss_is_explicit() {
    let (hub, task) = spawn_hub(3);

    let mut drop_config = subscription_config();
    drop_config.queue_capacity = 1;
    drop_config.progress_coalescing = ProgressCoalescing::Disabled;
    let mut drops = hub
        .subscribe_interactive(drop_config)
        .await
        .expect("drop subscription");

    let mut disconnect_config = subscription_config();
    disconnect_config.queue_capacity = 1;
    disconnect_config.progress_coalescing = ProgressCoalescing::Disabled;
    disconnect_config.lag_policy = EventLagPolicy::Disconnect;
    let disconnect = hub
        .subscribe_interactive(disconnect_config)
        .await
        .expect("disconnect subscription");

    let mut block_config = subscription_config();
    block_config.queue_capacity = 1;
    block_config.progress_coalescing = ProgressCoalescing::Disabled;
    block_config.lag_policy = EventLagPolicy::BlockBounded {
        timeout: Duration::from_millis(25),
    };
    let blocked = hub
        .subscribe_interactive(block_config)
        .await
        .expect("blocked subscription");

    publish(&hub, vec![progress(0)]).await;
    tokio::task::yield_now().await;
    publish(&hub, vec![progress(1), progress(2), progress(3)]).await;
    tokio::task::yield_now().await;

    let first = drops.next_batch().await.expect("first drop-policy batch");
    assert_eq!(first.first_sequence(), 0);
    publish(&hub, vec![progress(4)]).await;
    tokio::task::yield_now().await;
    let reported = drops.next_batch().await.expect("reported drop batch");
    assert!(reported.dropped_progress() > 0);
    assert!(
        drops.status().stats.dropped_progress >= reported.dropped_progress(),
        "batch drop count must be a subset of the cumulative count"
    );

    tokio::time::advance(Duration::from_millis(25)).await;
    wait_for_reason(&disconnect, EventSubscriptionCloseReason::Lagged).await;
    wait_for_reason(&blocked, EventSubscriptionCloseReason::Lagged).await;
    close_hub(&hub, task).await;
}

#[tokio::test(start_paused = true)]
async fn undeliverable_durable_event_closes_with_missed_durable() {
    let (hub, task) = spawn_hub(1);
    let mut config = subscription_config();
    config.queue_capacity = 1;
    config.progress_coalescing = ProgressCoalescing::Disabled;
    config.lag_policy = EventLagPolicy::DropProgress {
        durable_timeout: Duration::from_millis(20),
    };
    let subscription = hub
        .subscribe_interactive(config)
        .await
        .expect("subscription");

    publish(&hub, vec![progress(0)]).await;
    tokio::task::yield_now().await;
    publish(&hub, vec![durable(1)]).await;
    tokio::time::advance(Duration::from_millis(20)).await;
    wait_for_reason(&subscription, EventSubscriptionCloseReason::MissedDurable).await;
    close_hub(&hub, task).await;
}

#[tokio::test]
async fn stalled_observer_does_not_delay_interactive_delivery() {
    let (hub, task) = spawn_hub(2);
    let mut observer_config = subscription_config();
    observer_config.queue_capacity = 1;
    observer_config.progress_coalescing = ProgressCoalescing::Disabled;
    observer_config.lag_policy = EventLagPolicy::Disconnect;
    let observer = hub
        .subscribe_observer(observer_config)
        .await
        .expect("observer subscription");
    let mut interactive_config = subscription_config();
    interactive_config.queue_capacity = 2;
    interactive_config.batching.flush_count = 100;
    let mut interactive = hub
        .subscribe_interactive(interactive_config)
        .await
        .expect("interactive subscription");

    let events = (0..100).map(progress).collect::<Vec<_>>();
    publish(&hub, events.clone()).await;
    let batch = interactive.next_batch().await.expect("interactive batch");
    assert_eq!(batch.events(), events);
    assert!(
        observer.status().close_reason.is_some() || observer.status().stats.dropped_progress > 0
    );
    close_hub(&hub, task).await;
}

#[tokio::test]
async fn direct_hub_keeps_one_hundred_thousand_progress_events_bounded() {
    let (hub, task) = event_hub(EventHubConfig {
        source_capacity: 2,
        max_subscribers: 1,
    })
    .expect("hub");
    let task = tokio::spawn(task.run());
    let mut config = subscription_config();
    config.queue_capacity = 4;
    config.batching.flush_count = 32;
    config.batching.flush_bytes = usize::MAX;
    let subscription = hub
        .subscribe_interactive(config)
        .await
        .expect("subscription");

    for start in (0..100_000_u64).step_by(64) {
        let events = (start..(start + 64).min(100_000))
            .map(progress)
            .collect::<Vec<_>>();
        publish(&hub, events).await;
    }
    close_hub(&hub, task).await;
    let stats = subscription.status().stats;
    assert_eq!(
        stats.delivered_events + stats.dropped_progress,
        100_000,
        "delivered={}, dropped={}",
        stats.delivered_events,
        stats.dropped_progress,
    );
    assert_eq!(
        subscription.status().close_reason,
        Some(EventSubscriptionCloseReason::HubClosed)
    );
}
