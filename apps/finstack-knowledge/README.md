# finstack-ai-knowledge

The first first-party product on the engine: a **knowledge agent** — ingest
documents, remember facts across sessions, answer questions with retrieval
and citations — delivered through three surfaces over one definition:

1. **CLI** (`finstack-know`, Rust) — the operator path.
2. **Python notebooks** (`k01`–`k05` in `examples/python-notebooks/`) — the
   analyst path.
3. **TypeScript browser example** (`examples/browser-knowledge/`) — the
   embedded path.

All three surfaces consume the same `RunEventKind` stream, the CLI and
notebooks share the same journaled sqlite session, and all three run the
same agent definition (composed per-surface; drift is held by the shared
golden-questions fixture in `fixtures/golden.json`).

This crate is the definition: `KnowledgeConfig`, `build_agent`,
`security`, embedded self-docs, and the golden fixture loader. The CLI is
one consumer of it (`[[bin]] finstack-know`, added by Plan B).

## Parity matrix

Asymmetries are documented boundaries, not bugs.

| | CLI (Rust) | Python notebooks | TypeScript browser |
|---|---|---|---|
| Journal | sqlite | sqlite (same file as CLI) | IndexedDB |
| Providers | linked native (ollama default; anthropic/openai/openrouter by config) | linked native | host adapter (scripted default; live optional) |
| Document ingest | `tools-document` + `middleware-document-ingest` | same | TS retrieval toolset over a bundled corpus (File API upload not in v1) |
| Memory | full extension | full extension | not wired in v1 (host-adapter seam available) |
| Composition parity | reference | full extension parity (zero divergences) | host-adapter subset |
| Cross-surface session | opens notebook sessions | opens CLI sessions | inspect/export only (browser storage is origin-local) |
| Confinement / net-guard | available | available | n/a (browser sandbox) |
| Golden questions | yes (CI, offline) | yes (CI, offline) | yes (CI, scripted) |

## Composition checklist (option a — per-surface composition)

Every surface composes exactly: the configured model provider; a sqlite
journal (`<data_dir>/journal.sqlite3`; IndexedDB in the browser); two
repository-instruction context roots (self-docs, project) plus the memory
recall provider; instructions, document-ingest, and compaction
(`sliding_window`) middleware; document, memory, and skills toolsets
(fetch only when the allowlist is non-empty); log and memory-capture
observers. A surface that cannot compose one of these documents the gap in
this README's parity matrix and files the binding issue.

## Security posture

Local single-user: tenant `local`, principal from the OS user, auth method
`local`, explicit policy/decision labels. Fetch is deny-by-default via the
allowlist. Filesystem tools are not part of the v1 composition (ingest
takes explicit paths). Confinement/net-guard integration is available in
the engine but not wired in v1.
