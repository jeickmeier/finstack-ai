//! Completion ingress service.

use std::sync::Arc;

use finstack_ai_kernel::{AuthorizationEvidence, EffectId, OperationLocator, PrincipalRef, Timestamp};
use finstack_ai_runtime::{IdempotencyHorizon, JournalStore, SecurityAuditGate};
use thiserror::Error;

use crate::config::{CompletionIngressConfig, CompletionIngressConfigError, ResolvedKeys, validated_keys};
use crate::token::{CLAIMS_VERSION, CallbackToken, Claims, KIND_EFFECT_COMPLETION, mint_token};

/// Caller-visible delivery failures.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IngressError {
    /// One non-existence-revealing response for every token/body/audit failure.
    #[error("external completion rejected")]
    Rejected,
    /// Store/commit/id-allocation failure on a known authorized target.
    #[error("external completion ingress unavailable: {reason_code}")]
    Unavailable {
        /// Stable retryable-failure reason.
        reason_code: &'static str,
    },
}

/// What a minted token authorizes: one effect on one run, until expiry.
#[derive(Clone)]
pub struct CompletionGrant {
    /// Durable target locator of the accepted run.
    pub locator: OperationLocator,
    /// Principal frozen at deferral time.
    pub principal: PrincipalRef,
    /// Authorization evidence frozen at deferral time.
    pub authorization: AuthorizationEvidence,
    /// Original deferred effect identity.
    pub effect_id: EffectId,
    /// Token expiry; must not exceed the configured idempotency horizon.
    pub expires_at: Timestamp,
}

/// Token issuance failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum MintError {
    /// Claims could not be canonically encoded or signed.
    #[error("token encoding failed")]
    Encoding,
}

/// Host-embedded ingress: mints callback tokens and delivers completions.
pub struct CompletionIngress {
    pub(crate) store: Arc<dyn JournalStore>,
    pub(crate) audit: Arc<SecurityAuditGate>,
    pub(crate) keys: ResolvedKeys,
    pub(crate) horizon: Option<IdempotencyHorizon>,
}

impl CompletionIngress {
    /// Construct with a validated signing config and a real audit gate.
    ///
    /// There is intentionally no no-op-audit constructor on this type.
    ///
    /// # Errors
    ///
    /// Returns [`CompletionIngressConfigError`] for invalid keys.
    pub fn try_new(
        store: Arc<dyn JournalStore>,
        audit: Arc<SecurityAuditGate>,
        config: CompletionIngressConfig,
    ) -> Result<Self, CompletionIngressConfigError> {
        Ok(Self {
            store,
            audit,
            keys: validated_keys(&config)?,
            horizon: None,
        })
    }

    /// Apply the application idempotency horizon to every delivery.
    #[must_use]
    pub fn with_horizon(mut self, horizon: IdempotencyHorizon) -> Self {
        self.horizon = Some(horizon);
        self
    }

    /// Mint one signed callback token for one deferred effect.
    ///
    /// # Errors
    ///
    /// Returns [`MintError::Encoding`] when signing fails.
    pub fn mint(&self, grant: &CompletionGrant) -> Result<CallbackToken, MintError> {
        let claims = Claims {
            v: CLAIMS_VERSION,
            kid: self.keys.active_id.as_ref().to_owned(),
            kind: KIND_EFFECT_COMPLETION.to_owned(),
            locator: grant.locator.clone(),
            principal: grant.principal.clone(),
            authorization: grant.authorization.clone(),
            effect_id: grant.effect_id,
            expires_at: grant.expires_at,
        };
        mint_token(&self.keys, &claims).map_err(|_| MintError::Encoding)
    }

    /// Deliver one external completion: verify the token, decode the body,
    /// and route the authenticated command.
    ///
    /// # Errors
    ///
    /// Returns the opaque [`IngressError::Rejected`] for every token, body,
    /// or audit failure without revealing target existence, and
    /// [`IngressError::Unavailable`] for store/commit failures on a known
    /// authorized target.
    pub async fn deliver(
        &self,
        token: &str,
        body: &[u8],
        submitted_at: Timestamp,
    ) -> Result<finstack_ai_runtime::ExternalRouteOutcome, IngressError> {
        use finstack_ai_runtime::SecurityAuditCategory;

        let claims = match crate::token::verify_token(&self.keys, token, submitted_at) {
            Ok(claims) => claims,
            Err(failure) => {
                let (category, reason) = match failure {
                    crate::token::VerifyFailure::Malformed => {
                        (SecurityAuditCategory::MalformedToken, "malformed_token")
                    }
                    crate::token::VerifyFailure::UnknownKey => {
                        (SecurityAuditCategory::AuthenticationFailure, "unknown_key")
                    }
                    crate::token::VerifyFailure::BadSignature => {
                        (SecurityAuditCategory::AuthenticationFailure, "bad_signature")
                    }
                    crate::token::VerifyFailure::Expired => {
                        (SecurityAuditCategory::AuthenticationFailure, "expired_token")
                    }
                };
                let event = crate::audit::ingress_audit_event(
                    category,
                    reason,
                    None,
                    None,
                    token.as_bytes(),
                    body,
                    submitted_at,
                );
                return Err(crate::audit::audit_and_reject(&self.audit, event).await);
            }
        };

        let reject_body = |reason: &'static str, claims: &crate::token::Claims| {
            crate::audit::ingress_audit_event(
                SecurityAuditCategory::MalformedToken,
                reason,
                Some(claims.principal.clone()),
                Some(claims.locator.tenant_scope.as_ref()),
                token.as_bytes(),
                body,
                submitted_at,
            )
        };

        if body.len() > MAX_BODY_BYTES {
            let event = reject_body("oversize_body", &claims);
            return Err(crate::audit::audit_and_reject(&self.audit, event).await);
        }
        let decoded: DeliveryBody = match serde_json::from_slice(body) {
            Ok(decoded) => decoded,
            Err(_) => {
                let event = reject_body("invalid_body", &claims);
                return Err(crate::audit::audit_and_reject(&self.audit, event).await);
            }
        };
        let completion_id = decoded
            .completion_id
            .unwrap_or_else(|| claims.effect_id.to_canonical_string());
        let completion = match finstack_ai_kernel::ExternalEffectCompletion::try_new(
            claims.effect_id,
            completion_id,
            decoded.outcome,
        ) {
            Ok(completion) => completion,
            Err(_) => {
                let event = reject_body("invalid_body", &claims);
                return Err(crate::audit::audit_and_reject(&self.audit, event).await);
            }
        };
        let command = match finstack_ai_kernel::ExternalEffectCompletionCommand::try_new(
            claims.locator.clone(),
            claims.principal.clone(),
            claims.authorization.clone(),
            completion,
        ) {
            Ok(command) => command,
            Err(_) => {
                let event = reject_body("invalid_body", &claims);
                return Err(crate::audit::audit_and_reject(&self.audit, event).await);
            }
        };

        let mut router = finstack_ai_runtime::ExternalCompletionRouter::new(
            Arc::clone(&self.store),
            Arc::clone(&self.audit),
        );
        if let Some(horizon) = self.horizon {
            router = router.with_horizon(horizon);
        }
        router.route(command, submitted_at).await.map_err(|error| match error {
            finstack_ai_runtime::ExternalRouteError::IngressRejected
            | finstack_ai_runtime::ExternalRouteError::InvalidNormalizedCommand => {
                IngressError::Rejected
            }
            finstack_ai_runtime::ExternalRouteError::IdAllocation => IngressError::Unavailable {
                reason_code: "id_allocation",
            },
            finstack_ai_runtime::ExternalRouteError::Runtime(_) => IngressError::Unavailable {
                reason_code: "runtime",
            },
        })
    }
}

/// Maximum accepted delivery body in bytes.
pub(crate) const MAX_BODY_BYTES: usize = 1_048_576;

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DeliveryBody {
    #[serde(default)]
    completion_id: Option<String>,
    outcome: finstack_ai_kernel::ExternalEffectOutcome,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CompletionIngressConfig;
    use crate::token::{KIND_EFFECT_COMPLETION, verify_token};
    use finstack_ai_kernel::{
        AuthorizationEvidence, EffectId, LaneId, OperationLocator, PrincipalRef, RunId, SessionId,
        Timestamp,
    };
    use finstack_ai_runtime::{
        PortFuture, SecretString, SecurityAuditError, SecurityAuditEvent, SecurityAuditGate,
        SecurityAuditHealth, SecurityAuditReceipt, SecurityAuditSink,
    };
    use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
    use std::sync::Arc;
    use std::time::Duration;

    /// In-test stand-in for the runtime's crate-private no-op audit sink:
    /// `SecurityAuditGate::enable_noop()` is `pub(crate)` to
    /// `finstack-ai-runtime` (see `driver/ingress/completion.rs`'s
    /// `ExternalCompletionRouter::trusted`), so this crate's tests build an
    /// always-healthy sink over the public `SecurityAuditGate::enable`/
    /// `SecurityAuditSink` API instead.
    struct TestNoopSink;

    impl SecurityAuditSink for TestNoopSink {
        fn record(
            &self,
            event: SecurityAuditEvent,
        ) -> PortFuture<Result<SecurityAuditReceipt, SecurityAuditError>> {
            let event_id = Arc::<str>::from(event.event_id());
            let recorded_at = event.timestamp();
            Box::pin(async move {
                Ok(SecurityAuditReceipt {
                    event_id,
                    recorded_at,
                })
            })
        }

        fn health(&self) -> PortFuture<Result<SecurityAuditHealth, SecurityAuditError>> {
            Box::pin(async { Ok(SecurityAuditHealth { ready: true }) })
        }
    }

    async fn noop_gate() -> Arc<SecurityAuditGate> {
        SecurityAuditGate::enable(Some(Arc::new(TestNoopSink)), Duration::from_millis(100))
            .await
            .expect("noop gate")
    }

    fn ts(ms: i64) -> Timestamp {
        Timestamp::from_unix_ms(ms).expect("timestamp")
    }

    fn config() -> CompletionIngressConfig {
        CompletionIngressConfig {
            key_id: "k-active".to_owned(),
            key: SecretString::try_new("a".repeat(32)).expect("secret"),
            additional_verification_keys: vec![],
        }
    }

    fn grant() -> CompletionGrant {
        CompletionGrant {
            locator: OperationLocator::try_new(
                "tenant-a",
                SessionId::parse("01234567-89ab-7cde-89ab-0123456789a1").expect("session"),
                LaneId::parse("01234567-89ab-7cde-89ab-0123456789a2").expect("lane"),
                RunId::parse("01234567-89ab-7cde-89ab-0123456789a3").expect("run"),
            )
            .expect("locator"),
            principal: PrincipalRef::try_new("issuer-a", "subject-a", Some("tenant-a"))
                .expect("principal"),
            authorization: AuthorizationEvidence::try_new("policy-v1", "decision-v1")
                .expect("auth"),
            effect_id: EffectId::parse("01234567-89ab-7cde-89ab-0123456789a4").expect("effect"),
            expires_at: ts(10_000),
        }
    }

    async fn ingress() -> CompletionIngress {
        let store = Arc::new(
            MemoryJournalStore::try_new(MemoryStoreLimits {
                sessions: 4,
                batches_per_session: 64,
                records_per_session: 256,
                snapshot_bytes: 64 * 1024,
            })
            .expect("store"),
        );
        let gate = noop_gate().await;
        CompletionIngress::try_new(store, gate, config()).expect("ingress")
    }

    /// In-test recording sink: records every audit event it receives, so
    /// pre-router rejection paths can be asserted on directly. Copied in
    /// shape from `crates/finstack-ai-test/tests/crash_prefix/helpers/mod.rs`.
    #[derive(Default)]
    struct RecordingSink {
        events: std::sync::Mutex<Vec<SecurityAuditEvent>>,
    }

    impl SecurityAuditSink for RecordingSink {
        fn record(
            &self,
            event: SecurityAuditEvent,
        ) -> PortFuture<Result<SecurityAuditReceipt, SecurityAuditError>> {
            let event_id = Arc::<str>::from(event.event_id());
            let recorded_at = event.timestamp();
            self.events.lock().expect("lock").push(event);
            Box::pin(async move {
                Ok(SecurityAuditReceipt {
                    event_id,
                    recorded_at,
                })
            })
        }

        fn health(&self) -> PortFuture<Result<SecurityAuditHealth, SecurityAuditError>> {
            Box::pin(async { Ok(SecurityAuditHealth { ready: true }) })
        }
    }

    async fn recording_ingress() -> (CompletionIngress, Arc<RecordingSink>) {
        let store = Arc::new(
            MemoryJournalStore::try_new(MemoryStoreLimits {
                sessions: 4,
                batches_per_session: 64,
                records_per_session: 256,
                snapshot_bytes: 64 * 1024,
            })
            .expect("store"),
        );
        let sink = Arc::new(RecordingSink::default());
        let gate = SecurityAuditGate::enable(
            Some(Arc::clone(&sink) as Arc<dyn SecurityAuditSink>),
            Duration::from_millis(100),
        )
        .await
        .expect("recording gate");
        let ingress = CompletionIngress::try_new(store, gate, config()).expect("ingress");
        (ingress, sink)
    }

    #[tokio::test]
    async fn garbage_token_audits_malformed_and_rejects_opaquely() {
        let (ingress, sink) = recording_ingress().await; // helper: like ingress() but real gate + RecordingSink
        let error = ingress
            .deliver("not-a-token", b"{}", ts(50))
            .await
            .expect_err("garbage");
        assert_eq!(error, IngressError::Rejected);
        let events = sink.events.lock().expect("lock");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].reason_code(), "malformed_token");
    }

    #[tokio::test]
    async fn identical_garbage_replay_is_one_audit_event() {
        let (ingress, sink) = recording_ingress().await;
        for _ in 0..2 {
            let _ = ingress.deliver("not-a-token", b"{}", ts(50)).await;
        }
        let events = sink.events.lock().expect("lock");
        // Two records with the same event_id; the gate/sink contract is
        // idempotent by event_id, so assert both share one id.
        assert!(events.iter().all(|event| event.event_id() == events[0].event_id()));
    }

    #[tokio::test]
    async fn expired_token_audits_authentication_failure() {
        let (ingress, sink) = recording_ingress().await;
        let mut expiring = grant();
        expiring.expires_at = ts(100);
        let token = ingress.mint(&expiring).expect("mint");
        let error = ingress
            .deliver(token.as_str(), b"{}", ts(100))
            .await
            .expect_err("expired");
        assert_eq!(error, IngressError::Rejected);
        let events = sink.events.lock().expect("lock");
        assert_eq!(events[0].reason_code(), "expired_token");
    }

    #[tokio::test]
    async fn oversize_and_undecodable_bodies_reject_after_auth() {
        let (ingress, sink) = recording_ingress().await;
        let token = ingress.mint(&grant()).expect("mint");
        let oversize = vec![b'x'; MAX_BODY_BYTES + 1];
        assert_eq!(
            ingress.deliver(token.as_str(), &oversize, ts(50)).await.expect_err("oversize"),
            IngressError::Rejected
        );
        assert_eq!(
            ingress
                .deliver(token.as_str(), br#"{"unexpected":true}"#, ts(50))
                .await
                .expect_err("bad body"),
            IngressError::Rejected
        );
        let events = sink.events.lock().expect("lock");
        assert!(events.iter().any(|event| event.reason_code() == "oversize_body"));
        assert!(events.iter().any(|event| event.reason_code() == "invalid_body"));
        // Post-auth events carry the authenticated principal.
        assert!(events
            .iter()
            .filter(|event| event.reason_code() == "oversize_body")
            .all(|event| event.principal().is_some()));
    }

    #[tokio::test]
    async fn mint_produces_a_verifiable_grant_bound_token() {
        let ingress = ingress().await;
        let token = ingress.mint(&grant()).expect("mint");
        let claims = verify_token(&ingress.keys, token.as_str(), ts(0)).expect("verify");
        assert_eq!(claims.kind, KIND_EFFECT_COMPLETION);
        assert_eq!(claims.effect_id, grant().effect_id);
        assert_eq!(claims.locator, grant().locator);
        assert_eq!(claims.kid, "k-active");
    }

    #[tokio::test]
    async fn try_new_rejects_invalid_config() {
        let store = Arc::new(
            MemoryJournalStore::try_new(MemoryStoreLimits {
                sessions: 1,
                batches_per_session: 8,
                records_per_session: 16,
                snapshot_bytes: 1024,
            })
            .expect("store"),
        );
        let gate = noop_gate().await;
        let bad = CompletionIngressConfig {
            key_id: "k1".to_owned(),
            key: SecretString::try_new("short").expect("secret"),
            additional_verification_keys: vec![],
        };
        assert!(CompletionIngress::try_new(store, gate, bad).is_err());
    }
}
