//! [`MemoryContextProvider`]: a store-backed `ContextProvider` that recalls
//! matching [`MemoryRecord`]s into bounded, cache-stable context items.
//!
//! This provider never mutates conversation history and never writes to the
//! [`MemoryStore`] — writes belong to the toolset (a later task). It only
//! reads.

use std::collections::BTreeMap;
use std::sync::Arc;

use finstack_ai_kernel::{
    ComponentId, ComponentInvocation, ContentBlock, Digest, InvocationRecovery, Metadata,
    TextBlock, Timestamp, Version,
};
use finstack_ai_runtime::{
    ArtifactStore, ContextAuthority, ContextCallContext, ContextContribution, ContextError,
    ContextItem, ContextItemKind, ContextOverflowPolicy, ContextProvenance, ContextProvider,
    ContextProviderDescriptor, ContextRequest, PortFuture,
};

use crate::record::{MemoryError, MemoryId, MemoryScope};
use crate::store::{
    MatchEvidence, MemoryHit, MemoryQuery, MemoryStore, reconcile_memory_artifacts,
};

/// Stable component identity for [`MemoryContextProvider`].
const COMPONENT_ID: &str = "finstack.context.memory";

/// Tunables for a [`MemoryContextProvider`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct RecallConfig {
    /// Maximum number of items a single `collect` may return.
    pub max_hits: usize,
}

impl Default for RecallConfig {
    fn default() -> Self {
        Self { max_hits: 8 }
    }
}

/// Store-backed recall [`ContextProvider`].
///
/// Reads [`MemoryStore::search`] for keyword and full-text matches, merges
/// and deterministically orders them, and emits bounded `Reference` context
/// items. Ordering is a pure function of the current records and match
/// evidence, so identical input produces identical output without mutable
/// per-provider state.
pub struct MemoryContextProvider {
    descriptor: ContextProviderDescriptor,
    store: Arc<dyn MemoryStore>,
    artifact_store: Arc<dyn ArtifactStore>,
    scope: MemoryScope,
    config: RecallConfig,
}

impl MemoryContextProvider {
    /// Construct a provider bound to one store, scope, and configuration.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryError::Configuration`] when the checked-in component
    /// identity is invalid.
    pub fn try_new(
        store: Arc<dyn MemoryStore>,
        artifact_store: Arc<dyn ArtifactStore>,
        scope: MemoryScope,
        config: RecallConfig,
    ) -> Result<Self, MemoryError> {
        scope.validate()?;
        if config.max_hits == 0 || config.max_hits > store.descriptor().limits.max_search_results {
            return Err(MemoryError::Configuration {
                reason: "invalid_recall_limit",
            });
        }
        let identity = serde_json_canonicalizer::to_vec(&serde_json::json!({
            "version": 1,
            "scope": scope,
            "recall": config,
            "memory_store_id": store.descriptor().store_id,
            "artifact_store_id": artifact_store.descriptor().store_id,
        }))
        .map_err(|_| MemoryError::Configuration {
            reason: "invalid_provider_identity",
        })?;
        let configuration_digest = Digest::domain_separated("memory-provider", 1, &identity)
            .map_err(|_| MemoryError::Configuration {
                reason: "invalid_provider_identity",
            })?;
        Ok(Self {
            descriptor: ContextProviderDescriptor {
                invocation: ComponentInvocation {
                    component: ComponentId::parse(COMPONENT_ID).map_err(|_| {
                        MemoryError::Configuration {
                            reason: "invalid_component_id",
                        }
                    })?,
                    version: Version {
                        major: 0,
                        minor: 1,
                        patch: 0,
                    },
                    configuration_digest,
                    recovery: InvocationRecovery::RecomputeSafe,
                },
                trusted_application_instructions: false,
                metadata: Metadata::empty(),
            },
            store,
            artifact_store,
            scope,
            config,
        })
    }
}

impl ContextProvider for MemoryContextProvider {
    fn descriptor(&self) -> ContextProviderDescriptor {
        self.descriptor.clone()
    }

    fn collect(
        &self,
        ctx: ContextCallContext,
        request: ContextRequest,
    ) -> PortFuture<Result<ContextContribution, ContextError>> {
        let store = Arc::clone(&self.store);
        let artifact_store = Arc::clone(&self.artifact_store);
        let scope = self.scope.clone();
        let config = self.config;
        Box::pin(async move {
            if ctx.run.locator.tenant_scope.as_ref() != scope.tenant() {
                return Err(contribution_invalid(
                    "memory tenant scope does not match the committed effect",
                ));
            }

            let query = query_text(&request);
            let candidates = if query.is_empty() {
                Vec::new()
            } else {
                search_candidates(store.as_ref(), &scope, &query, config.max_hits).await?
            };
            reconcile_memory_artifacts(
                store.as_ref(),
                artifact_store.as_ref(),
                store.descriptor().limits.max_artifact_actions.min(256),
            )
            .await
            .map_err(|_| contribution_invalid("memory artifact reconciliation failed"))?;

            let ordered = order_candidates(candidates, config.max_hits);

            let mut entries = Vec::with_capacity(ordered.len());
            for hit in ordered {
                let item = build_item(&hit)?;
                entries.push(RecallEntry {
                    id: hit.record.id,
                    last_confirmed_at: hit.record.last_confirmed_at,
                    item,
                });
            }

            apply_budget(entries, &request)
        })
    }
}

/// One accepted recall result carrying the extra fields (`id`,
/// `last_confirmed_at`) needed for the cache key and stable-prefix state,
/// alongside the already-built [`ContextItem`].
struct RecallEntry {
    id: MemoryId,
    last_confirmed_at: Timestamp,
    item: ContextItem,
}

async fn search_candidates(
    store: &dyn MemoryStore,
    scope: &MemoryScope,
    query: &str,
    max_hits: usize,
) -> Result<Vec<MemoryHit>, ContextError> {
    let limit = max_hits;
    let tokens: Arc<[Arc<str>]> = query
        .split_whitespace()
        .map(Arc::<str>::from)
        .collect::<Vec<_>>()
        .into();

    let keyword_hits = store
        .search(scope.clone(), MemoryQuery::Keywords(tokens), limit)
        .await
        .map_err(|_| contribution_invalid("memory keyword search failed"))?;
    let full_text_hits = store
        .search(
            scope.clone(),
            MemoryQuery::FullText(Arc::from(query)),
            limit,
        )
        .await
        .map_err(|_| contribution_invalid("memory full-text search failed"))?;

    let mut merged: BTreeMap<MemoryId, MemoryHit> = BTreeMap::new();
    for hit in keyword_hits.into_iter().chain(full_text_hits) {
        // The store already excludes tombstoned/superseded records; filter
        // again defensively so a future store implementation cannot leak
        // stale records into recall silently.
        if hit.record.tombstoned || hit.record.superseded_by.is_some() {
            continue;
        }
        match merged.get_mut(&hit.record.id) {
            Some(existing) => {
                existing.score = existing.score.max(hit.score);
                // Keep the strongest evidence any leg produced for this
                // record, not whichever hit happened to score higher.
                // `score` is on a store-defined scale (a match count here, a
                // rank position there), so choosing evidence by score would
                // make recall order depend on which store is configured.
                if tier_of(&hit.matched) < tier_of(&existing.matched) {
                    existing.matched = hit.matched;
                }
            }
            None => {
                merged.insert(hit.record.id.clone(), hit);
            }
        }
    }
    Ok(merged.into_values().collect())
}

/// Sort candidates by evidence tier, score, confirmation time, then id.
fn order_candidates(mut candidates: Vec<MemoryHit>, max_hits: usize) -> Vec<MemoryHit> {
    candidates.sort_by(|a, b| {
        tier_of(&a.matched)
            .cmp(&tier_of(&b.matched))
            .then_with(|| b.score.cmp(&a.score))
            .then_with(|| b.record.last_confirmed_at.cmp(&a.record.last_confirmed_at))
            .then_with(|| a.record.id.cmp(&b.record.id))
    });
    candidates.truncate(max_hits);
    candidates
}

/// Score-bucket tier: keyword/exact-id matches recall before full-text
/// matches once the stable prefix has been applied.
fn tier_of(evidence: &MatchEvidence) -> u8 {
    match evidence {
        MatchEvidence::ExactId | MatchEvidence::Keyword(_) => 0,
        MatchEvidence::FullText => 1,
    }
}

fn build_item(hit: &MemoryHit) -> Result<ContextItem, ContextError> {
    let record = &hit.record;
    let artifact_label = artifact_label(record);
    let rendered = format!("{} [{}]", record.preview, artifact_label);
    let estimated_tokens = estimate_tokens(&rendered);
    ContextItem::try_new(
        ContextItemKind::Reference,
        vec![ContentBlock::Text(TextBlock::try_new(rendered).map_err(
            |_| contribution_invalid("memory preview is invalid"),
        )?)],
        ContextProvenance {
            source_id: Arc::from(COMPONENT_ID),
            source_ref: Some(Arc::from(record.id.as_str())),
            external: true,
        },
        ContextAuthority::Untrusted,
        0,
        estimated_tokens,
        record.sensitivity,
        false,
    )
}

fn artifact_label(record: &crate::record::MemoryRecord) -> Arc<str> {
    match &record.body {
        crate::record::MemoryBody::Inline(_) => Arc::from("memory"),
        crate::record::MemoryBody::Blob { artifact, .. } => artifact
            .blob()
            .name()
            .map_or_else(|| Arc::from("memory"), Arc::from),
    }
}

fn query_text(request: &ContextRequest) -> String {
    request
        .user_input
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text().to_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Apply the request's budget to `entries` in order, then attach a cache key
/// computed from `(id, last_confirmed_at)` pairs of the accepted items in
/// their final order.
///
/// Returns the built contribution plus the accepted ids (for stable-prefix
/// bookkeeping).
fn apply_budget(
    entries: Vec<RecallEntry>,
    request: &ContextRequest,
) -> Result<ContextContribution, ContextError> {
    let mut accepted_items = Vec::new();
    let mut accepted_meta: Vec<(MemoryId, Timestamp)> = Vec::new();
    let mut tokens = 0_u64;
    let mut bytes = 0_u64;
    for entry in entries {
        let next_tokens = tokens.saturating_add(entry.item.estimated_tokens);
        let next_bytes = bytes.saturating_add(entry.item.bytes);
        if accepted_items.len() >= request.budget.max_items
            || next_tokens > request.budget.max_tokens
            || next_bytes > request.budget.max_bytes
        {
            match request.budget.overflow {
                ContextOverflowPolicy::Reject => {
                    return Err(ContextError::try_new(
                        finstack_ai_runtime::CONTEXT_BUDGET_EXCEEDED,
                        finstack_ai_kernel::ErrorCategory::Limit,
                        "memory contribution exceeds the committed budget",
                        Metadata::empty(),
                    )
                    .unwrap_or_else(Into::into));
                }
                ContextOverflowPolicy::TruncateWithDiagnostic => break,
            }
        }
        tokens = next_tokens;
        bytes = next_bytes;
        accepted_meta.push((entry.id.clone(), entry.last_confirmed_at));
        accepted_items.push(entry.item);
    }
    let cache_key = cache_key_digest(&accepted_meta);
    ContextContribution::try_new(accepted_items, Some(cache_key))
}

/// Deterministic digest over `(id, last_confirmed_at.as_unix_ms())` pairs in
/// order, used as the contribution's cache key.
fn cache_key_digest(entries: &[(MemoryId, Timestamp)]) -> String {
    let mut bytes = Vec::new();
    for (id, timestamp) in entries {
        bytes.extend_from_slice(id.as_str().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(&timestamp.as_unix_ms().to_be_bytes());
        bytes.push(0);
    }
    Digest::raw_json(&bytes).to_hex()
}

fn estimate_tokens(text: &str) -> u64 {
    u64::try_from(text.len().div_ceil(4)).unwrap_or(1).max(1)
}

fn contribution_invalid(message: &'static str) -> ContextError {
    ContextError::try_new(
        finstack_ai_runtime::CONTEXT_CONTRIBUTION_INVALID,
        finstack_ai_kernel::ErrorCategory::Validation,
        message,
        Metadata::empty(),
    )
    .unwrap_or_else(Into::into)
}
