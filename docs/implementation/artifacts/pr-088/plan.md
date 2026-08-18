# PR-088 execution plan

Date: 2026-08-18
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.27 / PLAN-0.25

This file is the execution contract for PR-088 (draft W3a / PR-083).

## Purpose

Rewrite MCP transport framing so notifications, foreign ids, and
oversized lines fail closed instead of desynchronizing the stream.

## Principal changes

- `round_trip` is a dispatch loop: write, bounded line, decode once.
  Notifications queue; matching id interprets; anything else poisons.
- `StdioTransport` and `HttpTransport` implement `take_notifications`.
- Bounded `read_until` with `MAX_LINE_BYTES`. Oversized lines poison.
- `decode_frame` + `interpret_result`. SSE splits on blank lines into
  events. HTTP bodies are chunk-capped.
- `HttpConfig::sse_only` deleted.

## Acceptance mapping

- PR-088-A01: Notification-before-response still pairs; the next
  request is not off-by-one.
- PR-088-A02: Oversized line is a protocol violation.
- PR-088-A03: Observer-facing `take_notifications` is implemented on
  stdio and HTTP.

## §18 / TM-03

Protocol/parser. Foreign JSON-RPC ids and oversized frames poison the
transport because the stream cannot be resynchronized. SSE events are
no longer concatenated across event boundaries.

## Explicit exclusions

Classify/lib/resources (PR-089). Catalog drift. Gateway deletion.

## Compatibility class

Pre-1.0 public deletion of `HttpConfig::sse_only`.

## Dependencies

PR-085. Sequential predecessor of PR-089.
