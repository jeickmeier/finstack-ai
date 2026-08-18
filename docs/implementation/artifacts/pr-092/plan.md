# PR-092 execution plan

Date: 2026-08-18
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.27 / PLAN-0.25

This file is the execution contract for PR-092 (draft W6 / PR-087).

## Purpose

Bound remote-child connect/read/frame loops, derive command ids from
`request_digest`, reject cancel of a never-accepted locator, and
derive `Clone` on `RemoteChildRoute`.

## Principal changes

- Connect timeout and per-read deadline.
- Bound open and command frame loops by count and elapsed time.
- `RemoteCommand` id comes from `request.request_digest`, not a
  fresh UUID per attempt.
- `cancel` rejects a locator never accepted; accepted map is bounded.
- `#[derive(Clone)]` on `RemoteChildRoute`; delete `clone_route`.

## Acceptance mapping

- PR-092-A01: Peer that never sends `CommandResult` times out.
- PR-092-A02: 10k `EventBatch` frames fail closed at the frame bound.

## Explicit exclusions

`AgentInvoker::join`. Temporal engine. Gateway deletion.

## Dependencies

PR-085.
