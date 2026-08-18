# PR-091 execution plan

Date: 2026-08-18
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.27 / PLAN-0.25

This file is the execution contract for PR-091 (draft W5 / PR-086).

## Purpose

Bound E2B HTTP, recheck cancel/deadline before each POST, fix IPv6
loopback, and switch to shared `verify_authority`. No compensating
DELETE and no create/run/destroy split.

## Principal changes

- Recheck cancellation/deadline before each POST; race with
  `tokio::select!`.
- Stream response chunks and fail at 64 KiB.
- Bracketed IPv6 loopback hosts are accepted for plaintext HTTP.
- Shared `verify_authority`.

## Acceptance mapping

- PR-091-A01: Cancelled signal sends no HTTP to the fixture.
- PR-091-A02: Oversized JSON returns `E2B_LIMIT_EXCEEDED`.

## Residual

Orphaned remote sandboxes remain when a call is cancelled after
create. Explicit lifecycle is a later unit.

## Explicit exclusions

Compensating DELETE. create/run/destroy split. ADR-049.

## Dependencies

PR-085.
