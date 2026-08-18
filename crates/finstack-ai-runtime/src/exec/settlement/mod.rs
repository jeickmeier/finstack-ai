//! Shared commit-settlement helpers for native and host-driven run owners.

mod cancel;
mod context;
mod ids;
mod interaction;
mod model;
mod stage;
mod tool;

#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use finstack_ai_kernel::{EventId, Id, IdTag};

use crate::coordinator::{ModelDispatchSeed, ToolDispatchSeed};
use crate::run_types::RunHandleError;
use crate::tool::AssembledToolTerminal;
use crate::{
    Clock, IdGenerationError, LockedModelContextProfile, Model, ModelContextProfileOverride,
    ModelError, ModelRequestDraft, ModelTerminal, RandomSource, ToolError, UuidV7Generator,
    resolve_model_context_profile,
};

pub(crate) use cancel::drain_idle_cancellation;
#[cfg(feature = "native-tokio")]
pub(crate) use cancel::reconcile_cancelled_effect;
#[cfg_attr(
    feature = "native-tokio",
    allow(
        unused_imports,
        reason = "host-task recover consumes this under wasm-host"
    )
)]
pub(crate) use context::resume_pending_context_effects;
pub(crate) use interaction::apply_interaction_resume;
pub(crate) use model::{process_model_progress, process_model_result, resume_pending_model_effect};
pub(crate) use stage::{prepare_tool_batch_if_ready, stage_allocation};
pub(crate) use tool::{process_tool_progress, process_tool_result, resume_pending_tool_effects};

pub(crate) struct ModelDriverResult {
    pub(crate) seed: ModelDispatchSeed,
    pub(crate) draft: ModelRequestDraft,
    pub(crate) provider: Arc<str>,
    pub(crate) result: Result<ModelTerminal, ModelError>,
}

pub(crate) struct ToolDriverResult {
    pub(crate) seed: ToolDispatchSeed,
    pub(crate) result: Result<AssembledToolTerminal, ToolError>,
}

// --- extracted from task.rs 787-862 ---
pub(crate) struct SettlementSources<C, R> {
    clock: Arc<C>,
    random: R,
    progress_random: ProgressRandom,
}

impl<C: Clock, R: RandomSource> SettlementSources<C, R> {
    pub(crate) fn try_new(clock: C, random: R) -> Result<Self, RunHandleError> {
        let progress_random = ProgressRandom::try_new(&random)?;
        Ok(Self {
            clock: Arc::new(clock),
            random,
            progress_random,
        })
    }

    pub(crate) fn now(&self) -> Result<finstack_ai_kernel::Timestamp, RunHandleError> {
        self.clock.now().map_err(id_source_error)
    }

    #[cfg_attr(not(feature = "native-tokio"), allow(dead_code))]
    pub(crate) fn clock(&self) -> Arc<C> {
        Arc::clone(&self.clock)
    }

    pub(crate) fn generate<T: IdTag>(&self) -> Result<Id<T>, RunHandleError> {
        UuidV7Generator::new(self.clock.as_ref(), &self.random)
            .generate()
            .map_err(id_source_error)
    }

    pub(crate) fn generate_progress_event(&self) -> Result<EventId, RunHandleError> {
        UuidV7Generator::new(self.clock.as_ref(), &self.progress_random)
            .generate()
            .map_err(id_source_error)
    }
}

struct ProgressRandom {
    seed: [u8; 32],
    counter: AtomicU64,
}

impl ProgressRandom {
    fn try_new(random: &impl RandomSource) -> Result<Self, RunHandleError> {
        let mut seed = [0_u8; 32];
        random.fill_bytes(&mut seed).map_err(id_source_error)?;
        Ok(Self {
            seed,
            counter: AtomicU64::new(0),
        })
    }
}

impl RandomSource for ProgressRandom {
    fn fill_bytes(&self, bytes: &mut [u8]) -> Result<(), IdGenerationError> {
        let mut written = 0_usize;
        while written < bytes.len() {
            let counter = self
                .counter
                .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| {
                    IdGenerationError::Source("progress event entropy exhausted".into())
                })?;
            let mut material = [0_u8; 40];
            material[..32].copy_from_slice(&self.seed);
            material[32..].copy_from_slice(&counter.to_be_bytes());
            let digest = finstack_ai_kernel::Digest::raw_json(&material);
            let count = (bytes.len() - written).min(digest.as_bytes().len());
            bytes[written..written + count].copy_from_slice(&digest.as_bytes()[..count]);
            written += count;
        }
        Ok(())
    }
}

pub(crate) fn model_handle_error(error: &ModelError) -> RunHandleError {
    RunHandleError::Model {
        code: Arc::from(error.code()),
    }
}

pub(crate) fn tool_handle_error(error: &ToolError) -> RunHandleError {
    RunHandleError::Tool {
        code: Arc::from(error.code()),
    }
}

pub(crate) fn validate_model_binding(
    model: &dyn Model,
    profile: &LockedModelContextProfile,
) -> Result<(), RunHandleError> {
    let descriptor = model.descriptor();
    descriptor
        .validate()
        .map_err(|error| model_handle_error(&error))?;
    if descriptor.provider != profile.profile.provider
        || !descriptor.models.contains(&profile.profile.model)
    {
        return Err(RunHandleError::Model {
            code: Arc::from(crate::MODEL_PROFILE_INVALID),
        });
    }
    let provider = model.capabilities(&profile.profile.model).context_profile;
    let effective = &profile.profile;
    let overlay = ModelContextProfileOverride {
        hard_input_bytes: Some(effective.hard_input_bytes),
        context_window_tokens: Some(effective.context_window_tokens),
        max_output_tokens: Some(effective.max_output_tokens),
        reserved_output_tokens: Some(effective.reserved_output_tokens),
        provider_overhead_tokens: Some(effective.provider_overhead_tokens),
    };
    let relocked = resolve_model_context_profile(provider, Some(&overlay), None, false)
        .map_err(|error| model_handle_error(&error))?;
    if relocked != *profile {
        return Err(RunHandleError::Model {
            code: Arc::from(crate::MODEL_PROFILE_INVALID),
        });
    }
    Ok(())
}

pub(crate) fn id_source_error(_error: IdGenerationError) -> RunHandleError {
    RunHandleError::ModelSettlement {
        code: "model_settlement_id_source_failed",
    }
}
