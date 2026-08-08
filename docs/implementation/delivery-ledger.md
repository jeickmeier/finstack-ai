# Delivery ledger

This is the canonical live checklist for implementation status. The [Implementation Plan](../planning/04-finstack-ai-implementation-plan.md) remains the source for phase purpose, dependencies, logical-PR titles and changes, acceptance evidence, exclusions, and gate requirements. Use an ID from this ledger to locate its authoritative entry in that plan.

## Current snapshot

Last updated 2026-08-08 and reconciled against documentation pack v0.12. Update this date and the totals below in every change that alters delivery state.

| Item | Planned | Done or passed | Current state |
| --- | ---: | ---: | --- |
| Phases | 10 | 0 | Phase 0 `In progress`; others `Todo` |
| Logical PRs | 66 | 1 | PR-001 `Done`; PR-002–PR-066 `Todo` |
| PR acceptance-evidence bullets | 345 | 6 | PR-001 A01–A06 `Passed`; remainder not yet owned |
| Phase entrance and exit bullets | 62 | 0 | No coverage recorded |
| Program gates | 9 | 0 | All `Not ready` |
| Implementation tasks | 0 | 0 | Add only when a logical PR is decomposed |
| Open blockers | 0 | 0 | None recorded |

PR-001 is `Done` at `73bfe88c8dbc92c4e4c6eba1a5a7814240c2e862` with six passed acceptance criteria. Phase 0 and G0 remain incomplete until PR-002–PR-005 finish.

## Status values

### Logical PR and task

| Status | Meaning |
| --- | --- |
| `Todo` | Not assigned or not ready to start. |
| `Ready` | Dependencies and entrance conditions are evidenced; an owner and actual work reference exist. |
| `In progress` | Implementation is active. |
| `Blocked` | Progress cannot continue; a linked blocker has an owner, next action, and review date. |
| `In review` | Implementation is submitted and acceptance evidence is ready for review. |
| `Done` | All completion conditions below are satisfied. |
| `Deferred` | A versioned plan amendment moves the item beyond the governed sequence and is linked from the work-reference field. |
| `Cancelled` | A versioned plan amendment removes or replaces the item and is linked from the work-reference field. |

`Deferred` and `Cancelled` are not discretionary ledger states. Dropping, postponing beyond the governed sequence, or replacing planned work requires a linked versioned Implementation Plan amendment and any affected ADR, requirement, or security reconciliation. Preserve the row as history.

### Phase

A phase uses `Todo`, `Ready`, `In progress`, `Blocked`, or `Done`. A versioned plan amendment may preserve a removed or postponed phase as `Cancelled` or `Deferred`. Phase state is derived in this order:

1. `Done` when every completion condition is satisfied.
2. `Blocked` when any current logical PR is `Blocked` or a phase-level blocker is open.
3. `In progress` when any current logical PR is `In progress` or `In review`, or when at least one is `Done` while current phase work remains.
4. `Ready` when nothing has started and at least one current logical PR is `Ready`.
5. `Todo` otherwise.

### Gate

| Status | Meaning |
| --- | --- |
| `Not ready` | Required phase and gate evidence is incomplete. |
| `Ready for review` | Required evidence is complete and an approver has been assigned. |
| `Passed` | An authorized approver recorded a passing decision in the evidence register. |
| `Failed` | Review completed with blocking findings; the decision record links the remediation work. |

## Completion rules

A logical PR is `Done` only when:

- every mapped actual pull request is `Merged`, or is `Closed` with an approved linked disposition or replacement; every replacement is mapped, merged, and has its commit and date recorded;
- all child tasks are `Done`, or are `Deferred` or `Cancelled` through linked planning change control;
- every acceptance-evidence bullet in the plan is `Passed`, `Not applicable`, or covered by an unexpired approved exception;
- validation evidence identifies the final integrated commit, environment or target, result, and durable artifact or log;
- required review and security evidence is present; and
- no linked blocker remains open.

A phase is `Done` only when every logical PR remaining in the current plan is `Done`, every phase exit criterion has evidence, and no phase blocker or expired exception remains. Any preserved `Deferred` or `Cancelled` row must link the amendment that changed the phase inventory.

A gate is never inferred from phase status. It reaches `Passed` only through a separate, named approval in the [evidence register](evidence-register.md). G4 covers both Phase 4 and Phase 5.

Logical PR IDs are planning units, not hosting-platform pull-request numbers. One logical PR may map to several actual pull requests, but it remains open until the complete logical unit satisfies these rules.

## Phase ledger

Coverage is recorded as `verified criteria / total criteria` from the plan. Evidence records use the identifiers defined in the evidence register.

| Phase | Logical PRs | Entrance | Exit | Gate | Status | Owner | Active PRs | Blocker | Evidence | Updated |
| --- | --- | ---: | ---: | --- | --- | --- | --- | --- | --- | --- |
| [Phase 0](../planning/04-finstack-ai-implementation-plan.md#8-phase-0-foundation-and-architecture-governance) | PR-001–PR-005 | 0/1 | 0/5 | G0 | In progress | me@jeickmeier.com | PR-002 | — | PR-001 Done; PR-002 In progress; PR-003–PR-005 Todo | 2026-08-08 |
| [Phase 1](../planning/04-finstack-ai-implementation-plan.md#9-phase-1-semantic-agent-microkernel) | PR-006–PR-013 | 0/2 | 0/4 | G1 | Todo | — | — | — | — | — |
| [Phase 2](../planning/04-finstack-ai-implementation-plan.md#10-phase-2-native-runtime-and-effect-execution) | PR-014–PR-020 | 0/2 | 0/4 | G2 | Todo | — | — | — | — | — |
| [Phase 3](../planning/04-finstack-ai-implementation-plan.md#11-phase-3-rust-sdk-and-native-developer-preview) | PR-021–PR-026 | 0/2 | 0/4 | G3 | Todo | — | — | — | — | — |
| [Phase 4](../planning/04-finstack-ai-implementation-plan.md#12-phase-4-first-class-python-bindings) | PR-027–PR-032 | 0/2 | 0/4 | G4 | Todo | — | — | — | — | — |
| [Phase 5](../planning/04-finstack-ai-implementation-plan.md#13-phase-5-browser-and-javascript-webassembly-bindings) | PR-033–PR-038 | 0/2 | 0/4 | G4 | Todo | — | — | — | — | — |
| [Phase 6](../planning/04-finstack-ai-implementation-plan.md#14-phase-6-durability-recovery-and-lanes) | PR-039–PR-048 | 0/3 | 0/4 | G5 | Todo | — | — | — | — | — |
| [Phase 7](../planning/04-finstack-ai-implementation-plan.md#15-phase-7-isolated-witwasmtime-extensions) | PR-049–PR-054 | 0/3 | 0/4 | G6 | Todo | — | — | — | — | — |
| [Phase 8](../planning/04-finstack-ai-implementation-plan.md#16-phase-8-ecosystem-readiness-and-public-preview) | PR-055–PR-061 | 0/2 | 0/4 | G7 | Todo | — | — | — | — | — |
| [Phase 9](../planning/04-finstack-ai-implementation-plan.md#17-phase-9-10-hardening-and-general-availability) | PR-062–PR-066 | 0/2 | 0/4 | G8 | Todo | — | — | — | — | — |

## Gate ledger

| Gate | Scope | Status | Approver | Decision record | Evidence | Decision date | Remediation |
| --- | --- | --- | --- | --- | --- | --- | --- |
| G0 | Phase 0 | Not ready | — | — | — | — | — |
| G1 | Phase 1 | Not ready | — | — | — | — | — |
| G2 | Phase 2 | Not ready | — | — | — | — | — |
| G3 | Phase 3 | Not ready | — | — | — | — | — |
| G4 | Phases 4 and 5 | Not ready | — | — | — | — | — |
| G5 | Phase 6 | Not ready | — | — | — | — | — |
| G6 | Phase 7 | Not ready | — | — | — | — | — |
| G7 | Phase 8 | Not ready | — | — | — | — | — |
| G8 | Phase 9 | Not ready | — | — | — | — | — |

## Logical PR ledger

`Acceptance` is the number of plan acceptance-evidence bullets with a completed disposition, not the number of tests run. Detailed results belong in the evidence register.

### Phase 0

| Logical PR | Status | Owner | Issue / actual PRs / change | Tasks | Acceptance | Evidence | Blocker | Merged commits / dates | Updated |
| --- | --- | --- | --- | ---: | ---: | --- | --- | --- | --- |
| PR-001 | Done | me@jeickmeier.com | `main` @ `73bfe88c8dbc92c4e4c6eba1a5a7814240c2e862` (direct commit; no GitHub PR) | 0 | 6/6 | PR-001-E-dep-direction-1fe94f769044; PR-001-E-cargo-check-3e6a85e8d27f; PR-001-E-kernel-deps-1d8c0f41d158; PR-001-E-mise-doctor-cae2f2eb7918; PR-001-E-ownership-review-b14626f70259; PR-001-E-security-md-f70391db8ac2 | — | `73bfe88c8dbc92c4e4c6eba1a5a7814240c2e862` / 2026-08-08 | 2026-08-08 |
| PR-002 | In progress | me@jeickmeier.com | branch `pr-002-architecture-enforcement` | 0 | 0/7 | — | — | — | 2026-08-08 |
| PR-003 | Todo | — | — | 0 | 0/5 | — | — | — | — |
| PR-004 | Todo | — | — | 0 | 0/5 | — | — | — | — |
| PR-005 | Todo | — | — | 0 | 0/5 | — | — | — | — |

### Phase 1

| Logical PR | Status | Owner | Issue / actual PRs / change | Tasks | Acceptance | Evidence | Blocker | Merged commits / dates | Updated |
| --- | --- | --- | --- | ---: | ---: | --- | --- | --- | --- |
| PR-006 | Todo | — | — | 0 | 0/5 | — | — | — | — |
| PR-007 | Todo | — | — | 0 | 0/4 | — | — | — | — |
| PR-008 | Todo | — | — | 0 | 0/8 | — | — | — | — |
| PR-009 | Todo | — | — | 0 | 0/5 | — | — | — | — |
| PR-010 | Todo | — | — | 0 | 0/4 | — | — | — | — |
| PR-011 | Todo | — | — | 0 | 0/6 | — | — | — | — |
| PR-012 | Todo | — | — | 0 | 0/5 | — | — | — | — |
| PR-013 | Todo | — | — | 0 | 0/4 | — | — | — | — |

### Phase 2

| Logical PR | Status | Owner | Issue / actual PRs / change | Tasks | Acceptance | Evidence | Blocker | Merged commits / dates | Updated |
| --- | --- | --- | --- | ---: | ---: | --- | --- | --- | --- |
| PR-014 | Todo | — | — | 0 | 0/7 | — | — | — | — |
| PR-015 | Todo | — | — | 0 | 0/4 | — | — | — | — |
| PR-016 | Todo | — | — | 0 | 0/5 | — | — | — | — |
| PR-017 | Todo | — | — | 0 | 0/4 | — | — | — | — |
| PR-018 | Todo | — | — | 0 | 0/8 | — | — | — | — |
| PR-019 | Todo | — | — | 0 | 0/4 | — | — | — | — |
| PR-020 | Todo | — | — | 0 | 0/4 | — | — | — | — |

### Phase 3

| Logical PR | Status | Owner | Issue / actual PRs / change | Tasks | Acceptance | Evidence | Blocker | Merged commits / dates | Updated |
| --- | --- | --- | --- | ---: | ---: | --- | --- | --- | --- |
| PR-021 | Todo | — | — | 0 | 0/5 | — | — | — | — |
| PR-022 | Todo | — | — | 0 | 0/12 | — | — | — | — |
| PR-023 | Todo | — | — | 0 | 0/5 | — | — | — | — |
| PR-024 | Todo | — | — | 0 | 0/5 | — | — | — | — |
| PR-025 | Todo | — | — | 0 | 0/5 | — | — | — | — |
| PR-026 | Todo | — | — | 0 | 0/5 | — | — | — | — |

### Phase 4

| Logical PR | Status | Owner | Issue / actual PRs / change | Tasks | Acceptance | Evidence | Blocker | Merged commits / dates | Updated |
| --- | --- | --- | --- | ---: | ---: | --- | --- | --- | --- |
| PR-027 | Todo | — | — | 0 | 0/5 | — | — | — | — |
| PR-028 | Todo | — | — | 0 | 0/4 | — | — | — | — |
| PR-029 | Todo | — | — | 0 | 0/4 | — | — | — | — |
| PR-030 | Todo | — | — | 0 | 0/5 | — | — | — | — |
| PR-031 | Todo | — | — | 0 | 0/5 | — | — | — | — |
| PR-032 | Todo | — | — | 0 | 0/5 | — | — | — | — |

### Phase 5

| Logical PR | Status | Owner | Issue / actual PRs / change | Tasks | Acceptance | Evidence | Blocker | Merged commits / dates | Updated |
| --- | --- | --- | --- | ---: | ---: | --- | --- | --- | --- |
| PR-033 | Todo | — | — | 0 | 0/5 | — | — | — | — |
| PR-034 | Todo | — | — | 0 | 0/7 | — | — | — | — |
| PR-035 | Todo | — | — | 0 | 0/4 | — | — | — | — |
| PR-036 | Todo | — | — | 0 | 0/4 | — | — | — | — |
| PR-037 | Todo | — | — | 0 | 0/5 | — | — | — | — |
| PR-038 | Todo | — | — | 0 | 0/6 | — | — | — | — |

### Phase 6

| Logical PR | Status | Owner | Issue / actual PRs / change | Tasks | Acceptance | Evidence | Blocker | Merged commits / dates | Updated |
| --- | --- | --- | --- | ---: | ---: | --- | --- | --- | --- |
| PR-039 | Todo | — | — | 0 | 0/9 | — | — | — | — |
| PR-040 | Todo | — | — | 0 | 0/5 | — | — | — | — |
| PR-041 | Todo | — | — | 0 | 0/4 | — | — | — | — |
| PR-042 | Todo | — | — | 0 | 0/5 | — | — | — | — |
| PR-043 | Todo | — | — | 0 | 0/5 | — | — | — | — |
| PR-044 | Todo | — | — | 0 | 0/4 | — | — | — | — |
| PR-045 | Todo | — | — | 0 | 0/4 | — | — | — | — |
| PR-046 | Todo | — | — | 0 | 0/5 | — | — | — | — |
| PR-047 | Todo | — | — | 0 | 0/5 | — | — | — | — |
| PR-048 | Todo | — | — | 0 | 0/11 | — | — | — | — |

### Phase 7

| Logical PR | Status | Owner | Issue / actual PRs / change | Tasks | Acceptance | Evidence | Blocker | Merged commits / dates | Updated |
| --- | --- | --- | --- | ---: | ---: | --- | --- | --- | --- |
| PR-049 | Todo | — | — | 0 | 0/7 | — | — | — | — |
| PR-050 | Todo | — | — | 0 | 0/5 | — | — | — | — |
| PR-051 | Todo | — | — | 0 | 0/4 | — | — | — | — |
| PR-052 | Todo | — | — | 0 | 0/4 | — | — | — | — |
| PR-053 | Todo | — | — | 0 | 0/4 | — | — | — | — |
| PR-054 | Todo | — | — | 0 | 0/4 | — | — | — | — |

### Phase 8

| Logical PR | Status | Owner | Issue / actual PRs / change | Tasks | Acceptance | Evidence | Blocker | Merged commits / dates | Updated |
| --- | --- | --- | --- | ---: | ---: | --- | --- | --- | --- |
| PR-055 | Todo | — | — | 0 | 0/5 | — | — | — | — |
| PR-056 | Todo | — | — | 0 | 0/8 | — | — | — | — |
| PR-057 | Todo | — | — | 0 | 0/5 | — | — | — | — |
| PR-058 | Todo | — | — | 0 | 0/6 | — | — | — | — |
| PR-059 | Todo | — | — | 0 | 0/4 | — | — | — | — |
| PR-060 | Todo | — | — | 0 | 0/6 | — | — | — | — |
| PR-061 | Todo | — | — | 0 | 0/5 | — | — | — | — |

### Phase 9

| Logical PR | Status | Owner | Issue / actual PRs / change | Tasks | Acceptance | Evidence | Blocker | Merged commits / dates | Updated |
| --- | --- | --- | --- | ---: | ---: | --- | --- | --- | --- |
| PR-062 | Todo | — | — | 0 | 0/4 | — | — | — | — |
| PR-063 | Todo | — | — | 0 | 0/4 | — | — | — | — |
| PR-064 | Todo | — | — | 0 | 0/4 | — | — | — | — |
| PR-065 | Todo | — | — | 0 | 0/4 | — | — | — | — |
| PR-066 | Todo | — | — | 0 | 0/4 | — | — | — | — |

## Actual PR mapping

Add one row for every hosting-platform pull request. The logical PR row links all of its actual PR rows and summarizes their merged commits and dates; completion is evaluated from these structured rows, not from a free-form list.

Actual PR status is `Planned`, `Draft`, `Open`, `In review`, `Merged`, or `Closed`. A `Closed` unmerged row requires a linked disposition or replacement. Do not reuse an actual PR row for a different logical PR.

| Actual PR | Logical PR | Status | Owner | Issue | Branch / URL | Head commit | Merged commit | Opened | Updated | Merged | Evidence / disposition |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |

<!-- Example shape only; remove this comment when adding the first real row.
| hosting-platform PR link/number | PR-NNN | Draft | person/team | issue link | branch and PR URL | full commit | — | YYYY-MM-DD | YYYY-MM-DD | — | scoped evidence ID or change reference |
-->

## Implementation task ledger

Create a task only when a logical PR is actively decomposed. Use a merge-safe ID of the form `PR-NNN-T-short-slug-xxxxxxxxxxxx`, where the final 12 lowercase hexadecimal characters are generated randomly when the row is created. Keep the ID stable and never reuse it. Keep task summaries implementation-specific; do not copy the plan's principal-change bullets.

| Task | Logical PR | Summary | Status | Owner | Depends on | Issue / branch / actual PR / change | Acceptance refs | Evidence refs | Opened | Updated | Completed / disposition |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |

<!-- Example shape only; remove this comment when adding the first real row.
| PR-NNN-T-short-slug-xxxxxxxxxxxx | PR-NNN | concise executable work | Ready | person/team | scoped task ID or — | links | PR-NNN-Ann | scoped evidence ID | YYYY-MM-DD | YYYY-MM-DD | — |
-->

## Blocker ledger

Blocker IDs use `<scope>-B-short-slug-xxxxxxxxxxxx`, with a stable scope such as `PR-018`, `PH4`, or `G4` and a random 12-hex suffix. This permits parallel branches without a central counter. A blocked status without a live row here is invalid.

| Blocker | Scope | Description | Owner | Opened | Next action | Review date | Issue | Status | Resolution evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |

<!-- Example shape only; remove this comment when adding the first real row.
| PR-NNN-B-short-slug-xxxxxxxxxxxx | PR-NNN or scoped task ID | concise blocking condition | person/team | YYYY-MM-DD | concrete action | YYYY-MM-DD | link | Open | — |
-->

Blocker status is `Open` or `Resolved`. Resolution requires an evidence reference and removal of the affected `Blocked` status in the same change.

## Required update transaction

For each state change, make the following updates together:

1. Update the relevant task and logical-PR rows, including owner and date.
2. Update or close any blocker row.
3. Add acceptance and validation records to the evidence register.
4. Update the ADR database when the work implements or enforces an ADR.
5. Add or close an exception when acceptance depends on a waiver.
6. Recalculate the phase coverage and status.
7. If requesting a gate review, add the approver and evidence set; update the gate result only after the decision is recorded.

Reviewers must reject a status transition whose linked records do not agree.
