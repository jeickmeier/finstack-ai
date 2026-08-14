# finstack-ai-store-sqlite

Trusted native SQLite [`JournalStore`](https://docs.rs/finstack-ai-runtime) leaf.
This crate is opt-in. Do not treat it as the Agent, Python, or WASM default.

Records are append-only and authoritative. There is no prune API in this
version. Snapshots are a disposable cache, not a second semantic model.

## Durability

`SqliteDurability::Durable` uses WAL plus `synchronous=FULL` (and Darwin
`fullfsync` / `checkpoint_fullfsync` where SQLite exposes them). Only that
mode may report `health().durable = true`. Relaxed `NORMAL` / `OFF` and
in-memory paths stay labeled non-durable and must not be advertised as
satisfying NFR-REL-001.

An append acknowledgement is returned only after `COMMIT` succeeds under the
configured pragmas. Acknowledgement still depends on the OS and filesystem
honoring those flush settings.

## Schema

`PRAGMA user_version` `0` applies the v1 schema and becomes `1`. Version `1`
opens as-is. Any other version fails closed (`sqlite_schema_unsupported`).
The v1 migration is one-way. There is no reversible migrator in this crate.

## Backup

Before any later migrator or file-format change, copy the live database and
its sidecar files together while the writer is quiet or stopped:

- `*.sqlite` (or the configured path)
- `*-wal`
- `*-shm`

Restoring a subset of those files can yield a torn journal. Encryption and
SQLCipher remain a deployer concern.

## Exclusions

No PostgreSQL, distributed locking, journal pruning, or default binding
wiring. Snapshots remain a disposable replay cache.
