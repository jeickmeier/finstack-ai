# PR-030 implementation map

Date: 2026-08-12

## Delivered surface

- `bindings/finstack-ai-python/src/callbacks.rs` owns the trusted sync/async
  callback bridge, settlement-scoped context, and Model, Toolset,
  ContextProvider, Middleware, and Observer adapters.
- `bindings/finstack-ai-python/src/callback_fixture.rs` runs the public
  `finstack-ai-test` Model and Toolset conformance functions against real
  Python callbacks under a non-default feature.
- `bindings/finstack-ai-python/tests/test_callbacks.py` covers agent and tool
  loops, cooperative cancellation, stale-context rejection, timeout and error
  sanitization, thread sharing, descriptor caching, and pre-beta DTO parity.
- `bindings/finstack-ai-python/tests/test_callback_conformance.py` is the
  non-default Python entrypoint for exact public port conformance.
- `normalize_prebeta_shape()` exposes only Rust validation and normalization
  for child-lineage, interaction-resolution, and authenticated
  external-completion shapes. It does not route commands or claim PR-048
  durability semantics.

## Boundary decisions

- Python callbacks are trusted in-process extensions with host memory and
  authority; they are not an isolation boundary.
- One lazily started callback loop owns async callbacks across caller threads.
  Sync callbacks run on the native runtime's blocking executor and therefore do
  not depend on the Python event loop's default thread pool.
- No Rust lock is held across a Python callback. Each invocation has a bounded
  timeout and cancellation grace; late completion is discarded.
- Callback contexts are invalidated on settlement. Retained access fails with
  `python_callback_context_settled`.
- Tool descriptors and schemas and callback coroutine classification are
  normalized and cached at construction.
- Middleware is stage-level only and Observer is batch-level only. There is no
  default per-token Python callback surface.
- Callback context, middleware, and observer adapters are complete port
  implementations and conformance-testable, but the current native Agent
  facade only composes the Model and Toolset stages available in its preview
  driver. No deferred driver-stage activation is claimed.

## Local validation before immutable candidate

- `cargo clippy -p finstack-ai-python --all-targets --features callback-fixture --locked -- -D warnings`
- `mise run architecture` (24 passed, no findings)
- `mise run typecheck-python-binding` (strict mypy, no issues)
- `mise run test-python-binding` (23 passed, 1 non-default fixture skipped)
- non-default callback wheel conformance (1 passed)
- `mise run test-pr030` reached reproducible wheel/sdist proof and then exposed
  one forbidden direct kernel dependency; the dependency was removed and the
  architecture check rerun successfully. The immutable candidate must rerun the
  full aggregate.
