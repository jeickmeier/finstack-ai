use std::sync::Arc;

use finstack_ai_kernel::RunEvent;
use tokio::sync::{mpsc, oneshot};

use super::subscription::{EventSubscription, SizedEvent};
use super::{
    EventPublishError, EventSubscriptionConfig, EventSubscriptionError, RuntimeEventPublisher,
    SubscriberAudience,
};
use crate::ports::PortFuture;

#[derive(Clone)]
pub(crate) struct EventHubHandle {
    pub(super) sender: mpsc::Sender<HubCommand>,
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

pub(super) enum HubCommand {
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
