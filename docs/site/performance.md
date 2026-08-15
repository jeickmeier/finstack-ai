# Performance

Framework time is not model time. Scripted benches and the Python
fast-path fixture exclude provider, network, and durable-store latency.
Do not hide live-model latency inside framework numbers.

## Isolate framework cost

- Use scripted models for NFR-PERF evidence.
- Report native, Python, and WASM crossings separately.
- Python overhead must stay ≤ 10% of the native Rust-backed path.
- Browser WASM overhead must stay ≤ 15% of the native path.

## Batch events

Bindings deliver bounded event batches. Do not add a per-token host
callback. Drop-progress under backpressure is explicit; queues stay
bounded.

## Reuse providers and compiled schemas

Resolve the model, toolset, and store once. Keep the compiled JSON
Schema validators from construction. A second compile during a scripted
turn fails NFR-PERF-007 conformance.

## Prefer Rust-backed tools

A Rust-backed toolset stays on the native dispatch path. A Python or JS
callback is a coarse host crossing and is reported separately.

## Bound memory and queues

An idle session's framework-owned remainder excludes conversation
bytes, provider clients, store caches, and application data. Observer
and event-hub queues have explicit capacities. Do not unbind them to
win a benchmark.

Operational numbers live in
[`docs/implementation/perf-budgets.md`](../implementation/perf-budgets.md).
