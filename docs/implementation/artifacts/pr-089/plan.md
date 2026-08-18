# PR-089 execution plan

Date: 2026-08-18
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.27 / PLAN-0.25

This file is the execution contract for PR-089 (draft W3b / PR-084).

## Purpose

Close MCP classify/lib/resources fail-closed gaps and wire
`MCP_CATALOG_DRIFT`. Shared `verify_authority` on MCP and skills.

## Principal changes

- Dedup sanitized `tool_id` alongside raw names; collision fails closed.
- Truncate to a fraction and re-measure the serialized envelope.
- JSON-RPC codes are structural on `McpError`. Optional-catalog
  missing matches `-32601`, not message text.
- Resources apply budget before fetch and cap the joined result.
- `is_retry_safe` inlined; `MCP_CATALOG_DRIFT` on reconstruct digest
  mismatch. `ToolDeferralSupport::Never` preserved.

## Acceptance mapping

- PR-089-A01: Colliding sanitized ids fail closed.
- PR-089-A02: Oversize results still carry `truncated` +
  `MCP_LIMIT_EXCEEDED` after re-measure.
- PR-089-A03: Reconstruct of a changed catalog returns
  `MCP_CATALOG_DRIFT`.

## Explicit exclusions

Gateway deletion. e2b lifecycle.

## Compatibility class

Pre-1.0 reconstruct behavior change (drift is now an error).

## Dependencies

PR-088.
