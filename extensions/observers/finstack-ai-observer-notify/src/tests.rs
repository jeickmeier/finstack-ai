use std::sync::Arc;

use finstack_ai_runtime::{Observer, ObserverBackpressure, ObserverPayloadMode};
use finstack_ai_test::check_observer_conformance;

use super::{DeliveryPolicy, NotifyObserver};

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
