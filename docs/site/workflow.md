# Workflow

Durable workflow execution outside a single in-process call: a driver that
attaches a `WorkflowSession` and drives it to its next wait, a leased worker
that resumes parked sessions from adapter-owned hint tables, and a
human-in-the-loop router battery over that worker's interaction inbox. All
three are T1 native mappings. None is isolated; none replaces the kernel
journal as the authority for what actually happened.

## Shipping leaves

| Leaf | Status |
| --- | --- |
| [`finstack-ai-workflow-local`](../../extensions/workflow/finstack-ai-workflow-local/README.md) | In-process reference driver. Drives `WorkflowSession::attach` / `drive_until_wait`; owns adapter cron scheduling (not a kernel `RecordBody`), computed against the session's `ExternalClock`. |
| [`finstack-ai-workflow-worker`](../../extensions/workflow/finstack-ai-workflow-worker/README.md) | Leased worker for `finstack-ai-workflow-local`. Polls the wake index and cron table, claims due work with CAS leases, resumes Timer / Interaction / DeferredEffect waits. Ships a stock daemon binary; resuming runs requires a host-registered `PortsFactory`. |
| [`finstack-ai-workflow-hitl`](../../extensions/workflow/finstack-ai-workflow-hitl/README.md) | Battery. Durable interaction inbox over the workflow worker: capture on park, authorized resolve, expiry sweep. The shipped `ExpiryPolicy` default is fail-closed (declines every row) because no battery-authored credential satisfies the runtime's interaction ingress; credential-free expiry is instead driven by the worker tick (kernel `ExpireIfDue`, counted in `TickReport::sessions_expired`), and a host that accepted the run can still install a policy presenting the run's own credentials for an authored refusal. |

## Lifecycle across the three crates

1. A session parks on a wait (`finstack-ai-workflow-local` classifies it);
   `finstack-ai-workflow-worker::park` (or `finstack-ai-workflow-hitl::park`,
   which composes the same call) indexes a wake row.
2. `finstack-ai-workflow-hitl::capture` additionally records an `Interaction`
   wait as an inbox row (`Open`), keyed by `(tenant_scope, interaction_id)`.
3. An operator resolves through `HitlRouter::resolve`, or a host's
   `ExpiryPolicy` resolves it via `HitlRouter::sweep`; either way the
   resolution is buffered in the worker's inbox, not applied directly.
4. `WorkflowWorker::tick` (via `spawn`'s loop, or a host driving it directly)
   applies the buffered resolution to the kernel journal and resumes the
   run.

Every table involved — cron, wake, fires, worker inbox, HITL inbox — is a
hint. The kernel journal stays authoritative, and `HitlRouter::sweep`'s
reconcile pass exists specifically to close inbox rows the journal already
settled out of band.

## Source of truth

- [`finstack-ai-workflow-local` README](../../extensions/workflow/finstack-ai-workflow-local/README.md)
- [`finstack-ai-workflow-worker` README](../../extensions/workflow/finstack-ai-workflow-worker/README.md)
- [`finstack-ai-workflow-hitl` README](../../extensions/workflow/finstack-ai-workflow-hitl/README.md)
- [Technical Design §2](../planning/03-finstack-ai-technical-design.md) for
  ownership boundaries between kernel, runtime, and native-mapping leaves.

## License

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[Contributing](../../CONTRIBUTING.md). [Security](../../SECURITY.md).
