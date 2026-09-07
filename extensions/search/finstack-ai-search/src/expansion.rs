use finstack_ai_kernel::Sensitivity;
use finstack_ai_search_core::{
    GraphQuery, HybridLeg, LegResult, LexicalKind, SearchCitation, SearchError, SearchLimits,
    SearchQuery, SearchStrategy, highest_sensitivity, validate_id,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Explicit graph expansion stage. Verified neighboring entity labels seed
/// keyword/BM25 and semantic legs. The graph read and its coverage are returned
/// alongside ordinary legs; graph-derived terms never modify regex/literal data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphExpansion {
    /// Registered graph source namespace.
    pub source: Arc<str>,
    /// Maximum neighborhood edge distance, default two.
    pub depth: u8,
    /// Maximum visited entities and retained expansion labels, default 100.
    pub max_nodes: usize,
    /// Maximum relationships examined, default 1,000.
    pub max_edges: usize,
    /// Weight of graph evidence when fusing graph matches with ordinary results.
    pub weight_micros: u32,
}
impl GraphExpansion {
    /// Opt-in expansion with two hops, 100 entities and 1,000 examined edges.
    #[must_use]
    pub fn new(source: impl Into<Arc<str>>) -> Self {
        Self {
            source: source.into(),
            depth: 2,
            max_nodes: 100,
            max_edges: 1000,
            weight_micros: 1_000_000,
        }
    }
    pub(crate) fn leg(&self) -> HybridLeg {
        HybridLeg {
            source: self.source.clone(),
            strategy: SearchStrategy::Graph(GraphQuery::Neighborhood {
                depth: self.depth,
                max_nodes: self.max_nodes,
                max_edges: self.max_edges,
            }),
            weight_micros: self.weight_micros,
        }
    }
    pub(crate) fn validate(&self, limits: &SearchLimits) -> Result<(), SearchError> {
        validate_id(&self.source)?;
        if self.weight_micros == 0 || self.weight_micros > 1_000_000 {
            return Err(SearchError::invalid("graph_expansion_weight"));
        }
        SearchQuery {
            strategy: self.leg().strategy,
            ..SearchQuery::lexical("configuration", LexicalKind::Keyword)
        }
        .validate(limits, self.max_nodes)
    }
}

pub(crate) fn accepts_terms(strategy: &SearchStrategy) -> bool {
    matches!(
        strategy,
        SearchStrategy::Lexical(LexicalKind::Keyword | LexicalKind::Bm25)
            | SearchStrategy::Semantic { .. }
    )
}

pub(crate) struct ExpandedQuery {
    pub(crate) text: Arc<str>,
    pub(crate) graph: LegResult,
    citations: Vec<SearchCitation>,
    sensitivity: Option<Sensitivity>,
}
impl ExpandedQuery {
    pub(crate) fn prepare(
        text: &str,
        mut graph: LegResult,
        max_bytes: usize,
        result_limit: usize,
        accepts: impl Fn(&str) -> bool,
    ) -> Self {
        let mut expanded = text.to_owned();
        let mut citations = Vec::new();
        let mut sensitivity = None;
        let mut labels = std::collections::BTreeSet::new();
        for hit in &graph.result.hits {
            let Some(label) = &hit.entity_label else {
                graph
                    .result
                    .reasons
                    .push("graph_expansion_label_missing".into());
                continue;
            };
            if labels.contains(label) {
                continue;
            }
            let mut combined = citations.clone();
            for citation in &hit.provenance.citations {
                if !combined.contains(citation) {
                    combined.push(citation.clone());
                }
            }
            if combined.is_empty()
                || combined.len() > 16
                || expanded.len() + 1 + label.len() > max_bytes
                || !accepts(&format!("{expanded} {label}"))
            {
                graph
                    .result
                    .reasons
                    .push("graph_expansion_term_or_evidence_limit".into());
                continue;
            }
            labels.insert(label.clone());
            citations = combined;
            sensitivity = Some(
                sensitivity.map_or(hit.sensitivity, |s| highest_sensitivity(s, hit.sensitivity)),
            );
            expanded.push(' ');
            expanded.push_str(label);
        }
        if graph
            .result
            .reasons
            .iter()
            .any(|r| r.starts_with("graph_expansion_"))
        {
            graph.result.truncate("graph_expansion_partial");
        }
        graph.result.reasons.sort();
        graph.result.reasons.dedup();
        if graph.result.hits.len() > result_limit {
            graph.result.hits.truncate(result_limit);
            graph.result.truncate("graph_expansion_result_limit");
        }
        Self {
            text: expanded.into(),
            graph,
            citations,
            sensitivity,
        }
    }
    pub(crate) fn attach(&self, result: &mut LegResult) {
        if !accepts_terms(&result.leg.strategy) || self.citations.is_empty() {
            return;
        }
        let mut partial = false;
        result.result.hits.retain_mut(|hit| {
            let mut citations = hit.provenance.citations.clone();
            for citation in &self.citations {
                if !citations.contains(citation) {
                    citations.push(citation.clone());
                }
            }
            if citations.len() > 16 || hit.provenance.locators.len() >= 16 {
                partial = true;
                return false;
            }
            hit.provenance.citations = citations;
            hit.provenance
                .locators
                .push(format!("graph_query_expansion:{}", self.graph.leg.source).into());
            if let Some(sensitivity) = self.sensitivity {
                hit.sensitivity = highest_sensitivity(hit.sensitivity, sensitivity);
            }
            true
        });
        if partial {
            result.result.truncate("graph_expansion_provenance_limit");
        }
    }
}
