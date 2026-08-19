# Benchmark Regression Workflow

Use when reviewing runtime or allocation changes in performance-sensitive finstack code.

## Default Evidence

- Identify the changed hot path and expected workload size.
- Check whether a benchmark already covers it.
- If Python bindings are involved, use release-profile PyO3 builds for runtime conclusions.
- Compare against a saved baseline when available; otherwise report absolute numbers and uncertainty.

## Finstack Commands

- WASM package size: `uv run --no-project python scripts/perf/check_size_budgets.py`
- WASM benches: `mise run bench-wasm` when the host or JS facade changed
- Broad final checks: `mise run check-all` and relevant targeted tests after code changes

## Review Questions

- Is the algorithmic complexity appropriate for production-scale inputs?
- Are allocations inside kernel decide/apply, runtime dispatch, or binding conversion loops justified?
- Does parallelism preserve deterministic durable history and avoid contention?
- Are serialization and binding conversions outside hot loops?
- Does the benchmark measure the same path users care about?
