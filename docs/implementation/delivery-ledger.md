# Delivery ledger

This is the canonical live checklist for implementation status. The [Implementation Plan](../planning/04-finstack-ai-implementation-plan.md) remains the source for phase purpose, dependencies, logical-PR titles and changes, acceptance evidence, exclusions, and gate requirements. Use an ID from this ledger to locate its authoritative entry in that plan.

## Current snapshot

Last updated 2026-08-08 and reconciled against documentation pack v0.12. Update this date and the totals below in every change that alters delivery state.

| Item | Planned | Done or passed | Current state |
| --- | ---: | ---: | --- |
| Phases | 10 | 0 | Phase 0 `In progress`; others `Todo` |
| Logical PRs | 66 | 4 | PR-001–PR-004 `Done`; PR-005 `In review`; PR-006–PR-066 `Todo` |
| PR acceptance-evidence bullets | 345 | 27 | PR-001 A01–A06 `Passed`; PR-002 A01–A07 closed (4 `Passed`, 3 `Not applicable`); PR-003 A01–A05 `Passed`; PR-004 A01–A05 `Passed`; PR-005 A01–A04 `Passed`, A05 `Pending` |
| Phase entrance and exit bullets | 62 | 0 | No coverage recorded |
| Program gates | 9 | 0 | All `Not ready` |
| Implementation tasks | 19 | 18 | PR-003 and PR-004 tasks complete; PR-005 tasks complete except post-merge G0 |
| Open blockers | 0 | 1 | PR-003-B-no-remote-ede93913b2ea Resolved |

PR-001 is `Done` at `73bfe88c8dbc92c4e4c6eba1a5a7814240c2e862`. PR-002 is `Done` at `ee9754fe2d0f015181dcefa97e715392aadd28ed`. PR-003 is `Done` at `9b0709a8cf2d96b418406f953e7bdc958925c274` (merge of [#1](https://github.com/jeickmeier/finstack-ai/pull/1)). PR-004 is `Done` at `9b13fe02d4cf41305daa20195eb0a537f85f9712` (merge of [#2](https://github.com/jeickmeier/finstack-ai/pull/2); A01–A05 Passed). PR-005 is `In progress` on branch `pr-005-harnesses`. Phase 0 and G0 remain incomplete until PR-005 finishes and a named G0 decision is recorded.

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
| [Phase 0](../planning/04-finstack-ai-implementation-plan.md#8-phase-0-foundation-and-architecture-governance) | PR-001–PR-005 | 0/1 | 0/5 | G0 | In progress | me@jeickmeier.com | PR-005 | — | PR-001–PR-004 Done; PR-005 In review on `pr-005-harnesses` (A05/G0 pending merge) | 2026-08-08 |
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
| PR-002 | Done | me@jeickmeier.com | `main` @ `ee9754fe2d0f015181dcefa97e715392aadd28ed` (local merge of `pr-002-architecture-enforcement`; no GitHub remote/PR) | 0 | 7/7 | PR-002-E-architecture-6ed3268e6ff5; PR-002-E-unit-tests-fc419e9d2920; PR-002-E-wasm-binding-5253da7a99f9; PR-002-E-dep-direction-321ce9b2b4b4; PR-002-E-kernel-deps-2745bac40199; PR-002-E-waiver-fa6bff499990; PR-002-E-review-627382618b20 | — | `ee9754fe2d0f015181dcefa97e715392aadd28ed` / 2026-08-08 | 2026-08-08 |
| PR-003 | Done | me@jeickmeier.com | [#1](https://github.com/jeickmeier/finstack-ai/pull/1) merged @ `9b0709a8cf2d96b418406f953e7bdc958925c274` | 7 | 5/5 | PR-003-E-hosted-ci-b7f2fe44f7c1; PR-003-E-hosted-release-smoke-be7df0f3c45c; PR-003-E-channel-ownership-7d912e4d6ce2; PR-003-E-generated-docs-ba54420a0521; PR-003-E-supply-chain-9c119be89fd4; PR-003-E-secret-scan-b09efb8d60cb; PR-003-E-security-review-fc22979d5bcb; PR-003-E-release-smoke-5ac88e7bdf8a | — | `9b0709a8cf2d96b418406f953e7bdc958925c274` / 2026-08-08 | 2026-08-08 |
| PR-004 | Done | me@jeickmeier.com | [#2](https://github.com/jeickmeier/finstack-ai/pull/2) merged @ `9b13fe02d4cf41305daa20195eb0a537f85f9712` | 6 | 5/5 | PR-004-E-schema-governance-4c7efdcf83d8; PR-004-E-unit-tests-7ef93e2201a8; PR-004-E-security-review-965513c26e6f; PR-004-E-ci-local-1db7faa0aa17; PR-004-E-hosted-ci-31274063724 | — | `9b13fe02d4cf41305daa20195eb0a537f85f9712` / 2026-08-08 | 2026-08-08 |
| PR-005 | In review | me@jeickmeier.com | [#3](https://github.com/jeickmeier/finstack-ai/pull/3) @ `8f6d678` | 6 | 4/5 | PR-005-E-conformance-cbbe1f123fea; PR-005-E-schema-d48af29171bb; PR-005-E-benchmark-064298d926a0; PR-005-E-ci-local-bf37275bab12; PR-005-E-supply-chain-6796d4c42c41; PR-005-E-security-review-0bda5e478fb1 | — | — | 2026-08-08 |

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
| [#1](https://github.com/jeickmeier/finstack-ai/pull/1) | PR-003 | Merged | me@jeickmeier.com | — | `pr-003a-hosted-ci-evidence` / https://github.com/jeickmeier/finstack-ai/pull/1 | `87e137c52ea7d738cda20eb949b3c7d51467e58c` | `9b0709a8cf2d96b418406f953e7bdc958925c274` | 2026-08-08 | 2026-08-08 | 2026-08-08 | PR-003-E-hosted-ci-b7f2fe44f7c1; PR-003-E-hosted-release-smoke-be7df0f3c45c |
| [#2](https://github.com/jeickmeier/finstack-ai/pull/2) | PR-004 | Merged | me@jeickmeier.com | — | `pr-004-adrs-schema-governance` / https://github.com/jeickmeier/finstack-ai/pull/2 | `7634342b93e57aae0f93e17fd685b2e954584c1a` | `9b13fe02d4cf41305daa20195eb0a537f85f9712` | 2026-08-08 | 2026-08-08 | 2026-08-08 | PR-004-E-hosted-ci-31274063724 |
| [#3](https://github.com/jeickmeier/finstack-ai/pull/3) | PR-005 | Open | me@jeickmeier.com | — | `pr-005-harnesses` / https://github.com/jeickmeier/finstack-ai/pull/3 | `8f6d678` | — | 2026-08-08 | 2026-08-08 | — | A01–A04 local evidence bound; awaiting hosted CI + merge for A05/G0 |

## Implementation task ledger

Create a task only when a logical PR is actively decomposed. Use a merge-safe ID of the form `PR-NNN-T-short-slug-xxxxxxxxxxxx`, where the final 12 lowercase hexadecimal characters are generated randomly when the row is created. Keep the ID stable and never reuse it. Keep task summaries implementation-specific; do not copy the plan's principal-change bullets.

| Task | Logical PR | Summary | Status | Owner | Depends on | Issue / branch / actual PR / change | Acceptance refs | Evidence refs | Opened | Updated | Completed / disposition |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| PR-003-T-tracking-333044ed64e8 | PR-003 | Open ledger tracking, pending acceptance, and hosted-CI blocker | Done | me@jeickmeier.com | — | `pr-003-ci-release-matrix` | PR-003-A01–A05 | — | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-003-T-tasks-40f4012de0ea | PR-003 | Add pinned mise tasks and contributor command docs | Done | me@jeickmeier.com | PR-003-T-tracking-333044ed64e8 | `pr-003-ci-release-matrix` | PR-003-A01–A02 | — | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-003-T-release-c6167c9102e8 | PR-003 | Add private facade-dependent release-smoke binary and reproducibility tooling | Done | me@jeickmeier.com | PR-003-T-tasks-40f4012de0ea | `pr-003-ci-release-matrix` | PR-003-A03 | PR-003-E-release-smoke-5ac88e7bdf8a | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-003-T-security-a1fff419af88 | PR-003 | Implement cargo-deny, secret scan, and isolated canary-negative tests | Done | me@jeickmeier.com | PR-003-T-tasks-40f4012de0ea | `pr-003-ci-release-matrix` | PR-003-A05 | PR-003-E-supply-chain-9c119be89fd4; PR-003-E-secret-scan-b09efb8d60cb | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-003-T-workflows-0f6986484139 | PR-003 | Add CI/security/nightly workflows and reservation docs | Done | me@jeickmeier.com | PR-003-T-tasks-40f4012de0ea; PR-003-T-release-c6167c9102e8; PR-003-T-security-a1fff419af88 | `pr-003-ci-release-matrix` | PR-003-A01–A04 | PR-003-E-channel-ownership-7d912e4d6ce2; PR-003-E-generated-docs-ba54420a0521 | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-003-T-evidence-4d511b278623 | PR-003 | Run local/hosted verification, security review, and bind evidence | Done | me@jeickmeier.com | PR-003-T-workflows-0f6986484139 | [#1](https://github.com/jeickmeier/finstack-ai/pull/1) | PR-003-A01–A05 | local + hosted evidence bound | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-003-T-hosted-bb75dc133f03 | PR-003 | Open GitHub remote/PR-003a and bind hosted Linux/macOS/Windows A01/A03 evidence | Done | me@jeickmeier.com | PR-003-T-evidence-4d511b278623 | [#1](https://github.com/jeickmeier/finstack-ai/pull/1) @ `c2f4e79498c17dcea334d0e67b41289474cd2acd` | PR-003-A01; PR-003-A03 | PR-003-E-hosted-ci-b7f2fe44f7c1; PR-003-E-hosted-release-smoke-be7df0f3c45c | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-004-T-tracking-8a01f5662f4c | PR-004 | Open ledger tracking and pending acceptance for ADR/schema governance | Done | me@jeickmeier.com | — | `pr-004-adrs-schema-governance` | PR-004-A01–A05 | — | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-004-T-checker-6c8746c6141c | PR-004 | Add schema-governance checker tests and mise tasks | Done | me@jeickmeier.com | PR-004-T-tracking-8a01f5662f4c | `pr-004-adrs-schema-governance` | PR-004-A01; PR-004-A03; PR-004-A04 | local test-schema-governance | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-004-T-adrs-4aabc549ac5f | PR-004 | Materialize standalone ADR-001 through ADR-037 with security cross-links | Done | me@jeickmeier.com | PR-004-T-tracking-8a01f5662f4c | `pr-004-adrs-schema-governance` | PR-004-A01; PR-004-A05 | local schema-governance + security-review | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-004-T-schema-f1eb2fb50ac3 | PR-004 | Add contract registry, reserved schema/fixture roots, and change template | Done | me@jeickmeier.com | PR-004-T-tracking-8a01f5662f4c | `pr-004-adrs-schema-governance` | PR-004-A02; PR-004-A04 | local schema-governance GOV004 | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-004-T-ci-ccc7ea14f888 | PR-004 | Wire PR template, local CI, and hosted schema-governance enforcement | Done | me@jeickmeier.com | PR-004-T-checker-6c8746c6141c; PR-004-T-schema-f1eb2fb50ac3 | `pr-004-adrs-schema-governance` | PR-004-A03; PR-004-A04 | local schema-governance GOV007 + ci.yml job | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-004-T-evidence-ac51c9bc1666 | PR-004 | Run focused/full validation and bind local review evidence | Done | me@jeickmeier.com | PR-004-T-adrs-4aabc549ac5f; PR-004-T-ci-ccc7ea14f888 | [#2](https://github.com/jeickmeier/finstack-ai/pull/2) @ `9b13fe02d4cf41305daa20195eb0a537f85f9712` | PR-004-A01–A05 | PR-004-E-schema-governance-4c7efdcf83d8; PR-004-E-unit-tests-7ef93e2201a8; PR-004-E-security-review-965513c26e6f; PR-004-E-ci-local-1db7faa0aa17; PR-004-E-hosted-ci-31274063724 | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-005-T-tracking-a039ae985af1 | PR-005 | Open ledger tracking and pending acceptance for harness work | Done | me@jeickmeier.com | — | `pr-005-harnesses` | PR-005-A01–A05 | — | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-005-T-schemas-5e580636e3b8 | PR-005 | Add golden-trace and benchmark-report schemas with boundary fixtures | Done | me@jeickmeier.com | PR-005-T-tracking-a039ae985af1 | `pr-005-harnesses` | PR-005-A02; PR-005-A04 | PR-005-E-schema-d48af29171bb | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-005-T-harness-a44bd566ad97 | PR-005 | Implement Rust trace/conformance harness and deferred adapters | Done | me@jeickmeier.com | PR-005-T-schemas-5e580636e3b8 | `pr-005-harnesses` | PR-005-A01; PR-005-A02; PR-005-A04 | PR-005-E-conformance-cbbe1f123fea | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-005-T-benchmark-1c51653de533 | PR-005 | Add Criterion benches and machine-readable benchmark metadata | Done | me@jeickmeier.com | PR-005-T-harness-a44bd566ad97 | `pr-005-harnesses` | PR-005-A03 | PR-005-E-benchmark-064298d926a0 | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-005-T-ci-58972ee51950 | PR-005 | Wire mise tasks and non-blocking benchmark CI | Done | me@jeickmeier.com | PR-005-T-benchmark-1c51653de533 | `pr-005-harnesses` | PR-005-A01; PR-005-A03 | PR-005-E-ci-local-bf37275bab12 | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-005-T-evidence-54a7e6eb419c | PR-005 | Run validation and bind A01–A04 evidence; leave G0 for post-merge | Done | me@jeickmeier.com | PR-005-T-ci-58972ee51950 | `pr-005-harnesses` | PR-005-A01–A04 | local evidence bound; A05 pending G0 | 2026-08-08 | 2026-08-08 | 2026-08-08 |

## Blocker ledger

Blocker IDs use `<scope>-B-short-slug-xxxxxxxxxxxx`, with a stable scope such as `PR-018`, `PH4`, or `G4` and a random 12-hex suffix. This permits parallel branches without a central counter. A blocked status without a live row here is invalid.

| Blocker | Scope | Description | Owner | Opened | Next action | Review date | Issue | Status | Resolution evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| PR-003-B-no-remote-ede93913b2ea | PR-003 | Hosted PR runs and immutable Linux/macOS/Windows CI evidence were blocked until a Git remote existed. | me@jeickmeier.com | 2026-08-08 | — | 2026-08-08 | https://github.com/jeickmeier/finstack-ai | Resolved | PR-003-E-hosted-ci-b7f2fe44f7c1; PR-003-E-hosted-release-smoke-be7df0f3c45c |

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
