# Finstack Quant Hot-Path Examples

## Portfolio Valuation

Watch for repeated market-data lookups, cloned instruments, string metric-key construction inside loops, and binding conversions around each instrument.

## Attribution

Check that serial and parallel attribution share the same core path and deterministic ordering. Avoid allocating intermediate maps per factor when a reusable accumulator would be clearer and faster.

## Monte Carlo

Review path generation, payoff evaluation, RNG seeding, memory layout, and parallel aggregation. Favor deterministic reductions and preallocated buffers.

## Python Bindings

Runtime claims for Python workflows should use release-profile builds. Debug PyO3 builds are useful for correctness checks, not portfolio-scale performance conclusions.
