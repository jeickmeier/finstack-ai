use finstack_ai_kernel::{
    AllocatedIds, AppendBatchTag, Digest, EffectCompleted, EffectFailed, EffectInput, EffectKind,
    EffectOutputContract, EffectOutputKind, EffectRequested, EventTag, ExtensionEffectSettled,
    ExtensionSettlement, KernelInput, PipelinePosition, ProviderIds, RECORD_KIND_VERSION,
    RecordBody, RecordTag, RequestExtensionEffect, RetrySafety, StageCursor, TransitionEnv,
};

use crate::context::{
    CONTEXT_STAGE, CommittedContextCall, ContextCallContext, ContextError, ContextProvider,
    ContextProviderDescriptor, ContextRequest,
};
use crate::ids::{Clock, RandomSource};
use crate::ports::model::RunCallContext;
use crate::run_types::RunHandleError;
use crate::settlement::SettlementSources;

pub(crate) struct ContextInvocation<'a, C, R> {
    pub(crate) provider: &'a dyn ContextProvider,
    pub(crate) provider_index: u32,
    pub(crate) run: RunCallContext,
    pub(crate) request: ContextRequest,
    pub(crate) chain_digest: Digest,
    pub(crate) cursor: StageCursor,
    pub(crate) sources: &'a SettlementSources<C, R>,
}

/// Build the committed guard and invoke one locked provider.
///
/// # Errors
///
/// Returns a stable context or identity error when the envelope cannot be
/// constructed or the provider fails.
pub(crate) async fn committed_context_call<C: Clock, R: RandomSource>(
    coordinator: &mut crate::commit::CommitCoordinator,
    invocation: ContextInvocation<'_, C, R>,
) -> Result<crate::context::ContextContribution, RunHandleError> {
    let descriptor = invocation.provider.descriptor();
    let pipeline = PipelinePosition::try_new(
        invocation.chain_digest,
        CONTEXT_STAGE,
        invocation.provider_index,
    )
    .map_err(|_| context_stage_error(crate::ports::context::CONTEXT_CONFIGURATION_INVALID))?;
    let raw = invocation
        .request
        .to_raw_json()
        .map_err(|error| context_error(&error))?;
    let output_contract = EffectOutputContract {
        kind: EffectOutputKind::ContextContribution,
        schema_version: 1,
        schema_digest: Digest::raw_json(b"context-contribution-v1"),
    };
    let recovering = coordinator
        .state()
        .pending_extension_effect()
        .is_some_and(|pending| pending.requested.effect_id() == invocation.run.effect_id);
    let (requested, envelope) = if recovering {
        let pending = coordinator
            .state()
            .pending_extension_effect()
            .ok_or_else(|| context_stage_error(crate::ports::context::CONTEXT_COMMIT_REQUIRED))?;
        let envelope = coordinator
            .replayed_extension_envelope(invocation.run.effect_id)
            .cloned()
            .ok_or_else(|| context_stage_error(crate::ports::context::CONTEXT_COMMIT_REQUIRED))?;
        (pending.requested.clone(), envelope)
    } else {
        let requested = EffectRequested::try_new(
            invocation.run.effect_id,
            EffectKind::Context,
            None,
            Some(descriptor.invocation.clone()),
            Some(pipeline),
            output_contract,
            EffectInput::Context {
                cursor: invocation.cursor,
                request: raw,
            },
            RetrySafety::SafeToRetry,
            invocation.run.deadline,
        )
        .map_err(|_| context_stage_error(crate::ports::context::CONTEXT_COMMIT_REQUIRED))?;
        let env = request_environment(invocation.sources, &requested)?;
        let outcome = coordinator
            .submit(
                env,
                KernelInput::RequestExtensionEffect(RequestExtensionEffect {
                    requested: requested.clone(),
                }),
            )
            .await
            .map_err(RunHandleError::Coordinator)?;
        let envelope = outcome
            .committed
            .as_ref()
            .and_then(|batch| {
                batch.records.iter().find(|record| {
                    matches!(record.body(), RecordBody::EffectRequested(value) if value.effect_id() == requested.effect_id())
                })
            })
            .cloned()
            .ok_or_else(|| context_stage_error(crate::ports::context::CONTEXT_COMMIT_REQUIRED))?;
        (requested, envelope)
    };
    let context = ContextCallContext {
        run: invocation.run,
        provider_index: invocation.provider_index,
        chain_digest: invocation.chain_digest,
    };
    let call = CommittedContextCall::try_new(&envelope, context, invocation.request, &descriptor)
        .map_err(|error| context_error(&error))?;
    let result = if recovering {
        call.resume(invocation.provider).await
    } else {
        call.invoke(invocation.provider).await
    };
    let settlement = context_settlement(&requested, &result)?;
    coordinator
        .submit(
            settlement_environment(invocation.sources, &settlement)?,
            KernelInput::ExtensionEffectSettled(ExtensionEffectSettled {
                cursor: invocation.cursor,
                outcome: settlement,
            }),
        )
        .await
        .map_err(RunHandleError::Coordinator)?;
    result.map_err(|error| context_error(&error))
}

fn context_settlement(
    requested: &EffectRequested,
    result: &Result<crate::context::ContextContribution, ContextError>,
) -> Result<ExtensionSettlement, RunHandleError> {
    match result {
        Ok(contribution) => {
            let bytes = serde_json_canonicalizer::to_vec(contribution).map_err(|_| {
                context_stage_error(crate::ports::context::CONTEXT_CONTRIBUTION_INVALID)
            })?;
            let output = finstack_ai_kernel::RawJson::parse(bytes).map_err(|_| {
                context_stage_error(crate::ports::context::CONTEXT_CONTRIBUTION_INVALID)
            })?;
            Ok(ExtensionSettlement::Completed(
                EffectCompleted::try_new(
                    requested.effect_id(),
                    requested.output_contract().clone(),
                    output,
                    None,
                    Vec::new(),
                    ProviderIds::empty(),
                    None::<&str>,
                    None,
                )
                .map_err(|_| {
                    context_stage_error(crate::ports::context::CONTEXT_CONTRIBUTION_INVALID)
                })?,
            ))
        }
        Err(error) => Ok(ExtensionSettlement::Failed(
            EffectFailed::try_new(
                requested.effect_id(),
                requested.output_contract().clone(),
                error.descriptor(),
                None,
                None::<&str>,
            )
            .map_err(|_| {
                context_stage_error(crate::ports::context::CONTEXT_CONTRIBUTION_INVALID)
            })?,
        )),
    }
}

pub(crate) fn chain_digest(providers: &[std::sync::Arc<dyn ContextProvider>]) -> Digest {
    let descriptors: Vec<ContextProviderDescriptor> = providers
        .iter()
        .map(|provider| provider.descriptor())
        .collect();
    let bytes = serde_json_canonicalizer::to_vec(&descriptors).unwrap_or_else(|_| Vec::from(b"[]"));
    Digest::domain_separated("context-provider-chain", 1, &bytes)
        .unwrap_or_else(|_| Digest::raw_json(b"context-provider-chain"))
}

fn request_environment<C: Clock, R: RandomSource>(
    sources: &SettlementSources<C, R>,
    requested: &EffectRequested,
) -> Result<TransitionEnv, RunHandleError> {
    transition_env(
        sources,
        &RecordBody::EffectRequested(requested.clone()),
        vec![requested.effect_id()],
        crate::ports::context::CONTEXT_COMMIT_REQUIRED,
    )
}

fn settlement_environment<C: Clock, R: RandomSource>(
    sources: &SettlementSources<C, R>,
    settlement: &ExtensionSettlement,
) -> Result<TransitionEnv, RunHandleError> {
    let body = match settlement {
        ExtensionSettlement::Completed(value) => RecordBody::EffectCompleted(value.clone()),
        ExtensionSettlement::Failed(value) => RecordBody::EffectFailed(value.clone()),
    };
    transition_env(
        sources,
        &body,
        Vec::new(),
        crate::ports::context::CONTEXT_CONTRIBUTION_INVALID,
    )
}

/// Allocate one record, its derived events, and one batch id for `body`.
fn transition_env<C: Clock, R: RandomSource>(
    sources: &SettlementSources<C, R>,
    body: &RecordBody,
    effect_ids: Vec<finstack_ai_kernel::EffectId>,
    error_code: &'static str,
) -> Result<TransitionEnv, RunHandleError> {
    let event_count = body
        .derived_event_count(RECORD_KIND_VERSION)
        .map_err(|_| context_stage_error(error_code))?;
    Ok(TransitionEnv {
        now: sources.now()?,
        ids: AllocatedIds::try_new(
            vec![sources.generate::<RecordTag>()?],
            (0..event_count)
                .map(|_| sources.generate::<EventTag>())
                .collect::<Result<Vec<_>, _>>()?,
            effect_ids,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            vec![sources.generate::<AppendBatchTag>()?],
            Vec::new(),
        )
        .map_err(|_| context_stage_error(error_code))?,
    })
}

fn context_stage_error(code: &'static str) -> RunHandleError {
    RunHandleError::Middleware {
        code: std::sync::Arc::from(code),
    }
}

pub(crate) fn context_error(error: &ContextError) -> RunHandleError {
    RunHandleError::Middleware {
        code: std::sync::Arc::from(error.code().as_str()),
    }
}
