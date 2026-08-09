# Mise format task design

Date: 2026-08-09  
Status: approved for implementation  
Approach: direct aggregate commands

## Goal

Make `mise run format` actively format every language currently implemented in
the repository.

## Design

The existing `format` task remains the single contributor-facing aggregate. It
runs:

1. `cargo fmt --all` for the Rust workspace.
2. Pinned Ruff through `uv run --no-project` with `ruff format .` for all Python
   sources, including repository tooling and the Python binding.

TypeScript formatting is deferred until the repository contains a TypeScript
package and formatter configuration. At that point, the package's canonical
format command will be appended to this aggregate. This avoids introducing
unused Node or formatter infrastructure before the planned TypeScript surface
exists.

The specialized `format-benchmark`, `format-architecture`, `format-ci`, and
`format-schema-governance` tasks remain check-only validation tasks. Their
behavior does not change.

## Failure behavior

The aggregate uses strict shell error handling and stops on the first formatter
failure. Both formatters may modify tracked source files in place.

The local and hosted CI aggregates continue to invoke this write-mode task.
They intentionally format drift rather than reject it; no separate
`format-check` task is added.

## Verification

- Run `mise run format`.
- Run `git diff --check`.
- Confirm the task description states that it formats Rust and Python sources.
- Confirm no TypeScript tooling is added before TypeScript sources or package
  configuration exist.
