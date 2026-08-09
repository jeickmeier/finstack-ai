# Benchmark Regression Workflow

Use when reviewing runtime or allocation changes in performance-sensitive finstack-quant code.

## Default Evidence

- Identify the changed hot path and expected workload size.
- Check whether a benchmark already covers it.
- If Python bindings are involved, use release-profile PyO3 builds for runtime conclusions.
- Compare against a saved baseline when available; otherwise report absolute numbers and uncertainty.

## Finstack Commands

- Rust benchmarks: `mise run rust-bench`
- Saved baseline comparison: `mise run rust-bench-compare` if configured for the branch
- Python extension build: `mise run python-build -- --release` for runtime-sensitive Python benchmarks
- Broad final checks: `mise run all-lint` and relevant targeted tests after code changes

## Review Questions

- Is the algorithmic complexity appropriate for portfolio-scale inputs?
- Are allocations inside pricing, attribution, scenario, or Monte Carlo loops justified?
- Does parallelism preserve deterministic results and avoid contention?
- Are serialization and binding conversions outside hot loops?
- Does the benchmark measure the same path users care about?
