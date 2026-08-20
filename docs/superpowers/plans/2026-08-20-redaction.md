# PII / Secret Redaction Middleware Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A `BeforeModel` middleware crate that redacts API keys, tokens, account numbers, and emails from the model-visible `ModelRequestDraft` via `StageOutcome::Replace`, fail-soft, leaving canonical messages untouched.

**Architecture:** New crate `extensions/middleware/finstack-ai-middleware-redaction`, structured exactly like `finstack-ai-middleware-document-ingest` (lib.rs + tests.rs). A pure regex-based detection engine feeds a draft rewriter; a wrapping constructor composes redaction around document-ingest because the middleware chain keeps only the last `BeforeModel` `Replace`. Optional `AfterModel` `Fail` output policy (AfterModel cannot Replace).

**Tech Stack:** Rust, `finstack-ai-runtime` middleware trait, `regex` (new workspace dep, `default-features = false, features = ["std", "perf"]`), `serde_json_canonicalizer`, `thiserror`.

**Spec:** `docs/superpowers/specs/2026-08-20-redaction-design.md`

## Global Constraints

- Component id `finstack.middleware.redaction`, version `1.0.0`, `InvocationRecovery::RecomputeSafe`.
- Standalone order: `OrderTier::RequestShaping`, priority 0, `MiddlewareRole::Standard`; stage mask `BeforeModel` (+ `AfterModel` only when `OutputPolicy::Fail`).
- Model-visible draft only; canonical `Message`s never touched; fail-soft everywhere (internal failure → pass-through, never abort).
- Markers are `[REDACTED:<kind>]`, kinds: `email`, `api-key`, `jwt`, `private-key`, `card`, `iban`; redaction must be idempotent.
- Crate lint header identical to the verify middleware; workspace lints; `publish = false`.
- Error descriptors and notes must never contain matched secret text.
- Run tests with `cargo nextest run -p finstack-ai-middleware-redaction`; lint with `cargo clippy -p finstack-ai-middleware-redaction --all-targets`.

---

### Task 1: Crate scaffold, workspace registration, descriptor

**Files:**
- Modify: `Cargo.toml` (root: `members` + `[workspace.dependencies]`: the crate itself and `regex`)
- Create: `extensions/middleware/finstack-ai-middleware-redaction/Cargo.toml`
- Create: `extensions/middleware/finstack-ai-middleware-redaction/src/lib.rs`
- Create: `extensions/middleware/finstack-ai-middleware-redaction/src/tests.rs`

**Interfaces:**
- Produces: `RedactionMiddleware::try_new() -> Result<Self, RedactionError>`, `try_with_config(RedactionConfig)`, `RedactionConfig { detect_emails, detect_api_keys, detect_account_numbers, output_policy }` (Default: all true, `OutputPolicy::Off`), `OutputPolicy { Off, Fail }`, `RedactionError::Configuration { reason: &'static str }`. `Middleware::descriptor()` returns the standalone descriptor; `invoke` returns `Continue` for now.

- [ ] **Step 1: Write failing descriptor tests** in `src/tests.rs`: `try_new` succeeds; descriptor has component id `finstack.middleware.redaction`, tier `RequestShaping`, priority 0, stage mask contains `BeforeModel` and not `AfterModel`; `OutputPolicy::Fail` config adds `AfterModel`; all-detectors-disabled config is a `Configuration` error; two configs produce distinct `configuration_digest`s.
- [ ] **Step 2: Run to verify failure** (crate doesn't compile / tests fail).
- [ ] **Step 3: Implement** Cargo.toml (deps: kernel, runtime `default-features = false`, regex, serde_json, serde_json_canonicalizer, thiserror; dev: none yet), lib.rs with config/error types, descriptor construction (configuration_digest = `Digest::raw_json` over a canonical JSON encoding of the config), stub `invoke` returning `Continue`, `mod tests` gate.
- [ ] **Step 4: Run tests to verify pass**; run clippy.
- [ ] **Step 5: Commit** `feat: scaffold redaction middleware crate with descriptor`

### Task 2: Detection engine

**Files:**
- Create: `extensions/middleware/finstack-ai-middleware-redaction/src/detect.rs`
- Test: same-crate `src/tests.rs`

**Interfaces:**
- Produces: `pub(crate) struct Detectors` (compiled once, held in `Arc` inside the middleware); `Detectors::try_new(&RedactionConfig) -> Result<Self, RedactionError>`; `Detectors::redact(&self, text: &str) -> Option<String>` returning `None` when nothing matched; `pub(crate) fn kinds_in(&self, text: &str) -> BTreeSet<&'static str>` for the AfterModel Fail descriptor.

- [ ] **Step 1: Write failing tests**, one per behavior:
  - email hit → `[REDACTED:email]`; bare `user at host` untouched.
  - each api-key family hit (`sk-…`, `ghp_…`, `github_pat_…`, `xoxb-…`, `AKIA…`, `AIza…`, `sk_live_…`); short `sk-abc` untouched.
  - JWT hit; two-segment lookalike untouched.
  - PEM block (BEGIN→END inclusive) collapses to one `[REDACTED:private-key]`; header-only input also redacted.
  - 16-digit Luhn-valid card (spaced, dashed, bare) hit; Luhn-invalid 16 digits untouched; 12-digit run untouched.
  - valid IBAN (`GB82WEST12345698765432`) hit; mod-97-invalid candidate untouched.
  - overlap: JWT containing an email-like substring redacts once as `jwt`.
  - idempotence: `redact(redact(x)) == None`-change (second pass returns `None`).
  - disabled detector groups don't fire.
- [ ] **Step 2: Run to verify failures.**
- [ ] **Step 3: Implement** `detect.rs`: per-group compiled `Regex` sets built from the spec's patterns; candidate post-validation (`luhn`, `iban_mod97`, uniform card separators); match collection sorted by (start, longest-first), overlap-skipping single-pass rebuild.
- [ ] **Step 4: Run tests to verify pass**; clippy.
- [ ] **Step 5: Commit** `feat: add redaction detection engine (emails, keys, jwt, pem, card, iban)`

### Task 3: BeforeModel draft rewrite

**Files:**
- Modify: `extensions/middleware/finstack-ai-middleware-redaction/src/lib.rs`
- Test: `src/tests.rs`

**Interfaces:**
- Consumes: `Detectors::redact`.
- Produces: real `invoke` for `StageInput::BeforeModel`: rewrite every message's `Text` blocks and `Text` blocks nested in `ToolResult` content across all roles; unchanged → `Continue`; changed → `Replace(RawJson)` canonicalized from the rewritten `ModelRequestDraft` (mirror document-ingest's `serde_json_canonicalizer_bytes` + `stable_error`). Rebuild failures fall back to the original message (fail-soft; unlike document-ingest there is no must-strip invariant).

- [ ] **Step 1: Write failing tests** using a hand-built `BeforeModelInput` (copy the document-ingest test fixture approach): secret in user text → `Replace` draft has marker and identical ids/roles/metadata; secret in assistant history text → redacted (next-turn path); secret inside `ToolResult` nested text → redacted; clean draft → `Continue`; non-BeforeModel input → `Continue`; oversized rewrite falling out of `TextBlock` bounds → original text preserved (fail-soft).
- [ ] **Step 2: Run to verify failures.**
- [ ] **Step 3: Implement** `rewrite_message`/`rewrite_block` + `rebuild_message` (same identity-preserving constructor as document-ingest, but fallback = `message.clone()`).
- [ ] **Step 4: Run tests to verify pass**; clippy.
- [ ] **Step 5: Commit** `feat: redact model-visible draft at before_model`

### Task 4: AfterModel output policy (Fail)

**Files:**
- Modify: `extensions/middleware/finstack-ai-middleware-redaction/src/lib.rs`
- Test: `src/tests.rs`

**Interfaces:**
- Consumes: `Detectors::kinds_in`.
- Produces: with `OutputPolicy::Fail`, `invoke` on `StageInput::AfterModel { value }` decodes the assistant `Message` JSON; any detector hit → `StageOutcome::Fail(ErrorDescriptor)` with stable code `redaction_output_detected`, message naming kinds + count only; clean or undecodable → `Continue`. With `Off`, AfterModel input returns `Continue` (and the stage isn't declared anyway).

- [ ] **Step 1: Write failing tests**: Fail policy + secret in assistant text → `Fail`, descriptor text contains `api-key` but not the key material; clean message → `Continue`; garbage payload → `Continue`; Off policy → `Continue`.
- [ ] **Step 2: Run to verify failures.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run tests to verify pass**; clippy.
- [ ] **Step 5: Commit** `feat: optional after_model fail policy for output secrets`

### Task 5: Wrapping composition with document-ingest

**Files:**
- Modify: `extensions/middleware/finstack-ai-middleware-redaction/src/lib.rs`
- Modify: `extensions/middleware/finstack-ai-middleware-redaction/Cargo.toml` (dev-dependency on `finstack-ai-middleware-document-ingest` only if the integration test uses the real crate; a stub inner middleware suffices otherwise)
- Test: `src/tests.rs`

**Interfaces:**
- Produces: `RedactionMiddleware::try_wrapping(inner: Arc<dyn Middleware>, config) -> Result<Self, RedactionError>`; wrapper descriptor = redaction id/version, inner's `MiddlewareOrder`, `BeforeModel`-only mask, configuration digest covering config + inner `ComponentInvocation`; `invoke` = inner first, redact its `Replace` payload (unparsable → pass through unchanged), redact base draft on `Continue`, pass through other outcomes/errors.

- [ ] **Step 1: Write failing tests** with a stub inner middleware: inner `Replace` containing a secret → wrapper `Replace` redacted; inner `Continue` + secret in base draft → wrapper `Replace`; inner `Fail` passes through; inner with `AfterModel` in its mask or non-Standard role → `Configuration` error; wrapper descriptor adopts inner tier (`ContextMutation`).
- [ ] **Step 2: Run to verify failures.**
- [ ] **Step 3: Implement.**
- [ ] **Step 4: Run tests to verify pass**; clippy.
- [ ] **Step 5: Commit** `feat: wrapping constructor composing redaction around Replace-emitting middleware`

### Task 6: Docs and final verification

**Files:**
- Create: `extensions/middleware/finstack-ai-middleware-redaction/README.md`
- Modify: `extensions/middleware/finstack-ai-middleware-redaction/src/lib.rs` (crate docs: deployment rule, scan-surface limits, marker format)

- [ ] **Step 1: Write README** (pattern: document-ingest README): what it does, deployment rule (standalone at RequestShaping **or** wrapped — never standalone next to document-ingest, with the last-Replace-wins explanation), detector table, output-policy note, v1 non-goals.
- [ ] **Step 2: Full verification**: `cargo fmt --check`, `cargo clippy -p finstack-ai-middleware-redaction --all-targets`, `cargo nextest run -p finstack-ai-middleware-redaction`, then workspace sanity `cargo check --workspace --locked`.
- [ ] **Step 3: Commit** `docs: redaction middleware README and crate docs`
