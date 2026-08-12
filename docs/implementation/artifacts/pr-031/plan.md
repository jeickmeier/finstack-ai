# PR-031 implementation plan

Date: 2026-08-12
Baseline: `73e2863cd090dd0a88b5f91c2c69987856685193` (`main`, PR-030 closed)
Mode: integrated into local `main`

## Admission

- PR-030 is `Done`; both Phase 4 entrance criteria and G3 pass.
- Scope is the PR-031 Pydantic adapter contract in Implementation Plan v0.18,
  FR-PY structured typing, PRD G-03, Technical Design 15.3 and 25.6-25.8,
  ADR-022, Engineering Standards 7.2/8, and Threat Model TM-02/TM-16.
- Rust remains the canonical Draft 2020-12 validator and retry owner. Pydantic
  adapters derive and cache schemas and convert only at the trusted Python
  object boundary; the kernel receives normalized validation outcomes only.
- Pydantic remains an optional extra and package import remains side-effect
  free. No provider-specific schema dialect, ambient reference lookup, Python
  kernel/reducer implementation, browser binding, publication, or G4 decision
  is introduced.

## Implementation slices

1. Add one binding-owned portable Draft 2020-12 normalization policy for the
   Pydantic subset. Canonicalize once, inject closed-object semantics, allow
   local definitions/references, and reject unsupported keywords, optional
   provider fields, external references, or non-object provider roots with an
   exact JSON-pointer diagnostic.
2. Add a lazy optional Pydantic adapter that accepts BaseModel subclasses,
   dataclasses, TypedDicts, arbitrary TypeAdapter-compatible targets, and
   annotated function signatures. Cache TypeAdapter and validation/serialization
   schemas per registration; expose an explicit pre-registration refresh.
3. Add `@tool` plus a concise toolset factory. Convert validated raw arguments
   to Python once, execute sync or async callables through the PR-030 bridge,
   serialize results once, and leave invalid argument/output retry settlement
   on the existing Rust schema path.
4. Add `output_type=` to Python-agent composition. Generate and compile its
   schema once, configure the kernel output contract before `BeforeRun`, submit
   Rust validator outcomes after model settlement, drive bounded validation
   retries through the existing durable timer path, and expose the final typed
   object on `RunResult`.
5. Align the typed facade, native stubs, optional dependency metadata, README,
   examples, and precise help/error text. Keep top-level import independent of
   Pydantic.
6. Add deterministic offline tests for all supported shapes, schema caching and
   refresh, precise portability failures, sync/async tools, structured output,
   Rust/Pydantic validation parity, and retry traces; add `mise run test-pr031`
   and run exact candidate, hosted, and post-merge validation.

## Expected files

- `crates/finstack-ai/src/agent.rs`
- `bindings/finstack-ai-python/src/callbacks.rs`
- `bindings/finstack-ai-python/src/lib.rs`
- `bindings/finstack-ai-python/python/finstack_ai/__init__.py`
- `bindings/finstack-ai-python/python/finstack_ai/_pydantic.py`
- `bindings/finstack-ai-python/python/finstack_ai/_finstack_ai.pyi`
- `bindings/finstack-ai-python/tests/test_pydantic_adapters.py`
- `bindings/finstack-ai-python/README.md`
- `bindings/finstack-ai-python/pyproject.toml`
- `mise.toml`
- `.github/workflows/ci.yml`
- `docs/implementation/*` and `docs/implementation/artifacts/pr-031/*`

## Verification

- Focused Rust tests and Clippy for the native agent and Python binding.
- Ruff, strict mypy, editable-install tests with and without the Pydantic extra,
  installed-wheel tests, and top-level import side-effect checks.
- Native/Pydantic success/failure and validation-retry trace parity fixtures.
- `mise run test-pr031` and `mise run ci` on one immutable source candidate.
- Exact hosted source distribution plus the approved CPython/platform wheel
  rows, aggregate Linux/macOS/Windows CI, and hosted security at one evidence
  revision before local integration.
- Post-merge `mise run test-pr031` and `mise run ci`, followed by one atomic
  register/evidence closure transaction.
