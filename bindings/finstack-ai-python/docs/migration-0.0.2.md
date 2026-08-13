# Migrating the Python candidate from 0.0.1 to 0.0.2

`0.0.2` is still alpha: breaking changes remain possible before public preview.
The Python half is staged in PR-032; the exact cross-binding checkpoint is not
cut until browser WASM conformance and G4 pass in PR-038.

## What changed

- `Capability` supports `always`, `application`, `model`, and `disabled`
  delivery modes. Pass application-selected IDs through
  `active_capabilities`; model entries are chosen by the shared bounded policy.
- `RunResult.active_capabilities` and `RunResult.trace` expose Rust-owned
  conformance snapshots.
- `Agent.capability_catalog()` and `compact_capability_catalog()` expose the
  compact model catalog without eager activation.
- Pydantic tools/structured outputs, callback ports, typed handles, and package
  matrix from PR-027 through PR-031 are now part of the staged Python candidate.

## Unchanged boundaries

- Rust still owns continuation, activation records, validation retry, ordering,
  cancellation, and error semantics.
- Pydantic remains optional and lazy.
- Python callbacks remain trusted in-process code, not plugins or sandboxes.
- No durable restart/pruning parity is claimed before PR-048.
- No PyPI publication, stable API guarantee, or G4 passage is implied by a
  staged artifact or signed attestation.
