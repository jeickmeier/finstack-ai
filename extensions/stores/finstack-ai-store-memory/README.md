# finstack-ai-store-memory

Bounded, explicitly non-durable in-memory `JournalStore`
implementation. Envelopes are encoded and verified through
`finstack-ai-protocol`. `health().durable` stays `false`.

Use this crate for tests and short-lived processes. Do not advertise it as
satisfying NFR-REL-001. The durable local leaf is
[`finstack-ai-store-sqlite`](../finstack-ai-store-sqlite/README.md).

`MemoryJournalStore::try_new` requires explicit resource ceilings. A zero
ceiling fails closed (`StoreError::InvalidRequest`).

See [durability](../../../docs/site/durability.md).
