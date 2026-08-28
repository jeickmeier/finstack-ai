# Document ingestion

Two released components cooperate:

- **`finstack-ai-tools-document`** — model-callable tools that convert
  documents (pdf, docx, xlsx, pptx, odf, rtf, epub, csv) to
  GitHub-Flavored Markdown via a pure-Rust parser. Scanned PDFs succeed
  with `requires_ocr: true` rather than erroring; OCR is not enabled.
- **`finstack-ai-middleware-document-ingest`** — a `BeforeModel`
  middleware. When a run carries attachments
  (`AgentRunRequest.attachments`, staged through the artifact store),
  it resolves the bytes, verifies digests, parses, and swaps the
  model-visible file blocks for extracted Markdown. The journaled
  conversation keeps the original attachment references, so provenance
  survives.

The knowledge agent's `ingest` flow attaches a file to a run on the
target session and asks the model to summarize it and `remember` the key
facts — after that, the document's content is reachable both through the
session history and through memory recall in later sessions.
