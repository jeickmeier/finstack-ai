# Finstack Hot-Path Examples

## Kernel decide/apply

Watch for cloned records, stringly-typed lookups, and allocations inside the per-turn decide or apply loop. Kernel code must stay I/O-free; do not "optimize" by adding caches that hide I/O.

## Runtime effect dispatch

Check that commit-before-effect stays on one path. Avoid allocating intermediate maps per tool call when a reusable accumulator would be clearer and faster.

## Binding conversions

Runtime claims for Python or WASM workflows should use release-profile builds. Debug PyO3 builds are useful for correctness checks, not production-scale performance conclusions. Keep conversions outside the inner loop.

## WASM host

Minimize crossings of the WASM boundary. Do bulk work in Rust and return results in one call. Do not call into WASM per element of a batch.
