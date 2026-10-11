# Python SQLite compatibility fixtures

Binding fixtures for migration and settlement-idempotency behavior. Rust
remains the semantic owner. Python opens the store through
`Agent.from_python(..., sqlite_path=..., sqlite_durability=...)`.

- `v1/migration/valid--open-user-version-1.json` — empty `user_version` 0
  file opens and lands on schema version 2.
- `v1/migration/valid--open-user-version-1-upgrades-to-2.json` — a
  `user_version` 1 file is migrated forward to 2 on open (session counters
  backfilled).
- `v1/migration/invalid--unsupported-user-version.json` — `user_version` 99
  fails closed.
- `v1/settlement/valid--completed-run-restart.json` — a completed run
  reopens without an active run; duplicate interaction resolution stays
  idempotent.
