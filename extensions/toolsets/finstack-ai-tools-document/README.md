# finstack-ai-tools-document

Document-to-Markdown ingestion toolset built on `anydoc` (container format
conversion) and `pdf-inspector` (PDF classification and page extraction).
Two tools convert pdf/docx/doc/xlsx/xls/pptx/ppt/odt/ods/odp/rtf/epub/csv
input to GitHub-Flavored Markdown or classify a PDF's page content — no
OCR is performed in this crate.

```rust
use std::sync::Arc;

use finstack_ai_runtime::ArtifactStore;
use finstack_ai_tools_document::DocumentToolset;

# fn build(artifact_store: Arc<dyn ArtifactStore>) {
let tools = DocumentToolset::try_new(Arc::clone(&artifact_store))
    .expect("document toolset");
# }
```

## Tools

| Tool id | Model name | Purpose |
| --- | --- | --- |
| `finstack.tools.document.parse` | `document_parse` | Convert a document to Markdown. Optional PDF page range. |
| `finstack.tools.document.classify` | `pdf_classify` | Classify a PDF (`text`/`scanned`/`mixed`/`image`) with page count; no extraction. |

Each tool call resolves an `artifact` reference through the store supplied at
construction. Filesystem paths are deliberately absent from the model-facing
schema. `document_parse` output larger than `max_result_bytes` is staged as a
new artifact, with bounded inline Markdown in the tool result.

## Error codes

| Code | Meaning |
| --- | --- |
| `document_invalid_arguments` | Malformed call arguments, ambiguous/missing source, or a reversed/zero-based `page_range`. |
| `document_parse_failed` | The detected format's parser rejected the content. |
| `document_too_large` | Input exceeds the configured byte or page ceiling. |
| `document_source_unavailable` | The artifact source could not be resolved. |
| `document_unsupported_format` | No supported document format detected. |

## The scanned-PDF / `requires_ocr` contract

A scanned or image-only PDF is a **successful** `document_parse` call, not
an error: `requires_ocr` is set to `true` and `markdown` may be empty (or
partially populated for a `mixed` PDF). Callers must check `requires_ocr`
rather than treating empty `markdown` as failure. `pdf_classify` exposes
the same signal ahead of time via its `classification` field
(`scanned`/`image` implies OCR would be needed to recover full text).

## OCR non-goal

This crate performs no OCR.

## Debugging

`parser::parse(bytes, media_type_hint, limits) -> Result<ParsedDocument, DocumentParseError>`
is the crate's public churn-boundary entry point (see [`src/parser.rs`](src/parser.rs))
and needs no toolset, artifact store, or run to call directly. Both bindings
expose a thin debug wrapper over it so a developer can see exactly what
Markdown the ingest middleware would inject for a given file without
constructing an `Agent` or `Run`:

- Python: `finstack_ai.parse_document_markdown(media_type, data=..., path=...)`
  returns the Markdown string; `finstack_ai.parse_document(...)` returns the
  full `ParsedDocument` fields as a dict.
- WASM/JS: `parseDocumentMarkdown(data, mediaType)` returns the Markdown
  string; `parseDocument(data, mediaType)` returns the full result.

Both raise/throw using the stable `document_*` error codes on parse failure.

## Fixtures

`fixtures/documents/` (text PDF, scanned PDF, docx, xlsx, pptx, csv, and a
corrupt file) is shared by this crate's unit tests, middleware tests, and
binding tests.

This crate is a T1 native adapter (anydoc/pdf-inspector are pure-Rust,
wasm32-clean; no OCR, no native C dependency).
