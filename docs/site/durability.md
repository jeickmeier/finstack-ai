# Durability

JournalStore is a port. Memory and SQLite are leaf implementations. The
kernel remains authoritative for records, effect identity, and
`InteractionId`.

## Memory vs SQLite

| Store | Use |
| --- | --- |
| `finstack-ai-store-memory` | Tests and short-lived processes |
| `finstack-ai-store-sqlite` | Local durable journals (WAL + FULL) |

Browser IndexedDB is experimental host storage. It does not meet
NFR-REL-001.

## Inspect, do not continue

`Session::open` / `Agent.open_session` rebuilds a handle from the journal
and does not respawn a parked run. Resume belongs to an explicit workflow
driver when one is composed. See
[examples/durable-interaction](../../examples/durable-interaction/README.md).

## Interactions and at-least-once

Typed interactions persist before a privileged tool may run. Recovery is
at-least-once. Exactly-once is not claimed. Conflicting completions fail
closed.

## Trust

Stores and workflow drivers that run in-process are [T1](security-trust-levels.md).
Journal contents are [T5](security-trust-levels.md) data.

## License

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[Contributing](../../CONTRIBUTING.md). [Security](../../SECURITY.md).
