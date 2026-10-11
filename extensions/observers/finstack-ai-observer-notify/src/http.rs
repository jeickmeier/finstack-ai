//! Shared outbound JSON POST helper. Sink URLs are bearer credentials.

use std::sync::Arc;

use finstack_ai_net_guard::{NetGuardError, UrlPolicy, validate_fixed_endpoint};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::model::SecretString;
use reqwest::redirect::Policy;

use crate::{NotifyObserverError, SinkError};

/// Sink URLs are operator-fixed endpoints: `https` to any host, plaintext
/// `http` only to loopback (local test receivers), and a non-443 `https`
/// port is allowed for self-hosted receivers.
const SINK_URL_POLICY: UrlPolicy = UrlPolicy {
    allow_loopback_http: true,
    allow_nonstandard_https_port: true,
};

/// Map a net-guard rejection onto the sink's stable configuration reasons.
/// Pre-existing reasons are preserved; every other `InvalidUrl` reason is
/// net-guard's own stable, non-secret label.
fn sink_url_error(error: &NetGuardError) -> NotifyObserverError {
    let reason = match *error {
        NetGuardError::InvalidUrl {
            reason: "url_malformed",
        } => "invalid_sink_url",
        NetGuardError::InvalidUrl {
            reason: "scheme_not_allowed",
        } => "invalid_sink_url_scheme",
        NetGuardError::InvalidUrl { reason } => reason,
        _ => "invalid_sink_url",
    };
    NotifyObserverError::Configuration { reason }
}

/// POSTs JSON bodies to one fixed, secret URL. No redirects are followed.
pub(crate) struct JsonPoster {
    client: reqwest::Client,
    url: SecretString,
}

impl JsonPoster {
    /// Vet the URL as a fixed endpoint and build the HTTP client.
    ///
    /// The URL must be `https`, or `http` to a loopback host; userinfo,
    /// query, and fragment components are rejected because the sink URL is
    /// a bearer credential that is sent verbatim on every request.
    ///
    /// No client-level timeout is set: the observer's `DeliveryPolicy` is the
    /// single owner of the per-request timeout.
    pub(crate) fn try_new(url: SecretString) -> Result<Self, NotifyObserverError> {
        validate_fixed_endpoint(url.expose(), &SINK_URL_POLICY)
            .map_err(|error| sink_url_error(&error))?;
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
