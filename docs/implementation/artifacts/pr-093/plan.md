# PR-093 execution plan

Date: 2026-08-18
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.27 / PLAN-0.25

This file is the execution contract for PR-093 (draft W7 / PR-088).

## Purpose

Independent small-fix wave across observer-otel, observer-log,
middleware-compaction, tools-filesystem, and middleware-verify.

## Principal changes

- Bound otel `captured` to queue capacity; delete no-op `otlp`.
- Move log-observer `write_all` onto `spawn_blocking`.
- Hoist sliding-window estimates, increment the total, and `break`
  at the target. Drop dead `{"depth":1}` resume state.
- Filesystem write/edit are `Sequential`; unix list/search/walk skip
  unreadable children; shared `verify_authority`.
- Verify `InteractionId` follows `ctx.run.effect_id`; version is 1.0.0.

## Acceptance mapping

- PR-093-A01: otel snapshot stays within the queue cap.
- PR-093-A02: compaction terminates on 5k entries.
- PR-093-A03: two verify runs yield distinct interaction ids.
- PR-093-A04: one `chmod 000` entry does not fail a listing.

## Explicit exclusions

Gateway deletion. Temporal deletion. Verify `publish = false` (PR-097).

## Dependencies

PR-085.
