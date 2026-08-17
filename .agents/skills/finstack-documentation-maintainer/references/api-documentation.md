# Documentation Standards Reference

## Rust documentation conventions

### Module documentation

Every module should have a `//!` doc comment at the top:

```rust
//! Brief module description.
//!
//! Extended description of what the module provides,
//! its main types, and how to use them.
//!
//! # Examples
//!
//! ```rust
//! use crate::module::MainType;
//!
//! let thing = MainType::new();
//! ```
```

### Struct documentation

```rust
/// Brief description of the struct.
///
/// Extended description explaining:
/// - What this type represents
/// - When to use it
/// - Any important invariants
///
/// # Examples
///
/// ```rust
/// let instance = MyStruct::new(value);
/// ```
pub struct MyStruct {
    /// Description of this field.
    pub field1: Type1,
    /// Description with units or constraints.
    /// Value must be non-negative.
    pub field2: f64,
}
```

### Enum documentation

```rust
/// Brief description of the enum.
///
/// Extended description of what choices this enum represents.
pub enum MyEnum {
    /// First variant - when to use it.
    Variant1,
    /// Second variant with associated data.
    ///
    /// The inner value represents...
    Variant2(InnerType),
}
```

### Trait documentation

```rust
/// Brief description of what implementors provide.
///
/// # Required methods
///
/// Implementors must define:
/// - `method1`: for doing X
/// - `method2`: for doing Y
///
/// # Examples
///
/// ```rust
/// struct MyImpl;
///
/// impl MyTrait for MyImpl {
///     fn method1(&self) -> Output {
///         // implementation
///     }
/// }
/// ```
pub trait MyTrait {
    /// Description of this required method.
    fn method1(&self) -> Output;
}
```

### Error handling documentation

```rust
/// Brief description.
///
/// # Errors
///
/// Returns `Err` if:
/// - Input is empty
/// - Required lookup fails
///
/// # Panics
///
/// Panics if `debug_assertions` are enabled and invariant X is violated.
pub fn fallible_function() -> Result<T, Error> {
    // ...
}
```

### Callable input documentation (required)

Every public Rust function, associated function, trait method, and constructor
that accepts a caller-supplied input must have a `# Arguments` section. Use the
exact Rust parameter names and give each entry a substantive description:

```rust
/// Start a run from a resolved agent spec.
///
/// # Arguments
///
/// * `spec` - Resolved agent specification used to construct the run.
/// * `timeout` - Wall-clock deadline; expired runs fail closed.
/// * `cancellation` - Token checked before privileged dispatch.
///
/// # Errors
///
/// Returns an error when the spec is incomplete or the run cannot be authorized.
pub fn start_run(spec: &AgentSpec, timeout: Duration, cancellation: &CancellationToken) -> Result<Run> {
    // ...
}
```

Do not substitute a type repetition (for example, "the input value") for an
explanation. State units and representation for numerical values, accepted
shapes and alignment for collections, and the fallback behavior of `Option`
inputs. Document mutation, ownership, or lookup effects when they are visible
to the caller. Reviewers remain responsible for the semantic accuracy of those
entries.

## Python documentation conventions

### `.pyi` stub completeness

The `.pyi` stub is the primary IDE-facing doc surface (hover, signature help,
mypy), and the Rust source is invisible to Python users. Every public binding
needs a detailed stub docstring, not a one-line summary — even thin wrappers
that delegate to Rust. A complete stub documents:

- a one-line summary,
- every parameter (meaning, units/conventions, length/shape constraints),
- the return value (shape, alignment, units),
- raised exceptions and when they occur,
- behavioral notes: supported `op`/`method` strings, missing-data handling,
  defaults, and any divergence from the Rust API.
- a runnable doctest at module level and for every public class, classmethod,
  and free function. Class examples may cover routine instance accessors.

Name the concrete public exception types from the binding error-conversion
contract and state the condition that raises each type. Do not use generic
"raises an error" language.

Match the docstring flavor already used in the module (NumPy `Parameters`
sections or Google `Args:` sections); do not mix flavors within one module.

Pure-Python binding modules (`.py` files, e.g. host-language adapters like
`_pydantic.py`) have no separate stub; document them to the same bar
directly in their function and class docstrings, since those are the only IDE
surface. Thin re-export shims that only rebind compiled types need just a module
docstring — the symbol docs come from the compiled extension.

### NumPy docstring style

This project uses NumPy-style or Google-style docstrings depending on the
module. Follow the flavor already used in the file you are editing.

### Class documentation

```python
class MyClass:
    """Brief description of the class.

    Extended description explaining:
    - What this type represents
    - When to use it
    - Any important invariants

    Attributes
    ----------
    field1 : Type1
        Description of this attribute.
    field2 : float
        Description with units or constraints.

    Examples
    --------
    >>> obj = MyClass(value1, value2)
    >>> obj.field1
    expected_value
    """

    def __init__(self, field1: Type1, field2: float) -> None:
        """Initialize the instance.

        Parameters
        ----------
        field1 : Type1
            Description of first parameter.
        field2 : float
            Description with constraints (must be non-negative).
        """
```

### Method documentation

```python
def start_run(
    self,
    spec: AgentSpec,
    timeout_ms: int,
) -> Run:
    """Start a run from a resolved agent spec.

    Parameters
    ----------
    spec : AgentSpec
        Resolved agent specification used to construct the run.
    timeout_ms : int
        Wall-clock deadline in milliseconds; expired runs fail closed.

    Returns
    -------
    Run
        The started run handle.

    Raises
    ------
    ConfigurationError
        If the spec is incomplete.
    TimeoutError
        If timeout_ms is negative.

    Examples
    --------
    >>> agent = Agent.from_spec(spec)
    >>> run = agent.start_run(spec, timeout_ms=30_000)
    >>> run.id
    'run_...'
    """
```

## Quality standards

### Description quality

**Good:**
> Start a run from a resolved agent spec and fail closed when the
> deadline or cancellation token is already expired.

**Bad:**
> Start a run.

### Argument documentation quality

**Good:**
```
* `timeout` - Wall-clock deadline; expired runs fail closed
```

**Bad:**
```
* `timeout` - The timeout
```

### Example quality

**Good:**
```rust
/// ```rust
/// use finstack_ai::Agent;
///
/// let agent = Agent::builder()
///     .with_model(model)
///     .with_toolset(tools)
///     .build()?;
///
/// assert!(agent.spec().model().is_some());
/// ```
```

**Bad:**
```rust
/// ```rust
/// let x = my_function();
/// ```
```

## Documenting conventions

When code relies on protocol or lifecycle conventions, document them explicitly:

```rust
/// Commit recoverable effect intent before execution.
///
/// # Conventions
///
/// - Commit happens before privileged dispatch
/// - Duplicate effect IDs are idempotent
/// - Conflicts fail closed
///
/// # Arguments
/// ...
```

## Documenting numerical precision

For numerical code, note precision characteristics:

```rust
/// Encode a bounded label.
///
/// # Numerical notes
///
/// - Maximum length: `LABEL_MAX_BYTES`
/// - Encoding: UTF-8
/// - Empty input is rejected
///
/// # Arguments
/// ...
```
