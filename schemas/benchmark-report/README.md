# benchmark-report

Owner: `me@jeickmeier.com`  
Compatibility profile: strict reject-unknown  
Fixtures: `fixtures/compatibility/benchmark-report/`

JSON Schema 2020-12 document for machine-readable Criterion metadata:

```text
schemas/benchmark-report/v1/metadata.schema.json
schemas/benchmark-report/v1/python-fast-path.schema.json
schemas/benchmark-report/v1/wasm-js-crossing.schema.json
schemas/benchmark-report/v1/size-budgets.schema.json
```

Required fields include compiler, target, commit, feature set, and machine
metadata (PR-005-A03; TDD §33.4). Benchmark regression remains non-merge-blocking.
The Python fast-path report separates import, construction, FFI, external I/O,
throughput, allocation, and idle-memory evidence for PR-029. The WASM/JS
crossing report isolates init, create, scripted reducer, event throughput, and
host-callback costs for PR-038. The size-budgets document is the A03 fail
table ratified by PR-063.
