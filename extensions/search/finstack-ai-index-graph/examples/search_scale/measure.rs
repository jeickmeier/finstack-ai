use super::Error;
use finstack_ai_search::{SearchEngine, SearchRequest};
use finstack_ai_search_core::{
    SearchQuery, SearchScope, SearchSource, SearchStrategy, SourceStatus,
};
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path, time::Instant};

fn report(
    mut times: Vec<f64>,
    hits: usize,
    coverage: &BTreeMap<String, usize>,
    examined: u64,
) -> Value {
    times.sort_by(f64::total_cmp);
    let n = times.len();
    json!({"samples":n,"p50_ms":times[(n-1)/2],"p95_ms":times[(n*95).div_ceil(100).saturating_sub(1)],"p99_ms":times[(n*99).div_ceil(100).saturating_sub(1)],"min_ms":times[0],"max_ms":times[n-1],"mean_ms":times.iter().sum::<f64>() / f64::from(u32::try_from(n).unwrap_or(u32::MAX)),"returned_hits":hits,"source_outcomes":coverage,"max_examined":examined,"latencies_ms":times})
}

pub async fn source(
    source: &dyn SearchSource,
    scope: &SearchScope,
    strategy: SearchStrategy,
    samples: usize,
    memories: usize,
) -> Result<Value, Error> {
    let mut times = Vec::new();
    let mut hits = 0;
    let mut coverage = BTreeMap::new();
    let mut examined = 0;
    for i in 0..samples + 3 {
        let text = if matches!(strategy, SearchStrategy::Graph(_)) {
            format!("Company{:05}", i * 37 % memories)
        } else {
            format!("sector{:03}", i * 37 % 100)
        };
        let query = SearchQuery {
            strategy: strategy.clone(),
            ..SearchQuery::lexical(text, finstack_ai_search_core::LexicalKind::Bm25)
        };
        let start = Instant::now();
        let result = source.search(scope.clone(), query, 8).await?;
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        if result.hits.is_empty()
            || matches!(
                result.status,
                SourceStatus::Unavailable | SourceStatus::Unsupported
            )
        {
            return Err(format!(
                "benchmark source {} failed: {:?}",
                source.descriptor().source_id,
                result.status
            )
            .into());
        }
        if result.hits.len() > 8
            || result
                .hits
                .iter()
                .any(|hit| hit.preview.chars().count() > 512 || hit.provenance.scope != *scope)
        {
            return Err("unbounded or incorrectly scoped benchmark result".into());
        }
        if i >= 3 {
            times.push(ms);
            hits += result.hits.len();
            examined = examined.max(result.examined);
            *coverage
                .entry(format!("{:?}", result.status).to_lowercase())
                .or_insert(0) += 1;
        }
    }
    Ok(report(times, hits, &coverage, examined))
}

pub async fn hybrid(engine: &SearchEngine, samples: usize) -> Result<Value, Error> {
    let mut times = Vec::new();
    let mut hits = 0;
    let mut coverage = BTreeMap::new();
    let mut examined = 0;
    for i in 0..samples + 3 {
        let start = Instant::now();
        let result = engine
            .search(SearchRequest {
                limit: Some(8),
                ..SearchRequest::text(format!("sector{:03}", i * 37 % 100))
            })
            .await?;
        let ms = start.elapsed().as_secs_f64() * 1000.0;
        if result.hits.is_empty()
            || result.outcomes.iter().any(|o| {
                matches!(
                    o.status,
                    SourceStatus::Unavailable | SourceStatus::Unsupported
                )
            })
        {
            return Err("hybrid benchmark lost a source".into());
        }
        if i >= 3 {
            times.push(ms);
            hits += result.hits.len();
            for outcome in result.outcomes {
                examined = examined.max(outcome.examined);
                *coverage
                    .entry(format!("{}:{:?}", outcome.source, outcome.status).to_lowercase())
                    .or_insert(0) += 1;
            }
        }
    }
    Ok(report(times, hits, &coverage, examined))
}

fn bytes(path: &Path) -> std::io::Result<u64> {
    if path.is_file() {
        return Ok(path.metadata()?.len());
    }
    let mut total = 0;
    for entry in path.read_dir()? {
        total += bytes(&entry?.path())?;
    }
    Ok(total)
}
pub fn sizes(root: &Path) -> std::io::Result<BTreeMap<String, u64>> {
    let mut sizes = BTreeMap::new();
    for entry in root.read_dir()? {
        let path = entry?.path();
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            sizes.insert(name.to_owned(), bytes(&path)?);
        }
    }
    Ok(sizes)
}
