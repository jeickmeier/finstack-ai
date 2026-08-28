//! Composition of the knowledge agent from released components.
//!
//! Everything here is assembly: no port implementations, no kernel or
//! runtime changes. The composition mirrors the parity checklist in the
//! crate README; the Python and TypeScript surfaces re-state it in their
//! own languages.

use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};

use finstack_ai::runtime::ports::journal::JournalStore;
use finstack_ai::runtime::ports::model::{Model, ModelName};
use finstack_ai::runtime::ports::observer::ObserverPayloadMode;
use finstack_ai::{Agent, NativeCapabilityHost};
use finstack_ai_kernel::{CapabilityId, ComponentId, ComponentRef, Version};
use finstack_ai_memory::extract::RuleBasedExtractor;
use finstack_ai_memory::observer::MemoryObserver;
use finstack_ai_memory::provider::{MemoryContextProvider, RecallConfig};
use finstack_ai_memory::record::{MemoryScope, system_clock};
use finstack_ai_memory::store::{MemoryStore, SqliteMemoryStore};
use finstack_ai_memory::toolset::{MemoryPolicy, MemoryToolset};
use finstack_ai_middleware_compaction::{CompactionConfig, CompactionMiddleware};
use finstack_ai_middleware_document_ingest::DocumentIngestMiddleware;
use finstack_ai_middleware_instructions::{
    InstructionsMiddleware, PolicyEntry, PolicyInstructionsConfig,
};
use finstack_ai_provider_anthropic::{AnthropicConfig, AnthropicModelConfig, AnthropicProvider};
use finstack_ai_provider_ollama::{OllamaConfig, OllamaModelConfig, OllamaProvider};
use finstack_ai_provider_openai::{OpenAiConfig, OpenAiModelConfig, OpenAiProvider};
use finstack_ai_provider_openrouter::{
    OpenRouterConfig, OpenRouterModelConfig, OpenRouterProvider,
};
use finstack_ai_store_sqlite::{
    SqliteDurability, SqliteJournalStore, SqliteStoreConfig, SqliteStoreLimits,
};
use finstack_ai_tools_document::DocumentToolset;
use finstack_ai_tools_fetch::{HostPattern, HttpFetchConfig, HttpFetchToolset};
use finstack_ai_tools_skills::{SkillsHost, SkillsHostError, SkillsToolset};

use crate::config::{KnowledgeConfig, KnowledgeError, ProviderChoice};
use crate::docs::materialize_self_docs;

/// Exact component version for every `finstack.know.*` registration.
const KNOW_VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};

/// Citation skill identity shown in the capability catalog.
const CITATIONS_CAPABILITY: &str = "finstack.know.skill.citations";
const CITATIONS_DESCRIPTION: &str = "Citation discipline for retrieved sources";
const CITATIONS_INSTRUCTION: &str = "When answering from retrieved documents or memory, \
name the source (document name or memory record) for each claim and quote sparingly.";

/// Base instruction every surface shares.
const BASE_INSTRUCTION: &str = "You are the finstack knowledge assistant. Answer from \
ingested documents, remembered facts, and the bundled self-docs; say when you do not \
know. Prefer citing sources by name. Use registered tools when they help.";

/// Provider-neutral model context bounds (bytes, context, output, reserve, overhead).
const MODEL_BOUNDS: (u64, u64, u64, u64, u64) = (1_048_576, 131_072, 8_192, 8_192, 64);

/// Sliding-window compaction thresholds in tokens (threshold, hysteresis).
const COMPACTION_TOKENS: (u64, u64) = (120_000, 24_000);

/// The model name the configured provider advertises.
///
/// # Errors
///
/// Returns [`KnowledgeError::Compose`] when the configured name is not a
/// valid model name.
pub fn model_name(config: &KnowledgeConfig) -> Result<ModelName, KnowledgeError> {
    let name = match &config.provider {
        ProviderChoice::Ollama { model, .. }
        | ProviderChoice::Anthropic { model, .. }
        | ProviderChoice::OpenAi { model, .. }
        | ProviderChoice::OpenRouter { model, .. } => model,
    };
    ModelName::try_new(name).map_err(compose_error)
}

/// Build the composed knowledge agent for one data directory.
///
/// Opens only `config.data_dir` (journal, memory store, self-docs); never
/// reads the environment or the network.
///
/// # Errors
///
/// Returns [`KnowledgeError`] when the data directory is unusable, a fetch
/// allowlist pattern is invalid, or a released component rejects its
/// composition inputs.
pub async fn build_agent(config: &KnowledgeConfig) -> Result<Agent, KnowledgeError> {
    let journal = open_journal(config)?;
    build_agent_with_journal(config, journal).await
}

/// Open the shared sqlite journal at `<data_dir>/journal.sqlite3`.
///
/// The CLI session commands and the agent composition share this store, so
/// sessions created by one are visible to the other (and to the Python
/// notebooks pointed at the same file).
///
/// # Errors
///
/// Returns [`KnowledgeError`] when the data directory or the store cannot
/// be opened.
pub fn open_journal(config: &KnowledgeConfig) -> Result<Arc<dyn JournalStore>, KnowledgeError> {
    Ok(open_journal_sqlite(config)? as Arc<dyn JournalStore>)
}

/// Open the shared journal as the concrete sqlite store.
///
/// The CLI's `sessions list` uses the store's `list_sessions` convenience,
/// which lives beyond the `JournalStore` port.
///
/// # Errors
///
/// As for [`open_journal`].
pub fn open_journal_sqlite(
    config: &KnowledgeConfig,
) -> Result<Arc<SqliteJournalStore>, KnowledgeError> {
    std::fs::create_dir_all(&config.data_dir).map_err(|_| KnowledgeError::Config {
        reason: "data_dir_unwritable",
    })?;
    Ok(Arc::new(
        SqliteJournalStore::try_open(SqliteStoreConfig::new(
            config.data_dir.join("journal.sqlite3"),
            SqliteDurability::Durable,
            journal_limits(),
        ))
        .map_err(compose_error)?,
    ))
}

/// Build the composed knowledge agent over an already-open journal store.
///
/// # Errors
///
/// Returns [`KnowledgeError`] as for [`build_agent`].
pub async fn build_agent_with_journal(
    config: &KnowledgeConfig,
    journal: Arc<dyn JournalStore>,
) -> Result<Agent, KnowledgeError> {
    std::fs::create_dir_all(&config.data_dir).map_err(|_| KnowledgeError::Config {
        reason: "data_dir_unwritable",
    })?;
    let self_docs_root = materialize_self_docs(&config.data_dir)?;

    let provider = provider(config)?;

    let artifact_store: Arc<dyn finstack_ai::runtime::artifact::ArtifactStore> =
        Arc::new(finstack_ai::runtime::artifact::InProcessArtifactStore::default());

    let (memory_provider, memory_toolset, memory_observer) =
        memory_components(config, &artifact_store)?;

    // Capability catalog: the citations skill, activatable by the model
    // through the skills toolset.
    let capability_host = Arc::new(NativeCapabilityHost::new(format!(
        "{CITATIONS_CAPABILITY}: {CITATIONS_DESCRIPTION}"
    )));
    let skills = SkillsToolset::try_new(skills_host(&capability_host)).map_err(compose_error)?;
    let citations = citations_capability()?;

    let mut builder = Agent::builder(
        finstack_ai_kernel::AgentId::parse("finstack.know.agent").map_err(compose_error)?,
        finstack_ai_kernel::BundleId::parse("finstack.know.bundle").map_err(compose_error)?,
        (component("finstack.know.model")?, provider),
        (component("finstack.know.store.journal")?, journal),
    )
    .try_instruction(BASE_INSTRUCTION)
    .map_err(compose_error)?
    .artifact_store(Arc::clone(&artifact_store))
    // Port extensions with self-identifying descriptors must be registered
    // under their declared identity, not a finstack.know.* alias.
    .context_provider(
        versioned("finstack.context.repository", 0, 0, 4)?,
        Arc::new(repository_provider(&self_docs_root)?),
    )
    .context_provider(
        versioned("finstack.context.memory", 0, 1, 0)?,
        Arc::new(memory_provider),
    )
    .middleware(
        versioned("finstack.middleware.instructions", 1, 0, 0)?,
        Arc::new(instructions_middleware()?),
    )
    .middleware(
        versioned("finstack.middleware.document-ingest", 1, 0, 0)?,
        Arc::new(DocumentIngestMiddleware::try_new(Arc::clone(&artifact_store))
            .map_err(compose_error)?),
    )
    .middleware(
        versioned("finstack.middleware.compaction", 0, 0, 4)?,
        Arc::new(
            CompactionMiddleware::try_new(CompactionConfig::sliding_window(
                COMPACTION_TOKENS.0,
                COMPACTION_TOKENS.1,
            ))
            .map_err(compose_error)?,
        ),
    )
    .toolset(
        component("finstack.know.tools.document")?,
        Arc::new(DocumentToolset::try_new(Arc::clone(&artifact_store)).map_err(compose_error)?),
    )
    .toolset(component("finstack.know.tools.memory")?, Arc::new(memory_toolset))
    .toolset(component("finstack.know.tools.skills")?, Arc::new(skills))
    .observer(
        versioned("finstack.observer.log", 0, 0, 4)?,
        Arc::new(log_observer()?),
    )
    .observer(
        versioned("finstack.observer.memory", 0, 1, 0)?,
        Arc::new(memory_observer),
    )
    .capability(citations)
    .capability_activation_host(capability_host);

    if let Some(project_root) = &config.project_root {
        builder = builder.context_provider(
            versioned("finstack.context.repository", 0, 0, 4)?,
            Arc::new(repository_provider(project_root)?),
        );
    }

    if !config.fetch_allowlist.is_empty() {
        builder = builder.toolset(
            component("finstack.know.tools.fetch")?,
            Arc::new(fetch_toolset(&config.fetch_allowlist)?),
        );
    }

    builder.build().await.map_err(compose_error)
}

/// Memory: one sqlite store behind the recall provider, toolset, and
/// capture observer.
fn memory_components(
    config: &KnowledgeConfig,
    artifact_store: &Arc<dyn finstack_ai::runtime::artifact::ArtifactStore>,
) -> Result<(MemoryContextProvider, MemoryToolset, MemoryObserver), KnowledgeError> {
    let memory_store: Arc<dyn MemoryStore> = Arc::new(
        SqliteMemoryStore::try_open(&config.data_dir.join("memory.sqlite3"))
            .map_err(compose_error)?,
    );
    let scope = MemoryScope::try_new("local").map_err(compose_error)?;
    let provider = MemoryContextProvider::try_new(
        Arc::clone(&memory_store),
        artifact_store.as_ref(),
        scope.clone(),
        RecallConfig::default(),
    )
    .map_err(compose_error)?;
    let toolset = MemoryToolset::try_new(
        Arc::clone(&memory_store),
        Arc::clone(artifact_store),
        scope.clone(),
        MemoryPolicy::default(),
        system_clock(),
    )
    .map_err(compose_error)?;
    let observer = MemoryObserver::try_new(
        memory_store,
        scope,
        Arc::new(RuleBasedExtractor::default()),
        system_clock(),
    )
    .map_err(compose_error)?;
    Ok((provider, toolset, observer))
}

fn provider(config: &KnowledgeConfig) -> Result<Arc<dyn Model>, KnowledgeError> {
    let (bytes, context, output, reserve, overhead) = MODEL_BOUNDS;
    match &config.provider {
        ProviderChoice::Ollama { base_url, model } => Ok(Arc::new(
            OllamaProvider::try_new(
                OllamaConfig::try_new(base_url).map_err(compose_error)?,
                vec![
                    OllamaModelConfig::try_new(model, bytes, context, output, reserve, overhead)
                        .map_err(compose_error)?,
                ],
            )
            .map_err(compose_error)?,
        )),
        ProviderChoice::Anthropic { api_key, model } => Ok(Arc::new(
            AnthropicProvider::try_new(
                AnthropicConfig::try_new("https://api.anthropic.com")
                    .map_err(compose_error)?
                    .with_authentication(api_key_auth(api_key)?)
                    .map_err(compose_error)?,
                vec![
                    AnthropicModelConfig::try_new(
                        model, bytes, context, output, reserve, overhead,
                    )
                    .map_err(compose_error)?,
                ],
            )
            .map_err(compose_error)?,
        )),
        ProviderChoice::OpenAi { api_key, model } => Ok(Arc::new(
            OpenAiProvider::try_new(
                OpenAiConfig::try_new("https://api.openai.com")
                    .map_err(compose_error)?
                    .with_authentication(bearer_auth(api_key)?)
                    .map_err(compose_error)?,
                vec![
                    OpenAiModelConfig::try_new(model, bytes, context, output, reserve, overhead)
                        .map_err(compose_error)?,
                ],
            )
            .map_err(compose_error)?,
        )),
        ProviderChoice::OpenRouter { api_key, model } => Ok(Arc::new(
            OpenRouterProvider::try_new(
                OpenRouterConfig::try_new("https://openrouter.ai")
                    .map_err(compose_error)?
                    .with_authentication(bearer_auth(api_key)?)
                    .map_err(compose_error)?,
                vec![
                    OpenRouterModelConfig::try_new(
                        model, bytes, context, output, reserve, overhead,
                    )
                    .map_err(compose_error)?,
                ],
            )
            .map_err(compose_error)?,
        )),
    }
}

fn api_key_auth(
    api_key: &str,
) -> Result<finstack_ai::runtime::ports::model::Authentication, KnowledgeError> {
    Ok(finstack_ai::runtime::ports::model::Authentication::ApiKey(
        finstack_ai::runtime::ports::model::SecretString::try_new(api_key)
            .map_err(|_| KnowledgeError::Config {
                reason: "api_key_invalid",
            })?,
    ))
}

fn bearer_auth(
    api_key: &str,
) -> Result<finstack_ai::runtime::ports::model::Authentication, KnowledgeError> {
    Ok(finstack_ai::runtime::ports::model::Authentication::Bearer(
        finstack_ai::runtime::ports::model::SecretString::try_new(api_key)
            .map_err(|_| KnowledgeError::Config {
                reason: "api_key_invalid",
            })?,
    ))
}

fn repository_provider(
    root: &Path,
) -> Result<finstack_ai_context_repository::RepositoryContextProvider, KnowledgeError> {
    finstack_ai_context_repository::RepositoryContextProvider::try_new(root)
        .map_err(compose_error)
}

fn instructions_middleware() -> Result<InstructionsMiddleware, KnowledgeError> {
    InstructionsMiddleware::try_new(PolicyInstructionsConfig {
        entries: vec![PolicyEntry {
            label: "knowledge-grounding".to_owned(),
            text: "Ground answers in retrieved context; never invent citations.".to_owned(),
        }],
    })
    .map_err(compose_error)
}

fn fetch_toolset(allowlist: &[String]) -> Result<HttpFetchToolset, KnowledgeError> {
    for pattern in allowlist {
        HostPattern::parse(pattern).map_err(|_| KnowledgeError::Config {
            reason: "fetch_allowlist_pattern_invalid",
        })?;
    }
    HttpFetchToolset::try_new(HttpFetchConfig {
        allowlist: allowlist.to_vec(),
        ..HttpFetchConfig::default()
    })
    .map_err(compose_error)
}

fn log_observer() -> Result<finstack_ai_observer_log::LogObserver, KnowledgeError> {
    let sink: Arc<Mutex<dyn Write + Send>> = Arc::new(Mutex::new(std::io::stderr()));
    finstack_ai_observer_log::LogObserver::try_new(ObserverPayloadMode::MetadataOnly, sink)
        .map_err(compose_error)
}

fn citations_capability() -> Result<finstack_ai::runtime::spec::CapabilitySpec, KnowledgeError> {
    Ok(finstack_ai::runtime::spec::CapabilitySpec {
        id: CapabilityId::parse(CITATIONS_CAPABILITY).map_err(compose_error)?,
        description: Arc::from(CITATIONS_DESCRIPTION),
        activation: finstack_ai::runtime::spec::CapabilityActivation::Model,
        instructions: Arc::from([finstack_ai::runtime::spec::InstructionSpec::try_new(
            CITATIONS_INSTRUCTION,
        )
        .map_err(compose_error)?]),
        context_providers: Arc::from([]),
        middleware: Arc::from([]),
        toolsets: Arc::from([]),
    })
}

fn skills_host(host: &Arc<NativeCapabilityHost>) -> SkillsHost {
    let catalog = host.compact_catalog().to_owned();
    let active = Arc::clone(host);
    let activate = Arc::clone(host);
    SkillsHost {
        catalog,
        active: Arc::new(move |run| {
            active.active(run).map_err(|error| SkillsHostError::Failed {
                reason: error.to_string().into(),
            })
        }),
        activate: Arc::new(move |run, complete| {
            activate
                .queue_activation(run, complete)
                .map_err(|error| SkillsHostError::Failed {
                    reason: error.to_string().into(),
                })
        }),
    }
}

fn journal_limits() -> SqliteStoreLimits {
    SqliteStoreLimits {
        sessions: 1_024,
        batches_per_session: 16_384,
        records_per_session: 131_072,
        snapshot_bytes: 4 * 1_024 * 1_024,
    }
}

fn component(id: &str) -> Result<ComponentRef, KnowledgeError> {
    Ok(ComponentRef::new(
        ComponentId::parse(id).map_err(compose_error)?,
        Some(KNOW_VERSION),
    ))
}

/// Reference a component under the exact identity its descriptor declares.
fn versioned(id: &str, major: u16, minor: u16, patch: u16) -> Result<ComponentRef, KnowledgeError> {
    Ok(ComponentRef::new(
        ComponentId::parse(id).map_err(compose_error)?,
        Some(Version {
            major,
            minor,
            patch,
        }),
    ))
}

fn compose_error(error: impl std::fmt::Display) -> KnowledgeError {
    KnowledgeError::Compose {
        reason: error.to_string(),
    }
}
