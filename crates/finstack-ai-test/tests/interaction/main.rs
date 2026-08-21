//! interaction contract typed-interaction crash, router, and envelope proofs.

use std::sync::Arc;

use finstack_ai_kernel::{
    AllocatedIds, CancelRequested, CancellationInitiator, CancellationRequestTag,
    InteractionCancelled, InteractionId, InteractionKind, InteractionSettled, InteractionTag,
    InteractionTerminalOutcome, KernelInput, RawJson, RequestInteraction, RunPhase, TransitionEnv,
};
use finstack_ai_runtime::{
    CommitCoordinator, ExternalRouteOutcome, InteractionResumeAction, RunTaskOwner,
    interaction_resume_action,
};

mod helpers;
use helpers::*;

include!("approval_flow.rs");
include!("envelope.rs");
include!("cancel.rs");
include!("projection.rs");
include!("schema.rs");
