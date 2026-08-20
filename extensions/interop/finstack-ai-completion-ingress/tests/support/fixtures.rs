// Shared test fixtures, `include!`-d into both the unit-test module in
// `src/ingress.rs` and the integration-test helpers in `tests/deliver.rs`,
// so trait-shape-sensitive code exists once. Unqualified names here
// (`SecurityAuditEvent` and friends, `SecretString`,
// `CompletionIngressConfig`) resolve in the including module's scope; both
// includers import them.

/// One active 32-byte signing key, no rotation keys.
pub(crate) fn config() -> CompletionIngressConfig {
    CompletionIngressConfig {
        key_id: "k-active".to_owned(),
        key: SecretString::try_new("a".repeat(32)).expect("secret"),
        additional_verification_keys: vec![],
    }
}

/// Records every audit event the gate flushes to it. Copied in shape from
/// `crates/finstack-ai-test/tests/crash_prefix/helpers/mod.rs`.
#[derive(Default)]
pub(crate) struct RecordingSink {
    pub(crate) events: std::sync::Mutex<Vec<SecurityAuditEvent>>,
}

impl SecurityAuditSink for RecordingSink {
    fn record(
        &self,
        event: SecurityAuditEvent,
    ) -> PortFuture<Result<SecurityAuditReceipt, SecurityAuditError>> {
        let event_id = std::sync::Arc::<str>::from(event.event_id());
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
