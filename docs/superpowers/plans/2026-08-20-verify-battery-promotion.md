# Evidence Verifier Battery Promotion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Promote `finstack-ai-middleware-verify` from a `before_finalize` fixture into a real evidence-verifier battery that can bounce a candidate back to the model with citation/test/artifact feedback via `StageOutcome::Retry`, with `RequestInteraction` removed from its API.

**Architecture:** Four layers change, bottom-up. (1) The kernel gains a `RetryClassification::Verification` family and admits a `before_finalize` `Retry` over a **Completed** terminal candidate for that family (today `retry_bodies` requires `TerminalCandidate::Failed`, so a verifier can only re-shape an already-failed validation retry — it cannot bounce a good-schema candidate). (2) The runtime enriches `StageInput::BeforeFinalize` with the candidate's result-message content (today the input is just the serialized `TerminalCandidate` — ids and a digest — which no content-level verifier can check). (3) The facade drive loop learns that its `FinalizeAccepted` submission can be superseded by a middleware `Retry` (today it errors with "terminal state is missing"). (4) The crate replaces the fixture's construction-time `VerifyDecision` with a pure `EvidenceVerifier` trait mapped to `Continue`/`Retry`/`Fail`, re-runs the same verifier at `before_model` to inject deterministic feedback context on the bounce cycle, and drops `RequestInteraction` entirely. Packaging then flips fixture → battery (publishable, README, `docs/site/middleware.md`, CHANGELOG, public-api baselines).

**Tech Stack:** Rust workspace (`crates/finstack-ai-kernel`, `crates/finstack-ai-runtime`, `crates/finstack-ai`, `extensions/middleware/finstack-ai-middleware-verify`), `finstack-ai-test` conformance helpers, `mise run check-public-api` baselines.

**Spec:** This plan is self-contained; the load-bearing current-state facts are cited inline as `file:line` (verified 2026-08-20 on `main` @ 306b776). Re-verify each cited site before editing — line numbers drift.

## Global Constraints

- Workspace lints: `#![forbid(unsafe_code)]`, `deny(clippy::unwrap_used)`, `deny(clippy::expect_used)`, `deny(clippy::panic)` in non-test code (see the header of `extensions/middleware/finstack-ai-middleware-verify/src/lib.rs`). Fallible constructors, never panics.
- Error codes are stable strings matched by code, not message (`docs/site/middleware.md` "Stable codes"). New codes introduced here: `verify_rejected` (already used by the fixture) stays; no new middleware codes are needed.
- Middleware `invoke` must be pure with respect to external state — invocations are never journaled and re-run wholesale on crash recovery (`docs/site/middleware.md` "What an author must know"). The feedback mechanism in Task 5 is deliberately stateless (re-derivation, not caching).
- `RequestInteraction` stays out. It is unlandable by design (`crates/finstack-ai-runtime/src/exec/middleware_driver/fold.rs:141-145`); this plan removes it from the crate's API instead of trying to land it.
- Verify tests use workspace binaries with explicit exit-code checks (`cargo test -p <crate>`), per RTK guidance — never a summarizing wrapper for gating commands.
- Public API baselines live in `fixtures/compatibility/public-rust-api/cargo-public-api/` (one per crate, including `finstack-ai-middleware-verify.txt`); regenerate via `scripts/compat/public_items.py` (no `--check` writes, `--check` compares) after any public-surface change, then gate with `mise run check-public-api`.
- Commit after every green task; message prefix `feat(verify):` / `feat(kernel):` / `feat(runtime):` as appropriate.

## Current-state map (read before starting)

| Fact | Where |
| --- | --- |
| Fixture API: `VerifyDecision::{Accept, Fail, RequestInteraction}` fixed at construction; never inspects the candidate | `extensions/middleware/finstack-ai-middleware-verify/src/lib.rs:44-52, 140-184` |
| Middleware `Retry` folds only at `BeforeFinalize`; a fold terminal supersedes the facade base outcome | `fold.rs:156-165`, `stage_settlement/apply.rs:41-48` |
| Kernel schedules the retry **only** for `TerminalCandidate::Failed` + `error.retryable`; `Validation` classification is coupled to `state.validation_failure` | `crates/finstack-ai-kernel/src/reducer/decide/stage.rs:285-341` (`retry_bodies`) |
| Retry landing allocates `(3,1,1)` ids, commits `RetryScheduled` + timer `EffectRequested`, post-commit executes the timer | `decide/stage.rs:343-400`, `settlement/stage.rs:529` |
| `StageInput::BeforeFinalize { candidate }` is the canonical `TerminalCandidate` (ids + `result_digest`), not message content | `stage_settlement/input.rs:65-67`, `codec.rs:43-49` |
| Facade drive loop: validation-failure → its own `Retry(Validation)`; otherwise submits `FinalizeAccepted` then **requires** a terminal state | `crates/finstack-ai/src/agent/drive.rs:268-330` |
| `ContinueModel` (facade-only, not producible by the fold) is the other re-cycle shape; carries only an optional reason string | `kernel/src/reducer/input.rs:220-224`, `decide/stage.rs:151-163` |
| Battery vs fixture packaging: compaction has no `publish = false`; verify and document-ingest do | `extensions/middleware/*/Cargo.toml` |
| Battery status table ("Shipping leaves") | `docs/site/middleware.md:75-82` |

---

### Task 1: Kernel — `RetryClassification::Verification` admits a Completed candidate

**Files:**
- Modify: `crates/finstack-ai-kernel/src/records/lifecycle.rs:113-122` (`RetryClassification`)
- Modify: `crates/finstack-ai-kernel/src/reducer/decide/stage.rs:285-341` (`retry_bodies`)
- Test: kernel reducer tests (co-located; find the existing `retry` decide tests with `grep -rn "retry_bodies\|RetryScheduled" crates/finstack-ai-kernel/src --include="*.rs" | grep -i test`)

**Interfaces:**
- Consumes: existing `RetryDirective`, `TerminalCandidate`, `RetryScheduled`.
- Produces: `RetryClassification::Verification` (new enum variant) and the admission rule later tasks rely on: a `ReducerStageOutcome::Retry(directive)` at `BeforeFinalize` with `directive.classification == Verification` lands over `TerminalCandidate::Completed`, committing a `RetryScheduled` whose `prior_error` is the synthesized descriptor below. All other classifications keep today's exact rules.

Synthesized descriptor (kernel-owned, middleware-agnostic wording):

```rust
// in retry_bodies, for the Completed-candidate arm:
let error = crate::ErrorDescriptor::new(
    "candidate_rejected",
    "a before-finalize verifier rejected the completed candidate",
    crate::ErrorCategory::Validation,
    true, // retryable
)
.map_err(|_| KernelError::InvariantViolation)?;
```

- [ ] **Step 1: Write the failing decide test** — restore a `KernelState` at `RunPhase::BeforeFinalize` with a `TerminalCandidate::Completed` (copy the state fixture from `crates/finstack-ai-runtime/src/exec/stage_settlement/tests/terminal_fold.rs:173-192`, which builds exactly this state, but in the kernel crate's own test module with `max_retries: Some(3)` so the limit does not intercept). Submit `KernelInput::StageSettled` with `ReducerStageOutcome::Retry(RetryDirective::try_new(RetryClassification::Verification, Duration::from_millis(250), "verify-policy-v1")?)` and a `(3,1,1)` id bag. Assert: `decide` returns `Ok`, the committed bodies contain a `RecordBody::RetryScheduled` with `classification == Verification`, `attempt == 1`, and `prior_error.code == "candidate_rejected"`, plus a timer `EffectRequested`.
- [ ] **Step 2: Add a second failing test for the guard matrix** — same state, `RetryClassification::Framework` over the Completed candidate must still be `Err(InvalidPhaseInput)`; and `Verification` over a `TerminalCandidate::Failed { error.retryable: true }` candidate must also land (reusing the candidate's own error as `prior_error`, same as today's Failed arm).
- [ ] **Step 3: Run tests, confirm both fail** — `cargo test -p finstack-ai-kernel <new_test_names>`; the first fails because `Verification` doesn't exist (compile error), which counts.
- [ ] **Step 4: Add the variant** — in `lifecycle.rs`, append `/// Before-finalize verification retry.\n Verification,` to `RetryClassification`. Check serialization: the enum derives serde on the record path — run `grep -rn "RetryClassification" crates/finstack-ai-kernel/src/state/hash_projection schemas/ tools/ bindings/` and mirror the new variant anywhere the classification names are enumerated (hash projection, JSON schema files, Python/JS binding name lists). If a schema file enumerates the variants, add `"verification"` there.
- [ ] **Step 5: Rework `retry_bodies`** — replace the hard `TerminalCandidate::Failed` destructure with a match:

```rust
let prior_error = match state.terminal_candidate.as_ref() {
    Some(TerminalCandidate::Failed { error, .. }) => {
        if !error.retryable {
            return Err(KernelError::InvalidPhaseInput { phase: state.phase, input: "stage_settled" });
        }
        error.clone()
    }
    Some(TerminalCandidate::Completed { .. })
        if directive.classification == RetryClassification::Verification =>
    {
        crate::ErrorDescriptor::new(
            "candidate_rejected",
            "a before-finalize verifier rejected the completed candidate",
            crate::ErrorCategory::Validation,
            true,
        )
        .map_err(|_| KernelError::InvariantViolation)?
    }
    _ => return Err(KernelError::InvalidPhaseInput { phase: state.phase, input: "stage_settled" }),
};
```

Keep the two `validation_failure` coupling guards exactly as they are (`Validation` requires `validation_failure`; `validation_failure` requires `Validation`) — `Verification` must be rejected whenever `validation_failure` is set, which the existing second guard already does. Thread `prior_error` into `RetryScheduled::try_new` in place of `error.clone()`.
- [ ] **Step 6: Run kernel tests to green** — `cargo test -p finstack-ai-kernel`; also `cargo test -p finstack-ai-runtime settlement` because `settlement/stage.rs:529` and the allocation-table test enumerate this arm.
- [ ] **Step 7: Commit** — `git commit -m "feat(kernel): admit Verification retries over a completed candidate at before_finalize"`.

---

### Task 2: Runtime — `BeforeFinalize` stage input carries the result message

**Files:**
- Modify: `crates/finstack-ai-runtime/src/ports/middleware/types.rs:264-268` (`StageInput::BeforeFinalize`)
- Modify: `crates/finstack-ai-runtime/src/exec/stage_settlement/input.rs:65-67`
- Test: `crates/finstack-ai-runtime/src/exec/stage_settlement/tests/stage_input.rs`
- Modify (mechanical): every `StageInput::BeforeFinalize {` construction/match site — find with `grep -rn "BeforeFinalize {" crates extensions --include="*.rs"` (includes the verify crate's `tests.rs`, fixed properly in Task 5)

**Interfaces:**
- Consumes: `TerminalCandidate::Completed { message_id, .. }` and `state.messages`.
- Produces: `StageInput::BeforeFinalize { candidate: RawJson, result_message: Option<RawJson> }` where `result_message` is the JCS-canonical `Message` (`codec.rs::canonical_message`) whose id equals the Completed candidate's `message_id`, and `None` for a Failed candidate. Task 5's verifier consumes `result_message`.

- [ ] **Step 1: Write the failing test** in `tests/stage_input.rs`: build a state with one assistant `Message` and a Completed candidate pointing at it (mirror the existing BeforeFinalize input test in that file); assert `stage_input(...)` returns `result_message == Some(canonical_message(&message)?)`. Second case: Failed candidate → `result_message == None`.
- [ ] **Step 2: Run to confirm failure** — `cargo test -p finstack-ai-runtime stage_input` (compile error on the new field is the expected failure).
- [ ] **Step 3: Implement** — add the field with `#[serde(default, skip_serializing_if = "Option::is_none")]` so existing serialized inputs stay parseable; in `input.rs`:

```rust
Stage::BeforeFinalize => {
    let result_message = match state.terminal_candidate.as_ref() {
        Some(finstack_ai_kernel::TerminalCandidate::Completed { message_id, .. }) => state
            .messages
            .iter()
            .find(|message| message.id() == message_id)
            .map(canonical_message)
            .transpose()?,
        _ => None,
    };
    Ok(StageInput::BeforeFinalize {
        candidate: canonical_terminal_candidate(state)?,
        result_message,
    })
}
```

Update every other construction site with `result_message: None` and every match with `..` where the field is unused.
- [ ] **Step 4: Run to green** — `cargo test -p finstack-ai-runtime`.
- [ ] **Step 5: Regenerate runtime public-api baseline** — `uv run --no-project python scripts/compat/public_items.py` then `mise run check-public-api` (exit 0 required).
- [ ] **Step 6: Commit** — `git commit -m "feat(runtime): expose the candidate result message to before_finalize middleware"`.

---

### Task 3: Runtime — end-to-end proof that a Verification bounce lands

**Files:**
- Test: `crates/finstack-ai-runtime/src/exec/stage_settlement/tests/terminal_fold.rs` (new test alongside `the_stage_table_and_the_limit_requirement_are_mutually_exclusive`, which stops at allocation)

**Interfaces:**
- Consumes: Task 1's kernel admission; the existing choke point `settle_facade_stage` and test fixtures (`accepted_coordinator`, `driver_for`, `env`).
- Produces: confidence only — no new API. This is the test the promotion claim hangs on.

- [ ] **Step 1: Write the test** — coordinator driven to `BeforeFinalize` with a Completed candidate and `max_retries: Some(3)`; a `driver_for("finstack.middleware.verify", Stage::BeforeFinalize, StageOutcome::Retry(RetryDirective::try_new(RetryClassification::Verification, Duration::from_millis(1), "verify-policy-v1")?))`; call `settle_facade_stage` with base `ReducerStageOutcome::FinalizeAccepted` and the finalize id bag. Assert: commit succeeds, `coordinator.state().terminal` is `None`, `state.retry.pending` is `Some`, and the journal contains `RecordBody::RetryScheduled { classification: Verification, prior_error.code: "candidate_rejected", .. }`.
- [ ] **Step 2: Extend the test through the timer** — settle the timer effect the way the existing retry/sleep tests do (find with `grep -rn "TimerFiring\|Sleeping" crates/finstack-ai-runtime/src/exec --include="*.rs" | grep -i test`), then assert the run re-enters `RunPhase::PreparingContext` on cycle `n+1` **with `state.messages` still containing the bounced assistant message** — Task 5's feedback design depends on that persistence, so pin it here.
- [ ] **Step 3: Run to green** — `cargo test -p finstack-ai-runtime terminal_fold`.
- [ ] **Step 4: Commit** — `git commit -m "test(runtime): verification bounce lands end-to-end at before_finalize"`.

---

### Task 4: Facade — drive loop survives a superseded `FinalizeAccepted`

**Files:**
- Modify: `crates/finstack-ai/src/agent/drive.rs:303-330` (the block that submits `FinalizeAccepted` and then demands a terminal state)
- Test: `crates/finstack-ai/src/agent/tests.rs` (agent-level test with a registered verify-style middleware)

**Interfaces:**
- Consumes: the runtime choke point folding the facade's `FinalizeAccepted` into a middleware `Retry` (Tasks 1–3).
- Produces: `Agent::run` loops back to the next cycle when finalize was superseded, instead of failing with "terminal state is missing".

- [ ] **Step 1: Write the failing agent test** — an agent whose middleware plan registers a `before_finalize` component returning `Retry(Verification)` on cycle 0 and `Continue` from cycle 1 on (a tiny local test middleware in `tests.rs` keyed on `ctx.run` cycle is fine — determinism only matters within one invocation), with a scripted model that answers twice. Assert the run completes on the second cycle and the journal shows one `RetryScheduled { classification: Verification }`.
- [ ] **Step 2: Run to confirm the current failure mode** — expect `AgentRunError` "terminal state is missing" (or the kernel rejection surfacing, if the middleware plan path predates Tasks 1–3 in your branch order — either way, red).
- [ ] **Step 3: Implement** — after the `FinalizeAccepted` submission, replace the hard terminal expectation: `recover_state`; if `terminal` is `Some`, proceed exactly as today; if `terminal` is `None` and the phase is one of `Sleeping` / `PreparingContext` (retry landed; the timer post-commit action fires it), `wait_for_phase(&[RunPhase::PreparingContext, RunPhase::Failed, RunPhase::Cancelled])`, `ensure_nonterminal_failure`, and `continue` the drive loop. Any other phase stays the existing error.
- [ ] **Step 4: Run to green** — `cargo test -p finstack-ai agent`.
- [ ] **Step 5: Commit** — `git commit -m "feat(agent): continue the drive loop when finalize is superseded by a middleware retry"`.

---

### Task 5: Verify crate — `EvidenceVerifier` API, bounce mapping, feedback at `before_model`, `RequestInteraction` removed

**Files:**
- Rewrite: `extensions/middleware/finstack-ai-middleware-verify/src/lib.rs`
- Rewrite: `extensions/middleware/finstack-ai-middleware-verify/src/tests.rs`

**Interfaces:**
- Consumes: `StageInput::BeforeFinalize { candidate, result_message }` (Task 2), `RetryClassification::Verification` (Task 1), `ContextItem::try_new` (see `crates/finstack-ai-runtime/src/exec/stage_settlement/tests/fixtures.rs:115-127` for the exact constructor shape).
- Produces the crate's new public surface:

```rust
/// Evidence category a finding refers to.
pub enum EvidenceKind { Citation, Test, Artifact }

/// One bounded, non-secret finding. `note` is capped (reuse the kernel text bound).
pub struct EvidenceFinding { pub kind: EvidenceKind, pub note: Arc<str> }
impl EvidenceFinding { pub fn try_new(kind: EvidenceKind, note: &str) -> Result<Self, VerifyError>; }

/// Verifier decision for one candidate message.
pub enum Verdict {
    /// Land the candidate.
    Accept,
    /// Bounce it back to the model with feedback (maps to Retry at before_finalize,
    /// AddContext at before_model).
    Bounce(Vec<EvidenceFinding>),
    /// Fail the run (maps to Fail with code `verify_rejected`, non-retryable).
    Reject(Vec<EvidenceFinding>),
}

/// Pure, deterministic content check. Same input must give the same verdict:
/// invocations are re-run wholesale on recovery and are never journaled.
pub trait EvidenceVerifier: Send + Sync + std::fmt::Debug {
    /// Stable identity folded into the middleware configuration digest.
    fn verifier_id(&self) -> &str;
    /// Judge one canonical assistant `Message` JSON.
    fn verify(&self, message: &RawJson) -> Verdict;
}

/// Bounce policy: backoff + policy version label for the RetryDirective.
pub struct VerifyPolicy { /* backoff: Duration, policy_version: Arc<str> */ }
impl VerifyPolicy { pub fn try_new(backoff: Duration, policy_version: &str) -> Result<Self, VerifyError>; }

pub struct VerifyMiddleware { /* verifier: Arc<dyn EvidenceVerifier>, policy, descriptor */ }
impl VerifyMiddleware {
    pub fn try_new(verifier: Arc<dyn EvidenceVerifier>, policy: VerifyPolicy) -> Result<Self, VerifyError>;
}
```

`VerifyDecision`, `try_accept`, and every `RequestInteraction` code path are **deleted**. `interaction_error` goes with them.

Behavior matrix (the whole crate):

| Stage | Input | Verdict | Outcome |
| --- | --- | --- | --- |
| `BeforeFinalize` | `result_message: Some(m)` | `Accept` | `Continue` |
| `BeforeFinalize` | `result_message: Some(m)` | `Bounce(_)` | `Retry(RetryDirective { Verification, policy.backoff, policy.policy_version })` |
| `BeforeFinalize` | `result_message: Some(m)` | `Reject(f)` | `Fail(ErrorDescriptor::new("verify_rejected", <joined notes, bounded>, Validation, false))` |
| `BeforeFinalize` | `result_message: None` (candidate already failed) | — verifier not called | `Continue` (pass the failure through untouched) |
| `BeforeModel` | trailing draft message is assistant `m` and `verify(m)` is `Bounce(f)`/`Reject(f)` | | `AddContext([feedback_item(f)])` |
| `BeforeModel` | otherwise (no assistant yet, or `Accept`) | | `Continue` |
| any other stage | | | `MiddlewareError` (`MIDDLEWARE_OUTCOME_NOT_ALLOWED`), as the fixture does today |

The `BeforeModel` row is the feedback channel and it is deliberately **stateless**: on the bounce cycle the rejected assistant message is still in the draft (pinned by Task 3 Step 2), so re-running the same pure verifier re-derives the same findings and renders them as one user-role context item — no cached state to lose on crash recovery. Descriptor changes: `stages: StageMask::from_stages([Stage::BeforeModel, Stage::BeforeFinalize])`; `configuration_digest: Digest::raw_json` over a canonical JSON of `{verifier_id, policy_version, backoff_ms}`.

`feedback_item(findings)` builds `ContextItem::try_new(ContextItemKind::Instruction, vec![ContentBlock::Text(TextBlock::try_new(text)?)], ContextProvenance { source_id: Arc::from("finstack.middleware.verify"), source_ref: None, external: false }, ContextAuthority::TrustedApplication, 0, estimated_tokens)` where `text` is `"Evidence verification rejected the previous answer:\n- [citation] …\n- [test] …"` truncated to the kernel text bound.

- [ ] **Step 1: Write the failing tests** (rewrite `tests.rs`, keeping the `ctx()` fixture and the `finstack_ai_test::check_middleware_conformance` case): a scripted `EvidenceVerifier` (returns a verdict keyed on the message text) driven through every row of the matrix above. Include: `Bounce` produces `Retry` with `classification == Verification` and the policy's backoff/version; `Reject` produces `Fail` with code `verify_rejected` and `retryable == false`; `result_message: None` short-circuits to `Continue` without calling the verifier (assert via a call-counting verifier); `BeforeModel` feedback item text contains each finding's kind tag and note; oversized notes are truncated, not errors; wrong stage still errors.
- [ ] **Step 2: Run to confirm failure** — `cargo test -p finstack-ai-middleware-verify` (compile errors from the removed API are the expected first red).
- [ ] **Step 3: Implement `lib.rs`** per the interface block and matrix. Keep `#![warn(missing_docs)]` satisfied — every new public item gets a doc comment with the same register as the current file. Keep "never writes a store" true (no store dependency exists; don't add one).
- [ ] **Step 4: Run to green** — `cargo test -p finstack-ai-middleware-verify` and `cargo clippy -p finstack-ai-middleware-verify -- -D warnings`.
- [ ] **Step 5: Fix in-tree consumers** — `grep -rln "finstack_ai_middleware_verify\|VerifyDecision" crates extensions` (known: `crates/finstack-ai-runtime/src/exec/middleware_driver/mod.rs` references the crate; agent tests from Task 4). Update them to the new constructor with a trivial always-`Accept` verifier where they only need a passthrough component.
- [ ] **Step 6: Run the workspace test suite** — `cargo test --workspace` (or the repo's `mise` test task if one exists — check `mise.toml [tasks]`).
- [ ] **Step 7: Commit** — `git commit -m "feat(verify): evidence-verifier battery API with retry bounce and before_model feedback"`.

---

### Task 6: Packaging — fixture → battery

**Files:**
- Modify: `extensions/middleware/finstack-ai-middleware-verify/Cargo.toml` (drop `publish = false`; description → `"Before-finalize evidence verification middleware battery for finstack-ai"`)
- Rewrite: `extensions/middleware/finstack-ai-middleware-verify/README.md`
- Modify: `docs/site/middleware.md` (lines 19 table note if needed, and the Shipping-leaves row at :79)
- Modify: `CHANGELOG.md`
- Regenerate: `fixtures/compatibility/public-rust-api/cargo-public-api/finstack-ai-middleware-verify.txt` (and runtime/kernel baselines if not already done in Tasks 1–2)

**Interfaces:**
- Consumes: everything above, finished and green.
- Produces: the crate presented and released as a battery.

- [ ] **Step 1: Cargo.toml** — remove `publish = false`, update `description`; confirm `cargo publish --dry-run -p finstack-ai-middleware-verify` passes (licenses/readme fields are already workspace-inherited).
- [ ] **Step 2: README rewrite** — new sections: what the battery verifies (pluggable `EvidenceVerifier`, citation/test/artifact findings), the bounce loop (Bounce → `Retry` at `before_finalize` → timer → new cycle → same verifier injects feedback at `before_model`), the determinism obligation on implementors, the explicit note that `RequestInteraction` was removed because it is unlandable by design (link the fold doc), and a compile-checked usage snippet:

```rust
use std::sync::Arc;
use finstack_ai_middleware_verify::{EvidenceVerifier, Verdict, VerifyMiddleware, VerifyPolicy};

#[derive(Debug)]
struct CitationsPresent;
impl EvidenceVerifier for CitationsPresent {
    fn verifier_id(&self) -> &str { "citations-present-v1" }
    fn verify(&self, message: &finstack_ai_kernel::RawJson) -> Verdict {
        // pure content check over the canonical assistant message …
        Verdict::Accept
    }
}

let middleware = VerifyMiddleware::try_new(
    Arc::new(CitationsPresent),
    VerifyPolicy::try_new(finstack_ai_kernel::Duration::from_millis(250), "verify-policy-v1")?,
)?;
```

- [ ] **Step 3: docs/site/middleware.md** — Shipping-leaves row becomes: battery; `Accept`/`Bounce`(retry)/`Reject` land; `RequestInteraction` removed from the API. Update the stage/outcome prose if it still says the verify leaf is a fixture, and add `Verification` to any classification enumeration on that page. Also update the outcome table's `Retry` row note if Task 1's Completed-candidate admission changes its wording ("`Retry` | `before_finalize` only").
- [ ] **Step 4: CHANGELOG.md** — one entry under the unreleased heading covering: kernel `RetryClassification::Verification` + Completed-candidate admission, runtime `result_message` on the `before_finalize` stage input, agent drive-loop supersession handling, and the verify crate's breaking API change (`VerifyDecision` removed, `EvidenceVerifier` introduced, `RequestInteraction` gone).
- [ ] **Step 5: Baselines** — `uv run --no-project python scripts/compat/public_items.py` then `mise run check-public-api`; confirm the verify/kernel/runtime `.txt` diffs contain exactly the intended additions/removals and commit them.
- [ ] **Step 6: Full gate** — workspace tests + clippy + `mise run check-public-api`, all exit 0.
- [ ] **Step 7: Commit** — `git commit -m "feat(verify): promote evidence verifier from fixture to battery"`.

---

## Deliberately out of scope

- **`RequestInteraction` / human-approval finalize.** Unlandable by design; stays out per the promotion brief. If approval-gated finalize is ever wanted, it needs a kernel input of its own, not a middleware outcome.
- **Feedback payload inside `RetryDirective`/`RetryScheduled`.** The records keep their current wire shape; feedback travels through the stateless `before_model` re-derivation instead. Extending the record schema is a separate ADR if re-derivation ever proves too limiting (e.g., non-deterministic verifiers).
- **Effect-bearing verification (actually running tests, fetching sources).** Middleware must stay pure; a verifier that needs committed effects should follow the ADR-042 compaction pattern (runtime-owned phase) — its own project.
- **Facade re-export.** Batteries (e.g. compaction) are consumed as standalone crates, not through `finstack-ai` features; verify follows suit.

## Self-review notes

- Spec coverage: bounce-with-feedback (Tasks 1–5), RequestInteraction removal (Task 5), fixture→battery packaging (Task 6), `middleware_stage_unlandable` no longer reachable from this crate's API (Task 5 removes the only trigger).
- Type consistency: `Verdict`/`EvidenceFinding`/`EvidenceVerifier`/`VerifyPolicy` named identically in Task 5's interface block, matrix, README snippet, and consumer-fix step; `RetryClassification::Verification` consistent across Tasks 1, 3, 4, 5.
- Known risks called out where they bite: line-number drift (header), serialization/enumeration sites for the new enum variant (Task 1 Step 4), message persistence across the bounce (pinned by Task 3 Step 2), and the facade's hard terminal expectation (Task 4).
