use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use crate::expansion::{ExpandedQuery, GraphExpansion};
use finstack_ai_kernel::Digest;
use finstack_ai_search_core::{
    HybridLeg, HybridPlan, JournalFilter, LegResult, LexicalKind, SearchError, SearchLimits,
    SearchQuery, SearchResponse, SearchScope, SearchSource, SearchSourceDescriptor, SearchStrategy,
    SourceResult, SourceStatus, configuration_digest, fuse, validate_id,
};
use futures_util::{StreamExt, TryStreamExt};
use serde::{Deserialize, Serialize};

/// Host-owned, frozen federation configuration. Native calls use Tokio deadlines;
/// dropping a search future drops all outstanding read futures, without creating
/// detached facade tasks. Sources own any bounded in-progress database work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchConfig {
    /// Authenticated complete scope, unavailable to model query arguments.
    pub scope: SearchScope,
    /// Explicit default legs and weights; graph/semantic are opt-ins.
    pub default_plan: HybridPlan,
    /// Optional graph neighborhood used to seed lexical/semantic query terms.
    /// Literal, regex and explicit graph queries keep their exact meaning.
    #[serde(default)]
    pub graph_expansion: Option<GraphExpansion>,
    /// Resource ceilings applied before every fan-out.
    pub limits: SearchLimits,
    /// Maximum simultaneously awaited source calls, 1..=16.
    pub max_concurrency: usize,
    /// Per-source timeout, 1..=60,000 milliseconds.
    pub source_timeout_ms: u64,
}

/// Model/application query data. Scope is deliberately absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchRequest {
    /// Bounded text/literal/regex/graph label.
    pub text: Arc<str>,
    /// Override all selected legs with this strategy; absent uses the default plan.
    #[serde(default)]
    pub strategy: Option<SearchStrategy>,
    /// Explicit source subset; absent uses configured default-plan sources.
    #[serde(default)]
    pub sources: Option<Vec<Arc<str>>>,
    /// Bounded top-k, default 8 (also capped by configured maximum).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Optional journal restrictions, never new session authority.
    #[serde(default)]
    pub journal: JournalFilter,
}

impl SearchRequest {
    /// Search the configured default plan for text, retaining up to eight hits.
    #[must_use]
    pub fn text(text: impl Into<Arc<str>>) -> Self {
        Self {
            text: text.into(),
            strategy: None,
            sources: None,
            limit: None,
            journal: JournalFilter::default(),
        }
    }
}

#[derive(Clone)]
struct BoundSource {
    source: Arc<dyn SearchSource>,
    descriptor: SearchSourceDescriptor,
}

/// Immutable source registry and direct handles resolved once at construction.
#[derive(Clone)]
pub struct SearchEngine {
    config: SearchConfig,
    sources: BTreeMap<Arc<str>, BoundSource>,
    digest: Digest,
}

impl SearchEngine {
    /// Validate and bind a federation. No store I/O or embedding is performed.
    ///
    /// # Errors
    /// Rejects duplicate/invalid source identities, limits, plans, and any source
    /// that cannot enforce the configured scope before a request begins.
    pub fn try_new(
        config: SearchConfig,
        sources: Vec<Arc<dyn SearchSource>>,
    ) -> Result<Self, SearchError> {
        config.scope.validate()?;
        config.limits.validate()?;
        if config.max_concurrency == 0
            || config.max_concurrency > 16
            || config.source_timeout_ms == 0
            || config.source_timeout_ms > 60_000
            || sources.is_empty()
            || sources.len() > 32
        {
            return Err(SearchError::invalid("federation_limits"));
        }
        let probe = SearchQuery::lexical("configuration", LexicalKind::Keyword);
        config.default_plan.validate(&probe, &config.limits, 1)?;
        let mut bound = BTreeMap::new();
        for source in sources {
            let descriptor = source.descriptor();
            validate_id(&descriptor.source_id)?;
            validate_id(&descriptor.kind)?;
            descriptor.limits.validate()?;
            source.authorize(&config.scope, &probe)?;
            if bound
                .insert(
                    Arc::clone(&descriptor.source_id),
                    BoundSource { source, descriptor },
                )
                .is_some()
            {
                return Err(SearchError::invalid("duplicate_source"));
            }
        }
        for leg in &config.default_plan.legs {
            if !bound.contains_key(&leg.source) {
                return Err(SearchError::invalid("default_source_missing"));
            }
        }
        if let Some(expansion) = &config.graph_expansion {
            expansion.validate(&config.limits)?;
            let source = bound
                .get(&expansion.source)
                .ok_or_else(|| SearchError::invalid("graph_expansion_source"))?;
            if !source.descriptor.graph
                || config
                    .default_plan
                    .legs
                    .iter()
                    .any(|l| l.source == expansion.source)
                || config.default_plan.legs.len() >= config.limits.max_legs
            {
                return Err(SearchError::invalid("graph_expansion_plan"));
            }
            expansion.validate(&source.descriptor.limits)?;
        }
        let descriptors: Vec<_> = bound.values().map(|value| &value.descriptor).collect();
        let digest = configuration_digest("search-engine", &(&config, descriptors))?;
        Ok(Self {
            config,
            sources: bound,
            digest,
        })
    }

    /// Frozen configuration for composition/introspection.
    #[must_use]
    pub const fn config(&self) -> &SearchConfig {
        &self.config
    }

    /// Immutable lock fingerprint including all source descriptors.
    #[must_use]
    pub const fn configuration_digest(&self) -> Digest {
        self.digest
    }

    /// Source descriptors in deterministic namespace order.
    #[must_use]
    pub fn sources(&self) -> Vec<SearchSourceDescriptor> {
        self.sources
            .values()
            .map(|value| value.descriptor.clone())
            .collect()
    }

    /// Search with bounded concurrency and source deadlines. All source scope
    /// and query checks finish before any leg starts. Failures are explicit
    /// outcomes; authorization failures fail the whole request.
    ///
    /// # Errors
    /// Rejects invalid/unauthorized requests and returns all leg outcomes when no
    /// source completed. Source-specific unavailability cannot masquerade as empty success.
    pub async fn search(&self, request: SearchRequest) -> Result<SearchResponse, SearchError> {
        let limit = request
            .limit
            .unwrap_or(8.min(self.config.limits.max_results));
        let query = SearchQuery {
            text: request.text.clone(),
            strategy: request
                .strategy
                .clone()
                .unwrap_or(SearchStrategy::Lexical(LexicalKind::Keyword)),
            journal: request.journal.clone(),
        };
        let plan = self.plan(&request)?;
        let mut legs = plan.validate(&query, &self.config.limits, limit)?;
        for leg in &legs {
            if let Some(bound) = self.sources.get(&leg.source) {
                let mut selected = query.clone();
                selected.strategy = leg.strategy.clone();
                selected.validate(&bound.descriptor.limits, limit)?;
                bound.source.authorize(&self.config.scope, &selected)?;
            }
        }
        let expanded = self.expand(&request, &query, &legs, limit).await?;
        if let Some(expanded) = &expanded {
            legs.retain(|leg| leg.source != expanded.graph.leg.source);
        }
        let queries = legs
            .into_iter()
            .map(|leg| {
                let mut selected = query.clone();
                selected.strategy = leg.strategy.clone();
                if let Some(expanded) = &expanded
                    && crate::expansion::accepts_terms(&selected.strategy)
                {
                    selected.text = expanded.text.clone();
                }
                selected.validate(&self.config.limits, limit)?;
                if let Some(bound) = self.sources.get(&leg.source) {
                    selected.validate(&bound.descriptor.limits, limit)?;
                    bound.source.authorize(&self.config.scope, &selected)?;
                }
                Ok((leg, selected))
            })
            .collect::<Result<Vec<_>, SearchError>>()?;
        let pending = futures_util::stream::iter(
            queries
                .into_iter()
                .map(|(leg, selected)| async move { self.run_leg(leg, selected, limit).await }),
        );
        let mut results = pending
            .buffer_unordered(self.config.max_concurrency)
            .try_collect::<Vec<_>>()
            .await?;
        if let Some(expanded) = expanded {
            for result in &mut results {
                expanded.attach(result);
            }
            results.push(expanded.graph);
        }
        fuse(results, plan.fusion, limit, &self.config.limits)
    }

    async fn expand(
        &self,
        request: &SearchRequest,
        query: &SearchQuery,
        legs: &[HybridLeg],
        limit: usize,
    ) -> Result<Option<ExpandedQuery>, SearchError> {
        let Some(config) = &self.config.graph_expansion else {
            return Ok(None);
        };
        if !query.journal.is_empty()
            || request
                .strategy
                .as_ref()
                .is_some_and(|s| !crate::expansion::accepts_terms(s))
            || request
                .sources
                .as_ref()
                .is_some_and(|ids| !ids.contains(&config.source))
        {
            return Ok(None);
        }
        if legs.len() >= self.config.limits.max_legs
            && !legs.iter().any(|l| l.source == config.source)
        {
            return Err(SearchError::invalid("graph_expansion_leg_limit"));
        }
        let bound = self
            .sources
            .get(&config.source)
            .ok_or(SearchError::SearchUnavailable)?;
        let graph_query = SearchQuery {
            strategy: config.leg().strategy,
            ..query.clone()
        };
        graph_query.validate(&bound.descriptor.limits, config.max_nodes)?;
        bound.source.authorize(&self.config.scope, &graph_query)?;
        let graph = self
            .run_leg(config.leg(), graph_query, config.max_nodes)
            .await?;
        let max_bytes = legs
            .iter()
            .filter(|l| crate::expansion::accepts_terms(&l.strategy))
            .filter_map(|l| self.sources.get(&l.source))
            .map(|b| b.descriptor.limits.max_query_bytes)
            .fold(self.config.limits.max_query_bytes, usize::min);
        Ok(Some(ExpandedQuery::prepare(
            &query.text,
            graph,
            max_bytes,
            limit,
            |text| {
                legs.iter()
                    .filter(|leg| crate::expansion::accepts_terms(&leg.strategy))
                    .all(|leg| {
                        let selected = SearchQuery {
                            text: text.into(),
                            strategy: leg.strategy.clone(),
                            ..query.clone()
                        };
                        self.sources.get(&leg.source).is_none_or(|bound| {
                            selected.validate(&bound.descriptor.limits, limit).is_ok()
                                && bound
                                    .source
                                    .authorize(&self.config.scope, &selected)
                                    .is_ok()
                        })
                    })
            },
        )))
    }

    fn plan(&self, request: &SearchRequest) -> Result<HybridPlan, SearchError> {
        let selected = match &request.sources {
            Some(ids) => {
                if ids.is_empty() || ids.len() > self.config.limits.max_legs {
                    return Err(SearchError::invalid("selected_sources"));
                }
                let mut unique = BTreeSet::new();
                for id in ids {
                    validate_id(id)?;
                    if !unique.insert(Arc::clone(id)) {
                        return Err(SearchError::invalid("duplicate_selected_source"));
                    }
                }
                unique
            }
            None => self
                .config
                .default_plan
                .legs
                .iter()
                .map(|leg| Arc::clone(&leg.source))
                .collect(),
        };
        let mut legs = Vec::new();
        for source in selected {
            if let Some(strategy) = &request.strategy {
                let weight = self
                    .config
                    .default_plan
                    .legs
                    .iter()
                    .find(|leg| leg.source == source)
                    .map_or(1_000_000, |leg| leg.weight_micros);
                legs.push(HybridLeg {
                    source,
                    strategy: strategy.clone(),
                    weight_micros: weight,
                });
            } else {
                let configured: Vec<_> = self
                    .config
                    .default_plan
                    .legs
                    .iter()
                    .filter(|leg| leg.source == source)
                    .cloned()
                    .collect();
                if configured.is_empty() {
                    legs.push(HybridLeg {
                        source,
                        strategy: SearchStrategy::Lexical(LexicalKind::Keyword),
                        weight_micros: 1_000_000,
                    });
                } else {
                    legs.extend(configured);
                }
            }
        }
        Ok(HybridPlan {
            legs,
            fusion: self.config.default_plan.fusion,
        })
    }

    async fn run_leg(
        &self,
        leg: HybridLeg,
        query: SearchQuery,
        limit: usize,
    ) -> Result<LegResult, SearchError> {
        let Some(bound) = self.sources.get(&leg.source) else {
            return Ok(failed_leg(leg, SourceStatus::Unavailable, "source_missing"));
        };
        if !bound.descriptor.supports(&leg.strategy) {
            return Ok(failed_leg(
                leg,
                SourceStatus::Unsupported,
                "strategy_unsupported",
            ));
        }
        let call = bound
            .source
            .search(self.config.scope.clone(), query.clone(), limit);
        let result =
            match tokio::time::timeout(Duration::from_millis(self.config.source_timeout_ms), call)
                .await
            {
                Ok(Ok(result)) => result,
                Ok(Err(SearchError::SearchScopeDenied)) => {
                    return Err(SearchError::SearchScopeDenied);
                }
                Ok(Err(SearchError::SearchUnsupported)) => {
                    return Ok(failed_leg(
                        leg,
                        SourceStatus::Unsupported,
                        "query_unsupported",
                    ));
                }
                Ok(Err(error)) => {
                    return Ok(failed_leg(leg, SourceStatus::Unavailable, error.code()));
                }
                Err(_) => return Ok(failed_leg(leg, SourceStatus::Unavailable, "source_timeout")),
            };
        for hit in &result.hits {
            bound.source.authorize(&hit.provenance.scope, &query)?;
            if hit.provenance.scope.tenant != self.config.scope.tenant {
                return Err(SearchError::SearchScopeDenied);
            }
        }
        let leg = LegResult { leg, result };
        leg.validate(limit, &bound.descriptor.limits)?;
        leg.validate(limit, &self.config.limits)?;
        Ok(leg)
    }
}

fn failed_leg(leg: HybridLeg, status: SourceStatus, reason: &'static str) -> LegResult {
    LegResult {
        leg,
        result: SourceResult {
            hits: Vec::new(),
            status,
            examined: 0,
            historical_complete: false,
            reasons: vec![Arc::from(reason)],
        },
    }
}
