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
    if let InteractionKind::Custom { name } = kind {
        return Arc::clone(name);
    }
    // Derive the label from the kernel's own serde wire mapping so the two
    // can never drift.
    match serde_json::to_value(kind) {
        Ok(value) => match value.get("kind").and_then(serde_json::Value::as_str) {
            Some(label) => Arc::from(label),
            None => Arc::from("unknown"),
        },
        Err(_) => Arc::from("unknown"),
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
        // Exhaustive by name so the compiler forces a projection decision for
        // every new kernel event variant.
        RunEventBody::RunAccepted(_)
        | RunEventBody::EffectRequested(_)
        | RunEventBody::EffectDeferred(_)
        | RunEventBody::EffectCompleted(_)
        | RunEventBody::EffectFailed(_)
        | RunEventBody::EffectCancelled(_)
        | RunEventBody::MessageFinalized { .. }
        | RunEventBody::ToolSettled { .. }
        | RunEventBody::LimitReached { .. }
        | RunEventBody::RunSuspended { .. }
        | RunEventBody::RunCompleted { .. }
        | RunEventBody::RunFailed { .. }
        | RunEventBody::RunCancelled { .. }
        | RunEventBody::ModelTextDelta(_)
        | RunEventBody::ReasoningDelta(_)
        | RunEventBody::ToolProgress(_)
        | RunEventBody::QueueDepthWarning(_)
        | RunEventBody::ProviderHeartbeat(_) => return None,
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
