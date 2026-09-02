//! Debug helpers exposing `finstack-ai-tools-document`'s parser directly.
//!
//! These do not go through the ingest middleware or a run; they let a
//! developer see exactly what Markdown the middleware would inject for a
//! given file, without constructing an `Agent` or `Run`.

use finstack_ai_tools_document::parser::{self, DocumentLimits};
use pyo3::exceptions::PyValueError;
use pyo3::prelude::*;
use pyo3::types::PyDict;

use crate::run::resolve_data_or_path;

/// Parse a document to Markdown and return only the Markdown string.
///
/// Exactly one of `data`/`path` is required, matching [`crate::run::PyAttachment`]'s
/// contract; a `path` is read bounded to 4 MiB.
#[pyfunction]
#[pyo3(signature = (media_type, data = None, path = None))]
#[expect(
    clippy::needless_pass_by_value,
    reason = "pyo3 #[pyfunction] arguments are owned Python-extracted values"
)]
pub(crate) fn parse_document_markdown(
    media_type: String,
    data: Option<Vec<u8>>,
    path: Option<String>,
) -> PyResult<String> {
    let bytes = resolve_data_or_path(data, path.as_deref())?;
    let parsed = parser::parse(
        &bytes,
        Some(media_type.as_str()),
        &DocumentLimits::default(),
    )
    .map_err(|error| PyValueError::new_err(error.to_string()))?;
    Ok(parsed.markdown)
}

/// Parse a document and return the full detailed result as a dict.
///
/// Exactly one of `data`/`path` is required, matching [`crate::run::PyAttachment`]'s
/// contract; a `path` is read bounded to 4 MiB.
#[pyfunction]
#[pyo3(signature = (media_type, data = None, path = None))]
#[expect(
    clippy::needless_pass_by_value,
    reason = "pyo3 #[pyfunction] arguments are owned Python-extracted values"
)]
pub(crate) fn parse_document(
    py: Python<'_>,
    media_type: String,
    data: Option<Vec<u8>>,
    path: Option<String>,
) -> PyResult<Py<PyDict>> {
    let bytes = resolve_data_or_path(data, path.as_deref())?;
    let parsed = parser::parse(
        &bytes,
        Some(media_type.as_str()),
        &DocumentLimits::default(),
    )
    .map_err(|error| PyValueError::new_err(error.to_string()))?;
    let result = PyDict::new(py);
    result.set_item("markdown", &parsed.markdown)?;
    result.set_item("format", wire_name(parsed.format))?;
    result.set_item("page_count", parsed.page_count)?;
    result.set_item("classification", parsed.classification.map(wire_name))?;
    result.set_item("requires_ocr", parsed.requires_ocr)?;
    result.set_item("truncated", parsed.truncated)?;
    Ok(result.unbind())
}

/// Render a parser enum's stable `snake_case` wire name (matches its `Serialize`).
fn wire_name(value: impl serde::Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}
