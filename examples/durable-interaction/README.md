# durable-interaction

Offline approval workflow using the SDK's `durable-host` feature. The executable
starts two separate child processes. The first registers an application,
admits a run, ticks until approval is required, and exits. The second opens
the same SQLite database, registers the same stateless model and tool definitions,
resolves the interaction, and ticks through the tool and subsequent model cycle.
No original request or reducer-stage commands are supplied on recovery.

This crate is `publish = false` and is not a default dependency of
`finstack-ai-native-examples`.

Trust class: driver is T1;
journal payloads are T5.
In-process code is not isolated.

Workspace manifests are staged at **2.0.0**; build this example from the
repository until a 2.0 release is published.

## Quick start

```text
cargo run -p finstack-ai-example-durable-interaction --offline --locked
```

The assertions check `Accepted` delivery independently of run completion,
inbox/wake cleanup, and an unrelated abandoned interaction becoming `Closed`
through sweeping. `mise run test-durable-host` runs this example and the
SDK/worker recovery tests; `mise run test-examples` also executes it.

The embedded host owns scheduling, leases and the SQLite journal. Your application
owns registered component handles, credentials, artifact storage, the tick loop
and shutdown. Use a distinct worker ID for each concurrently live host. Keep
the application definition and durable artifact path available after restart.
Shutdown joins local driving; a dispatched external operation may already have
occurred. Non-retryable dispatch without a receipt reports uncertainty and is
never repeated automatically.

Recovery descriptors are immutable, versioned and bound to the exact run.
Configuration drift, missing descriptors, unavailable context/artifacts and lost
capability-activation proposals fail explicitly. Only committed capability masks
restore authority. Preserve the journal prefix needed by the host context descriptor;
pruning it currently requires application reconciliation before recovery.
