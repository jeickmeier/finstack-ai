use crate::{GraphVocabulary, database::Database, vocabulary::Compiled};
use finstack_ai_kernel::Digest;
use finstack_ai_runtime::ports::PortFuture;
use finstack_ai_search_core::{
    GraphQuery, LexicalKind, ScopeMapping, SearchError, SearchEvidence, SearchLimits, SearchQuery,
    SearchScope, SearchSource, SearchSourceDescriptor, SearchStrategy, SourceRef, SourceResult,
    configuration_digest, validate_id,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path, sync::Arc};

/// Exact authority, extraction vocabulary and bounded graph resource capacities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphIndexConfig {
    /// Source namespace used in graph citations.
    pub source_id: Arc<str>,
    /// Complete host-authenticated scope.
    pub scope: SearchScope,
    /// Exact mapping by default; restrictive filling must be explicit.
    #[serde(default)]
    pub scope_mapping: ScopeMapping,
    /// Versioned entity/relationship vocabulary and bounded regex rules.
    pub vocabulary: GraphVocabulary,
    /// Query, preview, scan, and traversal ceilings.
    #[serde(default)]
    pub limits: SearchLimits,
    /// Maximum indexed source references in this scope, at most 100,000.
    pub max_sources: usize,
    /// Maximum stored entities in this scope, at most 100,000.
    pub max_entities: usize,
    /// Maximum stored relationships in this scope, at most 100,000.
    pub max_edges: usize,
    /// Maximum extracted entities from one reference, at most 1,024.
    pub max_entities_per_source: usize,
    /// Maximum extracted relationships from one reference, at most 4,096.
    pub max_edges_per_source: usize,
    /// Maximum supporting source references per entity or edge, at most 256.
    pub max_support_per_item: usize,
    /// Maximum distinct source reads during one query, at most 4,096.
    pub max_evidence_reads: usize,
    /// Timeout for each exact source read, at most 60 seconds.
    pub source_timeout_ms: u64,
}
impl GraphIndexConfig {
    /// Conservative opt-in graph defaults. Lexical search compositions do not
    /// construct this source unless the application supplies a vocabulary.
    #[must_use]
    pub fn new(
        source_id: impl Into<Arc<str>>,
        scope: SearchScope,
        vocabulary: GraphVocabulary,
    ) -> Self {
        Self {
            source_id: source_id.into(),
            scope,
            scope_mapping: ScopeMapping::Exact,
            vocabulary,
            limits: SearchLimits::default(),
            max_sources: 100_000,
            max_entities: 10_000,
            max_edges: 100_000,
            max_entities_per_source: 128,
            max_edges_per_source: 256,
            max_support_per_item: 64,
            max_evidence_reads: 256,
            source_timeout_ms: 1000,
        }
    }
    /// Validate authority, vocabulary and finite extraction/query capacities.
    ///
    /// # Errors
    /// Rejects unsupported vocabularies, invalid scope or excessive capacities.
    pub fn validate(&self) -> Result<(), SearchError> {
        validate_id(&self.source_id)?;
        self.scope.validate()?;
        self.limits.validate()?;
        self.vocabulary.validate()?;
        for (value, max) in [
            (self.max_sources, 100_000),
            (self.max_entities, 100_000),
            (self.max_edges, 100_000),
            (self.max_entities_per_source, 1024),
            (self.max_edges_per_source, 4096),
            (self.max_support_per_item, 256),
            (self.max_evidence_reads, 4096),
        ] {
            if value == 0 || value > max {
                return Err(SearchError::invalid("graph_index_limits"));
            }
        }
        if self.source_timeout_ms == 0 || self.source_timeout_ms > 60_000 {
            return Err(SearchError::invalid("graph_source_timeout"));
        }
        Ok(())
    }
}
/// Scoped directed property graph. Neighborhoods follow both directions;
/// shortest-path queries follow directed relationships. Sources remain behind
/// `SearchSource` and must support exact evidence reads. Graph-on-graph recursion
/// is rejected, avoiding cyclic evidence authority.
#[derive(Clone)]
pub struct GraphSearchSource {
    pub(crate) database: Database,
    pub(crate) config: GraphIndexConfig,
    pub(crate) scope_digest: String,
    pub(crate) extraction_digest: Digest,
    pub(crate) compiled: Arc<Compiled>,
    pub(crate) sources: BTreeMap<Arc<str>, Arc<dyn SearchSource>>,
    pub(crate) descriptor: SearchSourceDescriptor,
    pub(crate) maintenance: Arc<tokio::sync::Mutex<()>>,
}
impl GraphSearchSource {
    /// Open an owned schema-versioned graph and bind its authorized source set.
    /// Construction performs no extraction, model call or evidence read.
    ///
    /// # Errors
    /// Rejects invalid configuration, duplicate/graph sources, missing exact-read
    /// support, unauthorized source mappings or unavailable index storage.
    pub fn try_open(
        path: &Path,
        config: GraphIndexConfig,
        sources: Vec<Arc<dyn SearchSource>>,
    ) -> Result<Self, SearchError> {
        config.validate()?;
        if sources.is_empty() || sources.len() > 32 {
            return Err(SearchError::invalid("graph_sources"));
        }
        let mut bound = BTreeMap::new();
        let mut descriptors = Vec::new();
        for source in sources {
            let descriptor = source.descriptor();
            validate_id(&descriptor.source_id)?;
            if descriptor.graph
                || !descriptor.evidence_lookup
                || descriptor.source_id == config.source_id
            {
                return Err(SearchError::invalid("graph_evidence_source"));
            }
            source.authorize(
                &config.scope,
                &SearchQuery::lexical("evidence", LexicalKind::Literal),
            )?;
            if bound.insert(descriptor.source_id.clone(), source).is_some() {
                return Err(SearchError::invalid("graph_duplicate_source"));
            }
            descriptors.push(descriptor);
        }
        descriptors.sort_by(|a, b| a.source_id.cmp(&b.source_id));
        let database = Database::open(path)?;
        let compiled = Arc::new(Compiled::new(&config.vocabulary)?);
        let extraction_digest = configuration_digest("graph-extraction", &(1, &config.vocabulary))?;
        let digest =
            configuration_digest("graph-search", &(&config, descriptors, database.identity))?;
        let descriptor = SearchSourceDescriptor {
            source_id: config.source_id.clone(),
            kind: "graph".into(),
            lexical: vec![],
            semantic_spaces: vec![],
            evidence_lookup: false,
            graph: true,
            durable: path != Path::new(":memory:"),
            scope_mapping: config.scope_mapping,
            limits: config.limits,
            configuration_digest: digest,
        };
        Ok(Self {
            database,
            scope_digest: config.scope.digest()?.to_hex(),
            config,
            extraction_digest,
            compiled,
            sources: bound,
            descriptor,
            maintenance: Arc::new(tokio::sync::Mutex::new(())),
        })
    }
    /// Immutable graph configuration and resource ceilings.
    #[must_use]
    pub const fn config(&self) -> &GraphIndexConfig {
        &self.config
    }
    pub(crate) async fn evidence(
        &self,
        source_id: &str,
        reference: SourceRef,
    ) -> Result<Option<SearchEvidence>, SearchError> {
        let source = self
            .sources
            .get(source_id)
            .ok_or(SearchError::SearchUnsupported)?;
        reference.key()?;
        let evidence = tokio::time::timeout(
            std::time::Duration::from_millis(self.config.source_timeout_ms),
            source.read_evidence(self.config.scope.clone(), reference.clone()),
        )
        .await
        .map_err(|_| SearchError::SearchUnavailable)??;
        if let Some(evidence) = &evidence {
            evidence.validate(&source.descriptor().limits)?;
            if evidence.hit.source.as_ref() != source_id
                || evidence.hit.provenance.scope != self.config.scope
                || evidence.hit.reference != reference
            {
                return Err(SearchError::SearchScopeDenied);
            }
        }
        Ok(evidence)
    }
}
impl SearchSource for GraphSearchSource {
    fn descriptor(&self) -> SearchSourceDescriptor {
        self.descriptor.clone()
    }
    fn authorize(&self, scope: &SearchScope, query: &SearchQuery) -> Result<(), SearchError> {
        self.config
            .scope_mapping
            .resolve(&self.config.scope, scope)?;
        match &query.strategy {
            SearchStrategy::Graph(GraphQuery::Entity) => {
                crate::vocabulary::normalize(&query.text)?;
            }
            SearchStrategy::Graph(GraphQuery::Path { target, .. }) => {
                crate::vocabulary::normalize(&query.text)?;
                crate::vocabulary::normalize(target)?;
            }
            _ => {}
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
}
