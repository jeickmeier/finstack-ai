# Migrating the Python candidate from 0.0.1 to 0.0.2

`0.0.2` was an alpha candidate: breaking changes remained possible before the
public preview, and Python artifacts were staged before cross-binding release.

## What changed

- `Capability` supports `always`, `application`, `model`, and `disabled`
  delivery modes. Pass application-selected IDs through
  `active_capabilities`; model entries are chosen by the shared bounded policy.
- `RunResult.active_capabilities` and `RunResult.trace` expose Rust-owned
  conformance snapshots.
- `Agent.capability_catalog()` and `compact_capability_catalog()` expose the
  compact model catalog without eager activation.
- Pydantic tools and structured outputs, callback ports, typed handles, and the
  supported package matrix are part of the staged Python candidate.

## Unchanged boundaries

- Rust still owns continuation, activation records, validation retry, ordering,
  cancellation, and error semantics.
- Pydantic remains optional and lazy.
- Python callbacks remain trusted in-process code, not plugins or sandboxes.
- The candidate does not claim durable restart or pruning parity.
- A staged artifact or verified keyless signature bundle does not imply PyPI
  publication or a stable API guarantee.
