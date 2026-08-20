//! HTML → Markdown conversion for the `Auto`/`Markdown` delivery modes
//! (spec §4.5).
//!
//! Conversion is best-effort: any failure, or a converted result that
//! exceeds `output_cap`, degrades to [`None`] so the caller can fall back to
//! inlining the original text, which is already budget-checked. This
//! function never returns an error.

use htmd::HtmlToMarkdown;

/// Cap on the heuristic nesting-depth scan performed by
/// [`exceeds_safe_nesting_depth`] before a document is handed to
/// `htmd`/`html5ever`. See that function's doc comment for what "depth"
/// means here and why the cap is heuristic rather than exact.
const MAX_SCAN_DEPTH: usize = 512;

/// Cheap pre-parse guard against pathologically deep HTML (F-4): `htmd`'s
/// underlying `html5ever` parse produces a DOM that is walked, and dropped,
/// recursively, so a document with tens or hundreds of thousands of nested
/// elements can exhaust the stack and abort the process rather than
/// returning an error — confirmed by a 100,000-level `<div>` document
/// aborting with SIGABRT before this guard existed.
///
/// This is a single-pass byte scan, not a real parser: it treats `<` followed
/// by an ASCII letter as an opening-tag transition (depth += 1) and `</` as a
/// closing-tag transition (depth = `depth.saturating_sub(1)`), without
/// tracking tag names or void-element rules. That means a run of sibling
/// void elements (e.g. many consecutive `<br>`) is over-counted as if it
/// nested, which can only make this guard reject *more* documents than
/// strictly necessary — never fewer. Every failure mode here is fail-closed:
/// `html_to_markdown` returns `None` and the caller falls back to its
/// already-budget-checked plain-text path, it does not error the whole
/// fetch.
///
/// `MAX_SCAN_DEPTH` (512) is well above any HTML a legitimate document is
/// likely to nest by hand or by templating, and well below the depth that
/// risks stack exhaustion in the underlying parser/DOM-walk/Drop.
fn exceeds_safe_nesting_depth(html: &str) -> bool {
    let bytes = html.as_bytes();
    let mut depth: usize = 0;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            if bytes.get(i + 1) == Some(&b'/') {
                depth = depth.saturating_sub(1);
            } else if bytes.get(i + 1).is_some_and(u8::is_ascii_alphabetic) {
                depth += 1;
                if depth > MAX_SCAN_DEPTH {
                    return true;
                }
            }
        }
        i += 1;
    }
    false
}

/// Convert `html` to Markdown, dropping scripts/styles/comments.
///
/// Returns `None` when conversion fails, when the input's heuristic nesting
/// depth exceeds [`MAX_SCAN_DEPTH`] (see [`exceeds_safe_nesting_depth`],
/// F-4), or when the converted Markdown's byte length exceeds `output_cap`
/// — in every case the caller falls back to inlining the original text.
/// Conversion is expected to run only on bodies already under the raw read
/// cap. Callers pass the same `effective_cap`/`max_result_budget` used by
/// every other inline delivery path here, not `max_result_bytes - 4096`, so
/// there is only one budget concept to reason about.
///
/// Note: `htmd` treats `<title>` as an ordinary block element (only
/// `script`/`style` are skipped), so a document's `<title>` text appears as
/// a leading plain-text line ahead of the rest of the conversion.
pub(crate) fn html_to_markdown(html: &str, output_cap: usize) -> Option<String> {
    if exceeds_safe_nesting_depth(html) {
        return None;
    }
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

    /// Build a document with `depth` levels of nested `<div>` wrapping a
    /// short text node, e.g. depth=3 -> `<div><div><div>text</div></div></div>`.
    fn nested_divs(depth: usize) -> String {
        let mut html = String::with_capacity(depth * 11 + 16);
        for _ in 0..depth {
            html.push_str("<div>");
        }
        html.push_str("text");
        for _ in 0..depth {
            html.push_str("</div>");
        }
        html
    }

    #[test]
    fn deeply_nested_html_does_not_crash() {
        // F-4: a document with 100_000 levels of nesting must not blow the
        // stack during parse/convert/drop. Confirmed pre-mitigation: this
        // test aborted the process with SIGABRT (stack overflow) when run
        // in isolation before the depth-scan guard was added. With the
        // guard in place, 100_000 is far past MAX_SCAN_DEPTH (512), so the
        // guard trips and conversion returns `None` well before htmd ever
        // sees the input.
        let html = nested_divs(100_000);
        let result = html_to_markdown(&html, BIG_CAP);
        assert!(result.is_none(), "expected the depth guard to trip");
    }

    #[test]
    fn moderately_nested_html_still_converts() {
        // ~100 levels deep is well under the guard's cap and should convert
        // normally rather than being rejected.
        let html = nested_divs(100);
        let result = html_to_markdown(&html, BIG_CAP);
        assert!(result.is_some(), "moderate nesting should still convert");
        assert!(result.unwrap().contains("text"));
    }
}
