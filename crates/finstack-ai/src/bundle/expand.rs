//! Spec expansion, component selection, and lock construction.

use std::collections::BTreeMap;
use std::sync::Arc;

use finstack_ai_kernel::{ComponentId, ComponentRef, Digest, RawJson, Version};

use crate::{
    AgentComponentSelection, AgentSpec, CapabilityActivation, ComponentSelector, ResolvedAgent,
};

use super::types::MAX_CONFIG_ENTRIES;
use super::{
    BUNDLE_SCHEMA_VERSION, BundleRequirement, BundleResolutionError, BundleSpec, CompositionRecipe,
    HostFeature, LockedBundle, LockedCapability, LockedComponent, RequiredServices,
    ResolvedAgentLock,
};

pub(super) fn expand_spec(recipe: &CompositionRecipe) -> Result<AgentSpec, BundleResolutionError> {
    let mut spec = (*recipe.base_spec).clone();
    let mut instructions = spec.instructions.to_vec();
    let mut toolsets = spec.toolsets.to_vec();
    let mut context = spec.context_providers.to_vec();
    let mut middleware = spec.middleware.to_vec();
    for (id, (_, capability)) in &recipe.capabilities {
        if capability.activation == CapabilityActivation::Disabled {
            continue;
        }
        extend_unique_components(&mut toolsets, capability.toolsets.iter().cloned());
        extend_unique_components(&mut context, capability.context_providers.iter().cloned());
        for component in capability.middleware.iter().cloned() {
            if middleware
                .iter()
                .any(|existing| existing.component().id() == component.id())
            {
                continue;
            }
            middleware.push(
                finstack_ai_kernel::MiddlewareRef::try_new(component, None::<&str>).map_err(
                    |error| BundleResolutionError::Invalid {
                        message: Arc::from(error.to_string()),
                    },
                )?,
            );
        }
        let instructions_active = match capability.activation {
            CapabilityActivation::Always => true,
            CapabilityActivation::Application => recipe.active_application.contains(id),
            CapabilityActivation::Model => recipe.active_model.contains(id),
            CapabilityActivation::Disabled => false,
        };
        if instructions_active {
            instructions.extend(capability.instructions.iter().cloned());
        }
    }
    spec.instructions = instructions.into();
    spec.toolsets = toolsets.into();
    spec.context_providers = context.into();
    spec.middleware = middleware.into();
    spec.validate()
        .map_err(|error| BundleResolutionError::Invalid {
            message: Arc::from(error.to_string()),
        })?;
    Ok(spec)
}

pub(super) fn effective_config(
    recipe: &CompositionRecipe,
) -> Result<BTreeMap<ComponentId, RawJson>, BundleResolutionError> {
    let mut effective = recipe.bundle.defaults.extension_config.clone();
    for (component, value) in &recipe.application_config {
        effective.insert(component.clone(), value.clone());
    }
    for (component, value) in &recipe.base_spec.extension_config {
        effective.insert(component.clone(), value.clone());
    }
    if effective.len() > MAX_CONFIG_ENTRIES {
        return Err(BundleResolutionError::Invalid {
            message: Arc::from("effective_config_too_many_entries"),
        });
    }
    Ok(effective)
}

pub(super) fn selection(
    spec: &AgentSpec,
) -> Result<AgentComponentSelection, BundleResolutionError> {
    let store = spec
        .store
        .clone()
        .ok_or_else(|| BundleResolutionError::Missing {
            item: Arc::from("agent journal store"),
        })?;
    Ok(AgentComponentSelection {
        model: ComponentSelector::Component(spec.model.clone()),
        toolsets: spec
            .toolsets
            .iter()
            .cloned()
            .map(ComponentSelector::Component)
            .collect::<Vec<_>>()
            .into(),
        context_providers: spec
            .context_providers
            .iter()
            .cloned()
            .map(ComponentSelector::Component)
            .collect::<Vec<_>>()
            .into(),
        middleware: spec
            .middleware
            .iter()
            .map(|reference| ComponentSelector::Component(reference.component().clone()))
            .collect::<Vec<_>>()
            .into(),
        store: ComponentSelector::Component(store),
        observers: spec
            .observers
            .iter()
            .cloned()
            .map(ComponentSelector::Component)
            .collect::<Vec<_>>()
            .into(),
    })
}

pub(super) fn build_lock(
    engine_version: Version,
    recipe: &CompositionRecipe,
    resolved: &ResolvedAgent,
    effective_config: &BTreeMap<ComponentId, RawJson>,
) -> Result<ResolvedAgentLock, BundleResolutionError> {
    let run_plan = resolved.run_plan();
    let mut components = Vec::new();
    let mut push = |component: &crate::RegisteredComponentDescriptor| {
        components.push(LockedComponent {
            component: component.component.clone(),
            kind: component.kind.into(),
            config_digest: effective_config
                .get(component.component.id())
                .map(RawJson::digest),
            configuration_schema: component.configuration_schema.clone(),
        });
    };
    push(run_plan.model().descriptor());
    for component in run_plan.toolsets() {
        push(component.descriptor());
    }
    for component in run_plan.context_providers() {
        push(component.descriptor());
    }
    for component in run_plan.middleware() {
        push(component.descriptor());
    }
    push(run_plan.store().descriptor());
    for component in run_plan.observers() {
        push(component.descriptor());
    }
    let capabilities = locked_capabilities(recipe)?;
    let config_bytes = serde_json_canonicalizer::to_vec(effective_config).map_err(|error| {
        BundleResolutionError::Invalid {
            message: Arc::from(error.to_string()),
        }
    })?;
    let effective_config_digest = Digest::domain_separated("effective-config", 1, &config_bytes)
        .map_err(|error| BundleResolutionError::Invalid {
            message: Arc::from(error.to_string()),
        })?;
    let mut schema_digests = components
        .iter()
        .filter_map(|component| {
            component
                .configuration_schema
                .as_ref()
                .map(|schema| schema.schema_digest)
        })
        .chain(
            recipe
                .bundle
                .config_schema
                .as_ref()
                .map(|schema| schema.schema_digest),
        )
        .collect::<Vec<_>>();
    schema_digests.sort_unstable();
    schema_digests.dedup();
    Ok(ResolvedAgentLock {
        schema_version: BUNDLE_SCHEMA_VERSION,
        engine_version,
        agent_spec_digest: recipe.base_spec.fingerprint().map_err(|error| {
            BundleResolutionError::Invalid {
                message: Arc::from(error.to_string()),
            }
        })?,
        bundle: Some(LockedBundle {
            id: recipe.bundle.id.clone(),
            version: recipe.bundle.version,
        }),
        components: components.into(),
        capabilities: capabilities.into(),
        effective_config_digest,
        middleware_chain_digest: run_plan.middleware_chain().digest(),
        schema_digests: schema_digests.into(),
        required_services: recipe.required_services,
    })
}

pub(super) fn locked_capabilities(
    recipe: &CompositionRecipe,
) -> Result<Vec<LockedCapability>, BundleResolutionError> {
    recipe
        .capabilities
        .iter()
        .map(|(id, (bundle, capability))| {
            let bytes = serde_json_canonicalizer::to_vec(capability.as_ref()).map_err(|error| {
                BundleResolutionError::Invalid {
                    message: Arc::from(error.to_string()),
                }
            })?;
            let definition_digest = Digest::domain_separated("capability-spec", 1, &bytes)
                .map_err(|error| BundleResolutionError::Invalid {
                    message: Arc::from(error.to_string()),
                })?;
            Ok(LockedCapability {
                id: id.clone(),
                bundle: bundle.clone(),
                activation: capability.activation,
                active: capability.activation == CapabilityActivation::Always
                    || recipe.active_application.contains(id)
                    || recipe.active_model.contains(id),
                definition_digest,
            })
        })
        .collect()
}

pub(super) fn required_services(bundle: &BundleSpec) -> RequiredServices {
    let mut required = RequiredServices::default();
    let features = bundle
        .requirements
        .iter()
        .filter_map(|requirement| match requirement {
            BundleRequirement::RequiredHostFeature { feature } => Some(feature),
            _ => None,
        })
        .chain(bundle.compatibility.required_host_features.iter());
    for feature in features {
        match feature {
            HostFeature::AgentInvoker => required.agent_invoker = true,
            HostFeature::BudgetLedger => required.budget_ledger = true,
            HostFeature::ArtifactStore => required.artifact_store = true,
            HostFeature::Custom { .. } => {}
        }
    }
    required
}

fn extend_unique_components(
    target: &mut Vec<ComponentRef>,
    extra: impl IntoIterator<Item = ComponentRef>,
) {
    for component in extra {
        if target
            .iter()
            .any(|existing| existing.id() == component.id())
        {
            continue;
        }
        target.push(component);
    }
}
