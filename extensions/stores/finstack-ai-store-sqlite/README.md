# finstack-ai-store-sqlite

Trusted native SQLite [`JournalStore`](https://docs.rs/finstack-ai-runtime) leaf.
This crate is opt-in. Do not treat it as the Agent, Python, or WASM default.

Records are append-only and authoritative. Optional `JournalStore::prune`
deletes snapshot-covered prefix records while keeping the snapshot-boundary
record, outstanding tail, and settlement indexes. Snapshots stay a disposable
cache, not a second semantic model.

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

## Backup and leaf commands

`sqlite_ops` is a leaf binary over the existing store APIs. It is not a
seventh port.

```text
sqlite_ops backup <src-db> <dest-db>
sqlite_ops restore <src-db> <dest-db>
sqlite_ops export <db> <session>
sqlite_ops import <db> <session>
sqlite_ops diagnose <db> <session>
sqlite_ops migrate <db>
```

`backup` / `restore` copy the live database and its sidecar files together
while the writer is quiet or stopped:

- `*.sqlite` (or the configured path)
- `*-wal`
- `*-shm`

`restore` refuses a destination that already exists and refuses orphan
destination sidecars. Restoring a subset of those files can yield a torn
journal. Encryption and SQLCipher remain a deployer concern.

`export` / `import` use the protocol diagnostic JSONL projection.
`diagnose` prints head sequence, checksum, snapshot validity, outstanding
work, child mappings, and a corruption class. `migrate` applies user_version
`0`/`1` → `1` and fails closed on any other version.

`sqlite_fault_helper` stays a test helper, not a product command.

## Exclusions

No PostgreSQL, distributed locking, or default binding wiring. Snapshots
remain a disposable replay cache. IndexedDB is not this crate.
