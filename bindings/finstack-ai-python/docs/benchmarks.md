# Python alpha benchmark evidence

PR-029 established the deterministic Rust-backed fast-path benchmark categories:
import/construction, isolated FFI, framework runtime, callback overhead, external
I/O, throughput, and idle memory. PR-031 re-ran the inherited fast-path proof
with optional Pydantic adapters and measured 4.9677% binding overhead against
the 10% warning threshold.

PR-032 treats capability selection as a bounded pre-run policy. Catalog bytes
are limited to 8 KiB at registration, tokenization is linear in input plus
catalog text, ties are deterministic by capability ID, and the ordinary model
loop retains direct pre-resolved handles. Focused conformance proves inactive
and activated paths; the aggregate benchmark smoke remains the machine-readable
performance report. The 61,952-byte idle-session result remains a warning, not a
release budget, until PR-063 ratifies the 1.0 workload and thresholds.

No external-provider latency is included in framework performance claims.
