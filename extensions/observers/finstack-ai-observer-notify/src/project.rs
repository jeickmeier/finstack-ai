//! Structural redaction: `RunEvent` -> whitelisted `InteractionNotification`.

use std::sync::Arc;

use finstack_ai_kernel::{AssigneeHint, InteractionKind, PrincipalRef, RunEvent, RunEventBody};

use crate::{
    AssigneeLabel, InteractionEventKind, InteractionNotification, NotificationDetail,
    PrincipalLabel,
};

fn principal_label(principal: &PrincipalRef) -> PrincipalLabel {
    PrincipalLabel {
        issuer: Arc::from(principal.issuer()),
        subject: Arc::from(principal.subject()),
    }
}

fn kind_label(kind: &InteractionKind) -> Arc<str> {
    match kind {
        InteractionKind::Approval => Arc::from("approval"),
        InteractionKind::Choice => Arc::from("choice"),
        InteractionKind::Form => Arc::from("form"),
        InteractionKind::FreeText => Arc::from("free_text"),
        InteractionKind::Review => Arc::from("review"),
        InteractionKind::Correction => Arc::from("correction"),
        InteractionKind::Custom { name } => Arc::clone(name),
    }
}

fn assignee_label(hint: &AssigneeHint) -> AssigneeLabel {
    match hint {
        AssigneeHint::Principal(principal) => AssigneeLabel::Principal(principal_label(principal)),
        AssigneeHint::Role(role) => AssigneeLabel::Role(Arc::clone(role)),
        AssigneeHint::Queue(queue) => AssigneeLabel::Queue(Arc::clone(queue)),
    }
}

/// Project one run event into a redacted notification.
///
/// Non-interaction events map to `None`. The projection is a closed
/// whitelist: prompt content, response schemas, resolution responses,
/// authorization evidence, digests, policy identity, and metadata are
/// never copied.
#[must_use]
pub fn project(event: &RunEvent) -> Option<InteractionNotification> {
    let (event_kind, interaction_id, detail) = match event.body() {
        RunEventBody::InteractionRequested(request) => (
            InteractionEventKind::Requested,
            request.interaction_id(),
            NotificationDetail::Requested {
                kind: kind_label(request.kind()),
                assignee: request.assignee_hint().map(assignee_label),
                expires_at: request.expires_at(),
                delegatable: request.delegatable(),
            },
        ),
        RunEventBody::InteractionResolved(resolution) => (
            InteractionEventKind::Resolved,
            resolution.interaction_id(),
            NotificationDetail::Resolved {
                resolution_id: Arc::from(resolution.resolution_id()),
                principal: principal_label(resolution.principal()),
                comment: resolution.comment().map(Arc::from),
            },
        ),
        RunEventBody::InteractionExpired(expired) => (
            InteractionEventKind::Expired,
            expired.interaction_id,
            NotificationDetail::Expired {
                expired_at: expired.expired_at,
            },
        ),
        RunEventBody::InteractionCancelled(cancelled) => (
            InteractionEventKind::Cancelled,
            cancelled.interaction_id(),
            NotificationDetail::Cancelled {
                principal: cancelled.principal().map(principal_label),
                reason: cancelled.reason().map(Arc::from),
            },
        ),
        _ => return None,
    };
    Some(InteractionNotification {
        event: event_kind,
        interaction_id,
        session_id: event.session_id(),
        run_id: event.run_id(),
        timestamp: event.timestamp(),
        detail,
    })
}
