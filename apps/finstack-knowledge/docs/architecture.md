# finstack-ai architecture

finstack-ai is a deterministic agent microkernel with a layered workspace:

- **Kernel** (`crates/finstack-ai-kernel`) — deterministic semantic state:
  records, events (`RunEventKind`), effects, identities, digests. Nothing
  in the kernel performs I/O.
- **Runtime** (`crates/finstack-ai-runtime`) — the six ports (model,
  tool, context, middleware, observer, journal) and effect execution.
  Ports are object-safe traits with `PortObject`/`PortFuture` bounds so
  the same contracts compile natively and on browser WASM.
- **SDK** (`crates/finstack-ai`) — composition: `Agent::builder` wires a
  model provider, journal store, toolsets, context providers, middleware,
  and observers into a resolved, locked agent. `Agent::run` executes one
  request; `Lane::run` executes on a durable session lane.
- **Protocol and bindings** — outward-facing codecs plus the Python
  (`finstack_ai`) and TypeScript/WASM (`@finstack/ai`) bindings.
- **Extensions** (`extensions/`) — trusted native implementations:
  stores (sqlite, postgres, memory), providers (ollama, openai,
  anthropic, openrouter, gemini), toolsets (document, fetch, skills,
  calculator, …), context providers (repository instructions, memory),
  middleware (instructions, document-ingest, compaction, …), observers
  (log, metrics, …).
- **Apps** (`apps/`) — end-user products composed from released
  components. This knowledge agent is one: it implements no ports and
  only composes existing extensions.

Every run appends authoritative committed records to the journal. Live event
streams also contain transient progress, including token deltas, which is not
replayed by `JournalStore::scan`. Journal search cites committed entry/record IDs
and reports incomplete historical coverage when retained history is insufficient.
