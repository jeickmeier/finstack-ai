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
    ComponentId, ComponentInvocation, ContentBlock, Digest, ErrorCategory, ErrorDescriptor,
    InvocationRecovery, Message, Metadata, RawJson, Stage, TextBlock, ToolResultBlock, Version,
};
use finstack_ai_runtime::{
    BeforeModelInput, MIDDLEWARE_OUTCOME_NOT_ALLOWED, Middleware, MiddlewareContext,
    MiddlewareDescriptor, MiddlewareError, MiddlewareOrder, MiddlewareRole, ModelRequestDraft,
    OrderTier, PortFuture, StageInput, StageMask, StageOutcome,
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

    /// Compose redaction around another `BeforeModel` middleware.
    ///
    /// The middleware chain hands every `BeforeModel` component the same
    /// base draft and keeps only the last `Replace` in chain order, so a
    /// standalone redaction middleware cannot coexist with another
    /// Replace-emitting `BeforeModel` middleware (such as document-ingest):
    /// one of the two rewrites would be silently discarded. This constructor
    /// solves that by registering redaction *as* the inner middleware's
    /// chain slot: the wrapper adopts `inner`'s ordering, invokes it first,
    /// and redacts whatever draft it produces — so text the inner middleware
    /// injects (e.g. Markdown extracted from attachments) is redacted too.
    ///
    /// # Errors
    ///
    /// Rejects an inner middleware whose stage mask is not exactly
    /// `BeforeModel` or whose role is not `Standard`, an all-disabled
    /// detector set, and an invalid checked-in identity.
    pub fn try_wrapping(
        inner: Arc<dyn Middleware>,
        config: RedactionConfig,
    ) -> Result<Self, RedactionError> {
        let detectors = Arc::new(Detectors::try_new(config)?);
        let inner_descriptor = inner.descriptor();
        if inner_descriptor.stages != StageMask::from_stages([Stage::BeforeModel]) {
            return Err(RedactionError::Configuration {
                reason: "wrapped_middleware_not_before_model_only",
            });
        }
        if inner_descriptor.role != MiddlewareRole::Standard {
            return Err(RedactionError::Configuration {
                reason: "wrapped_middleware_not_standard_role",
            });
        }
        let stages = match config.output_policy {
            OutputPolicy::Off => StageMask::from_stages([Stage::BeforeModel]),
            OutputPolicy::Fail => StageMask::from_stages([Stage::BeforeModel, Stage::AfterModel]),
        };
        Ok(Self {
            descriptor: MiddlewareDescriptor {
                invocation: ComponentInvocation {
                    component: parse_component_id()?,
                    version: REDACTION_VERSION,
                    configuration_digest: configuration_digest(
                        config,
                        Some(&inner_descriptor.invocation),
                    )?,
                    recovery: InvocationRecovery::RecomputeSafe,
                },
                stages,
                order: inner_descriptor.order,
                role: MiddlewareRole::Standard,
                metadata: Metadata::empty(),
            },
            detectors,
            config,
            inner: Some(inner),
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
        ctx: MiddlewareContext,
        input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
        let middleware = self.clone();
        Box::pin(async move {
            match input {
                StageInput::BeforeModel(before_model) => {
                    let Some(inner) = middleware.inner.clone() else {
                        return Ok(middleware.redact_before_model(&before_model));
                    };
                    let outcome = inner
                        .invoke(ctx, StageInput::BeforeModel(before_model.clone()))
                        .await?;
                    Ok(middleware.redact_inner_outcome(&before_model, outcome))
                }
                StageInput::AfterModel { value } => Ok(middleware.check_after_model(&value)),
                _ => Ok(StageOutcome::Continue),
            }
        })
    }
}

impl RedactionMiddleware {
    /// Rewrite the model-visible draft, replacing detected secrets with
    /// markers. Unchanged drafts continue; every internal failure degrades
    /// to passing the affected content through unmodified (fail-soft).
    fn redact_before_model(&self, before_model: &BeforeModelInput) -> StageOutcome {
        let (draft, changed) = self.redact_draft(&before_model.request);
        if !changed {
            return StageOutcome::Continue;
        }
        let Ok(bytes) = serde_json_canonicalizer::to_vec(&draft) else {
            return StageOutcome::Continue;
        };
        RawJson::parse(bytes).map_or(StageOutcome::Continue, StageOutcome::Replace)
    }

    /// Redact every message of `draft`, reporting whether anything changed.
    fn redact_draft(&self, draft: &ModelRequestDraft) -> (ModelRequestDraft, bool) {
        let mut changed = false;
        let mut messages: Vec<Message> = Vec::with_capacity(draft.messages.len());
        for message in draft.messages.iter() {
            let (rewritten, message_changed) = self.redact_message(message);
            changed |= message_changed;
            messages.push(rewritten);
        }
        if !changed {
            return (draft.clone(), false);
        }
        (
            ModelRequestDraft {
                messages: messages.into(),
                ..draft.clone()
            },
            true,
        )
    }

    /// Redact one message's text-bearing blocks, preserving its identity
    /// (id, role, timestamps, model, provider ids, metadata) exactly. A
    /// rebuild failure falls back to the original message: unlike
    /// document-ingest there is no must-strip invariant here — fail-soft
    /// means unredacted pass-through, never an aborted run.
    fn redact_message(&self, message: &Message) -> (Message, bool) {
        let mut changed = false;
        let mut blocks: Vec<ContentBlock> = Vec::with_capacity(message.content().len());
        for block in message.content() {
            let (rewritten, block_changed) = self.redact_block(block);
            changed |= block_changed;
            blocks.push(rewritten);
        }
        if !changed {
            return (message.clone(), false);
        }
        match Message::try_new(
            *message.id(),
            message.role(),
            blocks,
            message.created_at(),
            message.model().cloned(),
            message.provider_ids().clone(),
            message.metadata().clone(),
        ) {
            Ok(rebuilt) => (rebuilt, true),
            Err(_) => (message.clone(), false),
        }
    }

    /// Redact whatever draft the wrapped middleware produced.
    ///
    /// `Replace` payloads are parsed back into a [`ModelRequestDraft`],
    /// redacted, and re-canonicalized; an unparsable payload or a failed
    /// re-canonicalization passes the inner `Replace` through unchanged
    /// (fail-soft — the inner middleware's work is never dropped).
    /// `Continue` falls back to redacting the base draft, and every other
    /// outcome passes through untouched.
    fn redact_inner_outcome(
        &self,
        before_model: &BeforeModelInput,
        outcome: StageOutcome,
    ) -> StageOutcome {
        match outcome {
            StageOutcome::Replace(raw) => {
                let Ok(inner_draft) = serde_json::from_slice::<ModelRequestDraft>(raw.as_bytes())
                else {
                    return StageOutcome::Replace(raw);
                };
                let (draft, changed) = self.redact_draft(&inner_draft);
                if !changed {
                    return StageOutcome::Replace(raw);
                }
                let Ok(bytes) = serde_json_canonicalizer::to_vec(&draft) else {
                    return StageOutcome::Replace(raw);
                };
                RawJson::parse(bytes).map_or(StageOutcome::Replace(raw), StageOutcome::Replace)
            }
            StageOutcome::Continue => self.redact_before_model(before_model),
            other => other,
        }
    }

    /// Apply the configured [`OutputPolicy`] to the assistant message the
    /// model just produced. `AfterModel` middleware cannot `Replace`, so
    /// `Fail` is the only enforcement available here; the descriptor names
    /// detector kinds and the match count only — never the matched text.
    /// Undecodable payloads are fail-soft `Continue`.
    fn check_after_model(&self, value: &RawJson) -> StageOutcome {
        if self.config.output_policy != OutputPolicy::Fail {
            return StageOutcome::Continue;
        }
        let Ok(message) = serde_json::from_slice::<Message>(value.as_bytes()) else {
            return StageOutcome::Continue;
        };
        let mut kinds = std::collections::BTreeSet::new();
        let mut count = 0_usize;
        for block in message.content() {
            collect_findings(&self.detectors, block, &mut kinds, &mut count);
        }
        if count == 0 {
            return StageOutcome::Continue;
        }
        let kinds = kinds.into_iter().collect::<Vec<_>>().join(", ");
        ErrorDescriptor::new(
            "redaction_output_detected",
            format!("model output contains detectable secrets ({count} matches): {kinds}"),
            ErrorCategory::Middleware,
            false,
        )
        .map_or(StageOutcome::Continue, |descriptor| {
            StageOutcome::Fail(Box::new(descriptor))
        })
    }

    /// Redact one content block. `Text` is rewritten directly; `ToolResult`
    /// recurses one level into its nested content (tool results cannot nest
    /// further tool blocks). Everything else — `Json`, `Opaque`, media, and
    /// `ToolCall` arguments — passes through untouched (v1 scan surface).
    fn redact_block(&self, block: &ContentBlock) -> (ContentBlock, bool) {
        match block {
            ContentBlock::Text(text) => match self.detectors.redact(text.text()) {
                Some(redacted) => match TextBlock::try_new(redacted) {
                    Ok(rewritten) => (ContentBlock::Text(rewritten), true),
                    Err(_) => (block.clone(), false),
                },
                None => (block.clone(), false),
            },
            ContentBlock::ToolResult(result) => {
                let mut changed = false;
                let mut nested: Vec<ContentBlock> = Vec::with_capacity(result.content().len());
                for inner in result.content() {
                    let (rewritten, block_changed) = self.redact_block(inner);
                    changed |= block_changed;
                    nested.push(rewritten);
                }
                if !changed {
                    return (block.clone(), false);
                }
                match ToolResultBlock::try_new(*result.tool_call_id(), nested, result.is_error()) {
                    Ok(rebuilt) => (ContentBlock::ToolResult(rebuilt), true),
                    Err(_) => (block.clone(), false),
                }
            }
            other => (other.clone(), false),
        }
    }
}

/// Accumulate detector findings over one block's text surface (the same
/// surface [`RedactionMiddleware::redact_block`] rewrites).
fn collect_findings(
    detectors: &Detectors,
    block: &ContentBlock,
    kinds: &mut std::collections::BTreeSet<&'static str>,
    count: &mut usize,
) {
    match block {
        ContentBlock::Text(text) => {
            let (found, matches) = detectors.findings(text.text());
            kinds.extend(found);
            *count += matches;
        }
        ContentBlock::ToolResult(result) => {
            for inner in result.content() {
                collect_findings(detectors, inner, kinds, count);
            }
        }
        _ => {}
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
