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

    let markdown = match anydoc::to_markdown_bytes(bytes, anydoc_format(format)) {
        Ok(text) => text,
        // A scanned/image PDF yielding no extractable text is a success, not
        // an error — but only that specific "nothing to extract" case; a
        // genuinely broken, encrypted, or over-limit PDF must still surface
        // as a failure even when we already know it needs OCR.
        Err(anydoc::ConvertError::Unsupported(_))
            if requires_ocr && format == DocumentFormat::Pdf =>
        {
            String::new()
        }
        Err(error) => {
            return Err(DocumentParseError::ParseFailed {
                message: bounded_message(&error.to_string()),
            });
        }
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

/// Parse a 1-based inclusive page range of a PDF to Markdown.
///
/// Only valid for PDF bytes; the range itself (non-zero, non-reversed, in
/// bounds) is the caller's responsibility to validate against the page
/// count returned by [`classify_pdf`] before calling this, since a
/// reversed/zero-based range is an argument-shape error rather than a parse
/// failure.
///
/// # Errors
///
/// Returns `UnsupportedFormat` for non-PDF bytes, `TooLarge` for an
/// oversized input or page count, and `ParseFailed` for an out-of-bounds
/// range or a broken PDF.
pub fn parse_pages(
    bytes: &[u8],
    page_range: (u32, u32),
    limits: &DocumentLimits,
) -> Result<ParsedDocument, DocumentParseError> {
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > limits.max_input_bytes {
        return Err(DocumentParseError::TooLarge {
            len: bytes.len(),
            max: limits.max_input_bytes,
        });
    }
    let (classification, page_count) = classify_pdf(bytes)?;
    if page_count > limits.max_pages {
        return Err(DocumentParseError::TooLarge {
            len: bytes.len(),
            max: u64::from(limits.max_pages),
        });
    }
    let (start, end) = page_range;
    if start == 0 || end < start || end > page_count {
        return Err(DocumentParseError::ParseFailed {
            message: bounded_message("page_range is out of bounds"),
        });
    }
    let zero_indexed: Vec<u32> = (start - 1..end).collect();
    let extracted =
        pdf_inspector::extract_pages_markdown_mem(bytes, Some(&zero_indexed)).map_err(|error| {
            DocumentParseError::ParseFailed {
                message: bounded_message(&error.to_string()),
            }
        })?;
    let requires_ocr = matches!(
        classification,
        DocumentClassification::Scanned | DocumentClassification::Image
    ) || extracted.pages.iter().any(|page| page.needs_ocr);
    let markdown = extracted
        .pages
        .iter()
        .map(|page| page.markdown.as_str())
        .collect::<Vec<_>>()
        .join("\n\n");
    let (markdown, truncated) = truncate_utf8(markdown, limits.max_output_bytes);
    Ok(ParsedDocument {
        markdown,
        format: DocumentFormat::Pdf,
        page_count: Some(page_count),
        classification: Some(classification),
        requires_ocr,
        truncated,
    })
}

fn detect_format(bytes: &[u8], hint: Option<DocumentFormat>) -> DocumentFormat {
    match anydoc::Format::from_bytes(bytes) {
        // anydoc has a single `Excel` variant covering both legacy `.xls`
        // and modern `.xlsx` containers; use the caller's hint to recover
        // which one it was, defaulting to the modern format.
        Some(anydoc::Format::Excel) => match hint {
            Some(DocumentFormat::Xls) => DocumentFormat::Xls,
            _ => DocumentFormat::Xlsx,
        },
        Some(detected) => from_anydoc(detected),
        None => hint.unwrap_or(DocumentFormat::Unknown),
    }
}

/// Map an `anydoc::Format` (content-sniffed) to our public [`DocumentFormat`].
///
/// `Excel` is handled by the caller ([`detect_format`]) because splitting it
/// into `Xls`/`Xlsx` needs the media-type hint that isn't available here.
fn from_anydoc(format: anydoc::Format) -> DocumentFormat {
    match format {
        anydoc::Format::Doc => DocumentFormat::Doc,
        anydoc::Format::Docx => DocumentFormat::Docx,
        anydoc::Format::Odt => DocumentFormat::Odt,
        anydoc::Format::Pdf => DocumentFormat::Pdf,
        anydoc::Format::Ppt => DocumentFormat::Ppt,
        anydoc::Format::Pptx => DocumentFormat::Pptx,
        anydoc::Format::Rtf => DocumentFormat::Rtf,
        anydoc::Format::Epub => DocumentFormat::Epub,
        anydoc::Format::Excel => DocumentFormat::Xlsx,
        anydoc::Format::Ods => DocumentFormat::Ods,
        anydoc::Format::Odp => DocumentFormat::Odp,
        anydoc::Format::Csv => DocumentFormat::Csv,
    }
}

/// Map our public [`DocumentFormat`] to the `anydoc::Format` that selects its
/// parser. Only ever called with a format `parse` has already confirmed is
/// not [`DocumentFormat::Unknown`].
fn anydoc_format(format: DocumentFormat) -> anydoc::Format {
    match format {
        DocumentFormat::Pdf => anydoc::Format::Pdf,
        DocumentFormat::Docx => anydoc::Format::Docx,
        DocumentFormat::Doc => anydoc::Format::Doc,
        DocumentFormat::Pptx => anydoc::Format::Pptx,
        DocumentFormat::Ppt => anydoc::Format::Ppt,
        DocumentFormat::Xlsx | DocumentFormat::Xls => anydoc::Format::Excel,
        DocumentFormat::Odt => anydoc::Format::Odt,
        DocumentFormat::Ods => anydoc::Format::Ods,
        DocumentFormat::Odp => anydoc::Format::Odp,
        DocumentFormat::Rtf => anydoc::Format::Rtf,
        DocumentFormat::Epub => anydoc::Format::Epub,
        DocumentFormat::Csv => anydoc::Format::Csv,
        DocumentFormat::Unknown => unreachable!("parse() rejects Unknown before this is called"),
    }
}

/// Map pdf-inspector's classification kind to [`DocumentClassification`].
fn map_pdf_type(classification: &pdf_inspector::PdfClassification) -> DocumentClassification {
    match classification.pdf_type {
        pdf_inspector::PdfType::TextBased => DocumentClassification::Text,
        pdf_inspector::PdfType::Scanned => DocumentClassification::Scanned,
        pdf_inspector::PdfType::ImageBased => DocumentClassification::Image,
        pdf_inspector::PdfType::Mixed => DocumentClassification::Mixed,
    }
}

/// Read the page count off a pdf-inspector classification.
fn page_count_of(classification: &pdf_inspector::PdfClassification) -> u32 {
    classification.page_count
}

/// Truncate `text` to at most `max_bytes` at a UTF-8 char boundary.
pub(crate) fn truncate_utf8(mut text: String, max_bytes: u64) -> (String, bool) {
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
