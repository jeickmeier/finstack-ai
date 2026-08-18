# Performance budgets

Operational register for NFR-PERF-001 through NFR-PERF-007. Planning
sentences in `docs/planning/` remain authoritative. This file names the
reference environment, versioned workloads, fail-versus-warning hosts,
and the A03 size table. It does not record `G8-D-*`.

Workspace version stays `0.1.0`. Do not treat a local Darwin candidate
as a hosted perf-lab result.

## Reference environment

| Field | Value |
| --- | --- |
| Machine class | Apple Silicon desktop (arm64) |
| OS | Darwin 25.5.0 |
| CPU | arm64 |
| rustc | 1.97.1 (`mise.toml` pin) |
| Python | 3.14 (`mise.toml` pin) |
| Node | 22.18.0 (`mise.toml` pin) |
| Feature set | default native-tokio SDK; scripted models only |
| Comparison commit | tag `v0.1.0` = `9d09b87108f6286918fcc436c53d195d0b6b10cc` |
| Candidate commit | `0a84c8196624bdc4eeff8ec229a304fe6d69866d` |

PR-061 rehearsal artifacts are not a Criterion corpus. The checked-in
`0.1.0` comparison line is the method-plus-number corpus that exists at
the tag:

- [`artifacts/pr-029/python-fast-path-report.json`](artifacts/pr-029/python-fast-path-report.json)
- [`artifacts/pr-038/wasm-js-crossing.json`](artifacts/pr-038/wasm-js-crossing.json)
- [`artifacts/pr-038/bundle-size.json`](artifacts/pr-038/bundle-size.json)
- [`artifacts/pr-020/benchmark-baselines.txt`](artifacts/pr-020/benchmark-baselines.txt) (host-local `--quick` smoke; not a 1.0 fail line)

Do not invent missing Criterion numbers.

## Workload IDs

| ID | Surface | Command |
| --- | --- | --- |
| `kernel-micro-v1` | Native kernel decide/apply, init, RawJson, restore, record JSON, history view | `cargo bench -p finstack-ai-test --offline --locked --bench conformance -- kernel_micro` |
| `resolved-dispatch-v1` | Resolved native model dispatch excluding impl work | `cargo bench -p finstack-ai-test --offline --locked --bench conformance -- resolved_dispatch` |
| `scripted-reducer-v1` | Native scripted model-only reducer | `cargo bench -p finstack-ai-test --offline --locked --bench conformance -- scripted_model_reducer` |
| `idle-session-rss-v1` | Incremental process RSS via `ps` | `cargo bench -p finstack-ai-test --offline --locked --bench native_runtime -- --mode idle --sessions 1000` |
| `active-session-rss-v1` | Concurrent scripted Agent runs | `cargo bench -p finstack-ai-test --offline --locked --bench native_runtime -- --mode active --sessions 100` |
| `rust-backed-python-fast-path-v1` | Python vs native | editable install with `--features benchmark-fixture`; `uv run --no-project python tools/perf/python_fast_path.py` |
| `wasm-js-crossing-v1` | Browser WASM + JS | `mise run bench-wasm` |
| `minimal-cli-startup-v1` | Warm FS + kernel init | `uv run --no-project python tools/perf/measure_startup.py` |
| `size-budgets-v1` | Wheel / WASM / CLI bytes | `uv run --no-project python tools/perf/check_size_budgets.py` |
| `session-append-scaling-v1` | One structural append after 100 / 1,000 / 10,000 prior records (memory + SQLite) | `cargo bench -p finstack-ai-test --offline --locked --bench growth -- session_append_scaling` |
| `sqlite-scan-paging-v1` | Full SQLite scan of 10k / 100k records at page 64 / 256 (`1024` exceeds `SCAN_PAGE_MAX_RECORDS`) | `cargo bench -p finstack-ai-store-sqlite --offline --locked --bench restore_growth -- sqlite_scan_paging` |
| `sqlite-concurrent-v1` | 1 / 8 / 64 sessions, durable and relaxed; append/load plus event-loop delay | `cargo bench -p finstack-ai-store-sqlite --offline --locked --bench restore_growth -- sqlite_concurrent` |
| `state-scaling-activated-v1` | 0 / 64 / 256 tool identities × 16 / 1,024 / 4,096 messages; validate, hash, apply, failed-apply | `cargo bench -p finstack-ai-test --offline --locked --bench conformance -- state_scaling` |
| `tool-settlement-width-v1` | 1 / 16 / 64 / 256 calls; sequential, parallel, reverse completion | `cargo bench -p finstack-ai-test --offline --locked --bench growth -- tool_settlement_width` |
| `kernel-micro-extras-v1` | `raw_json_parse_64kib` / `1mib`, `raw_json_de_map_64kib`, `tool_result_ids_append`, `decide_context_prepared`, `apply_context_prepared`, `state_hash_v6_tools` | `cargo bench -p finstack-ai-test --offline --locked --bench conformance -- kernel_micro` |
| `packaging-profile-v1` | Isolated default-release vs thin-LTO/codegen compile and provider/TLS graphs | `uv run --no-project python tools/perf/measure_packaging_profile.py --packages kernel,runtime` |

These growth rows are probes. They do not invent NFR fail numbers. Host-local `--quick` smoke is not the 1.0 fail line except on the ratified Darwin arm64 host for the existing NFR-PERF-001–007 rows.

`packaging-profile-v1` is also a probe. It compares isolated release compile cost and provider/TLS graphs. It does not change `[profile.release]` and is not an NFR fail number. Recorded host-local results live under [`artifacts/perf-phase5/`](artifacts/perf-phase5/packaging-profile.md).

## Fail versus warning

Budgets fail the candidate **on this ratified Darwin arm64 host**.
Other hosts stay warning/regression only. Live model or network latency
is excluded (Engineering Standards principle 10).

| ID | Fail on reference host | Warning elsewhere | Notes |
| --- | --- | --- | --- |
| NFR-PERF-001 | median < 5 µs, p99 < 25 µs | same numbers, warning | `kernel-micro-v1` `decide_accept_run` |
| NFR-PERF-002 | < 10 µs | warning | resolved native dispatch excluding impl work and user-payload alloc |
| NFR-PERF-003 | Python ≤ 10%; WASM ≤ 15% | warning | PR-038 JSON stored `target_percent: 10`; ratify **15%** for WASM |
| NFR-PERF-004 | ≥ 100,000 small in-process progress events/s | warning | before binding batching |
| NFR-PERF-005 | < 32 KiB framework-owned / idle session | warning | excludes conversation, provider clients, store caches, app data, and tokio task stacks. Incremental `ps` RSS is a separate regression metric |
| NFR-PERF-006 | warm FS startup < 25 ms; kernel init < 1 ms | warning | `minimal-cli-startup-v1` |
| NFR-PERF-007 | compile-once conformance | same | not a numeric budget |

The 61,824-byte idle-session figure in the PR-029 report is incremental
process RSS after `MemoryJournalStore` construction, not allocator-exact
framework-owned bytes. Do not silently raise the 32 KiB sentence.

## Size table (A03)

Machine-readable copy: [`perf-size-budgets.json`](perf-size-budgets.json).
CI fails on exceed via `uv run --no-project python tools/perf/check_size_budgets.py`.

| Artifact | Budget | Source | Slack |
| --- | --- | --- | --- |
| Python wheel | 10 MiB (10,485,760 bytes) | PR-027–PR-032; do not raise | none |
| WASM `*_bg.wasm` raw | 10,223,616 bytes | checked-in tree 9,449,844 (`0.1.0` was 8,186,356; PR-038 historical 5,950,310) | ~8.2% |
| WASM `*_bg.wasm` gzip | 3,014,656 bytes | checked-in tree 2,790,984 (`0.1.0` was 2,464,652; PR-038 historical 1,811,048) | ~8.0% |
| Minimal native CLI | 9,437,184 bytes (9 MiB) | measured `target/release/minimal` 8,730,288 | ~8.1% |

`tools/wasm_package/check.py size` remains a measurement writer. It is
not the 1.0 fail gate and must not overwrite this table into
`artifacts/pr-038/bundle-size.json`.
