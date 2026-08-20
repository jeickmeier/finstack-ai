# finstack-ai-middleware-redaction

`BeforeModel` middleware that redacts PII and secrets from the model-visible
request draft before it leaves the process. The canonical, journaled
conversation is never touched: only the `ModelRequestDraft` is rewritten, via
`StageOutcome::Replace`, exactly like the sibling document-ingest middleware.
Every detected secret is replaced by a stable `[REDACTED:<kind>]` marker
carrying nothing recoverable. Redaction is deterministic, idempotent, and
fail-soft — an internal failure passes the affected content through
unmodified rather than aborting the run.

## Detectors

Curated, validated patterns only — no entropy heuristics (v1):

| Kind | Detects | Validation |
|---|---|---|
| `email` | email addresses | pattern |
| `api-key` | OpenAI/Anthropic `sk-…`, GitHub `ghp_…`/`github_pat_…`, Slack `xox…`, AWS `AKIA…`, Google `AIza…`, Stripe `sk_live_…` | pattern |
| `jwt` | three-segment JSON Web Tokens | pattern |
| `private-key` | PEM `PRIVATE KEY` blocks (whole block, or a bare header) | pattern |
| `card` | 13–19-digit payment-card numbers, uniform separators | Luhn + digit boundaries |
| `iban` | IBANs | ISO 7064 mod-97 + word boundaries |

Detector groups toggle via `RedactionConfig` (`detect_emails`,
`detect_api_keys`, `detect_account_numbers`); an all-disabled set is a
construction error.

Scan surface (v1): `Text` blocks in messages of every role, including `Text`
blocks nested inside `ToolResult` content. `Json`, `Opaque`, `ToolCall`
arguments, media blocks, and draft settings are not scanned.

## Deployment rule

The middleware chain hands **every** `BeforeModel` component the same base
draft and keeps only the **last** `Replace` in chain order, so two
Replace-emitting `BeforeModel` middlewares do not compose — the earlier one's
rewrite is silently discarded. Therefore:

- **Standalone** (`RedactionMiddleware::try_new()` /
  `try_with_config(...)`): registers on the `RequestShaping` tier. Use only
  when no other `BeforeModel` middleware emits `Replace`.
- **Wrapping** (`RedactionMiddleware::try_wrapping(inner, config)`): use when
  a Replace-emitting `BeforeModel` middleware such as document-ingest is
  registered. The wrapper takes the inner middleware's chain slot (its
  ordering), invokes it first, and redacts whatever draft it produces — so
  Markdown extracted from attached documents is redacted too. Register the
  wrapper *instead of* the inner middleware, never both.
- **Context compactors**: do not register this middleware together with a
  `ContextCompactor`-role middleware (e.g. finstack-ai-middleware-compaction).
  The settlement applier lands the compaction projection — validated against
  the *unredacted* base draft — on top of any `Replace` in the same fold, so
  the compactor silently discards this middleware's rewrite for every entry
  its projection covers.

## Output redaction

`AfterModel` middleware cannot `Replace`, so model output is never rewritten
in place:

- `OutputPolicy::Off` (default): assistant history is redacted on the next
  `BeforeModel` pass, before it is ever sent back to a provider.
- `OutputPolicy::Fail`: additionally declares the `AfterModel` stage and
  fails the run with a stable `redaction_output_detected` descriptor naming
  only detector kinds and match count — never the matched text.

## Non-goals (v1)

Entropy/heuristic detection, custom patterns, allowlists, reversible
tokenization, structured-JSON redaction, SSN/phone/address detection, and
bindings exposure (follow-up). See
`docs/superpowers/specs/2026-08-20-redaction-design.md`.
