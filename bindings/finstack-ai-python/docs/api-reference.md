# Python API reference

The `finstack_ai` package is a typed facade over the Rust-owned semantic engine.
Async APIs are primary. Dropping a handle detaches observation and does not
cancel a run. Hover docs live on the `_finstack_ai.pyi` stub; this page is the
narrative index.

Workspace version is **2.0.0**. The package is not on PyPI.

## Agent composition

- `Agent.openai(model, instruction=None, capabilities=None,
  active_capabilities=None, *, api_key, reasoning_effort=None,
  reasoning_summary=None,
  toolsets=None, context_providers=None, middleware=None, observers=None,
  output_type=None)` builds the official OpenAI Responses provider (T1).
  Keyword-only ports are the same T2 callbacks as `from_python`. `api_key` is
  required keyword-only Bearer auth. `reasoning_effort` is `none` / `minimal`
  / `low` / `medium` / `high` / `xhigh` / `max`; `reasoning_summary` is
  `auto` / `concise` / `detailed`. Requests use `store: false`. The binding
  does not read environment variables or expose alternate gateways.
- `Agent.anthropic(base_url, model, api_key=None, instruction=None,
  capabilities=None, active_capabilities=None, *, toolsets=None,
  context_providers=None, middleware=None, observers=None, output_type=None)`
  builds the curated Anthropic Messages leaf (T1) plus the same keyword-only
  T2 ports. An `api_key` remains positional and requires HTTPS; keyless HTTP
  loopback is allowed. Requests ask for at most 64,000 output tokens, the
  current Claude ceiling.
- `Agent.ollama(base_url, model, instruction=None, capabilities=None,
  active_capabilities=None, *, toolsets=None, context_providers=None,
  middleware=None, observers=None, output_type=None)` builds the keyless
  native Ollama `/api/chat` provider plus the same keyword-only T2 ports.
- `Agent.from_python(model, toolsets=None, instruction=None, output_type=None,
  capabilities=None, active_capabilities=None, context_providers=None,
  middleware=None, observers=None, *, sqlite_path=None,
  sqlite_durability=None)` accepts trusted coarse callback adapters.
  `output_type` lazily requires the Pydantic extra. Keyword-only
  `sqlite_path` and `sqlite_durability` open `finstack-ai-store-sqlite`.
  `SqliteDurability.Durable` is WAL plus `synchronous=FULL`.
  `SqliteDurability.Relaxed` is named non-durable. `:memory:` requires
  `Relaxed`.
- `Agent.start(..., capability=None) -> Run` starts one bounded run and returns
  immediately.
- `await Agent.run(..., capability=None) -> RunResult` waits for the committed
  terminal result.
- `Agent.capability_catalog()` returns only compact `model` entries.
- `Agent.compact_capability_catalog()` renders `id: description` lines under the
  shared 8 KiB registration ceiling.
- `await Agent.create_session(tenant_scope="default")` bootstraps a `main` lane.
- `await Agent.open_session(session_id, tenant_scope)` rebuilds a handle from
  the journal and does not respawn parked runs. Call `await lane.resume(agent)`
  to respawn the owner.
- `await Session.lane_by_id(lane_id)` looks up a lane by durable identity.
- `Lane.run(agent, input, ...)` starts a root run on an idle lane.
  `await Lane.suspend()` parks the in-process driver. `await Lane.resume(agent)`
  respawns it. Cancel through the live `Run` handle so cancellation retains
  the initiating principal and authorization evidence.
  `await Lane.append_text(text)` appends a user message and does not start a
  run.

## Capabilities

`Capability(id, description, instructions, activation=...)` is declarative.
Python capabilities contribute instructions; native bundle definitions may also
reference registered toolsets, context providers, and middleware.

- `always` enters every resolved plan.
- `application` enters only when its ID is in `active_capabilities`.
- `model` entries are listed in the compact catalog. Pass `capability=` on
  `start` / `run` to execute that variant. `None` runs this agent. A missing
  catalog id raises `ConfigurationError`. User-input token overlap is not used.
- `disabled` cannot activate.

Activation never broadens permissions. The kernel commits the complete sorted
active set with `always`, `application`, or `model` source. `RunResult.trace`
exposes stable record-kind names for conformance; `active_capabilities` exposes
the committed set. Capability instructions append after the unchanged base
instruction prefix.

## Runs, events, and results

- `Run.result()`, `Run.cancel()`, `Run.events()`, and `Run.close_events()` retain
  Rust-owned lifecycle and batching semantics.
- `EventBatch` is the primary transport unit; expanding to individual `Event`
  values is explicit.
- `RunResult.text`, `output`, `retry_attempts`, `active_capabilities`, `trace`,
  and `locator` are immutable snapshots. `session` is a locator-shaped alias
  retained on the result object; live session operations use `Run.session`.

## Trusted callback ports

`PythonModel`, `PythonToolset`, `PythonContextProvider`, `PythonMiddleware`, and
`PythonObserver` are trusted in-process adapters. Each invocation is coarse,
bounded by timeout, cancellation-aware, and supplied an invocation-scoped
`CallbackContext`. Retained contexts reject access after settlement
(`RuntimeError`, code `python_callback_context_settled`). There is no per-token
callback.

## Pydantic extra

`@tool`, `PydanticTool`, and `pydantic_toolset()` cache TypeAdapters and schemas
at registration. Rust validates the portable Draft 2020-12 subset. Pydantic
converts JSON only at the Python object boundary. See the package README for a
complete example and unsupported-schema behavior.

## Errors and typing

All public imports ship through `finstack_ai.__all__`, `py.typed`, and the
native `_finstack_ai.pyi` facade. Runtime failures derive from `FinstackError`
and expose `code`, `retryable`, and safe locator `context` where available.

| Type | Typical `code` |
| --- | --- |
| `ConfigurationError` | `agent_run_invalid_configuration` |
| `RuntimeError` | runtime / callback-settled failures |
| `CancelledError` | `agent_run_cancelled` |
| `TimeoutError` | `agent_run_timeout` |

## Troubleshooting

### Native module does not import

Run `mise run build-python-dev`, then execute Python through the repository
environment (`uv run python`). The package requires the native
`finstack_ai._finstack_ai` extension; importing the source directory with an
unbuilt or stale extension is unsupported.

### Pydantic adapters are unavailable

Install the optional dependency set used by the workspace. `@tool`,
`pydantic_toolset()`, and `output_type=` import Pydantic lazily and raise a
`TypeError` naming the missing `finstack-ai[pydantic]` extra.

### Dropping a run did not cancel it

This is intentional. Dropping `Run` detaches observation. Call
`await run.cancel()` to commit cancellation, or `await run.close_events()` to
stop only event observation.

### An opened session did not resume work

`Agent.open_session()` is inspect-only. Find the suspended lane and call
`await lane.resume(agent)` explicitly. This prevents journal replay from
silently re-executing effects.

### A callback context raises `python_callback_context_settled`

`CallbackContext` is invocation-scoped. Copy required identity fields with
`to_dict()` before the callback returns; retained contexts deliberately reject
later access.

## Native evaluation and search

`finstack_ai.eval` contains frozen `EvalSpec`/`TaskSample` configuration,
`SubjectBinding`, memory/SQLite stores, typed built-in and Python scorers,
`EvalRunner.run/resume/rescore`, `EvalReport` and threshold gates. Rust owns
reservation, reconciliation, scheduling, measurement and statistics. The
[comparison notebook](../../../examples/python-notebooks/12_evaluation.ipynb)
executes the complete API offline, including failure cases and rescoring.

`finstack_ai.search` contains `SearchScope`, `SearchLimits`, `SearchConfig`,
`SearchRequest`, the four native source classes, `SearchEngine`, and typed
source/evidence/coverage reports. Document indexing runs through the explicit
`DocumentIndexToolset`; journal observers only enqueue hints. Graph and semantic
retrieval require explicit configuration. `sqlite_session_ids(path, limit=256)`
reads IDs from an application-owned SQLite journal without adding discovery to
the journal port. See the [search guide](../../../extensions/search/README.md)
and [knowledge composition](../../../apps/finstack-knowledge/README.md).

The new module wrappers carry typed signatures and IDE documentation; their data
contracts are re-exported through module `__all__`. Browser execution limitations
and staged constructor changes are in the
[capability matrix](../../../docs/capabilities.md) and
[migration guide](../../../docs/migration-2.0.md).
