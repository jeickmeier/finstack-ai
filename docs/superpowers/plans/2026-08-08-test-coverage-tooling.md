# Test Coverage Tooling Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add diagnostic Rust/Python/WASM coverage mise tasks, convert Python helper tests to pytest, and upload coverage artifacts from a dedicated Ubuntu CI job.

**Architecture:** Thin mise wrappers around `cargo-llvm-cov` and `pytest`/`pytest-cov`; WASM coverage is a scaffold artifact until real wasm tests exist. No percentage gates.

**Tech Stack:** mise, cargo-llvm-cov 0.8.7, pytest 9.1.1, pytest-cov 7.1.0, GitHub Actions `upload-artifact`

## Global Constraints

- Coverage percentages are diagnostic only (Engineering Standards §10.2); never fail CI on %.
- Root `mise.toml` is the sole toolchain pin and task entrypoint; no `xtask`.
- Keep root `pyproject.toml` as non-installable uv workspace root.
- Do not edit `docs/planning/` unless a genuine contract conflict appears.
- Do not mix unrelated PR-007 / planning diffs into this work.

---

### Task 1: Config, ignores, and plan/spec docs

**Files:**
- Modify: `.gitignore`
- Modify: `pyproject.toml`
- Modify: `README.md`
- Modify: `.github/ci/README.md`
- Exists: `docs/superpowers/specs/2026-08-08-test-coverage-tooling-design.md`

- [ ] **Step 1: Gitignore coverage/pytest artifacts**

Add:

```gitignore
# Coverage / pytest
target/coverage/
.coverage
htmlcov/
coverage.xml
.pytest_cache/
```

- [ ] **Step 2: Add pytest + coverage settings to root `pyproject.toml`**

```toml
[tool.pytest.ini_options]
pythonpath = ["tools", "bindings/finstack-ai-python/python"]
testpaths = [
  "tools/architecture/tests",
  "scripts/ci/tests",
  "tools/security/tests",
  "tools/schema_governance/tests",
  "tools/benchmark/tests",
  "bindings/finstack-ai-python/tests",
]

[tool.coverage.run]
branch = true
source = [
  "tools/architecture",
  "scripts/ci",
  "tools/security",
  "tools/schema_governance",
  "tools/benchmark",
  "bindings/finstack-ai-python/python/finstack_ai",
]
omit = ["*/tests/*"]

[tool.coverage.report]
skip_empty = true
```

- [ ] **Step 3: Document mise coverage tasks in README and `.github/ci/README.md`**

- [ ] **Step 4: Commit**

```bash
git add .gitignore pyproject.toml README.md .github/ci/README.md docs/superpowers/plans/2026-08-08-test-coverage-tooling.md
git commit -m "Add pytest coverage config and document coverage tasks."
```

---

### Task 2: Convert Python tests to pytest + binding smoke

**Files:**
- Modify: `tools/*/tests/test_*.py` (drop `unittest`, use plain asserts / `pytest.raises`)
- Create: `bindings/finstack-ai-python/tests/test_import.py`
- Remove path `sys.path.insert` once `pythonpath` is set

- [ ] **Step 1: Convert small suites** (`test_sources`, `test_waivers`, `test_secret_canary`, `test_release`, `test_run`)

- [ ] **Step 2: Convert large suites** (`tools/architecture/tests/test_check.py`, `tools/schema_governance/tests/test_check.py`)

- [ ] **Step 3: Add binding import smoke**

```python
"""Smoke tests for the placeholder Python package."""

def test_import_finstack_ai() -> None:
    import finstack_ai

    assert finstack_ai.__doc__ is not None
    assert "placeholder" in finstack_ai.__doc__.lower()
```

- [ ] **Step 4: Verify with pytest**

Run: `uv run --no-project --with pytest==9.1.1 pytest -q`

- [ ] **Step 5: Commit**

```bash
git commit -m "Convert helper tests to pytest and add Python import smoke."
```

---

### Task 3: Mise coverage and test tasks

**Files:**
- Modify: `mise.toml`

Pins:

```toml
"github:taiki-e/cargo-llvm-cov" = { version = "0.8.7", version_prefix = "v" }
```

Tasks:

- Update `doctor` to install `llvm-tools-preview` and print `cargo llvm-cov --version`
- Replace all `python -m unittest discover` with pinned pytest
- Add `coverage-rust`, `coverage-python`, `coverage-wasm`, `coverage`

`coverage-rust` writes `target/coverage/rust/{html,lcov.info}`.  
`coverage-python` writes `target/coverage/python/{html,coverage.xml}`.  
`coverage-wasm` writes `target/coverage/wasm/PENDING.md` and exits 0.

- [ ] **Step 1: Implement mise changes**
- [ ] **Step 2: `mise install` and run `mise run test-architecture` / `test-ci` / `test-schema-governance` / `test-benchmark`**
- [ ] **Step 3: Run `mise run coverage`**
- [ ] **Step 4: Commit**

```bash
git commit -m "Add Rust, Python, and WASM coverage mise tasks."
```

---

### Task 4: Dedicated Ubuntu CI coverage job

**Files:**
- Modify: `.github/workflows/ci.yml`

Add job `coverage` on `ubuntu-24.04` that runs the three coverage tasks and uploads `target/coverage/**` (retention 30 days). Fail only on command failure, never on %.

- [ ] **Step 1: Add job**
- [ ] **Step 2: `mise run lint-workflows`**
- [ ] **Step 3: Commit**

```bash
git commit -m "Upload diagnostic coverage artifacts from Ubuntu CI."
```

---

### Task 5: Final verification

- [ ] Run `mise run coverage`
- [ ] Run converted python test tasks
- [ ] Confirm artifacts exist under `target/coverage/{rust,python,wasm}/`
- [ ] Confirm no planning-doc edits unless required
