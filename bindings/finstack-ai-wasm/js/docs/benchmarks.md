# WASM/JS crossing benchmarks

This benchmark records warning-threshold measurements for the browser binding.
These numbers are diagnostic and are not a release requirement.

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

against the 15% WASM budget. The in-module `wasm_drive_median` is the scripted
run minus measured JS host callback time.

## How to run

```bash
mise run bench-wasm
```

The task writes `target/performance/wasm-js-crossing.json`. The harness is not
packed into the npm tarball.
