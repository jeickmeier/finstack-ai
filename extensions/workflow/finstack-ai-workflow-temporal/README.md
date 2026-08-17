# finstack-ai-workflow-temporal

Temporal-shaped mapping over `finstack_ai_runtime::WorkflowSession`. This crate
does not depend on `temporalio` or a Temporal cluster. The in-process reference
driver is `finstack-ai-workflow-local`.

Activity retry intent is mapped through `retry_decision`. A deny decision never
enqueues `ExecuteEffect`.

This crate is a T1 native mapping. It is not isolated and does not depend on
`temporalio`. See [Technical Design §2](../../../docs/planning/03-finstack-ai-technical-design.md)
for ownership boundaries.

```rust
use finstack_ai_runtime::WorkflowSession;
use finstack_ai_workflow_temporal::TemporalShapedWorker;

// `session` is a recovered `WorkflowSession`. This crate maps Temporal-shaped
// IDs and retry intent; it does not construct the session.
let worker = TemporalShapedWorker::wrap(session, 3);
```
