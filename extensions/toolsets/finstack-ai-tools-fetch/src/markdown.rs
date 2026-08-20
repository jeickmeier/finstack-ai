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
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// Parse an ASCII tag name (letters, digits, `-`, `:`) from the start of
/// `bytes`, lowercased. Returns `(name, bytes_consumed)`; `name` is `None`
/// when `bytes` does not start with a valid name character.
///
/// This is a strict subset of html5ever's real tag-name tokenizer state,
/// which appends essentially any byte (`_`, `.`, `@`, non-ASCII, ...) to the
/// name rather than stopping. That means this function can stop mid-name on
/// input html5ever would keep consuming — see [`is_tag_name_terminator`] and
/// the doc comment on [`exceeds_safe_nesting_depth`] for why a name is only
/// trusted when the byte immediately after it is a genuine terminator.
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

/// Whether `byte` is a genuine HTML tag-name terminator: ASCII whitespace,
/// `/`, `>`, or NUL. [`parse_tag_name`]'s output is only trustworthy as a
/// *complete* tag name when the byte right after it is one of these —
/// otherwise `parse_tag_name` stopped early because it hit a character it
/// doesn't understand (e.g. `_`, `.`, `@`, non-ASCII), while html5ever's
/// tokenizer would have kept appending that character to the name. See
/// [`exceeds_safe_nesting_depth`]'s doc comment for the bypass this closes.
fn is_tag_name_terminator(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 0x0C | b'/' | b'>' | 0)
}

/// Placeholder pushed onto the depth-tracking stack in place of a real tag
/// name when [`parse_tag_name`]'s output could not be trusted (see
/// [`is_tag_name_terminator`]). Deliberately a byte sequence
/// [`parse_tag_name`] can never itself produce (it only emits ASCII
/// alphanumerics, `-`, and `:`), so this marker can never accidentally
/// string-equal a later trusted, properly-terminated tag name and get
/// popped by it — an untrusted push must stay on the stack for the rest of
/// the scan, exactly mirroring "the real element's true identity is unknown,
/// so nothing we parse later can be assumed to close it."
const UNTRUSTED_TAG_MARKER: &str = "\u{0}untrusted";

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
/// A third, independent bypass was found and fixed the same way:
/// [`parse_tag_name`] stops at the first byte outside `[A-Za-z0-9\-:]`, but
/// html5ever's real tag-name tokenizer state appends essentially *any*
/// other byte (`_`, `.`, `@`, non-ASCII, ...) to the name instead of
/// stopping there. So a void element name padded with such a byte (e.g.
/// `<img_x>`) used to truncate to a void match ("img") and get skipped,
/// while html5ever parsed a *different*, non-void element ("`img_x`") that
/// stayed open and nested — `"<img_x>".repeat(100_000)` reproduced the same
/// SIGABRT. The same truncation is a bypass on the closing side too: a
/// padded close like `</div_x>` used to truncate to "div" and could
/// wrongly pop a genuinely open `<div>`, when html5ever would treat
/// `</div_x>` as an unmatched end tag for an unrelated element and ignore
/// it. The fix ([`is_tag_name_terminator`]) inspects the byte immediately
/// after a parsed name: the name is trusted only when that byte is a real
/// HTML tag-name terminator (ASCII whitespace, `/`, `>`, or NUL); anything
/// else means the name is a truncation of something this scan cannot
/// identify, and both directions now fail closed on that — an untrusted
/// opening tag is pushed unconditionally (bypassing the void check, see
/// [`UNTRUSTED_TAG_MARKER`]) rather than skipped, and an untrusted closing
/// tag never pops. Only an exact-and-terminated name may match
/// [`VOID_ELEMENTS`] or pop the stack.
///
/// Because closes only pop on an exact-and-terminated match, a trailing `/`
/// no longer suppresses a push except for true void elements, and an
/// untrusted name is always pushed rather than trusted either way, this now
/// genuinely can only reject more documents than strictly necessary, never
/// fewer: any push this scan misses would have to come from a tag html5ever
/// also wouldn't count as an open element, and any close this scan fails to
/// apply (a mismatched, untrusted, or otherwise non-matching close) only
/// leaves the tracked depth higher than reality, not lower. Known
/// false-positive sources from that same conservative bias:
///   - Implied closes handled by html5ever's tree-construction
///     adoption-agency / implied-end-tag rules (e.g. a huge run of sibling
///     `<li>`s that HTML treats as auto-closing one another, or misnesting
///     patterns like `<b><i></b>`) are not modeled here.
///   - Genuine foreign-content self-closing elements (SVG/MathML, e.g.
///     `<path/>`, `<circle/>`) ARE honored by html5ever as self-closing, but
///     this scan has no namespace awareness and pushes them like any other
///     non-void element, so an SVG-heavy document with more than
///     [`MAX_SCAN_DEPTH`] such elements will over-count and trip the guard.
///   - Any tag name this scan cannot fully parse (an untrusted name, per
///     above) is always pushed, whether or not the real element html5ever
///     builds is void or otherwise short-lived, so a large run of oddly
///     punctuated tag-like fragments (attacker-chosen or otherwise) can
///     trip the guard even if html5ever's real tree stays shallow.
///
/// In every case a legitimate document at extreme scale could trip this
/// guard and fall back to plain-text delivery even though html5ever would
/// have handled it without unbounded real nesting. That fallback is inline
/// text, not an error, so this is an accepted, documented tradeoff.
///
/// A differential test (`differential_guard_never_under_trips_against_the_real_parser`,
/// below) parses an adversarial corpus with the real `html5ever` +
/// `markup5ever_rcdom` parser and asserts the invariant this whole function
/// exists to uphold: whenever the real DOM's depth exceeds
/// [`MAX_SCAN_DEPTH`], this guard trips. It may trip early (over-count); it
/// must never fail to trip when the real parser would recurse past the cap.
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
            // Closing tag: pop only when the parsed name is BOTH an exact
            // match with the top of the stack AND properly terminated (see
            // the doc comment above for why an unconditional decrement is
            // unsound, and for the untrusted-name direction of the third
            // bypass this terminator check closes). A truncated name (e.g.
            // `</div_x>` parsing as "div") must never be allowed to pop a
            // genuinely open "div" -- html5ever would treat `</div_x>` as
            // an unmatched end tag for an unrelated element and ignore it
            // entirely, so this scan must too.
            let (name, consumed) = parse_tag_name(&bytes[i + 2..]);
            let after_name = i + 2 + consumed;
            let terminated = bytes
                .get(after_name)
                .is_none_or(|&b| is_tag_name_terminator(b));
            let tag_end =
                find_gt(&bytes[after_name..]).map_or(bytes.len(), |offset| after_name + offset + 1);
            if terminated
                && let Some(name) = name
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
        // `<div>` would. Only [`VOID_ELEMENTS`] are exempt from the stack —
        // and only when the parsed name is properly terminated (see
        // [`is_tag_name_terminator`]): a truncated name (e.g. `<img_x>`
        // parsing as "img") must NOT be treated as the void element "img",
        // because html5ever's real tag name is "img_x", a non-void element
        // that stays open. An untrusted name is pushed unconditionally
        // (skipping the void check entirely) using
        // [`UNTRUSTED_TAG_MARKER`] rather than the truncated string, so it
        // can never later be matched and popped by an unrelated, correctly
        // terminated closing tag. See the doc comment above for the second
        // and third bypasses this closed and the residuals both accept.
        let (name, consumed) = parse_tag_name(&bytes[i + 1..]);
        let after_name = i + 1 + consumed;
        let terminated = bytes
            .get(after_name)
            .is_none_or(|&b| is_tag_name_terminator(b));
        let tag_end =
            find_gt(&bytes[after_name..]).map_or(bytes.len(), |offset| after_name + offset + 1);
        if let Some(name) = name {
            let pushed = if !terminated {
                Some(UNTRUSTED_TAG_MARKER.to_owned())
            } else if VOID_ELEMENTS.contains(&name.as_str()) {
                None
            } else {
                Some(name)
            };
            if let Some(pushed) = pushed {
                stack.push(pushed);
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

    #[test]
    fn truncated_void_name_padding_does_not_bypass_the_depth_guard() {
        // Critical regression: an earlier version of `parse_tag_name` had no
        // notion of a "terminator", so a void element name padded with a
        // byte outside `[A-Za-z0-9\-:]` (e.g. `<img_x>`) truncated to a void
        // match ("img") and was skipped, while html5ever parsed the real,
        // *different*, non-void element "img_x" that stayed open and
        // nested. `"<img_x>".repeat(100_000)` reported "not exceeded" from
        // the old guard while `htmd::HtmlToMarkdown::convert` on the same
        // input aborted the process (SIGABRT). Easier to trigger than
        // either prior bypass: one repeated fragment, no padding trick.
        let html = "<img_x>".repeat(100_000);
        let result = html_to_markdown(&html, BIG_CAP);
        assert!(
            result.is_none(),
            "expected the depth guard to trip on truncated-void-name padding"
        );
    }

    #[test]
    fn truncated_close_padding_does_not_bypass_the_depth_guard() {
        // The closing-tag direction of the same bypass: a genuinely open
        // run of `<div>`s followed by padded closes (`</div_x>`) that
        // truncate to "div" must NOT be allowed to pop them. html5ever
        // treats `</div_x>` as an unmatched end tag for an unrelated
        // element and ignores it, so all the `<div>`s stay open and nested.
        let opens = "<div>".repeat(1_000);
        let padded_closes = "</div_x>".repeat(1_000);
        let html = format!("{opens}{padded_closes}");
        let result = html_to_markdown(&html, BIG_CAP);
        assert!(
            result.is_none(),
            "expected the depth guard to trip: padded closes must not pop real opens"
        );
    }

    // --- Differential test against the real parser -------------------------
    //
    // Three bypasses have now slipped through this heuristic guard, each
    // only found by a reviewer hand-compiling a harness against the real
    // parser. This brings that harness into the suite permanently: for an
    // adversarial corpus, assert that whenever the REAL DOM's max depth
    // exceeds the cap, the guard also trips. The guard may over-trip
    // (reject a document the real parser would have handled); it must never
    // under-trip.

    use html5ever::tendril::TendrilSink;
    use html5ever::{ParseOpts, parse_document};
    use markup5ever_rcdom::{Handle, NodeData, RcDom};

    /// The real maximum element-nesting depth of `html`, as built by the
    /// same parser (`html5ever`/`markup5ever_rcdom`) that `htmd` uses
    /// internally. Walked iteratively (an explicit `Vec` stack, never Rust
    /// call-stack recursion) so that measuring an adversarial, very-deep
    /// corpus item in a test never itself risks the exact failure mode this
    /// guard exists to prevent.
    fn real_dom_max_depth(html: &str) -> usize {
        let dom = parse_document(RcDom::default(), ParseOpts::default())
            .from_utf8()
            .read_from(&mut html.as_bytes())
            .expect("html5ever's TendrilSink does not fail on arbitrary UTF-8 input");
        let mut max_depth = 0usize;
        let mut stack: Vec<(Handle, usize)> = vec![(dom.document.clone(), 0)];
        while let Some((node, depth)) = stack.pop() {
            if matches!(node.data, NodeData::Element { .. }) {
                max_depth = max_depth.max(depth);
            }
            for child in node.children.borrow().iter() {
                stack.push((child.clone(), depth + 1));
            }
        }
        max_depth
    }

    #[test]
    fn differential_guard_never_under_trips_against_the_real_parser() {
        // N=1000 keeps this fast (the dedicated 100_000-repetition cases
        // above remain as the slower crash regressions); it's already well
        // past MAX_SCAN_DEPTH (512) for every case here that should trip.
        const N: usize = 1_000;
        let corpus: Vec<(&str, String)> = vec![
            ("well_formed_div_opens", "<div>".repeat(N)),
            ("mismatched_close_padding", "<div></span>".repeat(N)),
            ("self_closing_non_void", "<div/>".repeat(N)),
            ("truncated_void_underscore", "<img_x>".repeat(N)),
            ("truncated_void_dot", "<br.x>".repeat(N)),
            ("truncated_void_at", "<hr@x>".repeat(N)),
            ("truncated_void_dot_input", "<input.x>".repeat(N)),
            ("truncated_void_non_ascii", "<img\u{00ef}>".repeat(N)),
            (
                "truncated_close_direction",
                format!("{}{}", "<div>".repeat(N), "</div_x>".repeat(N)),
            ),
            ("quoted_gt_in_attribute", "<div title=\"a>b\">".repeat(N)),
            (
                "comment_wrapped_fake_tags",
                "<!-- <div><div><div> -->".repeat(N),
            ),
        ];

        for (label, html) in corpus {
            let real_depth = real_dom_max_depth(&html);
            if real_depth > super::MAX_SCAN_DEPTH {
                assert!(
                    super::exceeds_safe_nesting_depth(&html),
                    "corpus item {label:?}: real DOM depth {real_depth} exceeds \
                     MAX_SCAN_DEPTH ({}) but the guard did not trip -- this is a bypass",
                    super::MAX_SCAN_DEPTH
                );
            }
        }
    }

    #[test]
    fn differential_guard_does_not_falsely_trip_on_a_shallow_legitimate_document() {
        // The flip side of the invariant above, checked directly for one
        // concrete legitimate case rather than corpus-wide: a real,
        // ~100-deep document must convert normally, not fall back to
        // plain text.
        let html = nested_divs(100);
        assert!(
            real_dom_max_depth(&html) < super::MAX_SCAN_DEPTH,
            "sanity: this corpus item's real depth must stay under the cap"
        );
        assert!(!super::exceeds_safe_nesting_depth(&html));
        assert!(html_to_markdown(&html, BIG_CAP).is_some());
    }
}
