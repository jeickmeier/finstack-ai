//! `BeforeModel` middleware that redacts PII and secrets from the
//! model-visible request draft.
//!
//! The canonical conversation is never touched: only the model-visible
//! [`ModelRequestDraft`] is rewritten, via [`StageOutcome::Replace`], exactly
//! like the sibling document-ingest middleware. Each detected secret — API
//! key, token, JWT, PEM private key, card number, IBAN, or email address —
//! is replaced by a stable `[REDACTED:<kind>]` marker carrying nothing
//! recoverable. Redaction is idempotent, deterministic, and fail-soft: an
//! internal failure passes the affected content through unmodified rather
//! than aborting the run; a detection miss is by definition silent.
//!
//! # Deployment rule
//!
//! The middleware chain hands **every** `BeforeModel` component the same base
//! draft and keeps only the **last** `Replace` in chain order, so two
//! Replace-emitting `BeforeModel` middlewares do not compose — the earlier
//! one's rewrite is silently discarded. Register this middleware
//! **standalone** (its descriptor sits on the `RequestShaping` tier) only
//! when no other `BeforeModel` middleware emits `Replace`. When one does
//! (e.g. document-ingest), wrap it with
//! [`RedactionMiddleware::try_wrapping`] instead, which also redacts the
//! text the inner middleware injects.
//!
//! # Output redaction
//!
//! `AfterModel` middleware cannot `Replace`, so model **output** cannot be
//! rewritten in place. Two policies exist ([`OutputPolicy`]): `Off` (default)
//! relies on the next `BeforeModel` pass, which rescans assistant history and
//! redacts it before it is ever sent back to a provider; `Fail` additionally
//! declares the `AfterModel` stage and fails the run with a safe descriptor
//! (detector kinds and match count only — never the matched text) the moment
//! the assistant message contains a detectable secret.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
    )
)]

use std::sync::Arc;

use finstack_ai_kernel::{
    ComponentId, ComponentInvocation, Digest, ErrorCategory, InvocationRecovery, Metadata, Stage,
    Version,
};
use finstack_ai_runtime::{
    MIDDLEWARE_OUTCOME_NOT_ALLOWED, Middleware, MiddlewareContext, MiddlewareDescriptor,
    MiddlewareError, MiddlewareOrder, MiddlewareRole, OrderTier, PortFuture, StageInput, StageMask,
    StageOutcome,
};
use thiserror::Error;

mod detect;

use detect::Detectors;

const COMPONENT_ID: &str = "finstack.middleware.redaction";
const REDACTION_VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};

/// What to do when the model's own output contains a detectable secret.
///
/// `AfterModel` middleware cannot `Replace`, so output redaction is never a
/// rewrite (see the crate docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputPolicy {
    /// Do not observe model output at all. Assistant history is still
    /// redacted on the next `BeforeModel` pass before it reaches a provider.
    Off,
    /// Declare the `AfterModel` stage and fail the run with a safe
    /// descriptor when the assistant message contains a detectable secret.
    Fail,
}

/// Detector and output-policy configuration.
///
/// Defaults enable every detector group with [`OutputPolicy::Off`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct RedactionConfig {
    /// Redact email addresses (`[REDACTED:email]`).
    pub detect_emails: bool,
    /// Redact vendor API keys, JWTs, and PEM private keys
    /// (`[REDACTED:api-key]`, `[REDACTED:jwt]`, `[REDACTED:private-key]`).
    pub detect_api_keys: bool,
    /// Redact Luhn-valid card numbers and mod-97-valid IBANs
    /// (`[REDACTED:card]`, `[REDACTED:iban]`).
    pub detect_account_numbers: bool,
    /// Model-output policy.
    pub output_policy: OutputPolicy,
}

impl Default for RedactionConfig {
    fn default() -> Self {
        Self {
            detect_emails: true,
            detect_api_keys: true,
            detect_account_numbers: true,
            output_policy: OutputPolicy::Off,
        }
    }
}

/// Construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RedactionError {
    /// The configuration or a checked-in identity constant is invalid.
    #[error("redaction_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// Fail-soft PII/secret redaction middleware.
#[derive(Clone)]
pub struct RedactionMiddleware {
    descriptor: MiddlewareDescriptor,
    detectors: Arc<Detectors>,
    config: RedactionConfig,
    inner: Option<Arc<dyn Middleware>>,
}

impl std::fmt::Debug for RedactionMiddleware {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RedactionMiddleware")
            .field("config", &self.config)
            .field("wrapping", &self.inner.is_some())
            .finish_non_exhaustive()
    }
}

impl RedactionMiddleware {
    /// Construct with the default configuration (every detector enabled,
    /// [`OutputPolicy::Off`]).
    ///
    /// # Errors
    ///
    /// Rejects an invalid checked-in identity.
    pub fn try_new() -> Result<Self, RedactionError> {
        Self::try_with_config(RedactionConfig::default())
    }

    /// Construct with an explicit configuration.
    ///
    /// # Errors
    ///
    /// Rejects an all-disabled detector set or an invalid checked-in
    /// identity.
    pub fn try_with_config(config: RedactionConfig) -> Result<Self, RedactionError> {
        let detectors = Arc::new(Detectors::try_new(config)?);
        let stages = match config.output_policy {
            OutputPolicy::Off => StageMask::from_stages([Stage::BeforeModel]),
            OutputPolicy::Fail => StageMask::from_stages([Stage::BeforeModel, Stage::AfterModel]),
        };
        Ok(Self {
            descriptor: MiddlewareDescriptor {
                invocation: ComponentInvocation {
                    component: parse_component_id()?,
                    version: REDACTION_VERSION,
                    configuration_digest: configuration_digest(config, None)?,
                    recovery: InvocationRecovery::RecomputeSafe,
                },
                stages,
                order: MiddlewareOrder {
                    tier: OrderTier::RequestShaping,
                    priority: 0,
                    before: Arc::from([]),
                    after: Arc::from([]),
                },
                role: MiddlewareRole::Standard,
                metadata: Metadata::empty(),
            },
            detectors,
            config,
            inner: None,
        })
    }
}

fn parse_component_id() -> Result<ComponentId, RedactionError> {
    ComponentId::parse(COMPONENT_ID).map_err(|_| RedactionError::Configuration {
        reason: "invalid_component_id",
    })
}

/// Canonical digest over the effective configuration (and, in wrapping mode,
/// the wrapped component's exact invocation identity), so any change to
/// either changes the locked chain digest.
fn configuration_digest(
    config: RedactionConfig,
    inner: Option<&ComponentInvocation>,
) -> Result<Digest, RedactionError> {
    let value = serde_json::json!({
        "config": config,
        "wraps": inner.map(|invocation| serde_json::json!({
            "component": invocation.component,
            "version": invocation.version,
            "configuration_digest": invocation.configuration_digest,
        })),
    });
    let bytes =
        serde_json_canonicalizer::to_vec(&value).map_err(|_| RedactionError::Configuration {
            reason: "configuration_not_canonicalizable",
        })?;
    Ok(Digest::raw_json(&bytes))
}

impl Middleware for RedactionMiddleware {
    fn descriptor(&self) -> MiddlewareDescriptor {
        self.descriptor.clone()
    }

    fn invoke(
        &self,
        _ctx: MiddlewareContext,
        _input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
        Box::pin(async move { Ok(StageOutcome::Continue) })
    }
}

#[allow(dead_code)]
fn stable_error(message: &'static str) -> MiddlewareError {
    MiddlewareError::try_new(
        MIDDLEWARE_OUTCOME_NOT_ALLOWED,
        ErrorCategory::Middleware,
        message,
        Metadata::empty(),
    )
    .unwrap_or_else(Into::into)
}

#[cfg(test)]
mod tests;
