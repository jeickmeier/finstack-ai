# TDD §33 coverage for PR-063

Reuse existing harnesses. No second bench family.

## 33.1 Microbenchmarks

| Item | Where |
| --- | --- |
| `decide` / `apply` by transition kind | `kernel_micro` `decide_accept_run` / `apply_accept_run`; reducer group covers model-only apply |
| record encode/decode | `kernel_micro` `record_draft_json_roundtrip` |
| tool registry lookup | construction-time only; `resolved_plan_retains_direct_handles_without_registry_lookup` plus `resolved_dispatch` |
| context assembly | `scripted_model_reducer` / `execute_model_only_completion` |
| event batching | `model_stream_throughput` / `tool_stream_throughput` |
| message history view | `kernel_micro` `message_history_view`; `state_scaling` |
| snapshot replay | `kernel_micro` `try_restore_default`; sqlite `restore_growth` bench |

## 33.2 Synthetics

| Item | Where |
| --- | --- |
| (1) text-only | `scripted_model_reducer`; `examples/rust-minimal` |
| (2) 1/10/100/1,000 deltas | Python fixture max 4,096; stream assembler 256; PR-029 512-delta report |
| (3) one fast tool | calculator bench; NFR-PERF-007 calculator registration |
| (4) 100 fast tools | calculator 256-operand throughput |
| (5) parallel vs sequential | `native_runtime --mode active`; Python independent-await gate |
| (6) output validation | NFR-PERF-007 output schema |
| (7) cancellation | existing runtime/Python callback cancellation tests |
| (8) approval suspend/resume | existing interaction tests |
| (9) journal restore | sqlite `restore_growth`; kernel `try_restore` |
| (10) 1,000 idle sessions | `native_runtime --mode idle --sessions 1000` |
| (11) 100 active sessions | `native_runtime --mode active --sessions 100` |
| (12) large result via blob | existing blob-ref public-API fixtures |

## 33.3 Binding benches

| Surface | Where |
| --- | --- |
| native from Rust | `finstack-ai-test` benches |
| native from Python | `tools/perf/python_fast_path.py` + `benchmark_fixture.rs` |
| one Python tool callback | `test_callbacks.py` / callback fixture (reported separately; not the 10% fast path) |
| Python model stream callback | same callback suite |
| browser WASM + JS | `mise run benchmark-wasm` → `artifacts/pr-063/wasm-js-crossing.json` |
| native host + WIT | `plugins/finstack-ai-plugin-host/benches/plugin_host.rs` |

## 33.4 Artifacts

Machine metadata, rustc, commit, feature set, and secret-free reports live
under this directory. Flamegraphs are the text stand-in in
`flamegraph-notes.md`.
