# finstack-ai-middleware-document-ingest

`BeforeModel` middleware that rewrites attached documents into
model-visible Markdown. The canonical, journaled conversation keeps its
`ContentBlock::File` blocks unchanged; only the model-visible
`ModelRequestDraft` is rewritten, so replay never re-parses and providers
never see a media block they cannot map.

```rust
use std::sync::Arc;

use finstack_ai_middleware_document_ingest::DocumentIngestMiddleware;

let middleware = DocumentIngestMiddleware::try_new(store)
    .expect("document ingest middleware");
```

## Surface

| Item | Detail |
| --- | --- |
| Component id | `finstack.middleware.document-ingest` |
| Stage | `BeforeModel` only |
| Order | `OrderTier::ContextMutation`, priority `0` |
| Outcome | `StageOutcome::Continue` (no supported attachments) or `StageOutcome::Replace` (rewritten draft) |

## Configuration identity

`configuration_digest` is `Digest::raw_json` over RFC 8785 canonical JSON of
a crate-private versioned limits shape: `max_input_bytes`, `max_output_bytes`,
`max_pages`, and an explicit version tag (`document-ingest-limits-v1`).
Distinct limits produce distinct identities. `try_new` hashes the
store-derived input ceiling together with the default output and page
limits.

This is an intentional descriptor-identity change from the previous constant
`document-ingest-v1` digest.

## Scoped artifact lookup

A `ContentBlock::File` carries a `BlobRef`. `ArtifactStore::get_by_blob` resolves it inside the authorized run scope and returns the store-owned exact `ArtifactRef` with verified bytes. Digestless blobs become a deterministic coded note; no process-local index participates.

## Failure contract

Store unavailability, cancellation, missing scoped content, and reference or content-integrity failures abort with a stable non-secret middleware error. Unsupported formats, scanned PDFs, parser failures, and digestless legacy blobs become bounded coded notes and never leak a `File` block to the model.

**Deviation from spec decision 16** (documented per plan): decision 16
describes the middleware as emitting a distinct observer-visible event on
a fail-soft skip. This implementation does not emit a separate event —
the replacement note appended to the model-visible request (via
`StageOutcome::Replace`) *is* the observer-visible signal; there is no
additional event alongside it.

## `document_parse` / `pdf_classify` per-tool ids

This middleware only rewrites `ContentBlock::File` blocks directly; it
does not itself call
[`finstack.tools.document.parse`](../../toolsets/finstack-ai-tools-document/README.md)
or `finstack.tools.document.classify`. Those are the two tool ids exposed
by `finstack-ai-tools-document` (the sibling toolset crate this middleware
depends on for its parsing engine, `finstack_ai_tools_document::parser`) —
listed here because a document-ingestion deployment typically registers
both the toolset and this middleware together, and the ids are otherwise
only documented in the toolset's own README.

## The scanned-PDF / `requires_ocr` contract

A scanned PDF that parses successfully but yields no extractable text
(`requires_ocr && markdown.is_empty()`) gets its own note instead of a
generic parse-failure note: `[Attached document "..." is a scanned PDF;
text extraction requires OCR, which is not enabled.]`. This crate performs
no OCR; see `finstack-ai-tools-document`'s README for the designated
future OCR path (pdf-inspector's `ocr` feature paired with `oar-ocr`).

This crate is a T1 native adapter. It is not isolated.
