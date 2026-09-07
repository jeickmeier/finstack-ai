# finstack-ai-knowledge

Persistent knowledge composition for native Rust and Python: ingest artifacts,
remember facts, and query memory, documents, committed journals and optional graphs
through one scoped search tool. The CLI and Python helper can share the same
application-owned journal/artifact directory and tenant. The browser knowledge
example is a host-adapter subset; it does not contain native SQLite search or
embedded durable recovery. The [capability matrix](../../docs/capabilities.md)
is the canonical platform comparison.

## Run the offline recipe

From the repository root:

```sh
cargo run -p finstack-ai-knowledge --example persistent --offline --locked
uv run python examples/python-notebooks/persistent_assistant.py
```

Both exercise the real composition, disk reopening, durable attachments,
structured results, events, explicit cancellation and index maintenance. They use
temporary storage and offline model fixtures. See
[application recipes](../../docs/application-recipes.md) for deployment wiring.

## Use the composition

`build_agent`, `build_agent_with_journal` and `build_agent_with_stores` return
`KnowledgeAgent { agent, search, initial_maintenance }`. Run the SDK through
`.agent`; query `.search.engine`; call `.search.maintain(limit).await` between
settled turns. Limits are 1..=256. The initial bounded pass runs during construction.
Every maintenance report exposes failures and remaining work. Keep the composition
alive so observer hints and maintenance cursors remain available.

The Python helper in `examples/python-notebooks/_knowledge.py` returns the
corresponding `KnowledgeAgent`. Use `.agent` for SDK calls and
`await composition.maintain(limit)` for host maintenance. A new session must exist
before constructing sources if its records should join live journal indexing.
The persistent recipe demonstrates this order.

Default recall is lexical across memory, documents and the authorized journal
catalog. Global recall replaces standalone memory recall; memory write/manage
and capture remain enabled. `index_document` is a separate committed idempotent
effect, and `ingest` checks its current receipt before returning success. Artifacts
remain authoritative. Derived indexes never extend source retention or authority.

## Select sessions, embeddings and graphs

`KnowledgeConfig::with_search_sessions` binds explicit authorized session IDs.
The local SQLite application can enumerate its own file, with an explicit-selection
error above 256 sessions. Compositions over arbitrary journal ports use the supplied
session set and never discover other sessions. Python uses the same native SQLite
catalog reader through `finstack_ai.sqlite_session_ids`.

`with_embedder` enables semantic memory and document retrieval together. Its
endpoint and model are independent of the chat provider. `with_graph` supplies a
validated deterministic entity/edge vocabulary; graph extraction and two-hop,
100-entity expansion are opt-in. Python accepts `embedder=` and
`graph_vocabulary=` on the shared helper. Bounds, citations and incomplete source
coverage remain visible in query results.

The CLI accepts `--search-session` (repeatable), `--embedding-model` together with
`--embedding-dimensions`, optional `--embedding-url`, and `--graph-vocabulary`
pointing to a bounded JSON vocabulary. It maintains indexes at startup and after
each settled ask/ingest/REPL turn. Maintenance diagnostics use stderr alongside
`--json`; stdout retains the NDJSON run-event format. Live-provider commands need
the configured provider; the offline recipes need no credentials.

See [search contracts](../../extensions/search/README.md),
[measured scale](../../docs/search-scale.md), and
[migration guidance](../../docs/migration-2.0.md). Run `mise run test-search` for
native integration and `mise run test-python` for the actual Python composition.

## Authority and deployment

The CLI is local single-user: tenant `local`, OS-user principal, explicit local
policy/decision labels. The Python helper's tenant must match its session. Native
extensions and callbacks are trusted application code; retrieval content is data,
never instructions. Fetch remains deny-by-default unless the application supplies
an allowlist. Deploy with persistent journal/artifact directories, explicit source
retention and bounded maintenance scheduling. The library's remote server remains
a reference implementation.
