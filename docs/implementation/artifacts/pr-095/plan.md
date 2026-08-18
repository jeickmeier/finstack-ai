# PR-095 execution plan

Date: 2026-08-18
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.27 / PLAN-0.25

This file is the execution contract for PR-095 (draft W9 / PR-090).

## Purpose

Delete the Anthropic process-global prompt-cache mutex and fix the
three surviving providers' catalog and capability holes.

## Principal changes

- Anthropic `request` uses `try_from_draft` only. Catalog-change
  omission stays proven in `request.rs`.
- Unknown-model `capabilities` return a zeroed profile named for the
  requested model, not the first catalog entry.
- Structured capability on the unknown path matches the adapter
  (`Native` / `Prompted`). Openai still rewrites a caller-supplied
  `prompt_cache_key` and does not insert one.

## Acceptance mapping

- PR-095-A01: catalog-change omits stale `cache_control` without a
  process-global mutex.
- PR-095-A02: unknown model does not inherit the first profile.
- PR-095-A03: structured capability matches what the adapter sends.

## Explicit exclusions

Reintroducing a global cache key. Gateway crate revival.

## Dependencies

PR-094.
