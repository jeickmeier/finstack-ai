# Public preview roadmap

In-repo issue roadmap for unpublished `0.1.0`. Opening GitHub issues is an
external action and is not claimed here. Phase 9 themes are links to the
Implementation Plan, not admitted work.

## Known preview limitations

- Experimental WIT `@0.0.4` worlds. `@1.0.0` generation stays blocked.
- Experimental IndexedDB adapter. It is not a durable store.
- Session open is inspect-not-continue.
- Delivery is at-least-once (ADR-013). Do not claim exactly-once.
- No marketplace, native dylib ABI, or commercial support portal.
- No 1.0 compatibility freeze and no independent leaf versioning.
- Process protocol is handshake-only; session vocabulary is later.
- Live provider smokes stay `#[ignore]` unless a later sentence names
  network use.
- Registries, `git tag v0.1.0`, and `G7-D-*` wait on separately named
  owner actions.

## Deferred backlog rows

Rows already marked `defer past preview` in
[`public-api-change-backlog.md`](public-api-change-backlog.md):

- Anthropic JavaScript adapter
- WIT `@1.0.0` world generation
- Process session vocabulary
- IndexedDB durability
- SharedArrayBuffer / cross-origin isolation
- Named Criterion performance budgets (PR-063)
- Marketplace, PostgreSQL, exactly-once delivery

## Phase 9 themes (not admitted)

These stay `Todo` until Phase 9 entrance is `Passed` (2/2) and G7 is
`Passed` by a named decision. Do not infer admission from this file.

| Theme | Plan entry |
| --- | --- |
| Contract freeze and independent leaf versioning | [PR-062](../planning/04-finstack-ai-implementation-plan.md) |
| Performance budgets and release engineering | [PR-063](../planning/04-finstack-ai-implementation-plan.md) |
| Migration tooling | [PR-064](../planning/04-finstack-ai-implementation-plan.md) |
| Ecosystem conformance | [PR-065](../planning/04-finstack-ai-implementation-plan.md) |
| 1.0 closeout | [PR-066](../planning/04-finstack-ai-implementation-plan.md) |
