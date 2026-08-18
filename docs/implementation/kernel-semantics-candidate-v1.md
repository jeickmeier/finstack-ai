# Kernel semantics candidate-v1

Status: reviewed implementation reference for the Phase 1 candidate-v1 kernel.

Authority: this file summarizes the contracts in `docs/planning/`; it does not
replace them. Candidate-v1 is the only compatibility promise. Runtime effect
execution, provider streaming, interaction routing, journal stores, protocol
framing, bindings, and plugin hosts remain owned by later phases.

## Phases and reachability

| `RunPhase` | Candidate-v1 meaning | Reachability |
| --- | --- | --- |
| `Accepted` | acceptance record has committed | transient, normalized to `BeforeRun` by apply |
| `BeforeRun` | settle the aggregate pre-run stage | reachable |
| `PreparingContext` | commit prepared model context | reachable |
| `BeforeModel` | settle the final stage before model request | reachable |
| `AwaitingModel` | direct model effect outstanding | reachable |
| `AfterModel` | model response awaits aggregate settlement | reachable |
| `BeforeToolBatch` | validate and open the source-ordered tool plan | reachable |
| `AwaitingTools` | direct tool effects outstanding | reachable |
| `AfterToolBatch` | complete tool-batch settlement | reachable |
| `BeforeFinalize` | final behavior-changing boundary | reachable |
| `AwaitingInteraction` | typed interaction outstanding | reserved; routing is later-owned |
| `AwaitingExternal` | original effect is deferred to an external handle | reachable |
| `Sleeping` | semantic timer outstanding | reserved lifecycle state |
| `Cancelling` | cancellation reconciliation in progress | reachable |
| `Suspended` | operator or application action required | reserved lifecycle state |
| `Completed` | successful immutable terminal | reachable |
| `Failed` | failed immutable terminal | reachable |
| `Cancelled` | cancelled immutable terminal | reachable |

## Normalized commands

The complete `KernelInput` vocabulary is `AcceptRun`, `StageSettled`,
`ModelSettled`, `ExternalEffectCompleted`, `ToolBatchSettled`, `CancelRequested`,
`CancellationReconciled`, `TimerFired`, `ConfigureOutput`,
`CapabilitiesActivated`, `OutputValidated`, `RecordExternalCommandRejected`,
`RequestInteraction`, `InteractionSettled`, and `RequestCompactionModel`.

| Input | Permitted state | Result class |
| --- | --- | --- |
| `AcceptRun` | no accepted run | `RunAccepted` |
| `ConfigureOutput` | `BeforeRun`, before execution starts | `OutputConfigured` |
| `CapabilitiesActivated` | safe activation checkpoint | `CapabilitiesActivated` |
| `StageSettled` | exact stage cursor for the current phase | stage record plus its required sibling records |
| `ModelSettled` | `AwaitingModel` | direct model settlement records |
| `ExternalEffectCompleted` | `AwaitingExternal` | settlement of the original deferred effect |
| `ToolBatchSettled` | `AwaitingTools` or deferred tool work | source-ordered tool settlement records |
| `CancelRequested` | non-terminal run | cancellation intent and authorized cancellation actions |
| `CancellationReconciled` | `Cancelling` | cumulative reconciliation or terminal cancellation |
| `TimerFired` | exact pending timer in `Sleeping` | timer firing and retry continuation |
| `OutputValidated` | structured output awaiting validation | valid final result or retry feedback |
| `RequestCompactionModel` | `BeforeModel` with no pending model (1.2-oriented; RFC-0001) | compaction-summary `EffectRequested` without consuming the cursor |

All other phase/input pairs fail with `invalid_phase_input`. Reserved phases do
not fabricate transitions. Equal duplicate completion identities are no-ops;
unequal reuse fails closed.

## Durable records

Candidate-v1 `RecordBody` vocabulary:

- `RunAccepted`, `EffectRequested`, `EffectDeferred`, `EffectCompleted`,
  `EffectFailed`, `EffectCancelled`
- `InteractionRequested`, `InteractionResolved`, `InteractionExpired`,
  `InteractionCancelled`
- `StageOutcomeRecorded`, `ContextPrepared`, `EntryAppended`
- `ToolBatchOpened`, `ToolCallSettled`, `ToolBatchClosed`
- `CancellationRequested`, `CancellationReconciled`, `LimitReached`,
  `RetryScheduled`, `TimerFired`, `RunSuspended`
- `RunCompleted`, `RunFailed`, `RunCancelled`
- `OutputConfigured`, `CapabilitiesActivated`, `FinalResultRecorded`,
  `OutputValidationFailed`
- `ExternalCommandRejected`, `ChildRunPrepared`
- `BudgetReservationRequested`, `BudgetReservationSettled`,
  `BudgetChargeRecorded`, `BudgetReservationReleased`
- `SessionCreated`, `LaneCreated`, `LaneMoved`, `SnapshotWritten`,
  `ConversationEntry`

Request records precede their `ExecuteEffect` action. Interaction request
records pair with exactly one matching interaction `EffectRequested` record in
the same append request. Settlement siblings are present once and in canonical
order. Tool results finalize in assistant source order, independent of external
completion order.

## Events, effects, and actions

`RunEventBody` vocabulary is `RunAccepted`, `EffectRequested`, `EffectDeferred`,
`EffectCompleted`, `EffectFailed`, `EffectCancelled`, `InteractionRequested`,
`InteractionResolved`, `InteractionExpired`, `InteractionCancelled`,
`MessageFinalized`, `ToolSettled`, `LimitReached`, `RunSuspended`, `RunCompleted`,
`RunFailed`, `RunCancelled`, `ModelTextDelta`, `ReasoningDelta`, `ToolProgress`,
`QueueDepthWarning`, and `ProviderHeartbeat`.

Durable events derive only from committed record body, declared event ordinal,
and correlations. Transient provider events do not mutate kernel state.

`EffectInput` vocabulary is `Model`, `Tool`, `Context`, `Middleware`,
`Interaction`, and `Timer`. Deferral preserves the original effect identity,
input, output contract, and retry-safety contract. `PostCommitAction` vocabulary
is `ExecuteEffect` and `CancelEffect`; actions are authorized only by the
preceding committed batch.

## Stable reducer failure families

`invalid_input_payload`, `invalid_run_acceptance`, `invalid_phase_input`,
`stage_cursor_mismatch`, `model_request_contract_mismatch`,
`model_settlement_mismatch`, `duplicate_tool_call`, `tool_batch_plan_mismatch`,
`tool_effect_contract_mismatch`, `tool_settlement_mismatch`,
`tool_result_mismatch`, `assistant_message_presence_mismatch`,
`assistant_message_mismatch`, `settlement_digest_mismatch`,
`context_digest_mismatch`, `conflicting_completion_id`, `cycle_overflow`,
`allocated_ids_exhausted`, `unused_allocated_ids`, `state_capacity_exceeded`,
`committed_batch_range_mismatch`, `non_contiguous_record_sequence`,
`record_identity_mismatch`, `invalid_record_order`, `effect_not_pending`,
`conflicting_settlement`, `terminal_state_immutable`, `state_hash_failed`, and
`invariant_violation`.

Strict DTO decoding additionally preserves stable value-type errors including
`unknown_field`, `unknown_variant`, `unsupported_format_version`,
`unsupported_kind_version`, `derived_event_count`, and
`invalid_cancellation_pair`.

## Numbered invariants

1. **INV-001 Pure decision.** `decide(state, env, input)` is deterministic and never mutates state.
2. **INV-002 Atomic apply.** A rejected batch leaves semantic state and state hash unchanged.
3. **INV-003 Replay equivalence.** Full and prefix replay preserve record order, event order, terminal projection, and state hash.
4. **INV-004 Commit before effect.** Every executable or cancellable action is authorized by a preceding committed request record.
5. **INV-005 Ordered tools.** Tool settlement is finalized in source order under every completion permutation.
6. **INV-006 Strict ceilings.** Exact limits succeed and one-over-limit inputs fail with stable errors.
7. **INV-007 Idempotency.** Equal duplicate identities are no-ops; conflicting reuse fails closed.
8. **INV-008 Attenuated lineage.** Child depth, principal, deadline, budget, and cancellation policy never amplify parent authority.
9. **INV-009 Deferral identity.** Deferral preserves original effect and output-contract identity.
10. **INV-010 Final boundary.** `BeforeFinalize` is the final behavior-changing transition and all terminal states are immutable.
11. **INV-011 Typed interactions.** DTO validation, pairing, sibling rules, bounds, serialization, and event derivation are enforced without routing behavior.
12. **INV-012 Version fail-closed.** Unsupported record, state, trace, and fixture versions are rejected before state mutation.

## Candidate-v1 fixture inventory

- `golden-trace/v1` freezes normalized durable records, events, effects, terminal
  projections, and hashes for the Phase 1 reducer paths.
- `public-rust-api/v1` freezes strict public DTO decoding, boundary recipes,
  record/event derivation, and the `corrupt-replay` mutation corpus.
- `conformance/v1` retains candidate-v1 fixture and golden-trace coverage
  assertions. Corpus counts live in executable tests, not prose.
