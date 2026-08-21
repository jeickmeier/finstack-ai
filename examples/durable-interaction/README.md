# durable-interaction

Offline example of a typed interaction that survives a simulated worker
restart. The local workflow driver resumes from the SQLite journal; the
kernel remains authoritative for `InteractionId` and effect identity.

This crate is `publish = false` and is not a default dependency of
`finstack-ai-native-examples`.

Trust class: driver is T1;
journal payloads are T5.
In-process code is not isolated.

Workspace version is **2.0.0** unpublished (last public tag `v0.1.0`).

## Quick start

```text
cargo run -p finstack-ai-example-durable-interaction --offline --locked
```
