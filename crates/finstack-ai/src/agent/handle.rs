use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::ResolvedAgent;
use finstack_ai_kernel::{
    AgentId, BundleId, ComponentInvocation, ComponentRef, Digest, InvocationRecovery,
    JsonSchemaDraft, RawJson, SchemaRef, Version,
};
use finstack_ai_runtime::{
    JsonSchemaToolValidatorCompiler, Model, ResolvedToolCatalog, ToolExecutionPolicy,
    ToolFailurePolicy, ToolPolicyDecision, ToolValidator, ToolValidatorCompiler,
    ToolsetRegistration,
};

use super::activation::NativeCapabilityHost;
use super::builder::NativeAgentBuilder;
use super::mask::CapabilityContributionIndex;
use super::run::{AgentRun, AgentRunInner, CancellationState, EventStreamState, publish_result};
use super::types::{
    AGENT_RUN_INVALID_CONFIGURATION, AgentRunError, AgentRunOutput, AgentRunRequest,
    CapabilityCatalogEntry,
};
use crate::{CapabilityActivation, CapabilitySpec};

#[cfg(all(feature = "wasm-host", not(feature = "native-tokio")))]
use finstack_ai_runtime::host_driver as driver;
#[cfg(feature = "native-tokio")]
use finstack_ai_runtime::native_driver as driver;

#[derive(Clone)]
pub(super) struct ModelCapabilityVariant {
    pub(super) entry: CapabilityCatalogEntry,
    pub(super) agent: Agent,
}

/// Immutable native facade over one fully resolved agent.
///
/// Construct with [`Agent::builder`] then [`NativeAgentBuilder::build`].
/// `start` / `run` execute one bounded turn. Dropping an [`AgentRun`]
/// detaches observation; it does not cancel the durable run.
#[derive(Clone)]
pub struct Agent {
    pub(super) resolved: Arc<ResolvedAgent>,
    pub(super) tools: Arc<ResolvedToolCatalog>,
    pub(super) structured_output: Option<StructuredOutputConfig>,
    pub(super) model_capabilities: Arc<[ModelCapabilityVariant]>,
    pub(super) capability_specs: Arc<[CapabilitySpec]>,
    pub(super) capability_index: CapabilityContributionIndex,
    pub(super) activation_host: Option<Arc<NativeCapabilityHost>>,
}

#[derive(Clone)]
pub(super) struct StructuredOutputConfig {
    pub(super) schema_ref: SchemaRef,
    pub(super) validator: Arc<dyn ToolValidator>,
}

impl Agent {
    /// Start ergonomic native composition over direct model and store handles.
    ///
    /// This is the live-handle constructor ([`NativeAgentBuilder`]). Use
    /// [`crate::AgentSpec::builder`] when you only need declarative spec data.
    ///
    /// # Arguments
    ///
    /// * `agent_id` - Stable agent identity recorded in the bundle spec and lock.
    /// * `bundle_id` - Bundle identity that owns this agent.
    /// * `model` - Exact-version component ref plus a ready [`Model`] handle.
    /// * `store` - Exact-version component ref plus a ready journal-store handle.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn compose(
    /// #     builder: finstack_ai::NativeAgentBuilder,
    /// # ) -> Result<(), finstack_ai::AgentRunError> {
    /// let agent = builder.build().await?;
    /// let _ = agent;
    /// # Ok(())
    /// # }
    /// ```
    #[must_use]
    pub fn builder(
        agent_id: AgentId,
        bundle_id: BundleId,
        model: (ComponentRef, Arc<dyn Model>),
        store: (ComponentRef, Arc<dyn finstack_ai_runtime::JournalStore>),
    ) -> NativeAgentBuilder {
        NativeAgentBuilder::new(agent_id, bundle_id, model, store)
    }

    /// Validate and retain one resolved, no-lookup execution plan.
    ///
    /// Supports direct model, Toolset, context-provider, middleware, and
    /// observer handles. Observer failures are isolated from run semantics.
    ///
    /// # Arguments
    ///
    /// * `resolved` - Agent produced by a bundle resolver with a spec and lock.
    ///
    /// # Errors
    ///
    /// Returns a stable configuration error when the agent did not come from a
    /// bundle resolver, contains a deferred stage, or has an invalid tool catalog.
    pub fn try_from_resolved(resolved: Arc<ResolvedAgent>) -> Result<Self, AgentRunError> {
        if resolved.spec().is_none() || resolved.lock().is_none() {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "native Agent requires a bundle-resolved specification and lock",
            ));
        }
        let plan = resolved.run_plan();
        let registrations =
            plan.toolsets()
                .iter()
                .map(|component| {
                    let toolset = Arc::clone(component.handle());
                    let invocation =
                        ComponentInvocation {
                            component: component.descriptor().component.id().clone(),
                            version: component.descriptor().component.version().unwrap_or(
                                Version {
                                    major: 0,
                                    minor: 0,
                                    patch: 1,
                                },
                            ),
                            configuration_digest: Digest::raw_json(b"{}"),
                            recovery: InvocationRecovery::RecomputeSafe,
                        };
                    let policies = toolset
                        .tools()
                        .iter()
                        .map(|tool| {
                            let approval = match tool.approval.requirement {
                                finstack_ai_runtime::ApprovalRequirement::NotRequired => {
                                    ToolPolicyDecision::Allow
                                }
                                finstack_ai_runtime::ApprovalRequirement::Required
                                | finstack_ai_runtime::ApprovalRequirement::Policy => {
                                    ToolPolicyDecision::RequireApproval
                                }
                            };
                            (
                                tool.id.clone(),
                                ToolExecutionPolicy {
                                    failure_policy: ToolFailurePolicy::ReturnToModel,
                                    approval,
                                    max_concurrency: 4,
                                },
                            )
                        })
                        .collect();
                    let components = toolset
                        .tools()
                        .iter()
                        .map(|tool| (tool.id.clone(), invocation.clone()))
                        .collect();
                    ToolsetRegistration {
                        toolset,
                        policies,
                        components,
                    }
                })
                .collect::<Vec<_>>();
        let tools = ResolvedToolCatalog::try_new(
            registrations,
            &BTreeMap::new(),
            &JsonSchemaToolValidatorCompiler,
        )
        .map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?;
        Ok(Self {
            resolved,
            tools: Arc::new(tools),
            structured_output: None,
            model_capabilities: Arc::from([]),
            capability_specs: Arc::from([]),
            capability_index: CapabilityContributionIndex::default(),
            activation_host: None,
        })
    }

    pub(super) fn attach_capability_surface(
        &mut self,
        specs: Arc<[CapabilitySpec]>,
        index: CapabilityContributionIndex,
        host: Option<Arc<NativeCapabilityHost>>,
    ) -> Result<(), AgentRunError> {
        for provider in self.resolved.run_plan().context_providers() {
            let component_id = provider.descriptor().component.id();
            let Some(owner) = index.owners().get(component_id) else {
                continue;
            };
            let untrusted = specs
                .iter()
                .any(|spec| spec.id == *owner && spec.activation == CapabilityActivation::Model);
            if untrusted
                && provider
                    .handle()
                    .descriptor()
                    .trusted_application_instructions
            {
                return Err(AgentRunError::configuration(
                    AGENT_RUN_INVALID_CONFIGURATION,
                    "untrusted capability cannot set trusted_application_instructions",
                ));
            }
        }
        self.capability_specs = specs;
        self.capability_index = index;
        self.activation_host = host;
        Ok(())
    }

    pub(super) fn capability_index(&self) -> &CapabilityContributionIndex {
        &self.capability_index
    }

    pub(super) fn capability_specs(&self) -> &[CapabilitySpec] {
        &self.capability_specs
    }

    pub(super) fn activation_host(&self) -> Option<&Arc<NativeCapabilityHost>> {
        self.activation_host.as_ref()
    }

    pub(super) fn validate_restored_mask(
        &self,
        active: &[finstack_ai_kernel::ActiveCapability],
    ) -> Result<(), AgentRunError> {
        let Some(lock) = self.resolved.lock() else {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "native Agent requires an exact resolved lock",
            ));
        };
        for item in active {
            if !lock
                .capabilities
                .iter()
                .any(|capability| capability.id == item.capability_id)
            {
                return Err(AgentRunError::configuration(
                    AGENT_RUN_INVALID_CONFIGURATION,
                    "capability_mask_not_in_lock",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn live_tool_specs(
        &self,
        active: &[finstack_ai_kernel::ActiveCapability],
    ) -> Vec<finstack_ai_runtime::ToolSpec> {
        self.tools
            .tools()
            .filter(|tool| {
                match tool
                    .component
                    .as_ref()
                    .map(|invocation| &invocation.component)
                {
                    Some(component) => self.capability_index.allows(component, active),
                    None => true,
                }
            })
            .map(|tool| tool.spec.clone())
            .collect()
    }

    /// Return an agent configured for one compile-once Draft 2020-12 output schema.
    ///
    /// # Arguments
    ///
    /// * `schema` - Canonical JSON Schema document compiled by the offline Rust validator.
    ///
    /// # Errors
    ///
    /// Returns a stable configuration error when the schema cannot be compiled
    /// by the canonical offline Rust validator.
    pub fn try_with_output_schema(mut self, schema: &RawJson) -> Result<Self, AgentRunError> {
        let validator = JsonSchemaToolValidatorCompiler
            .compile(schema, &BTreeMap::new())
            .map_err(|error| {
                AgentRunError::configuration(
                    AGENT_RUN_INVALID_CONFIGURATION,
                    format!("structured output schema is invalid: {}", error.message()),
                )
            })?;
        let schema_ref = SchemaRef {
            draft: JsonSchemaDraft::Draft202012,
            schema_version: 1,
            schema_digest: Digest::raw_json(schema.as_bytes()),
        };
        self.structured_output = Some(StructuredOutputConfig {
            schema_ref,
            validator,
        });
        let output = self.structured_output.clone();
        let mut model_capabilities = self.model_capabilities.to_vec();
        for variant in &mut model_capabilities {
            variant.agent.structured_output.clone_from(&output);
        }
        self.model_capabilities = model_capabilities.into();
        Ok(self)
    }

    /// Borrow the exact resolved composition retained by this facade.
    #[must_use]
    pub fn resolved(&self) -> &Arc<ResolvedAgent> {
        &self.resolved
    }

    /// Return the bounded model-activated capability catalog in identity order.
    #[must_use]
    pub fn capability_catalog(&self) -> Vec<CapabilityCatalogEntry> {
        self.model_capabilities
            .iter()
            .map(|variant| variant.entry.clone())
            .collect()
    }

    /// Render the compact catalog supplied to model-facing integrations.
    #[must_use]
    pub fn compact_capability_catalog(&self) -> String {
        self.model_capabilities
            .iter()
            .map(|variant| format!("{}: {}", variant.entry.id, variant.entry.description))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Start one bounded native run and return its detached control handle.
    ///
    /// The caller must already be inside the selected runtime driver. Dropping the returned
    /// handle detaches frontend observation; it does not cancel the durable run.
    /// `request.capability` selects a model-activated variant; `None` runs `self`.
    /// An unknown catalog id fails closed. User-input word overlap is not used.
    ///
    /// # Arguments
    ///
    /// * `request` - Bounded run input, security context, limits, and optional capability id.
    ///
    /// # Errors
    ///
    /// Returns a stable configuration or runtime error before the background
    /// run task is accepted.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// # async fn demo(agent: &finstack_ai::Agent, request: finstack_ai::AgentRunRequest)
    /// #     -> Result<(), finstack_ai::AgentRunError> {
    /// let run = agent.start(request)?;
    /// let output = run.result().await?;
    /// let _ = output.text();
    /// # Ok(())
    /// # }
    /// ```
    pub fn start(&self, request: AgentRunRequest) -> Result<AgentRun, AgentRunError> {
        let selected = self.select_for_request(&request)?.clone();
        let prepared = selected.prepare(request)?;
        Self::spawn_prepared(selected.clone(), prepared)
    }

    /// Start a new root run on an existing idle lane.
    ///
    /// # Arguments
    ///
    /// * `lane` - Idle lane that already belongs to this agent's journal store.
    /// * `request` - Bounded run input; `capability` selects a model-activated variant.
    ///
    /// # Errors
    ///
    /// Returns a busy-lane or configuration/runtime failure.
    pub fn start_on_lane(
        &self,
        lane: &crate::Lane,
        request: AgentRunRequest,
    ) -> Result<AgentRun, AgentRunError> {
        let selected = self.select_for_request(&request)?.clone();
        let prepared = selected.prepare_on(request, lane)?;
        Self::spawn_prepared(selected.clone(), prepared)
    }

    /// Start one run on a frozen locator and already-validated acceptance.
    ///
    /// Used by child accept after a durable `ChildRunPrepared` mapping exists.
    ///
    /// # Errors
    ///
    /// Returns a configuration or runtime failure when the child cannot start.
    pub(super) fn start_prepared(
        &self,
        request: AgentRunRequest,
        locator: finstack_ai_kernel::OperationLocator,
        accepted: finstack_ai_kernel::RunAccepted,
        session: crate::Session,
    ) -> Result<AgentRun, AgentRunError> {
        let selected = self.select_for_request(&request)?.clone();
        let prepared = selected.prepare_accepted(request, locator, accepted, session)?;
        Self::spawn_prepared(selected.clone(), prepared)
    }

    fn spawn_prepared(
        agent: Self,
        prepared: super::prepare::PreparedAgentRun,
    ) -> Result<AgentRun, AgentRunError> {
        let locator = prepared.locator.clone();
        let store = Arc::clone(&prepared.store);
        let cancellation_initiator = prepared.cancellation_initiator()?;
        let child_runs = agent
            .resolved
            .spec()
            .map(|spec| spec.policy.child_runs)
            .unwrap_or_default();
        let inner = Arc::new(AgentRunInner {
            locator,
            store,
            child_runs,
            child_invoker_starts: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            cancellation_initiator,
            handle: Mutex::new(None),
            handle_ready: driver::Signal::new(),
            result: Mutex::new(None),
            result_ready: driver::Signal::new(),
            events: Mutex::new(EventStreamState::Waiting),
            events_fault: OnceLock::new(),
            cancellation: Mutex::new(CancellationState::default()),
            cancellation_ready: driver::Signal::new(),
            children: Mutex::new(Vec::new()),
        });
        let execution = Arc::downgrade(&inner);
        driver::spawn(Box::pin(async move {
            let result = Box::pin(agent.execute_started(prepared, &execution)).await;
            publish_result(&execution, result);
        }))
        .map_err(|error| AgentRunError::runtime_message(error.to_string()))?;
        Ok(AgentRun { inner })
    }

    /// Borrow the resolved journal store.
    #[must_use]
    pub fn journal_store(&self) -> Arc<dyn finstack_ai_runtime::JournalStore> {
        Arc::clone(self.resolved.run_plan().store().handle())
    }

    fn select_for_request(&self, request: &AgentRunRequest) -> Result<&Self, AgentRunError> {
        let Some(capability) = request.capability.as_ref() else {
            return Ok(self);
        };
        self.model_capabilities
            .iter()
            .find(|variant| variant.entry.id == *capability)
            .map(|variant| &variant.agent)
            .ok_or_else(|| {
                AgentRunError::configuration(
                    AGENT_RUN_INVALID_CONFIGURATION,
                    format!("unknown model capability {capability}"),
                )
            })
    }

    /// Execute one bounded native run through the commit-before-effect runtime.
    ///
    /// Equivalent to [`Self::start`] followed by [`AgentRun::result`].
    /// `request.capability` selects a model-activated variant; `None` runs `self`.
    ///
    /// # Arguments
    ///
    /// * `request` - Bounded run input, security context, limits, and optional capability id.
    ///
    /// # Errors
    ///
    /// Returns a stable configuration, runtime, cancellation, or timeout error.
    pub async fn run(&self, request: AgentRunRequest) -> Result<AgentRunOutput, AgentRunError> {
        self.start(request)?.result().await
    }
}
