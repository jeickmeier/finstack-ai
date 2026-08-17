use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::{RunEvent, RunEventClass, RunEventKind};

use super::{
    EventBatch, EventDeliveryStats, EventHubConfig, EventLagPolicy, EventPublishError,
    EventSubscriptionCloseReason, EventSubscriptionConfig, EventSubscriptionError,
    EventSubscriptionStatus, ProgressCoalescing, RuntimeEventPublisher, SubscriberAudience,
    validate_event_sequences,
};
use crate::PortFuture;
use crate::host_driver::Signal;

#[derive(Debug, Clone)]
struct SizedEvent {
    event: RunEvent,
    bytes: usize,
}

#[derive(Debug, Default)]
struct SubscriptionState {
    public: EventSubscriptionStatus,
    unreported_dropped: u64,
}

struct PendingBatch {
    events: Vec<SizedEvent>,
    bytes: usize,
    flush_after: Option<Duration>,
}

struct SubscriberInner {
    config: EventSubscriptionConfig,
    pending: PendingBatch,
    delivered: VecDeque<EventBatch>,
    status: SubscriptionState,
    closed: bool,
}

struct Subscriber {
    audience: SubscriberAudience,
    inner: Mutex<SubscriberInner>,
    ready: Signal,
    space: Signal,
    closed: AtomicBool,
}

/// Host-local bounded event-batch receiver for one run subscription.
pub struct EventSubscription {
    subscriber: Arc<Subscriber>,
}

impl EventSubscription {
    /// Receive the next transport batch, or `None` after explicit closure.
    pub async fn next_batch(&mut self) -> Option<EventBatch> {
        loop {
            if let Some(batch) = self.take_ready_batch() {
                return Some(batch);
            }
            if self.subscriber.closed.load(Ordering::Acquire) {
                return self.take_ready_batch();
            }
            let wait = self.subscriber.ready.notified();
            let flush_after = self.flush_wait();
            if let Some(duration) = flush_after {
                if crate::host_driver::timeout(duration, wait).await.is_err() {
                    self.flush_due();
                }
            } else {
                wait.await;
            }
        }
    }

    /// Snapshot the current lifecycle and cumulative delivery statistics.
    #[must_use]
    pub fn status(&self) -> EventSubscriptionStatus {
        self.subscriber.inner.lock().map_or_else(
            |_| EventSubscriptionStatus {
                close_reason: Some(EventSubscriptionCloseReason::ReceiverDropped),
                stats: EventDeliveryStats::default(),
            },
            |inner| inner.status.public,
        )
    }

    /// Close this receiver without affecting the owning run.
    pub fn close(&mut self) {
        self.close_with(EventSubscriptionCloseReason::SubscriberClosed);
    }

    fn take_ready_batch(&self) -> Option<EventBatch> {
        let mut inner = self.subscriber.inner.lock().ok()?;
        let batch = inner.delivered.pop_front()?;
        drop(inner);
        self.subscriber.space.notify_waiters();
        Some(batch)
    }

    fn flush_wait(&self) -> Option<Duration> {
        self.subscriber
            .inner
            .lock()
            .ok()
            .and_then(|inner| inner.pending.flush_after)
    }

    fn flush_due(&self) {
        if let Ok(mut inner) = self.subscriber.inner.lock() {
            flush_pending(&mut inner);
        }
        self.subscriber.ready.notify_waiters();
    }

    fn close_with(&self, reason: EventSubscriptionCloseReason) {
        if let Ok(mut inner) = self.subscriber.inner.lock() {
            flush_pending(&mut inner);
            if inner.status.public.close_reason.is_none() {
                inner.status.public.close_reason = Some(reason);
            }
            inner.closed = true;
        }
        self.subscriber.closed.store(true, Ordering::Release);
        self.subscriber.ready.notify_waiters();
        self.subscriber.space.notify_waiters();
    }
}

impl Drop for EventSubscription {
    fn drop(&mut self) {
        self.close_with(EventSubscriptionCloseReason::ReceiverDropped);
    }
}

#[derive(Clone)]
pub(crate) struct EventHubHandle {
    shared: Arc<HubShared>,
}

struct HubShared {
    config: EventHubConfig,
    subscribers: Mutex<Vec<Arc<Subscriber>>>,
    closed: AtomicBool,
    next_sequence: Mutex<Option<u64>>,
}

impl EventHubHandle {
    pub(crate) fn subscribe_observer(
        &self,
        config: EventSubscriptionConfig,
    ) -> Result<EventSubscription, EventSubscriptionError> {
        self.subscribe(SubscriberAudience::Observer, config)
    }

    pub(crate) fn subscribe_interactive(
        &self,
        config: EventSubscriptionConfig,
    ) -> Result<EventSubscription, EventSubscriptionError> {
        self.subscribe(SubscriberAudience::Interactive, config)
    }

    fn subscribe(
        &self,
        audience: SubscriberAudience,
        config: EventSubscriptionConfig,
    ) -> Result<EventSubscription, EventSubscriptionError> {
        config.validate()?;
        if self.shared.closed.load(Ordering::Acquire) {
            return Err(EventSubscriptionError::HubClosed);
        }
        let mut subscribers = self
            .shared
            .subscribers
            .lock()
            .map_err(|_| EventSubscriptionError::HubClosed)?;
        subscribers.retain(|subscriber| !subscriber.closed.load(Ordering::Acquire));
        if subscribers.len() >= self.shared.config.max_subscribers {
            return Err(EventSubscriptionError::CapacityExhausted);
        }
        let subscriber = Arc::new(Subscriber {
            audience,
            inner: Mutex::new(SubscriberInner {
                config,
                pending: PendingBatch {
                    events: Vec::new(),
                    bytes: 0,
                    flush_after: None,
                },
                delivered: VecDeque::new(),
                status: SubscriptionState::default(),
                closed: false,
            }),
            ready: Signal::new(),
            space: Signal::new(),
            closed: AtomicBool::new(false),
        });
        subscribers.push(Arc::clone(&subscriber));
        Ok(EventSubscription { subscriber })
    }

    pub(crate) fn close(&self) {
        self.shared.closed.store(true, Ordering::Release);
        let subscribers = self
            .shared
            .subscribers
            .lock()
            .map(|mut subscribers| std::mem::take(&mut *subscribers))
            .unwrap_or_default();
        for subscriber in subscribers {
            if let Ok(mut inner) = subscriber.inner.lock() {
                flush_pending(&mut inner);
                if inner.status.public.close_reason.is_none() {
                    inner.status.public.close_reason =
                        Some(EventSubscriptionCloseReason::HubClosed);
                }
                inner.closed = true;
            }
            subscriber.closed.store(true, Ordering::Release);
            subscriber.ready.notify_waiters();
            subscriber.space.notify_waiters();
        }
    }
}

impl RuntimeEventPublisher for EventHubHandle {
    fn publish(&self, events: Arc<[RunEvent]>) -> PortFuture<Result<(), EventPublishError>> {
        let shared = Arc::clone(&self.shared);
        Box::pin(async move {
            if events.is_empty() {
                return Ok(());
            }
            let mut sized = Vec::with_capacity(events.len());
            for event in events.iter() {
                let bytes = super::json_byte_len(event)?;
                sized.push(SizedEvent {
                    event: event.clone(),
                    bytes,
                });
            }
            {
                let mut next_sequence =
                    shared.next_sequence.lock().map_err(|_| EventPublishError {
                        code: "event_hub_closed",
                    })?;
                validate_source_order(&sized, &mut next_sequence)?;
            }
            let subscribers = shared
                .subscribers
                .lock()
                .map_err(|_| EventPublishError {
                    code: "event_hub_closed",
                })?
                .clone();
            for subscriber in subscribers {
                deliver(&subscriber, &sized).await;
            }
            Ok(())
        })
    }
}

pub(crate) fn event_hub(
    config: EventHubConfig,
) -> Result<(EventHubHandle, ()), EventSubscriptionError> {
    let config = config.validate()?;
    Ok((
        EventHubHandle {
            shared: Arc::new(HubShared {
                config,
                subscribers: Mutex::new(Vec::new()),
                closed: AtomicBool::new(false),
                next_sequence: Mutex::new(None),
            }),
        },
        (),
    ))
}

async fn deliver(subscriber: &Subscriber, events: &[SizedEvent]) {
    for item in events {
        if subscriber.closed.load(Ordering::Acquire) {
            return;
        }
        loop {
            match try_ingest(subscriber, item) {
                IngestStep::Skipped | IngestStep::Accepted => break,
                IngestStep::Blocked {
                    timeout,
                    durable,
                    accepted,
                } => {
                    if crate::host_driver::timeout(timeout, wait_for_space(subscriber))
                        .await
                        .is_err()
                    {
                        close_subscriber(
                            subscriber,
                            if durable {
                                EventSubscriptionCloseReason::MissedDurable
                            } else {
                                EventSubscriptionCloseReason::Lagged
                            },
                        );
                        return;
                    }
                    if let IngestStep::Closed(reason) = complete_blocked_flush(subscriber) {
                        close_subscriber(subscriber, reason);
                        return;
                    }
                    if accepted {
                        break;
                    }
                }
                IngestStep::Closed(reason) => {
                    close_subscriber(subscriber, reason);
                    return;
                }
            }
        }
    }
    subscriber.ready.notify_waiters();
}

async fn wait_for_space(subscriber: &Subscriber) {
    loop {
        if subscriber.closed.load(Ordering::Acquire) {
            return;
        }
        if delivered_has_space(subscriber) {
            return;
        }
        subscriber.space.notified().await;
    }
}

fn delivered_has_space(subscriber: &Subscriber) -> bool {
    subscriber
        .inner
        .lock()
        .is_ok_and(|inner| inner.closed || inner.delivered.len() < inner.config.queue_capacity)
}

enum IngestStep {
    Skipped,
    Accepted,
    Blocked {
        timeout: Duration,
        durable: bool,
        accepted: bool,
    },
    Closed(EventSubscriptionCloseReason),
}

fn try_ingest(subscriber: &Subscriber, item: &SizedEvent) -> IngestStep {
    let Ok(mut inner) = subscriber.inner.lock() else {
        return IngestStep::Closed(EventSubscriptionCloseReason::ReceiverDropped);
    };
    if inner.closed {
        return IngestStep::Closed(EventSubscriptionCloseReason::ReceiverDropped);
    }
    if !inner.config.filter.matches(&item.event) {
        return IngestStep::Skipped;
    }
    let durable = item.event.class() == RunEventClass::DurableDerived;
    let oversized = item.bytes > inner.config.batching.flush_bytes;
    if (durable || oversized)
        && !inner.pending.events.is_empty()
        && let Some(step) = flush_or_wait(subscriber.audience, &mut inner, false)
    {
        return step;
    }
    if inner.pending.events.is_empty() {
        inner.pending.flush_after = Some(inner.config.batching.flush_interval);
    }
    inner.pending.bytes = inner.pending.bytes.saturating_add(item.bytes);
    let terminal = is_terminal(item.event.kind());
    inner.pending.events.push(item.clone());
    let immediate_progress =
        inner.config.progress_coalescing == ProgressCoalescing::Disabled && !durable;
    let should_flush = immediate_progress
        || durable
        || oversized
        || terminal
        || inner.pending.events.len() >= inner.config.batching.flush_count
        || inner.pending.bytes >= inner.config.batching.flush_bytes;
    if should_flush && let Some(step) = flush_or_wait(subscriber.audience, &mut inner, true) {
        return step;
    }
    IngestStep::Accepted
}

fn complete_blocked_flush(subscriber: &Subscriber) -> IngestStep {
    let Ok(mut inner) = subscriber.inner.lock() else {
        return IngestStep::Closed(EventSubscriptionCloseReason::ReceiverDropped);
    };
    if inner.closed {
        return IngestStep::Closed(EventSubscriptionCloseReason::ReceiverDropped);
    }
    flush_or_wait(subscriber.audience, &mut inner, true).unwrap_or(IngestStep::Accepted)
}

fn flush_or_wait(
    audience: SubscriberAudience,
    inner: &mut SubscriberInner,
    accepted: bool,
) -> Option<IngestStep> {
    match try_flush_pending(inner) {
        Ok(()) => None,
        Err(FlushBlock { durable }) => match wait_policy(audience, inner, durable) {
            Ok(None) => None,
            Ok(Some(timeout)) => Some(IngestStep::Blocked {
                timeout,
                durable,
                accepted,
            }),
            Err(reason) => Some(IngestStep::Closed(reason)),
        },
    }
}

struct FlushBlock {
    durable: bool,
}

fn wait_policy(
    audience: SubscriberAudience,
    inner: &mut SubscriberInner,
    durable: bool,
) -> Result<Option<Duration>, EventSubscriptionCloseReason> {
    match audience {
        SubscriberAudience::Observer => {
            if durable {
                Err(EventSubscriptionCloseReason::MissedDurable)
            } else {
                drop_pending_progress(inner);
                Ok(None)
            }
        }
        SubscriberAudience::Interactive => match inner.config.lag_policy {
            EventLagPolicy::DropProgress { .. } if !durable => {
                drop_pending_progress(inner);
                Ok(None)
            }
            EventLagPolicy::DropProgress { durable_timeout } => Ok(Some(durable_timeout)),
            EventLagPolicy::BlockBounded { timeout } => Ok(Some(timeout)),
            EventLagPolicy::Disconnect if durable => {
                Err(EventSubscriptionCloseReason::MissedDurable)
            }
            EventLagPolicy::Disconnect => Err(EventSubscriptionCloseReason::Lagged),
        },
    }
}

fn drop_pending_progress(inner: &mut SubscriberInner) {
    let dropped = u64::try_from(inner.pending.events.len()).unwrap_or(u64::MAX);
    inner.pending.events.clear();
    inner.pending.bytes = 0;
    inner.pending.flush_after = None;
    inner.status.public.stats.dropped_progress = inner
        .status
        .public
        .stats
        .dropped_progress
        .saturating_add(dropped);
    inner.status.unreported_dropped = inner.status.unreported_dropped.saturating_add(dropped);
}

fn try_flush_pending(inner: &mut SubscriberInner) -> Result<(), FlushBlock> {
    if inner.pending.events.is_empty() {
        inner.pending.flush_after = None;
        return Ok(());
    }
    let durable = inner
        .pending
        .events
        .iter()
        .any(|item| item.event.class() == RunEventClass::DurableDerived);
    if inner.delivered.len() >= inner.config.queue_capacity {
        return Err(FlushBlock { durable });
    }
    flush_pending(inner);
    Ok(())
}

fn flush_pending(inner: &mut SubscriberInner) {
    inner.pending.flush_after = None;
    if inner.pending.events.is_empty() {
        return;
    }
    let events = inner
        .pending
        .events
        .drain(..)
        .map(|item| item.event)
        .collect::<Vec<_>>();
    inner.pending.bytes = 0;
    let dropped = inner.status.unreported_dropped;
    let count = events.len();
    let last_sequence = events.last().map(RunEvent::transient_sequence);
    let Some(batch) = EventBatch::new(events, dropped) else {
        return;
    };
    inner.delivered.push_back(batch);
    inner.status.public.stats.delivered_batches = inner
        .status
        .public
        .stats
        .delivered_batches
        .saturating_add(1);
    inner.status.public.stats.delivered_events = inner
        .status
        .public
        .stats
        .delivered_events
        .saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
    inner.status.public.stats.last_delivered_sequence = last_sequence;
    inner.status.unreported_dropped = inner.status.unreported_dropped.saturating_sub(dropped);
}

fn close_subscriber(subscriber: &Subscriber, reason: EventSubscriptionCloseReason) {
    if let Ok(mut inner) = subscriber.inner.lock() {
        flush_pending(&mut inner);
        if inner.status.public.close_reason.is_none() {
            inner.status.public.close_reason = Some(reason);
        }
        inner.closed = true;
    }
    subscriber.closed.store(true, Ordering::Release);
    subscriber.ready.notify_waiters();
    subscriber.space.notify_waiters();
}

fn validate_source_order(
    events: &[SizedEvent],
    next_sequence: &mut Option<u64>,
) -> Result<(), EventPublishError> {
    validate_event_sequences(
        events.iter().map(|item| item.event.transient_sequence()),
        next_sequence,
    )
}

const fn is_terminal(kind: RunEventKind) -> bool {
    matches!(
        kind,
        RunEventKind::RunCompleted | RunEventKind::RunFailed | RunEventKind::RunCancelled
    )
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::sync::Arc;
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;

    use finstack_ai_kernel::{
        EventTag, Id, IdTag, LaneTag, QueueDepthWarning, RUN_EVENT_KIND_VERSION,
        RUN_EVENT_SCHEMA_VERSION, RunEvent, RunEventBody, RunTag, Sensitivity, SessionTag,
        Timestamp,
    };

    use super::*;
    use crate::host_driver;
    use crate::{EventBatchConfig, EventFilter, ProgressCoalescing};

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut future = std::pin::pin!(future);
        let waker = Waker::noop().clone();
        let mut cx = Context::from_waker(&waker);
        loop {
            host_driver::drive_local();
            if let Poll::Ready(output) = future.as_mut().poll(&mut cx) {
                return output;
            }
            host_driver::drive_local();
        }
    }

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

    fn hub() -> EventHubHandle {
        event_hub(EventHubConfig {
            source_capacity: 8,
            max_subscribers: 3,
        })
        .expect("hub")
        .0
    }

    fn publish(handle: &EventHubHandle, events: Vec<RunEvent>) {
        block_on(RuntimeEventPublisher::publish(handle, events.into())).expect("publish");
    }

    #[test]
    fn host_sequence_gap_uses_native_mismatch_code() {
        let handle = hub();
        let _subscription = handle
            .subscribe_interactive(subscription_config())
            .expect("subscribe");
        publish(&handle, vec![progress(0)]);
        let error = block_on(RuntimeEventPublisher::publish(
            &handle,
            Arc::from([progress(2)]),
        ))
        .expect_err("gap");
        assert_eq!(error.code, "event_sequence_mismatch");
    }

    #[test]
    fn host_observer_does_not_block_interactive_delivery() {
        let handle = hub();
        let mut observer_config = subscription_config();
        observer_config.queue_capacity = 1;
        observer_config.progress_coalescing = ProgressCoalescing::Disabled;
        observer_config.lag_policy = EventLagPolicy::Disconnect;
        let observer = handle
            .subscribe_observer(observer_config)
            .expect("observer");
        let mut interactive_config = subscription_config();
        interactive_config.queue_capacity = 2;
        interactive_config.batching.flush_count = 32;
        let mut interactive = handle
            .subscribe_interactive(interactive_config)
            .expect("interactive");
        let events = (0..32).map(progress).collect::<Vec<_>>();
        publish(&handle, events.clone());
        let batch = block_on(interactive.next_batch()).expect("interactive batch");
        assert_eq!(batch.events(), events);
        assert!(
            observer.status().close_reason.is_some()
                || observer.status().stats.dropped_progress > 0
        );
    }

    #[test]
    fn json_byte_len_matches_to_vec() {
        let event = progress(0);
        assert_eq!(
            super::super::json_byte_len(&event).expect("count"),
            serde_json::to_vec(&event).expect("vec").len()
        );
    }

    #[test]
    fn host_block_bounded_times_out_as_lagged() {
        let handle = hub();
        let mut config = subscription_config();
        config.queue_capacity = 1;
        config.progress_coalescing = ProgressCoalescing::Disabled;
        config.lag_policy = EventLagPolicy::BlockBounded {
            timeout: Duration::from_millis(15),
        };
        let subscription = handle.subscribe_interactive(config).expect("subscribe");
        publish(&handle, vec![progress(0)]);
        publish(&handle, vec![progress(1)]);
        assert_eq!(
            subscription.status().close_reason,
            Some(EventSubscriptionCloseReason::Lagged)
        );
    }
}
