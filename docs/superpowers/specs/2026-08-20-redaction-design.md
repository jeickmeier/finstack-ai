# PII / Secret Redaction Middleware — Spec

Date: 2026-08-20. Sibling of the document-ingest middleware
(`docs/superpowers/specs/2026-08-19-document-ingestion-design.md`), reusing its
proven pattern: a `BeforeModel` middleware that rewrites only the model-visible
`ModelRequestDraft` via `StageOutcome::Replace`, leaves canonical journaled
`Message`s untouched, and fails soft.

## Goal

Keep PII and secrets out of provider-bound model requests. One new crate:

- **`extensions/middleware/finstack-ai-middleware-redaction`** — scans the
  model-visible draft for API keys, tokens, account numbers, and emails, and
  replaces each match with a stable redaction marker before the request leaves
  the process.

## Runtime facts this spec is built on (verified 2026-08-20)

- `StageOutcome::Replace` is legal at `PrepareContext` and `BeforeModel` only
  (`validate_stage_outcome`, `StageFold::accumulate`). At `AfterModel` the only
  usable outcomes for a Standard middleware are `Continue`, `Fail`, `Retry`,
  `Complete` (plus unlandable variants). **Output redaction by rewrite is
  impossible**; it is `Fail` or next-turn.
- `invoke_middleware_stage` hands **every** component in a stage's chain the
  **same** base `StageInput` (`input.clone()` per component), and
  `StageFold::accumulate` keeps only the **last** `Replace` in chain order.
  Two Replace-emitting `BeforeModel` middlewares therefore do not compose:
  the earlier one's rewrite is silently discarded.
- Tier order is `Standard < RequestShaping < ContextMutation <
  ContextCompaction < PostCompactionValidation` (derived `Ord`).
  Document-ingest sits at `ContextMutation`, so a `RequestShaping` redaction
  middleware runs **before** it and — when both emit `Replace` — loses.
- The Python binding auto-registers `DocumentIngestMiddleware` on every agent,
  so the conflict above is live in practice, not hypothetical.

## Decisions

1. **Create `extensions/middleware/finstack-ai-middleware-redaction`**
   following the document-ingest crate layout: `src/lib.rs` + `src/tests.rs`,
   `publish = false`, workspace lints, the same lint header as the verify
   middleware (`#![warn(missing_docs)]`, `#![forbid(unsafe_code)]`, deny
   unwrap/expect/panic outside tests).
2. **Component id `finstack.middleware.redaction`**, version `1.0.0`,
   `InvocationRecovery::RecomputeSafe` (pure function of the draft — no I/O,
   no clocks, no randomness).
3. **Standalone descriptor**: stage `BeforeModel` (plus `AfterModel` only when
   the output policy demands it, decision 12), `OrderTier::RequestShaping`,
   `priority 0`, `MiddlewareRole::Standard`. `configuration_digest` is the
   canonical-JSON digest of the effective `RedactionConfig` (and, in wrapping
   mode, the wrapped component's identity), so a config change changes the
   locked chain digest.
4. **Model-visible only, fail-soft** — the exact document-ingest contract:
   - Rewrite `before_model.request` messages; emit
     `StageOutcome::Replace(RawJson)` canonicalized with
     `serde_json_canonicalizer`; never touch `source_entries` or canonical
     history.
   - No change detected → `StageOutcome::Continue`.
   - Any internal failure (message rebuild rejected, canonicalization of a
     rewritten draft fails) degrades to passing the affected message (or the
     whole draft) through unmodified rather than aborting the run. A detection
     *miss* is by definition silent.
5. **Scan surface (v1)**: `ContentBlock::Text` blocks in messages of **every
   role**, including `Text` blocks nested one level inside
   `ContentBlock::ToolResult` (tool results cannot nest further tool blocks).
   Out of scope in v1, documented in the crate: `Json` blocks, `Opaque`
   blocks, `ToolCall` argument JSON (rewriting inside JSON string literals
   risks corrupting escapes), media blocks, and `ModelRequestDraft.settings`.
   Because `BeforeModel` re-runs every cycle over the whole draft, prior
   **assistant** turns are also scanned — this is the "next-turn" output
   redaction path.
6. **Detectors (v1)** — curated, deterministic, regex-anchored, each with a
   stable kebab-case kind used in the marker. No entropy heuristics in v1
   (false-positive control). All patterns ASCII.
   - `email` — `[A-Za-z0-9._%+-]+@[A-Za-z0-9.-]+\.[A-Za-z]{2,}`
   - `api-key` — known vendor prefixes, one alternation:
     `sk-[A-Za-z0-9_-]{16,}` (OpenAI / Anthropic `sk-ant-…`),
     `gh[pousr]_[A-Za-z0-9]{36,}`, `github_pat_[A-Za-z0-9_]{22,}`,
     `xox[abprs]-[A-Za-z0-9-]{10,}`,
     `(?:AKIA|ASIA|ABIA|ACCA)[A-Z0-9]{16}`,
     `AIza[A-Za-z0-9_-]{35}`,
     `(?:sk|pk|rk)_(?:live|test)_[A-Za-z0-9]{10,}`
   - `jwt` — `eyJ[A-Za-z0-9_-]{4,}\.eyJ[A-Za-z0-9_-]{4,}\.[A-Za-z0-9_-]{4,}`
   - `private-key` — a whole PEM block:
     `-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----` through the matching
     `-----END … PRIVATE KEY-----` (dot-all, non-greedy), or the header alone
     when no end marker is present.
   - `card` — candidate `[0-9](?:[0-9 -]{11,17})[0-9]` runs whose digit count
     is 13–19 **and** which pass Luhn; separators must be uniform (all spaces
     or all dashes or none). Non-Luhn candidates are left alone.
   - `iban` — `[A-Z]{2}[0-9]{2}[A-Z0-9]{11,30}` word-bounded, confirmed by
     the ISO 7064 mod-97 check. Non-validating candidates are left alone.
7. **Marker format**: each match is replaced by `[REDACTED:<kind>]`, e.g.
   `[REDACTED:email]`. Markers contain no digest, length, or fragment of the
   original — nothing recoverable. The marker alphabet cannot re-match any
   detector, so redaction is **idempotent** — required because the chain
   re-runs the rewrite on every cycle and after every crash-recovery replay.
8. **Overlaps** resolve deterministically: matches are collected across all
   enabled detectors, sorted by (start, longest-first), and applied
   left-to-right skipping any match overlapping an already-applied one.
9. **Config**:

   ```rust
   pub struct RedactionConfig {
       pub detect_emails: bool,        // default true
       pub detect_api_keys: bool,      // default true (covers api-key, jwt, private-key)
       pub detect_account_numbers: bool, // default true (covers card, iban)
       pub output_policy: OutputPolicy,  // default Off
   }
   pub enum OutputPolicy { Off, Fail }
   ```

   Constructors: `RedactionMiddleware::try_new()` (defaults) and
   `try_with_config(config)`. Construction fails only on an invalid checked-in
   identity or an all-disabled detector set (`REDACTION_CONFIGURATION_INVALID`
   -style error enum, mirroring `DocumentIngestError`). Regexes are compiled
   once at construction; compile failure is a construction error, never a
   run-time one.
10. **Composition with document-ingest — the wrapping constructor.** Because
    the chain gives every component the base draft and keeps only the last
    `Replace` (runtime facts above), a standalone redaction middleware must
    **not** be registered in the same chain as another Replace-emitting
    `BeforeModel` middleware. For that deployment the crate provides:

    ```rust
    RedactionMiddleware::try_wrapping(
        inner: Arc<dyn Middleware>,
        config: RedactionConfig,
    ) -> Result<RedactionMiddleware, RedactionError>
    ```

    - Construction validates the inner descriptor: `BeforeModel`-only stage
      mask and `MiddlewareRole::Standard`, else
      `RedactionError::Configuration`. *(Amended during implementation,
      2026-08-20: the wrapper's own stage mask still follows decision 12 —
      `OutputPolicy::Fail` adds `AfterModel`, which the wrapper handles
      itself; the inner middleware never sees it.)*
    - The wrapper's descriptor keeps the redaction component id/version but
      adopts the **inner's** `MiddlewareOrder` (tier/priority/constraints), so
      it sits exactly where the inner middleware sat; its
      `configuration_digest` covers both the redaction config and the inner's
      `ComponentInvocation`.
    - `invoke`: call inner first. `Replace(raw)` → parse back to
      `ModelRequestDraft`, redact it, re-canonicalize, emit `Replace`;
      unparsable inner payload → fail-soft, pass the inner `Replace` through
      unchanged. `Continue` → redact the base draft (standalone behavior).
      Every other outcome and any inner error passes through untouched.
    - Net effect with document-ingest wrapped: one `Replace` emitter in the
      chain, and Markdown extracted from attachments is redacted too.
11. **Deployment rule** (documented in both crates' README/lib docs):
    register standalone redaction at `RequestShaping` **or** wrap the
    chain's single Replace-emitting `BeforeModel` middleware — never both,
    never standalone alongside document-ingest.
12. **Output policy.** `AfterModel` cannot `Replace` (runtime facts above), so:
    - `OutputPolicy::Off` (default): the stage mask is `BeforeModel` only.
      Assistant output is still redacted **next turn** by the `BeforeModel`
      pass (decision 5).
    - `OutputPolicy::Fail`: the stage mask adds `AfterModel`. The middleware
      decodes the stage payload (the canonical JSON of the assistant message
      the model just produced), scans its `Text`/nested-`ToolResult` text, and
      on any match returns `StageOutcome::Fail` with a stable descriptor
      naming only the detector kinds and match count — **never** the matched
      text. An undecodable payload is fail-soft `Continue`.
13. **Dependencies**: `finstack-ai-kernel`, `finstack-ai-runtime`
    (`default-features = false`), `serde_json`, `serde_json_canonicalizer`,
    `thiserror`, plus **`regex`** added to `[workspace.dependencies]` as
    `regex = { version = "1.11", default-features = false, features = ["std", "perf"] }`
    (already in `Cargo.lock` transitively; pure Rust, wasm-clean; ASCII
    classes only so no `unicode-*` features).
14. **Testing** (in-crate `src/tests.rs`, document-ingest style): per-detector
    positive/negative (incl. Luhn-fail and mod-97-fail candidates left
    intact), idempotence, overlap resolution, all-roles draft rewrite,
    nested `ToolResult` text, no-match → `Continue`, changed → `Replace` whose
    draft differs only in the matched spans, message-identity preservation
    (id/role/metadata/provider ids), fail-soft rebuild fallback, wrapping-mode
    composition against a stub inner middleware (Replace-rewrite, Continue,
    passthrough outcomes, invalid inner descriptor rejected), and
    `OutputPolicy::Fail` at `AfterModel` with a descriptor that contains no
    matched fragment.
15. **Workspace registration**: root `Cargo.toml` `members` +
    `[workspace.dependencies]` entry
    (`finstack-ai-middleware-redaction = { path = …, version = "1.0.0" }`),
    matching every existing middleware crate. No `crates/finstack-ai` change,
    so no public-API baseline regeneration.

## Non-goals (v1)

- **Bindings parity** (Python/WASM exposure, e.g. wrapping the auto-registered
  document-ingest middleware behind an opt-in flag) — follow-up plan; this
  plan delivers the Rust crate and its composition story.
- **Runtime chain changes** — threading one component's `Replace` into the
  next component's input would fix multi-Replace composition generically, but
  it alters documented driver invariants (`middleware_driver/mod.rs` §5) and
  belongs to its own spec if ever wanted. The wrapping constructor covers the
  known conflict without touching the runtime.
- **Entropy/heuristic secret detection, custom user patterns, allowlists,
  reversible tokenization/vault mapping, structured-JSON redaction,
  provider-side redaction.**
- **SSN / phone-number / address detection** — jurisdiction-heavy, high
  false-positive classes; revisit with a configurable pattern pack.
