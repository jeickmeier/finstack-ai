//! Recall provider bounds: long user turns are cut, rejected searches degrade.

use std::sync::{Arc, Mutex};

use finstack_ai_kernel::{
    ContentBlock, Digest, EffectId, LaneId, Metadata, OperationLocator, PrincipalRef, RunId,
    Sensitivity, SessionId, TextBlock,
};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_runtime::ports::context::{
    ContextBudget, ContextCallContext, ContextContribution, ContextItemKind, ContextOverflowPolicy,
    ContextProvider, ContextRequest,
};
use finstack_ai_runtime::ports::model::{AuthorizationContext, CancellationSignal, RunCallContext};
use finstack_ai_search::*;

/// A source that records every query it is asked to run and, like the
/// document index, refuses keyword queries with more terms than it supports.
#[derive(Clone)]
struct Source {
    id: &'static str,
    max_terms: usize,
    queries: Arc<Mutex<Vec<Arc<str>>>>,
}

impl Source {
    fn new(id: &'static str, max_terms: usize) -> Arc<Self> {
        Arc::new(Self {
            id,
            max_terms,
            queries: Arc::new(Mutex::new(Vec::new())),
        })
    }

    fn hit(&self) -> SearchHit {
        SearchHit {
            entity_label: None,
            source: self.id.into(),
            reference: SourceRef::Memory { id: "m1".into() },
            score: 1,
            preview: "retention policy".into(),
            sensitivity: Sensitivity::Internal,
            provenance: SearchProvenance {
                scope: scope(),
                content_digest: configuration_digest("fixture", &"m1").unwrap(),
                locators: vec![],
                citations: vec![],
            },
        }
    }
}

impl SearchSource for Source {
    fn descriptor(&self) -> SearchSourceDescriptor {
        SearchSourceDescriptor {
            source_id: self.id.into(),
            kind: "fixture".into(),
            lexical: vec![LexicalKind::Keyword],
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
        if scope.tenant.as_ref() != "tenant" {
            return Err(SearchError::SearchScopeDenied);
        }
        if lexical_terms(&query.text)?.len() > self.max_terms {
            return Err(SearchError::invalid("fixture_lexical_terms"));
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
            owned.queries.lock().unwrap().push(query.text.clone());
            Ok(SourceResult::completed(vec![owned.hit()], 1))
        })
    }
}

fn scope() -> SearchScope {
    SearchScope::try_new("tenant").unwrap()
}

fn recall(sources: Vec<Arc<dyn SearchSource>>) -> SearchContextProvider {
    let engine = SearchEngine::try_new(
        SearchConfig {
            graph_expansion: None,
            scope: scope(),
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
            source_timeout_ms: 1000,
        },
        sources,
    )
    .unwrap();
    SearchContextProvider::try_new(Arc::new(engine), 8).unwrap()
}

fn context() -> ContextCallContext {
    ContextCallContext {
        run: RunCallContext {
            relation_depth: 0,
            locator: OperationLocator::try_new(
                "tenant",
                SessionId::from_bytes([1; 16]),
                LaneId::from_bytes([2; 16]),
                RunId::from_bytes([3; 16]),
            )
            .unwrap(),
            authorization: AuthorizationContext {
                principal: PrincipalRef::try_new("issuer", "subject", Some("tenant")).unwrap(),
                authentication_method: Arc::from("test"),
                assurance_level: Arc::from("test"),
                roles: Arc::from([]),
                permitted_scopes: Arc::from([Arc::from("tenant")]),
                safe_claims: Metadata::empty(),
                policy_version: Arc::from("policy-v1"),
                decision_id: Arc::from("decision-v1"),
            },
            effect_id: EffectId::from_bytes([4; 16]),
            attempt: 1,
            deadline: None,
            budget_scope_id: None,
            cancellation: CancellationSignal::new(),
        },
        provider_index: 0,
        chain_digest: Digest::raw_json(b"chain"),
    }
}

fn request(blocks: &[&str]) -> ContextRequest {
    ContextRequest {
        session_id: SessionId::from_bytes([1; 16]),
        lane_id: LaneId::from_bytes([2; 16]),
        run_id: RunId::from_bytes([3; 16]),
        user_input: blocks
            .iter()
            .map(|text| ContentBlock::Text(TextBlock::try_new(*text).unwrap()))
            .collect::<Vec<_>>()
            .into(),
        recent_history: Arc::from([]),
        budget: ContextBudget {
            max_items: 16,
            max_tokens: 100_000,
            max_bytes: 1024 * 1024,
            overflow: ContextOverflowPolicy::Reject,
        },
        active_capabilities: Arc::from([]),
    }
}

fn coverage_of(contribution: &ContextContribution) -> serde_json::Value {
    let first = contribution.items.first().unwrap();
    assert_eq!(first.kind, ContextItemKind::Metadata);
    let ContentBlock::Text(text) = first.content.first().unwrap() else {
        panic!("coverage item is text");
    };
    serde_json::from_str(text.text()).unwrap()
}

#[tokio::test]
async fn long_user_turn_is_truncated_not_rejected() {
    let source = Source::new("docs", MAX_LEXICAL_TERMS);
    let provider = recall(vec![source.clone()]);
    // A hyphenated paragraph tokenizes to far more than 64 terms.
    let paragraph = (0..200)
        .map(|i| format!("well-known term{i}"))
        .collect::<Vec<_>>()
        .join(", ");
    assert!(paragraph.len() < SearchLimits::default().max_query_bytes);
    assert!(lexical_terms(&paragraph).is_err());
    let contribution = provider
        .collect(context(), request(&[&paragraph]))
        .await
        .unwrap();
    assert_eq!(contribution.items.len(), 2);
    assert!(coverage_of(&contribution).get("search_error").is_none());
    let asked = source.queries.lock().unwrap().clone();
    assert_eq!(asked.len(), 1);
    assert_eq!(lexical_terms(&asked[0]).unwrap().len(), MAX_LEXICAL_TERMS);
    assert!(paragraph.starts_with(asked[0].as_ref()));

    // Multi-byte text beyond the byte ceiling is cut on a character boundary
    // across blocks, never rejected.
    let long = "é".repeat(3000);
    let contribution = provider
        .collect(context(), request(&[&long, &long, "tail"]))
        .await
        .unwrap();
    assert_eq!(contribution.items.len(), 2);
    let asked = source.queries.lock().unwrap().clone();
    assert_eq!(asked.len(), 2);
    assert!(asked[1].len() <= SearchLimits::default().max_query_bytes);
    assert!(asked[1].chars().all(|c| c == 'é' || c == ' '));
    assert!(asked[1].len() > SearchLimits::default().max_query_bytes - 4);

    // Punctuation-only input has nothing to recall and is not an error.
    let empty = provider
        .collect(context(), request(&["--- ... !!!"]))
        .await
        .unwrap();
    assert!(empty.items.is_empty());
}

#[tokio::test]
async fn oversized_query_degrades_to_empty_contribution() {
    let strict = Source::new("strict", 4);
    let provider = recall(vec![strict.clone()]);
    let contribution = provider
        .collect(
            context(),
            request(&["one two three four five six seven eight"]),
        )
        .await
        .unwrap();
    assert_eq!(contribution.items.len(), 1);
    let coverage = coverage_of(&contribution);
    assert_eq!(coverage["search_error"], "search_no_successful_sources");
    assert_eq!(coverage["search_coverage"][0]["status"], "unavailable");
    assert_eq!(
        coverage["search_coverage"][0]["reasons"][0],
        "search_invalid"
    );
    assert!(strict.queries.lock().unwrap().is_empty());

    // Alongside a lenient source, the rejected leg stays visible and the
    // lenient leg still contributes.
    let lenient = Source::new("lenient", MAX_LEXICAL_TERMS);
    let provider = recall(vec![strict.clone(), lenient.clone()]);
    let contribution = provider
        .collect(
            context(),
            request(&["one two three four five six seven eight"]),
        )
        .await
        .unwrap();
    assert_eq!(contribution.items.len(), 2);
    let coverage = coverage_of(&contribution);
    assert!(coverage.get("search_error").is_none());
    let outcomes = coverage["search_coverage"].as_array().unwrap();
    assert_eq!(outcomes.len(), 2);
    assert!(
        outcomes
            .iter()
            .any(|o| o["source"] == "strict" && o["status"] == "unavailable")
    );
    assert!(
        outcomes
            .iter()
            .any(|o| o["source"] == "lenient" && o["status"] == "completed")
    );
    assert_eq!(lenient.queries.lock().unwrap().len(), 1);

    // Scope denial from the host context still fails closed.
    let mut denied = context();
    denied.run.locator.tenant_scope = "other".into();
    assert_eq!(
        provider
            .collect(denied, request(&["anything"]))
            .await
            .unwrap_err()
            .code(),
        "search_scope_denied"
    );
}
