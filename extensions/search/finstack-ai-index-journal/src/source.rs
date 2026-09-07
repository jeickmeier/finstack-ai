use crate::database::Database;
use finstack_ai_kernel::{Digest, Sensitivity, SessionId};
use finstack_ai_runtime::ports::{PortFuture, journal::JournalStore};
use finstack_ai_search_core::{
    LexicalKind, MAX_EVIDENCE_BYTES, ScopeMapping, SearchError, SearchEvidence, SearchLimits,
    SearchQuery, SearchScope, SearchSource, SearchSourceDescriptor, SearchStrategy, SourceRef,
    SourceResult, configuration_digest, validate_id,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeSet, path::Path, sync::Arc};

/// Immutable authority and finite operating limits for one journal index.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JournalIndexConfig {
    /// Namespace used in references.
    pub source_id: Arc<str>,
    /// Complete host-authenticated scope.
    pub scope: SearchScope,
    /// Explicit host-authorized sessions (at most 256), never discovered by scan.
    pub sessions: Vec<SessionId>,
    /// Exact mapping unless restrictive filling is explicitly requested.
    #[serde(default)]
    pub scope_mapping: ScopeMapping,
    /// Query scan, result, preview and traversal ceilings.
    #[serde(default)]
    pub limits: SearchLimits,
    /// Maximum indexed entries for the entire bound scope, at most 100,000.
    pub max_entries: usize,
    /// Maximum retained text for an entry, at most 64 KiB.
    pub max_entry_bytes: usize,
    /// Classification floor for journal text, which has no mandatory per-message label.
    pub sensitivity: Sensitivity,
    /// Timeout on every journal read, between 1 and 60,000 milliseconds.
    pub io_timeout_ms: u64,
}
impl JournalIndexConfig {
    /// Offline lexical defaults over explicitly authorized sessions.
    #[must_use]
    pub fn new(
        source_id: impl Into<Arc<str>>,
        scope: SearchScope,
        sessions: Vec<SessionId>,
    ) -> Self {
        Self {
            source_id: source_id.into(),
            scope,
            sessions,
            scope_mapping: ScopeMapping::Exact,
            limits: SearchLimits::default(),
            max_entries: 100_000,
            max_entry_bytes: MAX_EVIDENCE_BYTES,
            sensitivity: Sensitivity::Confidential,
            io_timeout_ms: 10_000,
        }
    }
    /// Validate authority and capacities before opening storage.
    ///
    /// # Errors
    /// Rejects duplicate/excessive sessions, invalid scope or unbounded capacities.
    pub fn validate(&self) -> Result<(), SearchError> {
        validate_id(&self.source_id)?;
        self.scope.validate()?;
        self.limits.validate()?;
        if self.sessions.len() > 256
            || self.sessions.iter().collect::<BTreeSet<_>>().len() != self.sessions.len()
            || self.max_entries == 0
            || self.max_entries > 100_000
            || self.max_entry_bytes == 0
            || self.max_entry_bytes > MAX_EVIDENCE_BYTES
            || self.io_timeout_ms == 0
            || self.io_timeout_ms > 60_000
            || self.sensitivity == Sensitivity::Credential
        {
            return Err(SearchError::invalid("journal_index_config"));
        }
        Ok(())
    }
}
/// Rebuildable `SQLite` index consuming the existing journal port read-only.
/// Index hints and maintenance never enter the journal append path.
#[derive(Clone)]
pub struct JournalSearchSource {
    pub(crate) database: Database,
    pub(crate) store: Arc<dyn JournalStore>,
    pub(crate) config: JournalIndexConfig,
    pub(crate) scope_digest: String,
    pub(crate) descriptor: SearchSourceDescriptor,
    pub(crate) maintenance: Arc<tokio::sync::Mutex<()>>,
}
impl JournalSearchSource {
    /// Open an owned schema-versioned index. Call `sync_session` or drain observer
    /// hints explicitly; construction performs no journal reads or writes.
    ///
    /// # Errors
    /// Rejects invalid configuration, foreign schemas, or unavailable index storage.
    pub fn try_open(
        path: &Path,
        store: Arc<dyn JournalStore>,
        config: JournalIndexConfig,
    ) -> Result<Self, SearchError> {
        config.validate()?;
        let database = Database::open(path)?;
        let digest = configuration_digest(
            "journal-search",
            &(&config, store.descriptor().store_id, database.identity),
        )?;
        let descriptor = SearchSourceDescriptor {
            source_id: config.source_id.clone(),
            kind: "journal".into(),
            lexical: vec![
                LexicalKind::Keyword,
                LexicalKind::Bm25,
                LexicalKind::Literal,
                LexicalKind::Regex,
            ],
            semantic_spaces: vec![],
            evidence_lookup: true,
            graph: false,
            durable: path != Path::new(":memory:"),
            scope_mapping: config.scope_mapping,
            limits: config.limits,
            configuration_digest: digest,
        };
        Ok(Self {
            database,
            store,
            scope_digest: config.scope.digest()?.to_hex(),
            config,
            descriptor,
            maintenance: Arc::new(tokio::sync::Mutex::new(())),
        })
    }
    /// Immutable source configuration and authorized session list.
    #[must_use]
    pub const fn config(&self) -> &JournalIndexConfig {
        &self.config
    }
    pub(crate) fn authorize_session(&self, session: SessionId) -> Result<(), SearchError> {
        if !self.config.sessions.contains(&session) {
            return Err(SearchError::SearchScopeDenied);
        }
        Ok(())
    }
    pub(crate) async fn read<T>(
        &self,
        future: impl std::future::Future<
            Output = Result<T, finstack_ai_runtime::ports::journal::StoreError>,
        >,
    ) -> Result<T, SearchError> {
        tokio::time::timeout(
            std::time::Duration::from_millis(self.config.io_timeout_ms),
            future,
        )
        .await
        .map_err(|_| SearchError::SearchUnavailable)?
        .map_err(crate::indexing::store_error)
    }
    /// Versioned index identity used by the hint observer and resolved components.
    #[must_use]
    pub const fn configuration_digest(&self) -> Digest {
        self.descriptor.configuration_digest
    }
}
impl SearchSource for JournalSearchSource {
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
        for session in &query.journal.sessions {
            self.authorize_session(*session)?;
        }
        if matches!(
            query.strategy,
            SearchStrategy::Lexical(LexicalKind::Keyword | LexicalKind::Bm25)
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
            if !source.descriptor.supports(&query.strategy) {
                return Err(SearchError::SearchUnsupported);
            }
            source.retrieve(query, limit).await
        })
    }
    fn read_evidence(
        &self,
        scope: SearchScope,
        reference: SourceRef,
    ) -> PortFuture<Result<Option<SearchEvidence>, SearchError>> {
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
