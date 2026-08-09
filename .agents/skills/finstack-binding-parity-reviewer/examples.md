# Python Binding Examples

## Good Patterns

### Example 1: Pure Wrapper with Type Conversion

```rust
// GOOD: Wrapper with flexible argument extraction
#[pyclass(name = "Money", module = "finstack_quant.core.money", frozen)]
pub struct PyMoney {
    pub(crate) inner: Money,
}

#[pymethods]
impl PyMoney {
    #[new]
    fn new(amount: f64, currency: CurrencyArg) -> Self {
        Self {
            inner: Money::new(amount, currency.0),
        }
    }

    /// Converts to target currency using FX rates
    fn convert(&self, target: CurrencyArg, fx: &PyFxMatrix) -> PyResult<Self> {
        // Delegates to Rust, maps error - NO LOGIC HERE
        self.inner
            .convert(target.0, &fx.inner)
            .map(Self::from_inner)
            .map_err(crate::errors::map_error)
    }

    #[getter]
    fn amount(&self) -> f64 {
        self.inner.amount()  // Just expose data
    }
}
```

### Example 2: Flexible Type Extraction

```rust
// GOOD: Accept multiple input types for better ergonomics
pub fn extract_float_pairs(obj: &Bound<'_, PyAny>) -> PyResult<Vec<(f64, f64)>> {
    // Try 1: Direct list of tuples
    if let Ok(vec) = obj.extract::<Vec<(f64, f64)>>() {
        return Ok(vec);
    }

    // Try 2: Dict {float: float}
    if let Ok(dict) = obj.downcast::<PyDict>() {
        let pairs: Vec<_> = dict
            .iter()
            .map(|(k, v)| Ok((k.extract()?, v.extract()?)))
            .collect::<PyResult<_>>()?;
        return Ok(pairs);
    }

    // Try 3: Pandas Series
    if let Ok(series) = obj.getattr("items") {
        // ... pandas handling
    }

    Err(PyTypeError::new_err("Expected list of pairs, dict, or pandas Series"))
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
        CoreError::Calibration { instrument, reason } => {
            CalibrationError::new_err(format!(
                "Calibration failed for {}: {}", instrument, reason
            ))
        }
        // ... other variants
    }
}
```

### Example 4: Ergonomic Python Helper (Acceptable)

```python
# finstack-quant/core/expr_helpers.py
# ACCEPTABLE: Operator overloading for ergonomics only


class ExprWrapper:
    """Wraps Expr to enable Python operator overloading."""

    def __init__(self, expr):
        self._expr = expr  # Rust Expr object

    def __add__(self, other):
        if isinstance(other, ExprWrapper):
            return ExprWrapper(self._expr.add(other._expr))
        return ExprWrapper(self._expr.add_scalar(float(other)))

    def __mul__(self, other):
        # Same pattern - all actual computation in Rust
        ...
```

## Anti-Patterns and Fixes

### Anti-Pattern 1: Business Logic in Binding

```rust
// BAD: Yield calculation implemented in binding
#[pymethods]
impl PyBond {
    fn yield_to_maturity(&self, price: f64) -> PyResult<f64> {
        // Newton-Raphson solver - THIS IS BUSINESS LOGIC
        let mut y = 0.05;
        let tolerance = 1e-10;

        for _ in 0..100 {
            let pv = self.calculate_pv(y);
            let dpv = self.calculate_duration(y);

            let diff = pv - price;
            if diff.abs() < tolerance {
                return Ok(y);
            }
            y = y - diff / dpv;
        }

        Err(ComputationError::new_err("Failed to converge"))
    }
}
```

**FIX**: Move to Rust core:

```rust
// In finstack_quant_valuations/src/instruments/bond.rs
impl Bond {
    pub fn yield_to_maturity(&self, price: f64) -> Result<f64, Error> {
        // Newton-Raphson solver in Rust core
        solver::find_yield(self, price, SolverConfig::default())
    }
}

// In finstack-quant-py/src/valuations/bond.rs
#[pymethods]
impl PyBond {
    fn yield_to_maturity(&self, price: f64) -> PyResult<f64> {
        // Just delegate to Rust
        self.inner.yield_to_maturity(price).map_err(map_error)
    }
}
```

### Anti-Pattern 2: Validation in Binding

```rust
// BAD: Validation logic in Python binding
#[pymethods]
impl PySwap {
    #[new]
    fn new(notional: f64, fixed_rate: f64, tenor: &str) -> PyResult<Self> {
        // These checks should be in Rust
        if notional <= 0.0 {
            return Err(PyValueError::new_err("Notional must be positive"));
        }
        if fixed_rate < -0.10 || fixed_rate > 0.50 {
            return Err(PyValueError::new_err("Rate out of reasonable range"));
        }
        if !["1Y", "2Y", "5Y", "10Y", "30Y"].contains(&tenor) {
            return Err(PyValueError::new_err("Invalid tenor"));
        }

        let inner = Swap::new(notional, fixed_rate, Tenor::parse(tenor)?);
        Ok(Self { inner })
    }
}
```

**FIX**: Move validation to Rust constructor:

```rust
// In finstack_quant_valuations/src/instruments/swap.rs
impl Swap {
    pub fn new(notional: f64, fixed_rate: f64, tenor: Tenor) -> Result<Self, Error> {
        if notional <= 0.0 {
            return Err(Error::validation("Notional must be positive"));
        }
        if fixed_rate < -0.10 || fixed_rate > 0.50 {
            return Err(Error::validation("Rate out of reasonable range"));
        }
        // ... construct swap
    }
}

// In finstack-quant-py/src/valuations/swap.rs
#[pymethods]
impl PySwap {
    #[new]
    fn new(notional: f64, fixed_rate: f64, tenor: TenorArg) -> PyResult<Self> {
        // Just delegate - Rust handles validation
        Swap::new(notional, fixed_rate, tenor.0)
            .map(Self::from_inner)
            .map_err(map_error)
    }
}
```

### Anti-Pattern 3: Data Transformation

```rust
// BAD: Aggregation logic in binding
#[pymethods]
impl PyPortfolio {
    fn exposure_by_currency(&self) -> HashMap<String, f64> {
        // Aggregation should be in Rust
        let mut result = HashMap::new();
        for position in self.inner.positions() {
            let ccy = position.currency().code().to_string();
            let value = position.market_value();
            *result.entry(ccy).or_insert(0.0) += value;
        }
        result
    }
}
```

**FIX**: Add method to Rust core:

```rust
// In finstack_quant_portfolio/src/portfolio.rs
impl Portfolio {
    pub fn exposure_by_currency(&self) -> HashMap<Currency, f64> {
        self.positions
            .iter()
            .fold(HashMap::new(), |mut acc, pos| {
                *acc.entry(pos.currency()).or_insert(0.0) += pos.market_value();
                acc
            })
    }
}

// In finstack-quant-py/src/portfolio/portfolio.rs
#[pymethods]
impl PyPortfolio {
    fn exposure_by_currency(&self) -> HashMap<String, f64> {
        // Convert Currency keys to strings for Python
        self.inner
            .exposure_by_currency()
            .into_iter()
            .map(|(ccy, val)| (ccy.code().to_string(), val))
            .collect()
    }
}
```

### Anti-Pattern 4: Algorithm in Binding

```rust
// BAD: Interpolation algorithm in binding
#[pymethods]
impl PyCurve {
    fn rate_at(&self, tenor: f64) -> f64 {
        // Linear interpolation - should be in Rust
        let pillars = self.inner.pillars();

        for i in 0..pillars.len() - 1 {
            let (t0, r0) = pillars[i];
            let (t1, r1) = pillars[i + 1];

            if tenor >= t0 && tenor <= t1 {
                let w = (tenor - t0) / (t1 - t0);
                return r0 * (1.0 - w) + r1 * w;
            }
        }

        // Flat extrapolation
        if tenor < pillars[0].0 {
            pillars[0].1
        } else {
            pillars.last().unwrap().1
        }
    }
}
```

**FIX**: Use Rust interpolation:

```rust
// In finstack_quant_core/src/market_data/curve.rs
impl Curve {
    pub fn rate_at(&self, tenor: f64) -> Result<f64, Error> {
        self.interpolator.interpolate(tenor)
    }
}

// In finstack-quant-py/src/bindings/core/market_data/curve.rs
#[pymethods]
impl PyCurve {
    fn rate_at(&self, tenor: f64) -> PyResult<f64> {
        self.inner.rate_at(tenor).map_err(map_error)
    }
}
```

### Anti-Pattern 5: Conditional Logic Based on Values

```rust
// BAD: Conditional logic based on instrument type
#[pymethods]
impl PyPricer {
    fn price(&self, instrument: &PyAny) -> PyResult<f64> {
        // Type-based dispatch should be in Rust
        if let Ok(bond) = instrument.extract::<PyRef<PyBond>>() {
            // Bond pricing logic
            let cashflows = bond.inner.cashflows();
            let mut pv = 0.0;
            for cf in cashflows {
                pv += cf.amount * self.discount(cf.date);
            }
            return Ok(pv);
        }

        if let Ok(swap) = instrument.extract::<PyRef<PySwap>>() {
            // Swap pricing logic
            // ...
        }

        Err(PyTypeError::new_err("Unsupported instrument"))
    }
}
```

**FIX**: Use Rust pricer with trait dispatch:

```rust
// In finstack_quant_valuations/src/pricer.rs
impl Pricer {
    pub fn price(&self, instrument: &dyn Priceable) -> Result<f64, Error> {
        instrument.price(self.market_data())
    }
}

// In finstack-quant-py/src/valuations/pricer.rs
#[pymethods]
impl PyPricer {
    fn price_bond(&self, bond: &PyBond) -> PyResult<f64> {
        self.inner.price(&bond.inner).map_err(map_error)
    }

    fn price_swap(&self, swap: &PySwap) -> PyResult<f64> {
        self.inner.price(&swap.inner).map_err(map_error)
    }
}
```

## WASM Parity Check Examples

When reviewing, verify Python implementations can be trivially replicated in WASM:

### Good: Trivial WASM Equivalent

```rust
// Python binding
#[pymethods]
impl PyBond {
    fn clean_price(&self, market_data: &PyMarketData) -> PyResult<f64> {
        self.inner.clean_price(&market_data.inner).map_err(map_error)
    }
}

// WASM equivalent - trivial to write
#[wasm_bindgen]
impl JsBond {
    pub fn clean_price(&self, market_data: &JsMarketData) -> Result<f64, JsValue> {
        self.inner.clean_price(&market_data.inner).map_err(core_to_js)
    }
}
```

### Bad: WASM Would Need Reimplementation

```rust
// Python binding with logic
#[pymethods]
impl PyBond {
    fn clean_price(&self, market_data: &PyMarketData) -> PyResult<f64> {
        let dirty = self.inner.dirty_price(&market_data.inner)?;
        let accrued = self.calculate_accrued_interest()?;  // <-- Logic here
        Ok(dirty - accrued)
    }

    fn calculate_accrued_interest(&self) -> PyResult<f64> {
        // Accrued interest calculation - should be in Rust
        let days = (today() - self.inner.last_coupon_date()).days();
        let period_days = self.inner.coupon_frequency().days();
        Ok(self.inner.coupon_rate() * self.inner.notional() * (days as f64 / period_days as f64))
    }
}

// WASM would need to reimplement calculate_accrued_interest()!
```
