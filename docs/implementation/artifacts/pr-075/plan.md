# PR-075 execution plan

Date: 2026-08-17
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.24 / PLAN-0.22

This file is the execution contract for PR-075 (E1). Closed
PR-001–PR-074 envelopes are not reused. PR-075 is the only active
logical PR once admitted. This file does not admit PR-076–PR-079.

## Purpose

Invoke registered `ContextProvider`s on the production
`prepare_context` / `before_model` path so repository and memory
leaves, protected projection, and the Python adapter become reachable.

## Principal changes

- Add a context-effect driver arm parallel to model/tool in the
  host-task dispatcher.
- Place the driver under
  `crates/finstack-ai-runtime/src/exec/context_driver/`.
- Replace hard-coded `protected = false` in
  `stage_settlement/input.rs` with authoritative projection from
  assembled context.
- Do not change `RequestCompactionModel` /
  `MIDDLEWARE_STAGE_UNLANDABLE`.

## Acceptance mapping

Five Implementation Plan bullets map 1:1 to A01–A05.

- PR-075-A01: A non-test caller of `assemble_context` /
  `CommittedContextCall::try_new` exists on the `prepare_context` /
  `before_model` path.
- PR-075-A02: Repository and memory leaves produce `ContextItem`s
  that reach `BeforeModelInput.source_entries` with authoritative
  `protected`.
- PR-075-A03: Sliding-window compaction in
  `examples/rust-minimal --bin coding` no longer fails
  `compaction_result_invalid`.
- PR-075-A04: A Python `ContextProvider` adapter test fires
  end-to-end and is not only `TypeError` on `.component`.
- PR-075-A05: `RequestCompactionModel` still returns
  `middleware_stage_unlandable`.

## Explicit exclusions

`RequestCompactionModel` / summarize compaction. `fold.rs`
unlandable-stage change. Journal/WIT/protocol meaning change.
Temporal. Phase 11 provider code. NFR-PORT-* amendment. E3/E4 code.

## Compatibility class

Additive runtime/SDK. The driver is internal; the Python adapter
becomes reachable. Freeze-gate baseline must be updated in the same
change if any public name is added.

## Dependencies

PR-074. Independent of PR-076–PR-079. B1 freeze gate must already
see additions.

## Suggested authorization sentence

```
Run PR-075; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

Do not infer admission from this planning file.
