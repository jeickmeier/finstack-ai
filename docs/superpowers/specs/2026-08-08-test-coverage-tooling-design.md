# Test coverage tooling design

Date: 2026-08-08  
Status: approved for implementation planning  
Approach: thin mise wrappers (Option A)

## Goals

Add diagnostic test-coverage capabilities for Rust, Python, and WASM, runnable via mise tasks, with a dedicated Ubuntu CI job that uploads report artifacts.

Non-goals:

- Coverage percentage gates (Engineering Standards §10.2: percentages are diagnostic only)
- Codecov/Coveralls or badge integrations
- Real `wasm-bindgen-test` coverage before the WASM binding has a test suite
- Expanding PyO3 binding test matrix beyond import smoke

## Decisions

| Topic | Choice |
| --- | --- |
| Primary purpose | Local reports + CI artifact upload |
| WASM coverage | Scaffold only (`PENDING.md`, exit 0) until real wasm tests land |
| Python scope | `tools/` helpers + binding package import smoke |
| Python runner | Convert existing `unittest` suites to pytest now |
| CI placement | Dedicated Ubuntu job (not folded into the OS/channel matrix) |
| Orchestration | Thin mise tasks; no `tools/coverage` orchestrator / xtask |

## Mise tasks

| Task | Behavior |
| --- | --- |
| `coverage-rust` | Run `cargo llvm-cov` over the Cargo workspace; emit HTML + LCOV |
| `coverage-python` | Run pytest with coverage over `tools/` and the Python binding package surface |
| `coverage-wasm` | Write scaffold artifact explaining deferred real wasm coverage; exit 0 |
| `coverage` | Convenience aggregate that runs the three tasks above |

Existing `test` / `test-*` tasks keep the meaning “run tests.” Coverage tasks are additive and are not added to `mise run ci` by default (keeps the local aggregate fast). CI invokes coverage tasks directly.

### Tool pins

- Rust: pin `cargo-llvm-cov` via mise (same pattern as `cargo-deny`). Ensure `llvm-tools-preview` is available from the coverage task and/or `doctor` path.
- Python: pin `pytest` and `pytest-cov` (or equivalent `coverage` integration) via `uv run --with`, matching the existing ruff pin style, unless a small checked-in constraints file is needed for reproducibility.

## Report layout

Reports are generated under a gitignored tree:

```text
target/coverage/
  rust/html/
  rust/lcov.info
  python/html/
  python/coverage.xml
  wasm/PENDING.md
```

Also gitignore ephemeral Python artifacts: `.coverage`, `htmlcov/`, `coverage.xml`, `.pytest_cache/`.

## Python: pytest migration

Convert now:

- Rewrite `tools/{architecture,ci,security,schema_governance,benchmark}/tests` from `unittest.TestCase` to pytest-style functions and plain `assert`.
- Remove `unittest.main()` / class boilerplate; preserve behavior.
- Prefer shared path/pythonpath setup via `conftest.py` or pytest config over repeated `sys.path.insert` where practical.

Update mise tasks:

- `test-architecture`, `test-ci`, `test-schema-governance`, `test-benchmark`, and the architecture pre-check call pytest.
- No remaining `python -m unittest discover` in mise tasks.

Binding smoke:

- Add a minimal pytest under `bindings/finstack-ai-python` that imports `finstack_ai`.
- `coverage-python` measures `tools/**` and `bindings/finstack-ai-python/python/finstack_ai/**`.

Config:

- Keep root `pyproject.toml` as the uv workspace root (not an installable package).
- Put shared `[tool.pytest.ini_options]` and `[tool.coverage.*]` settings in root `pyproject.toml` so mise tasks can call `uv run --no-project --with … pytest` with one config home and no second orchestration layer.

## WASM scaffold

`coverage-wasm`:

1. Ensures `target/coverage/wasm/` exists.
2. Writes `PENDING.md` stating that real wasm coverage waits on a `wasm-bindgen-test` (or equivalent) suite for `finstack-ai-wasm` / kernel-on-wasm execution.
3. Exits 0 so CI can upload the scaffold artifact.

`check-wasm` remains the compile gate.

## CI

Add a dedicated Ubuntu job on `ci.yml` (name e.g. `coverage`):

1. Checkout
2. Install mise / pinned tools
3. `mise run coverage-rust`
4. `mise run coverage-python`
5. `mise run coverage-wasm`
6. Upload `target/coverage/**` with `actions/upload-artifact` (retention ~30 days)

Failure policy: fail only if a coverage command fails (missing tool, test crash, write error). Never fail on a coverage percentage.

## Docs and registers

- Mention new mise task names wherever contributor tooling commands are documented (minimal).
- Do not edit planning contracts unless implementation exposes a genuine conflict.
- Update implementation registers only for facts created by the work; do not invent gate completion.

## Success criteria

1. `mise run coverage-rust` produces HTML + LCOV under `target/coverage/rust/`.
2. `mise run coverage-python` runs the converted pytest suites plus binding import smoke and writes reports under `target/coverage/python/`.
3. `mise run coverage-wasm` writes the pending scaffold and exits 0.
4. `mise run coverage` runs all three.
5. Existing Python `test-*` mise tasks use pytest and stay green.
6. Dedicated Ubuntu CI job uploads coverage artifacts without percentage gating.
