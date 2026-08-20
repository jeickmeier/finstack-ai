# Document Ingestion Extension Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Model-callable document→Markdown tools (`finstack-ai-tools-document`), an auto-ingest middleware (`finstack-ai-middleware-document-ingest`), and an `attachments` input on `AgentRunRequest`, with Python and WASM parity.

**Architecture:** A toolset crate wraps `anydoc` (all formats → GFM) and `pdf-inspector` (PDF classification) behind a public `parser` module. A `BeforeModel` middleware (ContextMutation tier) rewrites the model-visible `ModelRequestDraft`, replacing `ContentBlock::File` blocks whose bytes it resolves from the `ArtifactStore` with extracted-Markdown `Text` blocks — the journaled conversation keeps the original `File` blocks. Attachments enter runs as pre-staged `ArtifactRef`s on `AgentRunRequest`; `prepare.rs` maps them to `File` blocks.

**Tech Stack:** Rust (edition 2024), `anydoc` 0.1.x, `pdf-inspector` (default-features = false), existing `finstack-ai-runtime` ports (`Toolset`, `Middleware`, `ArtifactStore`), PyO3 binding, wasm-bindgen binding.

**Spec:** `docs/superpowers/specs/2026-08-19-document-ingestion-design.md`

## Global Constraints

- Workspace lints/version/edition inherited: every new crate uses `.workspace = true` keys like `extensions/toolsets/finstack-ai-tools-calculator/Cargo.toml`.
- `pdf-inspector` MUST be `default-features = false` (no OCR/PDFium/ONNX). `anydoc` uses default features (its default set is empty).
- No additions to `FORBIDDEN_WASM` in `scripts/wasm_package/check.py`; the wasm package check must stay green.
- Stable string constants exactly as spec'd: toolset id `finstack.tools.document`, tools `document_parse` / `pdf_classify`, error codes `document_invalid_arguments`, `document_parse_failed`, `document_too_large`, `document_source_unavailable`, `document_unsupported_format`, `document_path_unsupported` (lower snake_case values in `pub const` SCREAMING_SNAKE names, matching `CALCULATOR_INVALID_ARGUMENTS` precedent).
- `MAX_RUN_ATTACHMENTS = 8`; input cap = `MAX_ARTIFACT_BYTES` (4 MiB); scanned PDF is a **success** with `requires_ocr: true`.
- Middleware is fail-soft: parse/resolve failure injects a note, never fails the run.
- Spec decision 14 refinement (discovered during planning): the middleware port cannot append blocks to canonical messages. The equivalent mechanism is `StageOutcome::Replace` of the `BeforeModel` `ModelRequestDraft` — File blocks are replaced with Text blocks *in the model-visible request only*; canonical journaled messages keep the `File` blocks untouched. This also prevents providers (which reject media blocks) from erroring.
- Run full verification before claiming any task complete: `cargo test -p <crate>` for the touched crate, and `cargo clippy --workspace --all-targets` at the end of each task.
- Commit after every task with the step's exact `git add` list — the working tree contains unrelated in-flight changes; NEVER `git add -A` or `git add .`.

---

### Task 1: Fixture corpus

**Files:**
- Create: `scripts/gen_document_fixtures.py`
- Create: `fixtures/documents/` (generated: `text.pdf`, `scanned.pdf`, `sample.docx`, `sample.xlsx`, `sample.pptx`, `sample.csv`, `corrupt.bin`)

Spec decision 23 also names a table-heavy PDF; a convincing one cannot be hand-written. If no local tool can generate one deterministically in a few minutes, skip it, note the omission in the Task 10 changelog commit message, and flag it to the human partner — table rendering is then covered only by anydoc's own upstream corpus. The oversized-input fixture is not checked in; oversize cases construct large buffers in-test.

**Interfaces:**
- Produces: fixture files consumed by every later test task via `include_bytes!("../../../../fixtures/documents/<name>")` (path depth from `extensions/<kind>/<crate>/src/`).

- [ ] **Step 1: Write the generator script**

```python
#!/usr/bin/env python3
"""Generate the checked-in document fixture corpus. Deterministic output."""
from __future__ import annotations

import io
import zipfile
from pathlib import Path

OUT = Path(__file__).resolve().parent.parent / "fixtures" / "documents"

TEXT_PDF = b"""%PDF-1.4
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj
2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]/Contents 4 0 R/Resources<</Font<</F1 5 0 R>>>>>>endobj
4 0 obj<</Length 60>>stream
BT /F1 24 Tf 72 720 Td (Quarterly Revenue Report) Tj ET
endstream
endobj
5 0 obj<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>endobj
trailer<</Root 1 0 R/Size 6>>
"""

# A one-page PDF whose only content is a 1x1 image XObject: no text operators.
SCANNED_PDF = b"""%PDF-1.4
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj
2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 612 792]/Contents 4 0 R/Resources<</XObject<</Im1 5 0 R>>>>>>endobj
4 0 obj<</Length 44>>stream
q 612 0 0 792 0 0 cm /Im1 Do Q
endstream
endobj
5 0 obj<</Type/XObject/Subtype/Image/Width 1/Height 1/ColorSpace/DeviceGray/BitsPerComponent 8/Length 1>>stream
\xff
endstream
endobj
trailer<</Root 1 0 R/Size 6>>
"""

CONTENT_TYPES = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"""

ROOT_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"""

DOCUMENT_XML = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
<w:body><w:p><w:r><w:t>Hello from a docx fixture.</w:t></w:r></w:p></w:body></w:document>"""

XLSX_CONTENT_TYPES = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>
<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>
</Types>"""

XLSX_ROOT_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>
</Relationships>"""

WORKBOOK_XML = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"""

WORKBOOK_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>
</Relationships>"""

SHEET_XML = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">
<sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>Revenue</t></is></c><c r="B1"><v>1250</v></c></row></sheetData></worksheet>"""


def zip_bytes(parts: dict[str, str]) -> bytes:
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w", zipfile.ZIP_DEFLATED) as archive:
        for name, content in parts.items():
            info = zipfile.ZipInfo(name, date_time=(2026, 1, 1, 0, 0, 0))
            archive.writestr(info, content)
    return buffer.getvalue()


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "text.pdf").write_bytes(TEXT_PDF)
    (OUT / "scanned.pdf").write_bytes(SCANNED_PDF)
    (OUT / "sample.docx").write_bytes(zip_bytes({
        "[Content_Types].xml": CONTENT_TYPES,
        "_rels/.rels": ROOT_RELS,
        "word/document.xml": DOCUMENT_XML,
    }))
    (OUT / "sample.xlsx").write_bytes(zip_bytes({
        "[Content_Types].xml": XLSX_CONTENT_TYPES,
        "_rels/.rels": XLSX_ROOT_RELS,
        "xl/workbook.xml": WORKBOOK_XML,
        "xl/_rels/workbook.xml.rels": WORKBOOK_RELS,
        "xl/worksheets/sheet1.xml": SHEET_XML,
    }))
    (OUT / "sample.pptx").write_bytes(zip_bytes({
        "[Content_Types].xml": """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
<Default Extension="xml" ContentType="application/xml"/>
<Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/>
<Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/>
</Types>""",
        "_rels/.rels": """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>""",
        "ppt/presentation.xml": """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
<p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst></p:presentation>""",
        "ppt/_rels/presentation.xml.rels": """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/>
</Relationships>""",
        "ppt/slides/slide1.xml": """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
<p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>
<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/>
<p:txBody><a:bodyPr/><a:p><a:r><a:t>Slide fixture title</a:t></a:r></a:p></p:txBody></p:sp>
</p:spTree></p:cSld></p:sld>""",
    }))
    (OUT / "sample.csv").write_bytes(b"quarter,revenue\nQ1,1250\nQ2,1310\n")
    (OUT / "corrupt.bin").write_bytes(b"\x00\x01corrupt-not-a-document\x02\x03")
    print(f"wrote fixtures to {OUT}")


if __name__ == "__main__":
    main()
```

- [ ] **Step 2: Run it and inspect output**

Run: `python3 scripts/gen_document_fixtures.py && ls -la fixtures/documents/`
Expected: seven files, each under 4 KB.

- [ ] **Step 3: Sanity-check the PDFs parse**

Run: `python3 -c "print(open('fixtures/documents/text.pdf','rb').read(8))"`
Expected: `b'%PDF-1.4'`. (Full parser validation happens in Task 2's tests; if anydoc later rejects a hand-written fixture, fix the fixture bytes in the generator, not the parser.)

- [ ] **Step 4: Commit**

```bash
git add scripts/gen_document_fixtures.py fixtures/documents/
git commit -m "test: add document fixture corpus for ingestion extension"
```

---

### Task 2: `finstack-ai-tools-document` crate scaffold and `parser` module

**Files:**
- Create: `extensions/toolsets/finstack-ai-tools-document/Cargo.toml`
- Create: `extensions/toolsets/finstack-ai-tools-document/src/lib.rs` (module decls + error codes only for now)
- Create: `extensions/toolsets/finstack-ai-tools-document/src/parser.rs`
- Create: `extensions/toolsets/finstack-ai-tools-document/src/tests.rs`
- Modify: `Cargo.toml` (workspace `members` — add after the `finstack-ai-tools-calculator` line; and `[workspace.dependencies]` — add `anydoc` and `pdf-inspector`)

**Interfaces:**
- Produces (used by Tasks 3–5, 8, 9):
  - `parser::DocumentLimits { max_input_bytes: u64, max_output_bytes: u64, max_pages: u32 }` with `Default` (4 MiB / 1 MiB / 500)
  - `parser::DocumentFormat` enum (`Pdf, Docx, Doc, Pptx, Ppt, Xlsx, Xls, Odt, Ods, Odp, Rtf, Epub, Csv, Unknown`) with `fn from_media_type(&str) -> Self` and `fn is_supported_media_type(&str) -> bool`
  - `parser::DocumentClassification` enum (`Text, Scanned, Mixed, Image`)
  - `parser::ParsedDocument { markdown: String, format: DocumentFormat, page_count: Option<u32>, classification: Option<DocumentClassification>, requires_ocr: bool, truncated: bool }`
  - `parser::DocumentParseError` enum (`TooLarge { len: usize, max: u64 }, UnsupportedFormat, ParseFailed { message: String }`)
  - `parser::parse(bytes: &[u8], media_type_hint: Option<&str>, limits: &DocumentLimits) -> Result<ParsedDocument, DocumentParseError>`
  - `parser::classify_pdf(bytes: &[u8]) -> Result<(DocumentClassification, u32), DocumentParseError>`

- [ ] **Step 1: Add workspace entries**

In root `Cargo.toml` `members`, after `"extensions/toolsets/finstack-ai-tools-calculator",` add:

```toml
    "extensions/toolsets/finstack-ai-tools-document",
```

In `[workspace.dependencies]` (keep the section's existing alphabetical/грouping style; place near other third-party deps):

```toml
anydoc = "0.1"
pdf-inspector = { version = "1.14", default-features = false }
```

Note: pin `pdf-inspector` to the same minor anydoc pins (check with `cargo tree -p anydoc -i pdf-inspector` after first build; if anydoc pins 1.14.2, use `"1.14"` so one copy builds).

- [ ] **Step 2: Create crate `Cargo.toml`**

```toml
[package]
name = "finstack-ai-tools-document"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
authors.workspace = true
description = "Document-to-Markdown ingestion toolset (anydoc + pdf-inspector)"

[dependencies]
finstack-ai-runtime = { path = "../../../crates/finstack-ai-runtime", default-features = false }
anydoc.workspace = true
pdf-inspector.workspace = true
futures-util = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }

[lints]
workspace = true
```

(Match the exact dependency-table style of `extensions/toolsets/finstack-ai-tools-calculator/Cargo.toml` — if that crate writes `futures-util.workspace = true`, use that form.)

- [ ] **Step 3: Write failing parser tests**

`src/lib.rs` (initial):

```rust
//! Document-to-Markdown ingestion toolset built on anydoc and pdf-inspector.

#![warn(missing_docs)]

pub mod parser;

#[cfg(test)]
mod tests;
```

`src/tests.rs` (initial content — extended in later tasks):

```rust
use crate::parser::{
    self, DocumentClassification, DocumentFormat, DocumentLimits, DocumentParseError,
};

const TEXT_PDF: &[u8] = include_bytes!("../../../../fixtures/documents/text.pdf");
const SCANNED_PDF: &[u8] = include_bytes!("../../../../fixtures/documents/scanned.pdf");
const SAMPLE_DOCX: &[u8] = include_bytes!("../../../../fixtures/documents/sample.docx");
const SAMPLE_XLSX: &[u8] = include_bytes!("../../../../fixtures/documents/sample.xlsx");
const SAMPLE_CSV: &[u8] = include_bytes!("../../../../fixtures/documents/sample.csv");
const CORRUPT: &[u8] = include_bytes!("../../../../fixtures/documents/corrupt.bin");

#[test]
fn parses_text_pdf_to_markdown() {
    let parsed = parser::parse(TEXT_PDF, Some("application/pdf"), &DocumentLimits::default())
        .expect("text pdf parses");
    assert_eq!(parsed.format, DocumentFormat::Pdf);
    assert!(parsed.markdown.contains("Quarterly Revenue Report"));
    assert!(!parsed.requires_ocr);
    assert!(!parsed.truncated);
}

#[test]
fn scanned_pdf_is_success_with_requires_ocr() {
    let parsed = parser::parse(SCANNED_PDF, Some("application/pdf"), &DocumentLimits::default())
        .expect("scanned pdf is a successful parse");
    assert!(parsed.requires_ocr);
    assert_eq!(parsed.classification, Some(DocumentClassification::Scanned));
}

#[test]
fn parses_docx_xlsx_csv() {
    for (bytes, media, format) in [
        (
            SAMPLE_DOCX,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            DocumentFormat::Docx,
        ),
        (
            SAMPLE_XLSX,
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            DocumentFormat::Xlsx,
        ),
        (SAMPLE_CSV, "text/csv", DocumentFormat::Csv),
    ] {
        let parsed = parser::parse(bytes, Some(media), &DocumentLimits::default())
            .expect("fixture parses");
        assert_eq!(parsed.format, format);
        assert!(!parsed.markdown.is_empty());
    }
}

#[test]
fn detected_format_wins_over_wrong_media_type() {
    let parsed = parser::parse(SAMPLE_DOCX, Some("application/pdf"), &DocumentLimits::default())
        .expect("content sniffing wins");
    assert_eq!(parsed.format, DocumentFormat::Docx);
}

#[test]
fn corrupt_bytes_fail_with_parse_or_unsupported() {
    let error = parser::parse(CORRUPT, None, &DocumentLimits::default())
        .expect_err("corrupt bytes must not parse");
    assert!(matches!(
        error,
        DocumentParseError::ParseFailed { .. } | DocumentParseError::UnsupportedFormat
    ));
}

#[test]
fn oversized_input_is_rejected_not_truncated() {
    let limits = DocumentLimits {
        max_input_bytes: 16,
        ..DocumentLimits::default()
    };
    assert!(matches!(
        parser::parse(SAMPLE_CSV, Some("text/csv"), &limits),
        Err(DocumentParseError::TooLarge { .. })
    ));
}

#[test]
fn oversized_output_is_truncated_with_flag() {
    let limits = DocumentLimits {
        max_output_bytes: 8,
        ..DocumentLimits::default()
    };
    let parsed = parser::parse(SAMPLE_CSV, Some("text/csv"), &limits).expect("parses");
    assert!(parsed.truncated);
    assert!(parsed.markdown.len() <= 8);
}

#[test]
fn classify_pdf_distinguishes_text_and_scanned() {
    let (text_class, pages) = parser::classify_pdf(TEXT_PDF).expect("classifies");
    assert_eq!(text_class, DocumentClassification::Text);
    assert_eq!(pages, 1);
    let (scanned_class, _) = parser::classify_pdf(SCANNED_PDF).expect("classifies");
    assert_eq!(scanned_class, DocumentClassification::Scanned);
}

#[test]
fn classify_rejects_non_pdf() {
    assert!(matches!(
        parser::classify_pdf(SAMPLE_DOCX),
        Err(DocumentParseError::UnsupportedFormat)
    ));
}
```

- [ ] **Step 4: Run tests to verify they fail to compile (parser module absent)**

Run: `cargo test -p finstack-ai-tools-document`
Expected: FAIL — `parser` module items unresolved.

- [ ] **Step 5: Implement `src/parser.rs`**

```rust
//! Shared parsing engine over anydoc and pdf-inspector.
//!
//! This module is the crate's churn boundary: no anydoc or pdf-inspector
//! type appears in its public signatures.

use thiserror::Error;

/// Byte, output, and page ceilings for one parse.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentLimits {
    /// Reject inputs above this size; aligned with `MAX_ARTIFACT_BYTES`.
    pub max_input_bytes: u64,
    /// Truncate Markdown above this size and set `truncated`.
    pub max_output_bytes: u64,
    /// Reject PDFs above this page count.
    pub max_pages: u32,
}

impl Default for DocumentLimits {
    fn default() -> Self {
        Self {
            max_input_bytes: 4 * 1024 * 1024,
            max_output_bytes: 1024 * 1024,
            max_pages: 500,
        }
    }
}

/// Detected document format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum DocumentFormat {
    Pdf,
    Docx,
    Doc,
    Pptx,
    Ppt,
    Xlsx,
    Xls,
    Odt,
    Ods,
    Odp,
    Rtf,
    Epub,
    Csv,
    Unknown,
}

impl DocumentFormat {
    /// Map a declared media type to a format hint.
    #[must_use]
    pub fn from_media_type(media_type: &str) -> Self {
        match media_type {
            "application/pdf" => Self::Pdf,
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => Self::Docx,
            "application/msword" => Self::Doc,
            "application/vnd.openxmlformats-officedocument.presentationml.presentation" => {
                Self::Pptx
            }
            "application/vnd.ms-powerpoint" => Self::Ppt,
            "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet" => Self::Xlsx,
            "application/vnd.ms-excel" => Self::Xls,
            "application/vnd.oasis.opendocument.text" => Self::Odt,
            "application/vnd.oasis.opendocument.spreadsheet" => Self::Ods,
            "application/vnd.oasis.opendocument.presentation" => Self::Odp,
            "application/rtf" | "text/rtf" => Self::Rtf,
            "application/epub+zip" => Self::Epub,
            "text/csv" => Self::Csv,
            _ => Self::Unknown,
        }
    }

    /// Whether the ingest middleware should attempt this media type.
    #[must_use]
    pub fn is_supported_media_type(media_type: &str) -> bool {
        Self::from_media_type(media_type) != Self::Unknown
    }
}

/// PDF page-content classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
#[allow(missing_docs)]
pub enum DocumentClassification {
    Text,
    Scanned,
    Mixed,
    Image,
}

/// One successful parse.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ParsedDocument {
    /// GitHub-Flavored Markdown output (possibly truncated).
    pub markdown: String,
    /// Detected format; the declared media type is only a hint.
    pub format: DocumentFormat,
    /// Page count when the format has pages.
    pub page_count: Option<u32>,
    /// Classification for PDFs only.
    pub classification: Option<DocumentClassification>,
    /// Whether recovering full text needs OCR (scanned/image PDFs).
    pub requires_ocr: bool,
    /// Whether `markdown` was cut at `max_output_bytes`.
    pub truncated: bool,
}

/// Parse failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DocumentParseError {
    /// Input exceeds `max_input_bytes` or `max_pages`.
    #[error("document_too_large: {len} bytes exceeds {max}")]
    TooLarge {
        /// Submitted length.
        len: usize,
        /// Configured ceiling.
        max: u64,
    },
    /// No supported format detected.
    #[error("document_unsupported_format")]
    UnsupportedFormat,
    /// The detected format's parser rejected the content.
    #[error("document_parse_failed: {message}")]
    ParseFailed {
        /// Bounded non-secret diagnostic.
        message: String,
    },
}

/// Parse one document to Markdown.
///
/// Format is detected from content first; `media_type_hint` breaks ties for
/// signature-less formats (csv/rtf). A scanned PDF is a success with
/// `requires_ocr` set.
///
/// # Errors
///
/// Returns [`DocumentParseError`] for oversized, unsupported, or unparseable
/// input.
pub fn parse(
    bytes: &[u8],
    media_type_hint: Option<&str>,
    limits: &DocumentLimits,
) -> Result<ParsedDocument, DocumentParseError> {
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limits.max_input_bytes {
        return Err(DocumentParseError::TooLarge {
            len: bytes.len(),
            max: limits.max_input_bytes,
        });
    }
    let hinted = media_type_hint.map(DocumentFormat::from_media_type);
    let format = detect_format(bytes, hinted);
    if format == DocumentFormat::Unknown {
        return Err(DocumentParseError::UnsupportedFormat);
    }

    let (classification, page_count, requires_ocr) = if format == DocumentFormat::Pdf {
        let (classification, pages) = classify_pdf(bytes)?;
        if pages > limits.max_pages {
            return Err(DocumentParseError::TooLarge {
                len: bytes.len(),
                max: u64::from(limits.max_pages),
            });
        }
        let requires_ocr = matches!(
            classification,
            DocumentClassification::Scanned | DocumentClassification::Image
        );
        (Some(classification), Some(pages), requires_ocr)
    } else {
        (None, None, false)
    };

    let markdown = anydoc::to_markdown_bytes(bytes, anydoc_format(format))
        .map_err(|error| match (requires_ocr, format) {
            // A scanned PDF may yield no extractable text; that is a success.
            (true, DocumentFormat::Pdf) => DocumentParseError::ParseFailed {
                message: String::new(),
            },
            _ => DocumentParseError::ParseFailed {
                message: bounded_message(&error.to_string()),
            },
        });
    let markdown = match markdown {
        Ok(text) => text,
        Err(DocumentParseError::ParseFailed { message }) if message.is_empty() && requires_ocr => {
            String::new()
        }
        Err(error) => return Err(error),
    };

    let (markdown, truncated) = truncate_utf8(markdown, limits.max_output_bytes);
    Ok(ParsedDocument {
        markdown,
        format,
        page_count,
        classification,
        requires_ocr,
        truncated,
    })
}

/// Classify a PDF without full extraction.
///
/// # Errors
///
/// Returns `UnsupportedFormat` for non-PDF bytes and `ParseFailed` for a
/// broken PDF.
pub fn classify_pdf(bytes: &[u8]) -> Result<(DocumentClassification, u32), DocumentParseError> {
    if !bytes.starts_with(b"%PDF-") {
        return Err(DocumentParseError::UnsupportedFormat);
    }
    let classification = pdf_inspector::classify_pdf_mem(bytes).map_err(|error| {
        DocumentParseError::ParseFailed {
            message: bounded_message(&error.to_string()),
        }
    })?;
    Ok((
        map_pdf_type(&classification),
        page_count_of(&classification),
    ))
}

fn detect_format(bytes: &[u8], hint: Option<DocumentFormat>) -> DocumentFormat {
    if let Some(detected) = anydoc::Format::from_bytes(bytes) {
        return from_anydoc(detected);
    }
    hint.unwrap_or(DocumentFormat::Unknown)
}

fn truncate_utf8(mut text: String, max_bytes: u64) -> (String, bool) {
    let max = usize::try_from(max_bytes).unwrap_or(usize::MAX);
    if text.len() <= max {
        return (text, false);
    }
    let mut cut = max;
    while cut > 0 && !text.is_char_boundary(cut) {
        cut -= 1;
    }
    text.truncate(cut);
    (text, true)
}

fn bounded_message(message: &str) -> String {
    message.chars().take(256).collect()
}
```

**Upstream-API adapters** (`anydoc_format`, `from_anydoc`, `map_pdf_type`, `page_count_of`): the exact `anydoc::Format` variant names, `to_markdown_bytes` option type, and `pdf_inspector::PdfClassification` field/variant names must be confirmed against the built crates — run `cargo doc -p anydoc -p pdf-inspector --no-deps` and read `target/doc`. Write the four small adapter functions to match what you find (each is a straight `match`/field-read of ≤15 lines; e.g. `map_pdf_type` maps pdf-inspector's text/scanned/mixed/image kind to `DocumentClassification`, `page_count_of` reads its page-count field). If `to_markdown_bytes` returns bytes rather than `String`, convert with `String::from_utf8_lossy(...).into_owned()`. Do not redesign the public `parser` API to match upstream — adapt inward.

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test -p finstack-ai-tools-document`
Expected: all Task 2 tests PASS. If a fixture is rejected by anydoc (hand-written PDFs can be too minimal), adjust `scripts/gen_document_fixtures.py` output until it parses, regenerate, and re-run — the fixture serves the parser, not vice versa.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock extensions/toolsets/finstack-ai-tools-document/ fixtures/documents/ scripts/gen_document_fixtures.py
git commit -m "feat: add finstack-ai-tools-document parser module over anydoc"
```

---

### Task 3: DocumentToolset specs and descriptor

**Files:**
- Modify: `extensions/toolsets/finstack-ai-tools-document/src/lib.rs`
- Modify: `extensions/toolsets/finstack-ai-tools-document/src/tests.rs`

**Interfaces:**
- Produces (used by Tasks 4, 8, 9): `DocumentToolset::try_new() -> Result<Self, DocumentError>`, `DocumentToolset::with_artifact_store(self, Arc<dyn ArtifactStore>) -> Self`, error-code constants `DOCUMENT_INVALID_ARGUMENTS`, `DOCUMENT_PARSE_FAILED`, `DOCUMENT_TOO_LARGE`, `DOCUMENT_SOURCE_UNAVAILABLE`, `DOCUMENT_UNSUPPORTED_FORMAT`, `DOCUMENT_PATH_UNSUPPORTED`, plus `DocumentError` (`Configuration { reason: &'static str }`).

- [ ] **Step 1: Write failing tests**

Append to `src/tests.rs`:

```rust
use crate::DocumentToolset;
use finstack_ai_runtime::Toolset as _;

#[test]
fn toolset_exposes_two_validated_tools() {
    let toolset = DocumentToolset::try_new().expect("toolset");
    let tools = toolset.tools();
    assert_eq!(tools.len(), 2);
    let names: Vec<&str> = tools.iter().map(|spec| spec.model_name.as_ref()).collect();
    assert_eq!(names, ["document_parse", "pdf_classify"]);
    for spec in tools.iter() {
        spec.validate().expect("spec validates");
        assert_eq!(spec.id.as_str(), "finstack.tools.document");
    }
    assert_eq!(toolset.descriptor().name.as_ref(), "finstack-document");
}
```

(If `ToolId` has no `as_str`, compare via `spec.id == ToolId::parse("finstack.tools.document").unwrap()`.)

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-tools-document toolset_exposes`
Expected: FAIL — `DocumentToolset` unresolved.

- [ ] **Step 3: Implement in `src/lib.rs`**

Follow `finstack-ai-tools-calculator/src/lib.rs` structure exactly. Content:

```rust
//! Document-to-Markdown ingestion toolset built on anydoc and pdf-inspector.

#![warn(missing_docs)]

pub mod parser;
mod source;
mod toolset;

pub use source::DocumentSource;
pub use toolset::{DocumentError, DocumentToolset};

/// Stable invalid-argument code.
pub const DOCUMENT_INVALID_ARGUMENTS: &str = "document_invalid_arguments";
/// Stable parse-failure code.
pub const DOCUMENT_PARSE_FAILED: &str = "document_parse_failed";
/// Stable oversized-input code.
pub const DOCUMENT_TOO_LARGE: &str = "document_too_large";
/// Stable unresolvable-source code.
pub const DOCUMENT_SOURCE_UNAVAILABLE: &str = "document_source_unavailable";
/// Stable unsupported-format code.
pub const DOCUMENT_UNSUPPORTED_FORMAT: &str = "document_unsupported_format";
/// Stable path-source-unsupported-on-target code.
pub const DOCUMENT_PATH_UNSUPPORTED: &str = "document_path_unsupported";

#[cfg(test)]
mod tests;
```

Create `src/toolset.rs` with the toolset skeleton (call dispatch lands in Task 4):

```rust
//! Toolset identity, specs, and construction.

use std::sync::Arc;

use finstack_ai_runtime::{
    ApprovalMetadata, ApprovalRequirement, ArtifactStore, Metadata, RawJson, RetrySafety,
    SideEffectClass, ToolDeferralSupport, ToolExecutionMode, ToolId, ToolSpec, ToolsetDescriptor,
};
use thiserror::Error;

use crate::parser::DocumentLimits;

pub(crate) const TOOL_ID: &str = "finstack.tools.document";
pub(crate) const PARSE_NAME: &str = "document_parse";
pub(crate) const CLASSIFY_NAME: &str = "pdf_classify";

const PARSE_INPUT_SCHEMA: &[u8] = br#"{"additionalProperties":false,"properties":{"artifact":{"description":"Staged artifact reference JSON for the document bytes.","type":"object"},"path":{"description":"Absolute filesystem path (native hosts only).","type":"string"},"media_type_hint":{"type":"string"},"page_range":{"description":"1-based inclusive page range, PDFs only.","items":{"minimum":1,"type":"integer"},"maxItems":2,"minItems":2,"type":"array"},"max_output_bytes":{"description":"Per-call output ceiling, clamped to the configured limit.","minimum":1,"type":"integer"}},"type":"object"}"#;
const PARSE_OUTPUT_SCHEMA: &[u8] = br#"{"additionalProperties":false,"properties":{"markdown":{"type":"string"},"format":{"type":"string"},"page_count":{"type":["integer","null"]},"classification":{"type":["string","null"]},"requires_ocr":{"type":"boolean"},"truncated":{"type":"boolean"},"spilled_artifact":{"type":["object","null"]}},"required":["markdown","format","requires_ocr","truncated"],"type":"object"}"#;
const CLASSIFY_INPUT_SCHEMA: &[u8] = br#"{"additionalProperties":false,"properties":{"artifact":{"description":"Staged artifact reference JSON for the PDF bytes.","type":"object"},"path":{"description":"Absolute filesystem path (native hosts only).","type":"string"}},"type":"object"}"#;
const CLASSIFY_OUTPUT_SCHEMA: &[u8] = br#"{"additionalProperties":false,"properties":{"classification":{"enum":["text","scanned","mixed","image"],"type":"string"},"page_count":{"type":"integer"},"page_stats":{"description":"Optional per-page text-coverage stats when the classifier exposes them.","items":{"type":"object"},"type":["array","null"]}},"required":["classification","page_count"],"type":"object"}"#;

/// Document toolset construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DocumentError {
    /// A checked-in identity or schema constant is invalid.
    #[error("document_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// Document toolset with two immutable cached tool specifications.
#[derive(Clone)]
pub struct DocumentToolset {
    pub(crate) descriptor: ToolsetDescriptor,
    pub(crate) tools: Arc<[ToolSpec]>,
    pub(crate) tool_id: ToolId,
    pub(crate) limits: DocumentLimits,
    pub(crate) artifact_store: Option<Arc<dyn ArtifactStore>>,
}

impl std::fmt::Debug for DocumentToolset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentToolset")
            .field("tool_id", &self.tool_id)
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl DocumentToolset {
    /// Construct the toolset with default limits and no artifact store.
    ///
    /// # Errors
    ///
    /// Returns a configuration error only if a checked-in identity or schema
    /// constant is invalid.
    pub fn try_new() -> Result<Self, DocumentError> {
        Self::try_with_limits(DocumentLimits::default())
    }

    /// Construct with explicit limits.
    ///
    /// # Errors
    ///
    /// Returns a configuration error only if a checked-in identity or schema
    /// constant is invalid.
    pub fn try_with_limits(limits: DocumentLimits) -> Result<Self, DocumentError> {
        let tool_id = ToolId::parse(TOOL_ID).map_err(|_| DocumentError::Configuration {
            reason: "invalid_tool_id",
        })?;
        let parse_spec = spec(
            &tool_id,
            PARSE_NAME,
            "Parse Document",
            "Convert an attached document (pdf, docx, xlsx, pptx, odf, rtf, epub, csv) to GitHub-Flavored Markdown. Scanned PDFs succeed with requires_ocr=true.",
            PARSE_INPUT_SCHEMA,
            PARSE_OUTPUT_SCHEMA,
            1024 * 1024,
        )?;
        let classify_spec = spec(
            &tool_id,
            CLASSIFY_NAME,
            "Classify PDF",
            "Fast PDF classification (text, scanned, mixed, image) with page count; no text extraction.",
            CLASSIFY_INPUT_SCHEMA,
            CLASSIFY_OUTPUT_SCHEMA,
            4 * 1024,
        )?;
        Ok(Self {
            descriptor: ToolsetDescriptor {
                name: Arc::from("finstack-document"),
                metadata: Metadata::empty(),
            },
            tools: Arc::from([parse_spec, classify_spec]),
            tool_id,
            limits,
            artifact_store: None,
        })
    }

    /// Attach the artifact store used for `artifact` sources and output spill.
    #[must_use]
    pub fn with_artifact_store(mut self, store: Arc<dyn ArtifactStore>) -> Self {
        self.artifact_store = Some(store);
        self
    }
}

fn spec(
    tool_id: &ToolId,
    name: &str,
    title: &str,
    description: &str,
    input_schema: &[u8],
    output_schema: &[u8],
    max_result_bytes: u32,
) -> Result<ToolSpec, DocumentError> {
    let spec = ToolSpec {
        id: tool_id.clone(),
        model_name: Arc::from(name),
        title: Arc::from(title),
        description: Arc::from(description),
        input_schema: RawJson::parse(input_schema).map_err(|_| DocumentError::Configuration {
            reason: "invalid_input_schema",
        })?,
        output_schema: Some(RawJson::parse(output_schema).map_err(|_| {
            DocumentError::Configuration {
                reason: "invalid_output_schema",
            }
        })?),
        execution: ToolExecutionMode::Parallel,
        side_effect: SideEffectClass::ReadOnly,
        retry_safety: RetrySafety::SafeToRetry,
        approval: ApprovalMetadata {
            requirement: ApprovalRequirement::NotRequired,
            reason: None,
            attributes: Metadata::empty(),
        },
        max_result_bytes,
        metadata: Metadata::empty(),
        deferral: ToolDeferralSupport::Never,
    };
    spec.validate().map_err(|_| DocumentError::Configuration {
        reason: "invalid_tool_spec",
    })?;
    Ok(spec)
}
```

Also create a placeholder-free `src/source.rs` now because `lib.rs` declares it (full logic in Task 4):

```rust
//! Tool-call source union: staged artifact or native filesystem path.

use serde::Deserialize;

/// Where document bytes come from. Exactly one field must be set.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentSource {
    /// Staged artifact reference (serialized `ArtifactRef`).
    #[serde(default)]
    pub artifact: Option<serde_json::Value>,
    /// Absolute filesystem path (native hosts only).
    #[serde(default)]
    pub path: Option<String>,
}
```

If `max_result_bytes` is not `u32` in `ToolSpec`, match the calculator's field type. If `ToolSpec.max_result_bytes = 1_048_576` violates a runtime bound, lower to the largest accepted value and keep spill behavior.

- [ ] **Step 4: Run tests to verify pass**

Run: `cargo test -p finstack-ai-tools-document`
Expected: PASS (all tests including Task 2's).

- [ ] **Step 5: Commit**

```bash
git add extensions/toolsets/finstack-ai-tools-document/
git commit -m "feat: add DocumentToolset specs and descriptor"
```

---

### Task 4: DocumentToolset call dispatch

**Files:**
- Modify: `extensions/toolsets/finstack-ai-tools-document/src/toolset.rs` (add `impl Toolset`)
- Modify: `extensions/toolsets/finstack-ai-tools-document/src/source.rs` (resolution logic)
- Modify: `extensions/toolsets/finstack-ai-tools-document/src/tests.rs`

**Interfaces:**
- Consumes: `parser::parse`, `parser::classify_pdf` (Task 2); `DocumentToolset` fields (Task 3).
- Produces: working `Toolset` impl dispatching `document_parse` and `pdf_classify`; `document_parse` output JSON `{markdown, format, page_count, classification, requires_ocr, truncated, spilled_artifact}`.

- [ ] **Step 1: Write failing tests**

Append to `src/tests.rs`. Build `ToolCallContext`/`ValidatedToolCall` the same way `extensions/toolsets/finstack-ai-tools-calculator/src/tests.rs` builds them — copy its helper (`fn call_context()` / `fn validated_call(name, args)`) verbatim, and reuse the `CaptureArtifactStore` test double from `extensions/toolsets/finstack-ai-tools-filesystem/src/tests.rs:433` (copy it; test doubles are not shared across crates).

```rust
#[test]
fn document_parse_via_artifact_source_returns_markdown() {
    let store = Arc::new(CaptureArtifactStore::default());
    let scope = test_scope(); // same tenant/session/run as call_context()
    let artifact = block_on(finstack_ai_runtime::stage_required_artifact(
        store.as_ref(),
        scope,
        Bytes::copy_from_slice(SAMPLE_CSV),
        ArtifactMetadata {
            kind: Arc::from("attachment"),
            media_type: Arc::from("text/csv"),
            name: Some(Arc::from("sample.csv")),
            attributes: Metadata::empty(),
        },
    ))
    .expect("staged");
    let toolset = DocumentToolset::try_new()
        .expect("toolset")
        .with_artifact_store(store);
    let arguments = serde_json::json!({
        "artifact": serde_json::to_value(&artifact).expect("artifact json"),
    });
    let output = call_tool(&toolset, "document_parse", &arguments);
    assert!(output["markdown"].as_str().expect("markdown").contains("Q1"));
    assert_eq!(output["format"], "csv");
    assert_eq!(output["requires_ocr"], false);
}

#[test]
fn document_parse_without_store_rejects_artifact_source() {
    let toolset = DocumentToolset::try_new().expect("toolset");
    let arguments = serde_json::json!({"artifact": {}});
    let error = call_tool_err(&toolset, "document_parse", &arguments);
    assert_eq!(error.code(), crate::DOCUMENT_SOURCE_UNAVAILABLE);
}

#[test]
fn pdf_classify_via_artifact_source() {
    let store = Arc::new(CaptureArtifactStore::default());
    let artifact = stage(store.as_ref(), SCANNED_PDF, "application/pdf", "scan.pdf");
    let toolset = DocumentToolset::try_new()
        .expect("toolset")
        .with_artifact_store(store);
    let arguments =
        serde_json::json!({"artifact": serde_json::to_value(&artifact).expect("json")});
    let output = call_tool(&toolset, "pdf_classify", &arguments);
    assert_eq!(output["classification"], "scanned");
    assert_eq!(output["page_count"], 1);
}

#[cfg(unix)]
#[test]
fn document_parse_via_path_source() {
    let dir = std::env::temp_dir().join("finstack-tools-document-test");
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("sample.csv");
    std::fs::write(&path, SAMPLE_CSV).expect("write");
    let toolset = DocumentToolset::try_new().expect("toolset");
    let arguments = serde_json::json!({"path": path.to_string_lossy()});
    let output = call_tool(&toolset, "document_parse", &arguments);
    assert_eq!(output["format"], "csv");
}

#[test]
fn both_or_neither_source_is_invalid() {
    let toolset = DocumentToolset::try_new().expect("toolset");
    for arguments in [
        serde_json::json!({}),
        serde_json::json!({"artifact": {}, "path": "/tmp/x.pdf"}),
    ] {
        let error = call_tool_err(&toolset, "document_parse", &arguments);
        assert_eq!(error.code(), crate::DOCUMENT_INVALID_ARGUMENTS);
    }
}

#[test]
fn unsupported_format_maps_to_stable_code() {
    let store = Arc::new(CaptureArtifactStore::default());
    let artifact = stage(store.as_ref(), CORRUPT, "application/octet-stream", "x.bin");
    let toolset = DocumentToolset::try_new()
        .expect("toolset")
        .with_artifact_store(store);
    let arguments =
        serde_json::json!({"artifact": serde_json::to_value(&artifact).expect("json")});
    let error = call_tool_err(&toolset, "document_parse", &arguments);
    assert!(matches!(
        error.code(),
        crate::DOCUMENT_UNSUPPORTED_FORMAT | crate::DOCUMENT_PARSE_FAILED
    ));
}
```

Add the small shared helpers in `tests.rs`: `fn stage(store, bytes, media, name) -> ArtifactRef` (wraps `stage_required_artifact` with `test_scope()`), `fn call_tool(toolset, name, args) -> serde_json::Value` (drives `toolset.call(...)`, polls the stream with the same `block_on` helper style as `crates/finstack-ai-runtime/src/services/artifact.rs` tests, asserts `is_error == false`, parses `output`), and `fn call_tool_err(...) -> ToolError` (expects `Err`). Base `call_context()`/`ValidatedToolCall` construction on the calculator's tests — copy, don't invent.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai-tools-document document_parse_via_artifact`
Expected: FAIL — no `Toolset` impl.

- [ ] **Step 3: Implement dispatch**

In `src/source.rs`, add resolution:

```rust
use std::sync::Arc;

use finstack_ai_runtime::{ArtifactRef, ArtifactScope, ArtifactStore, ToolCallContext};

use crate::parser::DocumentLimits;

pub(crate) enum ResolvedSource {
    Bytes {
        bytes: Vec<u8>,
        media_type_hint: Option<String>,
        name: Option<String>,
    },
}

pub(crate) async fn resolve(
    source: &DocumentSource,
    ctx: &ToolCallContext,
    store: Option<&Arc<dyn ArtifactStore>>,
    limits: &DocumentLimits,
) -> Result<ResolvedSource, SourceError> {
    match (&source.artifact, &source.path) {
        (Some(artifact_json), None) => {
            let store = store.ok_or(SourceError::Unavailable("artifact_store_not_configured"))?;
            let artifact: ArtifactRef = serde_json::from_value(artifact_json.clone())
                .map_err(|_| SourceError::InvalidArguments("artifact_reference_invalid"))?;
            let scope = call_scope(ctx);
            let bytes = store
                .get(scope, artifact.clone())
                .await
                .map_err(|_| SourceError::Unavailable("artifact_get_failed"))?;
            if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limits.max_input_bytes {
                return Err(SourceError::TooLarge(bytes.len()));
            }
            Ok(ResolvedSource::Bytes {
                bytes: bytes.to_vec(),
                media_type_hint: Some(artifact.blob().media_type().to_owned()),
                name: artifact.blob().name().map(str::to_owned),
            })
        }
        (None, Some(path)) => resolve_path(path, limits).await,
        _ => Err(SourceError::InvalidArguments("exactly_one_source_required")),
    }
}

#[cfg(unix)]
async fn resolve_path(path: &str, limits: &DocumentLimits) -> Result<ResolvedSource, SourceError> {
    let path = path.to_owned();
    let max = limits.max_input_bytes;
    let bytes = std::fs::metadata(&path)
        .ok()
        .filter(|meta| meta.is_file() && meta.len() <= max)
        .and_then(|_| std::fs::read(&path).ok())
        .ok_or(SourceError::Unavailable("path_unreadable_or_oversized"))?;
    Ok(ResolvedSource::Bytes {
        bytes,
        media_type_hint: None,
        name: std::path::Path::new(&path)
            .file_name()
            .map(|name| name.to_string_lossy().into_owned()),
    })
}

#[cfg(not(unix))]
async fn resolve_path(_path: &str, _limits: &DocumentLimits) -> Result<ResolvedSource, SourceError> {
    Err(SourceError::PathUnsupported)
}

pub(crate) enum SourceError {
    InvalidArguments(&'static str),
    Unavailable(&'static str),
    TooLarge(usize),
    PathUnsupported,
}

fn call_scope(ctx: &ToolCallContext) -> ArtifactScope {
    // Mirror how finstack-ai-tools-filesystem builds its spill scope from
    // ToolCallContext (tenant scope + session + run + sensitivity). Copy that
    // construction exactly — grep for ArtifactScope in that crate.
    todo_scope_from(ctx)
}
```

For `call_scope`, copy the exact construction used by `finstack-ai-tools-filesystem` (grep `ArtifactScope` there) — the fields come from `ctx.run.locator` and the context's sensitivity; do not guess field paths, read them. Replace `todo_scope_from` with that real construction (the name here only marks where it goes; the committed code must not contain any `todo` identifier).

If synchronous `std::fs` in an async context conflicts with the crate being wasm-clean, note: this code path is `cfg(unix)` only; the non-unix build contains no `std::fs` use. Avoid `tokio` — plain sync reads inside the boxed future match the bounded 4 MiB ceiling.

In `src/toolset.rs`, add:

```rust
impl finstack_ai_runtime::Toolset for DocumentToolset {
    fn descriptor(&self) -> ToolsetDescriptor {
        self.descriptor.clone()
    }

    fn tools(&self) -> Arc<[ToolSpec]> {
        Arc::clone(&self.tools)
    }

    fn call(
        &self,
        ctx: finstack_ai_runtime::ToolCallContext,
        call: finstack_ai_runtime::ValidatedToolCall,
    ) -> finstack_ai_runtime::PortFuture<
        Result<finstack_ai_runtime::ToolEventStream, finstack_ai_runtime::ToolError>,
    > {
        let toolset = self.clone();
        Box::pin(async move {
            if call.tool_id != toolset.tool_id {
                return Err(invalid_arguments("document call identity is invalid"));
            }
            let source: crate::DocumentSource =
                serde_json::from_slice(call.call.arguments().as_bytes())
                    .map_err(|_| invalid_arguments("document arguments are invalid"))?;
            let resolved = crate::source::resolve(
                &source,
                &ctx,
                toolset.artifact_store.as_ref(),
                &toolset.limits,
            )
            .await
            .map_err(source_error)?;
            let crate::source::ResolvedSource::Bytes {
                bytes,
                media_type_hint,
                ..
            } = resolved;
            let output = match call.call.tool_name() {
                crate::toolset::PARSE_NAME => {
                    let parsed = crate::parser::parse(
                        &bytes,
                        media_type_hint.as_deref(),
                        &toolset.limits,
                    )
                    .map_err(parse_error)?;
                    serde_json::json!({
                        "markdown": parsed.markdown,
                        "format": parsed.format,
                        "page_count": parsed.page_count,
                        "classification": parsed.classification,
                        "requires_ocr": parsed.requires_ocr,
                        "truncated": parsed.truncated,
                        "spilled_artifact": serde_json::Value::Null,
                    })
                }
                crate::toolset::CLASSIFY_NAME => {
                    let (classification, page_count) =
                        crate::parser::classify_pdf(&bytes).map_err(parse_error)?;
                    serde_json::json!({
                        "classification": classification,
                        "page_count": page_count,
                    })
                }
                _ => return Err(invalid_arguments("unknown document tool name")),
            };
            completed_stream(output)
        })
    }
}
```

Plus the three small helpers in `toolset.rs` (exact bodies):

```rust
fn invalid_arguments(message: &'static str) -> finstack_ai_runtime::ToolError {
    tool_error(
        crate::DOCUMENT_INVALID_ARGUMENTS,
        finstack_ai_runtime::ErrorCategory::Validation,
        message,
    )
}

fn source_error(error: crate::source::SourceError) -> finstack_ai_runtime::ToolError {
    use crate::source::SourceError;
    match error {
        SourceError::InvalidArguments(message) => tool_error(
            crate::DOCUMENT_INVALID_ARGUMENTS,
            finstack_ai_runtime::ErrorCategory::Validation,
            message,
        ),
        SourceError::Unavailable(message) => tool_error(
            crate::DOCUMENT_SOURCE_UNAVAILABLE,
            finstack_ai_runtime::ErrorCategory::Tool,
            message,
        ),
        SourceError::TooLarge(_) => tool_error(
            crate::DOCUMENT_TOO_LARGE,
            finstack_ai_runtime::ErrorCategory::Validation,
            "document input exceeds the byte ceiling",
        ),
        SourceError::PathUnsupported => tool_error(
            crate::DOCUMENT_PATH_UNSUPPORTED,
            finstack_ai_runtime::ErrorCategory::Tool,
            "path sources are unsupported on this target",
        ),
    }
}

fn parse_error(error: crate::parser::DocumentParseError) -> finstack_ai_runtime::ToolError {
    use crate::parser::DocumentParseError;
    let (code, message) = match error {
        DocumentParseError::TooLarge { .. } => (
            crate::DOCUMENT_TOO_LARGE,
            "document exceeds size or page ceiling",
        ),
        DocumentParseError::UnsupportedFormat => (
            crate::DOCUMENT_UNSUPPORTED_FORMAT,
            "no supported document format detected",
        ),
        DocumentParseError::ParseFailed { .. } => {
            (crate::DOCUMENT_PARSE_FAILED, "document parsing failed")
        }
    };
    tool_error(code, finstack_ai_runtime::ErrorCategory::Tool, message)
}

fn tool_error(
    code: &'static str,
    category: finstack_ai_runtime::ErrorCategory,
    message: &'static str,
) -> finstack_ai_runtime::ToolError {
    finstack_ai_runtime::ToolError::try_new(
        code,
        category,
        false,
        message,
        finstack_ai_runtime::Metadata::empty(),
    )
    .unwrap_or_else(Into::into)
}

fn completed_stream(
    output: serde_json::Value,
) -> Result<finstack_ai_runtime::ToolEventStream, finstack_ai_runtime::ToolError> {
    let bytes = serde_json::to_vec(&output).map_err(|_| {
        tool_error(
            crate::DOCUMENT_PARSE_FAILED,
            finstack_ai_runtime::ErrorCategory::Internal,
            "document result serialization failed",
        )
    })?;
    let result = finstack_ai_runtime::ToolResult {
        output: finstack_ai_runtime::RawJson::parse(bytes).map_err(|_| {
            tool_error(
                crate::DOCUMENT_PARSE_FAILED,
                finstack_ai_runtime::ErrorCategory::Internal,
                "document result normalization failed",
            )
        })?,
        is_error: false,
    };
    Ok(Box::pin(futures_util::stream::once(async move {
        Ok(finstack_ai_runtime::ToolStreamItem::Completed(result))
    })) as finstack_ai_runtime::ToolEventStream)
}
```

Two spec-decision-6 details to fold into the dispatch (small, but required):
- **`page_range`** (deserialize as `Option<(u32, u32)>` on `DocumentSource`'s sibling argument struct): valid only when the resolved bytes are a PDF — otherwise `DOCUMENT_INVALID_ARGUMENTS`. For PDFs, implement via pdf-inspector's per-page extraction (`extract_pages_markdown_mem` — confirm the exact signature in the same `cargo doc` pass as Task 2's adapters) joined into one Markdown string; reject reversed or zero-based ranges.
- **`max_output_bytes` override**: clamp to `min(requested, toolset.limits.max_output_bytes)` and pass a per-call `DocumentLimits` copy into `parser::parse`.
- **`page_stats`**: populate from the pdf-inspector classification result if it exposes per-page coverage; otherwise emit `null` — the schema allows both.

Add tenant-scope validation identical to the calculator's `validate_call_context` (copy it, renaming the error to `invalid_arguments`). Output spill (`spilled_artifact`): when serialized output exceeds the spec's `max_result_bytes` AND an artifact store is configured, stage the full Markdown via `stage_required_artifact` with `kind: "tool-output"`, put the returned `ArtifactRef` JSON in `spilled_artifact`, and truncate inline `markdown` to fit — mirror how `finstack-ai-tools-filesystem` does this (read its spill call before writing yours).

- [ ] **Step 4: Run all crate tests**

Run: `cargo test -p finstack-ai-tools-document`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add extensions/toolsets/finstack-ai-tools-document/
git commit -m "feat: implement DocumentToolset call dispatch with artifact and path sources"
```

---

### Task 5: `finstack-ai-middleware-document-ingest` crate

**Files:**
- Create: `extensions/middleware/finstack-ai-middleware-document-ingest/Cargo.toml`
- Create: `extensions/middleware/finstack-ai-middleware-document-ingest/src/lib.rs`
- Create: `extensions/middleware/finstack-ai-middleware-document-ingest/src/tests.rs`
- Modify: `Cargo.toml` (workspace members — add after the `finstack-ai-middleware-verify` line)

**Interfaces:**
- Consumes: `finstack_ai_tools_document::parser::{parse, DocumentFormat, DocumentLimits}`; runtime `Middleware`, `StageInput::BeforeModel`, `StageOutcome::Replace`, `ArtifactStore`.
- Produces: `DocumentIngestMiddleware::try_new(Arc<dyn ArtifactStore>) -> Result<Self, DocumentIngestError>` with component id `finstack.middleware.document-ingest`, stage `BeforeModel`, tier `ContextMutation`, role `Standard`.

- [ ] **Step 1: Crate `Cargo.toml`**

```toml
[package]
name = "finstack-ai-middleware-document-ingest"
version.workspace = true
edition.workspace = true
rust-version.workspace = true
license.workspace = true
repository.workspace = true
homepage.workspace = true
authors.workspace = true
description = "BeforeModel middleware that converts attached documents to model-visible Markdown"

[dependencies]
finstack-ai-runtime = { path = "../../../crates/finstack-ai-runtime", default-features = false }
finstack-ai-kernel = { path = "../../../crates/finstack-ai-kernel" }
finstack-ai-tools-document = { path = "../../toolsets/finstack-ai-tools-document" }
serde_json = { workspace = true }
thiserror = { workspace = true }

[lints]
workspace = true
```

Add the workspace member line to root `Cargo.toml`:

```toml
    "extensions/middleware/finstack-ai-middleware-document-ingest",
```

(Match kernel-dep style: if `finstack-ai-middleware-verify/Cargo.toml` references these via workspace deps instead of paths, use its exact form.)

- [ ] **Step 2: Write failing tests**

`src/tests.rs`:

```rust
use std::sync::Arc;

use finstack_ai_runtime::{Middleware as _, StageInput, StageOutcome};

use crate::DocumentIngestMiddleware;

const SAMPLE_CSV: &[u8] = include_bytes!("../../../../fixtures/documents/sample.csv");
const SCANNED_PDF: &[u8] = include_bytes!("../../../../fixtures/documents/scanned.pdf");

// Test scaffolding to build:
// - `CaptureArtifactStore`: copy from
//   extensions/toolsets/finstack-ai-tools-filesystem/src/tests.rs:433.
// - `middleware_context()`: a MiddlewareContext whose ctx.run matches the
//   ArtifactScope used when staging. Copy construction from
//   extensions/middleware/finstack-ai-middleware-compaction/src/tests.rs
//   (it builds BeforeModel inputs and contexts already — reuse its helpers).
// - `before_model_input(messages) -> StageInput`: a Box<BeforeModelInput>
//   with a ModelRequestDraft whose messages include a user message carrying
//   ContentBlock::File(MediaRef(blob_of(artifact))) — again mirror the
//   compaction middleware's test constructors.
// - `blob_of(artifact: &ArtifactRef) -> BlobRef`: artifact.blob().clone().

#[test]
fn descriptor_declares_before_model_context_mutation() {
    let store = Arc::new(CaptureArtifactStore::default());
    let middleware = DocumentIngestMiddleware::try_new(store).expect("middleware");
    let descriptor = middleware.descriptor();
    assert!(descriptor.stages.contains(finstack_ai_runtime::Stage::BeforeModel));
    assert_eq!(
        descriptor.order.tier,
        finstack_ai_runtime::OrderTier::ContextMutation
    );
}

#[test]
fn replaces_supported_file_block_with_markdown_text() {
    let store = Arc::new(CaptureArtifactStore::default());
    let artifact = stage(store.as_ref(), SAMPLE_CSV, "text/csv", "revenue.csv");
    let middleware = DocumentIngestMiddleware::try_new(store).expect("middleware");
    let input = before_model_input_with_file(&artifact);
    let outcome = block_on(middleware.invoke(middleware_context(), input)).expect("outcome");
    let StageOutcome::Replace(json) = outcome else {
        panic!("expected Replace, got {outcome:?}");
    };
    let draft: finstack_ai_runtime::ModelRequestDraft =
        serde_json::from_slice(json.as_bytes()).expect("draft");
    let text = all_text(&draft);
    assert!(text.contains("revenue.csv"));
    assert!(text.contains("Q1"));
    assert!(!has_file_blocks(&draft));
}

#[test]
fn scanned_pdf_gets_ocr_note() {
    let store = Arc::new(CaptureArtifactStore::default());
    let artifact = stage(store.as_ref(), SCANNED_PDF, "application/pdf", "scan.pdf");
    let middleware = DocumentIngestMiddleware::try_new(store).expect("middleware");
    let outcome = block_on(middleware.invoke(
        middleware_context(),
        before_model_input_with_file(&artifact),
    ))
    .expect("outcome");
    let StageOutcome::Replace(json) = outcome else {
        panic!("expected Replace");
    };
    let draft: finstack_ai_runtime::ModelRequestDraft =
        serde_json::from_slice(json.as_bytes()).expect("draft");
    assert!(all_text(&draft).contains("scanned PDF"));
}

#[test]
fn unresolvable_artifact_is_fail_soft_note() {
    let store = Arc::new(CaptureArtifactStore::default());
    // Blob references an artifact that was never staged.
    let middleware = DocumentIngestMiddleware::try_new(store).expect("middleware");
    let outcome = block_on(middleware.invoke(
        middleware_context(),
        before_model_input_with_dangling_file(),
    ))
    .expect("fail-soft outcome is Ok");
    let StageOutcome::Replace(json) = outcome else {
        panic!("expected Replace");
    };
    let draft: finstack_ai_runtime::ModelRequestDraft =
        serde_json::from_slice(json.as_bytes()).expect("draft");
    assert!(all_text(&draft).contains("could not be read"));
}

#[test]
fn no_file_blocks_means_continue() {
    let store = Arc::new(CaptureArtifactStore::default());
    let middleware = DocumentIngestMiddleware::try_new(store).expect("middleware");
    let outcome = block_on(middleware.invoke(
        middleware_context(),
        before_model_input_text_only(),
    ))
    .expect("outcome");
    assert!(matches!(outcome, StageOutcome::Continue));
}

#[test]
fn unsupported_media_type_file_block_is_left_alone() {
    let store = Arc::new(CaptureArtifactStore::default());
    let artifact = stage(store.as_ref(), b"\x89PNG\r\n", "image/png", "chart.png");
    let middleware = DocumentIngestMiddleware::try_new(store).expect("middleware");
    let outcome = block_on(middleware.invoke(
        middleware_context(),
        before_model_input_with_file(&artifact),
    ))
    .expect("outcome");
    assert!(matches!(outcome, StageOutcome::Continue));
}
```

The commented scaffolding block at the top is a build list, not optional: implement those helpers in `tests.rs` by copying from the named files before running.

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p finstack-ai-middleware-document-ingest`
Expected: FAIL — crate empty.

- [ ] **Step 4: Implement `src/lib.rs`**

```rust
//! `BeforeModel` middleware that converts attached documents to Markdown.
//!
//! The canonical conversation keeps its `ContentBlock::File` blocks. Only the
//! model-visible `ModelRequestDraft` is rewritten: each `File` block whose
//! media type is a supported document format is replaced by a `Text` block
//! containing extracted Markdown (or a fail-soft note). Providers therefore
//! never see media blocks they cannot map.

#![warn(missing_docs)]

use std::sync::Arc;

use finstack_ai_kernel::{ContentBlock, Message, TextBlock};
use finstack_ai_runtime::{
    ArtifactRef, ArtifactScope, ArtifactStore, ComponentId, ComponentInvocation, Digest,
    InvocationRecovery, Metadata, Middleware, MiddlewareContext, MiddlewareDescriptor,
    MiddlewareError, MiddlewareOrder, MiddlewareRole, ModelRequestDraft, OrderTier, PortFuture,
    RawJson, Stage, StageInput, StageMask, StageOutcome, Version,
};
use finstack_ai_tools_document::parser::{self, DocumentFormat, DocumentLimits};
use thiserror::Error;

const COMPONENT_ID: &str = "finstack.middleware.document-ingest";
const INGEST_VERSION: Version = Version {
    major: 1,
    minor: 0,
    patch: 0,
};

/// Construction failure.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DocumentIngestError {
    /// A checked-in identity constant is invalid.
    #[error("document_ingest_configuration_invalid: {reason}")]
    Configuration {
        /// Stable non-secret reason.
        reason: &'static str,
    },
}

/// Fail-soft document ingest middleware.
#[derive(Clone)]
pub struct DocumentIngestMiddleware {
    descriptor: MiddlewareDescriptor,
    store: Arc<dyn ArtifactStore>,
    limits: DocumentLimits,
}

impl std::fmt::Debug for DocumentIngestMiddleware {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DocumentIngestMiddleware")
            .field("limits", &self.limits)
            .finish_non_exhaustive()
    }
}

impl DocumentIngestMiddleware {
    /// Construct with default limits.
    ///
    /// # Errors
    ///
    /// Rejects an invalid checked-in identity.
    pub fn try_new(store: Arc<dyn ArtifactStore>) -> Result<Self, DocumentIngestError> {
        Self::try_with_limits(store, DocumentLimits::default())
    }

    /// Construct with explicit parse limits.
    ///
    /// # Errors
    ///
    /// Rejects an invalid checked-in identity.
    pub fn try_with_limits(
        store: Arc<dyn ArtifactStore>,
        limits: DocumentLimits,
    ) -> Result<Self, DocumentIngestError> {
        Ok(Self {
            descriptor: MiddlewareDescriptor {
                invocation: ComponentInvocation {
                    component: ComponentId::parse(COMPONENT_ID).map_err(|_| {
                        DocumentIngestError::Configuration {
                            reason: "invalid_component_id",
                        }
                    })?,
                    version: INGEST_VERSION,
                    configuration_digest: Digest::raw_json(b"document-ingest-v1"),
                    recovery: InvocationRecovery::RecomputeSafe,
                },
                stages: StageMask::from_stages([Stage::BeforeModel]),
                order: MiddlewareOrder {
                    tier: OrderTier::ContextMutation,
                    priority: 0,
                    before: Arc::from([]),
                    after: Arc::from([]),
                },
                role: MiddlewareRole::Standard,
                metadata: Metadata::empty(),
            },
            store,
            limits,
        })
    }
}

impl Middleware for DocumentIngestMiddleware {
    fn descriptor(&self) -> MiddlewareDescriptor {
        self.descriptor.clone()
    }

    fn invoke(
        &self,
        ctx: MiddlewareContext,
        input: StageInput,
    ) -> PortFuture<Result<StageOutcome, MiddlewareError>> {
        let middleware = self.clone();
        Box::pin(async move {
            let StageInput::BeforeModel(before_model) = input else {
                return Ok(StageOutcome::Continue);
            };
            let scope = run_scope(&ctx);
            let mut changed = false;
            let mut messages: Vec<Message> = Vec::with_capacity(before_model.request.messages.len());
            for message in before_model.request.messages.iter() {
                let (rewritten, message_changed) =
                    middleware.rewrite_message(message, &scope).await;
                changed |= message_changed;
                messages.push(rewritten);
            }
            if !changed {
                return Ok(StageOutcome::Continue);
            }
            let draft = ModelRequestDraft {
                messages: messages.into(),
                ..before_model.request.clone()
            };
            let bytes = serde_json_canonicalizer_bytes(&draft)?;
            Ok(StageOutcome::Replace(RawJson::parse(bytes).map_err(
                |_| stable_error("document ingest replacement could not be normalized"),
            )?))
        })
    }
}

impl DocumentIngestMiddleware {
    async fn rewrite_message(&self, message: &Message, scope: &ArtifactScope) -> (Message, bool) {
        let mut changed = false;
        let mut blocks: Vec<ContentBlock> = Vec::with_capacity(message.content().len());
        for block in message.content() {
            match block {
                ContentBlock::File(media)
                    if DocumentFormat::is_supported_media_type(media.blob().media_type()) =>
                {
                    blocks.push(self.ingest_block(media, scope).await);
                    changed = true;
                }
                other => blocks.push(other.clone()),
            }
        }
        if !changed {
            return (message.clone(), false);
        }
        // Rebuild the message with identical role/metadata and new blocks.
        // Use the same Message constructor prepare.rs uses (read
        // crates/finstack-ai/src/agent/prepare.rs:839 region for the exact
        // constructor and copy it).
        (rebuild_message(message, blocks), true)
    }

    async fn ingest_block(
        &self,
        media: &finstack_ai_kernel::MediaRef,
        scope: &ArtifactScope,
    ) -> ContentBlock {
        let blob = media.blob();
        let name = blob.name().unwrap_or("attachment").to_owned();
        let bytes = match fetch_blob(self.store.as_ref(), scope, blob).await {
            Ok(bytes) => bytes,
            Err(_) => {
                return note_block(&format!(
                    "[Attached document \"{name}\" could not be read; it was skipped.]"
                ));
            }
        };
        match parser::parse(&bytes, Some(blob.media_type()), &self.limits) {
            Ok(parsed) if parsed.requires_ocr && parsed.markdown.is_empty() => note_block(&format!(
                "[Attached document \"{name}\" is a scanned PDF; text extraction requires OCR, which is not enabled.]"
            )),
            Ok(parsed) => {
                let pages = parsed
                    .page_count
                    .map(|count| format!(", {count} pages"))
                    .unwrap_or_default();
                let truncated = if parsed.truncated { ", truncated" } else { "" };
                note_block(&format!(
                    "Attached document \"{name}\" ({format:?}{pages}{truncated}), converted to Markdown:\n\n{markdown}",
                    format = parsed.format,
                    markdown = parsed.markdown,
                ))
            }
            Err(_) => note_block(&format!(
                "[Attached document \"{name}\" could not be parsed; it was skipped.]"
            )),
        }
    }
}

fn note_block(text: &str) -> ContentBlock {
    TextBlock::try_new(text).map_or_else(
        |_| {
            ContentBlock::Text(
                TextBlock::try_new("[attached document note unavailable]")
                    .expect("static note is valid"),
            )
        },
        ContentBlock::Text,
    )
}

fn stable_error(message: &'static str) -> MiddlewareError {
    MiddlewareError::try_new(
        finstack_ai_runtime::MIDDLEWARE_OUTCOME_NOT_ALLOWED,
        finstack_ai_runtime::ErrorCategory::Middleware,
        message,
        Metadata::empty(),
    )
    .unwrap_or_else(Into::into)
}

#[cfg(test)]
mod tests;
```

Three functions above intentionally name work to copy from existing code rather than invent (the plan freezes *what*, the neighboring code freezes *how*):
- `rebuild_message(message, blocks)` — the `Message` constructor used at `crates/finstack-ai/src/agent/prepare.rs:839`; keep role and every metadata field identical.
- `fetch_blob(store, scope, blob)` — construct the `ArtifactRef` for a `BlobRef` per spec decision 19 (`BlobRef.id` = artifact id; digest/length copied). Read `ArtifactRef::try_new` (`grep -n "fn try_new" crates/finstack-ai-runtime` / kernel) and rebuild the ref exactly as `stage_put` built it, then `store.get(scope.clone(), artifact)` and verify `Digest::blob_content(&bytes)` equals `blob.digest()` (mismatch → treat as unreadable). If `ArtifactRef` cannot be reconstructed from a `BlobRef` alone (e.g. `ArtifactId` is not derivable from the blob id string), stop and surface this to the human partner — it means spec decision 19 needs an explicit id-encoding convention (candidate: blob id string = hex `ArtifactId`), and the fix must go in Task 6's staging code too.
- `run_scope(&ctx)` — same field mapping as the toolset's `call_scope` (Task 4), sourced from `ctx.run`.
- `serde_json_canonicalizer_bytes(&draft)` — use the same canonicalization the middleware port itself uses (`serde_json_canonicalizer::to_vec`, see `crates/finstack-ai-runtime/src/ports/middleware/types.rs:486`); add `serde_json_canonicalizer` as a dependency with the workspace version.

Spec decision 16 also asks for an observer-visible event on fail-soft skips. The middleware port has no direct observer hook; the skip note in the replaced draft is itself durable and observer-visible via the journaled model request. If during implementation you find a supported middleware-side event/metadata channel (check how `finstack-ai-middleware-compaction` reports evidence), use it; otherwise record this as a documented deviation in the Task 10 README.

- [ ] **Step 5: Run tests**

Run: `cargo test -p finstack-ai-middleware-document-ingest`
Expected: PASS.

- [ ] **Step 6: Verify `Replace` is accepted at BeforeModel for Standard-role middleware**

Run: `grep -rn "StageOutcome::Replace" crates/finstack-ai-runtime/src crates/finstack-ai/src | grep -v tests`
Read the chain-driver handling. If `Replace` at `BeforeModel` is rejected for non-compactor roles (outcome-allowlist per stage), STOP — do not work around it silently. Options to bring back to the human partner: (a) allowlist `Replace` for ContextMutation-tier middleware in the runtime, (b) fall back to `AddContext` without File-block stripping plus a provider-side skip of media blocks. Record the decision in the spec before continuing.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml Cargo.lock extensions/middleware/finstack-ai-middleware-document-ingest/
git commit -m "feat: add document ingest BeforeModel middleware"
```

---

### Task 6: `AgentRunRequest.attachments` and prepare-path mapping

**Files:**
- Modify: `crates/finstack-ai/src/agent/types.rs` (add `AttachmentInput`, `attachments` field, validation)
- Modify: `crates/finstack-ai/src/agent/prepare.rs` (~line 839: user-message construction)
- Modify: `crates/finstack-ai/src/session/mod.rs` (~line 335: session-run user-message construction)
- Modify: `crates/finstack-ai/src/lib.rs` (re-export `AttachmentInput`, `MAX_RUN_ATTACHMENTS`)
- Modify: `crates/finstack-ai/src/agent/tests.rs`
- Modify: `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai.txt`

**Interfaces:**
- Produces (used by Tasks 7–9):

```rust
/// One pre-staged run attachment.
#[derive(Debug, Clone)]
pub struct AttachmentInput {
    /// Staged artifact whose blob carries media type, length, digest, name.
    pub artifact: ArtifactRef,
}

pub const MAX_RUN_ATTACHMENTS: usize = 8;
// AgentRunRequest gains: pub attachments: Arc<[AttachmentInput]>,
```

(Media type and name live on `artifact.blob()` already — a separate `media_type` field would be a second source of truth; spec decision 18's fields are satisfied through the blob. Note this refinement in the commit message.)

- [ ] **Step 1: Write failing tests**

In `crates/finstack-ai/src/agent/tests.rs`, next to existing `AgentRunRequest` tests (grep `try_new` there for the existing request-construction helper and reuse it):

```rust
#[test]
fn run_request_defaults_to_no_attachments() {
    let request = test_run_request(); // existing helper for a valid request
    assert!(request.attachments.is_empty());
}

#[test]
fn run_request_rejects_more_than_max_attachments() {
    let mut request = test_run_request();
    let attachment = test_attachment(); // build one staged ArtifactRef like services/artifact.rs tests do
    request.attachments = std::iter::repeat_with(|| attachment.clone())
        .take(MAX_RUN_ATTACHMENTS + 1)
        .collect();
    assert!(request.validate().is_err());
}

#[test]
fn prepared_user_message_carries_file_blocks_for_attachments() {
    // Drive the same prepare path the existing prepare tests use (grep
    // prepare.rs tests for the user-message assertion pattern) with a request
    // carrying one attachment; assert the prepared user message contains
    // exactly one ContentBlock::File whose blob id/digest/media_type match
    // the attachment's artifact.blob().
}
```

The third test's body: copy the closest existing prepare-path test in `crates/finstack-ai/src/agent/tests.rs` (one that asserts on the built user message from `input`), add `request.attachments`, and extend its assertions — the plan cannot transcribe that harness here; it must be the file's own idiom.

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p finstack-ai run_request_defaults_to_no_attachments`
Expected: FAIL — no `attachments` field.

- [ ] **Step 3: Implement**

In `types.rs`:

```rust
/// Maximum attachments accepted per run.
pub const MAX_RUN_ATTACHMENTS: usize = 8;

/// One pre-staged run attachment.
///
/// The artifact must already be durably staged in the run's `ArtifactStore`
/// scope; the run API never accepts raw bytes.
#[derive(Debug, Clone)]
pub struct AttachmentInput {
    /// Staged artifact whose blob carries media type, length, digest, name.
    pub artifact: ArtifactRef,
}
```

Add to `AgentRunRequest`:

```rust
    /// Pre-staged input attachments mapped to `File` blocks on the user message.
    pub attachments: Arc<[AttachmentInput]>,
```

Initialize `attachments: Arc::from([])` in `try_new`, and extend `validate()`:

```rust
        if self.attachments.len() > MAX_RUN_ATTACHMENTS {
            return Err(AgentRunError::configuration(
                AGENT_RUN_INVALID_CONFIGURATION,
                "run attachments exceed MAX_RUN_ATTACHMENTS",
            ));
        }
```

(`ArtifactRef` comes from `finstack_ai_runtime` — add it to the existing `use finstack_ai_runtime::{...}` list.)

In `prepare.rs` at the `vec![ContentBlock::Text(...)]` construction (line ~839), extend the block vector:

```rust
        let mut blocks = vec![ContentBlock::Text(TextBlock::try_new(text).map_err(
            /* keep the existing error mapping */
        )?)];
        for attachment in request.attachments.iter() {
            blocks.push(ContentBlock::File(finstack_ai_kernel::MediaRef::new(
                attachment.artifact.blob().clone(),
            )));
        }
```

(Adapt variable names to the surrounding function; `request` may be named differently there. The session path at `session/mod.rs:335` wraps input the same way — apply the identical extension, threading `attachments` through whatever request struct that path uses. If the session path funnels into the same prepare helper, one change suffices; verify by reading both call sites.)

- [ ] **Step 4: Run tests**

Run: `cargo test -p finstack-ai`
Expected: PASS.

- [ ] **Step 5: Regenerate the public-API compatibility fixture**

Run the repo's own check first to learn the command: `grep -rn "cargo-public-api\|public-rust-api" mise.toml .github/ tools/ | head`. Use the documented regeneration command (likely a `mise` task). Diff must show only additive lines (`AttachmentInput`, `MAX_RUN_ATTACHMENTS`, `attachments` field).

- [ ] **Step 6: Commit**

```bash
git add crates/finstack-ai/src/agent/types.rs crates/finstack-ai/src/agent/prepare.rs crates/finstack-ai/src/session/mod.rs crates/finstack-ai/src/lib.rs crates/finstack-ai/src/agent/tests.rs fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai.txt
git commit -m "feat: accept pre-staged attachments on AgentRunRequest as File blocks"
```

---

### Task 7: End-to-end integration lane

**Files:**
- Create: `crates/finstack-ai-test/tests/lanes/document_ingest.rs`
- Modify: `crates/finstack-ai-test/tests/lanes/main.rs` or the lane registry (grep how `session.rs` / `subagent.rs` lanes are registered — add `mod document_ingest;` in the same place)

**Interfaces:**
- Consumes: everything from Tasks 2–6.

- [ ] **Step 1: Write the lane test**

Model it on `crates/finstack-ai-test/tests/lanes/session.rs` (read it first; reuse its agent-builder + scripted-model harness). The test:

```rust
//! Attach → auto-ingest → model-visible Markdown lane.

// 1. Build an agent with the scripted/mock model the session lane uses,
//    registering:
//      .middleware(component_ref("finstack.middleware.document-ingest"),
//                  Arc::new(DocumentIngestMiddleware::try_new(store.clone())?))
//      .toolset(component_ref("finstack.tools.document"),
//               Arc::new(DocumentToolset::try_new()?.with_artifact_store(store.clone())))
//    where `store` is Arc<InProcessArtifactStore> from
//    finstack-ai-context-memory (add it as a dev-dependency of finstack-ai-test
//    if absent, matching how other lanes pull extension crates).
// 2. Stage fixtures/documents/sample.csv into `store` under the run's scope.
// 3. Run with AgentRunRequest { attachments: [AttachmentInput { artifact }], .. }.
// 4. Assert: (a) the run completes; (b) the model request captured by the
//    scripted model contains the Markdown ("Q1") and NO File block;
//    (c) the journaled conversation (or run output record kinds) still shows
//    the original user message with its File block.
```

Write it as real code against the harness found in `session.rs` — the four numbered behaviors are the acceptance criteria; the harness idioms come from the neighboring lane file.

- [ ] **Step 2: Run to verify it fails before wiring, then passes**

Run: `cargo test -p finstack-ai-test --test lanes document_ingest`
Expected: PASS once Tasks 2–6 are complete. If assertion (b) fails because the middleware never ran, check builder middleware registration (ComponentRef version must match `INGEST_VERSION`).

- [ ] **Step 3: Commit**

```bash
git add crates/finstack-ai-test/tests/lanes/
git commit -m "test: add attach-to-markdown document ingest integration lane"
```

---

### Task 8: Python binding parity

**Files:**
- Modify: `bindings/finstack-ai-python/src/run.rs` (`run_request` gains `attachments`)
- Modify: `bindings/finstack-ai-python/src/agent.rs` (expose `Attachment` pyclass; register toolset/middleware; artifact-store handle)
- Modify: python test suite (grep `bindings/finstack-ai-python` for its `tests/` or pytest location; add `test_document_attachments.py` beside existing run tests)

**Interfaces:**
- Consumes: `AttachmentInput`, `DocumentToolset`, `DocumentIngestMiddleware`, `InProcessArtifactStore`.
- Produces (Python surface):
  - `Attachment(data: bytes | None = None, path: str | None = None, media_type: str, name: str | None = None)` pyclass — exactly one of `data`/`path`; a `path` is read (bounded by 4 MiB) at staging time and its basename becomes the default `name` (spec decision 21)
  - `agent.run(..., attachments: list[Attachment] | None = None)` (and the same on the session-run method if the binding exposes one — mirror whichever run entry points exist in `run.rs`)

- [ ] **Step 1: Read the binding's agent construction**

Run: `grep -n "NativeAgentBuilder\|Agent::builder\|toolset\|middleware" bindings/finstack-ai-python/src/agent.rs | head -30`
Identify: (a) where toolsets/middleware are registered, (b) whether the binding already holds an `ArtifactStore` (Task 6 established `InProcessArtifactStore` lives in `finstack-ai-context-memory`). If the binding has no store, add one `Arc<InProcessArtifactStore>` created at agent build and held on the Python agent struct — it must be the SAME instance given to `DocumentToolset::with_artifact_store`, `DocumentIngestMiddleware::try_new`, and attachment staging.

- [ ] **Step 2: Write failing pytest**

```python
import finstack_ai


def test_run_with_csv_attachment(tmp_path, scripted_agent):
    # scripted_agent: reuse the existing fixture that builds an agent with a
    # scripted model (grep the python tests for how agents are built today).
    result = scripted_agent.run(
        "Summarize the attached file.",
        attachments=[
            finstack_ai.Attachment(
                data=b"quarter,revenue\nQ1,1250\n",
                media_type="text/csv",
                name="revenue.csv",
            )
        ],
    )
    assert result is not None


def test_more_than_eight_attachments_rejected(scripted_agent):
    attachment = finstack_ai.Attachment(data=b"a,b\n1,2\n", media_type="text/csv")
    try:
        scripted_agent.run("x", attachments=[attachment] * 9)
    except Exception as error:  # noqa: BLE001 - binding maps to its own error type
        assert "attachment" in str(error).lower()
    else:
        raise AssertionError("expected attachment-count rejection")
```

Adapt module name / fixture names to the existing python test suite's conventions (read one existing test file first).

- [ ] **Step 3: Run to verify failure**

Run the binding's documented test command (grep `mise.toml` for the python test task; likely `mise run test-python` or `uv run pytest`).
Expected: FAIL — `Attachment` missing.

- [ ] **Step 4: Implement**

`Attachment` pyclass (in `run.rs` or the binding's types module, matching where `PyLocator` lives):

```rust
/// One in-memory run attachment staged at submit time.
#[pyclass(name = "Attachment")]
#[derive(Clone)]
pub struct PyAttachment {
    pub(crate) data: Vec<u8>,
    pub(crate) media_type: String,
    pub(crate) name: Option<String>,
}

#[pymethods]
impl PyAttachment {
    #[new]
    #[pyo3(signature = (media_type, data = None, path = None, name = None))]
    fn new(
        media_type: String,
        data: Option<Vec<u8>>,
        path: Option<String>,
        name: Option<String>,
    ) -> PyResult<Self> {
        let data = match (data, &path) {
            (Some(data), None) => data,
            (None, Some(path)) => {
                let bytes = std::fs::read(path)
                    .map_err(|error| PyValueError::new_err(error.to_string()))?;
                if bytes.len() > 4 * 1024 * 1024 {
                    return Err(PyValueError::new_err("attachment exceeds 4 MiB"));
                }
                bytes
            }
            _ => {
                return Err(PyValueError::new_err(
                    "exactly one of data or path is required",
                ));
            }
        };
        let name = name.or_else(|| {
            path.as_deref().and_then(|value| {
                std::path::Path::new(value)
                    .file_name()
                    .map(|value| value.to_string_lossy().into_owned())
            })
        });
        Ok(Self {
            data,
            media_type,
            name,
        })
    }
}
```

Extend `run_request(...)` with `attachments: Vec<AttachmentInput>` (staged refs, staging happens in the caller where the store and scope are known) and set `request.attachments = attachments.into();` after the existing field assignments. In the agent run entry point, before building the request, stage each `PyAttachment` via `stage_required_artifact(store.as_ref(), scope, Bytes::from(attachment.data), ArtifactMetadata { kind: Arc::from("attachment"), media_type: attachment.media_type.into(), name: attachment.name.map(Into::into), attributes: Metadata::empty() })` with the scope construction the binding uses for the run's tenant/session (same fields as Task 4's `call_scope`), collecting `AttachmentInput { artifact }`. Register `DocumentToolset` + `DocumentIngestMiddleware` in the binding's agent builder using the same `ComponentRef` style its existing registrations use. Add `finstack-ai-tools-document`, `finstack-ai-middleware-document-ingest`, `finstack-ai-context-memory` to the binding's `Cargo.toml` dependencies.

- [ ] **Step 5: Run python tests**

Run: the binding's test command from Step 3.
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add bindings/finstack-ai-python/ Cargo.lock
git commit -m "feat: expose document attachments and ingest extension in python binding"
```

---

### Task 9: WASM binding parity and wasm cleanliness gate

**Files:**
- Modify: `bindings/finstack-ai-wasm/src/agent/build.rs` (register toolset/middleware)
- Modify: `bindings/finstack-ai-wasm/src/agent/run.rs` + `bindings/finstack-ai-wasm/src/agent/request.rs` (attachments on the run surface)
- Modify: `bindings/finstack-ai-wasm/Cargo.toml`
- Test: the wasm binding's existing JS/wasm test suite (grep `bindings/finstack-ai-wasm` for `tests/` and `wasm-bindgen-test`)

**Interfaces:**
- Consumes: `AttachmentInput`, `HostArtifactStore` (`bindings/finstack-ai-wasm/src/host_artifact.rs:97`), `DocumentToolset`, `DocumentIngestMiddleware`.
- Produces: run options accept `attachments: [{ data: Uint8Array, mediaType: string, name?: string }]`.

- [ ] **Step 1: Read the wasm run-request surface**

Run: `grep -n "run_request\|RunOptions\|serde_wasm_bindgen" bindings/finstack-ai-wasm/src/agent/run.rs bindings/finstack-ai-wasm/src/agent/request.rs | head -20`
Identify the options struct deserialized from JS and the `run_request` helper (seen at `request.rs` / `run.rs:119`).

- [ ] **Step 2: Write failing wasm test**

Follow the binding's existing test idiom (wasm-bindgen-test or JS harness — copy the nearest run test) asserting: a run submitted with one csv attachment completes, and a run with 9 attachments rejects with a message containing `attachment`.

- [ ] **Step 3: Implement**

Add to the options struct (serde field names in the binding's existing casing convention):

```rust
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AttachmentOption {
    #[serde(with = "serde_bytes")]
    pub data: Vec<u8>,
    pub media_type: String,
    #[serde(default)]
    pub name: Option<String>,
}
```

(If the binding doesn't already depend on `serde_bytes`, deserialize `data` as `js_sys::Uint8Array` the way other byte inputs in the binding are handled — copy that pattern instead.) Stage each via `stage_required_artifact` against the binding's `HostArtifactStore` instance with the run's scope, then `request.attachments = staged.into();`. In `build.rs`, register `DocumentToolset::try_new()?.with_artifact_store(store.clone())` and `DocumentIngestMiddleware::try_new(store.clone())?` exactly like existing toolset registrations there (same `ComponentRef` idiom). Path sources are automatically inert (non-unix → `DOCUMENT_PATH_UNSUPPORTED`).

- [ ] **Step 4: Run the wasm gate**

Run: `python3 scripts/wasm_package/check.py` (or the mise task wrapping it — grep `mise.toml` for `wasm_package`), plus the wasm build/test task.
Expected: PASS with zero `FORBIDDEN_WASM` additions. If `anydoc` or `pdf-inspector` drags a forbidden dep into the wasm tree, STOP and report — that falsifies the spec's wasm-clean claim and the human partner decides (feature-gate the crates out of wasm vs. upstream fix).

- [ ] **Step 5: Commit**

```bash
git add bindings/finstack-ai-wasm/ Cargo.lock
git commit -m "feat: expose document attachments and ingest extension in wasm binding"
```

---

### Task 10: Docs, changelog, and full verification

**Files:**
- Create: `extensions/toolsets/finstack-ai-tools-document/README.md`
- Create: `extensions/middleware/finstack-ai-middleware-document-ingest/README.md`
- Modify: `CHANGELOG.md`

- [ ] **Step 1: Write the two READMEs**

Each mirrors the structure of `extensions/stores/README.md` / an existing toolset README (read one first): one-paragraph purpose, construction example, tool/middleware surface table (names, error codes), the scanned-PDF/`requires_ocr` contract, the fail-soft middleware contract, and the OCR non-goal pointing at the spec's designated future path (pdf-inspector `ocr` feature / oar-ocr).

- [ ] **Step 2: Add CHANGELOG entry**

Under the current unreleased heading, following the file's existing entry style:

```markdown
- Add `finstack-ai-tools-document`: `document_parse` and `pdf_classify` tools converting pdf/docx/xlsx/pptx/odf/rtf/epub/csv to Markdown (anydoc + pdf-inspector, no OCR).
- Add `finstack-ai-middleware-document-ingest`: fail-soft `BeforeModel` middleware replacing attached-document `File` blocks with extracted Markdown in the model-visible request.
- Add `AgentRunRequest.attachments` (`AttachmentInput`, max 8 pre-staged artifacts) with Python (`Attachment`) and WASM (`attachments` run option) parity.
```

- [ ] **Step 3: Full verification**

Run, in order, and read every result:

```bash
cargo clippy --workspace --all-targets
```

```bash
cargo test --workspace
```

plus the python test task, the wasm build/test task, and `python3 scripts/wasm_package/check.py` (exact task names from `mise.toml`). All green before claiming completion — use the superpowers:verification-before-completion skill.

- [ ] **Step 4: Commit**

```bash
git add extensions/toolsets/finstack-ai-tools-document/README.md extensions/middleware/finstack-ai-middleware-document-ingest/README.md CHANGELOG.md
git commit -m "docs: document ingestion extension READMEs and changelog"
```

---

## Known Risks (read before starting)

1. **Upstream API drift** (`anydoc` 0.1.x): Task 2's adapter functions are the only place upstream names appear. Confirm names via `cargo doc` before writing them.
2. **`StageOutcome::Replace` acceptance at BeforeModel** (Task 5 Step 6): explicitly verified mid-plan; has a defined escalation path, not a workaround.
3. **BlobRef→ArtifactRef reconstruction** (spec decision 19): if the artifact id is not recoverable from the blob id, escalate — the fix (encode `ArtifactId` hex as the blob id at staging time) must be applied consistently in Tasks 6, 8, 9.
4. **Hand-written PDF fixtures** may be too minimal for real parsers; the generator is the thing to fix, and if that stalls, generating `text.pdf` once with any tool and checking in the bytes is acceptable — fixtures must stay < 50 KB.
5. **Unrelated in-flight working-tree changes exist.** Never stage with `-A`/`.`; every commit lists exact paths.
