# A01 comparison versus checked-in 0.1.0 numbers

PR-061 rehearsal is not a Criterion corpus. Comparison uses the
method-plus-number files that exist at tag `v0.1.0`.

| Workload | 0.1.0 reference | This host | Disposition |
| --- | --- | --- | --- |
| Idle RSS, 128 sessions | 61,824 bytes/session (PR-020 / PR-029) | 65,152 bytes/session | Same order; ~5% host/noise. Incremental `ps` RSS is not NFR-PERF-005. |
| Idle RSS, 1,000 sessions | none at 0.1.0 | 62,914 bytes/session | Bounded vs 128-session RSS. |
| Active 100 scripted runs | none at 0.1.0 | 346,849 incremental RSS bytes/session after completion | Completes; queues stay at configured capacities. |
| Reducer `execute_model_only_completion` | 160.795 µs mean (`--quick`) | 164.56 µs mean (gate 100/5s) | No unexplained regression. |
| Model stream 256 items | 14.507 µs / 17.65 Melem/s (`--quick`) | 14.951 µs / 17.12 Melem/s (gate) | No unexplained regression. NFR-PERF-004 pass. |
| Tool stream 256 items | 18.040 µs / 14.19 Melem/s (`--quick`) | 18.392 µs / 13.92 Melem/s (gate) | No unexplained regression. |
| Kernel `decide_accept_run` | not published | 163 ns median | New TDD §33.1 fixture; NFR-PERF-001 pass. |
| Kernel init | not published | 157 ns median | NFR-PERF-006 kernel-init pass. |
| Resolved dispatch | not published | 212 ns median | NFR-PERF-002 pass. |
| Python fast path | 3.1515% overhead (PR-029) | 2.160% overhead (9 samples) | No regression. NFR-PERF-003 pass. |
| WASM crossing | PR-038 stored `target_percent` 10; measured overhead 0 | 0% vs ratified 15% | NFR-PERF-003 pass. Bundle raw 8,186,356. |

No live model or network latency is included.
