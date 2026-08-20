# Durability

JournalStore is a port. Memory and SQLite are leaf implementations. The
kernel remains authoritative for records, effect identity, and
`InteractionId`.

## Memory vs SQLite vs PostgreSQL

| Store | Use |
| --- | --- |
| `finstack-ai-store-memory` | Tests and short-lived processes |
| `finstack-ai-store-sqlite` | Local durable journals (WAL + FULL) |
| `finstack-ai-store-postgres` | Multi-process durable journals (synchronous_commit=on) |

Browser IndexedDB is experimental host storage. It does not meet
NFR-REL-001.

`finstack-ai-store-postgres` also accepts a Relaxed durability mode
(`durable=false`, `health().detail` labeled `postgres synchronous_commit=off`)
that must never be reported as meeting NFR-REL-001. Its v1 wiring is
plaintext-only (`tokio_postgres::NoTls`): a connection URL whose `sslmode`
demands TLS (`require`, `verify-ca`, `verify-full`) is rejected up front with
a stable `postgres_tls_unsupported` error rather than silently connecting in
plaintext. Use network-layer TLS (a stunnel/PgBouncer sidecar, a private
network, or an SSH tunnel) until in-process TLS wiring lands.

## Inspect, do not continue

`Session::open` / `Agent.open_session` rebuilds a handle from the journal
and does not respawn a parked run. Python `Lane.resume` respawns the
in-process owner after open. See
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
