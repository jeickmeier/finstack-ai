# PR-029 implementation plan

Date: 2026-08-12  
Owner: `me@jeickmeier.com`  
Branch: `codex/pr-029-python-fast-path`  
Baseline: validated PR-028 integration closure at `d2489c4`

## Execution envelope

PR-029 is the only active member of the authorized PR-027 through PR-038
range. The range uses integrated mode with target `main`. Local branch,
commit, and merge actions plus external branch/`main` pushes and hosted
CI/security workflows are authorized. Hosted pull-request creation/merge and
package publication are prohibited; benchmark and release artifacts may be
staged.

## Frozen scope

1. Keep native orchestration in the existing Rust SDK/runtime path. Move any
   remaining synchronous Rust-only scheduling in the Python binding outside
   the attached interpreter thread; attach only for Python object and error
   conversion.
2. Preserve the existing bounded event hub's count, byte, and time coalescing.
   Add efficient batch-level conversion evidence and prove that default
   delivery crosses Python fewer times than logical model deltas.
3. Add a benchmark-only deterministic Rust-backed model fixture. It must not
   enter the production wheel feature set or create a second semantic engine.
   Compare the same SDK workload through native and Python frontends.
4. Stage a machine-readable report that separates import, construction,
   native-run, Python-run, FFI-batch, external-I/O, throughput, allocation, and
   idle-memory measurements. Treat NFR-PERF budgets as warning/regression
   targets until PR-063 ratifies the 1.0 environment.
5. Add deterministic concurrency proof showing two independent Python awaits
   reach Rust-owned blocking points before either is released. Retain offline,
   credential-free wheel/package and architecture validation.

Current PyO3 0.29 documentation names `Python::detach` as the supported way to
run synchronous Rust-only work outside the attached interpreter thread and
`Python::attach` as the re-entry boundary. The current
`pyo3-async-runtimes` bridge maps `Send + 'static` Rust futures into Python
awaitables; all HTTP streaming, storage, waiting, and native tool work remains
inside those Rust futures.

## Acceptance map

| Acceptance | Planned proof |
| --- | --- |
| PR-029-A01 | Versioned synthetic report compares identical native/Python SDK runs and enforces the documented 10% overhead warning target |
| PR-029-A02 | Multi-delta fixture reports logical events, delivered batches, and zero Python callbacks; batches are fewer than deltas |
| PR-029-A03 | Gate-controlled asyncio fixture proves independent Rust runs overlap before release on both classic and free-threaded builds |
| PR-029-A04 | JSON report schema and tests require distinct import, construction, FFI, external-I/O, throughput, allocation, and idle-memory fields |

## Security and compatibility disposition

PR-029 changes no trust class, callback boundary, protocol, parser, network
listener, secret-bearing adapter, privileged capability, primary port,
middleware stage, durable field, or effect guarantee. The synthetic model is
compiled only behind an explicit non-default benchmark feature and is absent
from release wheels, so no ADR or threat-model redesign trigger is identified.

Python-defined models/tools, callback performance, release publication,
live-provider measurements, final PR-063 budget ratification, and browser/WASM
performance remain explicitly excluded.
