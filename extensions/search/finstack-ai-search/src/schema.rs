//! Closed structured query schemas, including nested semantic/graph choices.

use std::collections::BTreeSet;

use finstack_ai_kernel::RawJson;
use serde_json::{Value, json};

use crate::{SearchEngine, SearchError};

pub(crate) fn input_schema(engine: &SearchEngine) -> Result<RawJson, SearchError> {
    let descriptors = engine.sources();
    let lexical: BTreeSet<_> = descriptors
        .iter()
        .flat_map(|source| source.lexical.iter().copied())
        .collect();
    let spaces: BTreeSet<_> = descriptors
        .iter()
        .flat_map(|source| source.semantic_spaces.iter().cloned())
        .collect();
    let source_ids: Vec<_> = descriptors.iter().map(|source| &source.source_id).collect();
    let mut choices = vec![json!({"type":"null"})];
    if !lexical.is_empty() {
        choices.push(object(json!({"kind":{"type":"string","enum":["lexical"]},"config":{"type":"string","enum":lexical}})));
    }
    if !spaces.is_empty() {
        let config = object(json!({"space":{"type":"string","enum":spaces}}));
        choices.push(object(
            json!({"kind":{"type":"string","enum":["semantic"]},"config":config}),
        ));
    }
    if descriptors.iter().any(|source| source.graph) {
        let limits = &engine.config().limits;
        let entity = object(json!({"kind":{"type":"string","enum":["entity"]}}));
        let depth = json!({"type":"integer","minimum":1,"maximum":8});
        let nodes = json!({"type":"integer","minimum":1,"maximum":limits.max_graph_nodes});
        let edges = json!({"type":"integer","minimum":1,"maximum":limits.max_graph_edges});
        let neighborhood = object(
            json!({"kind":{"type":"string","enum":["neighborhood"]},"depth":depth,"max_nodes":nodes,"max_edges":edges}),
        );
        let path = object(
            json!({"kind":{"type":"string","enum":["path"]},"target":{"type":"string","minLength":1,"maxLength":256},"depth":depth,"max_nodes":nodes,"max_edges":edges}),
        );
        choices.push(object(json!({"kind":{"type":"string","enum":["graph"]},"config":{"anyOf":[entity,neighborhood,path]}})));
    }
    let value = object(json!({
        "text":{"type":"string","minLength":1,"maxLength":engine.config().limits.max_query_bytes},
        "strategy":{"anyOf":choices},
        "sources":{"anyOf":[{"type":"null"},{"type":"array","items":{"type":"string","enum":source_ids},"minItems":1,"maxItems":32}]},
        "limit":{"type":["integer","null"],"minimum":1,"maximum":engine.config().limits.max_results}
    }));
    RawJson::parse(serde_json::to_vec(&value).map_err(|_| SearchError::invalid("search_schema"))?)
        .map_err(|_| SearchError::invalid("search_schema"))
}

fn object(properties: Value) -> Value {
    let required: Vec<_> = properties
        .as_object()
        .into_iter()
        .flat_map(|map| map.keys().cloned())
        .collect();
    let mut value = serde_json::Map::new();
    value.insert("type".into(), json!("object"));
    value.insert("additionalProperties".into(), json!(false));
    value.insert("required".into(), json!(required));
    value.insert("properties".into(), properties);
    Value::Object(value)
}
