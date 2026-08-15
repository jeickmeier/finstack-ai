# finstack-ai-workflow-local

In-process reference workflow driver. It wraps
`finstack_ai_runtime::WorkflowSession` and does not reimplement the
model/tool continuation loop.

Durable sleep advances only through `ExternalClock`. Signals use
`InteractionRouter` or `ExternalCompletionRouter` with an explicit
locator. Worker restart is `WorkflowSession::resume` after
`JournalStore::load` + `recover_run`.

See [`../README.md`](../README.md) for ownership boundaries.
