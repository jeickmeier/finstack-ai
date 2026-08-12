# PR-030 implementation plan

Date: 2026-08-12
Baseline: `b824467` (`main`, PR-029 closed)
Mode: integrated into local `main`

## Admission

- PR-028 and PR-029 are `Done`; both Phase 4 entrance criteria and G3 pass.
- Scope is the PR-030 callback-adapter contract in Implementation Plan v0.18,
  FR-PY-005, Architecture 13.3/13.5, TDD 14-17/19/25.1-25.6, Engineering
  Standards 7.2/8, and Threat Model TM-06 plus section 8.2.
- The existing six ports, seven middleware stages, Rust-owned run semantics,
  commit-before-effect order, and trusted in-process boundary remain unchanged.
  No ADR or Threat Model section 18 redesign trigger is introduced.
- The binding will not claim sandboxing, per-token middleware callbacks,
  arbitrary kernel/reducer subclassing, durable restart parity, Pydantic schema
  derivation, or publication. PR-031 owns Pydantic adapters; PR-048 owns durable
  restart/pruning/duplicate-completion parity.

## Implementation slices

1. Add one shared PyO3 callback bridge that caches sync/async classification at
   registration, converts one normalized request and result per invocation,
   executes sync callables through an executor coroutine, awaits async
   callables through `pyo3-async-runtimes`, and never holds a Rust lock across
   Python execution.
2. Give every invocation a bounded `CallbackContext` carrying immutable
   locator/identity data, cooperative cancellation, a configured timeout, and
   an atomic settlement guard. Invalidate the guard on success, exception,
   cancellation, or timeout so retained contexts fail closed and late Python
   completion is discarded.
3. Implement trusted in-process Python `Model` and `Toolset` adapters over the
   existing Rust traits. Cache model profiles, descriptors, tool metadata, and
   canonical schemas at registration. Add Python-agent composition for one
   Python model and ordered Python toolsets without moving orchestration into
   Python.
4. Implement the supported coarse `ContextProvider`, `Middleware`, and
   batched `Observer` trait adapters and their conformance entrypoints. Preserve
   the fixed stage/outcome matrix, metadata/redaction modes, and explicit trust
   label; do not add a per-token callback or pretend the current native preview
   driver executes deferred context/middleware/observer stages.
5. Expose pre-beta, Rust-normalized data handles for callback run identity,
   child lineage, interactions, and authenticated external completion, plus a
   shared scripted/in-memory parity fixture. These handles serialize existing
   Rust DTOs and commands only; Python does not implement routing, replay,
   idempotency, or authorization semantics.
6. Align the typed facade, native stubs, runtime signatures, help text, README,
   trust/reentrancy/lifetime documentation, CI task, and exact package checks.
   Add deterministic offline tests for sync/async model and tool callbacks,
   cancellation, timeout and late completion, sanitized exceptions, stale
   contexts, cached schemas, observer batching, free-threaded concurrent use,
   and the pre-beta parity trace.

## Expected files

- `bindings/finstack-ai-python/src/callbacks.rs`
- `bindings/finstack-ai-python/src/lib.rs`
- `bindings/finstack-ai-python/python/finstack_ai/__init__.py`
- `bindings/finstack-ai-python/python/finstack_ai/_finstack_ai.pyi`
- `bindings/finstack-ai-python/tests/test_callbacks.py`
- `bindings/finstack-ai-python/README.md`
- `bindings/finstack-ai-python/Cargo.toml`
- `mise.toml`
- `.github/workflows/ci.yml`
- `docs/implementation/*` and `docs/implementation/artifacts/pr-030/*`

## Verification

- Focused Rust check/Clippy for the default and callback-conformance feature
  graphs.
- Ruff, strict mypy, editable-install tests, installed-wheel tests, runtime/stub
  signature checks, architecture checks, and release-wheel exclusion of test
  fixtures.
- `mise run test-pr030` on an immutable source candidate.
- Exact hosted source distribution plus the 20 approved CPython/platform wheel
  rows, aggregate Linux/macOS/Windows CI, and hosted security at one evidence
  revision before local integration.
- Post-merge `mise run test-pr030` and `mise run ci`, followed by one atomic
  register/evidence closure transaction.
