use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio::task::JoinSet;

use super::batching::run_subscriber;
use super::dispatch::fan_out;
use super::handle::{EventHubHandle, HubCommand};
use super::subscription::{EventSubscription, SizedEvent, SubscriberSink, SubscriptionState};
use super::{
    EventHubConfig, EventPublishError, EventSubscriptionCloseReason, EventSubscriptionConfig,
    EventSubscriptionError, SubscriberAudience, validate_event_sequences,
};

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

fn validate_source_order(
    events: &[SizedEvent],
    next_sequence: &mut Option<u64>,
) -> Result<(), EventPublishError> {
    validate_event_sequences(
        events.iter().map(|item| item.event.transient_sequence()),
        next_sequence,
    )
}
