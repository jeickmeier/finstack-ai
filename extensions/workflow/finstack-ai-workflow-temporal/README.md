# finstack-ai-workflow-temporal

Temporal-shaped mapping over `finstack_ai_runtime::WorkflowSession`. This crate
does not depend on `temporalio`, a Temporal cluster, or the local sibling.

Activity retry intent is mapped through `retry_decision`. A deny decision never
enqueues `ExecuteEffect`.

See [`../README.md`](../README.md) for the vocabulary map and ownership
boundaries.
