# Established Codebase Conventions

Reference of patterns observed across the finstack-quant codebase. When reviewing for consistency, deviations from these patterns are candidates for findings.

## Rust Conventions

### Error Handling

| Convention | Example | Crates Using |
|-----------|---------|--------------|
| `thiserror::Error` derive | `#[derive(Error, Debug)]` | All crates |
| `#[non_exhaustive]` on error enums | `#[non_exhaustive] pub enum Error` | All crates |
| `pub type Result<T>` alias | `pub type Result<T> = std::result::Result<T, Error>` | All crates |
| `#[from]` for wrapped errors | `Core(#[from] finstack_quant_core::Error)` | All crates |
| Helper constructors on errors | `Error::missing_curve_with_suggestions(...)` | core (partial) |

**Deviation to watch:** Not all error variants have helper constructors. Only core crate uses them extensively.

### Builder Pattern

| Convention | Dominant Pattern | Deviation |
|-----------|-----------------|-----------|
| Entry point (ID-required) | `Type::builder(id)` | Curve builders, `Schedule::builder(start, end)` |
| Entry point (no required ID) | `Type::builder()` | Via `#[derive(FinancialBuilder)]` for instruments |
| Setter prefix | Bare name (`base_date()`, `knots()`) | `set_` prefix only on mutators (not builders) |
| Terminal method | `.build()` | See Terminal Methods section below |
| Return type | `Result<T>` from `.build()` | `T` directly for simple aggregators (see below) |

**Notes:**
- `Builder::new(...)` is acceptable as an internal constructor, but `Type::builder(...)` should be the public-facing entry point.
- Return type `T` (not `Result<T>`) is acceptable for simple aggregators without validation: `InstrumentCurvesBuilder`, `EquityInstrumentDepsBuilder`, `WaterfallBuilder`, `PayoffBuilder`.

### Terminal Methods

| Method | Where Used | Reason |
|--------|-----------|--------|
| `.build()` | All standard builders | Canonical terminal method |
| `.build_plan(exprs, meta)` | `DagBuilder` | Takes extra parameters; builder is reusable (`&mut self`) |
| `.build_with_curves(curves)` | `CashFlowBuilder` | Convenience overload; `.build()` delegates to `build_with_curves(None)` |

### Trait Naming

| Category | Form | Examples |
|----------|------|---------|
| Market data operations | Gerund | `Discounting`, `Forward`, `Survival` |
| Entity types | Noun | `Instrument`, `Solver`, `TermStructure`, `Payoff` |
| Capabilities | Adjective (`-able`) | `Discountable`, `Bumpable` |

### Trait Bounds

| Trait Category | Expected Bounds | Notes |
|---------------|----------------|-------|
| Public domain traits | `Send + Sync` | `Instrument`, `Pricer`, `TermStructure` |
| Extension traits | None required | `DateExt`, `OffsetDateTimeExt` |
| Provider traits | `Send + Sync` | `FxProvider` |

### Module Organization

| Crate Size | Pattern | Example |
|-----------|---------|---------|
| Large module (>3 submodules) | `mod.rs` with submodules | `core/src/dates/mod.rs` |
| Small module (<3 submodules) | Single file | `portfolio/src/error.rs` |
| Shared code | `common/` subdirectory | `instruments/common/`, `valuations/common/` |

### Re-exports

| Pattern | Where Used | Notes |
|---------|-----------|-------|
| `prelude.rs` | core, valuations | Common imports for downstream users |
| Root `pub use` | core `lib.rs` | `HashMap`, `HashSet`, error types |
| No re-exports | Some internal crates | Access via full module path |

## Python Binding Conventions

| Convention | Pattern | Example |
|-----------|---------|---------|
| Wrapper naming | `Py{Type}` | `PyCurrency`, `PyMoney`, `PyDiscountCurve` |
| Module registration | `pub(crate) fn register()` | Every submodule has one |
| Argument parsing | Centralized `*Arg` types | `CurrencyArg` in `common/args.rs` |
| Error mapping | `map_error()` function | `finstack-quant-py/src/errors.rs` |
| Inner field | `pub(crate) inner: RustType` | Consistent across all wrappers |

## WASM Binding Conventions

| Convention | Pattern | Example |
|-----------|---------|---------|
| Wrapper naming | `Js{Type}` | `JsCurrency`, `JsMoney`, `JsBond` |
| JS function naming | `camelCase` via `js_name` | `createStandardRegistry` |
| JS type naming | `PascalCase` via `js_name` | `Bond`, `Currency` |
| Wrapper trait | `InstrumentWrapper` | Consistent `from_inner()` / `inner()` |
| Inner field | `pub(crate) inner: RustType` | Matches Python pattern |

## Naming Patterns

### Financial Domain Terms

When the same concept appears in multiple places, use the same term:

| Concept | Canonical Term | Avoid |
|---------|---------------|-------|
| Interest rate curve | `discount_curve` | `rate_curve`, `yield_curve` (unless specific) |
| Credit risk curve | `hazard_curve` | `default_curve`, `credit_curve` |
| Instrument identifier | `InstrumentId` | `instrument_id` (as a String) |
| Curve identifier | `CurveId` | `curve_id` (as a String) |
| Valuation date | `val_date` | `pricing_date`, `as_of_date` (pick one per context) |

### Constants

| Scope | Style | Example |
|-------|-------|---------|
| `pub const` | `SCREAMING_SNAKE_CASE` | `DEFAULT_MIN_FORWARD_TENOR` |
| Module-level private | `SCREAMING_SNAKE_CASE` (preferred) | Mixed in practice |
| Function-level | `snake_case` | Local computation constants |

## Known Intentional Deviations

Document any places where divergence from the dominant pattern is intentional:

### Error & Module Structure
- `error/mod.rs` in core: Uses subdirectory because error module has `inputs.rs` and `suggestions.rs` submodules (justified by size). Valuations uses flat `error.rs` as a re-export facade.
- `prelude.rs` only in core/valuations: Other crates are too small to benefit from a prelude.

### Debug & Display
- `MarketContext` has a manual `Debug` impl that shows collection sizes instead of full contents -- intentional to avoid dumping large data structures in debug output.

### Serde Qualification
- Both `serde::Serialize` (fully qualified in derives) and `Serialize` (after `use serde::{Serialize, Deserialize}` import) are acceptable. No standardization required.

### Instrument Module Structure
- `parameters.rs` is only present for instruments with multiple pricing models or complex configuration (credit derivatives, options, swaptions). Simpler instruments embed parameters in `types.rs`.
- `pricing/` subdirectory vs `pricer.rs`: Use a directory when an instrument has 3+ distinct pricing engines; use a single `pricer.rs` file otherwise.

### Binding Names
- `CDSTranche`/`CDSOption` Rust struct names use all-caps `CDS` prefix (matching `CDSIndex` and `InstrumentType` enum variants). Python/JS binding names are preserved as `CdsTranche`/`CdsOption` for backward compatibility.

### Documentation
- Module READMEs: valuations has more READMEs than core; core uses inline doc comments instead. Both approaches are acceptable.
