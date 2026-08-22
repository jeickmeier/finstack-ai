use core::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use finstack_ai_kernel::ComponentRef;
use finstack_ai_runtime::ports::context::ContextProvider;
use finstack_ai_runtime::ports::journal::JournalStore;
use finstack_ai_runtime::ports::middleware::{Middleware, ResolvedMiddlewareChain};
use finstack_ai_runtime::ports::model::ReadyModel;
use finstack_ai_runtime::ports::observer::Observer;
use finstack_ai_runtime::ports::tool::Toolset;

use super::errors::{RegisteredComponentDescriptor, ResolutionReport};
use super::types::{
    AgentConstructionContext, ComponentHealth, LifecycleBinding, LifecycleError, ShutdownOwnership,
};

/// Frozen registration metadata plus one direct primary-port handle.
pub struct ResolvedComponent<T: ?Sized> {
    pub(super) descriptor: RegisteredComponentDescriptor,
    pub(super) handle: Arc<T>,
    pub(super) lifecycle: Option<LifecycleBinding>,
}

impl<T: ?Sized> ResolvedComponent<T> {
    /// Frozen registration metadata.
    #[must_use]
    pub const fn descriptor(&self) -> &RegisteredComponentDescriptor {
        &self.descriptor
    }

    /// Borrow the direct trait-object handle.
    #[must_use]
    pub const fn handle(&self) -> &Arc<T> {
        &self.handle
    }

    /// Declared lifecycle shutdown owner, when lifecycle hooks exist.
    #[must_use]
    pub fn shutdown_ownership(&self) -> Option<ShutdownOwnership> {
        self.lifecycle.as_ref().map(LifecycleBinding::ownership)
    }
}

impl<T: ?Sized> Clone for ResolvedComponent<T> {
    fn clone(&self) -> Self {
        Self {
            descriptor: self.descriptor.clone(),
            handle: Arc::clone(&self.handle),
            lifecycle: self.lifecycle.clone(),
        }
    }
}

pub(super) struct ResolvedHandles {
    pub(super) model: ResolvedComponent<ReadyModel>,
    pub(super) toolsets: Arc<[ResolvedComponent<dyn Toolset>]>,
    pub(super) context_providers: Arc<[ResolvedComponent<dyn ContextProvider>]>,
    pub(super) middleware: Arc<[ResolvedComponent<dyn Middleware>]>,
    pub(super) middleware_chain: Arc<ResolvedMiddlewareChain>,
    pub(super) store: ResolvedComponent<dyn JournalStore>,
    pub(super) observers: Arc<[ResolvedComponent<dyn Observer>]>,
}

/// Immutable per-run direct-handle plan. It performs no registry lookup.
#[derive(Clone)]
pub struct ResolvedRunPlan {
    pub(super) handles: Arc<ResolvedHandles>,
}

impl ResolvedRunPlan {
    /// Resolved model.
    #[must_use]
    pub fn model(&self) -> &ResolvedComponent<ReadyModel> {
        &self.handles.model
    }

    /// Resolved ordered toolsets.
    #[must_use]
    pub fn toolsets(&self) -> &[ResolvedComponent<dyn Toolset>] {
        &self.handles.toolsets
    }

    /// Resolved ordered context providers.
    #[must_use]
    pub fn context_providers(&self) -> &[ResolvedComponent<dyn ContextProvider>] {
        &self.handles.context_providers
    }

    /// Resolved middleware in configuration order.
    #[must_use]
    pub fn middleware(&self) -> &[ResolvedComponent<dyn Middleware>] {
        &self.handles.middleware
    }

    /// Dependency-resolved middleware chain.
    #[must_use]
    pub fn middleware_chain(&self) -> &Arc<ResolvedMiddlewareChain> {
        &self.handles.middleware_chain
    }

    /// Resolved journal store.
    #[must_use]
    pub fn store(&self) -> &ResolvedComponent<dyn JournalStore> {
        &self.handles.store
    }

    /// Resolved ordered observers.
    #[must_use]
    pub fn observers(&self) -> &[ResolvedComponent<dyn Observer>] {
        &self.handles.observers
    }
}

pub(super) struct ResolvedLifecycle {
    pub(super) descriptor: RegisteredComponentDescriptor,
    pub(super) binding: LifecycleBinding,
    pub(super) shutdown_started: AtomicBool,
}

/// One component health result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentHealthReport {
    /// Component.
    pub component: ComponentRef,
    /// Declared shutdown owner.
    pub ownership: ShutdownOwnership,
    /// Health result.
    pub result: Result<ComponentHealth, LifecycleError>,
}

/// At-most-once lifecycle shutdown result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ComponentShutdownOutcome {
    /// Resolved-agent-owned hook completed.
    Completed,
    /// A prior shutdown attempt already owns the hook.
    AlreadyCompleted,
    /// The embedding host owns this hook.
    HostOwned,
    /// The sole shutdown attempt failed.
    Failed(LifecycleError),
}

/// One component shutdown result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComponentShutdownReport {
    /// Component.
    pub component: ComponentRef,
    /// Shutdown outcome.
    pub outcome: ComponentShutdownOutcome,
}

/// Immutable resolved agent with ready direct handles and lifecycle ownership.
pub struct ResolvedAgent {
    pub(super) run_plan: ResolvedRunPlan,
    pub(super) report: ResolutionReport,
    pub(super) lifecycles: Arc<[ResolvedLifecycle]>,
    pub(super) spec: Option<Arc<crate::AgentSpec>>,
    pub(super) lock: Option<Arc<crate::ResolvedAgentLock>>,
    pub(super) composition: Option<Arc<crate::bundle::CompositionRecipe>>,
}

impl ResolvedAgent {
    /// Declarative agent specification when constructed through a bundle resolver.
    #[must_use]
    pub fn spec(&self) -> Option<&Arc<crate::AgentSpec>> {
        self.spec.as_ref()
    }

    /// Exact credential-free resolution lock when constructed through a bundle resolver.
    #[must_use]
    pub fn lock(&self) -> Option<&Arc<crate::ResolvedAgentLock>> {
        self.lock.as_ref()
    }

    /// Direct non-primary runtime services validated during composition.
    #[must_use]
    pub fn services(&self) -> Option<&crate::RuntimeServices> {
        self.composition.as_ref().map(|recipe| &recipe.services)
    }

    pub(crate) fn attach_composition(
        mut self,
        spec: Arc<crate::AgentSpec>,
        lock: Arc<crate::ResolvedAgentLock>,
        composition: Arc<crate::bundle::CompositionRecipe>,
    ) -> Self {
        self.spec = Some(spec);
        self.lock = Some(lock);
        self.composition = Some(composition);
        self
    }

    pub(crate) fn composition(&self) -> Option<&Arc<crate::bundle::CompositionRecipe>> {
        self.composition.as_ref()
    }
    /// Clone the immutable no-lookup run plan.
    #[must_use]
    pub fn run_plan(&self) -> ResolvedRunPlan {
        self.run_plan.clone()
    }

    /// Successful non-secret construction report.
    #[must_use]
    pub const fn resolution_report(&self) -> &ResolutionReport {
        &self.report
    }

    /// Query all registered lifecycle hooks in selected-component order.
    pub async fn health(&self) -> Arc<[ComponentHealthReport]> {
        let mut reports = Vec::with_capacity(self.lifecycles.len());
        for lifecycle in self.lifecycles.iter() {
            reports.push(ComponentHealthReport {
                component: lifecycle.descriptor.component.clone(),
                ownership: lifecycle.binding.ownership,
                result: lifecycle.binding.hooks.health().await,
            });
        }
        reports.into()
    }

    /// Shut down resolved-agent-owned hooks at most once in reverse selection order.
    pub async fn shutdown(
        &self,
        context: AgentConstructionContext,
    ) -> Arc<[ComponentShutdownReport]> {
        let mut reports = Vec::with_capacity(self.lifecycles.len());
        for lifecycle in self.lifecycles.iter().rev() {
            let outcome = if lifecycle.binding.ownership == ShutdownOwnership::Host {
                ComponentShutdownOutcome::HostOwned
            } else if lifecycle.shutdown_started.swap(true, Ordering::AcqRel) {
                ComponentShutdownOutcome::AlreadyCompleted
            } else {
                match lifecycle.binding.hooks.shutdown(context.clone()).await {
                    Ok(()) => ComponentShutdownOutcome::Completed,
                    Err(error) => ComponentShutdownOutcome::Failed(error),
                }
            };
            reports.push(ComponentShutdownReport {
                component: lifecycle.descriptor.component.clone(),
                outcome,
            });
        }
        reports.into()
    }
}
