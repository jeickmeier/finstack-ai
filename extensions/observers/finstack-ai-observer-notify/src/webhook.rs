//! Generic JSON webhook sink.

use core::fmt;

use finstack_ai_runtime::{PortFuture, SecretString};

use crate::http::JsonPoster;
use crate::{InteractionNotification, NotificationSink, NotifyObserverError, SinkError};

/// POSTs each notification as canonical JSON to a fixed URL.
///
/// The URL is treated as a bearer credential: it is stored as
/// [`SecretString`], `Debug` renders it redacted, and it never appears in
/// errors or diagnostics.
pub struct WebhookSink {
    poster: JsonPoster,
}

impl WebhookSink {
    /// Construct a webhook sink. The observer's `DeliveryPolicy` owns the
    /// per-request timeout.
    ///
    /// # Errors
    ///
    /// Rejects a non-http(s) or unparseable URL and HTTP-client build
    /// failures.
    pub fn try_new(url: SecretString) -> Result<Self, NotifyObserverError> {
        Ok(Self {
            poster: JsonPoster::try_new(url)?,
        })
    }
}

impl fmt::Debug for WebhookSink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WebhookSink")
            .field("url", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl NotificationSink for WebhookSink {
    fn name(&self) -> &'static str {
        "webhook"
    }

    fn deliver(&self, notification: InteractionNotification) -> PortFuture<Result<(), SinkError>> {
        match serde_json::to_vec(&notification) {
            Ok(body) => self.poster.post(body),
            Err(_) => Box::pin(async {
                Err(SinkError::Unavailable {
                    reason: "serialize_failed",
                })
            }),
        }
    }
}
