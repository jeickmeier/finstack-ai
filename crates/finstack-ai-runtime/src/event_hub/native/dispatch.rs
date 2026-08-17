use std::collections::BTreeMap;
use std::sync::Arc;

use finstack_ai_kernel::RunEventClass;
use tokio::sync::mpsc::error::TrySendError;
use tokio::time::timeout;

use super::super::{EventLagPolicy, EventSubscriptionCloseReason, SubscriberAudience};
use super::subscription::{SizedEvent, SubscriberSink, close_status, record_dropped};

pub(super) async fn fan_out(
    subscribers: &mut BTreeMap<u64, SubscriberSink>,
    events: Arc<[SizedEvent]>,
) {
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
                Err(TrySendError::Closed(_)) => Err(EventSubscriptionCloseReason::ReceiverDropped),
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
                Err(TrySendError::Closed(_)) => Err(EventSubscriptionCloseReason::ReceiverDropped),
            },
            (_, EventLagPolicy::Disconnect) => match sender.try_send(selected) {
                Ok(()) => Ok(()),
                Err(TrySendError::Full(_)) if durable => {
                    Err(EventSubscriptionCloseReason::MissedDurable)
                }
                Err(TrySendError::Full(_)) => Err(EventSubscriptionCloseReason::Lagged),
                Err(TrySendError::Closed(_)) => Err(EventSubscriptionCloseReason::ReceiverDropped),
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
