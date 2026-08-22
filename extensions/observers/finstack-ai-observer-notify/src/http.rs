//! Shared outbound JSON POST helper. Sink URLs are bearer credentials.

use std::sync::Arc;

use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::model::SecretString;
use reqwest::redirect::Policy;

use crate::{NotifyObserverError, SinkError};

/// POSTs JSON bodies to one fixed, secret URL. No redirects are followed.
pub(crate) struct JsonPoster {
    client: reqwest::Client,
    url: SecretString,
}

impl JsonPoster {
    /// Validate the URL and build the HTTP client.
    ///
    /// No client-level timeout is set: the observer's `DeliveryPolicy` is the
    /// single owner of the per-request timeout.
    pub(crate) fn try_new(url: SecretString) -> Result<Self, NotifyObserverError> {
        let parsed =
            reqwest::Url::parse(url.expose()).map_err(|_| NotifyObserverError::Configuration {
                reason: "invalid_sink_url",
            })?;
        if parsed.scheme() != "https" && parsed.scheme() != "http" {
            return Err(NotifyObserverError::Configuration {
                reason: "invalid_sink_url_scheme",
            });
        }
        let client = reqwest::Client::builder()
            .redirect(Policy::none())
            .build()
            .map_err(|_| NotifyObserverError::Configuration {
                reason: "http_client_build_failed",
            })?;
        Ok(Self { client, url })
    }

    /// POST one JSON body. Errors carry stable reasons only — never the URL
    /// or any response content.
    pub(crate) fn post(&self, body: Vec<u8>) -> PortFuture<Result<(), SinkError>> {
        let client = self.client.clone();
        let url: Arc<str> = Arc::from(self.url.expose());
        Box::pin(async move {
            let response = client
                .post(&*url)
                .header("content-type", "application/json")
                .body(body)
                .send()
                .await
                .map_err(|_| SinkError::Unavailable {
                    reason: "http_request_failed",
                })?;
            if response.status().is_success() {
                Ok(())
            } else {
                Err(SinkError::Unavailable {
                    reason: "http_status_error",
                })
            }
        })
    }
}
