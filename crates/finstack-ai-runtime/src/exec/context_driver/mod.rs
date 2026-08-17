//! Production `ContextProvider` driver.
//!
//! Registered providers are invoked on the `prepare_context` / `before_model`
//! path through [`CommittedContextCall`] and [`assemble_context`]. The host-task
//! dispatcher also has a Context arm so a posted `EffectKind::Context` is not
//! `unsupported_effect_driver`.
//!
//! Individual provider envelopes are constructed from the run locator and
//! locked chain so [`CommittedContextCall::try_new`] can prove the call. The
//! kernel does not emit `EffectRequested(Context)` at `PrepareContext` (journal
//! meaning is frozen); the guard still binds provider, request, locator, and
//! cursor before `collect` runs.

mod collect;
mod commit;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;
use std::sync::Arc;

use finstack_ai_kernel::{EntryId, Sensitivity};

use crate::context::{AssembledContext, ContextProvider};
use crate::model::CancellationSignal;

pub(crate) use collect::{collect_context_stage, structural_protected};

/// Locked provider list installed on the coordinator for one run.
#[derive(Clone)]
pub(crate) struct ContextDriver {
    providers: Arc<[Arc<dyn ContextProvider>]>,
    cancellation: CancellationSignal,
}

impl ContextDriver {
    /// Construct a driver over the installed providers and run cancellation.
    #[must_use]
    pub(crate) fn new(
        providers: Arc<[Arc<dyn ContextProvider>]>,
        cancellation: CancellationSignal,
    ) -> Self {
        Self {
            providers,
            cancellation,
        }
    }

    /// Installed providers in locked order.
    #[must_use]
    pub(crate) fn providers(&self) -> &[Arc<dyn ContextProvider>] {
        &self.providers
    }

    /// Run-scoped cancellation shared with stage settlement.
    #[must_use]
    pub(crate) fn cancellation(&self) -> &CancellationSignal {
        &self.cancellation
    }
}

/// Authoritative `protected` / sensitivity projection for one source entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProtectedProjection {
    by_entry: BTreeMap<EntryId, (bool, Sensitivity)>,
}

impl ProtectedProjection {
    pub(crate) fn new(by_entry: BTreeMap<EntryId, (bool, Sensitivity)>) -> Self {
        Self { by_entry }
    }

    pub(crate) fn into_map(self) -> BTreeMap<EntryId, (bool, Sensitivity)> {
        self.by_entry
    }
}

/// Empty assembled context used when no provider contributed.
#[must_use]
pub(crate) fn empty_assembled() -> AssembledContext {
    AssembledContext {
        items: Arc::from([]),
        diagnostics: Arc::from([]),
        estimated_tokens: 0,
        bytes: 0,
    }
}
