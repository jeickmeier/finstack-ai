//! Aggregate middleware chain driver.
//!
//! The kernel models the middleware boundary as exactly one
//! [`ReducerStageOutcome`] per `(cycle, stage)` cursor
//! ([`finstack_ai_kernel::StageOutcomeRecorded`]). This module runs the ordered
//! component chain for a stage in-process, validates each outcome against the
//! stage matrix, and folds the ordered results into that single aggregate
//! outcome. Individual invocations are not committed as effects.
//!
//! Middleware implementations must therefore be pure with respect to external
//! state: on recovery, a stage whose `StageOutcomeRecorded` is absent re-runs
//! its whole chain.
//!
//! [`StageFold`] is the pure aggregation step: it folds one stage's ordered
//! `Vec<StageOutcome>` (as produced by [`invoke_middleware_stage`]) into a
//! single result, or a stable error when an outcome has nowhere to land.
//! [`StageDriver`] is the handle a caller holds across a run's stage
//! boundaries: the locked chain plus the run's cancellation signal.

use std::collections::BTreeSet;
use std::sync::Arc;

use finstack_ai_kernel::{
    Digest, EffectId, ErrorDescriptor, OperationLocator, RawJson, RetryDirective, Stage,
    StageCursor, ToolId,
};

use crate::context::ContextItem;
use crate::middleware::{
    CompactionResult, MiddlewareError, ResolvedMiddleware, ResolvedMiddlewareChain, StageInput,
    StageOutcome, stage_name, validate_stage_outcome,
};
use crate::model::CancellationSignal;
use crate::{ModelRequestDraft, RunCallContext};

/// Stable code for a middleware outcome with no kernel landing path at the
/// stage it was produced at.
///
/// Covers outcomes that never have a `ReducerStageOutcome` peer at all
/// (`Suspend`, `Complete`, `RequestInteraction`, `RequestCompactionModel`),
/// and outcomes that have one only at a different stage than the one they
/// were produced at (`Retry` outside `BeforeFinalize`; `Replace` and
/// `CompactContext` outside the two stages that carry model context).
pub const MIDDLEWARE_STAGE_UNLANDABLE: &str = "middleware_stage_unlandable";

/// Stable code for a fold whose accumulated `AddInstructions`/`AddContext`
/// content would exceed a kernel-enforced array bound if landed.
///
/// This bound used to be checked only by the kernel, against the final
/// message array. It moves to the driver because middleware can now add to
/// that array; the driver checks what it can see (the aggregate additions),
/// which is a necessary — not sufficient — condition for the kernel accepting
/// the eventual `ReducerStageOutcome`.
pub const MIDDLEWARE_STAGE_BOUNDS_EXCEEDED: &str = "middleware_stage_bounds_exceeded";

/// Shared identity for one stage's chain invocation.
///
/// Constructed once per `(cycle, stage)` boundary and reused for every component
/// in that stage, so all components in a stage observe the same run identity and
/// the same locked chain digest.
#[derive(Debug, Clone)]
pub struct MiddlewareStageContext {
    /// Run-scoped call context reused for every component in the stage.
    pub run: RunCallContext,
    /// Locked chain digest from the resolved agent.
    pub chain_digest: Digest,
    /// `(cycle, stage)` cursor this invocation settles.
    ///
    /// Replaces the `parent: EffectRequested` field this module carried one
    /// commit ago. That field had no producer: no `KernelInput` ever commits
    /// an `EffectKind::Middleware` `EffectRequested` (stage settlement emits
    /// only `StageOutcomeRecorded`), so there is never a committed parent
    /// effect to reference. The cursor is what the driver actually has at a
    /// stage boundary, and it is also the input [`derived_stage_effect_id`]
    /// needs to fabricate `run.effect_id`.
    pub cursor: StageCursor,
}

impl MiddlewareStageContext {
    /// Construct a stage context from its run-call context, locked chain
    /// digest, and `(cycle, stage)` cursor.
    #[must_use]
    pub const fn new(run: RunCallContext, chain_digest: Digest, cursor: StageCursor) -> Self {
        Self {
            run,
            chain_digest,
            cursor,
        }
    }

    /// The stage this invocation settles.
    #[must_use]
    pub const fn stage(&self) -> Stage {
        self.cursor.stage
    }

    /// Per-component invocation context for the component at `index`.
    ///
    /// # Errors
    ///
    /// Returns a stable `middleware_resolution_invalid` when the chain index
    /// exceeds `u32`.
    pub fn middleware_context(
        &self,
        index: usize,
    ) -> Result<crate::middleware::MiddlewareContext, MiddlewareError> {
        Ok(crate::middleware::MiddlewareContext {
            run: self.run.clone(),
            chain_digest: self.chain_digest,
            chain_index: u32::try_from(index).map_err(|_| {
                MiddlewareError::stable(
                    crate::middleware::MIDDLEWARE_RESOLUTION_INVALID,
                    "middleware chain index exceeds u32",
                )
            })?,
            compaction_resume: None,
        })
    }
}

/// Fixed digest domain for [`derived_stage_effect_id`].
const DOMAIN_MIDDLEWARE_STAGE_INVOCATION: &str = "middleware-stage-invocation";

/// Derive a stable, deterministic, **non-committed** correlation id for one
/// stage's chain invocation.
///
/// # This is not a committed effect identity
///
/// No `KernelInput` ever commits an `EffectKind::Middleware`
/// `EffectRequested` — stage settlement emits only `StageOutcomeRecorded`
/// (see the kernel reducer) — so there is no faithful `EffectId` available at
/// a stage boundary in either the runtime or the facade layer. This function
/// fabricates one instead, deterministically from `(locator, cycle, stage)`,
/// so [`RunCallContext`]'s non-`Option` `effect_id` field can be filled with
/// *something* stable rather than left with no faithful value at all.
///
/// Two properties matter and are both required, not incidental:
/// - **Derived, not random**: the same `(locator, cycle, stage)` always
///   produces the same id, so a stage that re-runs its whole chain after a
///   crash (this driver never journals individual invocations) observes the
///   identical correlation id both times.
/// - **Domain-separated**: hashing under a fixed, dedicated digest domain
///   keeps this id from ever colliding with a UuidV7-generated *committed*
///   effect id minted elsewhere in the system.
///
/// Callers must not treat the returned value as a journal key, look it up as
/// a committed effect, or otherwise assume it is backed by any durable
/// record — it is a correlation id only. This distinction has a concrete
/// consequence: `sanitize_call_context`
/// (`plugins/finstack-ai-wit/src/mapping.rs`) copies `RunCallContext.effect_id`
/// verbatim into the guest-visible WIT call context, so a plugin author who
/// assumes that value names a committed effect is wrong, and any future code
/// that tries to correlate this id against the journal is wrong too.
///
/// # Panics
///
/// Does not panic for any real input. The internal canonicalization step is
/// fallible only for values that cannot be represented as JSON (this
/// function's inputs — a bounded `OperationLocator`, a `u64`, and a fixed
/// stage name — always can be), and the digest domain is a fixed non-empty,
/// NUL-free literal, so [`Digest::domain_separated`] cannot reject it either.
#[must_use]
pub fn derived_stage_effect_id(locator: &OperationLocator, cycle: u64, stage: Stage) -> EffectId {
    let canonical = serde_json_canonicalizer::to_vec(&(locator, cycle, stage_name(stage)))
        .expect("OperationLocator/cycle/stage-name are always canonically serializable");
    let digest = Digest::domain_separated(DOMAIN_MIDDLEWARE_STAGE_INVOCATION, 1, &canonical)
        .expect("the fixed middleware-stage-invocation domain is always valid");
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    EffectId::from_bytes(bytes)
}

/// Invoke every component registered for `stage`, in resolved order.
///
/// Each outcome is validated against the stage matrix before it is returned.
/// The caller folds the ordered results into one `ReducerStageOutcome`.
///
/// `ctx.stage()` and `input.stage()` are two independent sources of truth for
/// the same stage — the cursor this invocation settles, and the variant of the
/// payload it hands each component. A mismatched pair would correlate one
/// stage's components under another stage's derived id, so it is asserted in
/// debug builds rather than left to chance.
///
/// # Errors
///
/// Returns the component's own `MiddlewareError`, or a stable
/// `middleware_outcome_not_allowed` when an outcome fails the matrix.
pub async fn invoke_middleware_stage(
    chain: &ResolvedMiddlewareChain,
    ctx: &MiddlewareStageContext,
    input: StageInput,
) -> Result<Vec<StageOutcome>, MiddlewareError> {
    debug_assert_eq!(
        ctx.stage(),
        input.stage(),
        "stage context cursor and stage input must describe the same stage"
    );
    let stage = input.stage();
    let components: &[ResolvedMiddleware] = chain.stage(stage);
    let mut outcomes = Vec::with_capacity(components.len());
    for (index, resolved) in components.iter().enumerate() {
        let mw_ctx = ctx.middleware_context(index)?;
        let outcome = resolved.middleware.invoke(mw_ctx, input.clone()).await?;
        validate_stage_outcome(&resolved.descriptor, &input, &outcome)?;
        outcomes.push(outcome);
    }
    Ok(outcomes)
}

/// First terminal outcome in an ordered stage chain, short-circuiting every
/// component after it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageTerminal {
    /// The stage fails with a safe descriptor. Landable at every stage.
    Fail(Box<ErrorDescriptor>),
    /// The stage requests a bounded semantic retry. Landable only at
    /// `Stage::BeforeFinalize` — see [`MIDDLEWARE_STAGE_UNLANDABLE`].
    Retry(RetryDirective),
}

/// Pure left-to-right fold of one stage's ordered [`StageOutcome`] chain.
///
/// Aggregates the ordered results of a stage's component chain into exactly
/// one result, matching the kernel's model of the middleware boundary as one
/// `ReducerStageOutcome` per `(cycle, stage)` cursor. Construct with
/// [`StageFold::accumulate`].
///
/// This fold assumes every outcome already passed
/// [`validate_stage_outcome`] for its own component descriptor (as
/// [`invoke_middleware_stage`] guarantees); it does not re-check
/// per-component/role legality such as the single-compactor rule. What it
/// does check, independently, is whether the *aggregate* stage/outcome
/// combination has anywhere to land in the kernel, and whether the fold's own
/// accumulated content would exceed a kernel-enforced bound.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StageFold {
    /// `AddInstructions` contributions, concatenated in chain order so a
    /// later component's contribution sits after an earlier one's.
    pub instructions: Vec<ContextItem>,
    /// `AddContext` contributions, concatenated in chain order.
    pub context: Vec<ContextItem>,
    /// `FilterTools` narrowing, intersected across every component that
    /// emitted one — narrowing is monotone, so the result does not depend on
    /// component order. `None` means no component narrowed the tool set.
    pub retained_tools: Option<BTreeSet<ToolId>>,
    /// The last `Replace` value in chain order (`Replace` substitutes).
    pub replacement: Option<RawJson>,
    /// The unique compactor's `CompactContext` result, if any. At most one
    /// component may hold the `ContextCompactor` role in a resolved chain, so
    /// at most one `CompactContext` outcome can appear in a stage's chain.
    pub compaction: Option<Box<CompactionResult>>,
    /// The first `Fail` or `Retry` in chain order. Once set, no later
    /// component in the chain is folded.
    pub terminal: Option<StageTerminal>,
}

impl StageFold {
    /// Fold `outcomes` for `stage` left to right into one aggregate result.
    ///
    /// `Continue` is a no-op. `AddInstructions`/`AddContext` concatenate in
    /// chain order. `FilterTools` intersects. `Replace` substitutes (last
    /// writer wins). The first `Fail` or `Retry` becomes the [`StageTerminal`]
    /// and short-circuits every later component — the rest of the chain is
    /// never inspected, so a later outcome's own legality is irrelevant once
    /// an earlier terminal has won.
    ///
    /// # Errors
    ///
    /// Returns [`MIDDLEWARE_STAGE_UNLANDABLE`] the moment it encounters an
    /// outcome with no kernel landing path at `stage`: `Suspend`, `Complete`,
    /// `RequestInteraction`, and `RequestCompactionModel` unconditionally;
    /// `Retry` and `Replace` at any stage other than the ones the kernel
    /// admits them at; `CompactContext` outside `Stage::BeforeModel`.
    ///
    /// Returns [`MIDDLEWARE_STAGE_BOUNDS_EXCEEDED`] when the accumulated
    /// `instructions` and `context` (plus, at `Stage::BeforeModel`, any
    /// `CompactContext` replacement messages) would exceed the kernel's
    /// array bound for the landed outcome at `Stage::PrepareContext` or
    /// `Stage::BeforeModel`. Skipped entirely once a terminal has won, since
    /// a terminal outcome never lands as `ContextPrepared`/`ModelRequestPrepared`.
    pub fn accumulate(stage: Stage, outcomes: &[StageOutcome]) -> Result<Self, MiddlewareError> {
        let mut fold = Self::default();
        for outcome in outcomes {
            match outcome {
                StageOutcome::Continue => {}
                StageOutcome::AddInstructions(items) => {
                    fold.instructions.extend(items.iter().cloned());
                }
                StageOutcome::AddContext(items) => {
                    fold.context.extend(items.iter().cloned());
                }
                StageOutcome::FilterTools(ids) => {
                    let incoming: BTreeSet<ToolId> = ids.iter().cloned().collect();
                    fold.retained_tools = Some(match fold.retained_tools.take() {
                        Some(existing) => existing.intersection(&incoming).cloned().collect(),
                        None => incoming,
                    });
                }
                StageOutcome::Replace(raw) => {
                    if matches!(stage, Stage::PrepareContext | Stage::BeforeModel) {
                        fold.replacement = Some(raw.clone());
                    } else {
                        return Err(unlandable(
                            "middleware Replace has no kernel landing shape at this stage",
                        ));
                    }
                }
                StageOutcome::CompactContext(result) => {
                    if stage == Stage::BeforeModel {
                        fold.compaction = Some(result.clone());
                    } else {
                        return Err(unlandable(
                            "middleware CompactContext has no kernel landing shape at this stage",
                        ));
                    }
                }
                StageOutcome::RequestCompactionModel(_) => {
                    return Err(unlandable(
                        "middleware RequestCompactionModel has no StageSettled landing path",
                    ));
                }
                StageOutcome::RequestInteraction(_) => {
                    return Err(unlandable(
                        "middleware RequestInteraction does not consume a stage cursor",
                    ));
                }
                StageOutcome::Suspend(_) => {
                    return Err(unlandable(
                        "middleware Suspend has no StageSettled landing path",
                    ));
                }
                StageOutcome::Complete(_) => {
                    return Err(unlandable(
                        "middleware Complete has no ReducerStageOutcome peer",
                    ));
                }
                StageOutcome::Retry(directive) => {
                    if stage == Stage::BeforeFinalize {
                        fold.terminal = Some(StageTerminal::Retry(directive.clone()));
                    } else {
                        return Err(unlandable(
                            "the kernel admits ReducerStageOutcome::Retry only at BeforeFinalize",
                        ));
                    }
                    break;
                }
                StageOutcome::Fail(descriptor) => {
                    fold.terminal = Some(StageTerminal::Fail(descriptor.clone()));
                    break;
                }
            }
        }
        fold.check_bounds(stage)?;
        Ok(fold)
    }

    /// Whether this fold leaves the stage's base outcome unperturbed.
    ///
    /// True exactly when no component contributed anything: no added
    /// instructions or context, no tool narrowing, no replacement or
    /// compaction, and no terminal. A caller can use this to skip landing an
    /// aggregate outcome entirely and proceed as if no middleware ran.
    #[must_use]
    pub fn is_identity(&self) -> bool {
        self.instructions.is_empty()
            && self.context.is_empty()
            && self.retained_tools.is_none()
            && self.replacement.is_none()
            && self.compaction.is_none()
            && self.terminal.is_none()
    }

    /// Bounds-check the accumulated additions once folding is complete.
    ///
    /// A no-op once a terminal has won: a `Fail`/`Retry` outcome never lands
    /// as `ContextPrepared`/`ModelRequestPrepared`, so the partial
    /// instructions/context accumulated before the short-circuit are moot.
    fn check_bounds(&self, stage: Stage) -> Result<(), MiddlewareError> {
        if self.terminal.is_some() {
            return Ok(());
        }
        let added = self.instructions.len() + self.context.len();
        match stage {
            Stage::PrepareContext if added > finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS => {
                Err(bounds_exceeded(
                    "aggregate PrepareContext instructions/context exceed the semantic array bound",
                ))
            }
            Stage::BeforeModel => {
                let compacted = self
                    .compaction
                    .as_ref()
                    .map_or(0, |result| result.replacement_messages.len());
                if added + compacted > ModelRequestDraft::MAX_MESSAGES {
                    Err(bounds_exceeded(
                        "aggregate BeforeModel additions exceed the model request message bound",
                    ))
                } else {
                    Ok(())
                }
            }
            _ => Ok(()),
        }
    }
}

fn unlandable(reason: &'static str) -> MiddlewareError {
    MiddlewareError::stable(MIDDLEWARE_STAGE_UNLANDABLE, reason)
}

fn bounds_exceeded(reason: &'static str) -> MiddlewareError {
    MiddlewareError::stable(MIDDLEWARE_STAGE_BOUNDS_EXCEEDED, reason)
}

/// Handle for one resolved middleware chain's stage boundaries.
///
/// Holds the locked chain and the run-scoped cancellation signal shared by
/// every component invocation in the run. [`StageDriver::is_active`] answers
/// whether a stage has any component to run at all; [`StageDriver::run_stage`]
/// runs one.
#[derive(Clone)]
pub struct StageDriver {
    chain: Arc<ResolvedMiddlewareChain>,
    cancellation: CancellationSignal,
}

impl StageDriver {
    /// Construct a driver over a locked chain and its run's cancellation signal.
    #[must_use]
    pub fn new(chain: Arc<ResolvedMiddlewareChain>, cancellation: CancellationSignal) -> Self {
        Self {
            chain,
            cancellation,
        }
    }

    /// The locked, resolved middleware chain.
    #[must_use]
    pub fn chain(&self) -> &Arc<ResolvedMiddlewareChain> {
        &self.chain
    }

    /// Whether any component is registered for `stage`.
    ///
    /// The passthrough gate: `false` means the caller should skip the
    /// aggregate fold entirely and proceed as if no middleware were
    /// configured for this stage.
    #[must_use]
    pub fn is_active(&self, stage: Stage) -> bool {
        !self.chain.stage(stage).is_empty()
    }

    /// The run-scoped cancellation signal shared by every component invocation.
    #[must_use]
    pub fn cancellation(&self) -> &CancellationSignal {
        &self.cancellation
    }

    /// Invoke `stage`'s ordered chain against this driver's locked chain.
    ///
    /// Thin binding of [`invoke_middleware_stage`] to the chain and
    /// cancellation this driver owns, plus one behaviour of its own: a run that
    /// is already cancelled runs **no** component and returns an empty outcome
    /// list, which folds to the identity and leaves the caller's base outcome
    /// byte-for-byte unchanged. Cancellation must be able to skip optional
    /// work; it must never be able to *change* what a stage settles, because
    /// the kernel — not the driver — owns cancellation's effect on the run.
    ///
    /// # Errors
    ///
    /// Returns the component's own `MiddlewareError`, or a stable
    /// `middleware_outcome_not_allowed` when an outcome fails the stage matrix.
    pub async fn run_stage(
        &self,
        ctx: &MiddlewareStageContext,
        input: StageInput,
    ) -> Result<Vec<StageOutcome>, MiddlewareError> {
        if self.cancellation.is_cancelled() {
            return Ok(Vec::new());
        }
        invoke_middleware_stage(&self.chain, ctx, input).await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use finstack_ai_kernel::{
        ComponentId, ComponentRef, ContentBlock, EffectTag, ErrorCategory, ErrorDescriptor, Id,
        IdTag, InteractionKind, InteractionRequest, InteractionTag, LaneTag, OperationLocator,
        RawJson, RetryClassification, RetryDirective, RunTag, Sensitivity, SessionTag, Stage,
        StageCursor, TextBlock, ToolId, Version,
    };

    use super::{
        MIDDLEWARE_STAGE_BOUNDS_EXCEEDED, MIDDLEWARE_STAGE_UNLANDABLE, MiddlewareStageContext,
        RunCallContext, StageFold, StageTerminal, derived_stage_effect_id,
    };
    use crate::context::{ContextAuthority, ContextItem, ContextItemKind, ContextProvenance};
    use crate::middleware::{
        CompactionEvidence, CompactionModelRequest, CompactionResult, MiddlewareDescriptor,
        MiddlewareOrder, MiddlewareRegistration, MiddlewareRole, OrderTier, PromptCacheImpact,
        ResolvedMiddlewareChain, StageMask, StageOutcome,
    };
    use crate::model::{
        CancellationSignal, ModelName, ModelRequestDraft, ModelRequestLimits, ModelSettings,
    };

    fn id<T: IdTag>(value: u64) -> Id<T> {
        let mut bytes = [0_u8; 16];
        bytes[6] = 0x70;
        bytes[8] = 0x80;
        bytes[9..].copy_from_slice(&value.to_be_bytes()[1..]);
        Id::from_bytes(bytes)
    }

    fn item(text: &str) -> ContextItem {
        ContextItem::try_new(
            ContextItemKind::Instruction,
            vec![ContentBlock::Text(TextBlock::try_new(text).expect("text"))],
            ContextProvenance {
                source_id: Arc::from("fixture.source"),
                source_ref: None,
                external: false,
            },
            ContextAuthority::TrustedApplication,
            0,
            4,
            Sensitivity::Internal,
            false,
        )
        .expect("item")
    }

    fn item_text(value: &ContextItem) -> &str {
        match value.content.first() {
            Some(ContentBlock::Text(text)) => text.text(),
            other => panic!("expected a single text content block, got {other:?}"),
        }
    }

    fn tool_id(name: &str) -> ToolId {
        ToolId::parse(format!("fixture.{name}")).expect("tool id")
    }

    fn error(code: &str) -> Box<ErrorDescriptor> {
        Box::new(
            ErrorDescriptor::new(code, "fixture failure", ErrorCategory::Middleware, false)
                .expect("error descriptor"),
        )
    }

    fn retry_directive() -> RetryDirective {
        RetryDirective::try_new(
            RetryClassification::Framework,
            finstack_ai_kernel::Duration::from_millis(10),
            "fixture-policy-v1",
        )
        .expect("retry directive")
    }

    fn model_draft(messages: Arc<[finstack_ai_kernel::Message]>) -> ModelRequestDraft {
        ModelRequestDraft {
            model: ModelName::try_new("fixture-model").expect("model"),
            messages,
            tools: Arc::from([]),
            output: finstack_ai_kernel::OutputSpec::PlainText,
            settings: ModelSettings {
                values: RawJson::parse(b"{}").expect("settings"),
            },
            limits: ModelRequestLimits {
                max_input_bytes: 1_000_000,
                max_input_tokens: 10_000,
                max_output_tokens: 1_000,
            },
        }
    }

    /// A structurally valid `CompactionResult` whose semantic correctness
    /// (protected-content preservation, tool-pair atomicity, digest
    /// integrity...) is irrelevant here: the pure fold treats `CompactContext`
    /// opaquely, trusting that `validate_stage_outcome` already checked it.
    fn compaction_result(replacement_message_count: usize) -> CompactionResult {
        let replacement_messages: Arc<[finstack_ai_kernel::Message]> = (0
            ..replacement_message_count)
            .map(|ordinal| {
                finstack_ai_kernel::Message::try_new(
                    id(ordinal as u64),
                    finstack_ai_kernel::MessageRole::User,
                    vec![ContentBlock::Text(TextBlock::try_new("m").expect("text"))],
                    finstack_ai_kernel::Timestamp::from_unix_ms(1).expect("timestamp"),
                    None,
                    finstack_ai_kernel::ProviderIds::empty(),
                    finstack_ai_kernel::Metadata::empty(),
                )
                .expect("message")
            })
            .collect::<Vec<_>>()
            .into();
        CompactionResult {
            replacement_messages,
            derived_summaries: Arc::from([]),
            evidence: CompactionEvidence {
                strategy_id: Arc::from("fixture.strategy"),
                strategy_version: 1,
                configuration_digest: finstack_ai_kernel::Digest::raw_json(b"cfg"),
                model_context_profile_digest: finstack_ai_kernel::Digest::raw_json(b"profile"),
                source_digest: finstack_ai_kernel::Digest::raw_json(b"source"),
                protected_item_set_digest: finstack_ai_kernel::Digest::raw_json(b"protected"),
                covered_entry_ids: Arc::from([]),
                retained_entry_ids: Arc::from([]),
                projection_digest: finstack_ai_kernel::Digest::raw_json(b"projection"),
                estimated_tokens_before: 10,
                estimated_tokens_after: 5,
                summary_digest: None,
                cache_impact: PromptCacheImpact::CacheInvalidated,
            },
            checkpoint: None,
        }
    }

    fn interaction_request() -> Box<InteractionRequest> {
        let policy_component = ComponentId::parse("fixture.policy").expect("component");
        let version = Version {
            major: 1,
            minor: 0,
            patch: 0,
        };
        Box::new(
            InteractionRequest::try_new(
                1,
                id::<InteractionTag>(1),
                id::<EffectTag>(2),
                InteractionKind::Approval,
                vec![ContentBlock::Text(
                    TextBlock::try_new("approve?").expect("prompt"),
                )],
                RawJson::parse(
                    br#"{"additionalProperties":false,"properties":{"approved":{"type":"boolean"}},"required":["approved"],"type":"object"}"#,
                )
                .expect("schema"),
                ComponentRef::new(policy_component, Some(version)),
                version,
                None,
                None,
                false,
                finstack_ai_kernel::Metadata::empty(),
            )
            .expect("interaction request"),
        )
    }

    fn compaction_model_request() -> Box<CompactionModelRequest> {
        Box::new(CompactionModelRequest {
            model: ComponentRef::new(
                ComponentId::parse("fixture.child-model").expect("component"),
                None,
            ),
            request: model_draft(Arc::from([])),
            budget_scope_id: id(9),
            source_sensitivity: Sensitivity::Internal,
            residency_policy_digest: finstack_ai_kernel::Digest::raw_json(b"residency"),
            resume_state: RawJson::parse(b"{}").expect("resume"),
        })
    }

    const ALL_STAGES: [Stage; 7] = [
        Stage::BeforeRun,
        Stage::PrepareContext,
        Stage::BeforeModel,
        Stage::AfterModel,
        Stage::BeforeToolBatch,
        Stage::AfterToolBatch,
        Stage::BeforeFinalize,
    ];

    #[test]
    fn stage_names_round_trip() {
        for stage in ALL_STAGES {
            let name = crate::middleware::stage_name(stage);
            assert_eq!(crate::middleware::parse_stage(name), Some(stage));
        }
    }

    #[test]
    fn empty_outcomes_fold_to_identity() {
        let fold = StageFold::accumulate(Stage::PrepareContext, &[]).expect("fold");
        assert!(
            fold.is_identity(),
            "an empty chain must not perturb the base outcome"
        );
    }

    #[test]
    fn add_instructions_and_add_context_each_accumulate_in_chain_order_and_stay_separate() {
        let fold = StageFold::accumulate(
            Stage::PrepareContext,
            &[
                StageOutcome::AddInstructions(Arc::from([item("system-first")])),
                StageOutcome::AddContext(Arc::from([item("context-first")])),
                StageOutcome::AddInstructions(Arc::from([item("system-second")])),
                StageOutcome::AddContext(Arc::from([item("context-second")])),
            ],
        )
        .expect("fold");
        assert_eq!(
            fold.instructions.iter().map(item_text).collect::<Vec<_>>(),
            vec!["system-first", "system-second"],
            "later components append after earlier, and instructions never mix into context"
        );
        assert_eq!(
            fold.context.iter().map(item_text).collect::<Vec<_>>(),
            vec!["context-first", "context-second"],
            "later components append after earlier, and context never mixes into instructions"
        );
        assert!(!fold.is_identity());
    }

    #[test]
    fn filter_tools_intersects_so_order_cannot_matter() {
        let ab = StageFold::accumulate(
            Stage::BeforeModel,
            &[
                StageOutcome::FilterTools(Arc::from([tool_id("a"), tool_id("b")])),
                StageOutcome::FilterTools(Arc::from([tool_id("b"), tool_id("c")])),
            ],
        )
        .expect("fold");
        let ba = StageFold::accumulate(
            Stage::BeforeModel,
            &[
                StageOutcome::FilterTools(Arc::from([tool_id("b"), tool_id("c")])),
                StageOutcome::FilterTools(Arc::from([tool_id("a"), tool_id("b")])),
            ],
        )
        .expect("fold");
        assert_eq!(
            ab.retained_tools, ba.retained_tools,
            "intersection must be order-independent"
        );
        assert_eq!(
            ab.retained_tools.expect("narrowed").len(),
            1,
            "only b survives"
        );
    }

    #[test]
    fn filter_tools_is_landable_at_before_tool_batch_too() {
        let fold = StageFold::accumulate(
            Stage::BeforeToolBatch,
            &[StageOutcome::FilterTools(Arc::from([tool_id("a")]))],
        )
        .expect("fold");
        assert_eq!(fold.retained_tools.expect("narrowed").len(), 1);
    }

    #[test]
    fn first_terminal_short_circuits_the_rest_of_the_chain() {
        let fold = StageFold::accumulate(
            Stage::AfterModel,
            &[
                StageOutcome::Fail(error("first_failure")),
                StageOutcome::Fail(error("second_failure")),
            ],
        )
        .expect("fold");
        match fold.terminal {
            Some(StageTerminal::Fail(descriptor)) => {
                assert_eq!(descriptor.code.as_str(), "first_failure");
            }
            other => panic!("expected first Fail to win, got {other:?}"),
        }
    }

    #[test]
    fn fail_is_landable_at_every_stage() {
        for stage in ALL_STAGES {
            let fold = StageFold::accumulate(stage, &[StageOutcome::Fail(error("boom"))])
                .unwrap_or_else(|error| panic!("Fail must land at {stage:?}: {error}"));
            assert!(matches!(fold.terminal, Some(StageTerminal::Fail(_))));
        }
    }

    #[test]
    fn retry_lands_only_at_before_finalize() {
        let fold = StageFold::accumulate(
            Stage::BeforeFinalize,
            &[StageOutcome::Retry(retry_directive())],
        )
        .expect("Retry lands at BeforeFinalize");
        assert!(matches!(fold.terminal, Some(StageTerminal::Retry(_))));

        for stage in [Stage::AfterModel, Stage::AfterToolBatch] {
            let error = StageFold::accumulate(stage, &[StageOutcome::Retry(retry_directive())])
                .expect_err("the kernel admits Retry only at BeforeFinalize");
            assert_eq!(error.code(), MIDDLEWARE_STAGE_UNLANDABLE);
        }
    }

    #[test]
    fn terminal_short_circuits_before_a_bounds_violation_is_reached() {
        let oversized: Arc<[ContextItem]> = (0..=finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS)
            .map(|_| item("x"))
            .collect::<Vec<_>>()
            .into();
        let fold = StageFold::accumulate(
            Stage::PrepareContext,
            &[
                StageOutcome::Fail(error("stop_here")),
                StageOutcome::AddContext(oversized),
            ],
        )
        .expect("the terminal short-circuits before the oversized AddContext is ever folded");
        assert!(matches!(fold.terminal, Some(StageTerminal::Fail(_))));
        assert!(fold.context.is_empty());
    }

    #[test]
    fn replace_last_writer_wins() {
        let fold = StageFold::accumulate(
            Stage::PrepareContext,
            &[
                StageOutcome::Replace(RawJson::parse(b"{\"v\":1}").expect("a")),
                StageOutcome::Replace(RawJson::parse(b"{\"v\":2}").expect("b")),
            ],
        )
        .expect("fold");
        assert_eq!(
            fold.replacement,
            Some(RawJson::parse(b"{\"v\":2}").expect("b")),
            "Replace substitutes; the later component wins"
        );
    }

    #[test]
    fn replace_lands_at_prepare_context_and_before_model_only() {
        for stage in [Stage::PrepareContext, Stage::BeforeModel] {
            StageFold::accumulate(
                stage,
                &[StageOutcome::Replace(
                    RawJson::parse(b"{}").expect("replacement"),
                )],
            )
            .unwrap_or_else(|error| panic!("Replace must land at {stage:?}: {error}"));
        }
        for stage in [
            Stage::BeforeRun,
            Stage::AfterModel,
            Stage::BeforeToolBatch,
            Stage::AfterToolBatch,
        ] {
            let error = StageFold::accumulate(
                stage,
                &[StageOutcome::Replace(
                    RawJson::parse(b"{}").expect("replacement"),
                )],
            )
            .expect_err("no ReducerStageOutcome at this stage can carry a replaced raw value");
            assert_eq!(error.code(), MIDDLEWARE_STAGE_UNLANDABLE);
        }
    }

    #[test]
    fn compact_context_lands_only_at_before_model() {
        let fold = StageFold::accumulate(
            Stage::BeforeModel,
            &[StageOutcome::CompactContext(Box::new(compaction_result(2)))],
        )
        .expect("CompactContext lands at BeforeModel");
        assert!(fold.compaction.is_some());

        for stage in [Stage::PrepareContext, Stage::AfterModel] {
            let error = StageFold::accumulate(
                stage,
                &[StageOutcome::CompactContext(Box::new(compaction_result(0)))],
            )
            .expect_err("CompactContext has no landing shape outside BeforeModel");
            assert_eq!(error.code(), MIDDLEWARE_STAGE_UNLANDABLE);
        }
    }

    #[test]
    fn suspend_complete_request_interaction_and_request_compaction_model_are_always_unlandable() {
        let cases: Vec<(Stage, StageOutcome)> = vec![
            (
                Stage::AfterToolBatch,
                StageOutcome::Suspend(error("suspend_me")),
            ),
            (
                Stage::BeforeRun,
                StageOutcome::Complete(RawJson::parse(b"{}").expect("complete")),
            ),
            (
                Stage::BeforeFinalize,
                StageOutcome::Complete(RawJson::parse(b"{}").expect("complete")),
            ),
            (
                Stage::BeforeFinalize,
                StageOutcome::RequestInteraction(interaction_request()),
            ),
            (
                Stage::BeforeModel,
                StageOutcome::RequestCompactionModel(compaction_model_request()),
            ),
        ];
        for (stage, outcome) in cases {
            let error = StageFold::accumulate(stage, std::slice::from_ref(&outcome))
                .expect_err("has no kernel landing at any stage");
            assert_eq!(error.code(), MIDDLEWARE_STAGE_UNLANDABLE);
        }
    }

    #[test]
    fn unlandable_outcome_is_a_stable_error_not_a_silent_drop() {
        let error = StageFold::accumulate(
            Stage::BeforeRun,
            &[StageOutcome::Complete(RawJson::parse(b"{}").unwrap())],
        )
        .expect_err("Complete has no kernel landing at BeforeRun");
        assert_eq!(error.code(), MIDDLEWARE_STAGE_UNLANDABLE);
    }

    #[test]
    fn prepare_context_bounds_exceeded_is_a_stable_error() {
        let items: Arc<[ContextItem]> = (0..=finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS)
            .map(|_| item("x"))
            .collect::<Vec<_>>()
            .into();
        let error =
            StageFold::accumulate(Stage::PrepareContext, &[StageOutcome::AddContext(items)])
                .expect_err("one component alone exceeding the bound must not silently pass");
        assert_eq!(error.code(), MIDDLEWARE_STAGE_BOUNDS_EXCEEDED);
    }

    #[test]
    fn prepare_context_at_exactly_the_bound_is_not_exceeded() {
        let items: Arc<[ContextItem]> = (0..finstack_ai_kernel::SEMANTIC_ARRAY_MAX_ITEMS)
            .map(|_| item("x"))
            .collect::<Vec<_>>()
            .into();
        StageFold::accumulate(Stage::PrepareContext, &[StageOutcome::AddContext(items)])
            .expect("exactly the bound must still be admissible");
    }

    #[test]
    fn before_model_bounds_account_for_compaction_replacement_messages_too() {
        let half = ModelRequestDraft::MAX_MESSAGES / 2;
        let items: Arc<[ContextItem]> = (0..=half).map(|_| item("x")).collect::<Vec<_>>().into();
        let error = StageFold::accumulate(
            Stage::BeforeModel,
            &[
                StageOutcome::CompactContext(Box::new(compaction_result(half))),
                StageOutcome::AddContext(items),
            ],
        )
        .expect_err("compaction replacement messages plus additions must both count");
        assert_eq!(error.code(), MIDDLEWARE_STAGE_BOUNDS_EXCEEDED);
    }

    fn standard_descriptor(component: &str, stage: Stage) -> MiddlewareDescriptor {
        MiddlewareDescriptor {
            invocation: finstack_ai_kernel::ComponentInvocation {
                component: ComponentId::parse(component).expect("component"),
                version: Version {
                    major: 1,
                    minor: 0,
                    patch: 0,
                },
                configuration_digest: finstack_ai_kernel::Digest::raw_json(b"{}"),
                recovery: finstack_ai_kernel::InvocationRecovery::RecomputeSafe,
            },
            stages: StageMask::from_stages([stage]),
            order: MiddlewareOrder {
                tier: OrderTier::Standard,
                priority: 0,
                before: Arc::from([]),
                after: Arc::from([]),
            },
            role: MiddlewareRole::Standard,
            metadata: finstack_ai_kernel::Metadata::empty(),
        }
    }

    struct Stub {
        descriptor: MiddlewareDescriptor,
    }

    impl crate::middleware::Middleware for Stub {
        fn descriptor(&self) -> MiddlewareDescriptor {
            self.descriptor.clone()
        }

        fn invoke(
            &self,
            _ctx: crate::middleware::MiddlewareContext,
            _input: crate::middleware::StageInput,
        ) -> crate::PortFuture<Result<StageOutcome, crate::middleware::MiddlewareError>> {
            Box::pin(async { Ok(StageOutcome::Continue) })
        }
    }

    fn one_component_chain() -> Arc<ResolvedMiddlewareChain> {
        let stub: Arc<dyn crate::middleware::Middleware> = Arc::new(Stub {
            descriptor: standard_descriptor("fixture.only", Stage::BeforeRun),
        });
        Arc::new(
            ResolvedMiddlewareChain::try_new(vec![MiddlewareRegistration { middleware: stub }])
                .expect("chain"),
        )
    }

    #[test]
    fn stage_driver_is_active_reflects_registered_stages() {
        let driver = super::StageDriver::new(one_component_chain(), CancellationSignal::new());
        assert!(driver.is_active(Stage::BeforeRun));
        assert!(
            !driver.is_active(Stage::PrepareContext),
            "no component is registered for PrepareContext"
        );
    }

    #[test]
    fn stage_driver_exposes_the_locked_chain() {
        let chain = one_component_chain();
        let digest = chain.digest();
        let driver = super::StageDriver::new(Arc::clone(&chain), CancellationSignal::new());
        assert_eq!(driver.chain().digest(), digest);
    }

    #[test]
    fn stage_driver_cancellation_shares_the_underlying_signal() {
        let signal = CancellationSignal::new();
        let driver = super::StageDriver::new(one_component_chain(), signal.clone());
        assert!(!driver.cancellation().is_cancelled());
        signal.cancel();
        assert!(
            driver.cancellation().is_cancelled(),
            "CancellationSignal is a shared handle, not a snapshot"
        );
    }

    fn test_locator() -> OperationLocator {
        OperationLocator::try_new(
            "tenant-a",
            id::<SessionTag>(1),
            id::<LaneTag>(2),
            id::<RunTag>(3),
        )
        .expect("locator")
    }

    #[test]
    fn derived_stage_effect_id_is_deterministic_across_calls() {
        let locator = test_locator();
        let a = derived_stage_effect_id(&locator, 3, Stage::BeforeModel);
        let b = derived_stage_effect_id(&locator, 3, Stage::BeforeModel);
        assert_eq!(a, b, "derived id must be stable so replay reproduces it");
    }

    #[test]
    fn derived_stage_effect_id_separates_stage_and_cycle() {
        let locator = test_locator();
        let base = derived_stage_effect_id(&locator, 3, Stage::BeforeModel);
        assert_ne!(
            base,
            derived_stage_effect_id(&locator, 4, Stage::BeforeModel),
            "distinct cycles must not collide"
        );
        assert_ne!(
            base,
            derived_stage_effect_id(&locator, 3, Stage::AfterModel),
            "distinct stages must not collide"
        );
    }

    fn test_run_call_context() -> RunCallContext {
        let locator = test_locator();
        RunCallContext {
            effect_id: derived_stage_effect_id(&locator, 0, Stage::BeforeModel),
            locator,
            authorization: crate::model::AuthorizationContext {
                principal: finstack_ai_kernel::PrincipalRef::try_new(
                    "issuer",
                    "subject",
                    Some("tenant-a"),
                )
                .expect("principal"),
                authentication_method: Arc::from("fixture"),
                assurance_level: Arc::from("high"),
                roles: Arc::from([]),
                permitted_scopes: Arc::from([Arc::from("tenant-a")]),
                safe_claims: finstack_ai_kernel::Metadata::empty(),
                policy_version: Arc::from("v1"),
                decision_id: Arc::from("decision-1"),
            },
            attempt: 1,
            deadline: None,
            budget_scope_id: None,
            cancellation: CancellationSignal::new(),
        }
    }

    /// A component that counts its invocations, so a test can distinguish
    /// "ran and returned Continue" from "never ran".
    struct Counting {
        descriptor: MiddlewareDescriptor,
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl crate::middleware::Middleware for Counting {
        fn descriptor(&self) -> MiddlewareDescriptor {
            self.descriptor.clone()
        }

        fn invoke(
            &self,
            _ctx: crate::middleware::MiddlewareContext,
            _input: crate::middleware::StageInput,
        ) -> crate::PortFuture<Result<StageOutcome, crate::middleware::MiddlewareError>> {
            self.calls
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Box::pin(async { Ok(StageOutcome::Continue) })
        }
    }

    fn counting_chain(calls: &Arc<std::sync::atomic::AtomicUsize>) -> Arc<ResolvedMiddlewareChain> {
        let middleware: Arc<dyn crate::middleware::Middleware> = Arc::new(Counting {
            descriptor: standard_descriptor("fixture.counting", Stage::BeforeRun),
            calls: Arc::clone(calls),
        });
        Arc::new(
            ResolvedMiddlewareChain::try_new(vec![MiddlewareRegistration { middleware }])
                .expect("chain"),
        )
    }

    fn before_run_input() -> crate::middleware::StageInput {
        crate::middleware::StageInput::BeforeRun {
            value: RawJson::parse(b"[]").expect("value"),
        }
    }

    fn before_run_context() -> MiddlewareStageContext {
        MiddlewareStageContext::new(
            test_run_call_context(),
            finstack_ai_kernel::Digest::raw_json(b"{}"),
            StageCursor {
                cycle: 0,
                stage: Stage::BeforeRun,
            },
        )
    }

    fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                std::task::Poll::Ready(value) => return value,
                std::task::Poll::Pending => std::thread::yield_now(),
            }
        }
    }

    #[test]
    fn run_stage_invokes_every_component_registered_for_the_stage() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let driver = super::StageDriver::new(counting_chain(&calls), CancellationSignal::new());

        let outcomes = block_on(driver.run_stage(&before_run_context(), before_run_input()))
            .expect("chain runs");

        assert_eq!(outcomes, vec![StageOutcome::Continue]);
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::Relaxed),
            1,
            "the registered component must actually have been invoked"
        );
    }

    #[test]
    fn run_stage_on_a_cancelled_run_invokes_no_component_and_folds_to_identity() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let signal = CancellationSignal::new();
        signal.cancel();
        let driver = super::StageDriver::new(counting_chain(&calls), signal);

        let outcomes = block_on(driver.run_stage(&before_run_context(), before_run_input()))
            .expect("cancellation is not an error");

        assert!(outcomes.is_empty());
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::Relaxed),
            0,
            "a cancelled run must not invoke any component"
        );
        assert!(
            StageFold::accumulate(Stage::BeforeRun, &outcomes)
                .expect("fold")
                .is_identity(),
            "the skipped chain must leave the base outcome untouched"
        );
    }

    #[test]
    fn middleware_stage_context_stage_reflects_its_cursor() {
        let cursor = StageCursor {
            cycle: 2,
            stage: Stage::BeforeModel,
        };
        let ctx = MiddlewareStageContext::new(
            test_run_call_context(),
            finstack_ai_kernel::Digest::raw_json(b"{}"),
            cursor,
        );
        assert_eq!(ctx.stage(), Stage::BeforeModel);
        assert_eq!(ctx.cursor, cursor);
    }
}
