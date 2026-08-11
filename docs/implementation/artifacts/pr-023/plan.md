# PR-023 execution plan

Date: 2026-08-11

## Execution envelope

- mode: integrated
- branch: `codex/pr-023-test-kit`
- baseline: `b48885afcb9d4574a2b287112252bed167a28f93`
- integration target: `main`
- authorized local actions: branch, edit, test, commit, and merge
- authorized external actions: none

## Contract and scope

PR-023 publishes the dedicated `finstack-ai-test` support surface described by
Implementation Plan PR-023 and TDD section 32. It reuses the public six-port
contracts and existing PR-015 through PR-022 semantic helpers. It does not add
a kernel port, private runtime hook, certification surface, marketplace badge,
or production dependency on the test crate.

The implementation sequence is:

1. Complete deterministic public fixtures: manual time/ID sources, scripted
   context and middleware components, store faults, observer capture/failure,
   and trace assertions.
2. Add contract-diagnostic conformance functions for Model, Toolset,
   ContextProvider, Middleware, Observer, and JournalStore.
3. Publish strict golden-scenario helpers for child lineage, deferred external
   completion, typed interactions, duplicate completion, before-finalize
   continuation, and compaction projections/checkpoints.
4. Exercise the helpers from public-only leaf provider/toolset fixtures and a
   public SDK composition path.
5. Run focused, target/dependency, schema, architecture, security, and aggregate
   validation; bind immutable candidate and local integration evidence.

## Acceptance map

- A01: a sample public-only provider and toolset pass their conformance suites.
- A02: kernel and runtime production dependency graphs do not contain
  `finstack-ai-test`.
- A03: negative conformance cases name the port and violated stable contract.
- A04: checked-in golden scenarios execute through public SDK/runtime/kernel
  APIs without private internals.
- A05: compaction conformance proves canonical-history immutability, protected
  retention, tool-pair atomicity, checkpoint invalidation, hard-budget failure,
  and byte-identical shared projections.

## Security and decision review

- TM-10: duplicate/deferred external completion scenarios retain exact scoped
  identities and fail closed on conflicting replays.
- TM-21: compaction helpers retain protected policy context and provenance,
  inherit sensitivity, bind all frozen digests, and treat incompatible
  checkpoints as cache misses.
- ADR-037 advances only its PR-023 test-kit evidence axis; no decision change is
  required. No other ADR trigger is identified at admission.

## Exclusions

No certification, marketplace badge, live provider call, binding implementation,
remote registry, hosted publication, push, or hosted pull request is included.
