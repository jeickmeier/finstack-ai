use std::path::Path;
use std::sync::Arc;

use finstack_ai_embeddings::embedder::TextEmbedder;
use finstack_ai_kernel::Digest;
use finstack_ai_runtime::artifact::ArtifactStore;
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_search_core::{
    LexicalKind, ScopeMapping, SearchError, SearchLimits, SearchQuery, SearchScope, SearchSource,
    SearchSourceDescriptor, SourceResult, configuration_digest, validate_id,
};
use finstack_ai_tools_document::parser::DocumentLimits;
use serde::{Deserialize, Serialize};

use crate::ChunkerConfig;
use crate::database::Database;

/// Versioned document indexing and query limits bound to one application scope.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentIndexConfig {
    /// Unique source namespace used in citations.
    pub source_id: Arc<str>,
    /// Host-authenticated exact scope.
    pub scope: SearchScope,
    /// Exact by default; restrictive mapping never drops bound dimensions.
    #[serde(default)]
    pub scope_mapping: ScopeMapping,
    /// Character-based chunking configuration; default 4,096/512, version 1.
    #[serde(default)]
    pub chunker: ChunkerConfig,
    /// Query/scan/result/preview/embedding resource ceilings.
    #[serde(default)]
    pub limits: SearchLimits,
    /// Existing document parser's finite input/output/page ceilings.
    #[serde(default)]
    pub parse_limits: DocumentLimits,
    /// Maximum derived chunks in this complete scope, at most 100,000.
    pub max_chunks: usize,
    /// Maximum distinct indexed artifact/chunker pairs, including OCR-only entries.
    pub max_documents: usize,
}

impl DocumentIndexConfig {
    /// Offline lexical defaults for one scope (100,000 chunks, 10,000 documents).
    #[must_use]
    pub fn new(source_id: impl Into<Arc<str>>, scope: SearchScope) -> Self {
        Self {
            source_id: source_id.into(),
            scope,
            scope_mapping: ScopeMapping::default(),
            chunker: ChunkerConfig::default(),
            limits: SearchLimits::default(),
            parse_limits: DocumentLimits::default(),
            max_chunks: 100_000,
            max_documents: 10_000,
        }
    }

    /// Validate supported operating envelope before opening storage.
    ///
    /// # Errors
    /// Rejects invalid identifiers, scope, chunker, parser limits, or capacity.
    pub fn validate(&self) -> Result<(), SearchError> {
        validate_id(&self.source_id)?;
        self.scope.validate()?;
        self.chunker.validate()?;
        self.limits.validate()?;
        if self.max_chunks == 0
            || self.max_chunks > 100_000
            || self.max_documents == 0
            || self.max_documents > 100_000
            || self.parse_limits.max_input_bytes == 0
            || self.parse_limits.max_input_bytes > 4 * 1024 * 1024
            || self.parse_limits.max_output_bytes == 0
            || self.parse_limits.max_output_bytes > 4 * 1024 * 1024
            || self.parse_limits.max_pages == 0
            || self.parse_limits.max_pages > 500
        {
            return Err(SearchError::invalid("document_index_limits"));
        }
        Ok(())
    }

    /// Fingerprint of versioned extraction plus chunking policy. Parser output
    /// ceilings affect searchable content and therefore invalidate old receipts.
    ///
    /// # Errors
    /// Rejects invalid configuration or encoding failures.
    pub fn processing_digest(&self) -> Result<Digest, SearchError> {
        self.validate()?;
        configuration_digest(
            "document-processing",
            &(
                1,
                env!("CARGO_PKG_VERSION"),
                self.chunker,
                &self.parse_limits,
            ),
        )
    }
}

/// `SQLite` document index with exact scoped artifact references. Its database is
/// derived data and can be reconstructed from explicit authorized artifact inputs.
/// The index never pins artifacts: ownership remains with the system of record.
#[derive(Clone)]
pub struct DocumentSearchSource {
    pub(crate) database: Database,
    pub(crate) artifacts: Arc<dyn ArtifactStore>,
    pub(crate) embedder: Option<Arc<dyn TextEmbedder>>,
    pub(crate) config: DocumentIndexConfig,
    pub(crate) descriptor: SearchSourceDescriptor,
    pub(crate) scope_digest: String,
    pub(crate) chunker_digest: Digest,
}

impl DocumentSearchSource {
    /// Open an owned, schema-versioned `SQLite` index. `:memory:` is supported for
    /// tests. This source maps only the exact bound scope (or explicitly fills
    /// already-bound restrictions); artifact read scopes remain independently exact.
    ///
    /// # Errors
    /// Rejects schema/configuration drift, invalid embedder limits, or unavailable
    /// storage. A remote embedder is an explicit host egress decision.
    pub fn try_open(
        path: &Path,
        artifacts: Arc<dyn ArtifactStore>,
        config: DocumentIndexConfig,
        embedder: Option<Arc<dyn TextEmbedder>>,
    ) -> Result<Self, SearchError> {
        config.validate()?;
        let spaces = match &embedder {
            Some(embedder) => {
                let descriptor = embedder.descriptor();
                validate_id(&descriptor.embedder_id)?;
                if descriptor.dimensions == 0
                    || descriptor.dimensions > 4096
                    || descriptor.max_input_bytes == 0
                {
                    return Err(SearchError::invalid("document_embedder"));
                }
                vec![descriptor.embedder_id]
            }
            None => Vec::new(),
        };
        let database = Database::open(path)?;
        let digest = configuration_digest(
            "document-search",
            &(
                &config,
                database.identity,
                artifacts.descriptor().store_id,
                &spaces,
                embedder.as_ref().map(|value| {
                    let d = value.descriptor();
                    (d.embedder_id, d.dimensions, d.max_input_bytes)
                }),
            ),
        )?;
        let descriptor = SearchSourceDescriptor {
            source_id: Arc::clone(&config.source_id),
            kind: "documents".into(),
            lexical: vec![
                LexicalKind::Keyword,
                LexicalKind::Bm25,
                LexicalKind::Literal,
                LexicalKind::Regex,
            ],
            semantic_spaces: spaces,
            evidence_lookup: true,
            graph: false,
            durable: path != Path::new(":memory:"),
            scope_mapping: config.scope_mapping,
            limits: config.limits,
            configuration_digest: digest,
        };
        Ok(Self {
            database,
            artifacts,
            embedder,
            scope_digest: config.scope.digest()?.to_hex(),
            chunker_digest: config.processing_digest()?,
            config,
            descriptor,
        })
    }

    /// Immutable indexing configuration.
    #[must_use]
    pub const fn config(&self) -> &DocumentIndexConfig {
        &self.config
    }
}

impl SearchSource for DocumentSearchSource {
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
            source.catalog(cursor, limit).await
        })
    }
    fn descriptor(&self) -> SearchSourceDescriptor {
        self.descriptor.clone()
    }
    fn authorize(&self, scope: &SearchScope, query: &SearchQuery) -> Result<(), SearchError> {
        self.config
            .scope_mapping
            .resolve(&self.config.scope, scope)?;
        if matches!(
            query.strategy,
            finstack_ai_search_core::SearchStrategy::Lexical(
                LexicalKind::Keyword | LexicalKind::Bm25
            )
        ) {
            crate::retrieval::fts_expression(&query.text)?;
        }
        Ok(())
    }
    fn search(
        &self,
        scope: SearchScope,
        query: SearchQuery,
        limit: usize,
    ) -> PortFuture<Result<SourceResult, SearchError>> {
        let source = self.clone();
        Box::pin(async move {
            query.validate(&source.config.limits, limit)?;
            source.authorize(&scope, &query)?;
            if !query.journal.is_empty() || !source.descriptor.supports(&query.strategy) {
                return Err(SearchError::SearchUnsupported);
            }
            source.retrieve(query, limit).await
        })
    }

    fn read_evidence(
        &self,
        scope: SearchScope,
        reference: finstack_ai_search_core::SourceRef,
    ) -> PortFuture<Result<Option<finstack_ai_search_core::SearchEvidence>, SearchError>> {
        let source = self.clone();
        Box::pin(async move {
            source
                .config
                .scope_mapping
                .resolve(&source.config.scope, &scope)?;
            reference.key()?;
            source.exact_evidence(reference).await
        })
    }
}
