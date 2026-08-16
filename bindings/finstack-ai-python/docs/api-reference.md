# Python API reference

The `finstack_ai` package is a typed facade over the Rust-owned semantic engine.
Async APIs are primary. Dropping a handle detaches observation and does not
cancel a run. Hover docs live on the `_finstack_ai.pyi` stub; this page is the
narrative index.

Workspace version is **1.0.0**. The package is not on PyPI.

## Agent composition

- `Agent.openai_compatible(base_url, model, instruction=None, capabilities=None,
  active_capabilities=None)` builds the curated Rust-backed Chat Completions
  provider. Import still constructs no HTTP client.
- `Agent.anthropic(base_url, model, api_key=None, instruction=None,
  capabilities=None, active_capabilities=None)` builds the curated Anthropic
  Messages leaf. An `api_key` requires HTTPS; keyless HTTP loopback is allowed.
- `Agent.ollama(base_url, model, instruction=None, capabilities=None,
  active_capabilities=None)` builds the keyless Ollama/local path of the
  OpenAI-compatible crate. It does not change `Agent.openai_compatible`.
- `Agent.from_python(model, toolsets=None, instruction=None, output_type=None,
  capabilities=None, active_capabilities=None, context_providers=None,
  middleware=None, observers=None)` accepts trusted coarse callback adapters.
  `output_type` lazily requires the Pydantic extra.
- `Agent.start(..., capability=None) -> Run` starts one bounded run and returns
  immediately.
- `await Agent.run(..., capability=None) -> RunResult` waits for the committed
  terminal result.
- `Agent.capability_catalog()` returns only compact `model` entries.
- `Agent.compact_capability_catalog()` renders `id: description` lines under the
  shared 8 KiB registration ceiling.
- `await Agent.create_session(tenant_scope="default")` bootstraps a `main` lane.
- `await Agent.open_session(session_id, tenant_scope)` rebuilds a handle from
  the journal and does not respawn parked runs.

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

See [troubleshooting](../../../docs/site/troubleshooting.md).
