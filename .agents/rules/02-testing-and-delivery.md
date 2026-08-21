---
trigger: always_on
description: Require focused verification, compatibility evidence, and truthful implementation handoff for every coding change.
globs:
---

# Testing and delivery

Implementation code and tests are the normal output. Use checked-in mise tasks and CI commands (`mise run <task>`). Root `mise.toml` owns tool pins and repository tasks; do not add `rust-toolchain.toml` or a Cargo `xtask`. Never assume a planned wrapper or task exists: inspect `mise.toml`, keep task scripts sparse and fast, run focused checks after each coherent change, and run the active workstream's complete affected validation before handoff.

## Select tests from the changed invariant

- Kernel state or reducer changes require transition-table coverage, invalid-transition cases, deterministic replay, serialization, and relevant property tests.
- Record, effect, storage, cancellation, deadline, interaction, or recovery changes require fault/crash-prefix coverage, idempotency/conflict cases, and restart or reconciliation evidence.
- Public APIs, schemas, journals, events, protocols, feature flags, WIT, or bindings require compatibility fixtures, shared semantic traces, target-specific conformance, and applicable fuzz or property tests for parsers, records, and protocols.
- Async scheduling, streams, callbacks, or queues require bounds, ordering, cancellation, shutdown, backpressure, and leak checks.
- Security-boundary changes require the applicable adversarial, authorization, permission-denial, size-limit, and secret-redaction tests from the Threat Model.
- Compaction changes require protected-content, tool-pair atomicity, sensitivity and provenance preservation, checkpoint invalidation, canonical-history immutability, replay, cross-binding, and hard-budget safe-failure tests.
- Performance changes begin with a fixture proven to activate the hot path and report framework cost separately from external latency.

Every fixture must demonstrably activate the behavior it claims to test. Tests must be deterministic and offline by default. Do not rely on wall-clock races, unordered iteration, retries that hide flakes, external credentials, or live network access for ordinary validation. A green aggregate suite does not replace focused proof of the changed invariant.

## Keep the change reviewable

- Apply formatting, lint, architecture, dependency, target-build, and generated-artifact checks configured for the touched surface.
- Build minimal and default feature sets independently when the touched package exposes features.
- Keep generated output reproducible and separate from hand-authored logic where practical. Regenerate via the documented command only; fail the change on a dirty generated tree.
- Update public documentation and tested examples with public API changes. Update compatibility or migration fixtures with controlled contract changes.
- For public binding surface changes, confirm cross-binding name/code/shape parity (or an explicit tracked deferral) and refresh shared conformance fixtures when semantics change.
- For Python binding or stub changes, run the configured type-check against the public package/stubs once that task exists; report stub/runtime drift as a failure. Before handoff, smoke-check that IDE hover/signature help shows docs for new or changed public APIs (`04-python-coding.md`).
- For TypeScript/npm public surface changes, run the configured declaration/type-check against the published entrypoints once that task exists; report declaration drift as a failure. Before handoff, smoke-check that IDE hover shows TSDoc for new or changed public exports (`05-typescript-coding.md`).
- Report exact commands, targets, results, and validation limits. Never claim an unavailable or unrun check passed.

## Execute and hand off planned workstreams

Apply this state machine to each Codex plan step:

1. **Admit.** Recheck dependencies, architecture and compatibility decisions, exclusions, branch and base, worktree ownership, authorization, and required reviews.
2. **Implement.** Complete the coherent vertical slice and focused tests without pulling in unrelated work.
3. **Verify.** Run the step's focused checks and complete affected validation; retain exact command output and material failures in the task handoff.
4. **Integrate.** Reconcile shared manifests, generated surfaces, bindings, and docs, then rerun integration-sensitive checks.
5. **Advance.** Mark the step complete only when its acceptance criteria pass and its dependencies permit the next step.

Validation or implementation failure keeps the step in progress. Diagnose and
retry bounded checks; it is not a product pass and must not cause source changes
unless source is proven responsible. Do not repair unrelated failures without
authority or convert an aggregate failure into a pass by omission.

Provide one consolidated handoff when the plan completes or stops. Report the
requested and executed workstreams, exact validation and limits, retained
failures and remediation, blockers, security or compatibility decisions,
external actions actually taken, final worktree state, and the precise next
eligible action. Do not describe uncommitted work as merged, released, or
immutable evidence.
