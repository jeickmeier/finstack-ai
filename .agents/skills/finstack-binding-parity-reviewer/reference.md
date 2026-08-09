# Python Binding Reference

## Codebase Structure

```
finstack-quant-py/
├── src/                    # Rust binding code (PyO3)
│   ├── lib.rs             # Main entry, module registration
│   ├── errors.rs          # Exception hierarchy, error mapping
│   ├── bindings/          # Rust binding modules
│   │   ├── common/        # Shared argument extraction helpers
│   │   ├── core/          # Core domain bindings
│   │   ├── currency.rs
│   │   ├── money.rs
│   │   ├── dates/
│   │   └── market_data/
│   │   ├── valuations/    # Instrument bindings
│   │   ├── statements/    # Statement evaluation
│   │   ├── scenarios/     # Scenario engine
│   │   └── portfolio/     # Portfolio management
└── finstack-quant/              # Python package
    ├── __init__.py        # Package initialization
    ├── *.pyi              # Type stubs (auto-generated)
    └── core/
        └── expr_helpers.py # Ergonomic wrapper (acceptable)
```

## Standard Patterns

### 1. Wrapper Struct Pattern

Every Rust type exposed to Python follows this pattern:

```rust
use pyo3::prelude::*;
use finstack_quant_core::money::Money;

#[pyclass(name = "Money", module = "finstack_quant.core.money", frozen)]
pub struct PyMoney {
    pub(crate) inner: Money,  // Always named "inner"
}

impl PyMoney {
    /// Internal constructor - used by other bindings
    pub(crate) fn from_inner(inner: Money) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyMoney {
    /// Python constructor
    #[new]
    fn new(amount: f64, currency: CurrencyArg) -> PyResult<Self> {
        Ok(Self {
            inner: Money::new(amount, currency.0),
        })
    }

    /// Getter - just exposes Rust data
    #[getter]
    fn amount(&self) -> f64 {
        self.inner.amount()
    }

    /// Method - delegates to Rust, maps error
    fn convert(&self, target: CurrencyArg, fx: &PyFxMatrix) -> PyResult<Self> {
        self.inner
            .convert(target.0, &fx.inner)
            .map(Self::from_inner)
            .map_err(crate::errors::map_error)
    }
}
```

### 2. Flexible Argument Extraction

Accept multiple Python types for better ergonomics:

```rust
use pyo3::prelude::*;
use pyo3::types::PyString;

/// Wrapper for flexible Currency argument
pub struct CurrencyArg(pub Currency);

impl<'py> FromPyObject<'py> for CurrencyArg {
    fn extract_bound(obj: &Bound<'py, PyAny>) -> PyResult<Self> {
        // Try 1: Direct PyCurrency extraction
        if let Ok(ccy) = obj.extract::<PyRef<PyCurrency>>() {
            return Ok(CurrencyArg(ccy.inner));
        }

        // Try 2: String parsing
        if let Ok(code) = obj.extract::<&str>() {
            return Currency::from_str(code)
                .map(CurrencyArg)
                .map_err(|_| crate::errors::unknown_currency(code));
        }

        // Fallback: Type error
        Err(PyTypeError::new_err(
            "Expected Currency instance or ISO currency code string"
        ))
    }
}

// Usage: Accept both Currency objects and strings
#[pymethods]
impl PyMoney {
    fn convert(&self, target: CurrencyArg) -> PyResult<Self> {
        // Works with: money.convert(Currency.USD) or money.convert("USD")
        ...
    }
}
```

### 3. Error Mapping

Centralized error conversion in `errors.rs`:

```rust
use pyo3::prelude::*;
use pyo3::exceptions::*;
use finstack_quant_core::error::Error as CoreError;

// Custom exception hierarchy
pyo3::create_exception!(finstack, FinstackError, PyException);
pyo3::create_exception!(finstack, ConfigurationError, FinstackError);
pyo3::create_exception!(finstack, ComputationError, FinstackError);
pyo3::create_exception!(finstack, CalibrationError, FinstackError);

/// Map core error to Python exception
pub fn map_error(e: CoreError) -> PyErr {
    match e {
        CoreError::Configuration(msg) => ConfigurationError::new_err(msg),
        CoreError::Computation(msg) => ComputationError::new_err(msg),
        CoreError::Calibration(msg) => CalibrationError::new_err(msg),
        CoreError::Currency(msg) => CurrencyError::new_err(msg),
        CoreError::Interpolation(msg) => InterpolationError::new_err(msg),
        CoreError::Internal(msg) => InternalError::new_err(msg),
        _ => FinstackError::new_err(e.to_string()),
    }
}

// Convenience helpers for specific errors
pub fn unknown_currency(code: &str) -> PyErr {
    CurrencyError::new_err(format!("Unknown currency code: {}", code))
}

pub fn invalid_date(s: &str) -> PyErr {
    ConfigurationError::new_err(format!("Invalid date format: {}", s))
}
```

### 4. Module Registration

Each module has a consistent registration pattern:

```rust
use pyo3::prelude::*;

pub fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
    let m = PyModule::new(parent.py(), "currency")?;

    // Add classes
    m.add_class::<PyCurrency>()?;

    // Add functions
    m.add_function(wrap_pyfunction!(parse_currency, &m)?)?;

    // Set module metadata
    m.setattr("__all__", vec!["Currency", "parse_currency"])?;
    m.setattr("__doc__", "Currency types and utilities")?;

    // Register as submodule
    parent.add_submodule(&m)?;

    Ok(())
}
```

### 5. Builder Pattern

For complex objects with many optional parameters:

```rust
#[pyclass(name = "BondBuilder", module = "finstack_quant.valuations.bond", unsendable)]
pub struct PyBondBuilder {
    inner: BondBuilder,
}

#[pymethods]
impl PyBondBuilder {
    #[new]
    fn new() -> Self {
        Self { inner: BondBuilder::new() }
    }

    fn notional(mut slf: PyRefMut<'_, Self>, value: f64) -> PyRefMut<'_, Self> {
        slf.inner = slf.inner.clone().notional(value);
        slf
    }

    fn coupon(mut slf: PyRefMut<'_, Self>, rate: f64) -> PyRefMut<'_, Self> {
        slf.inner = slf.inner.clone().coupon(rate);
        slf
    }

    fn build(&self) -> PyResult<PyBond> {
        self.inner.build()
            .map(PyBond::from_inner)
            .map_err(crate::errors::map_error)
    }
}
```

### 6. Python Special Methods

Implement standard Python protocols:

```rust
#[pymethods]
impl PyCurrency {
    fn __repr__(&self) -> String {
        format!("Currency('{}')", self.inner.code())
    }

    fn __str__(&self) -> String {
        self.inner.code().to_string()
    }

    fn __hash__(&self) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.inner.hash(&mut hasher);
        hasher.finish()
    }

    fn __richcmp__(&self, other: &Self, op: pyo3::basic::CompareOp) -> bool {
        match op {
            CompareOp::Eq => self.inner == other.inner,
            CompareOp::Ne => self.inner != other.inner,
            _ => false,
        }
    }
}
```

## WASM Binding Comparison

Both Python and WASM bindings should expose identical functionality:

| Aspect | Python (PyO3) | WASM (wasm-bindgen) |
|--------|---------------|---------------------|
| Wrapper struct | `pub(crate) inner: T` | `pub(crate) inner: T` |
| Constructor | `from_inner(inner: T)` | `from_inner(inner: T)` |
| Error handling | `.map_err(map_error)` | `.map_err(core_to_js)` |
| String parsing | `FromPyObject` trait | `ParseFromString` trait |
| Module structure | Submodules via `register()` | Flat exports in `lib.rs` |

## Rust Core Crates

Bindings wrap these core crates:

| Crate | Purpose |
|-------|---------|
| `finstack_quant_core` | Dates, money, currency, market data, math |
| `finstack_quant_valuations` | Instruments, pricers, metrics, Greeks |
| `finstack_quant_portfolio` | Portfolio management, aggregation |
| `finstack_quant_statements` | Financial statement modeling |
| `finstack_quant_scenarios` | Scenario engine, stress testing |

All computation lives in these crates. Bindings only wrap and expose.
