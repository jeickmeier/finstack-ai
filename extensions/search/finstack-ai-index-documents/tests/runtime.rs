//! Committed SDK tool effects execute real indexing and federated retrieval.
use finstack_ai::{Agent, AgentRunRequest};
use finstack_ai_index_documents::*;
use finstack_ai_kernel::*;
use finstack_ai_runtime::Bytes;
use finstack_ai_runtime::artifact::*;
use finstack_ai_runtime::ports::{
    PortFuture,
    context::*,
    journal::*,
    model::*,
    tool::{ToolCallContext, ToolStreamItem, Toolset},
};
use finstack_ai_search::*;
use finstack_ai_store_artifact::LocalArtifactStore;
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_test::{ScriptedModel, ScriptedModelAction, ScriptedModelPlan};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

fn profile() -> ModelContextProfile {
    ModelContextProfile {
        provider: "scripted".into(),
        model: ModelName::try_new("search-test").unwrap(),
        hard_input_bytes: 1_048_576,
        context_window_tokens: 1_048_576,
        max_output_tokens: 4096,
        reserved_output_tokens: 4096,
        provider_overhead_tokens: 32,
        estimator: TokenEstimatorRef {
            id: "bytes-upper-bound".into(),
            version: "1".into(),
            source: TokenEstimatorSource::ConservativeUpperBound,
        },
    }
}
struct FixtureModel {
    artifacts: Arc<dyn ArtifactStore>,
    calls: AtomicUsize,
    requests: Arc<Mutex<Vec<ModelRequest>>>,
}
impl Model for FixtureModel {
    fn descriptor(&self) -> ModelDescriptor {
        ScriptedModel::from_plans(profile(), vec![]).descriptor()
    }
    fn capabilities(&self, name: &ModelName) -> ModelCapabilities {
        ScriptedModel::from_plans(profile(), vec![]).capabilities(name)
    }
    fn estimate_input_tokens(
        &self,
        name: &ModelName,
        bytes: &[u8],
    ) -> Result<ModelTokenEstimate, ModelError> {
        ScriptedModel::from_plans(profile(), vec![]).estimate_input_tokens(name, bytes)
    }
    fn request(&self, request: ModelRequest) -> PortFuture<Result<ModelEventStream, ModelError>> {
        self.requests.lock().unwrap().push(request.clone());
        let step = self.calls.fetch_add(1, Ordering::SeqCst);
        let artifacts = self.artifacts.clone();
        Box::pin(async move {
            let call = match step {
                0 | 1 => {
                    let locator = &request.call.run.locator;
                    let artifact = artifacts
                        .stage_put(
                            ArtifactScope {
                                tenant_scope: locator.tenant_scope.clone(),
                                session_id: locator.session_id,
                                run_id: Some(locator.run_id),
                                sensitivity: Sensitivity::Internal,
                            },
                            Bytes::from_static(b"# Retention\n\nKeep records seven years."),
                            ArtifactMetadata {
                                kind: "document".into(),
                                media_type: "text/markdown".into(),
                                name: Some("handbook".into()),
                                attributes: Metadata::empty(),
                            },
                        )
                        .await
                        .unwrap();
                    Some(("index_document", serde_json::json!({"artifact":artifact})))
                }
                2 => Some((
                    "search",
                    serde_json::json!({"text":"retention","strategy":null,"sources":null,"limit":8}),
                )),
                _ => None,
            };
            ScriptedModel::from_plans(profile(), vec![plan(call, step)])
                .request(request)
                .await
        })
    }
}
fn plan(call: Option<(&str, serde_json::Value)>, step: usize) -> ScriptedModelPlan {
    let mut actions = Vec::new();
    let mut calls = Vec::new();
    let mut content = Vec::new();
    if let Some((name, value)) = call {
        let text = serde_json::to_string(&value).unwrap();
        calls.push(ModelToolCall {
            name: name.into(),
            arguments: RawJson::parse(text.as_bytes()).unwrap(),
            provider_call_id: None,
        });
        actions.push(ScriptedModelAction::Emit(Ok(
            ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 0,
                name: Some(name.into()),
                arguments_delta: text.into(),
                provider_call_id: None,
            }),
        )));
    } else {
        content.push(ContentBlock::Text(TextBlock::try_new("retrieved").unwrap()));
        actions.push(ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(
            TextDelta {
                text: "retrieved".into(),
            },
        ))));
    }
    actions.push(ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(
        ModelResponse {
            assistant_content: content.into(),
            tool_calls: calls.into(),
            usage: Usage::try_new(
                Some(20),
                Some(10),
                Some(30),
                None,
                std::collections::BTreeMap::default(),
            )
            .unwrap(),
            provider_ids: ProviderIds::empty(),
            completion_id: format!("fixture-{step}").into(),
            continuation_state: None,
        },
    ))));
    ScriptedModelPlan { actions }
}
fn component(id: &str) -> ComponentRef {
    ComponentRef::new(
        ComponentId::parse(id).unwrap(),
        Some(Version {
            major: 1,
            minor: 0,
            patch: 0,
        }),
    )
}
fn search_engine(source: Arc<DocumentSearchSource>) -> Arc<SearchEngine> {
    Arc::new(
        SearchEngine::try_new(
            SearchConfig {
                graph_expansion: None,
                scope: source.config().scope.clone(),
                default_plan: HybridPlan {
                    legs: vec![HybridLeg {
                        source: "documents".into(),
                        strategy: SearchStrategy::Lexical(LexicalKind::Keyword),
                        weight_micros: 1_000_000,
                    }],
                    fusion: Fusion::default(),
                },
                limits: SearchLimits::default(),
                max_concurrency: 1,
                source_timeout_ms: 1000,
            },
            vec![source],
        )
        .unwrap(),
    )
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One committed index/repeat/search lifecycle with journal proof.
async fn committed_index_effect_repeats_and_search_returns_real_receipt() {
    let dir = tempfile::tempdir().unwrap();
    let artifacts: Arc<dyn ArtifactStore> =
        Arc::new(LocalArtifactStore::try_new(dir.path().join("artifacts")).unwrap());
    let source = Arc::new(
        DocumentSearchSource::try_open(
            &dir.path().join("index.sqlite"),
            artifacts.clone(),
            DocumentIndexConfig::new("documents", SearchScope::try_new("tenant").unwrap()),
            None,
        )
        .unwrap(),
    );
    let engine = search_engine(source.clone());
    let model = Arc::new(FixtureModel {
        artifacts: artifacts.clone(),
        calls: AtomicUsize::new(0),
        requests: Arc::new(Mutex::new(vec![])),
    });
    let journal: Arc<dyn JournalStore> = Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 8,
            batches_per_session: 256,
            records_per_session: 4096,
            snapshot_bytes: 262_144,
        })
        .unwrap(),
    );
    let index_tools: Arc<dyn Toolset> =
        Arc::new(DocumentIndexToolset::try_new(source.clone()).unwrap());
    let search_tools: Arc<dyn Toolset> = Arc::new(SearchToolset::try_new(engine.clone()).unwrap());
    let agent = Agent::builder(
        AgentId::parse("test.search").unwrap(),
        BundleId::parse("test.bundle").unwrap(),
        (component("test.model"), model.clone()),
        (component("test.journal"), journal.clone()),
    )
    .artifact_store(artifacts)
    .toolset(component("test.index"), index_tools)
    .toolset(component("test.search.tools"), search_tools)
    .context_provider(Arc::new(
        SearchContextProvider::try_new(engine.clone(), 8).unwrap(),
    ))
    .build()
    .await
    .unwrap();
    let security = RunSecurityContext::try_new(
        "tenant",
        PrincipalRef::try_new("test", "developer", Some("tenant")).unwrap(),
        "local",
        "test",
        "v1",
        "v1",
        None,
    )
    .unwrap();
    let output_result = agent
        .run(
            AgentRunRequest::try_new(
                ModelName::try_new("search-test").unwrap(),
                "retention",
                security,
            )
            .unwrap(),
        )
        .await;
    let output = output_result.unwrap();
    assert_eq!(output.text(), "retrieved");
    let requests = model.requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 4);
    let results: Vec<_> = requests[3]
        .draft
        .messages
        .iter()
        .flat_map(Message::content)
        .filter_map(|block| match block {
            ContentBlock::ToolResult(result) => Some(result),
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 3);
    assert!(
        results.iter().all(|result| !result.is_error()),
        "{results:?}"
    );
    assert_eq!(results[0].content(), results[1].content());
    let encoded = serde_json::to_string(results[2].content()).unwrap();
    assert!(
        encoded.contains("artifact_chunk") && encoded.contains("Keep records seven years"),
        "{encoded}"
    );
    let loaded = journal
        .load(LoadRequest {
            session_id: output.locator.session_id,
        })
        .await
        .unwrap();
    let records: Vec<_> = loaded
        .committed_batches
        .iter()
        .flat_map(|b| b.records.iter())
        .collect();
    assert_eq!(
        records
            .iter()
            .filter(
                |r| matches!(r.body(),RecordBody::EffectRequested(e) if e.kind()==EffectKind::Tool)
            )
            .count(),
        3
    );
    assert_eq!(source.inputs(None, 8).await.unwrap().len(), 1);
    check_tools(engine.clone(), requests[3].call.run.clone()).await;
    check_context(engine, requests[3].call.run.clone()).await;
}

async fn check_context(engine: Arc<SearchEngine>, mut run: RunCallContext) {
    run.cancellation = CancellationSignal::new();
    let provider = SearchContextProvider::try_new(engine, 8).unwrap();
    assert!(!provider.descriptor().trusted_application_instructions);
    let ctx = ContextCallContext {
        run: run.clone(),
        provider_index: 0,
        chain_digest: Digest::blob_content(b"fixture"),
    };
    let mut request = ContextRequest {
        session_id: run.locator.session_id,
        lane_id: run.locator.lane_id,
        run_id: run.locator.run_id,
        user_input: vec![ContentBlock::Text(TextBlock::try_new("retention").unwrap())].into(),
        recent_history: vec![].into(),
        budget: ContextBudget {
            max_items: 16,
            max_tokens: 8192,
            max_bytes: 32_768,
            overflow: ContextOverflowPolicy::Reject,
        },
        active_capabilities: vec![].into(),
    };
    let full = provider
        .collect(ctx.clone(), request.clone())
        .await
        .unwrap();
    assert_eq!(full.items.len(), 2);
    assert!(
        full.items
            .iter()
            .all(|item| item.authority == ContextAuthority::Untrusted && item.provenance.external)
    );
    request.budget.max_items = 1;
    assert!(
        provider
            .collect(ctx.clone(), request.clone())
            .await
            .is_err()
    );
    request.budget.overflow = ContextOverflowPolicy::TruncateWithDiagnostic;
    let partial = provider
        .collect(ctx.clone(), request.clone())
        .await
        .unwrap();
    assert_eq!(partial.items.len(), 1);
    assert!(
        serde_json::to_string(&partial.items)
            .unwrap()
            .contains("context_omitted_hits")
    );
    request.budget.max_bytes = 1;
    assert!(
        provider
            .collect(ctx.clone(), request.clone())
            .await
            .is_err()
    );
    let mut denied = ctx;
    denied.run.locator.tenant_scope = "other".into();
    assert_eq!(
        provider.collect(denied, request).await.unwrap_err().code(),
        "search_scope_denied"
    );
}

async fn check_tools(engine: Arc<SearchEngine>, mut run: RunCallContext) {
    use futures_util::StreamExt;
    run.cancellation = CancellationSignal::new();
    let tools = SearchToolset::try_new(engine).unwrap();
    let id = ToolCallId::from_bytes([3; 16]);
    let ctx = ToolCallContext {
        run,
        tool_batch_id: ToolBatchId::from_bytes([4; 16]),
        tool_call_id: id,
    };
    let call = ValidatedToolCall {
        call: ToolCallBlock::try_new(
            id,
            "search",
            RawJson::parse(r#"{"text":"retention","sources":["missing"]}"#).unwrap(),
        )
        .unwrap(),
        tool_id: ToolId::parse(SEARCH_TOOL_ID).unwrap(),
        component: None,
        output_contract: EffectOutputContract {
            kind: EffectOutputKind::ToolResult,
            schema_version: 1,
            schema_digest: Digest::blob_content(b"fixture"),
        },
        retry_safety: RetrySafety::SafeToRetry,
        deadline: None,
        execution: ToolExecutionMode::Parallel,
        failure_policy: ToolFailurePolicy::ReturnToModel,
    };
    let mut stream = tools.call(ctx.clone(), call.clone()).await.unwrap();
    let ToolStreamItem::Completed(result) = stream.next().await.unwrap().unwrap() else {
        panic!("completed result")
    };
    assert!(result.is_error);
    let value: serde_json::Value = serde_json::from_slice(result.output.as_bytes()).unwrap();
    assert_eq!(value["code"], "search_no_successful_sources");
    assert_eq!(value["outcomes"][0]["status"], "unavailable");
    let mut denied = ctx.clone();
    denied.run.locator.tenant_scope = "other".into();
    assert_eq!(
        tools.call(denied, call.clone()).await.err().unwrap().code(),
        "search_scope_denied"
    );
    ctx.run.cancellation.cancel();
    assert_eq!(
        tools.call(ctx, call).await.err().unwrap().code(),
        "search_cancelled"
    );
}
