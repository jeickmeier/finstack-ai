# PR-079 execution plan

Date: 2026-08-17
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.24 / PLAN-0.22

This file is the execution contract for PR-079 (E4d). Closed
PR-001–PR-078 envelopes are not reused. PR-079 is the only active
logical PR once admitted. D1–D18 is a later governance transaction
and is not admitted here.

## Purpose

Add `AgentRun` child-run and external-completion surfaces in Rust,
then route them from Python.

## Principal changes

- Rust `AgentRun` methods to prepare/accept a child via
  `ChildRunPrepared` + `AgentInvoker`.
- External completion routing patterned on
  `WorkflowSession::complete_external`.
- Python pymethods that actually route, retiring the data-only
  prebeta path for those two kinds (or keeping `normalize` as a
  validator in front of the router).

## Acceptance mapping

Two Implementation Plan bullets map 1:1 to A01–A02.

- PR-079-A01: Rust unit + journal fixture for child accept/cancel
  fan-out (lane tests already distinguish child vs lane).
- PR-079-A02: Python test that starts a child and completes an
  external effect, not merely round-trips JSON.

## Explicit exclusions

`RemoteChildSession` dispatch. Marketplace. Phase 11.
`RequestCompactionModel`. Temporal engine. NFR-PORT-* / D1–D18.
Registry publication.

## Compatibility class

Additive Rust then Python. Freeze-gate baseline must be updated in
the same change for new public names.

## Dependencies

PR-078.

## Suggested authorization sentence

```
Run PR-079; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

Do not infer admission from this planning file. D1–D18 requires a
later named sentence after E has settled what ships.
