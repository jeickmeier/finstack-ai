//! Effect and interaction envelopes (contract section 12.3).

mod error;
mod interaction;
mod kinds;
mod lifecycle;

#[cfg(test)]
mod tests;

pub use error::EffectError;
pub use interaction::{
    InteractionCancelled, InteractionExpired, InteractionKind, InteractionRequest,
    InteractionResolution,
};
pub use kinds::{
    ComponentInvocation, EffectInput, EffectKind, EffectOutputContract, EffectOutputKind,
    EffectPurpose, EffectRelation, InvocationRecovery, NestedModelKind, PipelinePosition,
    RetrySafety,
};
pub use lifecycle::{
    EffectCancelled, EffectCompleted, EffectDeferred, EffectFailed, EffectRequested,
    ReconciliationPolicy,
};
