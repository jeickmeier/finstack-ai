# finstack-ai Python bindings

PR-027 establishes the `finstack_ai` PyO3 extension package, typed facade,
editable development, and per-version wheel pipeline. PR-028 adds immutable
Rust-owned `Agent`, `Run`, `Session`, `RunResult`, `Event`, and `EventBatch`
handles, async result/cancellation, and batch-first event observation.
PR-030 adds trusted coarse Python adapters for Model, Toolset,
ContextProvider, Middleware, and batched Observer ports.

```bash
mise run python-develop
mise run test-pr028
```

The initial distribution links the Rust-backed OpenAI-compatible provider into
the same extension module. `Agent.openai_compatible()` constructs a keyless
Rust-backed client explicitly; importing `finstack_ai` still does not create a
provider client, initialize Tokio, read credentials, or open network resources.
Dropping a `Run` detaches observation rather than cancelling durable execution;
call `await run.cancel()` for explicit cancellation. Classic `abi3` wheels are
not part of the launch strategy.

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

`normalize_prebeta_shape()` exposes data-only Rust validation for child-lineage,
interaction-resolution, and authenticated external-completion shapes. It does
not route those commands or claim durable restart, pruning, or duplicate
completion semantics; PR-048 remains the blocking beta gate for those claims.

See the repository [README](../../README.md) for project bootstrap and documentation routing. License texts are centralized under [`../../licenses/`](../../licenses/).
