# durable-interaction

Offline example of a typed interaction that survives a simulated worker
restart. The local workflow driver resumes from the SQLite journal; the
kernel remains authoritative for `InteractionId` and effect identity.

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
