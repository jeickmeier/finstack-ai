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
- Windows: restricted-token *creation* is still required so a host
  without that primitive fails closed. The live spawn boundary is still
  the Job Object. `CreateProcessAsUser` so the child actually runs under
  the restricted token is not applied in this cut: stable `std::process::Child`
  has no supported wrap from `PROCESS_INFORMATION`. Residual stays
  documented; do not claim the token is the running process token.
- MCP stdio: optional `StdioConfig::with_confinement` /
  `McpConfig::stdio_confined`. Unix backends apply `pre_exec` via
  `ProcessConfinement::configure` then `tokio::process` spawn.
  Requested-and-unavailable fails closed. Unconfined stdio stays T1.
  Windows `configure` fails closed (token cannot attach to a later tokio
  spawn); confined MCP stdio is unix-only in this cut.

## Residuals

- Windows token-on-child (`CreateProcessAsUser` / `CreateProcessWithTokenW`
  plus a supported Child wrap)
- Windows confined MCP stdio (same token-on-child gap)
- Seatbelt deprecation / Darwin deny-default abort
- No isolation-label upgrade (T1 unchanged)
