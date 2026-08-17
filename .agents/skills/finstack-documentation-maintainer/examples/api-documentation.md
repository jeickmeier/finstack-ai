# Documentation Examples

## Good Rust documentation

### Fully documented enum

```rust
/// Capability advertised by a resolved agent.
///
/// Distinguishes host-provided capabilities from guest-plugin
/// capabilities. Use this when inspecting what a run may invoke.
///
/// # Examples
///
/// ```rust
/// use finstack_ai::Capability;
///
/// let capability = Capability::Toolset;
/// assert_eq!(capability.to_string(), "toolset");
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Capability {
    /// Model invocation through the model port.
    Model,
    /// Tool invocation through the toolset port.
    Toolset,
    /// Context retrieval through the context port.
    Context,
}
```

### Fully documented function

```rust
/// Start a run from a resolved agent spec.
///
/// # Arguments
///
/// * `spec` - Resolved agent specification used to construct the run.
/// * `timeout` - Wall-clock deadline; expired runs fail closed.
///
/// # Errors
///
/// Returns an error when the spec is incomplete or the run cannot be authorized.
///
/// # Examples
///
/// ```rust
/// use finstack_ai::{Agent, StartRunOptions};
///
/// let agent = Agent::from_spec(spec)?;
/// let run = agent.start_run(StartRunOptions::default())?;
/// assert!(!run.id().as_str().is_empty());
/// ```
pub fn start_run(spec: &AgentSpec, timeout: Duration) -> Result<Run, Error> {
    // implementation
}
```

## Bad Rust documentation

### Missing documentation (Blocker)

```rust
// BAD: No documentation at all
pub fn start_run(spec: &AgentSpec, timeout: Duration) -> Result<Run, Error> {
    // ...
}
```

### Incomplete documentation (Major)

```rust
/// Start a run.  // BAD: No arguments, returns, or examples
pub fn start_run(spec: &AgentSpec, timeout: Duration) -> Result<Run, Error> {
    // ...
}
```

## Good Python documentation

```python
class Agent:
    """Resolved agent handle for starting runs.

    Parameters
    ----------
    spec : AgentSpec
        Resolved agent specification.

    Examples
    --------
    >>> agent = Agent.from_spec(spec)
    >>> run = agent.start_run(timeout_ms=30_000)
    >>> run.id
    'run_...'
    """
```

## Bad Python documentation

```python
class Agent:
    """An agent."""  # BAD: one-line summary only
```
