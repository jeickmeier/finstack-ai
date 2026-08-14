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

against a 10% warning target, matching the PR-029 Python overhead field. The
in-module `wasm_drive_median` is the scripted run minus measured JS host
callback time.

## How to run

```bash
mise run benchmark-wasm
```

The task writes `docs/implementation/artifacts/pr-038/wasm-js-crossing.json`
and does not pack the harness into the npm tarball.
