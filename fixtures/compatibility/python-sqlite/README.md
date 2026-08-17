# Python SQLite compatibility fixtures

PR-078 binding fixtures for the PR-048 migration and settlement-idempotency
subset. Rust remains the semantic owner. Python opens the store through
`Agent.from_python(..., sqlite_path=..., sqlite_durability=...)`.

- `v1/migration/valid--open-user-version-1.json` — empty `user_version` 0
  file opens and becomes schema version 1.
- `v1/migration/invalid--unsupported-user-version.json` — `user_version` 99
  fails closed.
- `v1/settlement/valid--completed-run-restart.json` — a completed run
  reopens without an active run; duplicate interaction resolution stays
  idempotent.
