//! `BeforeModel` middleware that redacts PII and secrets from the
//! model-visible request draft.
//!
//! The canonical conversation is never touched: only the model-visible
//! [`ModelRequestDraft`] is rewritten, via [`StageOutcome::Replace`], exactly
//! like the sibling document-ingest middleware. Each detected secret — API
//! key, token, JWT, PEM private key, card number, IBAN, or email address —
//! is replaced by a stable `[REDACTED:<kind>]` marker carrying nothing
//! recoverable. Redaction is idempotent and deterministic. A detected value
//! whose marker rewrite would exceed the text ceiling becomes
//! `[REDACTED:oversized]`; unexpected reconstruction failures abort with a
//! stable non-secret middleware error rather than passing sensitive content
//! through.
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
//! The same hazard applies to a registered **context compactor**: the
//! settlement applier lands a `CompactContext` projection on top of any
//! `Replace` in the same fold (`apply_model_draft` overwrites
//! `draft.messages` with the compactor's `replacement_messages`, which were
//! validated against the *unredacted* base draft), so a compactor in the
//! chain discards this middleware's rewrite for every entry the projection
//! covers — silently. Do not register this middleware together with a
//! `ContextCompactor`-role middleware until the runtime redacts or threads
//! compaction projections through `Replace` payloads.
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
    ComponentId, ComponentInvocation, ContentBlock, ContentError, Digest, ErrorCategory,
    ErrorDescriptor, InvocationRecovery, Message, Metadata, RawJson, Stage, TextBlock,
    ToolResultBlock, Version,
};
use finstack_ai_runtime::{
    BeforeModelInput, Middleware, MiddlewareContext, MiddlewareDescriptor, MiddlewareError,
    MiddlewareOrder, MiddlewareRole, ModelRequestDraft, OrderTier, PortFuture, StageInput,
    StageMask, StageOutcome,
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
const FAIL_OUTCOME_FALLBACK_MESSAGE: &str = "model output failed redaction checking";
const FAIL_CONSTRUCTION_CODE: &str = "redaction_output_fail_unconstructable";
const REDACTION_REWRITE_CODE: &str = "redaction_rewrite_failed";
const REDACTION_REWRITE_MESSAGE: &str = "redaction rewrite could not be constructed safely";
const OVERSIZED_MARKER: &str = "[REDACTED:oversized]";

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
        Self::try_assemble(config, None)
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
        Self::try_assemble(config, Some(inner))
    }

    /// Shared standalone/wrapping constructor.
    ///
    /// Detector validation runs before wrapped-middleware checks so the
    /// error order matches the previous dual constructors. Wrapping adopts
    /// `inner`'s order; the stage mask still follows `config.output_policy`.
    fn try_assemble(
        config: RedactionConfig,
        inner: Option<Arc<dyn Middleware>>,
    ) -> Result<Self, RedactionError> {
        let detectors = Arc::new(Detectors::try_new(config)?);
        let (order, wrap_invocation) = match inner.as_ref() {
            Some(inner) => {
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
                if inner_descriptor.invocation.recovery != InvocationRecovery::RecomputeSafe {
                    return Err(RedactionError::Configuration {
                        reason: "wrapped_middleware_not_recompute_safe",
                    });
                }
                (inner_descriptor.order, Some(inner_descriptor.invocation))
            }
            None => (
                MiddlewareOrder {
                    tier: OrderTier::RequestShaping,
                    priority: 0,
                    before: Arc::from([]),
                    after: Arc::from([]),
                },
                None,
            ),
        };
        let stages = match config.output_policy {
            OutputPolicy::Off => StageMask::from_stages([Stage::BeforeModel]),
            OutputPolicy::Fail => StageMask::from_stages([Stage::BeforeModel, Stage::AfterModel]),
        };
        Ok(Self {
            descriptor: MiddlewareDescriptor {
                invocation: ComponentInvocation {
                    component: parse_component_id()?,
                    version: REDACTION_VERSION,
                    configuration_digest: configuration_digest(config, wrap_invocation.as_ref())?,
                    recovery: InvocationRecovery::RecomputeSafe,
                },
                stages,
                order,
                role: MiddlewareRole::Standard,
                metadata: Metadata::empty(),
            },
            detectors,
            config,
            inner,
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
            "recovery": invocation.recovery,
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
                        return middleware.redact_before_model(&before_model);
                    };
                    let outcome = inner
                        .invoke(ctx, StageInput::BeforeModel(before_model.clone()))
                        .await?;
                    middleware.redact_inner_outcome(&before_model, outcome)
                }
                StageInput::AfterModel { value } => Ok(middleware.check_after_model(&value)?),
                _ => Ok(StageOutcome::Continue),
            }
        })
    }
}

impl RedactionMiddleware {
    /// Rewrite the model-visible draft, replacing detected secrets with
    /// markers. Unchanged drafts continue; unexpected reconstruction failures
    /// fail closed with a stable non-secret error.
    fn redact_before_model(
        &self,
        before_model: &BeforeModelInput,
    ) -> Result<StageOutcome, MiddlewareError> {
        let Some(draft) = self.redact_draft(&before_model.request)? else {
            return Ok(StageOutcome::Continue);
        };
        encode_replacement(&draft)
    }

    /// Redact every message of `draft`. Returns `None` when nothing changed,
    /// so the dominant clean-draft path clones nothing.
    fn redact_draft(
        &self,
        draft: &ModelRequestDraft,
    ) -> Result<Option<ModelRequestDraft>, MiddlewareError> {
        let Some(messages) = redact_slice(&draft.messages, |message| self.redact_message(message))?
        else {
            return Ok(None);
        };
        Ok(Some(ModelRequestDraft {
            messages: messages.into(),
            ..draft.clone()
        }))
    }

    /// Redact one message's text-bearing blocks, preserving its identity
    /// (id, role, timestamps, model, provider ids, metadata) exactly.
    /// Returns `None` when unchanged. Rebuild failures fail closed.
    fn redact_message(&self, message: &Message) -> Result<Option<Message>, MiddlewareError> {
        let Some(blocks) = redact_slice(message.content(), |block| self.redact_block(block))?
        else {
            return Ok(None);
        };
        Message::try_new(
            *message.id(),
            message.role(),
            blocks,
            message.created_at(),
            message.model().cloned(),
            message.provider_ids().clone(),
            message.metadata().clone(),
        )
        .map(Some)
        .map_err(|_| rewrite_error())
    }

    /// Redact whatever draft the wrapped middleware produced.
    ///
    /// `Replace` payloads are parsed back into a [`ModelRequestDraft`],
    /// redacted, and re-canonicalized; an unparsable payload or failed
    /// reconstruction aborts with a stable non-secret middleware error.
    /// `Continue` falls back to redacting the base draft, and every other
    /// outcome passes through untouched.
    fn redact_inner_outcome(
        &self,
        before_model: &BeforeModelInput,
        outcome: StageOutcome,
    ) -> Result<StageOutcome, MiddlewareError> {
        match outcome {
            StageOutcome::Replace(raw) => {
                let inner_draft = serde_json::from_slice::<ModelRequestDraft>(raw.as_bytes())
                    .map_err(|_| rewrite_error())?;
                let Some(draft) = self.redact_draft(&inner_draft)? else {
                    return Ok(StageOutcome::Replace(raw));
                };
                encode_replacement(&draft)
            }
            StageOutcome::Continue => self.redact_before_model(before_model),
            other => Ok(other),
        }
    }

    /// Apply the configured [`OutputPolicy`] to the assistant message the
    /// model just produced. `AfterModel` middleware cannot `Replace`, so
    /// `Fail` is the only enforcement available here; the descriptor names
    /// detector kinds and the match count only — never the matched text.
    ///
    /// Unlike every other path in this crate, `Fail` mode fails **closed**:
    /// an undecodable payload means the output cannot be checked, and a
    /// strict mode that silently stops checking (e.g. after a drift in
    /// [`Message`]'s wire shape) would be a security control turned no-op.
    /// When a `Fail` descriptor cannot be constructed, this returns a
    /// stable [`MiddlewareError`] rather than [`StageOutcome::Continue`].
    fn check_after_model(&self, value: &RawJson) -> Result<StageOutcome, MiddlewareError> {
        if self.config.output_policy != OutputPolicy::Fail {
            return Ok(StageOutcome::Continue);
        }
        let Ok(message) = serde_json::from_slice::<Message>(value.as_bytes()) else {
            return fail_outcome(
                "redaction_output_undecodable",
                "model output could not be decoded for redaction checking".to_owned(),
            );
        };
        let mut kinds = std::collections::BTreeSet::new();
        let mut count = 0_usize;
        for block in message.content() {
            collect_findings(&self.detectors, block, &mut kinds, &mut count);
        }
        if count == 0 {
            return Ok(StageOutcome::Continue);
        }
        let kinds = kinds.into_iter().collect::<Vec<_>>().join(", ");
        fail_outcome(
            "redaction_output_detected",
            format!("model output contains detectable secrets ({count} matches): {kinds}"),
        )
    }

    /// Redact one content block. `Text` is rewritten directly; `ToolResult`
    /// recurses one level into its nested content (tool results cannot nest
    /// further tool blocks). Everything else — `Json`, `Opaque`, media, and
    /// `ToolCall` arguments — passes through untouched (v1 scan surface).
    ///
    /// Returns `None` when unchanged. A detected rewrite that grows past the
    /// text ceiling becomes a bounded marker; other failures fail closed.
    fn redact_block(&self, block: &ContentBlock) -> Result<Option<ContentBlock>, MiddlewareError> {
        match block {
            ContentBlock::Text(text) => {
                let Some(redacted) = self.detectors.redact(text.text()) else {
                    return Ok(None);
                };
                let block = match TextBlock::try_new(redacted) {
                    Ok(block) => block,
                    Err(ContentError::TextTooLarge { .. }) => {
                        TextBlock::try_new(OVERSIZED_MARKER).map_err(|_| rewrite_error())?
                    }
                    Err(_) => return Err(rewrite_error()),
                };
                Ok(Some(ContentBlock::Text(block)))
            }
            ContentBlock::ToolResult(result) => {
                let Some(nested) =
                    redact_slice(result.content(), |inner| self.redact_block(inner))?
                else {
                    return Ok(None);
                };
                ToolResultBlock::try_new(*result.tool_call_id(), nested, result.is_error())
                    .map(ContentBlock::ToolResult)
                    .map(Some)
                    .map_err(|_| rewrite_error())
            }
            _ => Ok(None),
        }
    }
}

/// Lazily rewrite a slice: apply `redact` to each item and return `None`
/// when no item changed. The originals are cloned only once a change has
/// actually occurred, so the dominant no-secrets path allocates and copies
/// nothing.
fn redact_slice<T: Clone>(
    items: &[T],
    mut redact: impl FnMut(&T) -> Result<Option<T>, MiddlewareError>,
) -> Result<Option<Vec<T>>, MiddlewareError> {
    let mut rewritten: Option<Vec<T>> = None;
    for (index, item) in items.iter().enumerate() {
        match redact(item)? {
            Some(new_item) => {
                let vec = rewritten.get_or_insert_with(|| {
                    let mut vec = Vec::with_capacity(items.len());
                    vec.extend(items[..index].iter().cloned());
                    vec
                });
                vec.push(new_item);
            }
            None => {
                if let Some(vec) = rewritten.as_mut() {
                    vec.push(item.clone());
                }
            }
        }
    }
    Ok(rewritten)
}

fn encode_replacement(draft: &ModelRequestDraft) -> Result<StageOutcome, MiddlewareError> {
    let bytes = serde_json_canonicalizer::to_vec(draft).map_err(|_| rewrite_error())?;
    RawJson::parse(bytes)
        .map(StageOutcome::Replace)
        .map_err(|_| rewrite_error())
}

fn rewrite_error() -> MiddlewareError {
    MiddlewareError::try_new(
        REDACTION_REWRITE_CODE,
        ErrorCategory::Middleware,
        REDACTION_REWRITE_MESSAGE,
        Metadata::empty(),
    )
    .unwrap_or_else(Into::into)
}

/// A `Fail` outcome with a stable code and a safe message (never matched
/// text). If the primary descriptor and the static fallback both fail to
/// construct, this returns a stable [`MiddlewareError`] — never
/// [`StageOutcome::Continue`].
fn fail_outcome(code: &'static str, message: String) -> Result<StageOutcome, MiddlewareError> {
    ErrorDescriptor::new(code, message, ErrorCategory::Middleware, false)
        .or_else(|_| {
            ErrorDescriptor::new(
                code,
                FAIL_OUTCOME_FALLBACK_MESSAGE,
                ErrorCategory::Middleware,
                false,
            )
        })
        .map(|descriptor| StageOutcome::Fail(Box::new(descriptor)))
        .map_err(|_| fail_construction_error())
}

/// Last-resort middleware error when no safe `Fail` descriptor can be
/// built. The text is a static literal and never includes caller input.
fn fail_construction_error() -> MiddlewareError {
    MiddlewareError::try_new(
        FAIL_CONSTRUCTION_CODE,
        ErrorCategory::Middleware,
        FAIL_OUTCOME_FALLBACK_MESSAGE,
        Metadata::empty(),
    )
    .unwrap_or_else(Into::into)
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

#[cfg(test)]
mod tests;
