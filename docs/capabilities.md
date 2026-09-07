# Choose an application path

Rust owns execution and retrieval semantics. Python exposes native handles and
trusted callbacks. Browser execution uses the same portable kernel and SDK with
host adapters; native stores and services require a native process.

| Task | Rust entry point | Python entry point | Browser / TypeScript | Persistence | Isolation |
|---|---|---|---|---|---|
| Run a model with tools | `Agent::builder` or individually enabled `linked_*` provider | `Agent.from_python` or provider factories | `@finstack/ai` host adapters | Select a journal explicitly; in-memory is process-local | Native tools and host callbacks are trusted |
| Continue a conversation | `Session`, `Lane::run` | `create_session`, `open_session`, `session.lane`, `lane.run` | Host-driven session/run handles | Native SQLite/PostgreSQL journals; browser IndexedDB is experimental inspection storage | Store selection does not isolate code |
| Stream events and cancel | `AgentRun` event batches, `cancel`, `result` | `Run.events`, `cancel`, `result` | Worker transport and explicit cancellation | Committed events survive with the journal; transient tokens do not | One consumer per event stream |
| Resume an approval after process loss | `DurableHost`, feature `durable-host` | `DurableHost` | Native durable hosting is deferred | Embedded SQLite host/worker state plus durable artifact store | Decisions bind committed principal, tenant, target and evidence |
| Supervise specialists | SDK child-run composition and supervisor recipe | Child-run configuration and supervisor recipe | Supported host-driven child composition; no embedded SQLite worker | Child lineage is committed; recipe host orchestration is in-process | Child capabilities and depth remain bounded |
| Attach documents and index them | Artifact store, ingest middleware, `DocumentIndexToolset` | Attachments, `DocumentSearchSource.toolset()` | Native parsers/indexing deferred; host-supplied content remains possible | Artifacts are authoritative; SQLite document index is rebuildable | Document content is untrusted evidence; no OCR |
| Remember and recall | `finstack-ai-memory` | `MemoryExtension` | Native memory batteries deferred | In-process or SQLite memory; correction/forget invalidates derived vectors | Scope is bound by the host |
| Search memory, documents, journals and graphs | `SearchSource`, `SearchEngine` | `finstack_ai.search` | Native search execution deferred; consume serialized results | Separate derived SQLite indexes; exact source references | Sources are trusted adapters; retrieved content is untrusted |
| Add semantic search | Explicit `TextEmbedder` | Explicit `HashEmbedder` or `OllamaEmbedder` | Native embedding/index execution deferred | Exact vector search, bounded bytes and results | Remote embedding is explicit host egress; graph expansion is separately opt-in |
| Evaluate agents | `finstack-ai-eval`, `EvalRunner` | `finstack_ai.eval` | Consume JSONL/report exports; execution deferred | Memory or separate SQLite evaluation store; journals retain execution evidence | Subjects/scorers are trusted; judge input is data and tools are off by default |
| Generate or compose media | Dedicated media/video toolsets | `OpenRouterMediaToolset`, `VideoComposeToolset`, `MediaPipelineToolset` | Existing host-adapter model | Shared explicit artifact store | Native subprocess/network capabilities remain explicit |
| Run an untrusted extension | Wasmtime plugin host and WIT guest SDK | Compose through a native host integration | JavaScript host adapters are trusted, not a sandbox | Host supplies storage capabilities | WIT/Wasmtime is the isolation boundary |
| Expose a network service | Reference server plus application deployment | Native application integration | Authenticated application/proxy endpoint | Application-owned | Reference server is not a production deployment control plane |

Start with the [offline recipes](application-recipes.md),
[Python guide](../bindings/finstack-ai-python/README.md), or
[Rust guide](../crates/finstack-ai/README.md). Unified search uses the existing
[knowledge application](../apps/finstack-knowledge/README.md).

Native search and evaluation execution in browser/WASM remain explicit deferrals
owned by the search/evaluation and WASM maintainers. A future implementation must
supply host storage/execution adapters and the same scope, lifecycle, coverage and
resource conformance tests. No unused remote/vector backend plugins or ANN index
are shipped. See [search contracts](../extensions/search/README.md) and the
[measured operating envelope](search-scale.md) before selecting a corpus size.
