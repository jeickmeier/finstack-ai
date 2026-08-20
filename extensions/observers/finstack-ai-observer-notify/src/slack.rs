//! Slack incoming-webhook sink.

use core::fmt;
use std::fmt::Write as _;
use std::time::Duration;

use finstack_ai_runtime::{PortFuture, SecretString};

use crate::http::JsonPoster;
use crate::{
    AssigneeLabel, InteractionEventKind, InteractionNotification, NotificationDetail,
    NotificationSink, NotifyObserverError, SinkError,
};

/// Render one notification as a single-line Slack message.
///
/// Uses only whitelisted notification fields; pure and deterministic.
#[must_use]
pub fn slack_text(notification: &InteractionNotification) -> String {
    let mut text = String::new();
    let _ = match notification.event {
        InteractionEventKind::Requested => write!(text, "Interaction requested"),
        InteractionEventKind::Resolved => write!(text, "Interaction resolved"),
        InteractionEventKind::Expired => write!(text, "Interaction expired"),
        InteractionEventKind::Cancelled => write!(text, "Interaction cancelled"),
    };
    let _ = write!(
        text,
        " — interaction {} (session {}, run {})",
        notification.interaction_id, notification.session_id, notification.run_id
    );
    match &notification.detail {
        NotificationDetail::Requested {
            kind,
            assignee,
            expires_at,
            delegatable,
        } => {
            let _ = write!(text, " | kind: {kind}");
            match assignee {
                Some(AssigneeLabel::Principal(principal)) => {
                    let _ = write!(
                        text,
                        " | assignee: {}/{}",
                        principal.issuer, principal.subject
                    );
                }
                Some(AssigneeLabel::Role(role)) => {
                    let _ = write!(text, " | role: {role}");
                }
                Some(AssigneeLabel::Queue(queue)) => {
                    let _ = write!(text, " | queue: {queue}");
                }
                None => {}
            }
            if expires_at.is_some() {
                let _ = write!(text, " | expires");
            }
            if *delegatable {
                let _ = write!(text, " | delegatable");
            }
        }
        NotificationDetail::Resolved {
            resolution_id,
            principal,
            comment,
        } => {
            let _ = write!(
                text,
                " | resolved by {}/{} ({resolution_id})",
                principal.issuer, principal.subject
            );
            if let Some(comment) = comment {
                let _ = write!(text, " | comment: {comment}");
            }
        }
        NotificationDetail::Expired { .. } => {}
        NotificationDetail::Cancelled { principal, reason } => {
            if let Some(principal) = principal {
                let _ = write!(text, " | by {}/{}", principal.issuer, principal.subject);
            }
            if let Some(reason) = reason {
                let _ = write!(text, " ({reason})");
            }
        }
    }
    text
}

/// Slack incoming-webhook sink.
///
/// The webhook URL is a bearer credential: stored as [`SecretString`],
/// `Debug` renders it redacted, and it never appears in errors.
pub struct SlackSink {
    poster: JsonPoster,
}

impl SlackSink {
    /// Construct a Slack sink.
    ///
    /// # Errors
    ///
    /// Rejects invalid URLs and HTTP-client build failures.
    pub fn try_new(
        webhook_url: SecretString,
        request_timeout: Duration,
    ) -> Result<Self, NotifyObserverError> {
        Ok(Self {
            poster: JsonPoster::try_new(webhook_url, request_timeout)?,
        })
    }
}

impl fmt::Debug for SlackSink {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SlackSink")
            .field("webhook_url", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl NotificationSink for SlackSink {
    fn name(&self) -> &'static str {
        "slack"
    }

    fn deliver(&self, notification: InteractionNotification) -> PortFuture<Result<(), SinkError>> {
        let payload = serde_json::json!({ "text": slack_text(&notification) });
        match serde_json::to_vec(&payload) {
            Ok(body) => self.poster.post(body),
            Err(_) => Box::pin(async {
                Err(SinkError::Unavailable {
                    reason: "serialize_failed",
                })
            }),
        }
    }
}
