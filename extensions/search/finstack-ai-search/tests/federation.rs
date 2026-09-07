//! Fan-out preflight, availability, deadlines, and cancellation conformance.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_search::*;

#[derive(Clone)]
struct Source {
    id: &'static str,
    calls: Arc<AtomicUsize>,
    dropped: Arc<AtomicUsize>,
    hangs: bool,
    fails: bool,
}

impl Source {
    fn new(id: &'static str) -> Self {
        Self {
            id,
            calls: Arc::new(AtomicUsize::new(0)),
            dropped: Arc::new(AtomicUsize::new(0)),
            hangs: false,
            fails: false,
        }
    }
}

impl SearchSource for Source {
    fn descriptor(&self) -> SearchSourceDescriptor {
        SearchSourceDescriptor {
            source_id: self.id.into(),
            kind: "fixture".into(),
            lexical: vec![LexicalKind::Keyword, LexicalKind::Regex],
            semantic_spaces: vec![],
            evidence_lookup: false,
            graph: false,
            durable: false,
            scope_mapping: ScopeMapping::Exact,
            limits: SearchLimits::default(),
            configuration_digest: configuration_digest("fixture", &self.id).unwrap(),
        }
    }
    fn authorize(&self, scope: &SearchScope, query: &SearchQuery) -> Result<(), SearchError> {
        if scope.tenant.as_ref() != "tenant" || !query.journal.sessions.is_empty() {
            return Err(SearchError::SearchScopeDenied);
        }
        Ok(())
    }
    fn search(
        &self,
        scope: SearchScope,
        query: SearchQuery,
        _limit: usize,
    ) -> PortFuture<Result<SourceResult, SearchError>> {
        let owned = self.clone();
        Box::pin(async move {
            owned.authorize(&scope, &query)?;
            owned.calls.fetch_add(1, Ordering::SeqCst);
            let _guard = Dropped(owned.dropped);
            if owned.hangs {
                std::future::pending::<()>().await;
            }
            if owned.fails {
                return Err(SearchError::SearchUnavailable);
            }
            Ok(SourceResult::completed(vec![], 0))
        })
    }
}

struct Dropped(Arc<AtomicUsize>);
impl Drop for Dropped {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn engine(sources: Vec<Arc<dyn SearchSource>>) -> SearchEngine {
    SearchEngine::try_new(
        SearchConfig {
            graph_expansion: None,
            scope: SearchScope::try_new("tenant").unwrap(),
            default_plan: HybridPlan {
                legs: sources
                    .iter()
                    .map(|s| HybridLeg {
                        source: s.descriptor().source_id,
                        strategy: SearchStrategy::Lexical(LexicalKind::Keyword),
                        weight_micros: 1_000_000,
                    })
                    .collect(),
                fusion: Fusion::default(),
            },
            limits: SearchLimits::default(),
            max_concurrency: 2,
            source_timeout_ms: 10,
        },
        sources,
    )
    .unwrap()
}

#[tokio::test]
async fn validation_and_authorization_finish_before_any_fanout() {
    let source = Arc::new(Source::new("a"));
    let other = Arc::new(Source::new("b"));
    let engine = engine(vec![source.clone(), other.clone()]);
    let mut request = SearchRequest::text("[");
    request.strategy = Some(SearchStrategy::Lexical(LexicalKind::Regex));
    assert!(matches!(
        engine.search(request).await,
        Err(SearchError::SearchInvalid { .. })
    ));
    let mut denied = SearchRequest::text("anything");
    denied.journal.sessions = vec![finstack_ai_kernel::SessionId::from_bytes([1; 16])];
    assert_eq!(
        engine.search(denied).await,
        Err(SearchError::SearchScopeDenied)
    );
    assert_eq!(source.calls.load(Ordering::SeqCst), 0);
    assert_eq!(other.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test(start_paused = true)]
async fn failed_unsupported_missing_and_timed_out_legs_remain_visible() {
    let ready = Arc::new(Source::new("a"));
    let mut hanging = Source::new("b");
    hanging.hangs = true;
    let hanging = Arc::new(hanging);
    let engine = engine(vec![ready, hanging.clone()]);
    let response = engine.search(SearchRequest::text("x")).await.unwrap();
    assert_eq!(response.outcomes[0].status, SourceStatus::Completed);
    assert_eq!(response.outcomes[1].status, SourceStatus::Unavailable);
    assert_eq!(
        response.outcomes[1].reasons,
        vec![Arc::<str>::from("source_timeout")]
    );
    assert_eq!(hanging.dropped.load(Ordering::SeqCst), 1);
    let mut unsupported = SearchRequest::text("x");
    unsupported.strategy = Some(SearchStrategy::Semantic {
        space: "unconfigured".into(),
    });
    assert!(
        matches!(engine.search(unsupported).await, Err(SearchError::SearchNoSuccessfulSources { outcomes }) if outcomes.iter().all(|o| o.status == SourceStatus::Unsupported))
    );
    let mut missing = SearchRequest::text("x");
    missing.sources = Some(vec!["absent".into()]);
    assert!(
        matches!(engine.search(missing).await, Err(SearchError::SearchNoSuccessfulSources { outcomes }) if outcomes[0].reasons[0].as_ref() == "source_missing")
    );
}

#[tokio::test(start_paused = true)]
async fn dropping_federation_future_drops_awaited_source_calls() {
    let mut source = Source::new("a");
    source.hangs = true;
    let source = Arc::new(source);
    let engine = engine(vec![source.clone()]);
    let mut query = Box::pin(engine.search(SearchRequest::text("x")));
    tokio::select! { biased; _ = &mut query => panic!("query must remain pending"), () = tokio::task::yield_now() => {} }
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
    drop(query);
    assert_eq!(source.dropped.load(Ordering::SeqCst), 1);
}
