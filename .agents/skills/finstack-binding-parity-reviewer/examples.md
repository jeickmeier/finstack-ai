# Binding Examples

## Good Patterns

### Example 1: Pure Wrapper with Type Conversion

```rust
// GOOD: Wrapper with flexible argument extraction
#[pyclass(name = "Agent", module = "finstack_ai", frozen)]
pub struct PyAgent {
    pub(crate) inner: Agent,
}

#[pymethods]
impl PyAgent {
    #[new]
    fn new(spec: PyAgentSpec) -> PyResult<Self> {
        Agent::new(spec.inner)
            .map(Self::from_inner)
            .map_err(crate::errors::map_error)
    }

    fn start_run(&self, options: PyStartRunOptions) -> PyResult<PyRun> {
        // Delegates to Rust, maps error - NO LOGIC HERE
        self.inner
            .start_run(options.into_inner())
            .map(PyRun::from_inner)
            .map_err(crate::errors::map_error)
    }

    #[getter]
    fn id(&self) -> String {
        self.inner.id().to_string()
    }
}
```

### Example 2: Flexible Type Extraction

```rust
// GOOD: Accept multiple input types for better ergonomics
pub fn extract_timeout(obj: &Bound<'_, PyAny>) -> PyResult<Duration> {
    if let Ok(ms) = obj.extract::<u64>() {
        return Ok(Duration::from_millis(ms));
    }
    if let Ok(secs) = obj.extract::<f64>() {
        return Ok(Duration::from_secs_f64(secs));
    }
    Err(PyTypeError::new_err("Expected timeout as milliseconds int or seconds float"))
}
```

### Example 3: Error Mapping

```rust
// GOOD: Centralized error handling preserving context
pub fn map_error(e: CoreError) -> PyErr {
    match e {
        CoreError::Configuration { message, source } => {
            let msg = if let Some(src) = source {
                format!("{}: {}", message, src)
            } else {
                message
            };
            ConfigurationError::new_err(msg)
        }
        _ => FinstackError::new_err(e.to_string()),
    }
}
```

## Bad Patterns

### Logic in the binding

```rust
// BAD: Binding decides policy instead of calling Rust
#[pyfunction]
fn start_run_like_python(...) -> PyResult<PyRun> {
    let timeout = if host_default { Duration::from_secs(30) } else { fallback };
    // policy decision belongs in Rust
    finstack_ai::start_run(..., timeout).map_err(map_error)
}
```

**Fix:** Move the defaulting rule into Rust. Binding extracts, calls, maps error.

### Missing stub or export

A Rust public method exists, Python `__init__.py` exports the type, but `_finstack_ai.pyi` omits the method.

**Fix:** Update the stub and public-item inventory in the same slice.

### Divergent names

Rust `start_run`, Python `begin_run`, JS `launchRun`.

**Fix:** Converge on the Rust semantic name with idiomatic case per language (`start_run` / `startRun`).
