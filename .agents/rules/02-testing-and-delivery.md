---
trigger: always_on
description: Require focused verification, compatibility evidence, and truthful implementation handoff for every coding change.
globs:
---

# Testing and delivery

Implementation code and tests are the normal output. Use checked-in mise tasks and CI commands (`mise run <task>`). Root `mise.toml` owns tool pins and repository tasks; do not add `rust-toolchain.toml` or a Cargo `xtask`. During PR-001–PR-003, standard tool commands needed to validate newly created artifacts are allowed; add and document the canonical mise task with that tooling. Never assume a planned wrapper or task exists. Planning describes required outcomes, not proof that a command is available or passed. Keep task scripts sparse and fast. Run focused checks after each coherent change and the logical PR's complete affected validation before handoff.

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

## Transition an authorized PR range

Apply this state machine independently to every logical PR in an explicitly authorized range:

1. **Admit.** Recheck dependencies, phase entrance, gate and ADR state, tracking reference, exclusions, branch and base, worktree ownership, and required reviews. Move only the active PR to truthful `Ready` or `In progress`.
2. **Implement.** Complete only that PR's coherent vertical slice and focused tests.
3. **Candidate.** Create an immutable candidate commit, run the PR's complete affected validation against that revision, retain artifacts and digests, and update acceptance, review, blocker, ADR, exception, task, and delivery records truthfully.
4. **Transition.** In `stacked` mode, leave the PR `In review`, preserve its separate candidate and evidence commits, and base the successor on its committed review head; do not claim integration or `Done`. In `integrated` mode, merge only into the target authorized in the execution envelope, verify the resulting integration tree, rerun integration-sensitive checks, complete the register transaction, and mark `Done` only when the existing completion rules are satisfied. Base the successor on the verified integrated commit.
5. **Advance.** Re-evaluate the successor's eligibility and continue without a routine handoff only while the original execution envelope remains valid.

Validation or implementation failure keeps the current PR active. Preserve material failing evidence, link remediation, create a new immutable candidate when source changes, and rerun the affected and required final checks before advancing. A recoverable tooling or environment failure may be diagnosed and retried against the same candidate with bounded targeted reruns or serialized task execution; it is not a product pass and must not cause source changes unless source is proven responsible.

Do not repair unrelated failures as part of the range. Continue only when an isolated clean candidate proves the failure is unrelated and every acceptance requirement for the active PR remains evidenced; otherwise record the blocker and stop the range. Never convert an aggregate failure into a pass by omission. An unresolved authority, design, gate, security, evidence, or worktree-ownership failure blocks the range rather than permitting a skip.

## Complete the PR boundary and final handoff

Use `docs/implementation/README.md` for the update transaction. At minimum:

- At coding start, assign the logical PR and record a real owner plus issue, branch, or pull-request reference before moving it to `Ready` or `In progress`; create task rows only for actual decomposition. If required metadata is unavailable, report the tracking gap instead of fabricating it.

1. Keep the logical PR, real issue/branch/actual PR references, task, blocker, acceptance coverage, snapshot totals, and derived phase state current in `delivery-ledger.md`; do not invent external metadata to satisfy a field.
2. Add immutable-commit validation and review records to `evidence-register.md`; preserve failures and superseded evidence.
3. Update `adr-register.md` when code implements, verifies, or supersedes a decision.
4. Track eligible standards waivers through every state in `exceptions-register.md`; only approved, unexpired exceptions support `Waived` acceptance.

For every range member, treat these register changes as one status transaction at the same final revision. Reconcile the logical-PR row, tasks, blockers, acceptance coverage, snapshot totals, derived phase state, evidence, ADR state, and exceptions together. If any claim lacks support, retain the conservative prior status rather than partially recording completion. Append failures and superseding evidence; never rewrite history.

During coding, use truthful `In progress` or `In review` state and report local validation without treating an uncommitted worktree as immutable evidence. `Done`, merged commits and dates, final verification, approvals, and gate passage are post-merge or authorized-review actions. Do not mark a logical PR `Done` until mapped work is merged or validly dispositioned, acceptance and required reviews are complete at the integrated commit, and blockers are closed. Phases additionally require exit evidence; gates require a named decision and are never inferred.

When the last PR in a phase contains gate-sign-off acceptance, first bring the PR and phase-exit evidence to reviewable state. The named approver then records a separate gate decision; only afterward may that decision satisfy the gate criterion and the PR, phase, and gate statuses be reconciled. Range authorization is never that decision.

Provide one consolidated range handoff when the range completes or stops. Report the requested and executed range, mode, per-PR candidate and integrated commits, statuses, exact validation and limits, evidence and register transactions, retained failures and remediation, blockers, gate state, external actions actually taken, final worktree state, and the precise next eligible action.
