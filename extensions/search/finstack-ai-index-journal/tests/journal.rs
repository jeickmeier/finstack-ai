//! Real committed SDK runs, observer hints, backfill and pruned reconstruction.
use finstack_ai::{Agent, AgentRunRequest, Session};
use finstack_ai_index_journal::*;
use finstack_ai_kernel::*;
use finstack_ai_runtime::ports::{PortFuture, journal::*, model::*};
use finstack_ai_search::*;
use finstack_ai_store_memory::{MemoryJournalStore, MemoryStoreLimits};
use finstack_ai_store_sqlite::{
    SqliteDurability, SqliteJournalStore, SqliteStoreConfig, SqliteStoreLimits,
};
use finstack_ai_test::{ScriptedModel, ScriptedModelAction, ScriptedModelPlan};
use std::sync::Arc;

fn profile() -> ModelContextProfile {
    ModelContextProfile {
        provider: "scripted".into(),
        model: ModelName::try_new("journal-test").unwrap(),
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
fn response(text: &str, call: bool) -> ScriptedModelPlan {
    let mut actions = vec![];
    let mut calls = vec![];
    let mut content = vec![];
    if call {
        let args = r#"{"text":"retention","strategy":null,"sources":null,"limit":8}"#;
        calls.push(ModelToolCall {
            name: "search".into(),
            arguments: RawJson::parse(args).unwrap(),
            provider_call_id: None,
        });
        actions.push(ScriptedModelAction::Emit(Ok(
            ModelStreamItem::ToolCallDelta(ToolCallDelta {
                index: 0,
                name: Some("search".into()),
                arguments_delta: args.into(),
                provider_call_id: None,
            }),
        )));
    } else {
        actions.push(ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(
            TextDelta { text: text.into() },
        ))));
        content.push(ContentBlock::Text(TextBlock::try_new(text).unwrap()));
    }
    actions.push(ScriptedModelAction::Emit(Ok(ModelStreamItem::Completed(
        ModelResponse {
            assistant_content: content.into(),
            tool_calls: calls.into(),
            usage: Usage::try_new(
                Some(10),
                Some(10),
                Some(20),
                None,
                std::collections::BTreeMap::default(),
            )
            .unwrap(),
            provider_ids: ProviderIds::empty(),
            completion_id: format!("completed-{call}-{text}").into(),
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
fn request(text: &str, tenant: &str) -> AgentRunRequest {
    AgentRunRequest::try_new(
        ModelName::try_new("journal-test").unwrap(),
        text,
        RunSecurityContext::try_new(
            tenant,
            PrincipalRef::try_new("fixture", "developer", Some(tenant)).unwrap(),
            "local",
            "fixture",
            "v1",
            "v1",
            None,
        )
        .unwrap(),
    )
    .unwrap()
}
fn config(session: SessionId) -> JournalIndexConfig {
    JournalIndexConfig::new(
        "journal",
        SearchScope::try_new("tenant").unwrap(),
        vec![session],
    )
}
fn memory() -> Arc<dyn JournalStore> {
    Arc::new(
        MemoryJournalStore::try_new(MemoryStoreLimits {
            sessions: 16,
            batches_per_session: 512,
            records_per_session: 8192,
            snapshot_bytes: 1_048_576,
        })
        .unwrap(),
    )
}
fn disk(path: &std::path::Path) -> Arc<dyn JournalStore> {
    Arc::new(
        SqliteJournalStore::try_open(SqliteStoreConfig::new(
            path,
            SqliteDurability::Durable,
            SqliteStoreLimits {
                sessions: 16,
                batches_per_session: 512,
                records_per_session: 8192,
                snapshot_bytes: 1_048_576,
            },
        ))
        .unwrap(),
    )
}
async fn agent(
    store: Arc<dyn JournalStore>,
    source: Arc<JournalSearchSource>,
    plans: Vec<ScriptedModelPlan>,
) -> (Agent, Arc<JournalIndexObserver>) {
    let observer = Arc::new(JournalIndexObserver::try_new(source.clone()).unwrap());
    let engine = Arc::new(
        SearchEngine::try_new(
            SearchConfig {
                graph_expansion: None,
                scope: source.config().scope.clone(),
                default_plan: HybridPlan {
                    legs: vec![HybridLeg {
                        source: "journal".into(),
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
    );
    let model = Arc::new(ScriptedModel::from_plans(profile(), plans));
    let agent = Agent::builder(
        AgentId::parse("test.journal").unwrap(),
        BundleId::parse("test.bundle").unwrap(),
        (component("test.model"), model),
        (component("test.journal.store"), store),
    )
    .observer(observer.clone())
    .toolset(
        component("test.search"),
        Arc::new(SearchToolset::try_new(engine).unwrap()),
    )
    .build()
    .await
    .unwrap();
    (agent, observer)
}
async fn query(source: &JournalSearchSource, text: &str) -> SourceResult {
    source
        .search(
            source.config().scope.clone(),
            SearchQuery::lexical(text, LexicalKind::Literal),
            8,
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn live_hints_backfill_and_reopening_agree_on_committed_entries() {
    let dir = tempfile::tempdir().unwrap();
    let store = memory();
    let session = Session::create(store.clone(), "tenant").await.unwrap();
    let path = dir.path().join("index.sqlite");
    let source = Arc::new(
        JournalSearchSource::try_open(&path, store.clone(), config(session.session_id())).unwrap(),
    );
    let (agent, observer) = agent(
        store.clone(),
        source.clone(),
        vec![
            response("", true),
            response("Acme retention seven years", false),
        ],
    )
    .await;
    assert_eq!(
        query(&source, "retention").await.status,
        SourceStatus::Unavailable
    );
    let run = session
        .lane("main")
        .await
        .unwrap()
        .run(&agent, request("retention question", "tenant"))
        .unwrap();
    let output = run.result().await.unwrap();
    assert_eq!(output.text(), "Acme retention seven years");
    // Observation enqueues identifiers only: no searchable entries until drain.
    assert!(query(&source, "Acme").await.hits.is_empty());
    let reports = observer.drain(8, 512).await.unwrap();
    assert_eq!(reports.len(), 1);
    assert!(reports[0].complete);
    assert!(observer.drain(8, 512).await.unwrap().is_empty());
    let live = query(&source, "retention").await;
    assert_eq!(live.status, SourceStatus::Completed);
    assert!(live.hits.len() >= 3);
    assert_eq!(
        query(&source, "search_no_successful_sources")
            .await
            .hits
            .len(),
        1
    );
    let loaded = store
        .load(LoadRequest {
            session_id: session.session_id(),
        })
        .await
        .unwrap();
    let ids: Vec<_> = loaded
        .committed_batches
        .iter()
        .flat_map(|b| b.records.iter().map(RecordEnvelope::record_id))
        .collect();
    for hit in &live.hits {
        let SourceRef::JournalSpan {
            first_record: Some(first),
            last_record: Some(last),
            ..
        } = hit.reference
        else {
            panic!("retained record citation")
        };
        assert!(ids.contains(&first) && ids.contains(&last));
    }
    let rebuilt = JournalSearchSource::try_open(
        &dir.path().join("rebuilt.sqlite"),
        store.clone(),
        config(session.session_id()),
    )
    .unwrap();
    while !rebuilt
        .sync_session(session.session_id(), 3)
        .await
        .unwrap()
        .complete
    {}
    assert_eq!(query(&rebuilt, "retention").await, live);
    let reopened =
        JournalSearchSource::try_open(&path, store, config(session.session_id())).unwrap();
    assert_eq!(query(&reopened, "retention").await, live);
    finstack_ai_search_core::test_support::check_source_contract(
        &rebuilt,
        config(session.session_id()).scope,
        SearchQuery::lexical("retention", LexicalKind::Keyword),
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn failed_stream_tokens_never_become_searchable_journal_entries() {
    let dir = tempfile::tempdir().unwrap();
    let store = memory();
    let session = Session::create(store.clone(), "tenant").await.unwrap();
    let source = Arc::new(
        JournalSearchSource::try_open(
            &dir.path().join("index.sqlite"),
            store.clone(),
            config(session.session_id()),
        )
        .unwrap(),
    );
    let failure = ModelError::try_new(
        "fixture_failed",
        ErrorCategory::Validation,
        false,
        "fixture failure",
        Metadata::empty(),
    )
    .unwrap();
    let plan = ScriptedModelPlan {
        actions: vec![
            ScriptedModelAction::Emit(Ok(ModelStreamItem::TextDelta(TextDelta {
                text: "ephemeral-token-only".into(),
            }))),
            ScriptedModelAction::Emit(Err(failure)),
        ],
    };
    let (agent, observer) = agent(store, source.clone(), vec![plan]).await;
    let run = session
        .lane("main")
        .await
        .unwrap()
        .run(&agent, request("start", "tenant"))
        .unwrap();
    assert!(run.result().await.is_err());
    observer.drain(8, 512).await.unwrap();
    assert!(query(&source, "ephemeral-token-only").await.hits.is_empty());
    assert_eq!(query(&source, "start").await.hits.len(), 1);
}

#[tokio::test]
async fn pruned_journal_rebuild_uses_verified_snapshot_and_reports_lost_history() {
    let dir = tempfile::tempdir().unwrap();
    let store = disk(&dir.path().join("journal.sqlite"));
    let session = Session::create(store.clone(), "tenant").await.unwrap();
    let source = Arc::new(
        JournalSearchSource::try_open(
            &dir.path().join("index.sqlite"),
            store.clone(),
            config(session.session_id()),
        )
        .unwrap(),
    );
    let (agent, _) = agent(
        store.clone(),
        source.clone(),
        vec![response("retention final", false)],
    )
    .await;
    let output = session
        .lane("main")
        .await
        .unwrap()
        .run(&agent, request("old-user-message", "tenant"))
        .unwrap()
        .result()
        .await
        .unwrap();
    source
        .sync_session(session.session_id(), 512)
        .await
        .unwrap();
    assert_eq!(query(&source, "old-user-message").await.hits.len(), 1);
    let coordinator = finstack_ai_runtime::commit::CommitCoordinator::recover_run(
        store.clone(),
        session.session_id(),
        Some(output.locator.run_id),
    )
    .await
    .unwrap();
    store
        .write_state_snapshot(StateSnapshotRequest {
            session_id: session.session_id(),
            state: coordinator.state().clone(),
            head_checksum: coordinator.head_checksum().unwrap(),
            pending_timer_scheduled_at: None,
            last_model_continuation: None,
        })
        .await
        .unwrap();
    store
        .prune(PruneRequest {
            session_id: session.session_id(),
            horizon: IdempotencyHorizon {
                expire_at: Timestamp::from_unix_ms(4_000_000_000_000).unwrap(),
            },
        })
        .await
        .unwrap();
    assert_eq!(
        query(&source, "retention").await.status,
        SourceStatus::Unavailable
    );
    let report = source
        .sync_session(session.session_id(), 512)
        .await
        .unwrap();
    assert!(report.used_load);
    assert!(!report.historical_complete);
    let results = query(&source, "retention").await;
    assert_eq!(results.hits.len(), 1);
    assert_eq!(results.status, SourceStatus::Truncated);
    assert!(!results.historical_complete);
    assert!(matches!(
        results.hits[0].reference,
        SourceRef::JournalSpan {
            first_record: None,
            last_record: None,
            ..
        }
    ));
    assert!(query(&source, "old-user-message").await.hits.is_empty());
    let rebuilt = JournalSearchSource::try_open(
        &dir.path().join("fresh.sqlite"),
        store,
        config(session.session_id()),
    )
    .unwrap();
    rebuilt
        .sync_session(session.session_id(), 512)
        .await
        .unwrap();
    assert_eq!(query(&rebuilt, "retention").await, results);
}

struct NoScan(Arc<dyn JournalStore>);
impl JournalStore for NoScan {
    fn descriptor(&self) -> JournalStoreDescriptor {
        self.0.descriptor()
    }
    fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        self.0.append(request)
    }
    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        self.0.load(request)
    }
    fn write_snapshot(
        &self,
        request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        self.0.write_snapshot(request)
    }
    fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
        self.0.health()
    }
}
#[tokio::test]
async fn scan_unsupported_falls_back_and_filters_cannot_grant_sessions() {
    let dir = tempfile::tempdir().unwrap();
    let store = memory();
    let session = Session::create(store.clone(), "tenant").await.unwrap();
    session
        .lane("main")
        .await
        .unwrap()
        .append_text("retention standalone")
        .await
        .unwrap();
    let source = JournalSearchSource::try_open(
        &dir.path().join("index.sqlite"),
        Arc::new(NoScan(store)),
        config(session.session_id()),
    )
    .unwrap();
    let report = source
        .sync_session(session.session_id(), 512)
        .await
        .unwrap();
    assert!(report.used_load && report.historical_complete);
    assert_eq!(query(&source, "retention").await.hits.len(), 1);
    let mut denied = SearchQuery::lexical("retention", LexicalKind::Literal);
    denied.journal.sessions = vec![SessionId::from_bytes([9; 16])];
    assert_eq!(
        source.authorize(&source.config().scope, &denied),
        Err(SearchError::SearchScopeDenied)
    );
    let mut lane = SearchQuery::lexical("retention", LexicalKind::Literal);
    lane.journal.lanes = vec![LaneId::from_bytes([8; 16])];
    assert!(
        source
            .search(source.config().scope.clone(), lane, 8)
            .await
            .unwrap()
            .hits
            .is_empty()
    );
}

struct BadLoad(Arc<dyn JournalStore>);
impl JournalStore for BadLoad {
    fn descriptor(&self) -> JournalStoreDescriptor {
        self.0.descriptor()
    }
    fn append(&self, request: AppendRequest) -> PortFuture<Result<CommittedBatch, StoreError>> {
        self.0.append(request)
    }
    fn load(&self, request: LoadRequest) -> PortFuture<Result<LoadedSession, StoreError>> {
        let inner = self.0.clone();
        Box::pin(async move {
            let mut loaded = inner.load(request).await?;
            loaded.head_checksum =
                Some(Digest::domain_separated("invalid-head", 1, b"invalid").unwrap());
            Ok(loaded)
        })
    }
    fn write_snapshot(
        &self,
        request: SnapshotRequest,
    ) -> PortFuture<Result<SnapshotReceipt, StoreError>> {
        self.0.write_snapshot(request)
    }
    fn health(&self) -> PortFuture<Result<StoreHealth, StoreError>> {
        self.0.health()
    }
}
#[tokio::test]
async fn full_load_rejects_corrupt_heads_and_capacity_without_indexing_partial_data() {
    let store = memory();
    let session = Session::create(store.clone(), "tenant").await.unwrap();
    session
        .lane("main")
        .await
        .unwrap()
        .append_text("bounded retention")
        .await
        .unwrap();
    let source = JournalSearchSource::try_open(
        std::path::Path::new(":memory:"),
        Arc::new(BadLoad(store.clone())),
        config(session.session_id()),
    )
    .unwrap();
    assert!(matches!(
        source.sync_session(session.session_id(), 128).await,
        Err(SearchError::SearchInvalid { .. })
    ));
    assert_eq!(
        query(&source, "retention").await.status,
        SourceStatus::Unavailable
    );
    let source = JournalSearchSource::try_open(
        std::path::Path::new(":memory:"),
        Arc::new(NoScan(store)),
        config(session.session_id()),
    )
    .unwrap();
    assert!(matches!(
        source.sync_session(session.session_id(), 1).await,
        Err(SearchError::SearchCapacityExceeded { .. })
    ));
    assert_eq!(
        query(&source, "retention").await.status,
        SourceStatus::Unavailable
    );
    source
        .sync_session(session.session_id(), 128)
        .await
        .unwrap();
    assert_eq!(query(&source, "retention").await.hits.len(), 1);
}

#[tokio::test]
async fn committed_tenant_authority_and_time_filters_are_enforced() {
    let store = memory();
    let session = Session::create(store.clone(), "tenant").await.unwrap();
    let source = Arc::new(
        JournalSearchSource::try_open(
            std::path::Path::new(":memory:"),
            store.clone(),
            config(session.session_id()),
        )
        .unwrap(),
    );
    let (agent, _) = agent(
        store.clone(),
        source.clone(),
        vec![response("retention policy", false)],
    )
    .await;
    session
        .lane("main")
        .await
        .unwrap()
        .run(&agent, request("time-filtered input", "tenant"))
        .unwrap()
        .result()
        .await
        .unwrap();
    let mut wrong = config(session.session_id());
    wrong.scope = SearchScope::try_new("foreign").unwrap();
    let foreign =
        JournalSearchSource::try_open(std::path::Path::new(":memory:"), store, wrong).unwrap();
    assert_eq!(
        foreign.sync_session(session.session_id(), 512).await,
        Err(SearchError::SearchScopeDenied)
    );
    source
        .sync_session(session.session_id(), 512)
        .await
        .unwrap();
    let query = SearchQuery {
        journal: JournalFilter {
            before: Some(UNIX_EPOCH),
            ..JournalFilter::default()
        },
        ..SearchQuery::lexical("retention", LexicalKind::Regex)
    };
    assert!(
        source
            .search(source.config().scope.clone(), query, 8)
            .await
            .unwrap()
            .hits
            .is_empty()
    );
}

#[test]
fn journal_index_rejects_foreign_and_future_database_schemas() {
    let dir = tempfile::tempdir().unwrap();
    let future = dir.path().join("future.sqlite");
    let c = rusqlite::Connection::open(&future).unwrap();
    c.pragma_update(None, "user_version", 77).unwrap();
    drop(c);
    assert!(
        JournalSearchSource::try_open(&future, memory(), config(SessionId::from_bytes([1; 16])))
            .is_err()
    );
    let foreign = dir.path().join("foreign.sqlite");
    let c = rusqlite::Connection::open(&foreign).unwrap();
    c.execute("CREATE TABLE foreign_data(id INTEGER)", [])
        .unwrap();
    drop(c);
    assert!(
        JournalSearchSource::try_open(&foreign, memory(), config(SessionId::from_bytes([1; 16])))
            .is_err()
    );
}
