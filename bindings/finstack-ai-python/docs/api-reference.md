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
  and `session` are immutable snapshots.

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

See troubleshooting.
