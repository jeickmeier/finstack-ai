# finstack-ai-tools-fetch

Bounded, allowlisted HTTP `GET` fetch toolset for finstack-ai. Deny-by-default:
`HttpFetchToolset::try_new` refuses to construct with an empty host allowlist,
and every numeric limit is a construction-time error rather than a silent
clamp when it exceeds the crate's hard ceiling.

This crate implements the full bounded-fetch pipeline: `HostPattern`
parsing/matching and `HttpFetchConfig` validation; resolve-and-pin address
vetting and redirect re-vetting on every hop (via `finstack-ai-net-guard`);
bounded response reads that error rather than silently truncate; HTML →
Markdown conversion for inline delivery; and artifact staging for
responses that exceed the inline budget.

**`allow_loopback_http` is for test fixtures only — never enable it in
production.** When set, a vetted URL that resolves to a loopback host
skips the allowlist check entirely, so a model-supplied URL could reach
any loopback port unchecked. It exists solely to let fixture servers on
`127.0.0.1`/`localhost` run in tests without enumerating every ephemeral
port in the allowlist.

```rust
use finstack_ai_tools_fetch::{HttpFetchConfig, HttpFetchToolset};

let config = HttpFetchConfig {
    allowlist: vec!["docs.rs".to_owned()],
    ..HttpFetchConfig::default()
};
let toolset = HttpFetchToolset::try_new(config).expect("fetch config");
```

## HTML → Markdown (Task 10, spec §4.5)

Under `mode: "auto"` and `mode: "markdown"`, HTML bodies (`text/html`,
`application/xhtml+xml`) are converted to Markdown before inlining;
`mode: "text"` always returns the raw body untouched.

License gate outcome: `htmd = "0.5"` (html5ever-based, MIT licensed) was
added to `[workspace.dependencies]` and this crate's `[dependencies]`, then
verified with `cargo deny check licenses` — the check passed cleanly (only
an unrelated pre-existing `license-not-encountered` warning for an unused
`NCSA` allowance in `deny.toml`, not related to this change). `htmd` was
kept; the `scraper`-based hand-rolled fallback described in the spec was not
needed.

Conversion (`src/markdown.rs::html_to_markdown`) drops `<script>`/`<style>`
content (explicitly configured via `HtmlToMarkdown::builder().skip_tags`,
matching `htmd`'s own default) and HTML comments (dropped by the underlying
`html5ever` parser). It never errors: a conversion failure, or Markdown
output exceeding the caller's byte budget, returns `None`, and the delivery
layer falls back to inlining the original text (still budget-checked).

`html5ever` is a browser-grade, error-tolerant parser, so no fixture
markup reliably makes `htmd::convert` fail — `fixtures/pathological.html`
(deeply malformed/unclosed tags) converts successfully rather than
exercising the `None` path. The `None` branch is instead covered directly
by a unit test that forces it via a tiny `output_cap`.
