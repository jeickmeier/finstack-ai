use std::sync::{Arc, Mutex};

use finstack_ai_kernel::RunEvent;
use tokio::sync::mpsc;

use super::{
    EventBatch, EventDeliveryStats, EventSubscriptionCloseReason, EventSubscriptionConfig,
    EventSubscriptionStatus, SubscriberAudience,
};

#[derive(Debug, Clone)]
pub(super) struct SizedEvent {
    pub(super) event: RunEvent,
    pub(super) bytes: usize,
}

#[derive(Debug, Default)]
pub(super) struct SubscriptionState {
    public: EventSubscriptionStatus,
    unreported_dropped: u64,
}

pub(super) type SharedStatus = Arc<Mutex<SubscriptionState>>;

/// Native bounded event-batch receiver for one run subscription.
pub struct EventSubscription {
    pub(super) receiver: mpsc::Receiver<EventBatch>,
    pub(super) status: SharedStatus,
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

pub(super) struct SubscriberSink {
    pub(super) audience: SubscriberAudience,
    pub(super) config: EventSubscriptionConfig,
    pub(super) sender: Option<mpsc::Sender<Arc<[SizedEvent]>>>,
    pub(super) status: SharedStatus,
}

impl SubscriberSink {
    pub(super) fn is_active(&self) -> bool {
        self.status
            .lock()
            .is_ok_and(|status| status.public.close_reason.is_none())
            && self
                .sender
                .as_ref()
                .is_some_and(|sender| !sender.is_closed())
    }

    pub(super) fn close(mut self, reason: EventSubscriptionCloseReason) {
        close_status(&self.status, reason);
        self.sender.take();
    }
}

pub(super) fn record_delivery(status: &SharedStatus, count: usize, last_sequence: Option<u64>) {
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

pub(super) fn record_dropped(status: &SharedStatus, count: usize) {
    if let Ok(mut value) = status.lock() {
        let dropped = u64::try_from(count).unwrap_or(u64::MAX);
        value.public.stats.dropped_progress =
            value.public.stats.dropped_progress.saturating_add(dropped);
        value.unreported_dropped = value.unreported_dropped.saturating_add(dropped);
    }
}

pub(super) fn unreported_dropped(status: &SharedStatus) -> u64 {
    status.lock().map_or(0, |value| value.unreported_dropped)
}

pub(super) fn clear_reported_dropped(status: &SharedStatus, reported: u64) {
    if let Ok(mut value) = status.lock() {
        value.unreported_dropped = value.unreported_dropped.saturating_sub(reported);
    }
}

pub(super) fn close_status(status: &SharedStatus, reason: EventSubscriptionCloseReason) {
    if let Ok(mut value) = status.lock()
        && value.public.close_reason.is_none()
    {
        value.public.close_reason = Some(reason);
    }
}
