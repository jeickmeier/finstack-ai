use std::pin::Pin;
use std::sync::Arc;

use finstack_ai_kernel::{RunEvent, RunEventClass, RunEventKind};
use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio::time::{Instant, Sleep, sleep_until, timeout};

use super::super::{
    EventBatch, EventLagPolicy, EventSubscriptionCloseReason, EventSubscriptionConfig,
    ProgressCoalescing,
};
use super::subscription::{
    SharedStatus, SizedEvent, clear_reported_dropped, close_status, record_delivery,
    record_dropped, unreported_dropped,
};

pub(super) async fn run_subscriber(
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
            .and_then(|result| result.map_err(|_| EventSubscriptionCloseReason::ReceiverDropped))
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
