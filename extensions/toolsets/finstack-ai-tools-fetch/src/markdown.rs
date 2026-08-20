//! HTML → Markdown conversion for the `Auto`/`Markdown` delivery modes
//! (spec §4.5).
//!
//! Conversion is best-effort: any failure, or a converted result that
//! exceeds `output_cap`, degrades to [`None`] so the caller can fall back to
//! inlining the original text, which is already budget-checked. This
//! function never returns an error.

use htmd::HtmlToMarkdown;

/// Convert `html` to Markdown, dropping scripts/styles/comments.
///
/// Returns `None` when conversion fails, or when the converted Markdown's
/// byte length exceeds `output_cap` — in either case the caller falls back
/// to inlining the original text. Conversion is expected to run only on
/// bodies already under the raw read cap.
pub(crate) fn html_to_markdown(html: &str, output_cap: usize) -> Option<String> {
    let converter = HtmlToMarkdown::builder()
        // Explicit even though htmd drops these by default, to keep the
        // guarantee visible at the call site regardless of upstream default
        // changes.
        .skip_tags(vec!["script", "style"])
        .build();
    let markdown = converter.convert(html).ok()?;
    if markdown.len() > output_cap {
        return None;
    }
    Some(markdown)
}

#[cfg(test)]
mod tests {
    use super::html_to_markdown;

    const BIG_CAP: usize = 1_048_576;

    fn golden(name: &str) -> String {
        // Trim trailing whitespace consistently: goldens are committed with
        // a single trailing newline, `htmd`'s output has none.
        let raw = match name {
            "nested_lists" => include_str!("../fixtures/nested_lists.md"),
            "table" => include_str!("../fixtures/table.md"),
            "script_style" => include_str!("../fixtures/script_style.md"),
            other => panic!("unknown golden {other}"),
        };
        raw.trim_end().to_owned()
    }

    #[test]
    fn nested_lists_match_golden() {
        let html = include_str!("../fixtures/nested_lists.html");
        let markdown = html_to_markdown(html, BIG_CAP).expect("conversion succeeds");
        assert_eq!(markdown.trim_end(), golden("nested_lists"));
    }

    #[test]
    fn table_matches_golden() {
        let html = include_str!("../fixtures/table.html");
        let markdown = html_to_markdown(html, BIG_CAP).expect("conversion succeeds");
        assert_eq!(markdown.trim_end(), golden("table"));
    }

    #[test]
    fn script_and_style_are_dropped() {
        let html = include_str!("../fixtures/script_style.html");
        let markdown = html_to_markdown(html, BIG_CAP).expect("conversion succeeds");
        assert_eq!(markdown.trim_end(), golden("script_style"));
        assert!(!markdown.contains("var secret"));
        assert!(!markdown.contains("color: red"));
        assert!(!markdown.contains("display: none"));
        assert!(!markdown.contains("document.write"));
        assert!(!markdown.contains("inline script marker"));
        assert!(!markdown.contains("a comment that must not appear"));
        assert!(markdown.contains("Visible Heading"));
        assert!(markdown.contains("Visible paragraph text"));
    }

    #[test]
    fn pathological_markup_converts_without_panicking() {
        // html5ever (htmd's parser) is a browser-grade forgiving parser, so
        // this malformed input is not expected to make conversion fail —
        // the assertion here is that it does not panic and produces some
        // output containing the visible text. The `None`-on-cap-exceeded
        // path is covered directly below instead, per the task brief (a
        // genuine conversion *failure* is not reproducible with htmd/
        // html5ever's forgiving parser).
        let html = include_str!("../fixtures/pathological.html");
        let markdown = html_to_markdown(html, BIG_CAP).expect("htmd tolerates malformed markup");
        assert!(markdown.contains("Broken"));
    }

    #[test]
    fn output_cap_exceeded_yields_none() {
        let html = include_str!("../fixtures/nested_lists.html");
        // The full conversion is far larger than one byte; a one-byte cap
        // forces the `None` fallback path deterministically.
        assert!(html_to_markdown(html, 1).is_none());
    }
}
