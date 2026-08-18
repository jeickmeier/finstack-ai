# RFC 0001: Compaction-summary model effects between PrepareContext and BeforeModel

- Status: Accepted (1.2-oriented additive work)
- Author: me@jeickmeier.com
- ADR companion: ADR-042 (required)
- Affects: journal event order (existing kinds only)

## Summary

Model-assisted compaction journals an `EffectRequested(Model)` /
`EffectCompleted` pair with `EffectPurpose::CompactionSummary` after
`ContextPrepared` and before `StageOutcomeRecorded` at `BeforeModel`.
No new `RecordBody` variant is added. Unknown kinds remain fatal.

## Motivation

FR-02 requires a committed, replayable child model effect with parent
linkage and commit-before-effect. The kernel's only existing model-request
landing consumes the BeforeModel cursor and becomes the primary request.
A runtime phase therefore needs a non-consuming command that still
emits the existing model-effect records.

## Proposal

- New `KernelInput::RequestCompactionModel` (command, not a journal kind).
- Decision emits `RecordBody::EffectRequested` with
  `EffectRelation { parent_effect_id, purpose: CompactionSummary { … } }`.
- `parent_effect_id` is the derived BeforeModel stage identity (correlation
  id). It is not a journaled `EffectKind::Middleware` record.
- `ModelSettled` of that effect emits `RecordBody::EffectCompleted` only.
  No `EntryAppended`. Phase returns to `BeforeModel`.
- 1.0.x readers already know these kinds. A 1.0.x writer never produces
  this sequence. A 1.2+ writer that emits it is additive.
- This work targets 1.2.0. Do not ship it in a 1.0.x patch: 1.0 readers
  that assume "no model effect before BeforeModel" will see a new legal
  order.

Unknown kinds stay fatal. This RFC does not add a kind.

## Compatibility

Additive event-order change using existing record kinds. Classified as
1.2-oriented under
[compatibility-governance.md](../implementation/compatibility-governance.md).
No migration of 1.0.x journals. No WIT or remote-protocol change.

## Security

SEC-INV-013 / TM-21. Summaries remain untrusted derived projections.
Token use is metered on the same run budget. Threat-model notes are in
ADR-042. This RFC does not invent a passed threat-model review id.

## Alternatives

A new `RecordBody` or `StageDisposition` variant — rejected; fatal to
1.0.x readers. Middleware-owned parent effect — rejected in ADR-042
option (a).

## Unresolved questions

None for the 1.2 landing. Prompt-cache interaction with the extra model
call is recorded, not solved.
