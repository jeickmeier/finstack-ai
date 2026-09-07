use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::Arc;

use finstack_ai_kernel::Sensitivity;
use finstack_ai_search_core::{
    GraphQuery, SearchCitation, SearchError, SearchEvidence, SearchHit, SearchProvenance,
    SearchQuery, SearchStrategy, SourceRef, SourceResult, SourceStatus, configuration_digest,
    highest_sensitivity, preview,
};
use rusqlite::{OptionalExtension, params};

use crate::{
    GraphSearchSource,
    indexing::{StoredSource, db_error, decode_source, parse_source},
    vocabulary::normalize,
};

#[derive(Clone)]
struct Node {
    id: String,
    kind: String,
    label: String,
}
struct Edge {
    id: String,
    source: String,
    target: String,
    kind: String,
}
#[derive(Clone, Default)]
struct Proof {
    citations: Vec<SearchCitation>,
    sensitivity: Option<Sensitivity>,
    locators: Vec<Arc<str>>,
}
impl Proof {
    fn merge(&mut self, other: &Self) -> bool {
        // Every path edge must keep at least one supporting citation. Never
        // return a plausible-looking path after dropping its required proof.
        for citation in &other.citations {
            if !self.citations.contains(citation) {
                self.citations.push(citation.clone());
            }
        }
        self.sensitivity = match (self.sensitivity, other.sensitivity) {
            (Some(a), Some(b)) => Some(highest_sensitivity(a, b)),
            (a, b) => a.or(b),
        };
        for locator in &other.locators {
            if !self.locators.contains(locator) {
                self.locators.push(locator.clone());
            }
        }
        self.citations.len() <= 16 && self.locators.len() <= 16
    }
}

struct ReadState<'a> {
    source: &'a GraphSearchSource,
    cache: BTreeMap<String, Option<SearchEvidence>>,
    result: SourceResult,
    verified: usize,
    unavailable: usize,
}
impl ReadState<'_> {
    fn budget(&self) -> usize {
        self.source
            .config
            .limits
            .max_scan_records
            .saturating_sub(usize::try_from(self.result.examined).unwrap_or(usize::MAX))
    }
    fn counted(&mut self, count: usize) {
        self.result.examined = self.result.examined.saturating_add(count as u64);
    }
    async fn verify(
        &mut self,
        stored: StoredSource,
    ) -> Result<Option<SearchEvidence>, SearchError> {
        if let Some(cached) = self.cache.get(&stored.key) {
            return Ok(cached.clone());
        }
        if self.cache.len() >= self.source.config.max_evidence_reads {
            self.result.truncate("graph_evidence_read_limit");
            return Ok(None);
        }
        let fresh = match self
            .source
            .evidence(&stored.source, stored.reference.clone())
            .await
        {
            Ok(Some(evidence))
                if stored.extraction == self.source.extraction_digest.to_hex()
                    && evidence.hit.provenance.content_digest
                        == stored.hit.provenance.content_digest =>
            {
                self.verified += 1;
                if !evidence.complete {
                    self.result.historical_complete = false;
                    self.result.truncate("graph_incomplete_source_text");
                }
                Some(evidence)
            }
            Ok(_) => {
                self.source
                    .remove_source(stored.key.clone(), Some(stored.hit))
                    .await?;
                self.result.truncate("graph_stale_evidence_removed");
                None
            }
            Err(SearchError::SearchScopeDenied) => return Err(SearchError::SearchScopeDenied),
            Err(_) => {
                self.unavailable += 1;
                self.result.truncate("graph_source_unavailable");
                None
            }
        };
        self.cache.insert(stored.key, fresh.clone());
        Ok(fresh)
    }
    async fn support(
        &mut self,
        id: &str,
        edge: bool,
        alias: Option<&str>,
    ) -> Result<Option<Proof>, SearchError> {
        let take = self.budget().min(self.source.config.max_support_per_item);
        if take == 0 {
            self.result.truncate("graph_scan_limit");
            return Ok(None);
        }
        let id = id.to_owned();
        let scope = self.source.scope_digest.clone();
        let alias = alias.map(str::to_owned);
        let rows = self.source.database.call(move |c| {
            let sql = if edge {
                "SELECT s.source_key,s.source_id,s.reference,s.hit_json,s.extraction_digest FROM sources s JOIN edge_sources x ON x.source_key=s.source_key WHERE s.scope=?1 AND x.edge=?2 ORDER BY s.source_key LIMIT ?3"
            } else if alias.is_some() {
                "SELECT s.source_key,s.source_id,s.reference,s.hit_json,s.extraction_digest FROM sources s JOIN aliases x ON x.source_key=s.source_key WHERE s.scope=?1 AND x.entity=?2 AND x.alias=?4 ORDER BY s.source_key LIMIT ?3"
            } else {
                "SELECT s.source_key,s.source_id,s.reference,s.hit_json,s.extraction_digest FROM sources s JOIN entity_sources x ON x.source_key=s.source_key WHERE s.scope=?1 AND x.entity=?2 ORDER BY s.source_key LIMIT ?3"
            };
            let mut statement = c.prepare(sql).map_err(db_error)?;
            let mut rows = if let Some(alias) = alias { statement.query(params![scope,id,take,alias]) } else { statement.query(params![scope,id,take]) }.map_err(db_error)?;
            let mut found = Vec::new(); while let Some(row) = rows.next().map_err(db_error)? { found.push(parse_source(decode_source(row).map_err(db_error)?)?); } Ok(found)
        }).await?;
        let more = rows.len() == take;
        self.counted(rows.len());
        if more {
            self.result.truncate("graph_support_or_scan_limit");
        }
        let mut proof = Proof::default();
        for row in rows.into_iter().take(take) {
            if let Some(evidence) = self.verify(row).await? {
                proof.sensitivity = Some(proof.sensitivity.map_or(evidence.hit.sensitivity, |s| {
                    highest_sensitivity(s, evidence.hit.sensitivity)
                }));
                let citation = SearchCitation {
                    source: evidence.hit.source,
                    reference: evidence.hit.reference,
                    scope: evidence.hit.provenance.scope,
                    content_digest: evidence.hit.provenance.content_digest,
                };
                if !proof.citations.contains(&citation) {
                    if proof.citations.len() < 16 {
                        proof.citations.push(citation);
                    } else {
                        self.result.truncate("graph_citation_limit");
                    }
                }
            }
        }
        Ok((!proof.citations.is_empty()).then_some(proof))
    }
    fn hit(
        &mut self,
        node: &Node,
        proof: &Proof,
        distance: u8,
        limit: usize,
    ) -> Result<(), SearchError> {
        if self.result.hits.len() >= limit {
            self.result.truncate("graph_result_limit");
            return Ok(());
        }
        let hit = SearchHit {
            entity_label: Some(node.label.clone().into()),
            source: self.source.config.source_id.clone(),
            reference: SourceRef::Entity {
                id: node.id.clone().into(),
            },
            score: u32::MAX - u32::from(distance),
            preview: preview(
                &format!("{}: {}", node.kind, node.label),
                self.source.config.limits.max_preview_chars,
            ),
            sensitivity: proof.sensitivity.unwrap_or(Sensitivity::Public),
            provenance: SearchProvenance {
                scope: self.source.config.scope.clone(),
                content_digest: configuration_digest(
                    "graph-hit",
                    &(&node.id, &proof.citations, &proof.locators),
                )?,
                locators: proof.locators.clone(),
                citations: proof.citations.clone(),
            },
        };
        hit.validate(&self.source.config.limits)?;
        self.result.hits.push(hit);
        Ok(())
    }
    async fn seeds(
        &mut self,
        text: &str,
        mentions: bool,
        max_nodes: usize,
    ) -> Result<Vec<(Node, Proof)>, SearchError> {
        let text = if mentions {
            text.to_lowercase()
        } else {
            normalize(text)?
        };
        let mut found = Vec::new();
        let mut seen = BTreeSet::new();
        let mut cursor = (String::new(), String::new());
        loop {
            let take = self.budget().min(64);
            if take == 0 {
                self.result.truncate("graph_scan_limit");
                break;
            }
            let scope = self.source.scope_digest.clone();
            let search = text.clone();
            let after = cursor.clone();
            let rows = self.source.database.call(move |c| {
                let sql = if mentions {
                    "SELECT DISTINCT e.id,e.kind,e.label,a.alias FROM entities e JOIN aliases a ON a.entity=e.id WHERE e.scope=?1 AND e.id>=?4 AND (e.id,a.alias)>(?4,?5) ORDER BY e.id,a.alias LIMIT ?2"
                } else {
                    "SELECT DISTINCT e.id,e.kind,e.label,a.alias FROM entities e JOIN aliases a ON a.entity=e.id WHERE e.scope=?1 AND a.alias=?3 AND e.id>=?4 AND (e.id,a.alias)>(?4,?5) ORDER BY e.id,a.alias LIMIT ?2"
                };
                let mut statement = c.prepare(sql).map_err(db_error)?;
                let mut rows = statement.query(params![scope,take,search,after.0,after.1]).map_err(db_error)?;
                let mut result = Vec::new();
                while let Some(row)=rows.next().map_err(db_error)? {
                    result.push((Node{id:row.get(0).map_err(db_error)?,kind:row.get(1).map_err(db_error)?,label:row.get(2).map_err(db_error)?},row.get::<_,String>(3).map_err(db_error)?));
                }
                Ok(result)
            }).await?;
            self.counted(rows.len());
            let exhausted = rows.len() < take;
            for (node, alias) in rows {
                cursor = (node.id.clone(), alias.clone());
                if (mentions && !contains_label(&text, &alias)) || seen.contains(&node.id) {
                    continue;
                }
                if found.len() >= max_nodes {
                    self.result.truncate("graph_node_limit");
                    return Ok(found);
                }
                if let Some(proof) = self.support(&node.id, false, Some(&alias)).await? {
                    seen.insert(node.id.clone());
                    found.push((node, proof));
                }
            }
            if exhausted {
                break;
            }
        }
        Ok(found)
    }
    async fn node(&mut self, id: String) -> Result<Option<Node>, SearchError> {
        if self.budget() == 0 {
            self.result.truncate("graph_scan_limit");
            return Ok(None);
        }
        self.counted(1);
        let scope = self.source.scope_digest.clone();
        self.source
            .database
            .call(move |c| {
                c.query_row(
                    "SELECT id,kind,label FROM entities WHERE scope=? AND id=?",
                    params![scope, id],
                    |r| {
                        Ok(Node {
                            id: r.get(0)?,
                            kind: r.get(1)?,
                            label: r.get(2)?,
                        })
                    },
                )
                .optional()
                .map_err(db_error)
            })
            .await
    }
    async fn edges(
        &mut self,
        id: &str,
        directed: bool,
        remaining: usize,
    ) -> Result<Vec<Edge>, SearchError> {
        let take = remaining.min(self.budget());
        if take == 0 {
            self.result.truncate("graph_edge_or_scan_limit");
            return Ok(Vec::new());
        }
        let scope = self.source.scope_digest.clone();
        let id = id.to_owned();
        let edges=self.source.database.call(move |c| {
            let sql=if directed {"SELECT id,src,dst,kind FROM edges WHERE scope=?1 AND src=?2 ORDER BY id LIMIT ?3"} else {"SELECT id,src,dst,kind FROM edges WHERE scope=?1 AND (src=?2 OR dst=?2) ORDER BY id LIMIT ?3"};
            let mut statement=c.prepare(sql).map_err(db_error)?;
            let rows=statement.query_map(params![scope,id,take],|r|Ok(Edge{id:r.get(0)?,source:r.get(1)?,target:r.get(2)?,kind:r.get(3)?})).map_err(db_error)?;
            rows.collect::<Result<Vec<_>,_>>().map_err(db_error)
        }).await?;
        if edges.len() == take {
            self.result.truncate("graph_edge_or_scan_limit");
        }
        self.counted(edges.len());
        Ok(edges)
    }
}

impl GraphSearchSource {
    #[allow(clippy::too_many_lines)] // One bounded breadth-first traversal owns its proof and cycle state.
    pub(crate) async fn retrieve(
        &self,
        query: SearchQuery,
        limit: usize,
    ) -> Result<SourceResult, SearchError> {
        let SearchStrategy::Graph(graph) = query.strategy else {
            return Err(SearchError::SearchUnsupported);
        };
        let (depth, max_nodes, max_edges, target, mentions) = match graph {
            GraphQuery::Entity => (0, self.config.limits.max_graph_nodes, 0, None, false),
            GraphQuery::Neighborhood {
                depth,
                max_nodes,
                max_edges,
            } => (depth, max_nodes, max_edges, None, true),
            GraphQuery::Path {
                target,
                depth,
                max_nodes,
                max_edges,
            } => (depth, max_nodes, max_edges, Some(target), false),
        };
        let mut state = ReadState {
            source: self,
            cache: BTreeMap::new(),
            result: SourceResult::completed(Vec::new(), 0),
            verified: 0,
            unavailable: 0,
        };
        let targets = if let Some(target) = &target {
            state
                .seeds(target, false, max_nodes)
                .await?
                .into_iter()
                .map(|(n, p)| (n.id, p))
                .collect::<BTreeMap<_, _>>()
        } else {
            BTreeMap::new()
        };
        let seeds = state.seeds(&query.text, mentions, max_nodes).await?;
        let mut queue = VecDeque::new();
        let mut visited = BTreeSet::new();
        for (node, proof) in seeds {
            visited.insert(node.id.clone());
            queue.push_back((node, proof, 0_u8));
        }
        let mut examined_edges = 0;
        while let Some((node, proof, distance)) = queue.pop_front() {
            if target.is_none() {
                state.hit(&node, &proof, distance, limit)?;
            } else if let Some(target_proof) = targets.get(&node.id) {
                let mut proof = proof;
                if proof.merge(target_proof) {
                    state.hit(&node, &proof, distance, limit)?;
                } else {
                    state.result.truncate("graph_path_proof_limit");
                }
                break;
            }
            if distance >= depth {
                continue;
            }
            let edges = state
                .edges(
                    &node.id,
                    target.is_some(),
                    max_edges.saturating_sub(examined_edges),
                )
                .await?;
            examined_edges += edges.len();
            for edge in edges {
                let next = if edge.source == node.id {
                    &edge.target
                } else {
                    &edge.source
                };
                if visited.contains(next) {
                    continue;
                }
                if visited.len() >= max_nodes {
                    state.result.truncate("graph_node_limit");
                    break;
                }
                let Some(mut edge_proof) = state.support(&edge.id, true, None).await? else {
                    continue;
                };
                let Some(node_proof) = state.support(next, false, None).await? else {
                    continue;
                };
                let Some(next_node) = state.node(next.clone()).await? else {
                    state.result.truncate("graph_concurrent_reconciliation");
                    continue;
                };
                edge_proof
                    .locators
                    .push(format!("{} -{}-> {}", edge.source, edge.kind, edge.target).into());
                let mut path_proof = proof.clone();
                if !path_proof.merge(&edge_proof) || !path_proof.merge(&node_proof) {
                    state.result.truncate("graph_path_proof_limit");
                    continue;
                }
                visited.insert(next.clone());
                queue.push_back((next_node, path_proof, distance + 1));
            }
        }
        if state.unavailable > 0 && state.verified == 0 {
            state.result.status = SourceStatus::Unavailable;
        }
        Ok(state.result)
    }
}

fn contains_label(text: &str, label: &str) -> bool {
    text.match_indices(label).any(|(start, _)| {
        let end = start + label.len();
        text[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !c.is_alphanumeric())
            && text[end..]
                .chars()
                .next()
                .is_none_or(|c| !c.is_alphanumeric())
    })
}
