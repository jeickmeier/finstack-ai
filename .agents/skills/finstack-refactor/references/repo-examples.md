# Repo examples

Use these examples as shape guides. They are intentionally short and operational rather than exhaustive.

## Example 1: move domain logic out of a binding

### Before

A `#[pyfunction]` in `finstack-quant-py` parses Python inputs and also decides a pricing or validation rule before calling core.

```rust
#[pyfunction]
fn price_like_python_api(...) -> PyResult<f64> {
    let bump = if use_market_convention {
        choose_bump_from_python_inputs(...)
    } else {
        fallback_bump(...)
    };
    finstack_quant_valuations::price_with_bump(..., bump).map_err(core_to_py)
}
```

### After

Move the rule into core, keep the binding focused on conversion and error mapping.

```rust
#[pyfunction]
fn price_like_python_api(...) -> PyResult<f64> {
    let params = PriceParams { ... };
    finstack_quant_valuations::price_like_api(params).map_err(core_to_py)
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
pub fn build_curve(
    id: CurveId,
    currency: Currency,
    day_count: DayCount,
    calendar: Calendar,
    convention: BusinessDayConvention,
    nodes: Vec<Node>,
    interp: Interpolator,
    extrap: Extrapolator,
) -> Result<Curve, Error>
```

### After

Introduce a cohesive params struct and keep callers explicit.

```rust
pub struct CurveBuildParams {
    pub id: CurveId,
    pub currency: Currency,
    pub day_count: DayCount,
    pub calendar: Calendar,
    pub convention: BusinessDayConvention,
    pub nodes: Vec<Node>,
    pub interp: Interpolator,
    pub extrap: Extrapolator,
}

pub fn build_curve(params: CurveBuildParams) -> Result<Curve, Error>
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

- keep `register()` behavior stable
- keep `__all__` stable unless the user asked for a public-surface cleanup
- keep package-level imports working through `finstack-quant-py/src/lib.rs` and Python `__init__.py`

This is the model used throughout the binding tree registered from `finstack-quant-py/src/lib.rs`.

## Example 4: rename toward repo conventions and update mirrored surfaces

### Before

A Python-visible accessor or helper uses a one-off name that drifts from the repo's `get_*` convention.

### After

Rename it toward the shared convention, then update every mirrored surface in one pass:

- Rust binding definition
- PyO3 registration and export lists
- Python package re-export file if relevant
- `.pyi` stub
- parity tests or docs that reference the old name

If the rename is not user-approved, keep the public name stable and do only internal cleanup.
