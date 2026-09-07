# Staged 2.0 composition migration

These APIs are staged source changes. Migrate callers directly; old and new
constructor forms are not maintained in parallel. Existing journal/protocol
meanings and commit-before-effect ordering are unchanged. New host recovery,
evaluation and search stores have explicit schema versions and fail closed on
unsupported schemas.

## Register components from their descriptors

Pass the handle alone to `NativeAgentBuilder::context_provider`, `middleware`
and `observer`. Context and middleware descriptors supply their invocation
component/version; observers supply their component reference. The capability
context-provider and middleware methods follow the same rule.

```rust,ignore
let builder = Agent::builder()
    .context_provider(context)
    .middleware(instructions)
    .observer(logger);
```

Remove the copied `ComponentRef` argument at these call sites. Models, toolsets
and journal stores still require explicit identities because their descriptors
do not provide an equivalent versioned component reference. Exact-lock and
configuration-digest checks remain enforced. A conflicting registration reports
the component and mismatched version.

Each linked provider constructor is enabled by its own provider feature. The
aggregate `linked-providers` feature is optional. `LinkedCommon::journal_store`
accepts an explicit `(ComponentRef, Arc<dyn JournalStore>)`; ports carry the
explicit artifact store. Rust's ordinary default remains a native runtime with
provider batteries opt-in.

Python provider factories and `Agent.from_python` accept the same explicit
journal/artifact keywords: `sqlite_path`, `sqlite_durability`, `postgres_dsn`,
`artifact_path`, and `artifact_store`. SQLite and PostgreSQL selections are
mutually exclusive, as are an artifact path and an existing artifact handle.
Reopen a session and use its lane to continue history; `Agent.run`/`start` admits
a new session.

## Compose media independently of the model

Remove media-generation, ffmpeg and MoviePlan options from `LinkedCommon` and
Python provider factory arguments. Build dedicated toolsets and pass them through
the existing toolset collection. Python uses `OpenRouterMediaToolset`,
`VideoComposeToolset` and `MediaPipelineToolset`; the pipeline takes the same
configured media and video objects. List those tools too when the model should
call them independently. All participating tools share the receiving agent's
artifact store. Model credentials and media credentials remain separate explicit
configuration. Tool identity, approval and result contracts are retained.

## Replace manual durable stage driving

Enable Rust's `durable-host` feature or use Python's `DurableHost`. Open the
embedded host, build/register workflow definitions against its journal, then
`start`, `tick`, `inspect`, resolve authenticated pending interactions and
`shutdown`. Starting a run records a versioned recovery descriptor before dispatch;
a fresh process registers the same definition and resumes the original run.

Do not manually submit reducer stages or re-accept a recovered run. The shared
SDK driver reconstructs committed authority, messages, limits and effects.
Configuration drift, missing artifacts and unresolved external effects are
explicit failures. Lease loss stops and joins local driving; it cannot undo an
external operation. The [approval recipe](application-recipes.md) executes a real
process restart and checks delivery, completion and cleanup separately.

## Retain the knowledge composition

Rust `build_agent`, `build_agent_with_journal` and `build_agent_with_stores` return
`KnowledgeAgent`. Use `.agent` for SDK execution and `.search` for query sources
and explicit `maintain(limit)` calls. Python's shared notebook composition also
returns `KnowledgeAgent`: use `.agent`, `.search`, and `await .maintain(limit)`.
Maintenance reports counts, remaining work and stable failures. Retain the
composition for the application's lifetime and maintain between settled turns.

`KnowledgeConfig::memory_embedder` and `with_memory_embedder` become `embedder`
and `with_embedder`; the same explicit embedder serves memory and documents.
Use `with_search_sessions` to bind authorized sessions and `with_graph` for an
explicit vocabulary. The local app can enumerate its own SQLite journal; custom
journal-store compositions require explicit session selection. Python exposes
`sqlite_session_ids(path, limit=256)` for an application-owned local file. This
concrete-store convenience adds no discovery method to `JournalStore`. Create the
session before resolving sources when its new records should be indexed live.

Global compositions expose one `search` query tool and one global recall provider.
They disable memory's separate read tool/provider while retaining remember,
correct, forget and capture. Standalone memory composition is unchanged.
`index_document` is an explicit idempotent tool effect. CLI `ingest` requires a
current index receipt before reporting success; a model-written summary alone
is insufficient. The ingest middleware supplies the exact uploaded blob reference
as untrusted citation data. It never writes the document index itself.

## Add evaluation without moving semantics to Python

Use `finstack-ai-eval` / `finstack_ai.eval` for frozen specifications, subject
bindings, built-in or custom scorers, and `run`, `resume`, `rescore`. Evaluation
creates one actual session per attempt and reserves its identity before dispatch.
Unfinished work must reconcile before replacement. Subject failures and scoring
failures remain separate; a failed subject may have no output. Rescoring does not
invoke subject models, though a judge scorer can incur new grader costs.

Reports retain integer score/cost micros and explicit unit/coverage information.
Missing or mixed-unit cost is unknown. Finite spending budgets require compatible
pricing coverage and bound admission, not concurrent in-flight spend. JSONL and
summary numerical strings should be read as integer/decimal values. TypeScript
execution is deferred; export consumption is supported. See the
[evaluation guide](../crates/finstack-ai-eval/README.md) and
[comparison notebook](../examples/python-notebooks/12_evaluation.ipynb).

## Configure finite retrieval limits

Memory stores now default to 10,000 retained records (including tombstones),
16 MiB of inline bodies and 256 MiB of aggregate stored embedding vectors.
Rust `MemoryStoreLimits` adds `max_embedding_bytes`; migrate complete struct
literals or use `..Default::default()`. Replacing a vector credits its previous
bytes atomically; correction, forgetting and expiry sweeps reclaim vectors.
`SearchLimits` separately controls scan admission, results, previews, vector bytes
and graph traversal. Semantic memory search/rebuild conservatively admits the
whole live scope at the configured embedding dimensionality before embedding.
The [scale report](search-scale.md) records the tested envelope and limitations.

## Validate the migrated application

Run `mise run test-examples`, `test-linked-features`, `test-durable-host`,
`test-recipes-rust`, `test-eval`, and `test-search` for the affected native paths.
Build the Python extension before `test-recipes-python` and `test-eval-python`.
`test-python` includes runtime, typing-sensitive fixtures and knowledge
composition tests. `bench-search` records the supported local corpus envelope.
Regenerate public inventories using `mise run write-public-api`; WIT and WASM
artifacts use their existing generators. Finish with `mise run ci-all`.
