use finstack_ai::runtime::ports::tool::{PortableSchemaKind, normalize_portable_schema};

/// Map Python-facing schema kinds into the shared portable runtime policy.
pub(crate) fn normalize_pydantic_schema(
    schema: serde_json::Value,
    kind: &str,
) -> Result<serde_json::Value, String> {
    let kind = match kind {
        "tool_input" => PortableSchemaKind::ToolInput,
        "tool_output" => PortableSchemaKind::ToolOutput,
        "structured_output" => PortableSchemaKind::StructuredOutput,
        _ => return Err(format!("unsupported Pydantic schema kind: {kind}")),
    };
    normalize_portable_schema(schema, kind).map_err(|error| error.to_string())
}
