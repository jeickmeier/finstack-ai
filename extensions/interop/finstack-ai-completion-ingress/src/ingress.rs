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
