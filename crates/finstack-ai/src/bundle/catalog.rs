//! In-memory catalog, optional host services, and retained composition recipes.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use finstack_ai_kernel::{BundleId, CapabilityId, ComponentId, RawJson};
use finstack_ai_runtime::{AgentInvoker, ArtifactStore, BudgetLedger};

use crate::{AgentSpec, CapabilitySpec};

use super::{BundleResolutionError, BundleSpec, LockedBundle, RequiredServices};

/// Direct optional non-primary service handles validated at construction.
#[derive(Clone, Default)]
pub struct RuntimeServices {
    /// Child-agent invocation service.
    pub agent_invoker: Option<Arc<dyn AgentInvoker>>,
    /// Shared-budget ledger service.
    pub budget_ledger: Option<Arc<dyn BudgetLedger>>,
    /// Scoped artifact service.
    pub artifact_store: Option<Arc<dyn ArtifactStore>>,
}

impl RuntimeServices {
    pub(super) fn validate(&self, required: RequiredServices) -> Result<(), BundleResolutionError> {
        for (is_required, is_present, name) in [
            (
                required.agent_invoker,
                self.agent_invoker.is_some(),
                "agent_invoker",
            ),
            (
                required.budget_ledger,
                self.budget_ledger.is_some(),
                "budget_ledger",
            ),
            (
                required.artifact_store,
                self.artifact_store.is_some(),
                "artifact_store",
            ),
        ] {
            if is_required && !is_present {
                return Err(BundleResolutionError::Missing {
                    item: Arc::from(name),
                });
            }
        }
        Ok(())
    }
}

/// In-memory exact-installation catalog; one bundle version per ID.
#[derive(Default)]
pub struct BundleCatalog {
    bundles: BTreeMap<BundleId, Arc<BundleSpec>>,
}

impl BundleCatalog {
    /// Install one validated exact bundle.
    ///
    /// # Errors
    ///
    /// Rejects malformed bundles and duplicate bundle IDs rather than solving versions.
    pub fn install(&mut self, bundle: BundleSpec) -> Result<(), BundleResolutionError> {
        bundle.validate()?;
        if self.bundles.contains_key(&bundle.id) {
            return Err(BundleResolutionError::Conflict {
                item: Arc::from(bundle.id.as_str()),
            });
        }
        self.bundles.insert(bundle.id.clone(), Arc::new(bundle));
        Ok(())
    }

    /// Look up one exact installed bundle identity.
    #[must_use]
    pub fn bundle(&self, id: &BundleId) -> Option<Arc<BundleSpec>> {
        self.bundles.get(id).cloned()
    }
}

/// Immutable recipe retained solely for explicit application activation rebuilds.
pub(crate) struct CompositionRecipe {
    pub bundle: Arc<BundleSpec>,
    pub base_spec: Arc<AgentSpec>,
    pub application_config: BTreeMap<ComponentId, RawJson>,
    pub capabilities: BTreeMap<CapabilityId, (LockedBundle, Arc<CapabilitySpec>)>,
    pub active_application: BTreeSet<CapabilityId>,
    pub active_model: BTreeSet<CapabilityId>,
    pub services: RuntimeServices,
}
