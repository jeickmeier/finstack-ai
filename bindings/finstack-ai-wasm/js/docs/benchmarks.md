# WASM/JS crossing benchmarks

PR-038 records warning-threshold measurements for the browser binding. PR-063
still owns performance-budget ratification. These numbers are not a release
gate.

## Categories

| Category | What is measured |
| --- | --- |
| Bundle size | Raw and gzip bytes of generated wasm/glue (`check.py size`) |
| Startup | `init()` then `Agent.create` |
| Reducer | One scripted model-only `Agent.run` |
| Event throughput | A multi-delta scripted stream |
| Host callback | Time spent in the JS model host versus WASM drive |

The report isolates WASM/JS crossing cost as

`(reducer_run_median - wasm_drive_median) / wasm_drive_median`

against the ratified 15% WASM budget (PRD NFR-PERF-003). The historical
PR-038 JSON stored `target_percent: 10`; that field is not the 1.0 fail
line. The in-module `wasm_drive_median` is the scripted run minus measured
JS host callback time.

## How to run

```bash
mise run benchmark-wasm
```

The task writes `docs/implementation/artifacts/pr-063/wasm-js-crossing.json`
and does not overwrite the PR-038 historical report. The harness is not
packed into the npm tarball.
