# Examples

Public-API examples for finstack-ai bindings. Workspace version is
**1.0.0** unpublished (last public tag `v0.1.0`; registries unpublished).

- [`rust-minimal/`](rust-minimal/) — T1 native binaries (`minimal`, `coding`,
  `service`, `diagnostic`).
- [`python-minimal/`](python-minimal/) — rust-backed (T1), callback (T2),
  and service (T2) starters pinned to `finstack-ai==1.0.0`.
- [`python-notebooks/`](python-notebooks/) — the learning notebooks.
- [`browser-minimal/`](browser-minimal/) — experimental same-origin IndexedDB
  inspect demo (T2 host / T5 content). Not crash-durable.
- [`ts-alpha-install/`](ts-alpha-install/) — TypeScript consumer that
  typechecks against a staged `@finstack/ai` tarball.
- [`durable-interaction/`](durable-interaction/) — full UC-05 story on a
  SQLite journal: a run parks on a tool-approval interaction, the worker
  process dies, a fresh host rebuilds the journal, worker, and HITL router
  stores from disk paths alone, an operator lists and authorizes the
  interaction through `HitlRouter`, and the same worker ticks the run to
  completion. Not a default dependency of `finstack-ai-native-examples`.
