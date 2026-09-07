//! Pure contract, scope, fusion, coverage, and resource-limit checks.

use std::sync::Arc;

use finstack_ai_kernel::Sensitivity;
use finstack_ai_search_core::*;

fn hit(source: &str, id: &str, score: u32) -> SearchHit {
    SearchHit {
        entity_label: None,
        source: source.into(),
        reference: SourceRef::Memory { id: id.into() },
        score,
        preview: id.into(),
        sensitivity: Sensitivity::Internal,
        provenance: SearchProvenance {
            scope: SearchScope::try_new("tenant").unwrap(),
            content_digest: configuration_digest("test-source", &id).unwrap(),
            locators: vec![],
            citations: vec![],
        },
    }
}

fn leg(source: &str, kind: LexicalKind, hits: Vec<SearchHit>) -> LegResult {
    LegResult {
        leg: HybridLeg {
            source: source.into(),
            strategy: SearchStrategy::Lexical(kind),
            weight_micros: 1_000_000,
        },
        result: SourceResult::completed(hits, 3),
    }
}

#[test]
fn complete_scope_is_never_broadened_by_mapping() {
    let tenant = SearchScope::try_new("tenant").unwrap();
    let mut bound = tenant.clone();
    bound.user = Some("alice".into());
    bound.workspace = Some("research".into());
    assert_eq!(
        ScopeMapping::Exact.resolve(&bound, &tenant),
        Err(SearchError::SearchScopeDenied)
    );
    assert_eq!(
        ScopeMapping::RestrictToBound
            .resolve(&bound, &tenant)
            .unwrap(),
        bound
    );
    let mut wrong = bound.clone();
    wrong.user = Some("bob".into());
    assert!(
        ScopeMapping::RestrictToBound
            .resolve(&bound, &wrong)
            .is_err()
    );
    assert!(
        ScopeMapping::RestrictToBound
            .resolve(&tenant, &bound)
            .is_err()
    );
    wrong = bound.clone();
    wrong.tenant = "other".into();
    assert!(
        ScopeMapping::RestrictToBound
            .resolve(&bound, &wrong)
            .is_err()
    );
    assert_eq!(ScopeMapping::Exact.resolve(&bound, &bound).unwrap(), bound);
    assert_ne!(tenant.digest().unwrap(), bound.digest().unwrap());
}

#[test]
fn rrf_is_hand_calculated_and_independent_of_leg_and_hit_order() {
    let first = leg(
        "memory",
        LexicalKind::Keyword,
        vec![hit("memory", "a", 9), hit("memory", "b", 1)],
    );
    let second = leg(
        "memory",
        LexicalKind::Bm25,
        vec![hit("memory", "a", 1), hit("memory", "b", 9)],
    );
    let expected = fuse(
        vec![first.clone(), second.clone()],
        Fusion::default(),
        10,
        &SearchLimits::default(),
    )
    .unwrap();
    assert_eq!(expected.hits.len(), 2);
    assert_eq!(
        expected.hits[0].reference,
        SourceRef::Memory { id: "a".into() }
    );
    assert_eq!(
        expected.hits[0].score,
        1_000_000_000 / 61 + 1_000_000_000 / 62
    );
    assert_eq!(expected.hits[0].score, expected.hits[1].score);
    assert_eq!(expected.hits[0].evidence.len(), 2);
    let mut shuffled = first;
    shuffled.result.hits.reverse();
    assert_eq!(
        fuse(
            vec![second, shuffled],
            Fusion::default(),
            10,
            &SearchLimits::default()
        )
        .unwrap(),
        expected
    );
}

#[test]
fn weighted_sum_normalizes_each_leg_before_applying_weights() {
    let first = leg(
        "memory",
        LexicalKind::Keyword,
        vec![hit("memory", "a", 1_000_000), hit("memory", "b", 500_000)],
    );
    let mut second = leg(
        "memory",
        LexicalKind::Bm25,
        vec![hit("memory", "b", 2), hit("memory", "a", 1)],
    );
    second.leg.weight_micros = 500_000;
    let result = fuse(
        vec![first, second],
        Fusion::WeightedSum,
        10,
        &SearchLimits::default(),
    )
    .unwrap();
    assert_eq!(result.hits[0].score, 1_000_000_000);
    assert_eq!(result.hits[1].score, 500_000_000);
    let singleton = fuse(
        vec![leg(
            "memory",
            LexicalKind::Keyword,
            vec![hit("memory", "a", 0)],
        )],
        Fusion::WeightedSum,
        1,
        &SearchLimits::default(),
    )
    .unwrap();
    assert_eq!(singleton.hits[0].score, 1_000_000_000);
}

#[test]
fn duplicate_evidence_survives_without_inflating_weight_or_crossing_namespaces() {
    let ordinary = hit("memory", "a", 4);
    let mut secret = ordinary.clone();
    secret.sensitivity = Sensitivity::Secret;
    secret.provenance.locators.push("second provenance".into());
    let first = leg("memory", LexicalKind::Keyword, vec![ordinary, secret]);
    let second = leg(
        "another",
        LexicalKind::Keyword,
        vec![hit("another", "a", 4)],
    );
    let result = fuse(
        vec![first.clone(), second.clone()],
        Fusion::default(),
        10,
        &SearchLimits::default(),
    )
    .unwrap();
    assert_eq!(result.hits.len(), 2);
    let memory = result
        .hits
        .iter()
        .find(|hit| hit.source.as_ref() == "memory")
        .unwrap();
    assert_eq!(memory.sensitivity, Sensitivity::Secret);
    assert_eq!(memory.evidence.len(), 2);
    assert_eq!(memory.score, 1_000_000_000 / 61);
    let mut reversed = first;
    reversed.result.hits.reverse();
    assert_eq!(
        fuse(
            vec![second, reversed],
            Fusion::default(),
            10,
            &SearchLimits::default()
        )
        .unwrap(),
        result
    );
}

#[test]
fn deduplication_cannot_merge_authority_across_scopes() {
    let a = hit("memory", "a", 1);
    let mut b = a.clone();
    b.provenance.scope.user = Some("bob".into());
    assert_eq!(
        fuse(
            vec![leg("memory", LexicalKind::Keyword, vec![a, b])],
            Fusion::default(),
            10,
            &SearchLimits::default()
        ),
        Err(SearchError::SearchScopeDenied)
    );
}

#[test]
fn zero_success_is_not_successful_empty_and_partial_coverage_is_preserved() {
    let mut missing = leg("missing", LexicalKind::Keyword, vec![]);
    missing.result.status = SourceStatus::Unavailable;
    missing.result.examined = 0;
    missing.result.historical_complete = false;
    missing.result.reasons = vec!["source_missing".into()];
    assert!(
        matches!(fuse(vec![missing.clone()], Fusion::default(), 1, &SearchLimits::default()), Err(SearchError::SearchNoSuccessfulSources { outcomes }) if outcomes.len() == 1)
    );
    let mut partial = leg("memory", LexicalKind::Keyword, vec![]);
    partial.result.truncate("history_pruned");
    partial.result.historical_complete = false;
    let result = fuse(
        vec![partial, missing],
        Fusion::default(),
        1,
        &SearchLimits::default(),
    )
    .unwrap();
    assert!(result.hits.is_empty());
    assert_eq!(result.outcomes.len(), 2);
    assert!(
        result.outcomes.iter().any(
            |outcome| outcome.status == SourceStatus::Truncated && !outcome.historical_complete
        )
    );
}

#[test]
fn plan_and_query_validation_precede_source_dispatch() {
    let limits = SearchLimits::default();
    let query = SearchQuery::lexical("[", LexicalKind::Keyword);
    let mut plan = HybridPlan {
        legs: vec![HybridLeg {
            source: "docs".into(),
            strategy: SearchStrategy::Lexical(LexicalKind::Regex),
            weight_micros: 1_000_000,
        }],
        fusion: Fusion::default(),
    };
    assert!(plan.validate(&query, &limits, 8).is_err());
    plan.legs[0].strategy = SearchStrategy::Lexical(LexicalKind::Keyword);
    assert!(plan.validate(&query, &limits, 8).is_ok());
    plan.legs.push(plan.legs[0].clone());
    assert!(plan.validate(&query, &limits, 8).is_err());
    assert!(query.validate(&limits, 257).is_err());
    let oversized = SearchQuery::lexical("x".repeat(4097), LexicalKind::Keyword);
    assert!(oversized.validate(&limits, 1).is_err());
    let huge_regex = SearchQuery::lexical("a{10000000}", LexicalKind::Regex);
    assert!(huge_regex.validate(&limits, 1).is_err());
}

#[test]
fn unicode_previews_and_source_payload_limits_are_enforced() {
    assert_eq!(preview("a🦀é東京z", 4).as_ref(), "a🦀é東");
    assert_eq!(preview("a\0b", 3).as_ref(), "a�b");
    let mut result = hit("memory", "a", 1);
    result.preview = Arc::from("🦀".repeat(513));
    assert!(result.validate(&SearchLimits::default()).is_err());
    result.preview = "ok".into();
    result.provenance.citations.push(SearchCitation {
        source: "docs".into(),
        reference: result.reference.clone(),
        scope: SearchScope::try_new("other").unwrap(),
        content_digest: result.provenance.content_digest,
    });
    assert_eq!(
        result.validate(&SearchLimits::default()),
        Err(SearchError::SearchScopeDenied)
    );
}
