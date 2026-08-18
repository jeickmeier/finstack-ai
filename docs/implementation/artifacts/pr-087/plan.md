# PR-087 execution plan

Date: 2026-08-18
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.27 / PLAN-0.25

This file is the execution contract for PR-087 (draft W2 / PR-082).

## Purpose

Isolate child lookup by tenant, delete `last_run_id`, call
`verify_authority` before the name check, and rename `subagent_await`
to `subagent_status`.

## Principal changes

- Child map keyed by `ChildKey { tenant_scope, session_id, run_id }`.
  Cross-tenant lookup returns `SUBAGENT_CHILD_NOT_FOUND`.
- `last_run_id` deleted. `status` without `run_id` is invalid arguments.
- Shared `verify_authority` before the tool-name check.
- Public tool name `subagent_status` (id `finstack.tools.subagent.status`).
  Bindings and README updated in the same change.

## Acceptance mapping

- PR-087-A01: Start under tenant A, `cancel`/`status` under B →
  `SUBAGENT_CHILD_NOT_FOUND`.
- PR-087-A02: `status` with no `run_id` → invalid arguments, not
  another tenant's `session_id`.
- PR-087-A03: Bindings and crate README use `subagent_status`.

## §18 / TM-02

Tenant isolation. The previous process-global `last_run_id` could
return another tenant's `session_id`. Lookup now requires the caller's
tenant scope. Residual: this crate still does not wait for child
completion (`AgentInvoker::join` remains out of scope).

## Explicit exclusions

`AgentInvoker::join`. Nested-effect await. Gateway deletion.

## Compatibility class

Pre-1.0 source-breaking rename.

## Dependencies

PR-085. Independent of PR-086.
