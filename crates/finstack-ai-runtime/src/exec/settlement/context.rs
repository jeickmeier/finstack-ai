use crate::context::{ContextProvider, InvocationResumeAction};
use crate::coordinator::CommitCoordinator;
use crate::run_types::RunHandleError;
use crate::{CancellationSignal, Clock, RandomSource};

use super::SettlementSources;

#[cfg_attr(
    feature = "native-tokio",
    allow(dead_code, reason = "host-task recover consumes this under wasm-host")
)]
/// Recover outstanding context effects after a crash between request and collect.
///
/// Collect remains an in-memory commit guard in 1.0, so this owner hook is a
/// no-op unless a later journaled `EffectRequested(Context)` appears. The
/// production resume path is [`crate::CommittedContextCall::resume`].
pub(crate) async fn resume_pending_context_effects<C: Clock, R: RandomSource>(
    coordinator: &CommitCoordinator,
    providers: &[std::sync::Arc<dyn ContextProvider>],
    sources: &SettlementSources<C, R>,
    cancellation: &CancellationSignal,
) -> Result<InvocationResumeAction, RunHandleError> {
    let _ = (coordinator, providers, sources, cancellation);
    Ok(InvocationResumeAction::UseRecorded)
}
