# finstack-ai-workflow-local

In-process reference workflow driver. It drives
`finstack_ai_runtime::WorkflowSession` and does not reimplement the
model/tool continuation loop.

Manual start uses `WorkflowSession::attach` / `drive_until_wait`.
Cron is adapter-owned state, not a kernel `RecordBody`. Schedule rows
live in `finstack_workflow_local_cron` in the same `JournalStore`
sqlite file (or an in-process table for `MemoryCronStore`), scoped by
`WorkflowSession::tenant_scope()`.

Next-fire instants are computed against `WorkflowSession::clock()`
(`ExternalClock`), never wall time. On attach, a schedule whose
next-fire is in the past fires once, then the expression advances to
the next future tick. Missed ticks are not backfilled.

This crate is a T1 native mapping. It is not isolated. See
[Technical Design §2](../../../docs/planning/03-finstack-ai-technical-design.md)
for ownership boundaries.

```rust
use finstack_ai_runtime::WorkflowSession;
use finstack_ai_workflow_local::{IntervalSchedule, LocalWorkflowDriver, MemoryCronStore};

// `session` is a recovered `WorkflowSession`. Cron state is loaded from
// the adapter table on `LocalWorkflowDriver::attach`.
let cron = MemoryCronStore::new();
let expr = IntervalSchedule::parse("every 10ms").expect("expr");
let _ = (session, cron, expr);
```
