# ADR-043 confinement review (Partial)

Date: 2026-08-17  
Reviewer: local gap-fix G6  
Status: Partial — no invented review or evidence id

This file records a source review of the internal `ProcessConfinement`
service against ADR-043 and the mapped threat-model rows. It is not a
Verified security review and does not invent a review identifier.

## Scope

- Service: `crates/finstack-ai-runtime/src/services/process_confinement.rs`
- Consumers: shell `try_with_confinement`; MCP optional `stdio_confined`
- Controls: TM-03, TM-06, SEC-INV-006, SEC-INV-012

## Findings

- Fail-closed remains: missing backends never spawn. The labeled
  unconfined `std::process` path stays a separate T1 runner.
- Linux: Landlock + `no_new_privs` in the child `pre_exec`.
- macOS: Seatbelt stays allow-default (deny-default still aborts under
  Darwin + dyld). Added denies for `network*`, `process-exec` except the
  allowlisted binary, and `file-write*` outside the capability root,
  then re-allow the root. Existing user-writable `file-read-data` denies
  remain. Apple deprecation is unchanged.
- Windows: `CreateProcessAsUser` starts the child under the restricted
  token; the Job Object remains assigned. Missing token, job, or
  `CreateProcessAsUser` privilege fails closed. `ConfinedChild` owns the
  process and job handles because `std::process::Child` has no supported
  wrap from `PROCESS_INFORMATION`.
- MCP stdio: optional `StdioConfig::with_confinement` /
  `McpConfig::stdio_confined` calls `ProcessConfinement::spawn` on every
  platform, including Windows token+job. Requested-and-unavailable fails
  closed. Unconfined stdio stays T1.

## Residuals

- Seatbelt deprecation / Darwin deny-default abort
- No isolation-label upgrade (T1 unchanged)
- `CreateProcessAsUser` still requires `SeAssignPrimaryTokenPrivilege`;
  hosts without that privilege fail closed rather than spawning
  Job-Object-only.
