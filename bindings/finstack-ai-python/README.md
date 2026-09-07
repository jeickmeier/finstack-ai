# finstack-ai Python bindings

Typed PyO3 facade over the Rust-owned `finstack-ai` engine. Async APIs are
primary. Dropping a `Run` detaches observation; it does not cancel durable
execution.

Workspace version is **2.0.0**. The package is not on PyPI. Consume a
staged wheel or an editable checkout.

See the [capability matrix](../../docs/capabilities.md) and
[staged 2.0 migration guide](../../docs/migration-2.0.md).

## Quick start

```bash
uv run --isolated --no-project --with-editable bindings/finstack-ai-python \
  python examples/python-minimal/python-callback/main.py
```

Callbacks are trusted in-process code and are not isolated.

The curated wheel links the Rust-backed OpenAI Responses, Anthropic Messages,
Gemini, OpenRouter Responses, and native Ollama paths into the same extension module.
`linked_providers()` reports `("openai", "anthropic", "gemini", "ollama", "openrouter")`.
`Agent.openai()`, `Agent.anthropic()`, `Agent.gemini()`, `Agent.ollama()`, and
`Agent.openrouter()` construct those T1 clients explicitly and accept the
same keyword-only T2 ports as `Agent.from_python` (`toolsets`,
`context_providers`, `middleware`, `observers`, `output_type`). `openai` and
`openrouter` take required keyword-only `api_key` as Bearer auth and
optional `reasoning_effort`; `openrouter` also accepts optional `referer`
and `title` attribution headers and always targets
`https://openrouter.ai/api/v1/responses`, while `openai` always targets
official OpenAI Responses. Output is capped at 128,000 tokens for `openai`
and `openrouter` and 64,000 tokens for `anthropic`, the current Claude
ceiling, while the linked context window remains 1,050,000 tokens. `ollama`
stays keyless and uses `/api/chat`.
The factories do not read environment variables. Importing `finstack_ai` still
does not create a provider client, initialize Tokio, read credentials, or open
network resources.
`finstack_ai.providers` exposes in-package availability probes and does not
construct provider clients.
Call `await run.cancel()` for explicit cancellation. Classic `abi3` wheels are
not part of the launch strategy.

## Capabilities

`Capability(id, description, instructions, activation=...)` is declarative.
`always` enters every resolved plan. `application` enters only when its ID is
in `active_capabilities`. `model` entries appear in the compact catalog;
`Agent.start` / `run` select one with the optional `capability` keyword.
`None` runs this agent. A missing catalog id fails closed. User-input word
overlap does not select a capability. `disabled` cannot activate.

Activation never broadens permissions. `RunResult.trace` exposes stable
record-kind names; `active_capabilities` exposes the committed set.

## Trusted Python callbacks

`PythonModel`, `PythonToolset`, `PythonContextProvider`, `PythonMiddleware`,
and `PythonObserver` run inside the application process and inherit its memory
and authority. Only register trusted callbacks; isolate untrusted extensions
through the plugin host. Model, tool, context, and middleware callbacks receive
one immutable `CallbackContext` plus one normalized request mapping. Observer
callbacks receive a logical event batch. There is no per-token middleware hook.

Both sync and async callbacks are accepted. Sync callbacks run through a Python
worker thread so they do not block the callback event loop. Async callbacks and
their `context.wait_cancelled()` waits run on a binding-owned loop. Callback
metadata, coroutine classification, tool schemas, and descriptors are cached at
registration. Application-owned callback state must therefore provide its own
synchronization when an agent is shared across threads.

Each invocation has a finite timeout. Rust cancellation is forwarded
cooperatively and late callback results are discarded. `CallbackContext` is
valid only during its invocation; retaining and reading it after settlement
raises `RuntimeError` with code `python_callback_context_settled`. Do not call
blocking `Agent` or `Run` operations recursively from a callback. Schedule
independent work and return the normalized callback result instead.

`journal_known_answer()` returns payload digest, envelope checksum, and
canonical-CBOR hex from the one Rust engine. It does not implement CBOR in
Python.

`normalize_prebeta_shape()` exposes data-only Rust validation for child-lineage,
interaction-resolution, and authenticated external-completion shapes. It does
not route those commands. `Agent.inspect_session(session_id)` returns the same
Rust-owned provisional replay projection exposed by the browser binding.
`Agent.open_session` inspects a journal and does not respawn parked runs;
`Lane.resume(agent)` respawns the parked owner.
`Agent.from_python` and the provider factories with
`sqlite_path=..., sqlite_durability=...` open
`finstack-ai-store-sqlite`. Interaction resolution and live
external-completion routing stay Rust-owned. IndexedDB remains experimental
and non-durable.

## Optional Pydantic adapters

Install Pydantic only when typed Python callbacks need it:

```bash
python -m pip install 'finstack-ai[pydantic]'
```

Top-level `import finstack_ai` does not import Pydantic. `@tool` accepts fully
annotated sync or async functions and caches one validation-mode input adapter
plus one serialization-mode output adapter. `pydantic_toolset()` registers the
derived canonical schemas with Rust; invalid model arguments never enter the
Python function, and invalid Python results re-enter the same Rust output
validation path as native tools.

```python
from pydantic import BaseModel

import finstack_ai


class Answer(BaseModel):
    answer: int


@finstack_ai.tool
def add(left: int, right: int) -> Answer:
    """Add two integers."""
    return Answer(answer=left + right)


tools = finstack_ai.pydantic_toolset(
    add,
    component="python.toolset.math",
    name="math",
)
agent = await finstack_ai.Agent.from_python(
    model,
    [tools],
    output_type=Answer,
)
result = await agent.run("Add 20 and 22")
assert result.output == Answer(answer=42)
```

Generated schemas use Draft 2020-12 and a deliberately small portable subset:
closed objects with all properties required, primitive types, arrays, enums,
constants, `anyOf`, and local `$defs`/`$ref`. Unsupported constraints fail at
registration with the exact keyword and JSON pointer. Call
`decorated_tool.refresh_schema()` before constructing a new toolset only when
annotations have intentionally changed.

## Docs

- [API reference](docs/api-reference.md)
- [0.1.0 → 1.0.0 migration](docs/migration-1.0.0.md)
- [0.0.2 migration notes](docs/migration-0.0.2.md)
- [Python starters](../../examples/python-minimal/)

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[DCO](../../CONTRIBUTING.md). [Maintainers](../../GOVERNANCE.md).

### Persistent stores in provider factories

Every provider factory (`openai`, `openrouter`, `anthropic`, `gemini`, `ollama`,
and `gateway`) accepts the same store keywords as `from_python`:
`sqlite_path`, `sqlite_durability`, `postgres_dsn`, `artifact_path`, and
`artifact_store`. SQLite and Postgres journal options are mutually exclusive.
For example, `await Agent.ollama("http://127.0.0.1:11434", "model",
sqlite_path="assistant.sqlite", artifact_path="./artifacts")` keeps the journal
and attachments available across restarts. Journals default to bounded memory;
SQLite defaults to durable mode. Reopening a journal alone does not provide
background execution or reconstruct process-local artifact bytes.

### Embedded durable hosting

`DurableHost` owns the SQLite journal, worker and interaction stores; supplied
agents provide the application definition, callbacks, credentials and artifact
store. Starting admits work without dispatch. Explicit ticks perform execution:

```python
host = await finstack_ai.DurableHost.open("host.sqlite", {"assistant": agent})
locator = await host.start("assistant", "Review the requested action")
report = await host.tick()
for interaction in host.pending():
    # Obtain an authenticated application decision before calling resolve.
    host.resolve(interaction["interaction_id"], authorized_resolution)
await host.tick()
inspection = await host.inspect(locator)
await host.shutdown()
```

A fresh process opens the same path and registers the same definitions, then
resolves/ticks the existing locator without supplying the original input. Keep
artifacts on durable storage and use a unique worker ID per concurrent host.
Shutdown joins local execution; it cannot undo external work already dispatched.
Check tick failures and inspect the run before deciding how to reconcile it.
Missing descriptors, definition drift, lost artifacts, unavailable context prefixes
and unresolved effects are explicit errors. Rust owns recovery and authorization.
`finstack_ai.durable` provides typed inspection, interaction and report shapes.

### Media toolsets independent of provider factories

Replace `media_tools` and the `openrouter_media_*`, `video_compose_*`, and
`media_pipeline_*` factory keywords with typed objects in `toolsets`:

```python
media = finstack_ai.OpenRouterMediaToolset(media_api_key)
compose = finstack_ai.VideoComposeToolset(
    "/usr/local/bin/ffmpeg",
    "/usr/local/bin/ffprobe",
    "./scratch",
    render_timeout_s=300,
)
pipeline = finstack_ai.MediaPipelineToolset(
    media,
    compose,
    max_scenes=4,
    max_total_video_s=120,
    max_concurrent_jobs=2,
    sqlite_state_path="render-state.sqlite",
)
agent = await finstack_ai.Agent.ollama(
    "http://127.0.0.1:11434",
    "model",
    toolsets=[media, compose, pipeline],
    artifact_path="./artifacts",
)
```

Use `OpenAiMediaToolset(media_api_key)` for OpenAI media generation. Media
credentials are explicit and separate from model-provider credentials. Each
agent construction materializes a supplied object once; pipeline dependencies
and individually exposed tools share the same handles and artifact store.
Objects can be reused when switching model providers. All prior tool identities,
approval requirements, result schemas, and native bounds remain in effect.
The pipeline's SQLite render state and the agent's journal are separate stores.
Browser composition continues to use supported host adapters.

## Evaluation

`finstack_ai.eval` exposes typed `EvalSpec`, `TaskSample`, `SubjectDecl`,
`EvalLimits`, `SubjectBinding`, built-in scorers, and `EvalRunner`. Scheduling,
journal measurement, cancellation, classification, scoring arithmetic and
memory/SQLite persistence are owned by Rust. See
[the offline comparison notebook](../../examples/python-notebooks/12_evaluation.ipynb)
and [the Rust evaluation contract](../../crates/finstack-ai-eval/README.md).

```python
from finstack_ai import eval as ev

spec = ev.EvalSpec(
    "retention",
    [ev.TaskSample("policy", "What is retention?", "seven years")],
    [ev.SubjectDecl("baseline")],
    ["exact_match"],
    repetitions=2,
)
runner = ev.EvalRunner(
    spec,
    ev.SqliteEvalStore("experiment.sqlite"),
    [ev.SubjectBinding("baseline", agent)],
    [ev.ExactMatchScorer()],
)
result = await runner.run()
result.export_jsonl("attempts.jsonl")
result.write_summary("summary.json")
rescored = await runner.rescore()  # no subject preparation or model execution
```

The experiment store uses a dedicated database file, separate from the agent's
journal. Call `runner.cancel()` and await the active operation to observe
settlement. Cancelling a Python await alone detaches from its native owner.
Reopening reconciles admitted work; it never silently repeats unresolved effects.
Judge rescoring may execute new graders and incur separately recorded spend.

Every provider factory and `Agent.from_python` accepts `document_tools=False`
to omit the automatically supplied document toolset. Construct grader agents with
that option and no explicit toolsets or capabilities; `JudgeScorer` rejects tools
by default. Attachment ingestion and artifact ownership remain configured on the
agent. The default `document_tools=True` preserves ordinary document workflows.

Use `await agent.with_limits(RunLimits(...))` to obtain a new resolved composition
with accepted token/tool/cost limits, retaining the same native components,
journal, artifact store and output schema. Existing runs keep their original
limits. Finite evaluation budgets require an accepted compatible `CostLimit`;
limits are admission thresholds and concurrent work can overspend them. Trusted
`PythonModel` callbacks may return `ModelOutput.usage` using the canonical `Usage`
and `CostAmount` shapes. Omitted measurements remain unknown.

Custom preparation callbacks receive `PreparationContext` and return bounded
`PreparedRequest` changes. They cannot replace execution authority, the agent,
journal or attachments and must not dispatch work. `PythonScorer` callbacks
receive optional failed-subject output and return integer `Score` values with
the bound identity/version. Async callbacks are cancelled on timeout/drop;
synchronous callbacks run in the blocking pool and their late results are
ignored, since Python threads cannot be forcibly stopped. Exceptions are
classified with stable codes and their private text is not persisted.

Reports return decimal-string statistics and exact cost totals, with scorer
versions, unknown units and missing coverage kept separate. JSONL excludes
input/target bodies, transcripts, arbitrary score metadata and explanations.
TypeScript execution remains deferred; exported JSONL/summary files are the
available integration. Python's `Agent.with_limits` and `document_tools` factory
control are documented native conveniences; the browser keeps its existing
host-configured composition surface (bindings owner; browser convenience parity
is deferred beyond this program).

## Unified search and evaluation

`finstack_ai.search` provides typed native memory, document, journal and graph
sources, query strategies, source outcomes and evidence. Lexical search is the
default; provide an embedder for semantic retrieval and a vocabulary for graphs.
Use the [knowledge composition](../../apps/finstack-knowledge/README.md) for the
complete application, retaining its `.agent` and explicit `.maintain()` lifecycle.
The standalone [search guide](../../extensions/search/README.md) describes source
binding, indexing effects, authorized journal backfill and bounded reconstruction.

`finstack_ai.eval` exposes Rust-owned experiments, subjects, scorers, persistence,
reports and async run/resume/rescore. Start with the tested
[comparison notebook](../../examples/python-notebooks/12_evaluation.ipynb) and
[evaluation contract guide](../../crates/finstack-ai-eval/README.md). Native search
and evaluation are not browser implementations; TypeScript can consume exports.
