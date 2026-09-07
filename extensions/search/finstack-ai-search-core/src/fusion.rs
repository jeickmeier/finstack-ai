use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::{
    FusedEvidence, FusedHit, SearchError, SearchLimits, SearchQuery, SearchResponse,
    SearchStrategy, SourceOutcome, SourceResult, SourceStatus, highest_sensitivity, validate_id,
};

const SCALE: u64 = 1_000_000_000;

/// How independently ranked legs contribute to final relevance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Fusion {
    /// Weighted reciprocal-rank fusion, weight / (k + one-based rank).
    Rrf {
        /// Rank smoothing; default 60, accepted range 1..=10,000.
        k: u32,
    },
    /// Normalize each leg's score range to `[0, 1]`, then add weighted values.
    /// A constant-score leg gives every distinct hit 1. Raw BM25 and cosine
    /// values are never combined directly.
    WeightedSum,
}

impl Default for Fusion {
    fn default() -> Self {
        Self::Rrf { k: 60 }
    }
}

/// One explicitly weighted source/strategy leg.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HybridLeg {
    /// Source namespace.
    pub source: Arc<str>,
    /// Retrieval strategy.
    pub strategy: SearchStrategy,
    /// Weight in millionths, 1..=1,000,000. Only relative weights matter.
    pub weight_micros: u32,
}

/// Bounded explicit search plan; no semantic/graph leg appears implicitly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HybridPlan {
    /// Unique source/strategy pairs. Input order does not affect results.
    pub legs: Vec<HybridLeg>,
    /// Deterministic fusion rule.
    #[serde(default)]
    pub fusion: Fusion,
}

impl HybridPlan {
    /// Validate all leg queries before fan-out and return canonical leg order.
    ///
    /// # Errors
    /// Rejects duplicate legs, bad weights/fusion, excessive legs, and any invalid query.
    pub fn validate(
        &self,
        query: &SearchQuery,
        limits: &SearchLimits,
        limit: usize,
    ) -> Result<Vec<HybridLeg>, SearchError> {
        query.validate(limits, limit)?;
        if self.legs.is_empty()
            || self.legs.len() > limits.max_legs
            || matches!(self.fusion, Fusion::Rrf { k } if k == 0 || k > 10_000)
        {
            return Err(SearchError::invalid("hybrid_plan"));
        }
        let mut seen = BTreeMap::new();
        for leg in &self.legs {
            validate_id(&leg.source)?;
            if leg.weight_micros == 0 || leg.weight_micros > 1_000_000 {
                return Err(SearchError::invalid("leg_weight"));
            }
            let mut leg_query = query.clone();
            leg_query.strategy = leg.strategy.clone();
            leg_query.validate(limits, limit)?;
            if seen.insert(leg_key(leg)?, leg.clone()).is_some() {
                return Err(SearchError::invalid("duplicate_leg"));
            }
        }
        Ok(seen.into_values().collect())
    }
}

/// One attempted leg, including unsupported/unavailable outcomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegResult {
    /// Exact requested leg and weight.
    pub leg: HybridLeg,
    /// Bounded source result with coverage.
    pub result: SourceResult,
}

impl LegResult {
    /// Validate source namespaces, bounded hits, evidence and coverage before
    /// using the result for fusion or query expansion.
    ///
    /// # Errors
    /// Rejects malformed, oversized, or internally inconsistent source results.
    pub fn validate(&self, limit: usize, limits: &SearchLimits) -> Result<(), SearchError> {
        limits.validate()?;
        if limit == 0 || limit > limits.max_results {
            return Err(SearchError::invalid("source_result_limit"));
        }
        validate_result(self, limit, limits)
    }
}

fn leg_key(leg: &HybridLeg) -> Result<(Arc<str>, String), SearchError> {
    let strategy = serde_json_canonicalizer::to_string(&leg.strategy)
        .map_err(|_| SearchError::invalid("strategy_encoding"))?;
    Ok((Arc::clone(&leg.source), strategy))
}

/// Fuse bounded, ranked source results. Deduplication uses source namespace plus
/// typed reference; ties use that same canonical key. Duplicates within one leg
/// contribute once and preserve all their provenance and highest sensitivity.
///
/// The result is independent of leg order and hit order. Within a source,
/// relevance is descending `score`, then canonical reference, then preview.
///
/// # Errors
/// Rejects invalid results/plans or reports all outcomes when no leg succeeded.
pub fn fuse(
    mut legs: Vec<LegResult>,
    fusion: Fusion,
    limit: usize,
    limits: &SearchLimits,
) -> Result<SearchResponse, SearchError> {
    limits.validate()?;
    if limit == 0
        || limit > limits.max_results
        || legs.is_empty()
        || legs.len() > limits.max_legs
        || matches!(fusion, Fusion::Rrf { k } if k == 0 || k > 10_000)
    {
        return Err(SearchError::invalid("fusion_bounds"));
    }
    let mut keys = BTreeSet::new();
    for leg in &legs {
        validate_id(&leg.leg.source)?;
        if !keys.insert(leg_key(&leg.leg)?)
            || leg.leg.weight_micros == 0
            || leg.leg.weight_micros > 1_000_000
        {
            return Err(SearchError::invalid("fusion_leg"));
        }
    }
    legs.sort_by_cached_key(|leg| leg_key(&leg.leg).unwrap_or_default());
    let mut outcomes = Vec::with_capacity(legs.len());
    let mut merged: BTreeMap<(Arc<str>, String), FusedHit> = BTreeMap::new();
    for leg in legs {
        validate_result(&leg, limit, limits)?;
        outcomes.push(SourceOutcome {
            source: Arc::clone(&leg.leg.source),
            strategy: leg.leg.strategy.clone(),
            status: leg.result.status,
            examined: leg.result.examined,
            historical_complete: leg.result.historical_complete,
            reasons: leg.result.reasons.clone(),
        });
        add_leg(&mut merged, leg, fusion)?;
    }
    if !outcomes.iter().any(|outcome| outcome.status.successful()) {
        return Err(SearchError::SearchNoSuccessfulSources { outcomes });
    }
    let mut matches: Vec<_> = merged.into_iter().collect();
    matches.sort_by(|(ka, a), (kb, b)| b.score.cmp(&a.score).then_with(|| ka.cmp(kb)));
    let results_truncated = matches.len() > limit;
    matches.truncate(limit);
    Ok(SearchResponse {
        hits: matches.into_iter().map(|(_, hit)| hit).collect(),
        outcomes,
        results_truncated,
    })
}

fn validate_result(
    leg: &LegResult,
    limit: usize,
    limits: &SearchLimits,
) -> Result<(), SearchError> {
    let result = &leg.result;
    if result.hits.len() > limit
        || result.examined > limits.max_scan_records as u64
        || result.reasons.len() > 16
        || (!result.status.successful() && !result.hits.is_empty())
        || (result.status == SourceStatus::Completed && !result.historical_complete)
    {
        return Err(SearchError::invalid("source_result_bounds"));
    }
    for reason in &result.reasons {
        validate_id(reason)?;
    }
    for hit in &result.hits {
        hit.validate(limits)?;
        if hit.source != leg.leg.source {
            return Err(SearchError::invalid("source_namespace"));
        }
    }
    Ok(())
}

fn add_leg(
    merged: &mut BTreeMap<(Arc<str>, String), FusedHit>,
    leg: LegResult,
    fusion: Fusion,
) -> Result<(), SearchError> {
    // Group repeated references without letting duplicates inflate rank or score.
    // Every duplicate's provenance survives in the evidence list, with zero
    // additional contribution. This also preserves higher sensitivity labels.
    let mut distinct: BTreeMap<String, Vec<_>> = BTreeMap::new();
    for hit in leg.result.hits {
        distinct.entry(hit.reference.key()?).or_default().push(hit);
    }
    let mut ranked = Vec::with_capacity(distinct.len());
    for (key, hits) in distinct {
        let mut ordered = hits
            .into_iter()
            .map(|hit| {
                serde_json_canonicalizer::to_string(&hit)
                    .map(|canonical| (canonical, hit))
                    .map_err(|_| SearchError::invalid("hit_encoding"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        ordered.sort_by(|(ka, a), (kb, b)| b.score.cmp(&a.score).then_with(|| ka.cmp(kb)));
        ranked.push((
            key,
            ordered.into_iter().map(|(_, hit)| hit).collect::<Vec<_>>(),
        ));
    }
    ranked.sort_by(|(ka, a), (kb, b)| {
        b.first()
            .map(|h| h.score)
            .cmp(&a.first().map(|h| h.score))
            .then_with(|| ka.cmp(kb))
    });
    let min = ranked
        .iter()
        .filter_map(|(_, hits)| hits.first().map(|hit| hit.score))
        .min()
        .unwrap_or(0);
    let max = ranked
        .iter()
        .filter_map(|(_, hits)| hits.first().map(|hit| hit.score))
        .max()
        .unwrap_or(0);
    for (index, (key, hits)) in ranked.into_iter().enumerate() {
        let rank = u32::try_from(index + 1).map_err(|_| SearchError::invalid("rank"))?;
        let mut counted = false;
        for hit in hits {
            let contribution = if counted {
                0
            } else {
                contribution(fusion, rank, hit.score, min, max, leg.leg.weight_micros)
            };
            counted = true;
            let fused = merged
                .entry((Arc::clone(&hit.source), key.clone()))
                .or_insert_with(|| FusedHit {
                    source: Arc::clone(&hit.source),
                    reference: hit.reference.clone(),
                    score: 0,
                    entity_label: hit.entity_label.clone(),
                    preview: Arc::clone(&hit.preview),
                    sensitivity: hit.sensitivity,
                    evidence: Vec::new(),
                });
            // A typed reference must never unite evidence from different scopes.
            if fused
                .evidence
                .first()
                .is_some_and(|e| e.provenance.scope != hit.provenance.scope)
            {
                return Err(SearchError::SearchScopeDenied);
            }
            if fused.entity_label != hit.entity_label {
                return Err(SearchError::invalid("conflicting_entity_label"));
            }
            fused.score += contribution;
            fused.sensitivity = highest_sensitivity(fused.sensitivity, hit.sensitivity);
            if hit.preview < fused.preview {
                fused.preview = hit.preview;
            }
            fused.evidence.push(FusedEvidence {
                strategy: leg.leg.strategy.clone(),
                rank,
                source_score: hit.score,
                weight_micros: leg.leg.weight_micros,
                contribution,
                provenance: hit.provenance,
                sensitivity: hit.sensitivity,
            });
        }
    }
    Ok(())
}

fn contribution(fusion: Fusion, rank: u32, score: u32, min: u32, max: u32, weight: u32) -> u64 {
    let normalized = match fusion {
        Fusion::Rrf { k } => SCALE / (u64::from(k) + u64::from(rank)),
        Fusion::WeightedSum if min == max => SCALE,
        Fusion::WeightedSum => SCALE * u64::from(score - min) / u64::from(max - min),
    };
    normalized * u64::from(weight) / 1_000_000
}
