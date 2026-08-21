# finstack-ai-middleware-verify

Evidence-verifier battery for `before_finalize`. It wraps one pure,
deterministic `EvidenceVerifier` and runs it at two stages:

- `before_finalize` judges the terminal candidate's canonical assistant
  `Message`. `Verdict::Accept` continues; `Verdict::Bounce` requests a
  semantic `RetryClassification::Framework` retry (`StageOutcome::Retry`);
  `Verdict::Reject` fails the run with the stable, non-retryable
  `verify_rejected` code.
- `before_model` re-derives the same verdict, statelessly, from the trailing
  draft message of a bounced cycle, and renders any `Bounce`/`Reject`
  findings as one bounded, truncated `AddContext` feedback item so the model
  sees what was wrong on its next attempt.

The middleware never writes a store and cannot `Replace`,
`AddInstructions`, or `CompactContext`.

## What it verifies

A verifier is anything implementing `EvidenceVerifier`:

```rust
pub trait EvidenceVerifier: Send + Sync + fmt::Debug {
    fn verifier_id(&self) -> &str;
    fn verify(&self, message: &finstack_ai_kernel::RawJson) -> Verdict;
}
```

`verify` receives the assistant candidate as JCS-canonical `Message` JSON —
**byte-identical at both call sites**: the runtime's own `before_finalize`
`result_message` encoding and the crate's stateless `before_model`
re-derivation use the same canonicalizer
(`finstack-ai-runtime::stage_settlement::codec::canonical_message`, mirrored
here via `serde_json_canonicalizer`). A verifier does not need to know which
stage it was called from.

`Verdict` carries `Vec<EvidenceFinding>` on its non-`Accept` arms.
`EvidenceFinding::try_new(kind, note)` pairs an `EvidenceKind`
(`Citation` | `Test` | `Artifact`) with a bounded, non-secret note; oversized
notes are truncated rather than rejected.

## The bounce loop

1. The candidate lands at `before_finalize`. The verifier returns
   `Verdict::Bounce(findings)`.
2. `VerifyMiddleware` turns that into `StageOutcome::Retry` carrying a
   `RetryDirective { classification: Framework, backoff, policy_version }`
   from the configured `VerifyPolicy`.
3. The kernel commits `RetryScheduled` — for a `Completed` candidate this is
   the one case where a *terminal, non-failed* candidate is still admitted
   into a retry; the kernel synthesizes a `candidate_rejected`
   (retryable, `Validation`) prior error since there is no middleware error
   to reuse.
4. A timer gates the next cycle. `Agent::run`'s drive loop continues past a
   `FinalizeAccepted` that gets superseded by this retry instead of erroring.
5. On the new cycle, at `before_model`, the same verifier re-judges the
   trailing draft message (byte-identical canonical JSON) and, if still not
   `Accept`, contributes one `AddContext` item summarizing the findings so
   the model has feedback for its next attempt. Nothing about the bounce is
   journaled beyond the `RetryScheduled` record. The durable
   `candidate_rejected` error and `policy_version` retain the bounce's
   attribution; recovery just re-runs the pure verifier.

## The determinism obligation

`EvidenceVerifier::verify` must be a pure function of its input message: the
same canonical JSON must always produce the same `Verdict`. Stage
invocations are never individually journaled, so after a crash the entire
chain re-runs from its first component, including components that already
ran. A verifier with hidden state, wall-clock reads, or network calls will
diverge between the original run and its replay. Verifiers needing
committed, effect-bearing checks (running a test suite, fetching a source)
do not belong here; they require a runtime-owned durable effect path that this
crate deliberately does not implement.

## `RequestInteraction` is gone

The former `VerifyDecision::RequestInteraction` mode has been removed
outright rather than kept as a documented dead end. Pausing a run for human
approval has no single-settlement middleware shape, so the fold has nowhere
for it to land. Approval-gated finalize, if added, needs a kernel input of its
own rather than a middleware outcome.

## Usage

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

This crate is a T1 native adapter. It is not isolated.
