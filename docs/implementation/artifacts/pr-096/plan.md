# PR-096 execution plan

Date: 2026-08-18
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.27 / PLAN-0.25

This file is the execution contract for PR-096 (draft W10 / PR-091).

## Purpose

Move Temporal-crate tests that prove runtime retry and journal parity
into workflow-local, then delete the non-engine Temporal shim.

## Principal changes

- `loop_parity.rs` and `retry_policy.rs` live under
  `finstack-ai-workflow-local` and call `WorkflowSession` /
  `retry_decision`.
- Delete `finstack-ai-workflow-temporal`, its workspace member, and
  its public-api baseline.

## Acceptance mapping

- PR-096-A01: moved tests still prove `retry_decision` parity.
- PR-096-A02: Temporal crate, workspace member, and in-repo
  references used as current members are gone.

## Explicit exclusions

Temporal engine integration. Bookmark navigation.

## Dependencies

PR-085. After PR-090 so workflow-local already holds one connection.
