//! Scoped [`SearchSource`] adapter over the existing memory store. Available
//! with `search`; standalone memory recall remains unchanged. A global-search
//! composition registers global recall instead of also registering memory recall.

use std::sync::Arc;

use finstack_ai_embeddings::{embedder::TextEmbedder, vector::truncate_to_bytes};
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_search_core::{
    LexicalKind, ScopeMapping, SearchError, SearchHit, SearchLimits, SearchProvenance, SearchQuery,
    SearchScope, SearchSource, SearchSourceDescriptor, SearchStrategy, SourceRef, SourceResult,
    configuration_digest, preview, validate_id,
};

use crate::record::MemoryScope;
use crate::store::{
    MemoryHit, MemoryPage, MemoryQuery, MemoryStore, MemoryStoreError, embedding_source_digest,
    embedding_source_text,
};

/// Memory adapter bound to one complete scope. Default exact mapping requires
/// all dimensions to match; `RestrictToBound` can only fill already-bound
/// restrictions. Both query the exact complete [`MemoryScope`].
///
/// Keyword maps to memory keywords; BM25 maps to the store's existing full-text
/// ordering. Neither pretends memory scores are comparable to document scores.
/// Literal/regex/graph and non-empty journal filters are unsupported.
#[derive(Clone)]
pub struct MemorySearchSource {
    descriptor: SearchSourceDescriptor,
    scope: SearchScope,
    memory_scope: MemoryScope,
    store: Arc<dyn MemoryStore>,
    embedder: Option<Arc<dyn TextEmbedder>>,
}

/// Progress through an explicit scoped embedding rebuild. Restart at offset zero
/// after concurrent correction/deletion, since pagination reflects live records.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MemoryEmbeddingReport {
    /// Live records examined in this bounded page.
    pub examined: usize,
    /// Total live records at the page's source snapshot.
    pub total: usize,
    /// Next live-record offset, absent at the end of this snapshot.
    pub next_offset: Option<usize>,
}

impl MemorySearchSource {
    /// Bind a memory store and optional explicitly configured embedder.
    /// Construction never embeds or mutates memory. Missing vectors remain
    /// visible through semantic coverage; indexing stays in the reconciler.
    ///
    /// # Errors
    /// Rejects malformed scope/configuration, incompatible store bounds, or
    /// invalid embedder dimensions/identity.
    pub fn try_new(
        source_id: &str,
        store: Arc<dyn MemoryStore>,
        scope: SearchScope,
        mapping: ScopeMapping,
        limits: SearchLimits,
        embedder: Option<Arc<dyn TextEmbedder>>,
    ) -> Result<Self, SearchError> {
        validate_id(source_id)?;
        scope.validate()?;
        limits.validate()?;
        let store_descriptor = store.descriptor();
        if store_descriptor.limits.max_records > limits.max_scan_records
            || limits.max_results > store_descriptor.limits.max_search_results
        {
            return Err(SearchError::invalid("memory_store_search_limits"));
        }
        let memory_scope = memory_scope(&scope)?;
        let semantic_spaces = match &embedder {
            None => Vec::new(),
            Some(embedder) => {
                let descriptor = embedder.descriptor();
                validate_id(&descriptor.embedder_id)?;
                if descriptor.dimensions == 0
                    || descriptor.dimensions > store_descriptor.limits.max_embedding_dimensions
                    || descriptor.max_input_bytes == 0
                {
                    return Err(SearchError::invalid("memory_embedder_limits"));
                }
                vec![descriptor.embedder_id]
            }
        };
        let digest = configuration_digest(
            "memory-search-config",
            &serde_json::json!({
                "version": 1, "source": source_id, "store": store_descriptor.store_id,
                "scope": scope, "mapping": mapping, "limits": limits, "spaces": semantic_spaces,
                "embedder": embedder.as_ref().map(|value| { let d = value.descriptor(); (d.embedder_id,d.dimensions,d.max_input_bytes) }),
            }),
        )?;
        Ok(Self {
            descriptor: SearchSourceDescriptor {
                source_id: source_id.into(),
                kind: "memory".into(),
                lexical: vec![LexicalKind::Keyword, LexicalKind::Bm25],
                semantic_spaces,
                evidence_lookup: true,
                graph: false,
                durable: store_descriptor.durable,
                scope_mapping: mapping,
                limits,
                configuration_digest: digest,
            },
            scope,
            memory_scope,
            store,
            embedder,
        })
    }

    /// Rebuild vectors for one exact scoped page with the configured embedder.
    /// This explicit host operation may send source text to that embedder. It
    /// never scans or embeds another scope; source digests guard concurrent writes.
    /// Repeat from offset zero after concurrent changes to the live listing.
    ///
    /// # Errors
    /// Rejects absent embedders, excessive pages, scope violations or source failure.
    pub async fn reconcile_embeddings(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<MemoryEmbeddingReport, SearchError> {
        if limit == 0 || limit > 256 || offset > self.descriptor.limits.max_scan_records {
            return Err(SearchError::invalid("memory_embedding_page"));
        }
        let embedder = self
            .embedder
            .as_ref()
            .ok_or(SearchError::SearchUnsupported)?;
        let listing = self
            .store
            .list(self.memory_scope.clone(), MemoryPage { offset, limit })
            .await
            .map_err(map_error)?;
        if listing.records.len() > limit || listing.total > self.descriptor.limits.max_scan_records
        {
            return Err(SearchError::invalid("memory_embedding_page"));
        }
        self.check_scan_budget(listing.total, true)?;
        let count = listing.records.len();
        let mut pending = Vec::with_capacity(count);
        for record in listing.records {
            if record.scope != self.memory_scope {
                return Err(SearchError::SearchScopeDenied);
            }
            let text = embedding_source_text(&record);
            let source_digest = embedding_source_digest(&text).map_err(map_error)?;
            pending.push(crate::store::EmbeddingSource {
                scope: record.scope.clone(),
                id: record.id.clone(),
                text: text.into(),
                source_digest,
            });
        }
        crate::store::apply_embedding_sources(self.store.as_ref(), embedder.as_ref(), pending)
            .await
            .map_err(map_error)?;
        let next = offset.saturating_add(count);
        Ok(MemoryEmbeddingReport {
            examined: count,
            total: listing.total,
            next_offset: (count > 0 && next < listing.total).then_some(next),
        })
    }

    async fn execute(&self, query: SearchQuery, limit: usize) -> Result<SourceResult, SearchError> {
        if !query.journal.is_empty() || !self.descriptor.supports(&query.strategy) {
            return Err(SearchError::SearchUnsupported);
        }
        let listing = self
            .store
            .list(
                self.memory_scope.clone(),
                MemoryPage {
                    offset: 0,
                    limit: 0,
                },
            )
            .await
            .map_err(map_error)?;
        self.check_scan_budget(
            listing.total,
            matches!(query.strategy, SearchStrategy::Semantic { .. }),
        )?;
        let memory_query = self.memory_query(&query).await?;
        let coverage = if let SearchStrategy::Semantic { space } = &query.strategy {
            self.store
                .embedding_coverage(self.memory_scope.clone(), Arc::clone(space))
                .await
                .map_err(map_error)?
        } else {
            None
        };
        let raw = self
            .store
            .search(self.memory_scope.clone(), memory_query, limit)
            .await
            .map_err(map_error)?;
        if raw.len() > limit {
            return Err(SearchError::invalid("memory_store_result_bounds"));
        }
        let mut hits = Vec::with_capacity(raw.len());
        for item in raw {
            if item.record.scope != self.memory_scope {
                return Err(SearchError::SearchScopeDenied);
            }
            if item.record.tombstoned || item.record.superseded_by.is_some() {
                continue;
            }
            hits.push(self.hit(&item)?);
        }
        let mut result = SourceResult::completed(hits, listing.total as u64);
        // The legacy store returns bounded hits without a has-more flag. A full
        // page is conservatively marked truncated rather than claiming coverage.
        if result.hits.len() == limit {
            result.truncate("memory_result_limit");
        }
        if matches!(query.strategy, SearchStrategy::Semantic { .. }) {
            match coverage {
                Some(coverage) if coverage.live_records == coverage.indexed_records => {}
                Some(coverage) => {
                    result.truncate("memory_embeddings_pending");
                    if coverage.indexed_records == 0 {
                        result.status = finstack_ai_search_core::SourceStatus::Unavailable;
                    }
                }
                None => result.truncate("memory_embedding_coverage_unknown"),
            }
        }
        Ok(result)
    }

    fn check_scan_budget(&self, records: usize, semantic: bool) -> Result<(), SearchError> {
        if records > self.descriptor.limits.max_scan_records {
            return Err(SearchError::SearchCapacityExceeded {
                resource: "scan_records".into(),
            });
        }
        if semantic && let Some(embedder) = &self.embedder {
            let bytes = (records as u64)
                .saturating_mul(embedder.descriptor().dimensions as u64)
                .saturating_mul(4);
            if bytes > self.descriptor.limits.max_embedding_bytes {
                return Err(SearchError::SearchCapacityExceeded {
                    resource: "embedding_bytes".into(),
                });
            }
        }
        Ok(())
    }

    async fn memory_query(&self, query: &SearchQuery) -> Result<MemoryQuery, SearchError> {
        Ok(match &query.strategy {
            SearchStrategy::Lexical(LexicalKind::Keyword) => MemoryQuery::Keywords(
                query
                    .text
                    .split_whitespace()
                    .map(Arc::<str>::from)
                    .collect::<Vec<_>>()
                    .into(),
            ),
            SearchStrategy::Lexical(LexicalKind::Bm25) => {
                MemoryQuery::FullText(Arc::clone(&query.text))
            }
            SearchStrategy::Semantic { space } => {
                let embedder = self
                    .embedder
                    .as_ref()
                    .ok_or(SearchError::SearchUnsupported)?;
                let descriptor = embedder.descriptor();
                let vectors = embedder
                    .embed(vec![Arc::from(truncate_to_bytes(
                        &query.text,
                        descriptor.max_input_bytes,
                    ))])
                    .await
                    .map_err(|_| SearchError::SearchUnavailable)?;
                if vectors.len() != 1 {
                    return Err(SearchError::SearchUnavailable);
                }
                let vector = vectors
                    .into_iter()
                    .next()
                    .ok_or(SearchError::SearchUnavailable)?;
                if vector.dimensions() != descriptor.dimensions {
                    return Err(SearchError::SearchUnavailable);
                }
                MemoryQuery::Embedding {
                    embedder_id: Arc::clone(space),
                    vector,
                }
            }
            _ => return Err(SearchError::SearchUnsupported),
        })
    }

    fn hit(&self, item: &MemoryHit) -> Result<SearchHit, SearchError> {
        let text = embedding_source_text(&item.record);
        let mut locators = Vec::new();
        for (label, value) in [
            ("session", &item.record.provenance.source_session),
            ("run", &item.record.provenance.source_run),
            ("source", &item.record.provenance.source_ref),
        ] {
            if let Some(value) = value {
                let value = format!("{label}:{value}");
                if value.len() > 1024 {
                    return Err(SearchError::invalid("memory_provenance_limit"));
                }
                locators.push(value.into());
            }
        }
        let hit = SearchHit {
            entity_label: None,
            source: Arc::clone(&self.descriptor.source_id),
            reference: SourceRef::Memory {
                id: item.record.id.as_str().into(),
            },
            score: item.score,
            preview: preview(&text, self.descriptor.limits.max_preview_chars),
            sensitivity: item.record.sensitivity,
            provenance: SearchProvenance {
                scope: self.scope.clone(),
                content_digest: embedding_source_digest(&text).map_err(map_error)?,
                locators,
                citations: Vec::new(),
            },
        };
        hit.validate(&self.descriptor.limits)?;
        Ok(hit)
    }
}

impl SearchSource for MemorySearchSource {
    fn references(
        &self,
        scope: SearchScope,
        cursor: Option<String>,
        limit: usize,
    ) -> PortFuture<Result<finstack_ai_search_core::SourceReferencePage, SearchError>> {
        let source = self.clone();
        Box::pin(async move {
            source.authorize(
                &scope,
                &SearchQuery::lexical("catalog", LexicalKind::Literal),
            )?;
            finstack_ai_search_core::validate_catalog_request(cursor.as_deref(), limit)?;
            let offset = cursor
                .as_deref()
                .unwrap_or("0")
                .parse::<usize>()
                .map_err(|_| SearchError::invalid("memory_catalog_cursor"))?;
            if offset > source.descriptor.limits.max_scan_records {
                return Err(SearchError::invalid("memory_catalog_cursor"));
            }
            let listing = source
                .store
                .list(source.memory_scope.clone(), MemoryPage { offset, limit })
                .await
                .map_err(map_error)?;
            source.check_scan_budget(listing.total, false)?;
            if listing.records.len() > limit {
                return Err(SearchError::invalid("memory_catalog_limit"));
            }
            let mut references = Vec::new();
            for record in listing.records {
                if record.scope != source.memory_scope {
                    return Err(SearchError::SearchScopeDenied);
                }
                references.push(SourceRef::Memory {
                    id: record.id.as_str().into(),
                });
            }
            let next = offset.saturating_add(references.len());
            Ok(finstack_ai_search_core::SourceReferencePage {
                next_cursor: (!references.is_empty() && next < listing.total)
                    .then(|| next.to_string()),
                references,
            })
        })
    }
    fn descriptor(&self) -> SearchSourceDescriptor {
        self.descriptor.clone()
    }

    fn authorize(&self, scope: &SearchScope, query: &SearchQuery) -> Result<(), SearchError> {
        self.descriptor.scope_mapping.resolve(&self.scope, scope)?;
        if matches!(
            query.strategy,
            SearchStrategy::Lexical(LexicalKind::Keyword)
        ) {
            let words: Vec<_> = query.text.split_whitespace().collect();
            if words.is_empty()
                || words.len() > crate::record::KEYWORDS_MAX_COUNT
                || words
                    .iter()
                    .any(|word| word.len() > crate::record::KEYWORD_MAX_BYTES)
            {
                return Err(SearchError::invalid("memory_query_keywords"));
            }
        }
        if matches!(query.strategy, SearchStrategy::Lexical(LexicalKind::Bm25))
            && query.text.len() > crate::record::INLINE_BODY_MAX_BYTES
        {
            return Err(SearchError::invalid("memory_query_text"));
        }
        Ok(())
    }

    fn search(
        &self,
        scope: SearchScope,
        query: SearchQuery,
        limit: usize,
    ) -> PortFuture<Result<SourceResult, SearchError>> {
        let owned = self.clone();
        Box::pin(async move {
            query.validate(&owned.descriptor.limits, limit)?;
            owned.authorize(&scope, &query)?;
            owned.execute(query, limit).await
        })
    }

    fn read_evidence(
        &self,
        scope: SearchScope,
        reference: SourceRef,
    ) -> PortFuture<Result<Option<finstack_ai_search_core::SearchEvidence>, SearchError>> {
        let source = self.clone();
        Box::pin(async move {
            source
                .descriptor
                .scope_mapping
                .resolve(&source.scope, &scope)?;
            reference.key()?;
            let SourceRef::Memory { id } = reference else {
                return Err(SearchError::SearchUnsupported);
            };
            let id = crate::record::MemoryId::parse(&id)
                .map_err(|_| SearchError::invalid("memory_reference"))?;
            let Some(record) = source
                .store
                .get(source.memory_scope.clone(), id)
                .await
                .map_err(map_error)?
            else {
                return Ok(None);
            };
            if record.scope != source.memory_scope {
                return Err(SearchError::SearchScopeDenied);
            }
            if record.tombstoned || record.superseded_by.is_some() {
                return Ok(None);
            }
            let text = embedding_source_text(&record);
            let complete = matches!(record.body, crate::record::MemoryBody::Inline(_));
            let hit = source.hit(&MemoryHit {
                record,
                score: 0,
                matched: crate::store::MatchEvidence::ExactId,
            })?;
            let evidence = finstack_ai_search_core::SearchEvidence {
                hit,
                text: text.into(),
                complete,
            };
            evidence.validate(&source.descriptor.limits)?;
            Ok(Some(evidence))
        })
    }
}

fn memory_scope(scope: &SearchScope) -> Result<MemoryScope, SearchError> {
    let mut result =
        MemoryScope::try_new(&scope.tenant).map_err(|_| SearchError::invalid("memory_scope"))?;
    if let Some(user) = &scope.user {
        result = result
            .try_with_user(user)
            .map_err(|_| SearchError::invalid("memory_scope"))?;
    }
    if let Some(agent) = &scope.agent {
        result = result
            .try_with_agent(agent)
            .map_err(|_| SearchError::invalid("memory_scope"))?;
    }
    if let Some(workspace) = &scope.workspace {
        result = result
            .try_with_workspace(workspace)
            .map_err(|_| SearchError::invalid("memory_scope"))?;
    }
    Ok(result)
}

#[allow(clippy::needless_pass_by_value)] // Result::map_err consumes the source error.
fn map_error(error: MemoryStoreError) -> SearchError {
    match error {
        MemoryStoreError::ScopeMismatch => SearchError::SearchScopeDenied,
        MemoryStoreError::InvalidRequest {
            reason: "memory_embeddings_unsupported",
        } => SearchError::SearchUnsupported,
        MemoryStoreError::InvalidRequest { .. } => SearchError::invalid("memory_query"),
        MemoryStoreError::CapacityExceeded { resource, .. } => {
            SearchError::SearchCapacityExceeded {
                resource: resource.into(),
            }
        }
        _ => SearchError::SearchUnavailable,
    }
}
