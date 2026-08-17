# Repo examples

Use these examples as shape guides. They are intentionally short and operational rather than exhaustive.

## Example 1: move domain logic out of a binding

### Before

A `#[pyfunction]` in `bindings/finstack-ai-python` parses Python inputs and also decides a validation or lifecycle rule before calling core.

```rust
#[pyfunction]
fn start_run_like_python_api(...) -> PyResult<PyRun> {
    let policy = if use_host_default {
        choose_policy_from_python_inputs(...)
    } else {
        fallback_policy(...)
    };
    finstack_ai::start_run_with_policy(..., policy).map_err(core_to_py)
}
```

### After

Move the rule into core, keep the binding focused on conversion and error mapping.

```rust
#[pyfunction]
fn start_run_like_python_api(...) -> PyResult<PyRun> {
    let params = StartRunParams { ... };
    finstack_ai::start_run(params).map_err(core_to_py)
}
```

Why this is the right refactor:

- Python and WASM can share the same rule.
- the binding is thinner and easier to maintain
- the behavior is easier to test at the Rust layer

## Example 2: replace a long Rust signature with a params struct

### Before

A core function grows past the repo's argument threshold and callers keep passing the same group of values together.

```rust
pub fn start_run(
    session_id: SessionId,
    agent_id: AgentId,
    timeout: Duration,
    cancellation: CancellationToken,
    metadata: Metadata,
    lane: LaneId,
    budget: BudgetScopeId,
) -> Result<Run, Error>
```

### After

Introduce a cohesive params struct and keep callers explicit.

```rust
pub struct StartRunParams {
    pub session_id: SessionId,
    pub agent_id: AgentId,
    pub timeout: Duration,
    pub cancellation: CancellationToken,
    pub metadata: Metadata,
    pub lane: LaneId,
    pub budget: BudgetScopeId,
}

pub fn start_run(params: StartRunParams) -> Result<Run, Error>
```

Also inspect:

- binding constructors or helpers that forward these arguments
- `.pyi` signatures if Python-facing constructors change
- docs for public fields if the params struct is public

## Example 3: split a large binding module without changing package shape

### Before

A binding module mixes wrapper types, extraction helpers, registration, and unrelated helper functions in one file.

### After

Split internals by responsibility, but keep the same external package shape:

- keep export behavior stable
- keep `__all__` stable unless the user asked for a public-surface cleanup
- keep package-level imports working through `bindings/finstack-ai-python/python/finstack_ai/__init__.py`

## Example 4: rename toward repo conventions and update mirrored surfaces

### Before

A Python-visible accessor or helper uses a one-off name that drifts from the repo's shared semantic vocabulary.

### After

Rename it toward the shared convention, then update every mirrored surface in one pass:

- Rust binding definition
- PyO3 registration and export lists
- Python package re-export file if relevant
- `.pyi` stub
- public-item inventory or docs that reference the old name

If the rename is not user-approved, keep the public name stable and do only internal cleanup.
