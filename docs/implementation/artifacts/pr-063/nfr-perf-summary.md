# NFR-PERF-001–007 on the ratified Darwin arm64 host

| ID | Budget | Result | Evidence |
| --- | --- | --- | --- |
| NFR-PERF-001 | median < 5 µs, p99 < 25 µs | PASS ~163 ns decide | `kernel-micro.txt` |
| NFR-PERF-002 | < 10 µs resolved dispatch | PASS ~212 ns | `kernel-micro.txt` `instant_model_request_poll` |
| NFR-PERF-003 | Python ≤ 10%; WASM ≤ 15% | PASS 2.160% / 0% | `python-fast-path-report.json`; `wasm-js-crossing.json` |
| NFR-PERF-004 | ≥ 100,000 events/s | PASS 17.12 M items/s assemble; 123,639 Python-native deltas/s | `kernel-micro.txt`; Python report |
| NFR-PERF-005 | < 32 KiB framework-owned / idle session | PASS 96-byte `RunTaskOwner` + bounded queues. Incremental `ps` RSS (~65 KiB at 128) is not this metric | `session-profiles.json` |
| NFR-PERF-006 | warm FS < 25 ms; kernel init < 1 ms | PASS kernel init ~157 ns; warm full example ~10.0 ms (includes scripted run) | `kernel-micro.txt`; `startup-report.json` |
| NFR-PERF-007 | no per-call compile | PASS | `cargo test -p finstack-ai --test nfr_perf_007` |

No `EX-*` row. Do not treat incremental RSS as a fail line. No `G8-D-*`.
