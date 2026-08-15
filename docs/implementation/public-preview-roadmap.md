# Public preview roadmap

In-repo issue roadmap for tagged `0.1.0` (`v0.1.0`). Opening GitHub
issues is an external action and is not claimed here. Phase 9 themes
are links to the Implementation Plan, not admitted work.

## Known preview limitations

- Experimental WIT `@0.0.4` worlds remain loadable. `@1.0.0` worlds are
  generated beside them and are the frozen exact-world line.
- Experimental IndexedDB adapter. It is not a durable store.
- Session open is inspect-not-continue.
- Delivery is at-least-once (ADR-013). Do not claim exactly-once.
- No marketplace, native dylib ABI, or commercial support portal.
- 1.0 compatibility policy is approved by
  `COMP-1.0-D-contract-freeze-00b78667ecc4`. Independent leaf
  versioning stays lockstep; no coupling-harm evidence. This is not
  `G8-D-*`.
- Process protocol is handshake-only; session vocabulary is later.
- Live provider smokes stay `#[ignore]` unless a later sentence names
  network use.
- G7 passed via `G7-D-public-preview-f7c7e70b9e04`. Tag `v0.1.0` is
  cut. crates.io / PyPI / npm remain unpublished until owner
  registry credentials are supplied.

## Deferred backlog rows

Rows already marked `defer past preview` in
[`public-api-change-backlog.md`](public-api-change-backlog.md):

- Anthropic JavaScript adapter
- WIT `@1.0.0` guest retarget of published `@0.x` components
- Process session vocabulary
- IndexedDB durability
- SharedArrayBuffer / cross-origin isolation
- Named Criterion performance budgets (PR-063)
- Marketplace, PostgreSQL, exactly-once delivery

## Phase 9 themes (not admitted)

Phase 9 entrance is `Passed` (2/2) under PLAN-0.19. G7 is `Passed`
via `G7-D-public-preview-f7c7e70b9e04`. PR-062 is `Done` via
`COMP-1.0-D-contract-freeze-00b78667ecc4`. Do not infer PR-063+ or
`G8-D-*` from this file.

| Theme | Plan entry |
| --- | --- |
| Contract freeze and independent leaf versioning | [PR-062](../planning/04-finstack-ai-implementation-plan.md) |
| Performance budgets and release engineering | [PR-063](../planning/04-finstack-ai-implementation-plan.md) |
| Migration tooling | [PR-064](../planning/04-finstack-ai-implementation-plan.md) |
| Ecosystem conformance | [PR-065](../planning/04-finstack-ai-implementation-plan.md) |
| 1.0 closeout | [PR-066](../planning/04-finstack-ai-implementation-plan.md) |
