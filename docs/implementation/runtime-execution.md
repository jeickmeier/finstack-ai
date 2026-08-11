# Native runtime execution contract

This document records the implemented PR-020 native runtime ownership, bounds,
scheduling, and failure semantics. The planning documents remain authoritative
for product scope; this is the code-aligned operational description used by G2.

## Ownership

`RunTaskOwner` is the sole owner of every Tokio task created for one run. A
model-only owner joins the serialized run worker, model worker, timer worker,
and event hub. A model-and-tool owner additionally joins the tool scheduler.
`RunHandle` clones only bounded senders/status receivers and never acquire task
ownership.

Explicit shutdown closes command intake, cascades cancellation to model, tool,
and timer children, and joins all owned tasks within `shutdown_deadline`.
Remaining tasks are aborted only after that deadline. Dropping the owner closes
intake, signals children, aborts all remaining tasks, and records
`OwnerDropped`; dropping a handle does not cancel the run.

## Queue and concurrency bounds

| Path | Bound |
| --- | --- |
| Commands and timer jobs/results | `RunTaskConfig.command_capacity` |
| Event publications | `EventHubConfig.source_capacity` |
| Event subscribers | `EventHubConfig.max_subscribers` |
| Each subscriber inbox and delivered-batch queue | `EventSubscriptionConfig.queue_capacity` |
| Model committed jobs | `ModelTaskConfig.job_capacity` |
| Model progress/results | `ModelTaskConfig.result_capacity` |
| Tool committed jobs and pending scheduler queue | `ToolTaskConfig.job_capacity` |
| Tool progress/results | `ToolTaskConfig.result_capacity` |
| Active tool calls | `ToolTaskConfig.global_max_concurrency` plus each resolved tool's `max_concurrency` |

All capacities and concurrency limits are validated as non-zero before tasks
are spawned. Producers await capacity; subscription lag policy determines the
bounded handling of a slow consumer. No runtime path creates an unbounded Tokio
channel.

## Scheduling order

One run worker serializes every kernel transition and durable settlement. Its
biased selection order is model result/progress, tool result/progress, timer
result, then command intake. Each selected item completes its coordinator
boundary before the next item is selected.

The tool scheduler retains committed jobs in source order. It selects the first
pending job whose per-tool semaphore has capacity, while also requiring a
global permit. This permits unrelated tools to progress without bypassing a
same-tool concurrency ceiling. The kernel remains the authority for execution
groups, tool-result ordering, and failure policy.

## Commit and failure semantics

The coordinator decides, appends, verifies the store receipt, applies the
committed batch, publishes durable-derived events, rechecks the dispatch
precondition, and only then dispatches an external execute/cancel action.

- Definite store errors before acknowledgement return without external dispatch.
- Ambiguous acknowledgement retries the identical frozen append once; continued
  ambiguity faults the run.
- Receipt mismatch, replay/apply uncertainty, or repeated conflict faults the
  run before dispatch.
- Event delivery or external dispatch failure after commit faults only that run;
  the committed prefix remains recoverable.
- Shutdown discards late progress/settlements after intake closes and uses the
  recorded shutdown report to distinguish graceful, forced, and dropped-owner
  termination.

Native tests may call `CommitCoordinator::enable_manual_drive` before spawning
an owner. Every committed execute or cancel action then pauses after the final
authorization/deadline recheck and immediately before external dispatch.
`ManualDriveController::next_effect` exposes the stable effect id/action, and
`ManualDrivePermit::continue_dispatch` releases exactly that action. Dropping a
permit or controller fails closed with a stable post-commit fault; it never
rewinds or conceals the committed prefix.

## G2 proof commands

- `mise run test-runtime`
- `mise run test-lifecycle`
- `mise run test-runtime-gate`
- `mise run test-miri`
- `mise run benchmark-smoke`
- `mise run ci`

The benchmark harness publishes reducer cost, normalized model-stream
throughput, normalized tool-stream throughput, and incremental idle-session RSS
without provider, network, or storage latency.
