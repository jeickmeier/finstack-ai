# finstack-ai Python bindings

PR-027 establishes the `finstack_ai` PyO3 extension package, typed facade,
editable development, and per-version wheel pipeline. PR-028 adds immutable
Rust-owned `Agent`, `Run`, `Session`, `RunResult`, `Event`, and `EventBatch`
handles, async result/cancellation, and batch-first event observation.

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

See the repository [README](../../README.md) for project bootstrap and documentation routing. License texts are centralized under [`../../licenses/`](../../licenses/).
