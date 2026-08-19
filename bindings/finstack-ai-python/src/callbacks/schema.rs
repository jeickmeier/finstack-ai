use std::collections::BTreeSet;

/// Normalize the deliberately small provider/native/WASM Pydantic schema subset.
pub(crate) fn normalize_pydantic_schema(
    mut schema: serde_json::Value,
    kind: &str,
) -> Result<serde_json::Value, String> {
    let require_object_root = match kind {
        "tool_input" | "structured_output" => true,
        "tool_output" => false,
        _ => return Err(format!("unsupported Pydantic schema kind: {kind}")),
    };
    normalize_schema_node(&mut schema, "", 0)?;
    if require_object_root
        && schema
            .as_object()
            .and_then(|object| object.get("type"))
            .and_then(serde_json::Value::as_str)
            != Some("object")
    {
        return Err(format!(
            "{kind} schema root must be an object in the portable subset"
        ));
    }
    Ok(schema)
}

#[expect(
    clippy::too_many_lines,
    reason = "portable schema validation is one bounded recursive policy traversal"
)]
fn normalize_schema_node(
    schema: &mut serde_json::Value,
    path: &str,
    depth: usize,
) -> Result<(), String> {
    if depth > 64 {
        return Err(format!(
            "Pydantic schema nesting exceeds 64 levels at {path}"
        ));
    }
    let object = schema
        .as_object_mut()
        .ok_or_else(|| format!("schema at {path} must be an object"))?;
    for keyword in object.keys() {
        if !matches!(
            keyword.as_str(),
            "$defs"
                | "$ref"
                | "$schema"
                | "additionalProperties"
                | "anyOf"
                | "const"
                | "description"
                | "enum"
                | "items"
                | "properties"
                | "required"
                | "title"
                | "type"
        ) {
            return Err(format!(
                "unsupported JSON Schema keyword '{keyword}' at {}",
                pointer(path, keyword)
            ));
        }
    }

    if let Some(draft) = object.get("$schema")
        && draft.as_str() != Some("https://json-schema.org/draft/2020-12/schema")
    {
        return Err(format!(
            "unsupported JSON Schema draft at {}",
            pointer(path, "$schema")
        ));
    }
    if let Some(reference) = object.get("$ref")
        && !reference
            .as_str()
            .is_some_and(|value| value.starts_with("#/$defs/"))
    {
        return Err(format!(
            "unsupported non-local JSON Schema reference at {}",
            pointer(path, "$ref")
        ));
    }
    if let Some(kind) = object.get("type") {
        let Some(kind) = kind.as_str() else {
            return Err(format!(
                "JSON Schema type must be a single string at {}",
                pointer(path, "type")
            ));
        };
        if !matches!(
            kind,
            "array" | "boolean" | "integer" | "null" | "number" | "object" | "string"
        ) {
            return Err(format!(
                "unsupported JSON Schema type '{kind}' at {}",
                pointer(path, "type")
            ));
        }
    }
    for label in ["title", "description"] {
        if object.get(label).is_some_and(|value| !value.is_string()) {
            return Err(format!(
                "JSON Schema {label} must be a string at {}",
                pointer(path, label)
            ));
        }
    }
    if object
        .get("enum")
        .is_some_and(|value| value.as_array().is_none_or(Vec::is_empty))
    {
        return Err(format!(
            "JSON Schema enum must be a non-empty array at {}",
            pointer(path, "enum")
        ));
    }

    let object_keywords = object.contains_key("properties")
        || object.contains_key("required")
        || object.contains_key("additionalProperties");
    if object_keywords && object.get("type").and_then(serde_json::Value::as_str) != Some("object") {
        return Err(format!(
            "object schema at {path} must declare type 'object'"
        ));
    }
    if object.get("type").and_then(serde_json::Value::as_str) == Some("object") {
        let property_names = object
            .get("properties")
            .map(|value| {
                value
                    .as_object()
                    .map(|properties| properties.keys().cloned().collect::<BTreeSet<_>>())
                    .ok_or_else(|| {
                        format!(
                            "JSON Schema properties must be an object at {}",
                            pointer(path, "properties")
                        )
                    })
            })
            .transpose()?
            .unwrap_or_default();
        let required = object
            .get("required")
            .map(|value| {
                let items = value.as_array().ok_or_else(|| {
                    format!(
                        "JSON Schema required must be an array at {}",
                        pointer(path, "required")
                    )
                })?;
                let mut names = BTreeSet::new();
                for item in items {
                    let name = item.as_str().ok_or_else(|| {
                        format!(
                            "JSON Schema required entries must be strings at {}",
                            pointer(path, "required")
                        )
                    })?;
                    if !names.insert(name.to_owned()) {
                        return Err(format!(
                            "duplicate required property '{name}' at {}",
                            pointer(path, "required")
                        ));
                    }
                }
                Ok(names)
            })
            .transpose()?
            .unwrap_or_default();
        if let Some(unknown) = required.difference(&property_names).next() {
            return Err(format!(
                "required property '{unknown}' is not declared at {}",
                pointer(path, "required")
            ));
        }
        if let Some(optional) = property_names.difference(&required).next() {
            return Err(format!(
                "unsupported optional property '{optional}' at {}",
                pointer(&pointer(path, "properties"), optional)
            ));
        }
        if !required.is_empty() {
            object.insert(
                "required".to_owned(),
                serde_json::Value::Array(
                    required
                        .into_iter()
                        .map(serde_json::Value::String)
                        .collect(),
                ),
            );
        }
        match object.get("additionalProperties") {
            None => {
                object.insert(
                    "additionalProperties".to_owned(),
                    serde_json::Value::Bool(false),
                );
            }
            Some(serde_json::Value::Bool(false)) => {}
            Some(_) => {
                return Err(format!(
                    "additionalProperties must be false at {}",
                    pointer(path, "additionalProperties")
                ));
            }
        }
    }

    if let Some(definitions) = object.get_mut("$defs") {
        let definitions = definitions.as_object_mut().ok_or_else(|| {
            format!(
                "JSON Schema $defs must be an object at {}",
                pointer(path, "$defs")
            )
        })?;
        for (name, definition) in definitions {
            normalize_schema_node(
                definition,
                &pointer(&pointer(path, "$defs"), name),
                depth + 1,
            )?;
        }
    }
    if let Some(properties) = object.get_mut("properties") {
        let properties = properties.as_object_mut().ok_or_else(|| {
            format!(
                "JSON Schema properties must be an object at {}",
                pointer(path, "properties")
            )
        })?;
        for (name, property) in properties {
            normalize_schema_node(
                property,
                &pointer(&pointer(path, "properties"), name),
                depth + 1,
            )?;
        }
    }
    if let Some(items) = object.get_mut("items") {
        normalize_schema_node(items, &pointer(path, "items"), depth + 1)?;
    }
    if let Some(branches) = object.get_mut("anyOf") {
        let branches = branches.as_array_mut().ok_or_else(|| {
            format!(
                "JSON Schema anyOf must be an array at {}",
                pointer(path, "anyOf")
            )
        })?;
        if branches.is_empty() || branches.len() > 64 {
            return Err(format!(
                "JSON Schema anyOf must contain 1 through 64 branches at {}",
                pointer(path, "anyOf")
            ));
        }
        for (index, branch) in branches.iter_mut().enumerate() {
            normalize_schema_node(
                branch,
                &pointer(&pointer(path, "anyOf"), &index.to_string()),
                depth + 1,
            )?;
        }
    }
    Ok(())
}

fn pointer(path: &str, token: &str) -> String {
    let escaped = token.replace('~', "~0").replace('/', "~1");
    format!("{path}/{escaped}")
}
