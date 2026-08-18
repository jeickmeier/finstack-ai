# ADR-042: Model-assisted compaction as a runtime phase

## Status

Accepted

## Date

2026-08-17

## Accountable role

Core/runtime lead

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

The `summarize` compaction strategy returns `StageOutcome::RequestCompactionModel`.
The aggregate-fold design settles a stage exactly once as one
`ReducerStageOutcome`, and a middleware invocation is never a committed
effect. There is therefore no middleware parent to attach a child model
effect to, and `RequestCompactionModel` remains `middleware_stage_unlandable`
when it reaches `StageFold::accumulate`.

FR-02 requires a committed child model effect under
`EffectPurpose::CompactionSummary`, chain re-entry that carries the
summary, at-least-once crash recovery without duplicating the summary,
and token charges against the same run budget as the primary request.
Three shapes were named:

- **(a)** Middleware-owned child: give `before_model` a committed parent
  envelope and allow bounded chain re-entry. Makes middleware
  effect-bearing and weakens "a stage settles exactly once."
- **(b)** Runtime-owned phase between `PrepareContext` and `BeforeModel`,
  extending the context-driver pattern. Middleware stays pure.
- **(c)** Two-pass settlement: a deferred stage outcome that settles after
  the runtime fulfills a declared model request. Changes recovery for
  every stage.

`protected` stays authoritative-from-the-context-port plus the structural
rule. This record does not fabricate it.

## Decision

Select **(b)**. Model-assisted compaction is a runtime-owned phase
between `PrepareContext` and `BeforeModel`.

- The phase commits an `EffectRequested(Model)` with
  `EffectPurpose::CompactionSummary` and parent linkage to the derived
  BeforeModel stage identity. That identity is a correlation id, not a
  journaled middleware effect.
- Dispatch uses `validate_model_request` against the locked context
  profile. Usage is applied through the ordinary `EffectCompleted` path
  and counts as a model request on the same `RunLimits` budget.
- `ModelSettled` of a compaction-summary effect does not append an
  assistant `ConversationEntry` and restores `RunPhase::BeforeModel`.
  Canonical history is unchanged.
- The BeforeModel chain then re-enters with `CompactionModelResume` and
  lands `CompactContext` once. `fold.rs` still refuses
  `RequestCompactionModel` if it reaches `accumulate`.
- Crash after the compaction request and before re-entry reuses the
  committed effect id (at-least-once). A completed summary is not
  requested again.

No new `RecordBody` variant. Unknown journal kinds stay fatal for 1.0
readers. Durable event *order* changes: compaction
`EffectRequested` / `EffectCompleted` may appear after `ContextPrepared`
and before `StageOutcomeRecorded(BeforeModel)`. That order change is
documented in RFC-0001 as 1.2-oriented additive work.

`KernelInput::RequestCompactionModel` is a command, not a journal kind.
It does not consume the BeforeModel cursor. Its decision emits no
`PostCommitAction`; the runtime phase executes the model after commit.

## Consequences

- Middleware remains non-effect-bearing. `Middleware::reconcile` stays
  unused. Sliding-window and large-tool-output `CompactContext` are
  unchanged.
- Summarize completes against a scripted model with deterministic output.
- One extra metered model request per summarize turn. No unmetered call.
- 1.0.x journals that never emitted this sequence remain readable.
  Readers that assume no model effect exists before BeforeModel must
  tolerate the new order on 1.2+ journals.

## Rejected alternatives

**(a) Middleware-owned child.** Rejected: it makes middleware
effect-bearing and weakens "a stage settles exactly once," which
recovery depends on (the driver re-runs the whole chain).

**(c) Two-pass settlement.** Rejected: it changes recovery semantics for
every stage, not only compaction.

**New `RecordBody` / `StageDisposition` variant.** Rejected: unknown
kinds are fatal to 1.0.x readers. Existing `EffectRequested` /
`EffectCompleted` with `EffectPurpose::CompactionSummary` carry the
child and parent linkage.

**Fabricate `protected` so deterministic strategies pass.** Rejected:
FR-01 is the correct unblock. The constrained party must not certify
itself.

**Land `RequestCompactionModel` in `fold.rs`.** Rejected: that is option
(a).

## Compatibility and schema-change classification

No new journal kind. Additive `KernelInput` command. Additive phase-matrix
arm: `BeforeModel` may accept a compaction `EffectRequested`;
`AwaitingModel` may accept a compaction `EffectCompleted` without
`EntryAppended` and return to `BeforeModel`. Event order between
`ContextPrepared` and BeforeModel settlement changes. See RFC-0001.

## Security classification

- References: SEC-INV-013; TM-21
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: Compaction summaries stay untrusted derived
  projections. The child model is authorized for the frozen source
  sensitivity and charges the locked budget. No seventh port. No
  fabricated `protected` bit.

## Affected requirements, design, and delivery

- Affected requirements: FR-02, FR-MW, FR-CTX
- Affected Technical Design: Technical Design §17.6 (model-assisted
  compaction). Planning files are not edited; this record selects
  option (b) over the middleware-parent sequence in §17.6.
- Implementation: FR-02 Task 14
- RFC companion: [RFC-0001](../../rfcs/0001-compaction-summary-event-order.md)

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.
It refines ADR-037 without replacing it.

## Reconsideration conditions

May change only through a new superseding ADR. Reopening option (a) or
(c), adding a `RecordBody` variant, or making middleware effect-bearing
requires that path.

## Approval and implementation-evidence links

- Approval: accepted by the decision owner to execute FR-02 Task 14
  locally only, without publication
- Implementation evidence: Partial — local phase `c8992bc312bdc3954788c4411fb3203e91167c9b`
  and host-task resume `033de86c634656cc69621d5d88589b63325a00ba`
  (`cargo test -p finstack-ai-runtime --locked --lib exec::compaction_driver`).
  No published evidence id. Fold residual if `RequestCompactionModel` reaches
  `StageFold::accumulate` is unchanged.
