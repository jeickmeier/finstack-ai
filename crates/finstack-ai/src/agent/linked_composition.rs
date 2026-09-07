//! Shared linked composition and explicit journal selection.

use super::builder::NativeAgentBuilder;
#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
use super::handle::Agent;
#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
use super::linked::component;
use super::linked::{LinkedAgent, LinkedCommon};
#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
use super::types::AGENT_RUN_INVALID_CONFIGURATION;
use super::types::AgentRunError;
use crate::RunPolicy;
#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
use finstack_ai_kernel::ComponentRef;
#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
use finstack_ai_kernel::{AgentId, BundleId};
use finstack_ai_runtime::ports::model::{ModelName, ModelSettings};
#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
use finstack_ai_runtime::ports::{journal::JournalStore, model::Model};
#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
use std::sync::Arc;
use std::time::Duration;

impl NativeAgentBuilder {
    /// Apply binding-resolved ports and finish as a [`LinkedAgent`].
    ///
    /// This is the single finish path for linked-provider constructors and
    /// host-handle factories. It does not read environment variables.
    ///
    /// # Errors
    ///
    /// Returns [`crate::AGENT_RUN_INVALID_CONFIGURATION`] when composition,
    /// lock, or output-schema compilation fails.
    pub async fn build_linked(
        mut self,
        common: LinkedCommon,
        model_name: ModelName,
        settings: ModelSettings,
        default_timeout: Duration,
    ) -> Result<LinkedAgent, AgentRunError> {
        let LinkedCommon {
            instruction,
            journal_store,
            capabilities,
            active_capabilities,
            ports,
            child_runs,
            approval_grant,
        } = common;
        if let Some(store) = journal_store {
            self.store = store;
        }
        for (component, toolset) in ports.toolsets {
            self = self.toolset(component, toolset);
        }
        for provider in ports.context_providers {
            self = self.context_provider(provider);
        }
        for middleware in ports.middleware {
            self = self.middleware(middleware);
        }
        for observer in ports.observers {
            self = self.observer(observer);
        }
        if let Some(store) = ports.artifact_store.clone() {
            self = self.artifact_store(store);
        }
        if let Some(instruction) = instruction {
            self = self.try_instruction(instruction)?;
        }
        for capability in capabilities {
            self = self.capability(capability);
        }
        for capability in active_capabilities {
            self = self.activate_application(capability);
        }
        self = self.policy(RunPolicy {
            child_runs,
            approval_grant,
            ..RunPolicy::default()
        });
        let agent = self.build().await?;
        let agent = if let Some(schema) = ports.output_schema {
            agent.try_with_output_schema(&schema)?
        } else {
            agent
        };
        Ok(LinkedAgent {
            agent,
            model: model_name,
            settings,
            default_timeout,
        })
    }
}

#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
pub(super) async fn build_linked_provider(
    (agent_id, bundle_id, model_id): (&str, &str, &str),
    provider: Arc<dyn Model>,
    model_name: ModelName,
    mut common: LinkedCommon,
    settings: ModelSettings,
    default_timeout: Duration,
) -> Result<LinkedAgent, AgentRunError> {
    let journal = match common.journal_store.take() {
        Some(journal) => journal,
        None => memory_store()?,
    };
    Agent::builder(
        AgentId::parse(agent_id).map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?,
        BundleId::parse(bundle_id).map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?,
        (component(model_id)?, provider),
        journal,
    )
    .build_linked(common, model_name, settings, default_timeout)
    .await
}

#[cfg(any(
    feature = "provider-openai",
    feature = "provider-openrouter",
    feature = "provider-anthropic",
    feature = "provider-gemini",
    feature = "provider-ollama"
))]
fn memory_store() -> Result<(ComponentRef, Arc<dyn JournalStore>), AgentRunError> {
    use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};

    let store = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 64,
            batches_per_session: 256,
            records_per_session: 4_096,
            snapshot_bytes: 64 * 1_024,
        })
        .map_err(|error| {
            AgentRunError::configuration(AGENT_RUN_INVALID_CONFIGURATION, error.to_string())
        })?,
    );
    Ok((component("python.store.memory")?, store))
}
