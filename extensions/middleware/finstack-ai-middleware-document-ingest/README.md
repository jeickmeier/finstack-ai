# finstack-ai-middleware-document-ingest

Fail-soft `BeforeModel` middleware that rewrites attached documents into
model-visible Markdown. The canonical, journaled conversation keeps its
`ContentBlock::File` blocks unchanged; only the model-visible
`ModelRequestDraft` is rewritten, so replay never re-parses and providers
never see a media block they cannot map.

```rust
use std::sync::Arc;

use finstack_ai_middleware_document_ingest::{AttachmentIndex, DocumentIngestMiddleware};

let index = Arc::new(AttachmentIndex::default());
// Wherever an attachment is staged, record it so the middleware can
// resolve it later:
// index.insert(artifact_ref.clone());

let middleware = DocumentIngestMiddleware::try_new(store, index)
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

## The `AttachmentIndex` mechanism (spec decision 19)

A `ContentBlock::File` only ever carries a bare `BlobRef` on the wire. The
original design assumed an `ArtifactRef` could be reconstructed from that
`BlobRef` alone, but `ArtifactStore` implementations verify exact
`ArtifactRef` identity against store-assigned state a bare `BlobRef`
cannot reproduce — that convention was unimplementable and was amended
during implementation (spec decision 19, amended 2026-08-19).

Instead, `AttachmentIndex` is a small, bounded (`1024` entries, FIFO
eviction), in-process `blob id -> ArtifactRef` map. Whoever stages an
attachment (the run/session attachment path, or a toolset) calls
`AttachmentIndex::insert(artifact_ref)` with the exact `ArtifactRef` it
received from `stage_put`. The middleware looks the blob back up via
`AttachmentIndex::lookup(&blob_ref)` before fetching bytes from the
`ArtifactStore`, and re-verifies the fetched bytes against the blob's
declared digest. A lookup miss (never staged, or evicted) is treated as a
fail-soft "could not be read" skip, identically to a store or parse
failure.

## The fail-soft contract

Every failure mode — index miss, store fetch failure, digest mismatch, or
a parser error — collapses to a one-line replacement note in the
model-visible text (e.g. `[Attached document "report.pdf" could not be
read; it was skipped.]`) and the run continues. The middleware never
aborts a run and never surfaces a distinguishable error to the caller for
a single bad attachment. There is no strict mode in this release.

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
