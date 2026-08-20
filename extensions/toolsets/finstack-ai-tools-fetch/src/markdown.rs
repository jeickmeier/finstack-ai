//! HTML → Markdown conversion for the `Auto`/`Markdown` delivery modes
//! (spec §4.5).
//!
//! Conversion is best-effort: any failure, or a converted result that
//! exceeds `output_cap`, degrades to [`None`] so the caller can fall back to
//! inlining the original text, which is already budget-checked. This
//! function never returns an error.

use std::cell::{Cell, RefCell};

use html5ever::tendril::StrTendril;
use html5ever::tokenizer::{
    BufferQueue, Tag, TagKind, Token, TokenSink, TokenSinkResult, Tokenizer, TokenizerOpts,
};
use htmd::HtmlToMarkdown;

/// Cap on the nesting-depth scan performed by [`exceeds_safe_nesting_depth`]
/// before a document is handed to `htmd`/`html5ever`'s full tree-building
/// parse. See that function's doc comment for what "depth" means here and
/// why the cap is heuristic rather than exact.
const MAX_SCAN_DEPTH: usize = 512;

/// HTML void elements (per the living standard, never have a closing tag)
/// that must not push onto the depth-tracking stack in
/// [`exceeds_safe_nesting_depth`].
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr",
];

/// [`TokenSink`] that tracks the depth of the open-element stack a real
/// html5ever tree builder would maintain, without building a tree, and
/// signals early once [`MAX_SCAN_DEPTH`] would be exceeded. See
/// [`exceeds_safe_nesting_depth`]'s doc comment for the full rationale.
struct DepthCounter {
    stack: RefCell<Vec<html5ever::LocalName>>,
    exceeded: Cell<bool>,
}

impl TokenSink for DepthCounter {
    type Handle = ();

    fn process_token(&self, token: Token, _line_number: u64) -> TokenSinkResult<()> {
        let Token::TagToken(Tag { kind, name, .. }) = token else {
            // Comments, bogus comments, CDATA-as-bogus-comment, doctypes,
            // NUL characters, and ordinary character data never affect
            // depth and are ignored outright -- the tokenizer has already
            // done the real parsing work of telling these apart from tags.
            return TokenSinkResult::Continue;
        };
        match kind {
            TagKind::StartTag => {
                // Void elements never hold the open-elements stack open,
                // regardless of a trailing self-closing `/` (which the
                // tokenizer resolves per-spec; void status is what actually
                // matters, not the flag). Everything else is pushed exactly
                // once, using the exact name the tokenizer parsed -- no
                // truncation is possible here, because this name is
                // produced by the same tag-name tokenizer state
                // html5ever's tree builder itself consumes.
                if !VOID_ELEMENTS.contains(&&*name) {
                    let mut stack = self.stack.borrow_mut();
                    stack.push(name);
                    if stack.len() > MAX_SCAN_DEPTH {
                        self.exceeded.set(true);
                        drop(stack);
                        // There is no explicit "abort" TokenSinkResult; but
                        // returning `Script` causes the tokenizer's run
                        // loop to return early rather than keep tokenizing
                        // the rest of a potentially huge document (see
                        // `html5ever::tokenizer::Tokenizer::run`, which
                        // matches this variant and returns immediately).
                        // `Handle = ()` here has no meaning beyond that.
                        return TokenSinkResult::Script(());
                    }
                }
            }
            TagKind::EndTag => {
                // Pop only on an exact match with the top of the stack,
                // mirroring html5ever's tree-construction rule that an end
                // tag with no matching open element is ignored rather than
                // treated as a decrement.
                let mut stack = self.stack.borrow_mut();
                if stack.last() == Some(&name) {
                    stack.pop();
                }
            }
        }
        TokenSinkResult::Continue
    }
}

/// Cheap pre-parse guard against pathologically deep HTML (F-4): `htmd`'s
/// underlying `html5ever` tree-building parse produces a DOM that is
/// walked, and dropped, recursively, so a document with tens or hundreds of
/// thousands of nested elements can exhaust the stack and abort the process
/// rather than returning an error — confirmed repeatedly by SIGABRT
/// reproductions during this guard's development.
///
/// # History: five rounds, one lesson
///
/// This guard went through five review rounds before landing on its current
/// design, and every one of the first four found a new way a hand-rolled
/// byte scanner disagreed with html5ever's real tokenizer:
///   1. An integer counter that decremented on any `</...>` unconditionally
///      — bypassed by mismatched-close padding (`<div></span>` repeated),
///      since html5ever ignores an end tag with no matching open element
///      rather than treating it as a decrement.
///   2. A tag-name stack that treated any trailing `/` as self-closing —
///      bypassed by `<div/>` repeated, since HTML5 only honors a
///      self-closing slash on void and foreign-content (SVG/MathML)
///      elements; on an ordinary element it is ignored and the element
///      stays open.
///   3. A name parser that stopped at the first byte outside
///      `[A-Za-z0-9\-:]` — bypassed by `<img_x>` repeated, since a byte like
///      `_` truncated the parsed name to the void element "img" while
///      html5ever's real tokenizer appends that byte to the name, building
///      a different, non-void, stays-open element.
///   4. Adding a terminator check for that truncation still had no notion
///      of comments, CDATA, bogus comments, or doctypes at all — bypassed
///      by `<div><!--</div>-->` repeated (the scanner read the `</div>`
///      *inside* the comment as a real close for the genuinely open `div`)
///      and by `NUL` inside a name being treated as a terminator when
///      html5ever actually replaces NUL with U+FFFD as a name
///      *continuation* byte, not a terminator.
///
/// Each fix closed the specific bypass a reviewer found by hand-compiling a
/// differential harness against the real parser — and each fix left room
/// for the next one, because a hand-rolled scanner cannot be made to agree
/// with a real tokenizer by iterative patching; there is always another
/// production rule it doesn't know about.
///
/// # Current design: drive html5ever's own tokenizer
///
/// This version does not parse HTML itself at all. It drives
/// `html5ever::tokenizer::Tokenizer` — the same tokenizer html5ever's tree
/// builder consumes, and thus the same one `htmd` uses internally — with a
/// [`DepthCounter`] [`TokenSink`] that:
///   - on a start tag: pushes the tokenizer-supplied name onto a stack
///     unless it is a [`VOID_ELEMENTS`] entry, and stops early (via
///     [`TokenSinkResult::Script`], the only early-exit signal a bare
///     tokenizer honors) the moment the stack would exceed
///     [`MAX_SCAN_DEPTH`], so a pathological document is never tokenized in
///     full;
///   - on an end tag: pops only when the name exactly matches the top of
///     the stack, exactly as html5ever's tree builder does for unmatched
///     end tags;
///   - ignores every other token kind (comments, bogus comments treated as
///     comments, CDATA-outside-foreign-content treated as a bogus comment,
///     doctypes, character data, and NUL characters) outright.
///
/// Because comments, bogus comments, CDATA, doctypes, NUL-in-a-name,
/// attribute values containing `>` or `</div>`-looking text, character
/// references, and case folding are all resolved *inside the tokenizer
/// itself* — identically to what the tree builder will see, since it is
/// the literal same component — none of the five prior bypasses are
/// reachable through this scan any more: there is no separate byte-scanning
/// logic left to disagree with html5ever about what a tag, a comment, or a
/// truncated name is.
///
/// ## What is NOT claimed
///
/// This does **not** claim to only ever reject more documents than
/// strictly necessary, never fewer — that claim was accurate-sounding but
/// false for every prior hand-rolled version, and is not repeated here.
/// What *is* true: this scan uses the exact same tokenizer as the real
/// parse, so the only remaining divergence from what the tree builder
/// would do is where this scan is *not* the tree builder:
///   - **Rawtext state.** A real tree builder switches the tokenizer into
///     rawtext mode for `<script>`, `<style>`, `<textarea>`, and `<title>`
///     content (so a literal `<div>` or `</div>` inside a `<script>` body
///     is just text, not markup). The bare tokenizer used here has no tree
///     builder driving that switch, so it tokenizes rawtext-element bodies
///     as ordinary markup. Every fake tag this produces is still pushed
///     under the same last-in-first-out matching discipline as real tags,
///     so a fake push can only ever get "stuck" behind (blocking a correct
///     pop of) whatever is really open — it cannot itself cause an
///     erroneous pop of a genuinely open ancestor, because a pop only
///     applies when the name exactly matches the current top, and entering
///     a rawtext element's body always pushes that element as the new top
///     first. Net effect: over-counting, not under-counting.
///   - **Implied end tags and the adoption agency.** html5ever's real tree
///     construction algorithm auto-closes some elements implicitly (e.g. a
///     new `<li>` closing a previous open `<li>`) and resolves certain
///     misnesting patterns (e.g. `<b><i></b>`) via the adoption agency
///     algorithm. Both are tree-construction rules, not tokenizer rules,
///     so this scan does not model them: an element that the real tree
///     builder would have implicitly closed stays open in this scan's
///     stack. Net effect: over-counting, not under-counting.
///   - **Foreign content (SVG/MathML) self-closing.** The tokenizer reports
///     a `self_closing` flag on every tag, but whether that flag is
///     honored depends on the element's namespace, which only the tree
///     builder tracks. This scan ignores the flag entirely (matching round
///     2's fix) and always pushes non-void elements regardless, so a
///     genuine foreign-content self-closing element (`<path/>`,
///     `<circle/>`) is over-counted as staying open.
///
/// In every documented case above, the divergence biases toward this scan
/// tracking *more* open elements than the real tree builder would, which is
/// the fail-closed direction: a legitimate document using these patterns at
/// extreme scale could trip this guard and fall back to plain-text
/// delivery even though html5ever would have handled it without unbounded
/// real nesting. That fallback is inline text, not an error, so this is an
/// accepted, documented tradeoff. No divergence that causes *under*-
/// counting (missing a push, or an erroneous pop of something genuinely
/// open) is known; the differential and generative tests below exist
/// specifically to keep checking that claim rather than asserting it once
/// and trusting it forever.
///
/// A differential test
/// (`differential_guard_never_under_trips_against_the_real_parser`) and a
/// seeded-PRNG generative test
/// (`generative_fuzz_guard_never_under_trips_against_the_real_parser`,
/// below) parse adversarial and randomized corpora with the real
/// `html5ever` + `markup5ever_rcdom` parser and assert the invariant this
/// whole function exists to uphold: whenever the real DOM's depth exceeds
/// [`MAX_SCAN_DEPTH`], this guard trips. It may trip early (over-count); it
/// must never fail to trip when the real parser would recurse past the cap.
///
/// `MAX_SCAN_DEPTH` (512) is well above any HTML a legitimate document is
/// likely to nest by hand or by templating, and well below the depth that
/// risks stack exhaustion in the underlying parser/DOM-walk/Drop.
fn exceeds_safe_nesting_depth(html: &str) -> bool {
    let sink = DepthCounter {
        stack: RefCell::new(Vec::new()),
        exceeded: Cell::new(false),
    };
    let input = BufferQueue::default();
    input.push_back(StrTendril::from_slice(html));
    let tokenizer = Tokenizer::new(sink, TokenizerOpts::default());
    // A single `feed` call processes the whole buffer (it is one tendril
    // covering the entire document); we deliberately never call `feed`
    // again or `end()` -- if the sink already tripped, there is nothing
    // further to learn and no reason to keep tokenizing a possibly huge
    // remainder.
    let _ = tokenizer.feed(&input);
    tokenizer.sink.exceeded.get()
}

/// Convert `html` to Markdown, dropping scripts/styles/comments.
///
/// Returns `None` when conversion fails, when the input's nesting depth
/// exceeds [`MAX_SCAN_DEPTH`] (see [`exceeds_safe_nesting_depth`], F-4), or
/// when the converted Markdown's byte length exceeds `output_cap` — in
/// every case the caller falls back to inlining the original text.
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

    // Goldens are committed with a single trailing newline; `htmd`'s output
    // has none, so every comparison below trims both sides consistently.

    #[test]
    fn nested_lists_match_golden() {
        let html = include_str!("../fixtures/nested_lists.html");
        let want = include_str!("../fixtures/nested_lists.md").trim_end();
        let markdown = html_to_markdown(html, BIG_CAP).expect("conversion succeeds");
        assert_eq!(markdown.trim_end(), want);
    }

    #[test]
    fn table_matches_golden() {
        let html = include_str!("../fixtures/table.html");
        let want = include_str!("../fixtures/table.md").trim_end();
        let markdown = html_to_markdown(html, BIG_CAP).expect("conversion succeeds");
        assert_eq!(markdown.trim_end(), want);
    }

    #[test]
    fn script_and_style_are_dropped() {
        let html = include_str!("../fixtures/script_style.html");
        let want = include_str!("../fixtures/script_style.md").trim_end();
        let markdown = html_to_markdown(html, BIG_CAP).expect("conversion succeeds");
        assert_eq!(markdown.trim_end(), want);
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
        // stack during parse/convert/drop.
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

    #[test]
    fn mismatched_close_padding_does_not_bypass_the_depth_guard() {
        // Bypass 1 regression: `<div></span>` repeated leaves every `<div>`
        // genuinely open in the real DOM (the `</span>` never matches), so
        // this must still trip the guard, not decrement past it.
        let html = "<div></span>".repeat(100_000);
        let result = html_to_markdown(&html, BIG_CAP);
        assert!(
            result.is_none(),
            "expected the depth guard to trip on mismatched-close padding"
        );
    }

    #[test]
    fn self_closing_non_void_padding_does_not_bypass_the_depth_guard() {
        // Bypass 2 regression: `<div/>` repeated stays open in the real DOM
        // (self-closing is ignored on a non-void, non-foreign element).
        let html = "<div/>".repeat(100_000);
        let result = html_to_markdown(&html, BIG_CAP);
        assert!(
            result.is_none(),
            "expected the depth guard to trip on self-closing non-void padding"
        );
    }

    #[test]
    fn truncated_void_name_padding_does_not_bypass_the_depth_guard() {
        // Bypass 3 regression: `<img_x>` is a different, non-void element
        // from the void `<img>`, and stays open in the real DOM.
        let html = "<img_x>".repeat(100_000);
        let result = html_to_markdown(&html, BIG_CAP);
        assert!(
            result.is_none(),
            "expected the depth guard to trip on truncated-void-name padding"
        );
    }

    #[test]
    fn truncated_close_padding_does_not_bypass_the_depth_guard() {
        // Bypass 3 regression, closing-tag direction: a genuinely open run
        // of `<div>`s followed by padded closes (`</div_x>`) that must not
        // pop them (html5ever ignores `</div_x>` as an unmatched end tag).
        let opens = "<div>".repeat(1_000);
        let padded_closes = "</div_x>".repeat(1_000);
        let html = format!("{opens}{padded_closes}");
        let result = html_to_markdown(&html, BIG_CAP);
        assert!(
            result.is_none(),
            "expected the depth guard to trip: padded closes must not pop real opens"
        );
    }

    #[test]
    fn comment_wrapped_close_padding_does_not_bypass_the_depth_guard() {
        // Bypass 4 regression: the scanner had no comment state at all, so
        // `<div><!--</div>-->` repeated let the `</div>` *inside* the
        // comment pop the genuinely open `<div>` right next to it -- the
        // tracked stack oscillated 0<->1 while html5ever's real DOM nested
        // 1000+ deep (every `<div>` stays open; comments are never tags).
        let html = "<div><!--</div>-->".repeat(100_000);
        let result = html_to_markdown(&html, BIG_CAP);
        assert!(
            result.is_none(),
            "expected the depth guard to trip: a `</div>` inside a comment must not pop a real div"
        );
    }

    #[test]
    fn nul_padded_close_does_not_bypass_the_depth_guard() {
        // Bypass 5 regression: NUL was wrongly treated as a tag-name
        // terminator. html5ever replaces NUL with U+FFFD *inside* a name
        // (a continuation byte, not a terminator), so `</div\0>` is an
        // unmatched end tag for a name that isn't "div" and must not pop a
        // genuinely open `<div>`.
        let opens = "<div>".repeat(1_000);
        let padded_closes = "</div\u{0}>".repeat(1_000);
        let html = format!("{opens}{padded_closes}");
        let result = html_to_markdown(&html, BIG_CAP);
        assert!(
            result.is_none(),
            "expected the depth guard to trip: a NUL-padded close must not pop a real open"
        );
    }

    // --- Differential test against the real parser -------------------------
    //
    // Five bypasses have now slipped through this guard's various
    // hand-rolled scanning approaches, each only found by a reviewer
    // hand-compiling a harness against the real parser. This brings that
    // harness into the suite permanently: for an adversarial corpus, assert
    // that whenever the REAL DOM's max depth exceeds the cap, the guard
    // also trips. The guard may over-trip (reject a document the real
    // parser would have handled); it must never under-trip.

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

    /// Assert the guard's core invariant for one document: if the real DOM
    /// exceeds the cap, the guard must trip. Never asserts the converse.
    fn assert_guard_never_under_trips(label: &str, html: &str) {
        let real_depth = real_dom_max_depth(html);
        if real_depth > super::MAX_SCAN_DEPTH {
            assert!(
                super::exceeds_safe_nesting_depth(html),
                "{label:?}: real DOM depth {real_depth} exceeds MAX_SCAN_DEPTH \
                 ({}) but the guard did not trip -- this is a bypass",
                super::MAX_SCAN_DEPTH
            );
        }
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
                "comment_wrapped_close_padding",
                "<div><!--</div>-->".repeat(N),
            ),
            ("cdata_wrapped_close_padding", "<div><![CDATA[</div>]]>".repeat(N)),
            ("processing_instruction_close_padding", "<div><?</div>></div>".repeat(N)),
            ("bogus_comment_close_padding", "<div><!</div>></div>".repeat(N)),
            (
                "doctype_wrapped_close_padding",
                "<div><!DOCTYPE </div>></div>".repeat(N),
            ),
            (
                "nul_padded_close_direction",
                format!("{}{}", "<div>".repeat(N), "</div\u{0}>".repeat(N)),
            ),
        ];

        for (label, html) in &corpus {
            assert_guard_never_under_trips(label, html);
        }

        // The comment-wrapped fake-tags case only makes sense as a shallow
        // *negative* check: real depth here is tiny (the fake tags never
        // leave the comment), so the invariant above would pass vacuously.
        // Assert the shallow direction explicitly instead of leaving it
        // untested.
        let shallow_comment_html = "<!-- <div><div><div> -->".repeat(N);
        let shallow_real_depth = real_dom_max_depth(&shallow_comment_html);
        assert!(
            shallow_real_depth <= 3,
            "sanity: fake tags inside a comment must not create real nesting, got depth {shallow_real_depth}"
        );
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

    // --- Generative (seeded PRNG) differential test -------------------------
    //
    // The hand-picked corpus above is backward-looking: every entry is a
    // known past bypass. This is a forward-looking discovery net: a small,
    // deterministic (fixed-seed) PRNG builds a few hundred randomized
    // documents from a token alphabet covering the same feature space —
    // void/non-void opens (with attributes, odd trailing bytes, NUL),
    // matching/mismatching closes, comments/CDATA/bogus-comments/doctypes,
    // rawtext blocks, and quoted attributes containing `>`/`</div>` — and
    // checks the same never-under-trips invariant against each one.

    /// Minimal deterministic PRNG (a Linear Congruential Generator, same
    /// constants as Knuth's MMIX/PCG family) -- no new dependency, and a
    /// fixed seed keeps this test's corpus, and thus CI, stable run to run.
    struct Lcg(u64);

    impl Lcg {
        fn next_u64(&mut self) -> u64 {
            self.0 = self
                .0
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            self.0
        }

        /// Uniform-ish value in `0..bound` (`bound` must be nonzero).
        fn below(&mut self, bound: usize) -> usize {
            let bound_u64 = u64::try_from(bound).unwrap_or(u64::MAX);
            usize::try_from(self.next_u64() % bound_u64).unwrap_or(0)
        }

        fn choose<'a, T>(&mut self, items: &'a [T]) -> &'a T {
            &items[self.below(items.len())]
        }
    }

    const FUZZ_VOID_NAMES: &[&str] = &["br", "img", "input", "hr", "area", "meta"];
    const FUZZ_NON_VOID_NAMES: &[&str] = &["div", "span", "p", "section", "article", "b", "i", "li"];
    const FUZZ_RAWTEXT_NAMES: &[&str] = &["script", "style", "textarea", "title"];
    /// Bytes html5ever's tokenizer treats as name *continuation* (never a
    /// terminator), so appending one mid-name changes the parsed identity
    /// rather than ending it -- exactly the shape bypass 3 exploited.
    const FUZZ_ODD_SUFFIXES: &[&str] = &["_x", ".y", "@z", "\u{00ef}"];

    /// Append one random "token" to `doc`, biased toward opening tags so a
    /// meaningful fraction of generated documents actually nest past the
    /// cap (the invariant under test is conditional on that, so a corpus
    /// that never nests deeply would exercise nothing).
    fn push_random_token(doc: &mut String, rng: &mut Lcg) {
        match rng.below(20) {
            // Plain non-void open (~55%): the main depth-building token.
            // Heavily biased so a meaningful share of generated documents
            // actually nest past the cap -- the invariant under test is
            // conditional on that, so a corpus that never nests deeply
            // would exercise nothing.
            0..=10 => {
                let name = rng.choose(FUZZ_NON_VOID_NAMES);
                doc.push('<');
                doc.push_str(name);
                if rng.below(2) == 0 {
                    doc.push_str(" title=\"a>b\"");
                }
                doc.push('>');
            }
            // Non-void open with an odd trailing byte before `>` (~15%):
            // must still count as an open (bypass 3/5 shape), never as the
            // void element it superficially resembles.
            11..=13 => {
                let name = rng.choose(FUZZ_VOID_NAMES);
                let suffix = rng.choose(FUZZ_ODD_SUFFIXES);
                doc.push('<');
                doc.push_str(name);
                doc.push_str(suffix);
                doc.push('>');
            }
            // Genuine void open (~5%): must never hold the stack open.
            14 => {
                let name = rng.choose(FUZZ_VOID_NAMES);
                doc.push('<');
                doc.push_str(name);
                doc.push('>');
            }
            // Matching close (~5%).
            15 => {
                let name = rng.choose(FUZZ_NON_VOID_NAMES);
                doc.push_str("</");
                doc.push_str(name);
                doc.push('>');
            }
            // Mismatching / truncated / NUL-padded close (~5%): must never
            // pop an unrelated genuinely open element.
            16 => {
                let name = rng.choose(FUZZ_NON_VOID_NAMES);
                let suffix = rng.choose(FUZZ_ODD_SUFFIXES);
                doc.push_str("</");
                doc.push_str(name);
                doc.push_str(suffix);
                doc.push('>');
            }
            // Comment / CDATA / bogus comment / doctype, each wrapping a
            // fake close that must never reach real tag content (~10%
            // combined).
            17 => {
                doc.push_str("<div><!--</div>-->");
            }
            18 => match rng.below(3) {
                0 => doc.push_str("<div><![CDATA[</div>]]>"),
                1 => doc.push_str("<div><?</div>>"),
                _ => doc.push_str("<div><!DOCTYPE </div>>"),
            },
            // Rawtext block containing a fake nested tag and a fake close
            // (~5%): exercises the documented rawtext-state residual.
            _ => {
                let name = rng.choose(FUZZ_RAWTEXT_NAMES);
                doc.push('<');
                doc.push_str(name);
                doc.push('>');
                doc.push_str("<div></div>");
                doc.push_str("</");
                doc.push_str(name);
                doc.push('>');
            }
        }
    }

    /// Build one deterministic pseudo-random document of roughly
    /// `token_count` tokens.
    fn generate_fuzz_doc(seed: u64, token_count: usize) -> String {
        let mut rng = Lcg(seed);
        let mut doc = String::new();
        for _ in 0..token_count {
            push_random_token(&mut doc, &mut rng);
        }
        doc
    }

    #[test]
    fn generative_fuzz_guard_never_under_trips_against_the_real_parser() {
        // Fixed seed base: deterministic corpus, stable CI. The token
        // alphabet is heavily biased toward opening tags (see
        // `push_random_token`) so a meaningful share of documents actually
        // nest past the cap -- the invariant under test is conditional on
        // that, so a corpus that never nests deeply would exercise
        // nothing. That bias needs ~1_300 tokens per document (tens of KB,
        // larger than a first-pass "few KB" estimate) before the expected
        // depth reliably clears MAX_SCAN_DEPTH (512); the vacuousness
        // assertion below exists precisely to catch this sizing drifting
        // wrong again. Runs in low single-digit seconds for 300 documents.
        const DOC_COUNT: u64 = 300;
        const TOKENS_PER_DOC: usize = 1_300;
        const SEED_BASE: u64 = 0x5EED_00F4_0000_0001;

        let mut any_exceeded_cap = false;
        for i in 0..DOC_COUNT {
            let seed = SEED_BASE.wrapping_add(i.wrapping_mul(0x9E37_79B9_7F4A_7C15));
            let html = generate_fuzz_doc(seed, TOKENS_PER_DOC);
            let real_depth = real_dom_max_depth(&html);
            if real_depth > super::MAX_SCAN_DEPTH {
                any_exceeded_cap = true;
            }
            assert_guard_never_under_trips(&format!("fuzz#{i}"), &html);
        }
        assert!(
            any_exceeded_cap,
            "generative corpus never exceeded MAX_SCAN_DEPTH -- the invariant \
             above passed vacuously; widen the token alphabet's open-tag bias"
        );
    }
}
