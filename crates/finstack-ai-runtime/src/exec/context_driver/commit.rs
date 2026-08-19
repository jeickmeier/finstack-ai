use finstack_ai_kernel::{
    Digest, EffectId, EffectInput, EffectKind, EffectOutputContract, EffectOutputKind,
    EffectRequested, EventTag, OperationLocator, PipelinePosition, RECORD_FORMAT_VERSION,
    RECORD_KIND_VERSION, RecordBody, RecordEnvelope, RecordTag, RetrySafety,
};

use crate::context::{
    CONTEXT_STAGE, CommittedContextCall, ContextCallContext, ContextError, ContextProvider,
    ContextProviderDescriptor, ContextRequest,
};
use crate::run_types::RunHandleError;
use crate::settlement::SettlementSources;
use crate::{Clock, RandomSource, RunCallContext};

use super::ContextDriver;

const DOMAIN_CONTEXT_INVOCATION: &str = "context-provider-invocation";

/// Build the committed guard and invoke one locked provider.
///
/// # Errors
///
/// Returns a stable context or identity error when the envelope cannot be
/// constructed or the provider fails.
pub(crate) async fn committed_context_call<C: Clock, R: RandomSource>(
    _driver: &ContextDriver,
    provider: &dyn ContextProvider,
    provider_index: u32,
    run: RunCallContext,
    request: ContextRequest,
    chain_digest: Digest,
    sources: &SettlementSources<C, R>,
) -> Result<crate::context::ContextContribution, RunHandleError> {
    let descriptor = provider.descriptor();
    let envelope = committed_envelope(
        &run,
        &descriptor,
        provider_index,
        chain_digest,
        &request,
        sources,
    )?;
    let context = ContextCallContext {
        run,
        provider_index,
        chain_digest,
    };
    let call = CommittedContextCall::try_new(&envelope, context, request, &descriptor)
        .map_err(|error| context_error(&error))?;
    call.invoke(provider)
        .await
        .map_err(|error| context_error(&error))
}

/// Domain-separated correlation id for one provider invocation.
#[must_use]
pub(crate) fn derived_context_effect_id(
    locator: &OperationLocator,
    cycle: u64,
    provider_index: u32,
) -> EffectId {
    let canonical =
        serde_json_canonicalizer::to_vec(&(locator, cycle, CONTEXT_STAGE, provider_index))
            .unwrap_or_else(|_| Vec::new());
    let digest = Digest::from_fixed_domain(DOMAIN_CONTEXT_INVOCATION, 1, &canonical);
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest.as_bytes()[..16]);
    EffectId::from_bytes(bytes)
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

fn committed_envelope<C: Clock, R: RandomSource>(
    run: &RunCallContext,
    descriptor: &ContextProviderDescriptor,
    provider_index: u32,
    chain_digest: Digest,
    request: &ContextRequest,
    sources: &SettlementSources<C, R>,
) -> Result<RecordEnvelope, RunHandleError> {
    let raw = request
        .to_raw_json()
        .map_err(|error| context_error(&error))?;
    let requested = EffectRequested::try_new(
        run.effect_id,
        EffectKind::Context,
        None,
        Some(descriptor.invocation.clone()),
        Some(
            PipelinePosition::try_new(chain_digest, CONTEXT_STAGE, provider_index).map_err(
                |_| RunHandleError::Middleware {
                    code: std::sync::Arc::from(crate::CONTEXT_CONFIGURATION_INVALID),
                },
            )?,
        ),
        EffectOutputContract {
            kind: EffectOutputKind::ContextContribution,
            schema_version: 1,
            schema_digest: Digest::raw_json(b"context-contribution-v1"),
        },
        EffectInput::Context { request: raw },
        RetrySafety::SafeToRetry,
        run.deadline,
    )
    .map_err(|_| RunHandleError::Middleware {
        code: std::sync::Arc::from(crate::CONTEXT_COMMIT_REQUIRED),
    })?;
    let body = RecordBody::EffectRequested(requested);
    let event_count =
        body.derived_event_count(RECORD_KIND_VERSION)
            .map_err(|_| RunHandleError::Middleware {
                code: std::sync::Arc::from(crate::CONTEXT_COMMIT_REQUIRED),
            })?;
    let mut events = Vec::with_capacity(event_count);
    for _ in 0..event_count {
        events.push(sources.generate::<EventTag>()?);
    }
    let now = sources.now()?;
    RecordEnvelope::try_new(
        RECORD_FORMAT_VERSION,
        RECORD_KIND_VERSION,
        sources.generate::<RecordTag>()?,
        run.locator.session_id,
        run.locator.lane_id,
        Some(run.locator.run_id),
        1,
        now,
        None,
        Digest::raw_json(b"context-request"),
        None,
        Digest::raw_json(b"context-request"),
        events,
        body,
    )
    .map_err(|_| RunHandleError::Middleware {
        code: std::sync::Arc::from(crate::CONTEXT_COMMIT_REQUIRED),
    })
}

pub(crate) fn context_error(error: &ContextError) -> RunHandleError {
    RunHandleError::Middleware {
        code: std::sync::Arc::from(error.code()),
    }
}
