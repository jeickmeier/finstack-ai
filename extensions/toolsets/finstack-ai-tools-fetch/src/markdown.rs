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

/// HTML void elements (per the living standard, never have a closing tag)
/// that must not push onto the depth-tracking stack in
/// [`exceeds_safe_nesting_depth`].
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
    "source", "track", "wbr",
];

/// Parse an ASCII tag name (letters, digits, `-`, `:`) from the start of
/// `bytes`, lowercased. Returns `(name, bytes_consumed)`; `name` is `None`
/// when `bytes` does not start with a valid name character.
fn parse_tag_name(bytes: &[u8]) -> (Option<String>, usize) {
    let len = bytes
        .iter()
        .take_while(|b| b.is_ascii_alphanumeric() || **b == b'-' || **b == b':')
        .count();
    if len == 0 {
        (None, 0)
    } else {
        (
            Some(String::from_utf8_lossy(&bytes[..len]).to_ascii_lowercase()),
            len,
        )
    }
}

/// Byte offset of the next `>` in `bytes`, if any.
fn find_gt(bytes: &[u8]) -> Option<usize> {
    bytes.iter().position(|&b| b == b'>')
}

/// Cheap pre-parse guard against pathologically deep HTML (F-4): `htmd`'s
/// underlying `html5ever` parse produces a DOM that is walked, and dropped,
/// recursively, so a document with tens or hundreds of thousands of nested
/// elements can exhaust the stack and abort the process rather than
/// returning an error — confirmed by a 100,000-level `<div>` document
/// aborting with SIGABRT before this guard existed.
///
/// This is a single-pass byte scan, not a real parser, but it does track a
/// bounded stack of open tag names (lowercased) rather than a bare integer
/// counter. That distinction matters: an earlier version of this guard
/// counted `</` transitions as unconditional decrements, which is *wrong*
/// for the direction that matters, because html5ever ignores an end tag
/// that has no matching open element on its stack of open elements (per the
/// HTML parsing spec's tree-construction algorithm) rather than treating it
/// as a decrement. A document built from `<div></span>` repeated 100,000
/// times therefore kept a naive counter at depth <= 1 while html5ever's real
/// DOM nested 100,000 unclosed `<div>`s — the stack-exhaustion abort stayed
/// reachable through that gap. This version fixes that: a closing tag pops
/// the stack ONLY when its name equals the top of the stack; a mismatched or
/// stray close is ignored (no pop), mirroring html5ever's ignore-unmatched
/// behavior in the conservative direction.
///
/// A second, independent bypass was found and fixed the same way: a trailing
/// `/` on an opening tag (`<div/>`) used to be treated as self-closing and
/// therefore skipped the push, but the HTML5 tree-construction algorithm
/// only honors that slash on void elements and foreign-content (SVG/MathML)
/// elements — on an ordinary HTML element like `<div/>` it is IGNORED and
/// the element stays open exactly as `<div>` would. `"<div/>".repeat(100_000)`
/// therefore pushed nothing under the old rule while html5ever built a real
/// DOM ~100,000 elements deep, and the guard reported "not exceeded" right
/// up to the SIGABRT. Only [`VOID_ELEMENTS`] are now exempt from the push;
/// a trailing `/` on anything else no longer matters.
///
/// Because closes only pop on an exact match, and a trailing `/` no longer
/// suppresses a push except for true void elements, this now genuinely can
/// only reject more documents than strictly necessary, never fewer: any
/// push this scan misses would have to come from a tag html5ever also
/// wouldn't count as an open element, and any close this scan fails to
/// apply (a mismatched close) only leaves the tracked depth higher than
/// reality, not lower. Known false-positive sources from that same
/// conservative bias:
///   - Implied closes handled by html5ever's tree-construction
///     adoption-agency / implied-end-tag rules (e.g. a huge run of sibling
///     `<li>`s that HTML treats as auto-closing one another, or misnesting
///     patterns like `<b><i></b>`) are not modeled here.
///   - Genuine foreign-content self-closing elements (SVG/MathML, e.g.
///     `<path/>`, `<circle/>`) ARE honored by html5ever as self-closing, but
///     this scan has no namespace awareness and pushes them like any other
///     non-void element, so an SVG-heavy document with more than
///     [`MAX_SCAN_DEPTH`] such elements will over-count and trip the guard.
///
/// In both cases a legitimate document at extreme scale could trip this
/// guard and fall back to plain-text delivery even though html5ever would
/// have handled it without unbounded real nesting. That fallback is inline
/// text, not an error, so this is an accepted, documented tradeoff.
///
/// `MAX_SCAN_DEPTH` (512) is well above any HTML a legitimate document is
/// likely to nest by hand or by templating, and well below the depth that
/// risks stack exhaustion in the underlying parser/DOM-walk/Drop.
fn exceeds_safe_nesting_depth(html: &str) -> bool {
    let bytes = html.as_bytes();
    let mut stack: Vec<String> = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] != b'<' {
            i += 1;
            continue;
        }
        if bytes.get(i + 1) == Some(&b'/') {
            // Closing tag: pop only on an exact match with the top of the
            // stack (see the doc comment above for why an unconditional
            // decrement is unsound here).
            let (name, consumed) = parse_tag_name(&bytes[i + 2..]);
            let after_name = i + 2 + consumed;
            let tag_end = find_gt(&bytes[after_name..]).map_or(bytes.len(), |offset| after_name + offset + 1);
            if let Some(name) = name
                && stack.last() == Some(&name)
            {
                stack.pop();
            }
            i = tag_end.max(i + 1);
            continue;
        }
        if !bytes.get(i + 1).is_some_and(u8::is_ascii_alphabetic) {
            i += 1;
            continue;
        }
        // Opening tag. A trailing `/` (`<div/>`) is NOT treated as a reason
        // to skip the push: per the HTML5 tree-construction algorithm, a
        // self-closing slash on a non-void, non-foreign (HTML-namespace)
        // element is ignored, and the element stays open exactly like
        // `<div>` would. Only [`VOID_ELEMENTS`] are exempt from the stack;
        // see the doc comment above for the second bypass this closed and
        // the foreign-content (SVG/MathML) residual it accepts.
        let (name, consumed) = parse_tag_name(&bytes[i + 1..]);
        let after_name = i + 1 + consumed;
        let tag_end = find_gt(&bytes[after_name..]).map_or(bytes.len(), |offset| after_name + offset + 1);
        if let Some(name) = name {
            let is_void = VOID_ELEMENTS.contains(&name.as_str());
            if !is_void {
                stack.push(name);
                if stack.len() > MAX_SCAN_DEPTH {
                    return true;
                }
            }
        }
        i = tag_end.max(i + 1);
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

    /// Build a document with `count` repetitions of `<div></span>`: an
    /// unclosed `<div>` immediately followed by a `</span>` close that
    /// cannot match it. html5ever ignores an end tag with no matching open
    /// element, so every `<div>` here stays open in the real DOM while the
    /// `</span>` closes stay unmatched — exactly the mismatched-close
    /// padding that bypassed the original (unconditional-decrement) guard.
    fn mismatched_close_padding(count: usize) -> String {
        "<div></span>".repeat(count)
    }

    #[test]
    fn mismatched_close_padding_does_not_bypass_the_depth_guard() {
        // Critical regression: an earlier version of `exceeds_safe_nesting_depth`
        // decremented on ANY `</...>` sequence, so 100_000 repetitions of
        // `<div></span>` kept its counter near 0 even though html5ever's
        // real DOM nests 100_000 unclosed `<div>`s -- the stack-exhaustion
        // abort stayed reachable. The fixed guard tracks a stack of open
        // tag names and pops only on an exact match, so `</span>` never
        // pops the `<div>` on top; the stack keeps growing and trips the
        // cap well before conversion, returning `None`. Critically, this
        // must not crash the process either way.
        let html = mismatched_close_padding(100_000);
        let result = html_to_markdown(&html, BIG_CAP);
        assert!(
            result.is_none(),
            "expected the depth guard to trip on mismatched-close padding"
        );
    }

    #[test]
    fn self_closing_non_void_padding_does_not_bypass_the_depth_guard() {
        // Critical regression: an earlier version of `exceeds_safe_nesting_depth`
        // treated a trailing `/` on ANY opening tag as self-closing and
        // skipped the push. But the HTML5 tree-construction algorithm only
        // honors that slash on void elements and foreign-content
        // (SVG/MathML) elements -- on an ordinary element like `<div/>` it
        // is ignored and the element stays open exactly as `<div>` would.
        // `"<div/>".repeat(100_000)` therefore pushed nothing under the old
        // rule (the guard reported "not exceeded") while html5ever built a
        // real DOM ~100,000 elements deep and aborted the process. This is
        // an easier trigger than the mismatched-close bypass above: one
        // repeated fragment, no padding trick. The fixed guard no longer
        // treats a trailing `/` as a reason to skip the push for a
        // non-void element, so the stack grows and trips the cap here too.
        let html = "<div/>".repeat(100_000);
        let result = html_to_markdown(&html, BIG_CAP);
        assert!(
            result.is_none(),
            "expected the depth guard to trip on self-closing non-void padding"
        );
    }
}
