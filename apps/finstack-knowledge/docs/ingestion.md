# Document ingestion

The document parser and `BeforeModel` ingest middleware resolve attachments through
the artifact store, verify digests, and replace model-visible file blocks with
extracted Markdown plus the exact uploaded blob reference. The journal retains the
original attachment. Scanned PDFs preserve `requires_ocr`; no OCR is performed.

`index_document` is a separate idempotent tool effect. It reads the authorized
artifact, parses it and stores heading/paragraph-aware chunks in a separate SQLite
index. Default targets are 4,096 Unicode characters with up to 512 overlap. Repeating
the same input/configuration returns the same receipt. Citations retain the artifact
and parsed-character locators; missing sources are excluded and reconciled out.

The CLI `ingest` flow attaches a file on the selected session, asks the model to
index it, summarize it and remember useful facts, then verifies a current indexing
receipt. A summary without the indexing effect fails explicitly. Global search can
retrieve document chunks, memory facts and committed session evidence independently.
The application owns artifact retention; indexing does not pin source artifacts.
Semantic document retrieval requires an explicit embedder and bounded maintenance.
