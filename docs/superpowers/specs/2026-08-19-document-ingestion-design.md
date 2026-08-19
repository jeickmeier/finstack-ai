# Document Ingestion Extension — Spec

Date: 2026-08-19. Based on investigation of the finstack-ai workspace (kernel content
model, toolset/middleware extension patterns, ArtifactStore service, binding surfaces)
and external verification of the `anydoc` and `pdf-inspector` crates. Related in-flight
work: `docs/superpowers/plans/2026-08-19-openrouter-provider-spec.md` decisions 16–22
(MediaResolver) — this spec is deliberately independent of it (see Non-goals).

## Goal

Give agents first-class document ingestion, in two components plus one core API change:

- **Phase 1 — toolset**: new crate `extensions/toolsets/finstack-ai-tools-document`
  wrapping `anydoc` (and `pdf-inspector` for PDF classification). Model-callable tools
  convert any supported office/document format to GitHub-Flavored Markdown.
- **Phase 2 — attachment pipeline**: new crate
  `extensions/middleware/finstack-ai-middleware-document-ingest` plus an `attachments`
  input on `AgentRunRequest`, so a document attached to a chat/run is auto-parsed into
  model-visible text before inference.
- **Phase 3 — bindings parity**: Python and WASM bindings accept attachments and can
  register the toolset/middleware. Full parity in v1 (all bindings).

## Format coverage

| Format family | Extensions | Engine | Notes |
|---|---|---|---|
| PDF (text/mixed) | .pdf | anydoc → pdf-inspector | position-aware text, tables, Markdown |
| PDF (scanned) | .pdf | pdf-inspector classifier | classified + flagged `requires_ocr`; no OCR in v1 |
| Word | .doc .docx .docm | anydoc | |
| PowerPoint | .ppt .pptx .pptm | anydoc | |
| Excel | .xls .xlsx .xlsm .xlsb | anydoc (calamine) | |
| OpenDocument | .odt .ods .odp | anydoc | |
| Other | .rtf .epub .csv | anydoc | |

## External crate facts (verified 2026-08-19)

- `anydoc` v0.1.9, MIT, crates.io. Converts 14 formats to GFM through a unified
  document model. Median conversion < 5 ms. Pure-Rust dependency tree: `calamine`,
  `cfb`, `csv`, `flate2`, `encoding_rs`, `quick-xml`, `zip`, `pdf-inspector` (pinned
  1.14.2, no OCR features). Ships a browser WASM binding; no tokio/reqwest/native C
  libraries. Young crate (0.1.x, ~109 commits) — API churn is expected.
- `pdf-inspector` v1.14/1.15, MIT, crates.io. **Empty default feature set**; the core
  (classification 10–50 ms, position-aware extraction, Markdown, table detection) is
  pure Rust with explicit wasm32 support (single-threaded lopdf, embedded CMaps). OCR
  (`ocr`, `render-pdfium`, `ocr-oar`, `model-download`) pulls PDFium/ONNX Runtime and
  stays disabled.

## Decisions

1. **Create `extensions/toolsets/finstack-ai-tools-document`** following the
   calculator/filesystem toolset pattern: `try_new() -> Result<Self, DocumentError>`,
   eager `ToolSpec` construction with hand-written JSON Schemas, `Arc<[ToolSpec]>`
   cache, `serde(deny_unknown_fields)` argument structs, single-item
   `ToolStreamItem::Completed` streams.
2. **Toolset id is `finstack.tools.document`.** Tool model names are `document_parse`
   and `pdf_classify`.
3. **Dependencies**: `anydoc` (default features only, which are empty) and
   `pdf-inspector` with `default-features = false`, version pinned to match anydoc's
   own pin to avoid duplicate builds. Plus the standard set: `finstack-ai-runtime`
   (`default-features = false`), `futures-util`, `serde`, `serde_json`, `thiserror`.
4. **Public `parser` module** in the toolset crate is the shared engine:
   `DocumentParser`, `DocumentLimits`, `ParsedDocument`, `DocumentClassification`.
   All anydoc/pdf-inspector types stay private behind it — this is the churn boundary
   for the 0.1.x dependency. The middleware crate consumes only this module.

   ```rust
   pub struct DocumentLimits {
       pub max_input_bytes: u64,   // default: MAX_ARTIFACT_BYTES (4 MiB)
       pub max_output_bytes: u64,  // default: aligned with TEXT_MAX_BYTES spill rules
       pub max_pages: u32,         // default: 500
   }

   pub struct ParsedDocument {
       pub markdown: String,
       pub format: DocumentFormat,        // detected, not caller-asserted
       pub page_count: Option<u32>,
       pub classification: Option<DocumentClassification>, // PDFs only
       pub requires_ocr: bool,
       pub truncated: bool,
   }
   ```
5. **Format detection is by content (magic bytes / container sniffing) with media type
   as a hint**, never by file extension alone. A mismatch between declared media type
   and detected format is not an error; the detected format wins and is reported.
6. **`document_parse` inputs**: exactly one source — `artifact` (a staged
   `ArtifactRef`) or `path` (native-only). Optional `page_range` (PDFs only) and
   per-call limit overrides bounded by the constructed `DocumentLimits`. Output:
   `ParsedDocument` fields plus source metadata.
7. **`path` sources fail closed off unix**, mirroring the filesystem toolset: on
   non-unix targets `document_parse` with a `path` source returns
   `DOCUMENT_PATH_UNSUPPORTED`. Artifact sources work on every target including wasm.
8. **`pdf_classify`** takes the same source union and returns classification
   (`text | scanned | mixed | image`), page count, and per-page text-coverage stats.
   Non-PDF input returns `DOCUMENT_UNSUPPORTED_FORMAT`. It exists as a separate cheap
   tool (10–50 ms) so workflows can route before paying for full parsing.
9. **A fully scanned PDF is a successful `document_parse` result**, with
   `requires_ocr: true`, `truncated: false`, and whatever text was recoverable
   (possibly empty markdown). It is never a tool error; agents decide the next step.
10. **Stable error codes** (`pub const` string constants, following the
    `CALCULATOR_*`/`FILESYSTEM_*` precedent): `DOCUMENT_INVALID_ARGUMENTS`,
    `DOCUMENT_PARSE_FAILED`, `DOCUMENT_TOO_LARGE`, `DOCUMENT_SOURCE_UNAVAILABLE`,
    `DOCUMENT_UNSUPPORTED_FORMAT`, `DOCUMENT_PATH_UNSUPPORTED`.
11. **Oversized Markdown output spills to the `ArtifactStore`** via
    `stage_required_artifact` when the toolset is constructed
    `with_artifact_store(...)`, exactly like the filesystem toolset; otherwise output
    is truncated with `truncated: true`. Inline results respect `max_result_bytes`.
12. **Tool metadata**: both tools reuse the exact `ToolExecutionMode`,
    `SideEffectClass`, `RetrySafety`, and `ApprovalRequirement` values used by the
    filesystem toolset's read-only tools (read/list/glob/search) — parsing is a
    read-only operation regardless of source, needs no approval, and is safe to
    retry.
13. **Create `extensions/middleware/finstack-ai-middleware-document-ingest`**
    implementing the runtime middleware trait (per compaction/verify precedent). It
    depends on `finstack-ai-tools-document` only for the `parser` module.
14. **Middleware behavior**: during run preparation, scan incoming *user* message
    content for `ContentBlock::File(MediaRef)` whose media type (or sniffed content)
    is a supported document format. For each match: resolve bytes from the
    `ArtifactStore`, verify length and digest against the `BlobRef`, parse via
    `DocumentParser`, and **append** a `ContentBlock::Text` framed as:
    `Attached document "<name>" (<format>, <n> pages, converted to Markdown):` followed
    by the Markdown. The original `File` block is preserved unmodified in the
    conversation record.
15. **Scanned PDFs get a note, not content**: the appended text states the attachment
    is a scanned PDF and OCR is not enabled, so the model can respond honestly or
    reach for tools.
16. **The middleware is fail-soft.** Resolution or parse failure appends a one-line
    explanatory note, emits an observer-visible event, and never aborts the run. A
    strict mode is not offered in v1.
17. **Determinism**: parsing happens once, pre-model; the appended text becomes part
    of the journaled conversation. Replay never re-parses. The middleware does no
    network or filesystem I/O beyond the `ArtifactStore` port.
18. **`AgentRunRequest` gains `attachments: Arc<[AttachmentInput]>`** (empty by
    default; existing constructors unaffected):

    ```rust
    pub struct AttachmentInput {
        pub artifact: ArtifactRef,
        pub media_type: MediaTypeBuf,
        pub name: Option<Arc<str>>,
    }
    ```

    `agent/prepare.rs` (and the session run path) maps each entry to a
    `ContentBlock::File(MediaRef(BlobRef))` on the user message. Role validation
    already permits `File` on `User`.
19. **BlobRef↔ArtifactRef convention (frozen)**: `BlobRef.id` is the `ArtifactRef`
    id verbatim; `BlobRef.digest` and `length` are copied from the staged artifact's
    metadata. The middleware resolves bytes by constructing the `ArtifactRef` from
    the `BlobRef.id` within the run's `ArtifactScope` and verifying the digest. No
    new port or registry is introduced.
20. **Attachment validation at request build time**: media type must parse; at most
    `MAX_RUN_ATTACHMENTS = 8` per run; each artifact must already be staged (the run
    API never accepts raw bytes). Violations are `AgentRunError` argument errors.
21. **Python binding**: run/session methods accept
    `attachments=[Attachment(data=bytes | path=..., media_type=..., name=...)]`. The
    binding stages bytes into the artifact store and builds `AttachmentInput`s. The
    document toolset and ingest middleware are constructible and registrable from
    Python like existing extensions.
22. **WASM binding**: same surface with `Uint8Array` data. `finstack-ai-tools-document`
    and the middleware must appear in the wasm build; `anydoc`/`pdf-inspector` are
    wasm-clean, so **no additions to `FORBIDDEN_WASM`** in
    `tools/wasm_package/check.py` — CI enforces the claim. Path sources return
    `DOCUMENT_PATH_UNSUPPORTED` on wasm (decision 7).
23. **Fixtures**: a small corpus under `fixtures/documents/` — text PDF, scanned PDF,
    table-heavy PDF, docx, xlsx, pptx, csv, an oversized file, and a corrupt file —
    shared by toolset unit tests, middleware tests (against the in-memory artifact
    store), a `finstack-ai-test` integration lane (attach → auto-parse →
    model-visible text), and binding tests.
24. **Public API compatibility**: the `AgentRunRequest` change updates
    `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai.txt`; the
    new field is additive.

## Non-goals

- **OCR** — no PDFium/ONNX in any build; follow-up behind an off-by-default cargo
  feature on the toolset crate.
- **Chunking, embeddings, RAG ingestion** — this extension produces Markdown; what
  consumes it is out of scope.
- **Provider-native document upload** (e.g. Anthropic document blocks) and any
  dependency on the OpenRouter spec's `MediaResolver` port. If MediaResolver lands,
  raw-media passthrough to providers composes with — and does not replace — this
  parse-to-text pipeline.
- **Blob storage service** — attachments ride the existing `ArtifactStore`; no new
  storage port.
- **Formats anydoc does not support** (images, HTML, email archives).
