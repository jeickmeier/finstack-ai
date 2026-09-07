use finstack_ai_search_core::{SearchError, configuration_digest, validate_id};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

/// A configured entity recognizer. The named `label` capture is required.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityRule {
    /// One configured entity kind.
    pub kind: Arc<str>,
    /// Bounded regex with a named `label` capture.
    pub pattern: Arc<str>,
    /// Optional canonical label for aliases recognized by this rule.
    pub canonical_label: Option<Arc<str>>,
}
/// Directed relationship vocabulary, independent of extraction regexes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeKind {
    /// Stable configured relationship name.
    pub kind: Arc<str>,
    /// Required source entity kind.
    pub source_kind: Arc<str>,
    /// Required target entity kind.
    pub target_kind: Arc<str>,
}
/// Conservative relationship recognizer with named `source` and `target` captures.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EdgeRule {
    /// Configured edge kind; endpoints derive their kinds from that vocabulary.
    pub kind: Arc<str>,
    /// Bounded regex with named source/target labels.
    pub pattern: Arc<str>,
}
/// Versioned finite vocabulary and deterministic extraction rules. No ontology
/// inference or model-based extraction is performed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphVocabulary {
    /// Supported extraction schema version, currently 1.
    pub version: u16,
    /// Allowed entity kinds, at most 64.
    pub entity_kinds: Vec<Arc<str>>,
    /// Allowed directed edge kinds, at most 64.
    pub edge_kinds: Vec<EdgeKind>,
    /// At most 64 entity extraction rules.
    pub entity_rules: Vec<EntityRule>,
    /// At most 64 relationship extraction rules.
    pub edge_rules: Vec<EdgeRule>,
}
impl GraphVocabulary {
    /// Validate vocabulary, regexes, captures and references before extraction.
    ///
    /// # Errors
    /// Rejects unknown kinds, conflicting names, unsupported versions or patterns.
    pub fn validate(&self) -> Result<(), SearchError> {
        Compiled::new(self).map(|_| ())
    }
}
pub(crate) struct Compiled {
    entities: Vec<(EntityRule, regex::Regex)>,
    edges: Vec<(EdgeKind, regex::Regex)>,
}
#[derive(Clone)]
pub(crate) struct Entity {
    pub(crate) id: String,
    pub(crate) kind: Arc<str>,
    pub(crate) label: Arc<str>,
    pub(crate) aliases: BTreeSet<String>,
}
pub(crate) struct Edge {
    pub(crate) id: String,
    pub(crate) source: String,
    pub(crate) kind: Arc<str>,
    pub(crate) target: String,
}
pub(crate) struct Extraction {
    pub(crate) entities: BTreeMap<String, Entity>,
    pub(crate) edges: BTreeMap<String, Edge>,
}
impl Compiled {
    pub(crate) fn new(v: &GraphVocabulary) -> Result<Self, SearchError> {
        if v.version != 1
            || v.entity_kinds.is_empty()
            || v.entity_kinds.len() > 64
            || v.edge_kinds.len() > 64
            || v.entity_rules.len() > 64
            || v.edge_rules.len() > 64
            || (v.entity_rules.is_empty() && v.edge_rules.is_empty())
        {
            return Err(SearchError::invalid("graph_vocabulary_bounds"));
        }
        let kinds: BTreeSet<_> = v.entity_kinds.iter().collect();
        if kinds.len() != v.entity_kinds.len() {
            return Err(SearchError::invalid("graph_duplicate_kind"));
        }
        for kind in &v.entity_kinds {
            validate_id(kind)?;
        }
        let mut edge_kinds = BTreeMap::new();
        for kind in &v.edge_kinds {
            validate_id(&kind.kind)?;
            if !kinds.contains(&kind.source_kind)
                || !kinds.contains(&kind.target_kind)
                || edge_kinds.insert(kind.kind.clone(), kind.clone()).is_some()
            {
                return Err(SearchError::invalid("graph_edge_kind"));
            }
        }
        let mut entities = Vec::new();
        for rule in &v.entity_rules {
            if !kinds.contains(&rule.kind) {
                return Err(SearchError::invalid("graph_entity_rule_kind"));
            }
            if let Some(label) = &rule.canonical_label {
                normalize(label)?;
            }
            let regex = compile(&rule.pattern)?;
            captures(&regex, &["label"])?;
            entities.push((rule.clone(), regex));
        }
        let mut edges = Vec::new();
        for rule in &v.edge_rules {
            let kind = edge_kinds
                .get(&rule.kind)
                .ok_or_else(|| SearchError::invalid("graph_edge_rule_kind"))?;
            let regex = compile(&rule.pattern)?;
            captures(&regex, &["source", "target"])?;
            edges.push((kind.clone(), regex));
        }
        Ok(Self { entities, edges })
    }
    pub(crate) fn extract(
        &self,
        scope: &str,
        text: &str,
        max_entities: usize,
        max_edges: usize,
    ) -> Result<Extraction, SearchError> {
        let mut result = Extraction {
            entities: BTreeMap::new(),
            edges: BTreeMap::new(),
        };
        let mut matches = 0;
        for (rule, regex) in &self.entities {
            for capture in regex.captures_iter(text) {
                matches += 1;
                if matches > 4096 {
                    return Err(SearchError::SearchCapacityExceeded {
                        resource: "graph_extraction_matches".into(),
                    });
                }
                let label = capture
                    .name("label")
                    .ok_or_else(|| SearchError::invalid("graph_entity_capture"))?
                    .as_str();
                self.entity(
                    &mut result,
                    scope,
                    &rule.kind,
                    label,
                    rule.canonical_label.as_deref(),
                    max_entities,
                )?;
            }
        }
        for (kind, regex) in &self.edges {
            for capture in regex.captures_iter(text) {
                matches += 1;
                if matches > 4096 {
                    return Err(SearchError::SearchCapacityExceeded {
                        resource: "graph_extraction_matches".into(),
                    });
                }
                let source = capture
                    .name("source")
                    .ok_or_else(|| SearchError::invalid("graph_source_capture"))?
                    .as_str();
                let target = capture
                    .name("target")
                    .ok_or_else(|| SearchError::invalid("graph_target_capture"))?
                    .as_str();
                let source = self.entity(
                    &mut result,
                    scope,
                    &kind.source_kind,
                    source,
                    None,
                    max_entities,
                )?;
                let target = self.entity(
                    &mut result,
                    scope,
                    &kind.target_kind,
                    target,
                    None,
                    max_entities,
                )?;
                let id =
                    configuration_digest("graph-edge", &(scope, &source, &kind.kind, &target))?
                        .to_hex();
                result.edges.insert(
                    id.clone(),
                    Edge {
                        id,
                        source,
                        kind: kind.kind.clone(),
                        target,
                    },
                );
                if result.edges.len() > max_edges {
                    return Err(SearchError::SearchCapacityExceeded {
                        resource: "graph_extraction_edges".into(),
                    });
                }
            }
        }
        Ok(result)
    }
    fn entity(
        &self,
        result: &mut Extraction,
        scope: &str,
        kind: &Arc<str>,
        alias: &str,
        explicit: Option<&str>,
        max: usize,
    ) -> Result<String, SearchError> {
        let original = alias;
        let alias = normalize(alias)?;
        let mut canonical = alias.clone();
        for (rule, regex) in &self.entities {
            if rule.kind == *kind
                && let Some(label) = &rule.canonical_label
                && regex
                    .find(original)
                    .is_some_and(|found| found.start() == 0 && found.end() == original.len())
            {
                canonical = normalize(label)?;
                break;
            }
        }
        if let Some(label) = explicit {
            canonical = normalize(label)?;
        }
        let id = configuration_digest("graph-entity", &(scope, kind, &canonical))?.to_hex();
        let entity = result.entities.entry(id.clone()).or_insert_with(|| Entity {
            id: id.clone(),
            kind: kind.clone(),
            label: canonical.clone().into(),
            aliases: BTreeSet::new(),
        });
        entity.aliases.insert(alias);
        entity.aliases.insert(canonical);
        if result.entities.len() > max {
            return Err(SearchError::SearchCapacityExceeded {
                resource: "graph_extraction_entities".into(),
            });
        }
        Ok(id)
    }
}
pub(crate) fn normalize(value: &str) -> Result<String, SearchError> {
    if value.len() > 256 || value.contains('\0') {
        return Err(SearchError::invalid("graph_label"));
    }
    let value = value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    if value.is_empty() || value.len() > 256 {
        return Err(SearchError::invalid("graph_label"));
    }
    Ok(value)
}
fn compile(pattern: &str) -> Result<regex::Regex, SearchError> {
    if pattern.len() > 1024 {
        return Err(SearchError::invalid("graph_pattern_limit"));
    }
    regex::RegexBuilder::new(pattern)
        .size_limit(1_048_576)
        .dfa_size_limit(1_048_576)
        .build()
        .map_err(|_| SearchError::invalid("graph_pattern"))
}
fn captures(regex: &regex::Regex, required: &[&str]) -> Result<(), SearchError> {
    if required
        .iter()
        .any(|name| !regex.capture_names().flatten().any(|found| found == *name))
    {
        return Err(SearchError::invalid("graph_capture_names"));
    }
    Ok(())
}
