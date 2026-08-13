# finstack-ai Python bindings

PR-027 establishes the `finstack_ai` PyO3 extension package, typed facade,
editable development, and per-version wheel pipeline. PR-028 adds immutable
Rust-owned `Agent`, `Run`, `Session`, `RunResult`, `Event`, and `EventBatch`
handles, async result/cancellation, and batch-first event observation.
PR-030 adds trusted coarse Python adapters for Model, Toolset,
ContextProvider, Middleware, and batched Observer ports.
PR-031 adds optional Pydantic tool and structured-output ergonomics while the
Rust validator and kernel continue to own schema outcomes and retries.
PR-032 completes the Python alpha candidate with shared golden traces,
declarative capability activation, starter projects, API/migration references,
and staged checksums/SBOM/keyless signatures.

```bash
mise run python-develop
mise run test-python
mise run test-pr031
mise run test-pr032

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

See the repository [README](../../README.md) for project bootstrap and documentation routing. License texts are centralized under [`../../licenses/`](../../licenses/).

The complete alpha surface is summarized in the
[API reference](docs/api-reference.md), with
[0.0.2 migration notes](docs/migration-0.0.2.md) and
[benchmark evidence](docs/benchmarks.md). The two
[Python starter projects](../../examples/python-minimal/) are checked against
the installed wheel without a compiler.
