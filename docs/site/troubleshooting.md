# Troubleshooting

Workspace version is **1.0.0**. Local tag `v1.0.0` exists. The last pushed
GitHub tag is `v0.1.0`. crates.io / PyPI / npm packages are not published.

Stable `code` strings below are the values callers should match. Display
messages may differ by language.

## `agent_run_invalid_configuration` / `ConfigurationError`

Typical causes:

- Empty, NUL-bearing, or oversized run input
- Zero timeout or `max_cycles`
- Unknown `capability` id (fail closed; no token-overlap fallback)
- Component ref without an exact version
- Invalid structured-output schema

Fix the request or catalog and retry. This class is not retryable as a
transient runtime fault.

## `python_callback_context_settled` / `RuntimeError`

`CallbackContext` is valid only during its invocation. Do not retain it
and read it after the callback returns. Copy needed fields with
`to_dict()` before returning.

## Unknown model capability

`Agent.start` / `run` with `capability="…"` requires that id in the
model-activation catalog. `None` keeps the agent that was called. List
ids with `capability_catalog()` / `Agent::capability_catalog`.

## Journal cannot be opened / `sqlite_schema_unsupported`

`Session::open` / `Agent.open_session` rebuilds a handle; it does not
continue a parked run. SQLite user_version must be `0` or `1`. Any other
version fails closed. See [durability](durability.md) and the SQLite
crate README.

## Plugin lock deny / signature failure

Isolated Wasmtime guests are deny-by-default. Missing publisher roots,
an unsigned component, or a lock digest mismatch fail closed. See
[plugins](plugin.md) and [trust levels](security-trust-levels.md).

## `import finstack_ai` appears to do nothing

That is expected. Import does not create a provider, start Tokio, read
credentials, or open sockets. Call `Agent.openai_compatible`,
`Agent.anthropic`, `Agent.ollama`, or `Agent.from_python` explicitly.

## IndexedDB did not resume my run

IndexedDB on `@finstack/ai/adapters/indexeddb` is experimental. Reload
restore is inspect, not continue-the-run. It is not JournalStore v1 and
is not crash-durable.

## Dropping a `Run` did not cancel it

Dropping a handle detaches observation. Call `await run.cancel()`
(Python) or `AgentRun::cancel` (Rust) for durable cancellation.

## License

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[Contributing](../../CONTRIBUTING.md). [Security](../../SECURITY.md).
