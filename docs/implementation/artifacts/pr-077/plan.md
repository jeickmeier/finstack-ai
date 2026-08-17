# PR-077 execution plan

Date: 2026-08-17
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.24 / PLAN-0.22

This file is the execution contract for PR-077 (E4a). Closed
PR-001–PR-076 envelopes are not reused. PR-077 is the only active
logical PR once admitted. This file does not admit PR-078–PR-079.

## Purpose

Complete the PR-047 minimum `Lane` verbs: `run(input) -> Run`,
`suspend()`, `resume()`.

## Principal changes

- `Lane::run(input)` on the existing `Agent::start` / `AcceptRun`
  path.
- `suspend` parks the driver without dropping the journal.
- `resume` uses `WorkflowSession::with_ports` plus local-workflow
  restart respawn (PR-076).
- `append_text` still does not start a run.

## Acceptance mapping

Four Implementation Plan bullets map 1:1 to A01–A04.

- PR-077-A01: Idle lane `run(input)` returns a live `Run`.
- PR-077-A02: `suspend` parks without dropping the journal.
- PR-077-A03: `resume` respawns `RunTaskOwner`.
- PR-077-A04: `append_text` still does not start a run.

## Explicit exclusions

Bookmark/checkout convenience navigation. Distributed multi-writer.
Python bindings (PR-078). Child-run API (PR-079). Journal field adds.
Phase 11.

## Compatibility class

Additive SDK. Freeze-gate baseline must be updated in the same change
for the new inherent methods.

## Dependencies

PR-076 for `suspend`/`resume`. `run` may be implemented first in this
PR if PR-076 is not yet merged; `suspend`/`resume` land in the same
PR once PR-076 is in.

## Suggested authorization sentence

```
Run PR-077; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

Do not infer admission from this planning file.
