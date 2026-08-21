# finstack-ai-workflow-local

In-process workflow driver and tenant-scoped interval scheduler.

`LocalWorkflowDriver::attach` is the production constructor and uses
operating-system entropy through `WorkflowSession`. Deterministic replay tests
and reproducible examples use `attach_seeded` explicitly.

`CronScheduleStore::load_due(now, limit)` is bounded. The memory and SQLite
stores return stable tenant/schedule ordering, and claims use compare-and-set
on the observed next-fire timestamp.

`SqliteCronStore` owns `finstack_workflow_local_schema` at version `1` without
changing the journal's `PRAGMA user_version`. An historical unversioned cron
table is rejected with `cron_schema_reset_required`; use a fresh adapter
database for this breaking release.

The kernel journal remains the semantic authority. Cron rows are adapter
state used only to decide which durable run start to attempt next.
