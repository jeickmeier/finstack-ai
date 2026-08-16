# Python binding performance notes

Deterministic Rust-backed fast-path categories: import/construction,
isolated FFI, framework runtime, callback overhead, external I/O,
throughput, and idle memory. Optional Pydantic adapters are measured
against the same 10% binding-overhead warning threshold.

Capability selection is an explicit pre-run argument. Catalog bytes are
limited to 8 KiB at registration. The ordinary model loop retains direct
pre-resolved handles. Focused conformance proves inactive and activated
paths; the aggregate benchmark smoke remains the machine-readable
performance report.

Idle-session RSS is a warning signal, not a release budget, until the
published 1.0 workload in [docs/site/performance.md](../../../docs/site/performance.md)
and [docs/implementation/perf-budgets.md](../../../docs/implementation/perf-budgets.md)
says otherwise.

No external-provider latency is included in framework performance claims.
