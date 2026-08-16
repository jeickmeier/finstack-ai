//! Bounded event delivery contracts and the native per-run event hub.

use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::{RunEvent, RunEventClass, RunEventKind, Sensitivity};
use thiserror::Error;

/// Per-run source bounds for the native event hub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventHubConfig {
    /// Maximum event publications waiting for hub fan-out.
    pub source_capacity: usize,
    /// Maximum simultaneously registered subscriptions.
    pub max_subscribers: usize,
}

impl EventHubConfig {
    /// Validate non-zero hub bounds.
    ///
    /// # Errors
    ///
    /// Returns [`EventSubscriptionError::InvalidConfiguration`] for a zero bound.
    pub fn validate(self) -> Result<Self, EventSubscriptionError> {
        if self.source_capacity == 0 || self.max_subscribers == 0 {
            return Err(EventSubscriptionError::InvalidConfiguration);
        }
        Ok(self)
    }
}

/// Transport batching thresholds for one subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EventBatchConfig {
    /// Flush after this many events.
    pub flush_count: usize,
    /// Flush when serialized event bytes reach this threshold.
    ///
    /// One valid event larger than this threshold is delivered alone.
    pub flush_bytes: usize,
    /// Flush after this much operational time from the first pending event.
    pub flush_interval: Duration,
}

/// Immutable event selector for one subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventFilter {
    /// Include durable-derived events.
    pub include_durable: bool,
    /// Include transient progress events.
    pub include_transient: bool,
    /// Included kinds; empty means all kinds in the selected classes.
    pub kinds: Arc<[RunEventKind]>,
    /// Highest sensitivity delivered to this subscription.
    pub max_sensitivity: Sensitivity,
}

impl EventFilter {
    fn validate(&self) -> Result<(), EventSubscriptionError> {
        if !self.include_durable && !self.include_transient {
            return Err(EventSubscriptionError::InvalidConfiguration);
        }
        for (index, kind) in self.kinds.iter().enumerate() {
            if self.kinds[..index].contains(kind) {
                return Err(EventSubscriptionError::InvalidConfiguration);
            }
        }
        Ok(())
    }

    #[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
    fn matches(&self, event: &RunEvent) -> bool {
        let class_allowed = match event.class() {
            RunEventClass::DurableDerived => self.include_durable,
            RunEventClass::Transient => self.include_transient,
        };
        class_allowed
            && (self.kinds.is_empty() || self.kinds.contains(&event.kind()))
            && sensitivity_rank(event.sensitivity()) <= sensitivity_rank(self.max_sensitivity)
    }
}

/// Whether fine-grained progress waits for transport batching thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressCoalescing {
    /// Flush each transient progress event without waiting for more progress.
    Disabled,
    /// Accumulate intact events until a count, byte, time, or durable boundary.
    Enabled,
}

/// Who registered the subscription. Observer delivery never waits.
#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SubscriberAudience {
    /// Frontend or SDK consumer; honors [`EventLagPolicy`].
    Interactive,
    /// Isolated observer; try-only, drop progress, fail closed on durable loss.
    Observer,
}

/// Behavior when a subscription cannot keep up with event delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventLagPolicy {
    /// Backpressure all selected events for at most the configured timeout.
    BlockBounded {
        /// Maximum operational wait for queue capacity.
        timeout: Duration,
    },
    /// Drop transient progress, but protect durable events with a bounded wait.
    DropProgress {
        /// Maximum operational wait for a durable event.
        durable_timeout: Duration,
    },
    /// Disconnect immediately when the bounded queue is full.
    Disconnect,
}

/// Complete operational configuration for one event subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventSubscriptionConfig {
    /// Bounded capacity of both the subscription inbox and delivered-batch queue.
    pub queue_capacity: usize,
    /// Immutable event filter.
    pub filter: EventFilter,
    /// Transport batching thresholds.
    pub batching: EventBatchConfig,
    /// Progress coalescing mode.
    pub progress_coalescing: ProgressCoalescing,
    /// Slow-consumer behavior.
    pub lag_policy: EventLagPolicy,
}

impl EventSubscriptionConfig {
    /// Validate all queue, batch, filter, and timeout bounds.
    ///
    /// # Errors
    ///
    /// Returns [`EventSubscriptionError::InvalidConfiguration`] when a bound is zero
    /// or the filter selects neither event class or repeats an event kind.
    pub fn validate(&self) -> Result<(), EventSubscriptionError> {
        if self.queue_capacity == 0
            || self.batching.flush_count == 0
            || self.batching.flush_bytes == 0
            || self.batching.flush_interval.is_zero()
        {
            return Err(EventSubscriptionError::InvalidConfiguration);
        }
        match self.lag_policy {
            EventLagPolicy::BlockBounded { timeout } if timeout.is_zero() => {
                return Err(EventSubscriptionError::InvalidConfiguration);
            }
            EventLagPolicy::DropProgress { durable_timeout } if durable_timeout.is_zero() => {
                return Err(EventSubscriptionError::InvalidConfiguration);
            }
            _ => {}
        }
        self.filter.validate()
    }
}

/// One transport batch of logically ordered runtime events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventBatch {
    events: Arc<[RunEvent]>,
    first_sequence: u64,
    last_sequence: u64,
    dropped_progress: u64,
}

impl EventBatch {
    #[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
    pub(crate) fn new(events: Vec<RunEvent>, dropped_progress: u64) -> Option<Self> {
        let first_sequence = events.first()?.transient_sequence();
        let last_sequence = events.last()?.transient_sequence();
        Some(Self {
            events: events.into(),
            first_sequence,
            last_sequence,
            dropped_progress,
        })
    }

    /// Ordered events in this transport batch.
    #[must_use]
    pub fn events(&self) -> &[RunEvent] {
        &self.events
    }

    /// First contained run-stream sequence.
    #[must_use]
    pub const fn first_sequence(&self) -> u64 {
        self.first_sequence
    }

    /// Last contained run-stream sequence.
    #[must_use]
    pub const fn last_sequence(&self) -> u64 {
        self.last_sequence
    }

    /// Lag-dropped transient events since the preceding delivered batch.
    #[must_use]
    pub const fn dropped_progress(&self) -> u64 {
        self.dropped_progress
    }
}

/// Cumulative delivery statistics for one subscription.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EventDeliveryStats {
    /// Delivered transport batches.
    pub delivered_batches: u64,
    /// Delivered logical events.
    pub delivered_events: u64,
    /// Transient progress dropped because of lag.
    pub dropped_progress: u64,
    /// Last delivered run-stream sequence.
    pub last_delivered_sequence: Option<u64>,
}

/// Explicit reason an event subscription stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventSubscriptionCloseReason {
    /// The owning run event hub closed normally.
    HubClosed,
    /// The subscriber explicitly closed the subscription.
    SubscriberClosed,
    /// The delivered-batch receiver was dropped.
    ReceiverDropped,
    /// The subscription exceeded its configured lag policy on transient work.
    Lagged,
    /// A durable event could not be delivered within the configured policy.
    MissedDurable,
}

/// Current lifecycle and delivery statistics for a subscription.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EventSubscriptionStatus {
    /// Close reason, or `None` while active.
    pub close_reason: Option<EventSubscriptionCloseReason>,
    /// Current cumulative statistics.
    pub stats: EventDeliveryStats,
}

/// Event subscription construction and lifecycle failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum EventSubscriptionError {
    /// A capacity, threshold, timeout, or filter is invalid.
    #[error("invalid event subscription configuration")]
    InvalidConfiguration,
    /// The per-run subscriber bound is exhausted.
    #[error("event subscriber capacity exhausted")]
    CapacityExhausted,
    /// The owning event hub is closed.
    #[error("event hub is closed")]
    HubClosed,
}

#[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
const fn sensitivity_rank(value: Sensitivity) -> u8 {
    match value {
        Sensitivity::Public => 0,
        Sensitivity::Internal => 1,
        Sensitivity::Confidential => 2,
        Sensitivity::Secret => 3,
        Sensitivity::Credential => 4,
    }
}

#[cfg(feature = "native-tokio")]
mod native {
    use std::collections::BTreeMap;
    use std::pin::Pin;
    use std::sync::{Arc, Mutex};

    use finstack_ai_kernel::{RunEvent, RunEventClass, RunEventKind};
    use tokio::sync::mpsc::error::TrySendError;
    use tokio::sync::{mpsc, oneshot};
    use tokio::task::JoinSet;
    use tokio::time::{Instant, Sleep, sleep_until, timeout};

    use super::{
        EventBatch, EventDeliveryStats, EventHubConfig, EventLagPolicy, EventPublishError,
        EventSubscriptionCloseReason, EventSubscriptionConfig, EventSubscriptionError,
        EventSubscriptionStatus, ProgressCoalescing, RuntimeEventPublisher, SubscriberAudience,
        validate_event_sequences,
    };
    use crate::PortFuture;

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

    type SharedStatus = Arc<Mutex<SubscriptionState>>;

    /// Native bounded event-batch receiver for one run subscription.
    pub struct EventSubscription {
        receiver: mpsc::Receiver<EventBatch>,
        status: SharedStatus,
    }

    impl EventSubscription {
        /// Receive the next transport batch, or `None` after explicit closure.
        pub async fn next_batch(&mut self) -> Option<EventBatch> {
            self.receiver.recv().await
        }

        /// Snapshot the current lifecycle and cumulative delivery statistics.
        #[must_use]
        pub fn status(&self) -> EventSubscriptionStatus {
            self.status.lock().map_or_else(
                |_| EventSubscriptionStatus {
                    close_reason: Some(EventSubscriptionCloseReason::ReceiverDropped),
                    stats: EventDeliveryStats::default(),
                },
                |status| status.public,
            )
        }

        /// Close this receiver without affecting the owning run.
        pub fn close(&mut self) {
            close_status(&self.status, EventSubscriptionCloseReason::SubscriberClosed);
            self.receiver.close();
        }
    }

    impl Drop for EventSubscription {
        fn drop(&mut self) {
            close_status(&self.status, EventSubscriptionCloseReason::ReceiverDropped);
            self.receiver.close();
        }
    }

    #[derive(Clone)]
    pub(crate) struct EventHubHandle {
        sender: mpsc::Sender<HubCommand>,
    }

    impl EventHubHandle {
        pub(crate) async fn subscribe_interactive(
            &self,
            config: EventSubscriptionConfig,
        ) -> Result<EventSubscription, EventSubscriptionError> {
            self.subscribe(SubscriberAudience::Interactive, config)
                .await
        }

        pub(crate) async fn subscribe_observer(
            &self,
            config: EventSubscriptionConfig,
        ) -> Result<EventSubscription, EventSubscriptionError> {
            self.subscribe(SubscriberAudience::Observer, config).await
        }

        async fn subscribe(
            &self,
            audience: SubscriberAudience,
            config: EventSubscriptionConfig,
        ) -> Result<EventSubscription, EventSubscriptionError> {
            config.validate()?;
            let (reply, receive) = oneshot::channel();
            self.sender
                .send(HubCommand::Subscribe {
                    audience,
                    config,
                    reply,
                })
                .await
                .map_err(|_| EventSubscriptionError::HubClosed)?;
            receive
                .await
                .map_err(|_| EventSubscriptionError::HubClosed)?
        }

        pub(crate) async fn close(&self) {
            let (reply, receive) = oneshot::channel();
            if self.sender.send(HubCommand::Close { reply }).await.is_ok() {
                let _ = receive.await;
            }
        }
    }

    impl RuntimeEventPublisher for EventHubHandle {
        fn publish(&self, events: Arc<[RunEvent]>) -> PortFuture<Result<(), EventPublishError>> {
            let sender = self.sender.clone();
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
                let (reply, receive) = oneshot::channel();
                sender
                    .send(HubCommand::Publish {
                        events: sized.into(),
                        reply,
                    })
                    .await
                    .map_err(|_| EventPublishError {
                        code: "event_hub_closed",
                    })?;
                receive.await.map_err(|_| EventPublishError {
                    code: "event_hub_closed",
                })?
            })
        }
    }

    pub(crate) struct EventHubTask {
        config: EventHubConfig,
        receiver: mpsc::Receiver<HubCommand>,
    }

    pub(crate) fn event_hub(
        config: EventHubConfig,
    ) -> Result<(EventHubHandle, EventHubTask), EventSubscriptionError> {
        let config = config.validate()?;
        let (sender, receiver) = mpsc::channel(config.source_capacity);
        Ok((EventHubHandle { sender }, EventHubTask { config, receiver }))
    }

    impl EventHubTask {
        pub(crate) async fn run(mut self) {
            let mut subscribers = BTreeMap::<u64, SubscriberSink>::new();
            let mut subscriber_tasks = JoinSet::new();
            let mut next_subscriber = 0_u64;
            let mut next_sequence = None;
            while let Some(command) = self.receiver.recv().await {
                while subscriber_tasks.try_join_next().is_some() {}
                match command {
                    HubCommand::Subscribe {
                        audience,
                        config,
                        reply,
                    } => {
                        subscribers.retain(|_, subscriber| subscriber.is_active());
                        if subscribers.len() >= self.config.max_subscribers {
                            let _ = reply.send(Err(EventSubscriptionError::CapacityExhausted));
                            continue;
                        }
                        let Some(id) = next_subscriber.checked_add(1) else {
                            let _ = reply.send(Err(EventSubscriptionError::CapacityExhausted));
                            continue;
                        };
                        let subscriber_id = next_subscriber;
                        next_subscriber = id;
                        let (subscription, sink) =
                            spawn_subscriber(audience, config, &mut subscriber_tasks);
                        subscribers.insert(subscriber_id, sink);
                        let _ = reply.send(Ok(subscription));
                    }
                    HubCommand::Publish { events, reply } => {
                        let result = validate_source_order(&events, &mut next_sequence);
                        if result.is_ok() {
                            fan_out(&mut subscribers, events).await;
                        }
                        let _ = reply.send(result);
                    }
                    HubCommand::Close { reply } => {
                        for (_, subscriber) in subscribers {
                            subscriber.close(EventSubscriptionCloseReason::HubClosed);
                        }
                        while subscriber_tasks.join_next().await.is_some() {}
                        let _ = reply.send(());
                        return;
                    }
                }
            }
            for (_, subscriber) in subscribers {
                subscriber.close(EventSubscriptionCloseReason::HubClosed);
            }
            while subscriber_tasks.join_next().await.is_some() {}
        }
    }

    enum HubCommand {
        Subscribe {
            audience: SubscriberAudience,
            config: EventSubscriptionConfig,
            reply: oneshot::Sender<Result<EventSubscription, EventSubscriptionError>>,
        },
        Publish {
            events: Arc<[SizedEvent]>,
            reply: oneshot::Sender<Result<(), EventPublishError>>,
        },
        Close {
            reply: oneshot::Sender<()>,
        },
    }

    struct SubscriberSink {
        audience: SubscriberAudience,
        config: EventSubscriptionConfig,
        sender: Option<mpsc::Sender<Arc<[SizedEvent]>>>,
        status: SharedStatus,
    }

    impl SubscriberSink {
        fn is_active(&self) -> bool {
            self.status
                .lock()
                .is_ok_and(|status| status.public.close_reason.is_none())
                && self
                    .sender
                    .as_ref()
                    .is_some_and(|sender| !sender.is_closed())
        }

        fn close(mut self, reason: EventSubscriptionCloseReason) {
            close_status(&self.status, reason);
            self.sender.take();
        }
    }

    fn spawn_subscriber(
        audience: SubscriberAudience,
        config: EventSubscriptionConfig,
        tasks: &mut JoinSet<()>,
    ) -> (EventSubscription, SubscriberSink) {
        let (input_sender, input_receiver) = mpsc::channel(config.queue_capacity);
        let (batch_sender, batch_receiver) = mpsc::channel(config.queue_capacity);
        let status = Arc::new(Mutex::new(SubscriptionState::default()));
        tasks.spawn(run_subscriber(
            input_receiver,
            batch_sender,
            Arc::clone(&status),
            config.clone(),
        ));
        (
            EventSubscription {
                receiver: batch_receiver,
                status: Arc::clone(&status),
            },
            SubscriberSink {
                audience,
                config,
                sender: Some(input_sender),
                status,
            },
        )
    }

    async fn fan_out(subscribers: &mut BTreeMap<u64, SubscriberSink>, events: Arc<[SizedEvent]>) {
        let mut closed = Vec::new();
        for (id, subscriber) in subscribers.iter_mut() {
            let selected = events
                .iter()
                .filter(|item| subscriber.config.filter.matches(&item.event))
                .cloned()
                .collect::<Vec<_>>();
            if selected.is_empty() {
                continue;
            }
            let selected: Arc<[SizedEvent]> = selected.into();
            let durable = selected
                .iter()
                .any(|item| item.event.class() == RunEventClass::DurableDerived);
            let Some(sender) = subscriber.sender.as_ref() else {
                closed.push(*id);
                continue;
            };
            let result = match (subscriber.audience, subscriber.config.lag_policy) {
                (SubscriberAudience::Observer, _) => match sender.try_send(selected) {
                    Ok(()) => Ok(()),
                    Err(TrySendError::Full(events)) if !durable => {
                        record_dropped(&subscriber.status, events.len());
                        Ok(())
                    }
                    Err(TrySendError::Full(_)) => Err(EventSubscriptionCloseReason::MissedDurable),
                    Err(TrySendError::Closed(_)) => {
                        Err(EventSubscriptionCloseReason::ReceiverDropped)
                    }
                },
                (_, EventLagPolicy::BlockBounded { timeout: wait }) => {
                    timeout(wait, sender.send(selected))
                        .await
                        .map_err(|_| {
                            if durable {
                                EventSubscriptionCloseReason::MissedDurable
                            } else {
                                EventSubscriptionCloseReason::Lagged
                            }
                        })
                        .and_then(|result| {
                            result.map_err(|_| EventSubscriptionCloseReason::ReceiverDropped)
                        })
                }
                (_, EventLagPolicy::DropProgress { durable_timeout }) if durable => {
                    timeout(durable_timeout, sender.send(selected))
                        .await
                        .map_err(|_| EventSubscriptionCloseReason::MissedDurable)
                        .and_then(|result| {
                            result.map_err(|_| EventSubscriptionCloseReason::ReceiverDropped)
                        })
                }
                (_, EventLagPolicy::DropProgress { .. }) => match sender.try_send(selected) {
                    Ok(()) => Ok(()),
                    Err(TrySendError::Full(events)) => {
                        record_dropped(&subscriber.status, events.len());
                        Ok(())
                    }
                    Err(TrySendError::Closed(_)) => {
                        Err(EventSubscriptionCloseReason::ReceiverDropped)
                    }
                },
                (_, EventLagPolicy::Disconnect) => match sender.try_send(selected) {
                    Ok(()) => Ok(()),
                    Err(TrySendError::Full(_)) if durable => {
                        Err(EventSubscriptionCloseReason::MissedDurable)
                    }
                    Err(TrySendError::Full(_)) => Err(EventSubscriptionCloseReason::Lagged),
                    Err(TrySendError::Closed(_)) => {
                        Err(EventSubscriptionCloseReason::ReceiverDropped)
                    }
                },
            };
            if let Err(reason) = result {
                close_status(&subscriber.status, reason);
                subscriber.sender.take();
                closed.push(*id);
            }
        }
        for id in closed {
            subscribers.remove(&id);
        }
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

    async fn run_subscriber(
        mut receiver: mpsc::Receiver<Arc<[SizedEvent]>>,
        sender: mpsc::Sender<EventBatch>,
        status: SharedStatus,
        config: EventSubscriptionConfig,
    ) {
        let mut pending = Vec::<SizedEvent>::new();
        let mut pending_bytes = 0_usize;
        let mut deadline: Option<Pin<Box<Sleep>>> = None;
        loop {
            if pending.is_empty() {
                let Some(events) = receiver.recv().await else {
                    break;
                };
                if !append_and_maybe_flush(
                    &mut pending,
                    &mut pending_bytes,
                    &mut deadline,
                    events,
                    &sender,
                    &status,
                    &config,
                )
                .await
                {
                    return;
                }
                continue;
            }

            let Some(sleeper) = deadline.as_mut() else {
                close_status(&status, EventSubscriptionCloseReason::ReceiverDropped);
                return;
            };
            tokio::select! {
                events = receiver.recv() => {
                    let Some(events) = events else { break; };
                    if !append_and_maybe_flush(
                        &mut pending,
                        &mut pending_bytes,
                        &mut deadline,
                        events,
                        &sender,
                        &status,
                        &config,
                    ).await {
                        return;
                    }
                }
                () = sleeper.as_mut() => {
                    if !flush_pending(
                        &mut pending,
                        &mut pending_bytes,
                        &mut deadline,
                        &sender,
                        &status,
                        config.lag_policy,
                    ).await {
                        return;
                    }
                }
            }
        }
        let _ = flush_pending(
            &mut pending,
            &mut pending_bytes,
            &mut deadline,
            &sender,
            &status,
            config.lag_policy,
        )
        .await;
        close_status(&status, EventSubscriptionCloseReason::HubClosed);
    }

    async fn append_and_maybe_flush(
        pending: &mut Vec<SizedEvent>,
        pending_bytes: &mut usize,
        deadline: &mut Option<Pin<Box<Sleep>>>,
        events: Arc<[SizedEvent]>,
        sender: &mpsc::Sender<EventBatch>,
        status: &SharedStatus,
        config: &EventSubscriptionConfig,
    ) -> bool {
        for item in events.iter().cloned() {
            let durable = item.event.class() == RunEventClass::DurableDerived;
            let oversized = item.bytes > config.batching.flush_bytes;
            if (durable || oversized)
                && !pending.is_empty()
                && !flush_pending(
                    pending,
                    pending_bytes,
                    deadline,
                    sender,
                    status,
                    config.lag_policy,
                )
                .await
            {
                return false;
            }
            if pending.is_empty() {
                *deadline = Some(Box::pin(sleep_until(
                    Instant::now() + config.batching.flush_interval,
                )));
            }
            *pending_bytes = pending_bytes.saturating_add(item.bytes);
            let terminal = is_terminal(item.event.kind());
            pending.push(item);
            let immediate_progress =
                config.progress_coalescing == ProgressCoalescing::Disabled && !durable;
            let should_flush = immediate_progress
                || durable
                || oversized
                || terminal
                || pending.len() >= config.batching.flush_count
                || *pending_bytes >= config.batching.flush_bytes;
            if should_flush
                && !flush_pending(
                    pending,
                    pending_bytes,
                    deadline,
                    sender,
                    status,
                    config.lag_policy,
                )
                .await
            {
                return false;
            }
        }
        true
    }

    async fn flush_pending(
        pending: &mut Vec<SizedEvent>,
        pending_bytes: &mut usize,
        deadline: &mut Option<Pin<Box<Sleep>>>,
        sender: &mpsc::Sender<EventBatch>,
        status: &SharedStatus,
        lag_policy: EventLagPolicy,
    ) -> bool {
        if pending.is_empty() {
            *deadline = None;
            return true;
        }
        let events = pending.drain(..).map(|item| item.event).collect::<Vec<_>>();
        *pending_bytes = 0;
        *deadline = None;
        let durable = events
            .iter()
            .any(|event| event.class() == RunEventClass::DurableDerived);
        let dropped = unreported_dropped(status);
        let count = events.len();
        let last_sequence = events.last().map(RunEvent::transient_sequence);
        let Some(batch) = EventBatch::new(events, dropped) else {
            return true;
        };
        let delivery = match lag_policy {
            EventLagPolicy::BlockBounded { timeout: wait } => timeout(wait, sender.send(batch))
                .await
                .map_err(|_| {
                    if durable {
                        EventSubscriptionCloseReason::MissedDurable
                    } else {
                        EventSubscriptionCloseReason::Lagged
                    }
                })
                .and_then(|result| {
                    result.map_err(|_| EventSubscriptionCloseReason::ReceiverDropped)
                })
                .map(|()| true),
            EventLagPolicy::DropProgress { durable_timeout } if durable => {
                timeout(durable_timeout, sender.send(batch))
                    .await
                    .map_err(|_| EventSubscriptionCloseReason::MissedDurable)
                    .and_then(|result| {
                        result.map_err(|_| EventSubscriptionCloseReason::ReceiverDropped)
                    })
                    .map(|()| true)
            }
            EventLagPolicy::DropProgress { .. } => match sender.try_send(batch) {
                Ok(()) => Ok(true),
                Err(TrySendError::Full(_)) => {
                    record_dropped(status, count);
                    Ok(false)
                }
                Err(TrySendError::Closed(_)) => Err(EventSubscriptionCloseReason::ReceiverDropped),
            },
            EventLagPolicy::Disconnect => match sender.try_send(batch) {
                Ok(()) => Ok(true),
                Err(TrySendError::Full(_)) if durable => {
                    Err(EventSubscriptionCloseReason::MissedDurable)
                }
                Err(TrySendError::Full(_)) => Err(EventSubscriptionCloseReason::Lagged),
                Err(TrySendError::Closed(_)) => Err(EventSubscriptionCloseReason::ReceiverDropped),
            },
        };
        match delivery {
            Ok(true) => {
                record_delivery(status, count, last_sequence);
                clear_reported_dropped(status, dropped);
                true
            }
            Ok(false) => true,
            Err(reason) => {
                close_status(status, reason);
                false
            }
        }
    }

    const fn is_terminal(kind: RunEventKind) -> bool {
        matches!(
            kind,
            RunEventKind::RunCompleted | RunEventKind::RunFailed | RunEventKind::RunCancelled
        )
    }

    fn record_delivery(status: &SharedStatus, count: usize, last_sequence: Option<u64>) {
        if let Ok(mut value) = status.lock() {
            value.public.stats.delivered_batches =
                value.public.stats.delivered_batches.saturating_add(1);
            value.public.stats.delivered_events = value
                .public
                .stats
                .delivered_events
                .saturating_add(u64::try_from(count).unwrap_or(u64::MAX));
            value.public.stats.last_delivered_sequence = last_sequence;
        }
    }

    fn record_dropped(status: &SharedStatus, count: usize) {
        if let Ok(mut value) = status.lock() {
            let dropped = u64::try_from(count).unwrap_or(u64::MAX);
            value.public.stats.dropped_progress =
                value.public.stats.dropped_progress.saturating_add(dropped);
            value.unreported_dropped = value.unreported_dropped.saturating_add(dropped);
        }
    }

    fn unreported_dropped(status: &SharedStatus) -> u64 {
        status.lock().map_or(0, |value| value.unreported_dropped)
    }

    fn clear_reported_dropped(status: &SharedStatus, reported: u64) {
        if let Ok(mut value) = status.lock() {
            value.unreported_dropped = value.unreported_dropped.saturating_sub(reported);
        }
    }

    fn close_status(status: &SharedStatus, reason: EventSubscriptionCloseReason) {
        if let Ok(mut value) = status.lock()
            && value.public.close_reason.is_none()
        {
            value.public.close_reason = Some(reason);
        }
    }

    #[cfg(test)]
    mod tests {
        use std::sync::Arc;
        use std::time::Duration;

        use finstack_ai_kernel::{
            EffectTag, EventTag, Id, IdTag, LaneTag, ModelRequestTag, ModelTextDelta,
            QueueDepthWarning, RUN_EVENT_KIND_VERSION, RUN_EVENT_SCHEMA_VERSION, RunEventBody,
            RunEventKind, RunTag, Sensitivity, SessionTag, Timestamp, TurnTag,
        };

        use super::*;
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

        async fn wait_for_reason(
            subscription: &EventSubscription,
            reason: EventSubscriptionCloseReason,
        ) {
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
            config.batching.flush_bytes =
                serde_json::to_vec(&progress(0)).expect("serialize").len() + 1;
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
                observer.status().close_reason.is_some()
                    || observer.status().stats.dropped_progress > 0
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
    }
}

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct EventPublishError {
    pub(crate) code: &'static str,
}

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
fn validate_event_sequences(
    sequences: impl IntoIterator<Item = u64>,
    next_sequence: &mut Option<u64>,
) -> Result<(), EventPublishError> {
    let mut sequences = sequences.into_iter();
    let Some(first) = sequences.next() else {
        return Ok(());
    };
    let mut expected = next_sequence.unwrap_or(first);
    for sequence in std::iter::once(first).chain(sequences) {
        if sequence != expected {
            return Err(EventPublishError {
                code: "event_sequence_mismatch",
            });
        }
        expected = expected.checked_add(1).ok_or(EventPublishError {
            code: "event_sequence_exhausted",
        })?;
    }
    *next_sequence = Some(expected);
    Ok(())
}

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
fn json_byte_len(event: &RunEvent) -> Result<usize, EventPublishError> {
    let mut writer = CountingJsonWriter::default();
    serde_json::to_writer(&mut writer, event).map_err(|_| EventPublishError {
        code: "event_serialization_failed",
    })?;
    Ok(writer.len)
}

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
#[derive(Default)]
struct CountingJsonWriter {
    len: usize,
}

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
impl std::io::Write for CountingJsonWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.len = self.len.saturating_add(buf.len());
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(all(test, any(feature = "native-tokio", feature = "wasm-host")))]
mod sequence_tests {
    use super::{EventPublishError, validate_event_sequences};

    struct SequenceCase {
        name: &'static str,
        sequences: &'static [u64],
        start: Option<u64>,
        expected: Result<Option<u64>, &'static str>,
    }

    #[test]
    fn source_sequence_table_matches_native_codes() {
        let cases = [
            SequenceCase {
                name: "empty keeps cursor",
                sequences: &[],
                start: Some(4),
                expected: Ok(Some(4)),
            },
            SequenceCase {
                name: "empty starts unset",
                sequences: &[],
                start: None,
                expected: Ok(None),
            },
            SequenceCase {
                name: "first batch sets cursor",
                sequences: &[3, 4],
                start: None,
                expected: Ok(Some(5)),
            },
            SequenceCase {
                name: "continues from cursor",
                sequences: &[5],
                start: Some(5),
                expected: Ok(Some(6)),
            },
            SequenceCase {
                name: "gap is mismatch",
                sequences: &[2],
                start: Some(1),
                expected: Err("event_sequence_mismatch"),
            },
            SequenceCase {
                name: "internal gap is mismatch",
                sequences: &[1, 3],
                start: None,
                expected: Err("event_sequence_mismatch"),
            },
            SequenceCase {
                name: "u64 overflow is exhausted",
                sequences: &[u64::MAX],
                start: Some(u64::MAX),
                expected: Err("event_sequence_exhausted"),
            },
        ];
        for case in cases {
            let mut next = case.start;
            let result = validate_event_sequences(case.sequences.iter().copied(), &mut next);
            match case.expected {
                Ok(cursor) => {
                    assert_eq!(result, Ok(()), "{}", case.name);
                    assert_eq!(next, cursor, "{}", case.name);
                }
                Err(code) => {
                    assert_eq!(result, Err(EventPublishError { code }), "{}", case.name);
                }
            }
        }
    }
}

#[cfg(any(feature = "native-tokio", feature = "wasm-host"))]
pub(crate) trait RuntimeEventPublisher: crate::PortObject {
    fn publish(&self, events: Arc<[RunEvent]>) -> crate::PortFuture<Result<(), EventPublishError>>;
}

#[cfg(feature = "native-tokio")]
pub use native::EventSubscription;

#[cfg(feature = "native-tokio")]
pub(crate) use native::{EventHubHandle, event_hub};

#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
pub use host::EventSubscription;

#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
pub(crate) use host::{EventHubHandle, event_hub};

#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
mod host {
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
}
