# Workflow runtime drivers

Trusted native leaves that drive the existing runtime. They are **not** a
seventh port and do not own agent-run semantics.

## Ownership

```text
workflow engine          business stages, durable sleep, signals,
                         worker identity, retry *intent*

kernel journal           agent-run semantics, effect IDs, phases,
                         settlement fingerprints

runtime driver           commit-before-effect, recover, ingress,
                         retry_decision, clock injection

application              tenant auth, inbox UI, Temporal/Restate
                         cluster, compensation graphs
```

Higher-level engines may compose multiple runs. One run follows the
canonical model/tool machine. `Session::open` stays inspect-not-continue;
resume lives on the workflow driver.

## Packages

- `finstack-ai-workflow-local` — in-process reference driver
- `finstack-ai-workflow-temporal` — Temporal-shaped mapping only
  (no `temporalio`, no cluster, no network listener)

## Temporal-shaped vocabulary

| Temporal-shaped primitive | Kernel / runtime target |
| --- | --- |
| workflow / run ID | `RunId` plus explicit `OperationLocator` |
| activity ID / idempotency key | original `EffectId` |
| `workflow.sleep` / timer | timer effect + `ExternalClock` / `WorkflowWait::Timer` |
| signal | interaction resolution or external completion |
| activity retry policy | `WorkflowDriver::retry_decision` then maybe re-dispatch |
| worker restart | `WorkflowDriver::resume` |
| continue-as-new / child workflow | out of scope |

Do not add Temporal, Restate, or DBOS SDKs to workspace dependencies.
Exactly-once external side effects are not claimed.
