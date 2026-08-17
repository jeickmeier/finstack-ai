# Python Binding Reference

## Codebase Structure

```
bindings/finstack-ai-python/
├── src/                    # Rust binding code (PyO3), if present
│   ├── lib.rs             # Main entry, module registration
│   └── errors.rs          # Exception hierarchy, error mapping
└── python/finstack_ai/    # Python package
    ├── __init__.py        # Package exports
    ├── _finstack_ai.pyi   # IDE-facing stubs
    └── _pydantic.py       # Host-language ergonomic adapters
```

## Standard Patterns

### 1. Wrapper Struct Pattern

Every Rust type exposed to Python follows this pattern:

```rust
use pyo3::prelude::*;
use finstack_ai::Agent;

#[pyclass(name = "Agent", module = "finstack_ai", frozen)]
pub struct PyAgent {
    pub(crate) inner: Agent,  // Always named "inner"
}

impl PyAgent {
    /// Internal constructor - used by other bindings
    pub(crate) fn from_inner(inner: Agent) -> Self {
        Self { inner }
    }
}

#[pymethods]
impl PyAgent {
    /// Python constructor
    #[new]
    fn new(spec: PyAgentSpec) -> PyResult<Self> {
        Agent::new(spec.inner)
            .map(Self::from_inner)
            .map_err(crate::errors::map_error)
    }

    /// Getter - just exposes Rust data
    #[getter]
    fn id(&self) -> String {
        self.inner.id().to_string()
    }

    /// Method - delegates to Rust, maps error
    fn start_run(&self, options: PyStartRunOptions) -> PyResult<PyRun> {
        self.inner
            .start_run(options.into_inner())
            .map(PyRun::from_inner)
            .map_err(crate::errors::map_error)
    }
}
```

### 2. Flexible Argument Extraction

Accept multiple Python types for better ergonomics:

```rust
use pyo3::prelude::*;

/// Wrapper for flexible timeout argument
pub struct TimeoutArg(pub Duration);

impl<'py> FromPyObject<'py> for TimeoutArg {
    fn extract_bound(obj: &Bound<'py, PyAny>) -> PyResult<Self> {
        if let Ok(ms) = obj.extract::<u64>() {
            return Ok(TimeoutArg(Duration::from_millis(ms)));
        }
        if let Ok(secs) = obj.extract::<f64>() {
            return Ok(TimeoutArg(Duration::from_secs_f64(secs)));
        }
        Err(PyTypeError::new_err("Expected timeout as milliseconds int or seconds float"))
    }
}
```

### 3. Error Mapping

Centralized error conversion:

```rust
use pyo3::prelude::*;
use finstack_ai_kernel::Error as CoreError;

pyo3::create_exception!(finstack_ai, FinstackError, PyException);
pyo3::create_exception!(finstack_ai, ConfigurationError, FinstackError);

pub fn map_error(e: CoreError) -> PyErr {
    match e {
        CoreError::Configuration(msg) => ConfigurationError::new_err(msg),
        _ => FinstackError::new_err(e.to_string()),
    }
}
```

Preserve stable error `code` strings from Rust. Do not invent a parallel code namespace in the binding.

### 4. Module Registration

Keep package exports stable:

```rust
pub fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
    parent.add_class::<PyAgent>()?;
    parent.add_class::<PySession>()?;
    parent.add_class::<PyRun>()?;
    parent.setattr("__all__", vec!["Agent", "Session", "Run"])?;
    Ok(())
}
```

### 5. Builder Pattern

For complex objects with many optional parameters, expose the same builder shape as Rust. Do not add binding-only required fields.

### 6. Python Special Methods

Implement standard Python protocols (`__repr__`, `__str__`, `__hash__`, `__richcmp__`) by delegating to Rust. Do not reimplement equality in the binding.

## WASM Binding Comparison

Both Python and WASM bindings should expose identical functionality:

| Aspect | Python (PyO3) | WASM (wasm-bindgen) |
|--------|---------------|---------------------|
| Wrapper struct | `pub(crate) inner: T` | `pub(crate) inner: T` |
| Constructor | `from_inner(inner: T)` | `from_inner(inner: T)` |
| Error handling | `.map_err(map_error)` | `.map_err(core_to_js)` |
| Naming | `snake_case` | `camelCase` via `js_name` |
| Facade | `__init__.py` + `.pyi` | `js/src/` over generated glue |

## Rust Core Crates

Bindings wrap these core crates:

| Crate | Purpose |
|-------|---------|
| `finstack-ai-kernel` | Deterministic state, records, events, effects |
| `finstack-ai-runtime` | Ports and effect execution |
| `finstack-ai` | SDK composition and public APIs |
| `finstack-ai-protocol` | Codecs and journal frames |

All domain decisions live in these crates. Bindings only wrap and expose.
