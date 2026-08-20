//! Redacted audit events for pre-router ingress failures.
//!
//! Mirrors the runtime's derivation (driver/ingress/shared.rs): the event id
//! is the hex of a domain-separated digest over (token digest, body digest,
//! `submitted_at`, `reason_code`), so identical replayed garbage produces one
//! idempotent audit event.

use finstack_ai_kernel::{Digest, PrincipalRef, Timestamp};
use finstack_ai_runtime::{SecurityAuditCategory, SecurityAuditEvent, SecurityAuditGate};

use crate::ingress::IngressError;

/// Domain separator for digesting the raw callback-token bytes.
pub(crate) const TOKEN_DIGEST_DOMAIN: &str = "completion-ingress-token";
/// Domain separator for digesting the raw request-body bytes.
pub(crate) const BODY_DIGEST_DOMAIN: &str = "completion-ingress-body";
const SECURITY_AUDIT_EVENT_DOMAIN: &str = "security-audit-event";

fn raw_digest(domain: &'static str, bytes: &[u8]) -> Option<Digest> {
    Digest::domain_separated(domain, 1, bytes).ok()
}

/// Derive a redacted, idempotent audit event for an ingress failure.
///
/// Returns `None` on any internal failure (digesting, canonicalization, or
/// event construction); the caller rejects either way.
#[allow(clippy::too_many_arguments)]
pub(crate) fn ingress_audit_event(
    category: SecurityAuditCategory,
    reason_code: &'static str,
    principal: Option<PrincipalRef>,
    tenant_scope: Option<&str>,
    token: &[u8],
    body: &[u8],
    submitted_at: Timestamp,
) -> Option<SecurityAuditEvent> {
    let token_digest = raw_digest(TOKEN_DIGEST_DOMAIN, token)?;
    let body_digest = raw_digest(BODY_DIGEST_DOMAIN, body)?;
    let id_bytes =
        serde_json_canonicalizer::to_vec(&(token_digest, body_digest, submitted_at, reason_code))
            .ok()?;
    let id_digest = raw_digest(SECURITY_AUDIT_EVENT_DOMAIN, &id_bytes)?;
    SecurityAuditEvent::try_new(
        id_digest.to_hex(),
        submitted_at,
        principal,
        tenant_scope,
        category,
        reason_code,
        Some(token_digest),
        Some(body_digest),
    )
    .ok()
}

/// Record evidence when possible, then reject either way (fail closed).
pub(crate) async fn audit_and_reject(
    gate: &SecurityAuditGate,
    event: Option<SecurityAuditEvent>,
) -> IngressError {
    if let Some(event) = event {
        // A failed audit write must not produce a distinct caller-visible
        // signal; the response is the same opaque rejection.
        let _ = gate.record(event).await;
    }
    IngressError::Rejected
}

#[cfg(test)]
mod tests {
    use super::*;
    use finstack_ai_kernel::Timestamp;
    use finstack_ai_runtime::SecurityAuditCategory;

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_unix_ms(ms).expect("timestamp")
    }

    #[test]
    fn identical_inputs_derive_identical_event_ids() {
        let first = ingress_audit_event(
            SecurityAuditCategory::MalformedToken,
            "malformed_token",
            None,
            None,
            b"garbage-token",
            b"{}",
            ts(5),
        )
        .expect("event");
        let second = ingress_audit_event(
            SecurityAuditCategory::MalformedToken,
            "malformed_token",
            None,
            None,
            b"garbage-token",
            b"{}",
            ts(5),
        )
        .expect("event");
        assert_eq!(first.event_id(), second.event_id());
    }

    #[test]
    fn different_reason_or_bytes_change_the_event_id() {
        let base = ingress_audit_event(
            SecurityAuditCategory::MalformedToken,
            "malformed_token",
            None,
            None,
            b"garbage-token",
            b"{}",
            ts(5),
        )
        .expect("event");
        let other_reason = ingress_audit_event(
            SecurityAuditCategory::AuthenticationFailure,
            "bad_signature",
            None,
            None,
            b"garbage-token",
            b"{}",
            ts(5),
        )
        .expect("event");
        let other_token = ingress_audit_event(
            SecurityAuditCategory::MalformedToken,
            "malformed_token",
            None,
            None,
            b"other-token",
            b"{}",
            ts(5),
        )
        .expect("event");
        assert_ne!(base.event_id(), other_reason.event_id());
        assert_ne!(base.event_id(), other_token.event_id());
    }
}
