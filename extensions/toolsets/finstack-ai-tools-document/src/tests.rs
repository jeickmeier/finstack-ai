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
