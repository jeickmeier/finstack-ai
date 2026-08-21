# finstack-ai-workflow-hitl

Human-in-the-loop inbox and resolution router over
`finstack-ai-workflow-worker`. The journal remains authoritative.

## Lifecycle

```text
Open or Rejected --resolve--> Buffered --worker ingress--> Accepted
                                      `---------------> Rejected
Open or Buffered --no matching wake--> Closed
```

- `capture` records the committed request plus the run's exact accepted
  principal and authorization evidence.
- `pending` returns at most 100 actionable `Open` or `Rejected` rows for one
  tenant, oldest first.
- `resolve` requires exact equality with that accepted principal and evidence
  before running the host authorizer or writing the worker inbox.
- `Buffered` means the worker durably holds the command. Register
  `HitlLifecycle` through `WorkerBuilder::interaction_lifecycle` to record the
  runtime ingress's authoritative `Accepted` or `Rejected` outcome and to
  capture new interactions created during worker re-park.
- `sweep` performs bounded point reconciliation against the wake index. It
  closes rows whose journal-authoritative interaction wait is gone.

Rejected rows remain actionable, and their stable `outcome_code` explains the
last ingress refusal. The rejected worker command is retained separately as a
dead letter. A corrected resolution can therefore be submitted without losing
the operator evidence for the prior refusal.

## Expiry ownership

This crate has no expiry-policy hook. The runtime already owns interaction
deadline semantics and applies credential-free expiry when a worker attaches a
past-deadline wait. The next reconciliation closes the adapter row. Keeping one
semantic owner prevents an inbox annotation from claiming an outcome the
journal did not accept.

## SQLite schema

`SqliteHitlStore` owns `finstack_workflow_hitl_schema` at version `1` and does
not modify `PRAGMA user_version`. It can share an adapter file with
`SqliteWorkerStore`. Historical unversioned HITL tables are rejected with
`hitl_schema_reset_required`; create a fresh adapter database for this
breaking release.
