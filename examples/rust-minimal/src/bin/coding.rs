//! Public composition of coding/research batteries over a keyless loopback model.

#![forbid(unsafe_code)]
#![warn(clippy::float_cmp)]
#![deny(clippy::unwrap_used)]
#![deny(clippy::expect_used)]
#![deny(clippy::panic)]
#![deny(clippy::unreachable)]
#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::indexing_slicing,
        clippy::float_cmp,
    )
)]
// Allow expect() in doc tests (they are test code)
#![doc(test(attr(allow(clippy::expect_used))))]

use std::path::PathBuf;
use std::sync::Arc;

use finstack_ai::runtime::artifact::InProcessArtifactStore;
use finstack_ai::runtime::ports::context::ContextProvider;
use finstack_ai::runtime::ports::journal::JournalStore;
use finstack_ai::runtime::ports::middleware::Middleware;
use finstack_ai::runtime::ports::model::{Model, ModelName};
use finstack_ai::runtime::ports::tool::Toolset;
use finstack_ai::{Agent, AgentRunRequest};
use finstack_ai_context_repository::RepositoryContextProvider;
use finstack_ai_kernel::{
    AgentId, BundleId, ComponentId, ComponentRef, Duration, RawJson, Version,
};
use finstack_ai_memory::provider::{MemoryContextProvider, RecallConfig};
use finstack_ai_memory::record::MemoryScope;
use finstack_ai_memory::store::InProcessMemoryStore;
use finstack_ai_middleware_compaction::{CompactionConfig, CompactionMiddleware};
use finstack_ai_middleware_verify::{EvidenceVerifier, Verdict, VerifyMiddleware, VerifyPolicy};
use finstack_ai_native_examples::{
    BoxError, PREVIEW_VERSION, calculator_response, security, serve_ndjson, text_response,
};
use finstack_ai_provider_ollama::{OllamaConfig, OllamaModelConfig, OllamaProvider};
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_tools_calculator::CalculatorToolset;
use finstack_ai_tools_filesystem::FileSystemToolset;
use finstack_ai_tools_shell::{ShellPolicy, ShellToolset};

/// Trivial pass-through verifier: always lands the candidate.
#[derive(Debug)]
struct AlwaysAcceptVerifier;

impl EvidenceVerifier for AlwaysAcceptVerifier {
    fn verifier_id(&self) -> &'static str {
        "example.always-accept"
    }

    fn verify(&self, _message: &RawJson) -> Verdict {
        Verdict::Accept
    }
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
    let root = std::env::args_os()
        .nth(1)
        .map_or_else(|| PathBuf::from("."), PathBuf::from);

    let calculator: Arc<dyn Toolset> = Arc::new(CalculatorToolset::try_new()?);
    let filesystem = FileSystemToolset::try_new(&root);
    println!(
        "filesystem tools: {}",
        filesystem.as_ref().map_or(0, |value| value.tools().len())
    );
    let shell_policy = ShellPolicy::try_new(["/bin/echo"])?.with_locale_env();
    let shell = ShellToolset::try_new(shell_policy, Some(&root));
    println!(
        "shell tools: {}",
        shell.as_ref().map_or(0, |value| value.tools().len())
    );
    let repository = RepositoryContextProvider::try_new(&root);
    println!(
        "repository context: {}",
        if repository.is_ok() {
            "ready"
        } else {
            "unavailable"
        }
    );
    let store: Arc<dyn JournalStore> = Arc::new(MemoryJournalStore::try_new(MemoryStoreLimits {
        sessions: 8,
        batches_per_session: 128,
        records_per_session: 1_024,
        snapshot_bytes: 16_384,
    })?);
    let memory = MemoryScope::try_new("preview-local").and_then(|scope| {
        MemoryContextProvider::try_new(
            Arc::new(InProcessMemoryStore::new()),
            &InProcessArtifactStore::default(),
            scope,
            RecallConfig::default(),
        )
    });
    let compaction: Arc<dyn Middleware> = Arc::new(CompactionMiddleware::try_new(
        CompactionConfig::sliding_window(8_192, 256),
    )?);
    let verify: Arc<dyn Middleware> = Arc::new(VerifyMiddleware::try_new(
        Arc::new(AlwaysAcceptVerifier),
        VerifyPolicy::try_new(Duration::from_millis(0), "example-v1")?,
    )?);

    let (base_url, server) = serve_ndjson(vec![
        calculator_response(),
        text_response("five", "tool-final"),
    ])
    .await?;
    let model_name = ModelName::try_new("preview-model")?;
    let provider: Arc<dyn Model> = Arc::new(OllamaProvider::try_new(
        OllamaConfig::try_new(base_url)?,
        vec![OllamaModelConfig::try_new(
            model_name.as_ref(),
            1_048_576,
            1_048_576,
            512,
            512,
            64,
        )?],
    )?);

    let mut builder = Agent::builder(
        AgentId::parse("preview.agent.coding")?,
        BundleId::parse("preview.bundle.coding")?,
        (component("preview.model.ollama")?, provider),
        (component("preview.store.memory")?, store),
    )
    .try_instruction("Answer directly and use registered tools when helpful.")?
    .toolset(component("preview.tools.calculator")?, calculator);
    if let Ok(filesystem) = filesystem {
        builder = builder.toolset(component("preview.tools.filesystem")?, Arc::new(filesystem));
    }
    if let Ok(shell) = shell {
        builder = builder.toolset(component("preview.tools.shell")?, Arc::new(shell));
    }
    if let Ok(repository) = repository {
        let provider: Arc<dyn ContextProvider> = Arc::new(repository);
        builder = builder.context_provider(leaf("finstack.context.repository")?, provider);
    }
    if let Ok(memory) = memory {
        let provider: Arc<dyn ContextProvider> = Arc::new(memory);
        builder = builder.context_provider(leaf("finstack.context.memory")?, provider);
    }
    builder = builder
        .middleware(leaf("finstack.middleware.compaction")?, compaction)
        .middleware(leaf("finstack.middleware.verify")?, verify);

    let agent = builder.build().await?;
    let output = agent
        .run(AgentRunRequest::try_new(
            model_name,
            "What is two plus three?",
            security()?,
        )?)
        .await?;
    server.await.map_err(|error| error.to_string())??;
    println!("coding result: {}", output.text());
    Ok(())
}

fn component(id: &str) -> Result<ComponentRef, BoxError> {
    Ok(ComponentRef::new(
        ComponentId::parse(id)?,
        Some(PREVIEW_VERSION),
    ))
}

fn leaf(id: &str) -> Result<ComponentRef, BoxError> {
    Ok(ComponentRef::new(
        ComponentId::parse(id)?,
        Some(Version {
            major: 0,
            minor: 0,
            patch: 4,
        }),
    ))
}
