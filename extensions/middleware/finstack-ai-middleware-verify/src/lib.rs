//! Evidence-verifier battery middleware.
//!
//! `VerifyMiddleware` wraps a pure, deterministic [`EvidenceVerifier`] and
//! runs it at two stages:
//!
//! - `before_finalize` judges the terminal candidate's canonical assistant
//!   message. `Accept` continues, `Bounce` requests a semantic
//!   [`finstack_ai_kernel::RetryClassification::Verification`] retry, and
//!   `Reject` fails the run with the stable `verify_rejected` code.
//! - `before_model` re-derives the same verdict from the trailing draft
//!   message (present on the bounce cycle), fed to the verifier as the same
//!   JCS-canonical `Message` JSON `before_finalize` uses, and, when it is
//!   not `Accept`, renders the findings as one user-visible feedback context
//!   item. This channel is deliberately stateless: nothing is journaled, so
//!   crash recovery just re-runs the pure verifier.
//!
//! The middleware never writes a store.

#![warn(missing_docs)]
#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
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
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

use std::fmt;
use std::sync::Arc;

use finstack_ai_kernel::{
    ComponentId, ComponentInvocation, ContentBlock, Digest, Duration, ErrorCategory,
    ErrorDescriptor, InvocationRecovery, LABEL_MAX_BYTES, Message, MessageRole, Metadata, RawJson,
    RetryClassification, RetryDirective, Sensitivity, Stage, TEXT_MAX_BYTES, TextBlock, Version,
};
use finstack_ai_runtime::{
    ContextAuthority, ContextItem, ContextItemKind, ContextProvenance,
    MIDDLEWARE_OUTCOME_NOT_ALLOWED, Middleware, MiddlewareContext, MiddlewareDescriptor,
    MiddlewareError, MiddlewareOrder, MiddlewareRole, OrderTier, PortFuture, StageInput, StageMask,
    StageOutcome,
};
use thiserror::Error;

const VERIFY_VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};

/// Evidence category a finding refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceKind {
    /// The finding refers to a citation.
    Citation,
    /// The finding refers to a test.
    Test,
    /// The finding refers to an artifact.
    Artifact,
}

impl EvidenceKind {
    /// Stable lowercase tag rendered into feedback and failure text.
    const fn tag(self) -> &'static str {
        match self {
            Self::Citation => "citation",
            Self::Test => "test",
            Self::Artifact => "artifact",
        }
    }
}

/// One bounded, non-secret finding produced by an [`EvidenceVerifier`].
#[derive(Debug, Clone)]
pub struct EvidenceFinding {
    /// Evidence category this finding refers to.
    pub kind: EvidenceKind,
    /// Human-readable, non-secret note. Capped at the kernel text bound.
    pub note: Arc<str>,
}

impl EvidenceFinding {
    /// Construct a finding, truncating an oversized note rather than
    /// rejecting it.
    ///
    /// # Errors
    ///
    /// Returns [`VerifyError`] when `note` is empty or contains a NUL byte.
    pub fn try_new(kind: EvidenceKind, note: &str) -> Result<Self, VerifyError> {
        if note.is_empty() || note.as_bytes().contains(&0) {
            return Err(VerifyError::Configuration {
                reason: "invalid_finding_note",
            });
        }
        Ok(Self {
            kind,
            note: Arc::from(truncate_to_bytes(note, TEXT_MAX_BYTES)),
        })
    }
}

/// Verifier decision for one candidate message.
#[derive(Debug, Clone)]
pub enum Verdict {
    /// Land the candidate.
    Accept,
    /// Bounce it back to the model with feedback (maps to `Retry` at
    /// `before_finalize`, `AddContext` at `before_model`).
    Bounce(Vec<EvidenceFinding>),
    /// Fail the run (maps to `Fail` with code `verify_rejected`,
    /// non-retryable).
    Reject(Vec<EvidenceFinding>),
}

/// Pure, deterministic content check.
///
/// Same input must give the same verdict: invocations are re-run wholesale
/// on recovery and are never journaled.
pub trait EvidenceVerifier: Send + Sync + fmt::Debug {
    /// Stable identity folded into the middleware configuration digest.
    fn verifier_id(&self) -> &str;
    /// Judge one canonical assistant `Message` JSON.
    fn verify(&self, message: &RawJson) -> Verdict;
}

/// Bounce policy: backoff and policy-version label folded into the
/// `RetryDirective` produced at `before_finalize`.
#[derive(Debug, Clone)]
pub struct VerifyPolicy {
    backoff: Duration,
    policy_version: Arc<str>,
}

impl VerifyPolicy {
    /// Construct a bounce policy with a bounded, non-empty policy version.
    ///
    /// # Errors
    ///
    /// Returns [`VerifyError`] when `policy_version` is empty, oversized, or
    /// contains a NUL byte.
    pub fn try_new(backoff: Duration, policy_version: &str) -> Result<Self, VerifyError> {
        validate_label(policy_version, "invalid_policy_version")?;
        Ok(Self {
            backoff,
            policy_version: Arc::from(policy_version),
        })
    }
}

/// Verify-leaf construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum VerifyError {
    /// Configuration is malformed.
    #[error("verify_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// Evidence-verifier battery middleware. It never writes a store.
#[derive(Debug, Clone)]
pub struct VerifyMiddleware {
    descriptor: MiddlewareDescriptor,
    verifier: Arc<dyn EvidenceVerifier>,
    policy: VerifyPolicy,
}

impl VerifyMiddleware {
    /// Construct the middleware from a verifier and its bounce policy.
    ///
    /// # Errors
    ///
    /// Rejects an invalid checked-in identity or configuration.
    pub fn try_new(
        verifier: Arc<dyn EvidenceVerifier>,
        policy: VerifyPolicy,
    ) -> Result<Self, VerifyError> {
        let verifier_id = verifier.verifier_id();
        validate_label(verifier_id, "invalid_verifier_id")?;
        let backoff_ms = policy.backoff.as_millis();
        let configuration_digest =
            configuration_digest(verifier_id, policy.policy_version.as_ref(), backoff_ms)?;
        Ok(Self {
            descriptor: MiddlewareDescriptor {
                invocation: ComponentInvocation {
                    component: ComponentId::parse("finstack.middleware.verify").map_err(|_| {
                        VerifyError::Configuration {
                            reason: "invalid_component_id",
                        }
                    })?,
                    version: VERIFY_VERSION,
                    configuration_digest,
                    recovery: InvocationRecovery::RecomputeSafe,
                },
                stages: StageMask::from_stages([Stage::BeforeModel, Stage::BeforeFinalize]),
                order: MiddlewareOrder {
                    tier: OrderTier::Standard,
                    priority: 0,
                    before: Arc::from([]),
                    after: Arc::from([]),
                },
                role: MiddlewareRole::Standard,
                metadata: Metadata::empty(),
            },
            verifier,
            policy,
        })
    }
}

impl Middleware for VerifyMiddleware {
    fn descriptor(&self) -> MiddlewareDescriptor {
        self.descriptor.clone()
    }

    fn invoke(
        &self,
        _ctx: MiddlewareContext,
        input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
        let verifier = Arc::clone(&self.verifier);
        let backoff = self.policy.backoff;
        let policy_version = Arc::clone(&self.policy.policy_version);
        Box::pin(async move {
            match input {
                StageInput::BeforeFinalize { result_message, .. } => match result_message {
                    None => Ok(StageOutcome::Continue),
                    Some(message) => match verifier.verify(&message) {
                        Verdict::Accept => Ok(StageOutcome::Continue),
                        Verdict::Bounce(_findings) => {
                            let directive = RetryDirective::try_new(
                                RetryClassification::Verification,
                                backoff,
                                policy_version.as_ref(),
                            )
                            .map_err(|_| {
                                stable_error(
                                    "verify_retry_directive_invalid",
                                    "verify retry directive is invalid",
                                )
                            })?;
                            Ok(StageOutcome::Retry(directive))
                        }
                        Verdict::Reject(findings) => {
                            let message_text = reject_message(&findings);
                            let error = ErrorDescriptor::new(
                                "verify_rejected",
                                message_text,
                                ErrorCategory::Validation,
                                false,
                            )
                            .map_err(|_| {
                                stable_error(
                                    "verify_rejected_descriptor_invalid",
                                    "verify rejection descriptor is invalid",
                                )
                            })?;
                            Ok(StageOutcome::Fail(Box::new(error)))
                        }
                    },
                },
                StageInput::BeforeModel(input) => {
                    let trailing_verdict = input
                        .request
                        .messages
                        .last()
                        .filter(|message| message.role() == MessageRole::Assistant)
                        .map(|message| {
                            assistant_message_raw_json(message).map(|raw| verifier.verify(&raw))
                        })
                        .transpose()?;
                    match trailing_verdict {
                        Some(Verdict::Bounce(findings) | Verdict::Reject(findings)) => {
                            let item = feedback_item(&findings)?;
                            Ok(StageOutcome::AddContext(Arc::from([item])))
                        }
                        _ => Ok(StageOutcome::Continue),
                    }
                }
                _ => Err(stable_error(
                    MIDDLEWARE_OUTCOME_NOT_ALLOWED,
                    "verify only runs at before_model or before_finalize",
                )),
            }
        })
    }
}

/// JCS-canonical `Message` JSON for the trailing assistant draft message.
///
/// Byte-identical to the runtime's own `before_finalize` `result_message`
/// encoding (`finstack-ai-runtime`'s `stage_settlement::codec::canonical_message`),
/// so the same pure [`EvidenceVerifier`] re-derives the same findings from
/// either call site.
fn assistant_message_raw_json(message: &Message) -> Result<RawJson, MiddlewareError> {
    let bytes = serde_json_canonicalizer::to_vec(message).map_err(|_| {
        stable_error(
            "verify_message_encoding_invalid",
            "verify could not canonicalize the assistant draft message",
        )
    })?;
    RawJson::parse(&bytes).map_err(|_| {
        stable_error(
            "verify_message_encoding_invalid",
            "verify could not canonicalize the assistant draft message",
        )
    })
}

/// Render bounce/reject findings as one trusted-application feedback
/// context item.
///
/// The wording deliberately claims no rejection event: the trailing
/// assistant message of a draft can also be one the facade bounced for a
/// structured-output validation failure (which never passes through this
/// middleware), so the item states the verifier's current findings about
/// the answer being retried rather than asserting that evidence
/// verification caused the retry.
fn feedback_item(findings: &[EvidenceFinding]) -> Result<ContextItem, MiddlewareError> {
    let text = verdict_message(
        "Evidence verification found issues with the previous assistant answer",
        findings,
    );
    let bounded = truncate_to_bytes(&text, TEXT_MAX_BYTES);
    let block = TextBlock::try_new(bounded).map_err(|_| {
        stable_error(
            "verify_feedback_text_invalid",
            "verify feedback text is invalid",
        )
    })?;
    let estimated_tokens = u64::try_from(bounded.len()).unwrap_or(u64::MAX);
    ContextItem::try_new(
        ContextItemKind::Instruction,
        vec![ContentBlock::Text(block)],
        ContextProvenance {
            source_id: Arc::from("finstack.middleware.verify"),
            source_ref: None,
            external: false,
        },
        ContextAuthority::TrustedApplication,
        0,
        estimated_tokens,
        Sensitivity::Internal,
        false,
    )
    .map_err(|_| {
        stable_error(
            "verify_feedback_item_invalid",
            "verify feedback item is invalid",
        )
    })
}

/// Bounded failure message for a `Reject` verdict.
fn reject_message(findings: &[EvidenceFinding]) -> String {
    verdict_message("evidence verification rejected the candidate", findings)
}

/// Shared verdict renderer: `intro` alone when there are no findings,
/// otherwise `intro:` followed by one finding line each, bounded to the
/// kernel text limit.
fn verdict_message(intro: &str, findings: &[EvidenceFinding]) -> String {
    let lines = findings_lines(findings);
    let message = if lines.is_empty() {
        intro.to_owned()
    } else {
        format!("{intro}:\n{lines}")
    };
    truncate_to_bytes(&message, TEXT_MAX_BYTES).to_owned()
}

/// One `- [kind] note` line per finding, newline-joined.
fn findings_lines(findings: &[EvidenceFinding]) -> String {
    findings
        .iter()
        .map(|finding| format!("- [{}] {}", finding.kind.tag(), finding.note))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Truncate `text` to at most `max_bytes` bytes on a UTF-8 boundary.
fn truncate_to_bytes(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// Bounded, non-empty label check shared by the verifier id and policy
/// version.
fn validate_label(value: &str, reason: &'static str) -> Result<(), VerifyError> {
    if value.is_empty() || value.len() > LABEL_MAX_BYTES || value.as_bytes().contains(&0) {
        return Err(VerifyError::Configuration { reason });
    }
    Ok(())
}

/// Serialized shape of the configuration identity behind
/// [`configuration_digest`]. Field names are the digest's JSON keys.
#[derive(serde::Serialize)]
struct VerifyConfiguration<'a> {
    backoff_ms: u64,
    policy_version: &'a str,
    verifier_id: &'a str,
}

/// Canonical-JSON digest over `{verifier_id, policy_version, backoff_ms}`.
fn configuration_digest(
    verifier_id: &str,
    policy_version: &str,
    backoff_ms: u64,
) -> Result<Digest, VerifyError> {
    let configuration = VerifyConfiguration {
        backoff_ms,
        policy_version,
        verifier_id,
    };
    let bytes = serde_json_canonicalizer::to_vec(&configuration).map_err(|_| {
        VerifyError::Configuration {
            reason: "invalid_configuration_encoding",
        }
    })?;
    let raw = RawJson::parse(&bytes).map_err(|_| VerifyError::Configuration {
        reason: "invalid_configuration_encoding",
    })?;
    Ok(raw.digest())
}

/// Build a stable, non-fallible middleware error.
fn stable_error(code: &'static str, message: &'static str) -> MiddlewareError {
    MiddlewareError::try_new(code, ErrorCategory::Middleware, message, Metadata::empty())
        .unwrap_or_else(Into::into)
}

#[cfg(test)]
mod tests;
