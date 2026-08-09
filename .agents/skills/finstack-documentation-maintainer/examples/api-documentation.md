# Documentation Examples

## Good Rust documentation

### Fully documented struct

```rust
/// Pricing model selection for the pricer registry.
///
/// Determines which mathematical model is used to price an instrument.
/// Each model has different computational characteristics and accuracy
/// profiles for different instrument types.
///
/// # Model Categories
///
/// ## Analytical Models
/// - [`Discounting`](Self::Discounting): Simple present value discounting
/// - [`Black76`](Self::Black76): Black-76 formula for options
/// - [`Normal`](Self::Normal): Bachelier (normal) model for rate options
///
/// ## Tree Models
/// - [`Tree`](Self::Tree): Binomial/trinomial lattice
/// - [`HullWhite1F`](Self::HullWhite1F): Hull-White one-factor short rate
///
/// # Examples
///
/// ```rust
/// use finstack_quant_valuations::pricer::ModelKey;
///
/// // Select appropriate model for instrument type
/// let model = ModelKey::Discounting;  // For bonds
/// let model = ModelKey::Black76;      // For caps/floors
///
/// // Parse from string
/// let model: ModelKey = "black76".parse().unwrap();
/// assert_eq!(model, ModelKey::Black76);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ModelKey {
    /// Present value discounting of projected cashflows.
    ///
    /// Used for: bonds, swaps, deposits, repos, forwards.
    Discounting = 1,

    /// Black-76 lognormal model for forward-based options.
    ///
    /// Used for: caps, floors, swaptions, FX options, commodity options.
    ///
    /// # References
    ///
    /// - Black (1976): see docs/REFERENCES.md#black1976
    Black76 = 3,

    /// Bachelier (normal) model for rate options.
    ///
    /// Used for: inflation caps/floors, options on rates near zero.
    ///
    /// # References
    ///
    /// - Bachelier (1900): see docs/REFERENCES.md#bachelier1900
    Normal = 6,
}
```

### Fully documented function

```rust
/// Calculate the Black-76 price for a European option on a forward.
///
/// Uses the Black-76 lognormal model to price options on forwards,
/// futures, and other forward-starting underlyings.
///
/// # Arguments
///
/// * `forward` - Forward price of the underlying at expiry
/// * `strike` - Option strike price
/// * `time_to_expiry` - Time to option expiry in years (ACT/365)
/// * `volatility` - Annualized lognormal volatility (e.g., 0.20 for 20%)
/// * `discount_factor` - Discount factor from expiry to valuation date
/// * `option_type` - Call or Put
///
/// # Returns
///
/// Option premium in currency units (same scale as forward/strike).
///
/// # Panics
///
/// Panics if `time_to_expiry` is negative or `volatility` is non-positive.
///
/// # Examples
///
/// ```rust
/// use finstack_quant_core::OptionType;
/// use finstack_quant_valuations::models::black76_price;
///
/// // Price a 1-year ATM call with 20% vol
/// let price = black76_price(
///     100.0,  // forward
///     100.0,  // strike (ATM)
///     1.0,    // 1 year to expiry
///     0.20,   // 20% vol
///     0.95,   // discount factor
///     OptionType::Call,
/// );
/// assert!((price - 7.57).abs() < 0.01);
/// ```
///
/// # References
///
/// - Black (1976): see docs/REFERENCES.md#black1976
/// - Hull: Options, Futures, and Other Derivatives, Ch. 19
pub fn black76_price(
    forward: f64,
    strike: f64,
    time_to_expiry: f64,
    volatility: f64,
    discount_factor: f64,
    option_type: OptionType,
) -> f64 {
    // implementation
}
```

## Bad Rust documentation

### Missing documentation (Blocker)

```rust
// BAD: No documentation at all
pub fn calculate_price(bond: &Bond, market: &Market) -> f64 {
    // ...
}
```

### Incomplete documentation (Major)

```rust
/// Calculate bond price.  // BAD: No arguments, returns, or examples
pub fn calculate_price(bond: &Bond, market: &Market) -> f64 {
    // ...
}
```

### Missing reference (Minor)

```rust
/// Calculate option price using Black-76.
///
/// # Arguments
/// * `forward` - Forward price
/// * `strike` - Strike price
/// ...
///
/// # Returns
/// Option premium.
// BAD: Missing reference to Black (1976)
pub fn black76_price(...) -> f64 {
    // ...
}
```

## Good Python documentation

### Fully documented class

```python
class Bond:
    """Fixed-rate bond instrument for pricing and risk calculations.

    Represents a bullet or amortizing fixed-rate bond with regular
    coupon payments. Supports various day count conventions and
    business day adjustment rules.

    Attributes
    ----------
    id : str
        Unique identifier for the bond.
    currency : Currency
        Settlement currency.
    notional : float
        Face value of the bond.
    coupon_rate : float
        Annual coupon rate as a decimal (e.g., 0.05 for 5%).
    maturity : date
        Final maturity date.
    day_count : DayCount
        Day count convention for accrual calculations.

    Examples
    --------
    >>> from finstack_quant import Bond, Currency, DayCount
    >>> bond = Bond(
    ...     id="BOND-001",
    ...     currency=Currency.USD,
    ...     notional=1_000_000.0,
    ...     coupon_rate=0.05,
    ...     maturity=date(2030, 6, 15),
    ...     day_count=DayCount.THIRTY_360,
    ... )
    >>> bond.coupon_rate
    0.05

    Sources
    -------
    - ISDA 2006 Definitions: see docs/REFERENCES.md#isda2006Definitions
    """
```

### Fully documented method

```python
def price(
    self,
    market: MarketContext,
    settlement: date | None = None,
) -> float:
    """Calculate the clean price of the bond.

    Computes the present value of future cashflows discounted using
    the discount curve from the market context, then subtracts
    accrued interest to get the clean price.

    Parameters
    ----------
    market : MarketContext
        Market data context containing discount curves.
    settlement : date, optional
        Settlement date for the calculation. Defaults to market
        valuation date if not provided.

    Returns
    -------
    float
        Clean price as a percentage of par (e.g., 98.5 for 98.5%).

    Raises
    ------
    KeyError
        If required discount curve is not in market context.
    ValueError
        If settlement date is after maturity.

    Examples
    --------
    >>> market = MarketContext(valuation_date=date(2024, 1, 15))
    >>> market.add_curve("USD-SOFR", sofr_curve)
    >>> price = bond.price(market)
    >>> round(price, 4)
    101.2345

    Notes
    -----
    Clean price excludes accrued interest. For dirty price (including
    accrued), use :meth:`dirty_price`.

    Sources
    -------
    - Fabozzi: Fixed Income Analysis, Ch. 5
    """
```

## Bad Python documentation

### Missing documentation (Blocker)

```python
# BAD: No docstring
def calculate_pv(cashflows, curve):
    return sum(cf.amount * curve.df(cf.date) for cf in cashflows)
```

### Incomplete documentation (Major)

```python
def calculate_pv(cashflows, curve):
    """Calculate present value."""  # BAD: No params, returns, examples
    return sum(cf.amount * curve.df(cf.date) for cf in cashflows)
```

### Vague description (Minor)

```python
def process_data(data):
    """Process the data.  # BAD: What data? What processing?

    Parameters
    ----------
    data : list
        The data.  # BAD: What kind of data?

    Returns
    -------
    result
        The result.  # BAD: What result?
    """
```

## Fixing documentation

### Before (bad)

```rust
pub struct BondPricer;

impl BondPricer {
    pub fn price(&self, bond: &Bond, market: &Market) -> f64 {
        // ...
    }
}
```

### After (good)

```rust
/// Pricer for fixed-rate bonds using discounted cashflow methodology.
///
/// Prices bonds by projecting future cashflows and discounting them
/// using the appropriate discount curve from the market context.
///
/// # Examples
///
/// ```rust
/// use finstack_quant_valuations::{BondPricer, Bond, MarketContext};
///
/// let pricer = BondPricer;
/// let bond = Bond::new(/* ... */);
/// let market = MarketContext::new(/* ... */);
///
/// let pv = pricer.price(&bond, &market);
/// ```
pub struct BondPricer;

impl BondPricer {
    /// Calculate the present value of a bond.
    ///
    /// Projects all future cashflows (coupons and principal) and
    /// discounts them using the discount curve from the market context.
    ///
    /// # Arguments
    ///
    /// * `bond` - The bond instrument to price
    /// * `market` - Market context with discount curves
    ///
    /// # Returns
    ///
    /// Present value in the bond's currency.
    ///
    /// # Errors
    ///
    /// Returns `PricingError::MissingMarketData` if the required
    /// discount curve is not present in the market context.
    pub fn price(&self, bond: &Bond, market: &Market) -> f64 {
        // ...
    }
}
```
