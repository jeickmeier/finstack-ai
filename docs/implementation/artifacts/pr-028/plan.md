# PR-028 implementation plan

Date: 2026-08-12  
Owner: `me@jeickmeier.com`  
Branch: `codex/pr-028-python-handles`  
Baseline: validated PR-027 integration at `84699e247fd818283c9d0cd14f5a723834f50f41`

## Execution envelope

PR-028 is the only active member of the authorized PR-027 through PR-038
range. The range uses integrated mode with target `main`. Local branch,
commit, and merge actions plus external branch/`main` pushes and hosted
CI/security workflows are authorized. Hosted pull-request creation/merge and
package publication are prohibited; release artifacts may be staged.

## Frozen scope

1. Add one Rust-owned native run-control handle that starts the existing SDK
   driver once, retains bounded event-batch delivery, supports explicit durable
   cancellation and repeatable result waits, and detaches rather than cancels
   when the last frontend handle is dropped.
2. Expose `Agent`, `Run`, `Session`, `RunResult`, `Event`, and `EventBatch` as
   immutable or internally synchronized PyO3 handles backed by shared Rust
   objects. Python receives snapshots and serializes them only on explicit
   request.
3. Make `Agent.run()`, `Run.cancel()`, `Run.result()`, and batch iteration
   awaitable through the current PyO3 Tokio bridge. `Agent.start()` returns the
   control handle immediately and performs no Python callback.
4. Map stable Rust codes into a documented `FinstackError` hierarchy with safe
   identifier context and retryability. Keep runtime signatures, checked-in
   stubs, facade exports, docstrings, and IDE discovery aligned.
5. Prove an offline Rust-backed model-only run, batch-first FFI delivery,
   cancellation/detach semantics, exception context, concurrent access, and
   absence of retained tasks or streams. Preserve the PR-027 wheel matrix,
   release staging, and Rust-only dependency boundaries.

The binding uses `pyo3-async-runtimes` 0.29 with its Tokio runtime bridge,
matching PyO3 0.29. Current upstream documentation confirms that
`future_into_py` maps `Send + 'static` Rust futures to Python awaitables and
propagates Python future cancellation to the wrapped Rust future. Durable run
cancellation remains an explicit `Run.cancel()` command rather than relying on
Python object or awaitable destruction.

## Acceptance map

| Acceptance | Planned proof |
| --- | --- |
| PR-028-A01 | Installed-package asyncio test completes a real model-only SDK run using only Rust-backed model/store objects |
| PR-028-A02 | Drop, detach, explicit cancel, repeated cancel/result, concurrent handle access, shutdown, and stream/task counters prove documented leak-free ownership |
| PR-028-A03 | A multi-delta run crosses Python as fewer `EventBatch` values than logical events; iteration preserves order and terminal delivery |
| PR-028-A04 | Configuration, runtime, cancellation, and timeout fixtures preserve stable code, retryability, safe locator context, and documented exception subclass |

## Security and compatibility disposition

PR-028 implements the already approved binding-safety design. It adds no new
trust class, callback path, protocol, parser, network listener, secret-bearing
adapter, privileged capability, primary port, middleware stage, durable field,
or stronger effect guarantee, so no separate ADR or threat-model review trigger
is identified. Existing Python free-threading controls still require concurrent
handle, cancellation, shutdown, and object-lifetime evidence before merge.

Python-defined models, tools, middleware, observers, per-token callbacks,
Pydantic adapters, fast-path performance claims, GIL microbenchmarks, release
metadata/provenance, and any browser/WASM API remain explicitly excluded.
