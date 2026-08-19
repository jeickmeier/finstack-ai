# finstack-ai-tools-document

Document-to-Markdown ingestion toolset built on `anydoc` (container format
conversion) and `pdf-inspector` (PDF classification and page extraction).
Two tools convert pdf/docx/doc/xlsx/xls/pptx/ppt/odt/ods/odp/rtf/epub/csv
input to GitHub-Flavored Markdown or classify a PDF's page content — no
OCR is performed in this crate.

```rust
use finstack_ai_tools_document::DocumentToolset;

let tools = DocumentToolset::try_new().expect("document toolset");
// Attach an artifact store to accept `artifact` sources and to spill
// oversized `document_parse` output:
// let tools = tools.with_artifact_store(store);
```

## Tools

| Tool id | Model name | Purpose |
| --- | --- | --- |
| `finstack.tools.document.parse` | `document_parse` | Convert a document to Markdown. Optional PDF page range. |
| `finstack.tools.document.classify` | `pdf_classify` | Classify a PDF (`text`/`scanned`/`mixed`/`image`) with page count; no extraction. |

Each tool call resolves its input from exactly one `DocumentSource`: a
staged `artifact` (`ArtifactRef` JSON, requires `with_artifact_store`) or a
native `path` (native hosts only — `DOCUMENT_PATH_UNSUPPORTED` on wasm,
per spec decision 7). `document_parse` output larger than the tool's
`max_result_bytes` is spilled to a new artifact via the attached store,
with the inline `markdown` truncated to fit.

## Error codes

| Code | Meaning |
| --- | --- |
| `document_invalid_arguments` | Malformed call arguments, ambiguous/missing source, or a reversed/zero-based `page_range`. |
| `document_parse_failed` | The detected format's parser rejected the content. |
| `document_too_large` | Input exceeds the configured byte or page ceiling. |
| `document_source_unavailable` | The artifact/path source could not be resolved (store miss, unreadable path). |
| `document_unsupported_format` | No supported document format detected. |
| `document_path_unsupported` | A `path` source was used on a target that does not support native filesystem access. |

## The scanned-PDF / `requires_ocr` contract

A scanned or image-only PDF is a **successful** `document_parse` call, not
an error: `requires_ocr` is set to `true` and `markdown` may be empty (or
partially populated for a `mixed` PDF). Callers must check `requires_ocr`
rather than treating empty `markdown` as failure. `pdf_classify` exposes
the same signal ahead of time via its `classification` field
(`scanned`/`image` implies OCR would be needed to recover full text).

## OCR non-goal

This crate performs no OCR in this release — no PDFium/ONNX Runtime
dependency in any build. The designated future path (evaluated
2026-08-19, not part of this implementation) is an off-by-default cargo
feature on this crate that enables pdf-inspector's `ocr` feature, pairing
`render-pdfium` rasterization with `oar-ocr` (a Rust PP-OCR v3–v6 port) —
native-only, pending a separate accuracy/perf spike. See the spec's
Non-goals section for the full rationale.

## Fixtures

`fixtures/documents/` (text PDF, scanned PDF, docx, xlsx, pptx, csv, a
corrupt file) is shared by this crate's unit tests, the middleware's
tests, and binding tests. The spec's fixture list also called for a
table-heavy PDF; the checked-in corpus omits it — anydoc's own upstream
test corpus already covers table-heavy PDF extraction, so this crate
relies on that coverage rather than duplicating it here.

This crate is a T1 native adapter (anydoc/pdf-inspector are pure-Rust,
wasm32-clean; no OCR, no native C dependency).
