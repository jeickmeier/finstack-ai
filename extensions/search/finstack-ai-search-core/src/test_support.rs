//! Shared source conformance checks, opt-in for backend integration tests.

use crate::{LexicalKind, SearchError, SearchQuery, SearchScope, SearchSource, SearchStrategy};

/// Run the same scope, unsupported-strategy, deterministic-result, provenance,
/// and bound checks against any real source. The supplied source is prepopulated
/// and query is a supported non-empty example for that implementation.
///
/// # Errors
/// Returns a stable conformance failure (or the underlying search error) when
/// a source violates its declaration. This helper never panics or mutates data.
pub async fn check_source_contract(
    source: &dyn SearchSource,
    scope: SearchScope,
    query: SearchQuery,
) -> Result<(), SearchError> {
    let descriptor = source.descriptor();
    source.authorize(&scope, &query)?;
    if !descriptor.supports(&query.strategy) {
        return Err(SearchError::invalid("conformance_supported_query"));
    }
    let limit = 8.min(descriptor.limits.max_results);
    let first = source.search(scope.clone(), query.clone(), limit).await?;
    let second = source.search(scope.clone(), query.clone(), limit).await?;
    if first != second || !first.status.successful() || first.hits.len() > limit {
        return Err(SearchError::invalid("conformance_determinism"));
    }
    for hit in &first.hits {
        hit.validate(&descriptor.limits)?;
        if hit.source != descriptor.source_id {
            return Err(SearchError::invalid("conformance_namespace"));
        }
        source.authorize(&hit.provenance.scope, &query)?;
        if descriptor.evidence_lookup {
            let evidence = source
                .read_evidence(scope.clone(), hit.reference.clone())
                .await?
                .ok_or_else(|| SearchError::invalid("conformance_missing_evidence"))?;
            evidence.validate(&descriptor.limits)?;
            if evidence.hit.reference != hit.reference
                || evidence.hit.source != hit.source
                || evidence.hit.provenance != hit.provenance
                || evidence.hit.sensitivity != hit.sensitivity
            {
                return Err(SearchError::invalid("conformance_evidence_identity"));
            }
        }
    }
    let mut denied = scope.clone();
    denied.tenant = "conformance.other.tenant".into();
    if denied.tenant == scope.tenant {
        denied.tenant = "conformance.third.tenant".into();
    }
    if source.authorize(&denied, &query) != Err(SearchError::SearchScopeDenied)
        || source.search(denied, query.clone(), limit).await != Err(SearchError::SearchScopeDenied)
    {
        return Err(SearchError::invalid("conformance_tenant_denial"));
    }
    let mut invalid = query.clone();
    invalid.text = "".into();
    if source.search(scope.clone(), invalid, limit).await.is_ok()
        || source
            .search(
                scope.clone(),
                query.clone(),
                descriptor.limits.max_results + 1,
            )
            .await
            .is_ok()
    {
        return Err(SearchError::invalid("conformance_query_bounds"));
    }
    for kind in [
        LexicalKind::Keyword,
        LexicalKind::Bm25,
        LexicalKind::Literal,
        LexicalKind::Regex,
    ] {
        if !descriptor.lexical.contains(&kind) {
            let mut unsupported = query.clone();
            unsupported.text = "conformance".into();
            unsupported.strategy = SearchStrategy::Lexical(kind);
            if source.search(scope.clone(), unsupported, limit).await
                != Err(SearchError::SearchUnsupported)
            {
                return Err(SearchError::invalid("conformance_strategy_honesty"));
            }
        }
    }
    Ok(())
}
