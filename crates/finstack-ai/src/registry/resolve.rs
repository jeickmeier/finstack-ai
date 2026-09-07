use core::future::{Future, poll_fn};
use core::sync::atomic::AtomicBool;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use finstack_ai_kernel::{ComponentId, ComponentRef, RawJson, Version};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::context::ContextProvider;
use finstack_ai_runtime::ports::journal::JournalStore;
use finstack_ai_runtime::ports::middleware::{
    Middleware, MiddlewareRegistration, ResolvedMiddlewareChain,
};
use finstack_ai_runtime::ports::model::{
    CancellationSignal, Model, ModelWarmupContext, ReadyModel,
};
use finstack_ai_runtime::ports::observer::Observer;
use finstack_ai_runtime::ports::tool::Toolset;

use super::errors::{
    AgentBuildError, RegisteredComponentDescriptor, RegistrationEvent, ResolutionDiagnostic,
    ResolutionDiagnosticKind, ResolutionReport,
};
use super::registrar::{FactoryConfiguration, RegisteredEntry, RegistrationSlot};
use super::resolved::{
    ResolvedAgent, ResolvedComponent, ResolvedHandles, ResolvedLifecycle, ResolvedRunPlan,
};
use super::types::{
    AgentConstructionContext, ComponentAlias, ComponentConstructionContext, ComponentKind,
    ComponentSelector, LABEL_MAX_BYTES, MAX_SELECTED_COMPONENTS, ReadyComponent, ResolveRequest,
};

/// Reusable deterministic registry.
///
/// Successful factory outputs are cached. Ordinary run paths retain direct
/// typed handles and do not look up components again.
pub struct Registry {
    pub(super) entries: BTreeMap<ComponentId, RegisteredEntry>,
    pub(super) aliases: BTreeMap<ComponentAlias, ComponentId>,
    pub(super) events: Arc<[RegistrationEvent]>,
    #[cfg(test)]
    pub(super) lookup_count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadyOutcome {
    ReadySelected,
    FactoryConstructed,
    CachedFactoryReused,
}

impl Registry {
    /// Look up one exact registered component descriptor without constructing it.
    #[must_use]
    pub fn registered_component(&self, id: &ComponentId) -> Option<&RegisteredComponentDescriptor> {
        self.entries.get(id).map(RegisteredEntry::descriptor)
    }

    /// Successful and replacement registration events in transaction order.
    #[must_use]
    pub const fn registration_events(&self) -> &Arc<[RegistrationEvent]> {
        &self.events
    }

    /// Resolve every selected component and factory before any run is accepted.
    ///
    /// Factories and model warmup are cached after their first successful
    /// construction. The host communicates a deadline and must cancel the
    /// context signal when that deadline fires.
    ///
    /// # Errors
    ///
    /// Returns a source-aware build error for missing, mismatched, duplicate,
    /// cancelled, invalid, or failed construction.
    #[expect(
        clippy::too_many_lines,
        reason = "fixed six-port resolution order is kept visible as one pre-run transaction"
    )]
    pub async fn resolve(
        &mut self,
        request: ResolveRequest,
        context: AgentConstructionContext,
    ) -> Result<ResolvedAgent, AgentBuildError> {
        let mut diagnostics = Vec::new();
        let mut selected = BTreeSet::new();
        Self::validate_repeated_bounds(&request)?;

        let model_id = self.resolve_target(
            &request.selection.model,
            ComponentKind::Model,
            &request.source,
            &mut selected,
            &mut diagnostics,
        )?;
        let model_configuration = request.configurations.get(&model_id).cloned();
        let model = self
            .resolve_model(
                &model_id,
                model_configuration,
                &request.source,
                &context,
                &mut diagnostics,
            )
            .await?;

        let mut toolsets = Vec::with_capacity(request.selection.toolsets.len());
        for selector in request.selection.toolsets.iter() {
            let id = self.resolve_target(
                selector,
                ComponentKind::Toolset,
                &request.source,
                &mut selected,
                &mut diagnostics,
            )?;
            let configuration = request.configurations.get(&id).cloned();
            toolsets.push(
                self.resolve_toolset(
                    &id,
                    configuration,
                    &request.source,
                    &context,
                    &mut diagnostics,
                )
                .await?,
            );
        }

        let mut context_providers = Vec::with_capacity(request.selection.context_providers.len());
        for selector in request.selection.context_providers.iter() {
            let id = self.resolve_target(
                selector,
                ComponentKind::ContextProvider,
                &request.source,
                &mut selected,
                &mut diagnostics,
            )?;
            let configuration = request.configurations.get(&id).cloned();
            context_providers.push(
                self.resolve_context_provider(
                    &id,
                    configuration,
                    &request.source,
                    &context,
                    &mut diagnostics,
                )
                .await?,
            );
        }

        let mut middleware = Vec::with_capacity(request.selection.middleware.len());
        for selector in request.selection.middleware.iter() {
            let id = self.resolve_target(
                selector,
                ComponentKind::Middleware,
                &request.source,
                &mut selected,
                &mut diagnostics,
            )?;
            let configuration = request.configurations.get(&id).cloned();
            middleware.push(
                self.resolve_middleware(
                    &id,
                    configuration,
                    &request.source,
                    &context,
                    &mut diagnostics,
                )
                .await?,
            );
        }

        let store_id = self.resolve_target(
            &request.selection.store,
            ComponentKind::Store,
            &request.source,
            &mut selected,
            &mut diagnostics,
        )?;
        let store_configuration = request.configurations.get(&store_id).cloned();
        let store = self
            .resolve_store(
                &store_id,
                store_configuration,
                &request.source,
                &context,
                &mut diagnostics,
            )
            .await?;

        let mut observers = Vec::with_capacity(request.selection.observers.len());
        for selector in request.selection.observers.iter() {
            let id = self.resolve_target(
                selector,
                ComponentKind::Observer,
                &request.source,
                &mut selected,
                &mut diagnostics,
            )?;
            let configuration = request.configurations.get(&id).cloned();
            observers.push(
                self.resolve_observer(
                    &id,
                    configuration,
                    &request.source,
                    &context,
                    &mut diagnostics,
                )
                .await?,
            );
        }

        for component in request
            .configurations
            .keys()
            .filter(|component| !selected.contains(*component))
        {
            diagnostics.push(ResolutionDiagnostic {
                kind: ResolutionDiagnosticKind::UnusedConfiguration,
                request_source: request.source.clone(),
                component: Some(ComponentRef::new(component.clone(), None)),
                registration_source: None,
                alias: None,
            });
        }

        let chain = ResolvedMiddlewareChain::try_new(
            middleware
                .iter()
                .map(|component| MiddlewareRegistration {
                    middleware: Arc::clone(component.handle()),
                })
                .collect(),
        )
        .map_err(|error| AgentBuildError::MiddlewareInvalid {
            request_source: request.source.clone(),
            failure_code: Arc::from(error.code().as_str()),
            message: error.descriptor().message,
        })?;

        let mut lifecycles = Vec::new();
        push_lifecycle(&mut lifecycles, &model);
        for component in &toolsets {
            push_lifecycle(&mut lifecycles, component);
        }
        for component in &context_providers {
            push_lifecycle(&mut lifecycles, component);
        }
        for component in &middleware {
            push_lifecycle(&mut lifecycles, component);
        }
        push_lifecycle(&mut lifecycles, &store);
        for component in &observers {
            push_lifecycle(&mut lifecycles, component);
        }

        let handles = Arc::new(ResolvedHandles {
            model,
            toolsets: toolsets.into(),
            context_providers: context_providers.into(),
            middleware: middleware.into(),
            middleware_chain: Arc::new(chain),
            store,
            observers: observers.into(),
        });
        Ok(ResolvedAgent {
            run_plan: ResolvedRunPlan { handles },
            report: ResolutionReport {
                request_source: request.source,
                diagnostics: diagnostics.into(),
            },
            lifecycles: lifecycles.into(),
            spec: None,
            lock: None,
            composition: None,
        })
    }

    fn validate_repeated_bounds(request: &ResolveRequest) -> Result<(), AgentBuildError> {
        for selectors in [
            &request.selection.toolsets,
            &request.selection.context_providers,
            &request.selection.middleware,
            &request.selection.observers,
        ] {
            if selectors.len() > MAX_SELECTED_COMPONENTS {
                return Err(AgentBuildError::DuplicateSelection {
                    request_source: request.source.clone(),
                    component: request.source.clone(),
                });
            }
        }
        Ok(())
    }

    fn resolve_target(
        &mut self,
        selector: &ComponentSelector,
        expected_kind: ComponentKind,
        request_source: &ComponentId,
        selected: &mut BTreeSet<ComponentId>,
        diagnostics: &mut Vec<ResolutionDiagnostic>,
    ) -> Result<ComponentId, AgentBuildError> {
        #[cfg(test)]
        {
            self.lookup_count += 1;
        }
        let (component, alias) = match selector {
            ComponentSelector::Component(component) => (component.id().clone(), None),
            ComponentSelector::Alias { alias, .. } => {
                let component = self.aliases.get(alias).cloned().ok_or_else(|| {
                    AgentBuildError::MissingComponent {
                        request_source: request_source.clone(),
                        selector: selector.display_name(),
                        expected_kind,
                    }
                })?;
                (component, Some(alias.clone()))
            }
        };
        let descriptor = self
            .entries
            .get(&component)
            .map(RegisteredEntry::descriptor)
            .ok_or_else(|| AgentBuildError::MissingComponent {
                request_source: request_source.clone(),
                selector: selector.display_name(),
                expected_kind,
            })?;
        if descriptor.kind != expected_kind {
            return Err(AgentBuildError::KindMismatch {
                request_source: request_source.clone(),
                component,
                registration_source: descriptor.source.id.clone(),
                expected_kind,
                actual_kind: descriptor.kind,
            });
        }
        let Some(actual) = descriptor.component.version() else {
            return Err(AgentBuildError::InvalidDescriptor {
                request_source: request_source.clone(),
                component,
                registration_source: descriptor.source.id.clone(),
                message: Arc::from("registration is missing an exact version"),
            });
        };
        if let Some(required) = selector.required_version()
            && required != actual
        {
            return Err(AgentBuildError::VersionMismatch {
                request_source: request_source.clone(),
                component,
                registration_source: descriptor.source.id.clone(),
                required,
                actual,
            });
        }
        if !selected.insert(component.clone()) {
            return Err(AgentBuildError::DuplicateSelection {
                request_source: request_source.clone(),
                component,
            });
        }
        if let Some(alias) = alias {
            diagnostics.push(ResolutionDiagnostic {
                kind: ResolutionDiagnosticKind::AliasExpanded,
                request_source: request_source.clone(),
                component: Some(descriptor.component.clone()),
                registration_source: Some(descriptor.source.id.clone()),
                alias: Some(alias),
            });
        }
        Ok(component)
    }

    async fn resolve_model(
        &mut self,
        id: &ComponentId,
        configuration: Option<RawJson>,
        source: &ComponentId,
        context: &AgentConstructionContext,
        diagnostics: &mut Vec<ResolutionDiagnostic>,
    ) -> Result<ResolvedComponent<ReadyModel>, AgentBuildError> {
        let Some(RegisteredEntry::Model(registration)) = self.entries.get_mut(id) else {
            return Err(typed_resolution_error(
                &self.entries,
                id,
                source,
                ComponentKind::Model,
            ));
        };
        let outcome = ensure_ready(
            &registration.descriptor,
            &mut registration.slot,
            configuration,
            source,
            context,
        )
        .await?;
        let RegistrationSlot::Ready {
            component,
            ready_model,
            ..
        } = &registration.slot
        else {
            return Err(factory_not_ready(source, id, &registration.descriptor));
        };
        let (component, cached) = (component.clone(), ready_model.clone());
        validate_model_descriptor(&registration.descriptor, component.handle(), source)?;
        let ready = if let Some(ready) = cached {
            ready
        } else {
            let warmup = ReadyModel::prepare_with_context(
                Arc::clone(component.handle()),
                ModelWarmupContext {
                    cancellation: context.cancellation.child(),
                    deadline: context.deadline,
                    metadata: context.metadata.clone(),
                },
            );
            match await_or_cancel(Box::pin(warmup), context.cancellation.clone()).await {
                Ok(Ok(ready)) => {
                    let ready = Arc::new(ready);
                    if let RegistrationSlot::Ready { ready_model, .. } = &mut registration.slot {
                        *ready_model = Some(Arc::clone(&ready));
                    }
                    ready
                }
                Ok(Err(error)) => {
                    return Err(AgentBuildError::FactoryFailed {
                        request_source: source.clone(),
                        component: id.clone(),
                        registration_source: registration.descriptor.source.id.clone(),
                        failure_code: Arc::from(error.code().as_str()),
                        message: Arc::from(error.message()),
                    });
                }
                Err(()) => {
                    return Err(AgentBuildError::Cancelled {
                        request_source: source.clone(),
                        component: id.clone(),
                    });
                }
            }
        };
        push_resolution_diagnostic(diagnostics, source, &registration.descriptor, outcome);
        Ok(ResolvedComponent {
            descriptor: registration.descriptor.clone(),
            handle: ready,
            lifecycle: component.lifecycle,
        })
    }
}

macro_rules! resolve_port {
    ($method:ident, $variant:ident, $trait:path, $validate:ident, $kind:expr) => {
        impl Registry {
            async fn $method(
                &mut self,
                id: &ComponentId,
                configuration: Option<RawJson>,
                source: &ComponentId,
                context: &AgentConstructionContext,
                diagnostics: &mut Vec<ResolutionDiagnostic>,
            ) -> Result<ResolvedComponent<dyn $trait>, AgentBuildError> {
                let Some(RegisteredEntry::$variant(registration)) = self.entries.get_mut(id) else {
                    return Err(typed_resolution_error(&self.entries, id, source, $kind));
                };
                let outcome = ensure_ready(
                    &registration.descriptor,
                    &mut registration.slot,
                    configuration,
                    source,
                    context,
                )
                .await?;
                let RegistrationSlot::Ready { component, .. } = &registration.slot else {
                    return Err(factory_not_ready(source, id, &registration.descriptor));
                };
                let ready = component.clone();
                $validate(&registration.descriptor, ready.handle(), source)?;
                push_resolution_diagnostic(diagnostics, source, &registration.descriptor, outcome);
                Ok(resolved(&registration.descriptor, ready))
            }
        }
    };
}

resolve_port!(
    resolve_toolset,
    Toolset,
    Toolset,
    validate_toolset_descriptor,
    ComponentKind::Toolset
);
resolve_port!(
    resolve_context_provider,
    ContextProvider,
    ContextProvider,
    validate_context_descriptor,
    ComponentKind::ContextProvider
);
resolve_port!(
    resolve_middleware,
    Middleware,
    Middleware,
    validate_middleware_descriptor,
    ComponentKind::Middleware
);
resolve_port!(
    resolve_store,
    Store,
    JournalStore,
    validate_store_descriptor,
    ComponentKind::Store
);
resolve_port!(
    resolve_observer,
    Observer,
    Observer,
    validate_observer_descriptor,
    ComponentKind::Observer
);

fn typed_resolution_error(
    entries: &BTreeMap<ComponentId, RegisteredEntry>,
    id: &ComponentId,
    source: &ComponentId,
    expected_kind: ComponentKind,
) -> AgentBuildError {
    match entries.get(id) {
        Some(entry) => AgentBuildError::KindMismatch {
            request_source: source.clone(),
            component: id.clone(),
            registration_source: entry.descriptor().source.id.clone(),
            expected_kind,
            actual_kind: entry.descriptor().kind,
        },
        None => AgentBuildError::MissingComponent {
            request_source: source.clone(),
            selector: Arc::from(id.as_str()),
            expected_kind,
        },
    }
}

fn factory_not_ready(
    source: &ComponentId,
    id: &ComponentId,
    descriptor: &RegisteredComponentDescriptor,
) -> AgentBuildError {
    AgentBuildError::InvalidDescriptor {
        request_source: source.clone(),
        component: id.clone(),
        registration_source: descriptor.source.id.clone(),
        message: Arc::from("factory did not cache a ready handle"),
    }
}

async fn ensure_ready<T: ?Sized + 'static>(
    descriptor: &RegisteredComponentDescriptor,
    slot: &mut RegistrationSlot<T>,
    configuration: Option<RawJson>,
    request_source: &ComponentId,
    context: &AgentConstructionContext,
) -> Result<ReadyOutcome, AgentBuildError> {
    let requested_configuration = FactoryConfiguration::from_raw(configuration.as_ref());
    match slot {
        RegistrationSlot::Ready {
            factory_configuration: Some(cached),
            ..
        } => {
            if *cached != requested_configuration {
                return Err(AgentBuildError::ConfigurationConflict {
                    request_source: request_source.clone(),
                    component: descriptor.component.id().clone(),
                    registration_source: descriptor.source.id.clone(),
                });
            }
            Ok(ReadyOutcome::CachedFactoryReused)
        }
        RegistrationSlot::Ready {
            factory_configuration: None,
            ..
        } => {
            if !matches!(requested_configuration, FactoryConfiguration::Empty) {
                return Err(AgentBuildError::ConfigurationConflict {
                    request_source: request_source.clone(),
                    component: descriptor.component.id().clone(),
                    registration_source: descriptor.source.id.clone(),
                });
            }
            Ok(ReadyOutcome::ReadySelected)
        }
        RegistrationSlot::Factory(factory) => {
            if context.cancellation.is_cancelled() {
                return Err(AgentBuildError::Cancelled {
                    request_source: request_source.clone(),
                    component: descriptor.component.id().clone(),
                });
            }
            let factory = Arc::clone(factory);
            let component_context = ComponentConstructionContext {
                component: descriptor.component.clone(),
                configuration,
                cancellation: context.cancellation.child(),
                deadline: context.deadline,
                metadata: context.metadata.clone(),
            };
            let ready = match await_or_cancel(
                factory.construct(component_context),
                context.cancellation.clone(),
            )
            .await
            {
                Ok(Ok(ready)) => ready,
                Ok(Err(error)) => {
                    return Err(AgentBuildError::FactoryFailed {
                        request_source: request_source.clone(),
                        component: descriptor.component.id().clone(),
                        registration_source: descriptor.source.id.clone(),
                        failure_code: Arc::from(error.code()),
                        message: Arc::from(error.message()),
                    });
                }
                Err(()) => {
                    return Err(AgentBuildError::Cancelled {
                        request_source: request_source.clone(),
                        component: descriptor.component.id().clone(),
                    });
                }
            };
            *slot = RegistrationSlot::Ready {
                component: ready,
                factory_configuration: Some(requested_configuration),
                ready_model: None,
            };
            Ok(ReadyOutcome::FactoryConstructed)
        }
    }
}

async fn await_or_cancel<T>(
    future: PortFuture<T>,
    cancellation: CancellationSignal,
) -> Result<T, ()> {
    let mut future = future;
    let mut cancelled = Box::pin(cancellation.cancelled());
    poll_fn(|context| {
        if let core::task::Poll::Ready(value) = future.as_mut().poll(context) {
            return core::task::Poll::Ready(Ok(value));
        }
        if cancelled.as_mut().poll(context).is_ready() {
            return core::task::Poll::Ready(Err(()));
        }
        core::task::Poll::Pending
    })
    .await
}

fn resolved<T: ?Sized>(
    descriptor: &RegisteredComponentDescriptor,
    ready: ReadyComponent<T>,
) -> ResolvedComponent<T> {
    ResolvedComponent {
        descriptor: descriptor.clone(),
        handle: ready.handle,
        lifecycle: ready.lifecycle,
    }
}

fn push_lifecycle<T: ?Sized>(
    lifecycles: &mut Vec<ResolvedLifecycle>,
    component: &ResolvedComponent<T>,
) {
    if let Some(binding) = &component.lifecycle {
        lifecycles.push(ResolvedLifecycle {
            descriptor: component.descriptor.clone(),
            binding: binding.clone(),
            shutdown_started: AtomicBool::new(false),
        });
    }
}

fn push_resolution_diagnostic(
    diagnostics: &mut Vec<ResolutionDiagnostic>,
    request_source: &ComponentId,
    descriptor: &RegisteredComponentDescriptor,
    outcome: ReadyOutcome,
) {
    diagnostics.push(ResolutionDiagnostic {
        kind: match outcome {
            ReadyOutcome::ReadySelected => ResolutionDiagnosticKind::ReadySelected,
            ReadyOutcome::FactoryConstructed => ResolutionDiagnosticKind::FactoryConstructed,
            ReadyOutcome::CachedFactoryReused => ResolutionDiagnosticKind::CachedFactoryReused,
        },
        request_source: request_source.clone(),
        component: Some(descriptor.component.clone()),
        registration_source: Some(descriptor.source.id.clone()),
        alias: None,
    });
}

fn invalid_descriptor(
    descriptor: &RegisteredComponentDescriptor,
    request_source: &ComponentId,
    message: impl Into<Arc<str>>,
) -> AgentBuildError {
    AgentBuildError::InvalidDescriptor {
        request_source: request_source.clone(),
        component: descriptor.component.id().clone(),
        registration_source: descriptor.source.id.clone(),
        message: message.into(),
    }
}

fn validate_model_descriptor(
    descriptor: &RegisteredComponentDescriptor,
    handle: &Arc<dyn Model>,
    request_source: &ComponentId,
) -> Result<(), AgentBuildError> {
    handle
        .descriptor()
        .validate()
        .map_err(|error| invalid_descriptor(descriptor, request_source, error.message()))
}

fn validate_toolset_descriptor(
    descriptor: &RegisteredComponentDescriptor,
    handle: &Arc<dyn Toolset>,
    request_source: &ComponentId,
) -> Result<(), AgentBuildError> {
    let name = handle.descriptor().name;
    if name.is_empty() || name.len() > LABEL_MAX_BYTES || name.as_bytes().contains(&0) {
        return Err(invalid_descriptor(
            descriptor,
            request_source,
            "toolset descriptor name is invalid",
        ));
    }
    Ok(())
}

fn validate_context_descriptor(
    descriptor: &RegisteredComponentDescriptor,
    handle: &Arc<dyn ContextProvider>,
    request_source: &ComponentId,
) -> Result<(), AgentBuildError> {
    let invocation = handle.descriptor().invocation;
    validate_invocation(
        descriptor,
        &invocation.component,
        invocation.version,
        request_source,
    )
}

fn validate_middleware_descriptor(
    descriptor: &RegisteredComponentDescriptor,
    handle: &Arc<dyn Middleware>,
    request_source: &ComponentId,
) -> Result<(), AgentBuildError> {
    let invocation = handle.descriptor().invocation;
    validate_invocation(
        descriptor,
        &invocation.component,
        invocation.version,
        request_source,
    )
}

#[expect(
    clippy::unnecessary_wraps,
    reason = "the store port has no identity descriptor; the shared resolver validator remains typed"
)]
fn validate_store_descriptor(
    _descriptor: &RegisteredComponentDescriptor,
    _handle: &Arc<dyn JournalStore>,
    _request_source: &ComponentId,
) -> Result<(), AgentBuildError> {
    Ok(())
}

fn validate_observer_descriptor(
    descriptor: &RegisteredComponentDescriptor,
    handle: &Arc<dyn Observer>,
    request_source: &ComponentId,
) -> Result<(), AgentBuildError> {
    let actual = handle.descriptor().component;
    if actual != descriptor.component {
        return Err(invalid_descriptor(
            descriptor,
            request_source,
            "observer descriptor identity does not match its registration",
        ));
    }
    Ok(())
}

fn validate_invocation(
    descriptor: &RegisteredComponentDescriptor,
    component: &ComponentId,
    version: Version,
    request_source: &ComponentId,
) -> Result<(), AgentBuildError> {
    if component != descriptor.component.id() || descriptor.component.version() != Some(version) {
        return Err(invalid_descriptor(
            descriptor,
            request_source,
            format!(
                "port invocation {}@{}.{}.{} does not match its registration {:?}",
                component, version.major, version.minor, version.patch, descriptor.component,
            ),
        ));
    }
    Ok(())
}
