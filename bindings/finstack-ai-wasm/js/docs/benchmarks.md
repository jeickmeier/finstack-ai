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
| Host baseline | Batched `Uint8Array.slice()` of a 64 KiB payload |
| WASM crossing | The same payload copied into and out of a benchmark-only wasm-bindgen function |

The report isolates WASM/JS crossing cost as

`(wasm_drive_median / host_callback_median - 1) * 100`

against a 200% diagnostic warning threshold. The v1 report retains the legacy
field names: `host_callback_median` is the per-operation pure-JavaScript copy
baseline, and `wasm_drive_median` is the matching per-operation WASM round trip.

## How to run

```bash
mise run bench-wasm
```

The task writes `target/performance/wasm-js-crossing.json`. The harness is not
packed into the npm tarball.
