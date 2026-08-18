# PR-086 execution plan

Date: 2026-08-18
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.27 / PLAN-0.25

This file is the execution contract for PR-086 (draft W1 / PR-081).
PR-085 remains the authorization slice. This file does not admit
PR-087–PR-098 code.

## Purpose

Make large-output shell runs fail closed instead of hanging, walk cwd
through the retained root fd, and switch to shared `verify_authority`.

## Principal changes

- Drain both pipes before the wait loop over `mpsc::sync_channel` pumps.
  Crossing `max_output_bytes` kills the child and returns
  `SHELL_LIMIT_EXCEEDED` immediately.
- `authorize_cwd` validates the relative string only. `run_process`
  does the per-component `openat` walk from the retained root fd, then
  `fchdir`. No fd is threaded through public `SandboxedCommand`.
- Shared `finstack_ai_runtime::verify_authority`.
- `check_toolset_conformance` for the published echo path.

## Acceptance mapping

- PR-086-A01: 1 MiB stdout returns `SHELL_LIMIT_EXCEEDED` (or a staged
  artifact) without the historical 5 s hang. The existing 8 KiB flood
  case stays green.
- PR-086-A02: A cwd test with a symlinked intermediate component is
  denied; a real nested cwd is accepted.
- PR-086-A03: The crate has `check_toolset_conformance`.

## §18 / TM-03

High-privilege argv battery. Intermediate cwd components are opened
with `O_NOFOLLOW|O_DIRECTORY` from the retained root fd so a symlink
cannot escape the authorized tree. Output is capped during the wait
loop so a flooding child cannot stall the runtime worker. Shared
`verify_authority` rejects a principal whose tenant scope disagrees
with the committed locator. Residual: Windows cwd expansion remains
out of scope.

## Explicit exclusions

Windows cwd expansion. e2b lifecycle. Gateway deletion. PR-087–PR-098.

## Compatibility class

Pre-1.0 toolset behavior. Authority mismatch now uses
`TOOL_POLICY_DENIED` from the shared helper.

## Dependencies

PR-085.
