# Delivery ledger

This is the canonical live checklist for implementation status. The [Implementation Plan](../planning/04-finstack-ai-implementation-plan.md) remains the source for phase purpose, dependencies, logical-PR titles and changes, acceptance evidence, exclusions, and gate requirements. Use an ID from this ledger to locate its authoritative entry in that plan.

## Current snapshot

Last updated 2026-08-11 and reconciled against documentation pack v0.20, PR-014 merge `399f3a7d9d987268f4d79ab90f31b93f854084f8`, local PR-015 merge `16f3a865d122aac34d1df5b3dc1d7a0b44c4aa82`, local PR-016 merge `b2f678693fcd3eb2fe09aafca55c4e06c17371ec`, local PR-017 merge `966f047fd28972d35c835ddf0441f8ba67348b25`, local PR-018 merge `3fe0314c6434211e1c8f493f24401888f9609050`, local PR-019 merge `bf748d5aa1ee7ffa11cf9ac3ab2dea399034dd0b`, local PR-020 merge `f101b260dd6c9ca6ffa82d805824669c5d974ca0`, and local PR-021 merge `ec56d175a8b31fe49d5ea672f89020bc3ec553cd`. Update this date and the totals below in every change that alters delivery state.

| Item | Planned | Done or passed | Current state |
| --- | ---: | ---: | --- |
| Phases | 10 | 3 | Phase 0–2 `Done`; Phase 3 `In progress`; Phase 4–9 `Todo` |
| Logical PRs | 66 | 23 | PR-001–PR-023 `Done`; PR-024–PR-066 `Todo` |
| PR acceptance-evidence bullets | 345 | 127 | PR-001 A01–A06 `Passed`; PR-002 A01–A07 closed (4 `Passed`, 3 `Not applicable`); PR-003–PR-006 A01–A05 `Passed`; PR-007 A01–A04 `Passed`; PR-008 A01–A08 `Passed` after review remediation; PR-009 A01–A05 `Passed`; PR-010 A01–A04 `Passed`; PR-011 A01–A06 `Passed`; PR-012 A01–A05 `Passed`; PR-013 A01–A04 `Passed`; PR-014 A01–A07 `Passed`; PR-015 A01–A04, PR-016 A01–A05, PR-017 A01–A04, PR-018 A01–A08, PR-019 A01–A04, PR-020 A01–A04, PR-021 A01–A05, PR-022 A01–A12, and PR-023 A01–A05 `Passed` with immutable local integration evidence |
| Phase entrance and exit bullets | 62 | 20 | Phase 0 entrance 1/1 and exit 5/5 `Passed`; Phase 1 entrance 2/2 and exit 4/4 `Passed`; Phase 2 entrance 2/2 and exit 4/4 `Passed`; Phase 3 entrance 2/2 `Passed` |
| Program gates | 9 | 3 | G0–G2 `Passed`; G3–G8 `Not ready` |
| Implementation tasks | 120 | 120 | PR-023 implementation, immutable candidate validation, security review, and local integration are complete |
| Open blockers | 0 | 3 | PR-003-B-no-remote-ede93913b2ea Resolved; PR-007-B-contract-0c60297af156 Resolved; PR-008-B-contract-ed6437d9ed08 Resolved |

PR-001 is `Done` at `73bfe88c8dbc92c4e4c6eba1a5a7814240c2e862`. PR-002 is `Done` at `ee9754fe2d0f015181dcefa97e715392aadd28ed`. PR-003 is `Done` at `9b0709a8cf2d96b418406f953e7bdc958925c274` (merge of [#1](https://github.com/jeickmeier/finstack-ai/pull/1)). PR-004 is `Done` at `9b13fe02d4cf41305daa20195eb0a537f85f9712` (merge of [#2](https://github.com/jeickmeier/finstack-ai/pull/2); A01–A05 Passed). PR-005 is `Done` at `c1108d207389a947d16e9b0dd7a76026108c01eb` (merge of [#3](https://github.com/jeickmeier/finstack-ai/pull/3); A01–A05 Passed). PR-006 is `Done` at `56d7777956df145213b03d2b0b5c1922db42b346` (merge of [#4](https://github.com/jeickmeier/finstack-ai/pull/4); A01–A05 Passed). PR-007 is `Done` at local merge `81a8706aeeca6a47ab0d64bc0bef681d6efc4621` (A01–A04 Passed). PR-008 is `Done` at merge `4b68a9397a8e07a581f34dfc34f0bfb96873c00d` ([#5](https://github.com/jeickmeier/finstack-ai/pull/5); A01–A08 Passed after review remediation and final hosted/merge verification). PR-009 is `Done` at local `main` integration `5843dce6d77498a75acdc15d816586cb26098456` (A01–A05 Passed; no GitHub issue or actual PR). PR-010 is `Done` at local `main` integration `ff2e6e7b80e34061dae4dcc5ceb4b259a34a89b5` (A01–A04 Passed; no GitHub issue or actual PR). PR-011 is `Done` at local `main` integration `01380ead5c5ca7b9e7d28d681d84719c9bf0279e` (A01–A06 Passed; no GitHub issue or actual PR). PR-012 is `Done` at local `main` integration `dc58a11fbc871e70326d04fd9840297b5179023f` (fast-forward of `codex/pr-012-structured-output`; A01–A05 Passed; no GitHub issue or actual PR). PR-013 is `Done` at merge `fa6222f20e4a4616f600e867be94afe12967dcb9` ([#6](https://github.com/jeickmeier/finstack-ai/pull/6); A01–A04 Passed). PR-014 is `Done` at merge `399f3a7d9d987268f4d79ab90f31b93f854084f8` ([#8](https://github.com/jeickmeier/finstack-ai/pull/8); A01–A07 Passed after hosted and immutable merge verification). PR-015 is `Done` at local `main` merge `16f3a865d122aac34d1df5b3dc1d7a0b44c4aa82` (A01–A04 Passed; no GitHub issue or actual PR). PR-016 is `Done` at local `main` merge `b2f678693fcd3eb2fe09aafca55c4e06c17371ec` (A01–A05 Passed; no GitHub issue or actual PR). Phase 0 and Phase 1 are `Done`. G0 passed via `G0-D-foundation-ready-bcf021e4873a`; G1 passed via `G1-D-kernel-semantics-4f52c8a91d6e`. Both Phase 2 entrance criteria are passed.

PR-017 is `Done` at local `main` merge `966f047fd28972d35c835ddf0441f8ba67348b25` (A01–A04 Passed; no GitHub issue or actual PR).

PR-018 is `Done` at local `main` merge `3fe0314c6434211e1c8f493f24401888f9609050` (A01–A08 Passed; no GitHub issue or actual PR).

PR-019 is `Done` at local `main` merge `bf748d5aa1ee7ffa11cf9ac3ab2dea399034dd0b` (A01–A04 Passed; no GitHub issue or actual PR).

PR-020 is `Done` at local `main` merge `f101b260dd6c9ca6ffa82d805824669c5d974ca0` (A01–A04 Passed; no GitHub issue or actual PR). All four Phase 2 exits passed, Phase 2 is `Done`, G2 passed via `G2-D-native-runtime-7c2e9a4d1b65`, and both Phase 3 entrance criteria passed.

PR-021 is `Done` at local `main` merge `ec56d175a8b31fe49d5ea672f89020bc3ec553cd` (A01–A05 Passed; no GitHub issue or actual PR).

## Active execution envelope

PR-018 through PR-026 are authorized for sequential execution in `integrated` mode against target `main`, starting from branch `main` at `3e699c37865b0f9c97b3f956fe3314bb6a734c0b`. Local worktree, branch, commit, and merge actions are authorized. Pushes, hosted pull requests or merges, publication, and other external actions are not authorized. Plan baseline: documentation pack v0.20 / Implementation Plan SHA-256 `555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`. G2 and G3 decisions are delegated to `me@jeickmeier.com` only after their complete immutable integration and phase-exit evidence passes; neither decision is pre-recorded or inferred.

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
| [Phase 0](../planning/04-finstack-ai-implementation-plan.md#8-phase-0-foundation-and-architecture-governance) | PR-001–PR-005 | 1/1 | 5/5 | G0 | Done | me@jeickmeier.com | — | — | PH0-E-entrance-abbcbb8c4715; PH0-E-exit-2ada36eb4a4b; G0-D-foundation-ready-bcf021e4873a @ `c1108d207389a947d16e9b0dd7a76026108c01eb` | 2026-08-08 |
| [Phase 1](../planning/04-finstack-ai-implementation-plan.md#9-phase-1-semantic-agent-microkernel) | PR-006–PR-013 | 2/2 | 4/4 | G1 | Done | me@jeickmeier.com | — | — | PH1-E-entrance-g0-3467dedeb0bd; PH1-E-entrance-adrs-1284c162dae7; PH1-E-exit-kernel-fa6222f20e4a; G1-D-kernel-semantics-4f52c8a91d6e @ `fa6222f20e4a4616f600e867be94afe12967dcb9` | 2026-08-09 |
| [Phase 2](../planning/04-finstack-ai-implementation-plan.md#10-phase-2-native-runtime-and-effect-execution) | PR-014–PR-020 | 2/2 | 4/4 | G2 | Done | me@jeickmeier.com | — | — | PH2-E-entrance-g1-2a7d9c4e6b13; PH2-E-entrance-kernel-6f3b8d1a5c72; PH2-E-exit-e2e-3d7a1c9e5b62; PH2-E-exit-faults-8e2c6a4d1b75; PH2-E-exit-bounds-5b9d3e7a2c61; PH2-E-exit-lifecycle-1c6f8a4d9b32; PR-020-E-integration-9a4c2e7d5b81; G2-D-native-runtime-7c2e9a4d1b65 @ `f101b260dd6c9ca6ffa82d805824669c5d974ca0` | 2026-08-11 |
| [Phase 3](../planning/04-finstack-ai-implementation-plan.md#11-phase-3-rust-sdk-and-native-developer-preview) | PR-021–PR-026 | 2/2 | 0/4 | G3 | In progress | me@jeickmeier.com | — | — | PH3-E-entrance-g2-4d8a1c6e9b35; PH3-E-entrance-ports-6b2e9d4a7c15; PR-021-E-sdk-8a4d2c6f1b73; PR-021-E-compat-3e7b9d1a5c82; PR-021-E-security-6c2f8a4d1b75; PR-021-E-ci-9d3a7c1e5b84; PR-021-E-integration-ec56d175a8b3 | 2026-08-11 |
| [Phase 4](../planning/04-finstack-ai-implementation-plan.md#12-phase-4-first-class-python-bindings) | PR-027–PR-032 | 0/2 | 0/4 | G4 | Todo | — | — | — | — | — |
| [Phase 5](../planning/04-finstack-ai-implementation-plan.md#13-phase-5-browser-and-javascript-webassembly-bindings) | PR-033–PR-038 | 0/2 | 0/4 | G4 | Todo | — | — | — | — | — |
| [Phase 6](../planning/04-finstack-ai-implementation-plan.md#14-phase-6-durability-recovery-and-lanes) | PR-039–PR-048 | 0/3 | 0/4 | G5 | Todo | — | — | — | — | — |
| [Phase 7](../planning/04-finstack-ai-implementation-plan.md#15-phase-7-isolated-witwasmtime-extensions) | PR-049–PR-054 | 0/3 | 0/4 | G6 | Todo | — | — | — | — | — |
| [Phase 8](../planning/04-finstack-ai-implementation-plan.md#16-phase-8-ecosystem-readiness-and-public-preview) | PR-055–PR-061 | 0/2 | 0/4 | G7 | Todo | — | — | — | — | — |
| [Phase 9](../planning/04-finstack-ai-implementation-plan.md#17-phase-9-10-hardening-and-general-availability) | PR-062–PR-066 | 0/2 | 0/4 | G8 | Todo | — | — | — | — | — |

## Gate ledger

| Gate | Scope | Status | Approver | Decision record | Evidence | Decision date | Remediation |
| --- | --- | --- | --- | --- | --- | --- | --- |
| G0 | Phase 0 | Passed | me@jeickmeier.com | G0-D-foundation-ready-bcf021e4873a | PH0-E-entrance-abbcbb8c4715; PH0-E-exit-2ada36eb4a4b; PR-005-E-hosted-ci-994e52f8f0de | 2026-08-08 | — |
| G1 | Phase 1 | Passed | me@jeickmeier.com | G1-D-kernel-semantics-4f52c8a91d6e | PH1-E-exit-kernel-fa6222f20e4a; PR-013-E-hosted-ci-31339759492; PR-013-E-long-fuzz-31339772213; PR-013-E-security-7d2a5f9c1e84 | 2026-08-09 | — |
| G2 | Phase 2 | Passed | me@jeickmeier.com | G2-D-native-runtime-7c2e9a4d1b65 | PH2-E-exit-e2e-3d7a1c9e5b62; PH2-E-exit-faults-8e2c6a4d1b75; PH2-E-exit-bounds-5b9d3e7a2c61; PH2-E-exit-lifecycle-1c6f8a4d9b32; PR-020-E-integration-9a4c2e7d5b81; PR-020-E-security-7b3d1a8c5e92 | 2026-08-11 | — |
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
| PR-005 | Done | me@jeickmeier.com | [#3](https://github.com/jeickmeier/finstack-ai/pull/3) merged @ `c1108d207389a947d16e9b0dd7a76026108c01eb` | 7 | 5/5 | PR-005-E-conformance-cbbe1f123fea; PR-005-E-schema-d48af29171bb; PR-005-E-benchmark-064298d926a0; PR-005-E-ci-local-bf37275bab12; PR-005-E-supply-chain-6796d4c42c41; PR-005-E-security-review-0bda5e478fb1; PR-005-E-hosted-ci-994e52f8f0de; G0-D-foundation-ready-bcf021e4873a | — | `c1108d207389a947d16e9b0dd7a76026108c01eb` / 2026-08-08 | 2026-08-08 |

### Phase 1

| Logical PR | Status | Owner | Issue / actual PRs / change | Tasks | Acceptance | Evidence | Blocker | Merged commits / dates | Updated |
| --- | --- | --- | --- | ---: | ---: | --- | --- | --- | --- |
| PR-006 | Done | me@jeickmeier.com | [#4](https://github.com/jeickmeier/finstack-ai/pull/4) merged @ `56d7777956df145213b03d2b0b5c1922db42b346` | 6 | 5/5 | PR-006-E-test-kernel-bc373cf935e4; PR-006-E-conformance-adfd96007628; PR-006-E-architecture-90b6f267a46d; PR-006-E-ci-5565fde35cf6; PR-006-E-security-c463d4eef6a2; PR-006-E-supply-bbbb7b4e6da4; PR-006-E-hosted-ci-9a1ccbb88ae5 | — | `56d7777956df145213b03d2b0b5c1922db42b346` / 2026-08-08 | 2026-08-08 |
| PR-007 | Done | me@jeickmeier.com | local merge on `main` @ `81a8706aeeca6a47ab0d64bc0bef681d6efc4621` | 7 | 4/4 | PR-007-E-test-kernel-57ac9861bcfb; PR-007-E-conformance-8b7b9c728785; PR-007-E-check-wasm-5aefa861beb5; PR-007-E-architecture-e59be4b35311; PR-007-E-docs-51eca2ecc0a3; PR-007-E-schema-final-4c7efdcf83d8; PR-007-E-supply-final-183fc11ba3c3; PR-007-E-security-1146ce970799; PR-007-E-ci-a29042aa7d6a; PR-007-E-merge-ci-acf14a6a28ca; PR-007-E-merge-wasm-3713f4641342 | — | `81a8706aeeca6a47ab0d64bc0bef681d6efc4621` / 2026-08-08 | 2026-08-08 |
| PR-008 | Done | me@jeickmeier.com | [#5](https://github.com/jeickmeier/finstack-ai/pull/5) merged @ `4b68a9397a8e07a581f34dfc34f0bfb96873c00d` | 9 | 8/8 | PR-008-E-remediation-test-a8ae4dbcdf37; PR-008-E-remediation-conformance-24e99a1432e8; PR-008-E-remediation-schema-209e13c5594d; PR-008-E-remediation-security-c7141f3b3c7f; PR-008-E-remediation-ci-3d74148b759a; PR-008-E-final-hosted-ci-e1b5e2591cc4; PR-008-E-merge-ci-eb68551e3bac; PR-008-E-merge-wasm-032694ffd669 | — | `4b68a9397a8e07a581f34dfc34f0bfb96873c00d` / 2026-08-08 | 2026-08-08 |
| PR-009 | Done | me@jeickmeier.com | local `main` integration @ `5843dce6d77498a75acdc15d816586cb26098456` (no GitHub issue or actual PR) | 5 | 5/5 | PR-009-E-merge-kernel-f294c67a385b; PR-009-E-merge-conformance-03a5d78b496c; PR-009-E-merge-wasm-14b6e89c5a7d; PR-009-E-security-d072a4581639; PR-009-E-ci-cf6193470528 | — | `5843dce6d77498a75acdc15d816586cb26098456` / 2026-08-09 | 2026-08-09 |
| PR-010 | Done | me@jeickmeier.com | local `main` integration @ `ff2e6e7b80e34061dae4dcc5ceb4b259a34a89b5` (fast-forward of `codex/pr-010-tool-reducer`; no GitHub issue or actual PR) | 5 | 4/4 | PR-010-E-merge-kernel-d8f6a2c9017b; PR-010-E-merge-conformance-71e4c3a8b205; PR-010-E-merge-wasm-5c0e7f4b92ad; PR-010-E-security-9ab3d6e1f470; PR-010-E-ci-f2c84d1a6b39 | — | `ff2e6e7b80e34061dae4dcc5ceb4b259a34a89b5` / 2026-08-09 | 2026-08-09 |
| PR-011 | Done | me@jeickmeier.com | local `main` integration @ `01380ead5c5ca7b9e7d28d681d84719c9bf0279e` (fast-forward of `codex/pr-011-kernel-termination`; no GitHub issue or actual PR) | 5 | 6/6 | PR-011-E-kernel-c8a314829b86; PR-011-E-conformance-4aae0a67b162; PR-011-E-wasm-64c7a443c3e6; PR-011-E-ci-d10ca2e00eba; PR-011-E-benchmark-8bf15225d8e7; PR-011-E-security-c338d5a3a19c | — | `01380ead5c5ca7b9e7d28d681d84719c9bf0279e` / 2026-08-09 | 2026-08-09 |
| PR-012 | Done | me@jeickmeier.com | local `main` integration @ `dc58a11fbc871e70326d04fd9840297b5179023f` (fast-forward of `codex/pr-012-structured-output`; no GitHub issue or actual PR) | 5 | 5/5 | PR-012-E-kernel-29bab69145af; PR-012-E-conformance-9772f14d868c; PR-012-E-ci-c9bbb3caa845; PR-012-E-security-829f512af935 | — | `dc58a11fbc871e70326d04fd9840297b5179023f` / 2026-08-09 | 2026-08-09 |
| PR-013 | Done | me@jeickmeier.com | [#6](https://github.com/jeickmeier/finstack-ai/pull/6) merged @ `fa6222f20e4a4616f600e867be94afe12967dcb9` | 5 | 4/4 | PR-013-E-golden-7f2c19a4d8e6; PR-013-E-kernel-3b7e91c5a2d4; PR-013-E-conformance-8c1d6f4a9b27; PR-013-E-fuzz-smoke-5e9a2c7d1f63; PR-013-E-wasm-6a4f8d2c9e15; PR-013-E-ci-1a6d9c3e7b25; PR-013-E-security-7d2a5f9c1e84; PR-013-E-hosted-ci-31339759492; PR-013-E-long-fuzz-31339772213; PR-013-E-integration-fa6222f20e4a; G1-D-kernel-semantics-4f52c8a91d6e | — | `fa6222f20e4a4616f600e867be94afe12967dcb9` / 2026-08-09 | 2026-08-09 |

### Phase 2

| Logical PR | Status | Owner | Issue / actual PRs / change | Tasks | Acceptance | Evidence | Blocker | Merged commits / dates | Updated |
| --- | --- | --- | --- | ---: | ---: | --- | --- | --- | --- |
| PR-014 | Done | me@jeickmeier.com | [#8](https://github.com/jeickmeier/finstack-ai/pull/8) merged @ `399f3a7d9d987268f4d79ab90f31b93f854084f8` | 5 | 7/7 | PH2-E-entrance-g1-2a7d9c4e6b13; PH2-E-entrance-kernel-6f3b8d1a5c72; PR-014-E-contracts-fae82274d718; PR-014-E-store-0f061d363c0a; PR-014-E-runtime-f7d0f0ecede0; PR-014-E-wasm-1de6c1fbd157; PR-014-E-architecture-4082c9d28550; PR-014-E-ci-cef693ae9837; PR-014-E-security-a8a835efcf19; PR-014-E-hosted-ci-abf368bf148b; PR-014-E-integration-e7ec699722ab | — | `399f3a7d9d987268f4d79ab90f31b93f854084f8` / 2026-08-10 | 2026-08-10 |
| PR-015 | Done | me@jeickmeier.com | local `main` merge @ `16f3a865d122aac34d1df5b3dc1d7a0b44c4aa82` of `codex/pr-015-model-port` (no issue or actual PR) | 5 | 4/4 | PR-015-E-model-4e91b7c2d6a8; PR-015-E-compat-9c3f2a7e5b10; PR-015-E-security-2d8a6f1c4b79; PR-015-E-ci-7b5e1d9a3c62; PR-015-E-integration-a6c2e8f14d53 | — | `16f3a865d122aac34d1df5b3dc1d7a0b44c4aa82` / 2026-08-10 | 2026-08-10 |
| PR-016 | Done | me@jeickmeier.com | local `main` merge @ `b2f678693fcd3eb2fe09aafca55c4e06c17371ec` of `codex/pr-016-toolset-scheduler` (no issue or actual PR) | 5 | 5/5 | PR-016-E-tool-8f2c4d7a1b63; PR-016-E-compat-3e9a6c1d5b72; PR-016-E-security-7c1e5a9d3b84; PR-016-E-release-2a6f8d4c1e95; PR-016-E-ci-5d9b2e7a4c16; PR-016-E-integration-b2f678693fcd | — | `b2f678693fcd3eb2fe09aafca55c4e06c17371ec` / 2026-08-10 | 2026-08-10 |
| PR-017 | Done | me@jeickmeier.com | local `main` merge @ `966f047fd28972d35c835ddf0441f8ba67348b25` of `codex/pr-017-event-hub` (no issue or actual PR) | 5 | 4/4 | PR-017-E-events-a3f7c9e1d5b2; PR-017-E-compat-6d2a8f4c1e93; PR-017-E-security-9b5e1c7a3d64; PR-017-E-ci-4f8a2d6c9b15; PR-017-E-integration-966f047fd289 | — | `966f047fd28972d35c835ddf0441f8ba67348b25` / 2026-08-10 | 2026-08-10 |
| PR-018 | Done | me@jeickmeier.com | local `main` merge @ `3fe0314c6434211e1c8f493f24401888f9609050` of `codex/pr-018-context-middleware` (no issue or actual PR) | 5 | 8/8 | PR-018-E-extensions-8c2a6f4d1b73; PR-018-E-compat-5e9b3d7a2c16; PR-018-E-security-7a1d4f8c3b62; PR-018-E-ci-2f6c9a5e1d84; PR-018-E-integration-3fe0314c6434 | — | `3fe0314c6434211e1c8f493f24401888f9609050` / 2026-08-10 | 2026-08-10 |
| PR-019 | Done | me@jeickmeier.com | local `main` merge @ `bf748d5aa1ee7ffa11cf9ac3ab2dea399034dd0b` of `codex/pr-019-cancellation-timers` (no issue or actual PR) | 5 | 4/4 | PR-019-E-lifecycle-6a2d8f4c1b73; PR-019-E-compat-9e3b7d1a5c82; PR-019-E-security-4c8e1a7d3b65; PR-019-E-ci-2f7a9c5e1d84; PR-019-E-integration-bf748d5aa1ee | — | `bf748d5aa1ee7ffa11cf9ac3ab2dea399034dd0b` / 2026-08-10 | 2026-08-10 |
| PR-020 | Done | me@jeickmeier.com | local `main` merge @ `f101b260dd6c9ca6ffa82d805824669c5d974ca0` of `codex/pr-020-native-runtime-gate` (no issue or actual PR) | 5 | 4/4 | PR-020-E-faults-6d3a8f1c5b72; PR-020-E-miri-2a7e4c9d1b63; PR-020-E-stress-8c1f5a3d7b94; PR-020-E-benchmark-4e9b2d6a1c75; PR-020-E-compat-5c2e7a9d4b16; PR-020-E-security-7b3d1a8c5e92; PR-020-E-ci-1f6a9c4e2b83; PR-020-E-integration-9a4c2e7d5b81; G2-D-native-runtime-7c2e9a4d1b65 | — | `f101b260dd6c9ca6ffa82d805824669c5d974ca0` / 2026-08-11 | 2026-08-11 |

### Phase 3

| Logical PR | Status | Owner | Issue / actual PRs / change | Tasks | Acceptance | Evidence | Blocker | Merged commits / dates | Updated |
| --- | --- | --- | --- | ---: | ---: | --- | --- | --- | --- |
| PR-021 | Done | me@jeickmeier.com | local `main` merge `ec56d175a8b31fe49d5ea672f89020bc3ec553cd` of `codex/pr-021-rust-sdk` (no issue or actual PR) | 5 | 5/5 | PH3-E-entrance-g2-4d8a1c6e9b35; PH3-E-entrance-ports-6b2e9d4a7c15; PR-021-E-sdk-8a4d2c6f1b73; PR-021-E-compat-3e7b9d1a5c82; PR-021-E-security-6c2f8a4d1b75; PR-021-E-ci-9d3a7c1e5b84; PR-021-E-integration-ec56d175a8b3 | — | `ec56d175a8b31fe49d5ea672f89020bc3ec553cd` / 2026-08-11 | 2026-08-11 |
| PR-022 | Done | me@jeickmeier.com | local `main` merge `9f4385b588ba840690c0b82f1816ee8378b2740e` of `codex/pr-022-agent-spec` (no issue or actual PR) | 7 | 12/12 | PR-022-E-specs-a6c2e8f14d53; PR-022-E-composition-b4e7c1a9d365; PR-022-E-compat-c9f2a6d1e847; PR-022-E-security-d3a7e1c5b962; PR-022-E-ci-e8c4a1d7b356; PR-022-E-integration-f2d6a9c3e174 | — | `9f4385b588ba840690c0b82f1816ee8378b2740e` / 2026-08-11 | 2026-08-11 |
| PR-023 | Done | me@jeickmeier.com | local `main` merge `949f34010a2fe121b62681560f00eac1f58dfe76` of `codex/pr-023-test-kit` (no issue or actual PR) | 6 | 5/5 | PR-023-E-kit-5c8a2d7e1b64; PR-023-E-external-8e3b6a1d5c72; PR-023-E-compaction-4d9a7c2e6b15; PR-023-E-security-7b2e4a9d1c65; PR-023-E-ci-1d6a4c9e2b83; PR-023-E-integration-949f34010a2f | — | `949f34010a2fe121b62681560f00eac1f58dfe76` / 2026-08-11 | 2026-08-11 |
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
| [#3](https://github.com/jeickmeier/finstack-ai/pull/3) | PR-005 | Merged | me@jeickmeier.com | — | `pr-005-harnesses` / https://github.com/jeickmeier/finstack-ai/pull/3 | `9b183e9baa574e30972568a6a8b26510630bb10b` | `c1108d207389a947d16e9b0dd7a76026108c01eb` | 2026-08-08 | 2026-08-08 | 2026-08-08 | PR-005-E-hosted-ci-994e52f8f0de; G0-D-foundation-ready-bcf021e4873a |
| [#5](https://github.com/jeickmeier/finstack-ai/pull/5) | PR-008 | Merged | me@jeickmeier.com | — | `pr-008-contract-amendment` / https://github.com/jeickmeier/finstack-ai/pull/5 | `44d86b0c8371f6ee4e9ad14014e9fc35ea807c3a` | `4b68a9397a8e07a581f34dfc34f0bfb96873c00d` | 2026-08-08 | 2026-08-08 | 2026-08-08 | PR-008-E-final-hosted-ci-e1b5e2591cc4; PR-008-E-merge-ci-eb68551e3bac; PR-008-E-merge-wasm-032694ffd669 |

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
| PR-005-T-g0-closure-bcf021e4873a | PR-005 | Record Phase 0 exit evidence and named G0 decision after merge | Done | me@jeickmeier.com | PR-005-T-evidence-54a7e6eb419c | [#3](https://github.com/jeickmeier/finstack-ai/pull/3) @ `c1108d207389a947d16e9b0dd7a76026108c01eb` | PR-005-A05; PH0-ENT-A01; PH0-EXIT-A01–A05 | PH0-E-entrance-abbcbb8c4715; PH0-E-exit-2ada36eb4a4b; PR-005-E-hosted-ci-994e52f8f0de; G0-D-foundation-ready-bcf021e4873a | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-006-T-tracking-9321fa243afb | PR-006 | Open Phase 1 entrance and PR-006 ledger tracking | Done | me@jeickmeier.com | — | `pr-006-kernel-value-types` | PH1-ENT-A01; PH1-ENT-A02; PR-006-A01–A05 | PH1-E-entrance-g0-3467dedeb0bd; PH1-E-entrance-adrs-1284c162dae7 | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-006-T-ids-time-916b8ba52395 | PR-006 | Implement typed IDs, namespaced keys, Timestamp, and Duration | Done | me@jeickmeier.com | PR-006-T-tracking-9321fa243afb | `pr-006-kernel-value-types` | PR-006-A01; PR-006-A03; PR-006-A05 | — | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-006-T-json-digest-f467e33a8043 | PR-006 | Implement bounded RawJson, Metadata, and digest foundations | Done | me@jeickmeier.com | PR-006-T-ids-time-916b8ba52395 | `pr-006-kernel-value-types` | PR-006-A02; PR-006-A05 | — | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-006-T-errors-gen-640ef1351a14 | PR-006 | Implement stable errors and runtime-owned UUIDv7 generation | Done | me@jeickmeier.com | PR-006-T-ids-time-916b8ba52395 | `pr-006-kernel-value-types` | PR-006-A01; PR-006-A04; PR-006-A05 | — | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-006-T-fixtures-0ec39ba3c35b | PR-006 | Activate public-rust-api fixtures and kernel fixture runner | Done | me@jeickmeier.com | PR-006-T-json-digest-f467e33a8043; PR-006-T-errors-gen-640ef1351a14 | `pr-006-kernel-value-types` | PR-006-A01–A04 | — | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-006-T-evidence-4e5cd5fd4d5e | PR-006 | Run validation, TM-16 review, and bind PR-006 evidence | Done | me@jeickmeier.com | PR-006-T-fixtures-0ec39ba3c35b | [#4](https://github.com/jeickmeier/finstack-ai/pull/4) @ `56d7777956df145213b03d2b0b5c1922db42b346` | PR-006-A01–A05 | PR-006-E-ci-5565fde35cf6; PR-006-E-security-c463d4eef6a2; PR-006-E-hosted-ci-9a1ccbb88ae5; PR-006-E-test-kernel-bc373cf935e4 | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-007-T-contract-3b7faf1d03c9 | PR-007 | Freeze PR-007 message contract via pack v0.13 amendment | Done | me@jeickmeier.com | — | `pr-007-content-messages` | PR-007-A01–A04 | PLAN-0.11 baseline | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-007-T-modelref-optionals-a8c1e2d4f6b0 | PR-007 | Extend `ModelRef` with optional thinking/context/fast (pack v0.14) | Done | me@jeickmeier.com | PR-007-T-contract-3b7faf1d03c9 | `pr-007-content-messages` | PR-007-A01; PR-007-A04 | PLAN-0.12 baseline | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-007-T-review-hardening-c4e8a31d7b52 | PR-007 | Enforce strict bounded message decoding and review hardening | Done | me@jeickmeier.com | PR-007-T-modelref-optionals-a8c1e2d4f6b0; PR-007-T-content-2213b78a277b | `pr-007-content-messages` | PR-007-A01–A04 | Local validation complete; immutable evidence pending | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-007-T-content-2213b78a277b | PR-007 | Implement BlobRef, content blocks, and message validation | Done | me@jeickmeier.com | PR-007-T-contract-3b7faf1d03c9 | `pr-007-content-messages` | PR-007-A02; PR-007-A03; PR-007-A04 | PR-007-E-test-kernel-63a235179fb7 | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-007-T-fixtures-4207b289689c | PR-007 | Extend public-rust-api fixtures and runner for message subjects | Done | me@jeickmeier.com | PR-007-T-content-2213b78a277b | `pr-007-content-messages` | PR-007-A01–A04 | PR-007-E-conformance-f0694bffee9a | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-007-T-wasm-5c926d2baef6 | PR-007 | Prove A01 via kernel wasm32 check and deterministic fixtures | Done | me@jeickmeier.com | PR-007-T-fixtures-4207b289689c | `pr-007-content-messages` | PR-007-A01 | PR-007-E-check-wasm-c5b673cf41ce | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-007-T-evidence-5a722ac8cbbf | PR-007 | Run validation, TM-16/TM-20 review, and bind PR-007 evidence | Done | me@jeickmeier.com | PR-007-T-review-hardening-c4e8a31d7b52; PR-007-T-wasm-5c926d2baef6 | `pr-007-content-messages-final` @ `4a0a77f668adcfa2679dc161779c4456d721c0c2` | PR-007-A01–A04 | PR-007-E-ci-a29042aa7d6a; PR-007-E-security-1146ce970799 | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-008-T-contract-70f7f6cb4af1 | PR-008 | Freeze PR-008 record/event/effect contract via pack v0.15 amendment | Done | me@jeickmeier.com | — | [#5](https://github.com/jeickmeier/finstack-ai/pull/5) @ `158c0996d0ff559b39ee5cc7b6d1c8b2144aa79e` | PR-008-A01–A08 | PLAN-0.13 / pack v0.15 amendment | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-008-T-tracking-eccee73cba49 | PR-008 | Open Gate 2 ledger tracking for PR-008 implementation | Done | me@jeickmeier.com | PR-008-T-contract-70f7f6cb4af1 | `pr-008-contract-amendment` | PR-008-A01–A08 | — | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-008-T-shared-c5acb0c5e743 | PR-008 | Implement shared refs, Usage, limits, lineage, RunAccepted | Done | me@jeickmeier.com | PR-008-T-tracking-eccee73cba49 | `pr-008-contract-amendment` | PR-008-A05; PR-008-A06; PR-008-A08 | Local kernel/unit + public-api fixtures | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-008-T-effects-f37e7ffd18ef | PR-008 | Implement effect and interaction envelopes with digests | Done | me@jeickmeier.com | PR-008-T-shared-c5acb0c5e743 | `pr-008-contract-amendment` | PR-008-A02; PR-008-A05 | Local kernel/unit + effect_requested fixtures | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-008-T-records-2ab5e2eb2629 | PR-008 | Implement RecordDraft/envelope and owned RecordBody variants | Done | me@jeickmeier.com | PR-008-T-effects-f37e7ffd18ef | `pr-008-contract-amendment` | PR-008-A03; PR-008-A04; PR-008-A07; PR-008-A08 | Local kernel/unit + record_draft/append_request fixtures | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-008-T-events-ea7ecc0516ac | PR-008 | Implement class-safe RunEvent constructors and ordinal mapping | Done | me@jeickmeier.com | PR-008-T-records-2ab5e2eb2629 | `pr-008-contract-amendment` | PR-008-A01; PR-008-A03; PR-008-A04 | Local kernel/unit + run_event fixtures | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-008-T-fixtures-77d3a205a578 | PR-008 | Activate public-api fixtures for PR-008 subjects | Done | me@jeickmeier.com | PR-008-T-events-ea7ecc0516ac | `pr-008-contract-amendment` | PR-008-A01–A08 | `public_rust_api_corpus_passes` (54 fixtures) | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-008-T-evidence-e2d79f1ab462 | PR-008 | Run validation, TM review, and bind PR-008 evidence | Done | me@jeickmeier.com | PR-008-T-fixtures-77d3a205a578 | [#5](https://github.com/jeickmeier/finstack-ai/pull/5) @ `7b5ededc2054b0b7dde8373efbe0ec95ba02b71b` | PR-008-A01–A08 | PR-008-E-ci-3c2b7922d2e0; PR-008-E-security-ee2bc5d1331d | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-008-T-review-remediation-d329cb905b78 | PR-008 | Remediate pre-merge correctness/security review findings and rebind evidence | Done | me@jeickmeier.com | PR-008-T-evidence-e2d79f1ab462 | `pr-008-contract-amendment` @ `cd1fa367b528225e5bba1484ecc377cc109d666c` | PR-008-A01–A08 | PR-008-E-remediation-test-a8ae4dbcdf37; PR-008-E-remediation-security-c7141f3b3c7f; PR-008-E-remediation-ci-3d74148b759a | 2026-08-08 | 2026-08-08 | 2026-08-08 |
| PR-009-T-contract-06d062b58555 | PR-009 | Freeze the model-only reducer contract and open truthful tracking | Done | me@jeickmeier.com | — | `main` @ `5843dce6d77498a75acdc15d816586cb26098456` | PR-009-A01–A05 | PLAN-0.14 / pack v0.16 amendment | 2026-08-08 | 2026-08-08 | 2026-08-09 |
| PR-009-T-red-tests-260ed0b05cde | PR-009 | Establish and review RED reducer contract tests | Done | me@jeickmeier.com | PR-009-T-contract-06d062b58555 | `main` @ `5843dce6d77498a75acdc15d816586cb26098456` | PR-009-A01–A05 | PR-009-E-merge-kernel-f294c67a385b | 2026-08-08 | 2026-08-08 | 2026-08-09 |
| PR-009-T-reducer-179a372a0fec | PR-009 | Implement and review the model-only reducer | Done | me@jeickmeier.com | PR-009-T-red-tests-260ed0b05cde | `main` @ `5843dce6d77498a75acdc15d816586cb26098456` | PR-009-A01–A05 | PR-009-E-merge-kernel-f294c67a385b; PR-009-E-security-d072a4581639 | 2026-08-08 | 2026-08-08 | 2026-08-09 |
| PR-009-T-conformance-402dbb87c868 | PR-009 | Activate reducer conformance, fixtures, and benchmark | Done | me@jeickmeier.com | PR-009-T-reducer-179a372a0fec | `main` @ `5843dce6d77498a75acdc15d816586cb26098456` | PR-009-A01–A05 | PR-009-E-merge-conformance-03a5d78b496c; PR-009-E-benchmark-e183b569274a | 2026-08-08 | 2026-08-08 | 2026-08-09 |
| PR-009-T-validation-b78f8e74489b | PR-009 | Run fresh local validation and security review | Done | me@jeickmeier.com | PR-009-T-conformance-402dbb87c868 | `main` @ `5843dce6d77498a75acdc15d816586cb26098456` | PR-009-A01–A05 | PR-009-E-ci-cf6193470528; PR-009-E-security-d072a4581639; PR-009-E-merge-kernel-f294c67a385b; PR-009-E-merge-conformance-03a5d78b496c; PR-009-E-merge-wasm-14b6e89c5a7d | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-010-T-contract-3fe553d17f48 | PR-010 | Freeze the tool-batch reducer contract through change control | Done | me@jeickmeier.com | — | `main` @ `ff2e6e7b80e34061dae4dcc5ceb4b259a34a89b5` | PR-010-A01–A04 | PLAN-0.15 / pack v0.17 amendment | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-010-T-reducer-6bfe5d98c4bf | PR-010 | Implement tool planning, settlement, replay, and state-hash semantics | Done | me@jeickmeier.com | PR-010-T-contract-3fe553d17f48 | `main` @ `ff2e6e7b80e34061dae4dcc5ceb4b259a34a89b5` | PR-010-A01–A04 | PR-010-E-merge-kernel-d8f6a2c9017b; PR-010-E-security-9ab3d6e1f470 | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-010-T-events-a6000041343b | PR-010 | Implement tool records, source-ordered messages, and event correlations | Done | me@jeickmeier.com | PR-010-T-contract-3fe553d17f48 | `main` @ `ff2e6e7b80e34061dae4dcc5ceb4b259a34a89b5` | PR-010-A01–A03 | PR-010-E-merge-kernel-d8f6a2c9017b; PR-010-E-merge-conformance-71e4c3a8b205 | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-010-T-conformance-ac5b876dbe70 | PR-010 | Add tool golden traces, compatibility fixtures, and replay coverage | Done | me@jeickmeier.com | PR-010-T-reducer-6bfe5d98c4bf; PR-010-T-events-a6000041343b | `main` @ `ff2e6e7b80e34061dae4dcc5ceb4b259a34a89b5` | PR-010-A01–A04 | PR-010-E-merge-conformance-71e4c3a8b205; PR-010-E-merge-kernel-d8f6a2c9017b | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-010-T-validation-31ec4c3d284c | PR-010 | Run validation, TM-02 review, and bind immutable evidence | Done | me@jeickmeier.com | PR-010-T-conformance-ac5b876dbe70 | `main` @ `ff2e6e7b80e34061dae4dcc5ceb4b259a34a89b5` | PR-010-A01–A04 | PR-010-E-ci-f2c84d1a6b39; PR-010-E-security-9ab3d6e1f470; PR-010-E-merge-kernel-d8f6a2c9017b; PR-010-E-merge-conformance-71e4c3a8b205; PR-010-E-merge-wasm-5c0e7f4b92ad | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-011-T-contract-c7a542044eb0 | PR-011 | Freeze limit, retry, cancellation, lineage, and state-v3 contracts through change control | Done | me@jeickmeier.com | — | `main` @ `01380ead5c5ca7b9e7d28d681d84719c9bf0279e` | PR-011-A01–A06 | Pack v0.18 / TDD v0.16 / Plan v0.16 / Threat Model v0.6 | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-011-T-surfaces-53598422aac4 | PR-011 | Implement control DTOs, records, events, and strict state-v3 projection | Done | me@jeickmeier.com | PR-011-T-contract-c7a542044eb0 | `main` @ `01380ead5c5ca7b9e7d28d681d84719c9bf0279e` | PR-011-A01–A04 | PR-011-E-kernel-c8a314829b86; PR-011-E-conformance-4aae0a67b162 | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-011-T-reducer-6179c0e00268 | PR-011 | Implement limit, retry, deadline, cancellation, reconciliation, and race semantics | Done | me@jeickmeier.com | PR-011-T-surfaces-53598422aac4 | `main` @ `01380ead5c5ca7b9e7d28d681d84719c9bf0279e` | PR-011-A01–A06 | PR-011-E-kernel-c8a314829b86; PR-011-E-security-c338d5a3a19c | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-011-T-conformance-8fb73c4f0c9e | PR-011 | Add compatibility, golden, property, replay, benchmark, and security fixtures | Done | me@jeickmeier.com | PR-011-T-reducer-6179c0e00268 | `main` @ `01380ead5c5ca7b9e7d28d681d84719c9bf0279e` | PR-011-A01–A06 | PR-011-E-conformance-4aae0a67b162; PR-011-E-benchmark-8bf15225d8e7 | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-011-T-validation-6e34c551532b | PR-011 | Run complete validation, security review, and bind immutable evidence | Done | me@jeickmeier.com | PR-011-T-conformance-8fb73c4f0c9e | `main` @ `01380ead5c5ca7b9e7d28d681d84719c9bf0279e` | PR-011-A01–A06 | PR-011-E-ci-d10ca2e00eba; PR-011-E-wasm-64c7a443c3e6; PR-011-E-security-c338d5a3a19c | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-012-T-contract-9ba2cd89fe88 | PR-012 | Freeze structured-output, internal-control, activation, and state-v4 contracts through change control | Done | me@jeickmeier.com | — | `main` @ `dc58a11fbc871e70326d04fd9840297b5179023f` | PR-012-A01–A05 | Pack v0.19 / TDD v0.17 / Plan v0.17 | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-012-T-surfaces-c92d829317ce | PR-012 | Implement bounded output, validation, capability, record, and strict state-v4 public surfaces | Done | me@jeickmeier.com | PR-012-T-contract-9ba2cd89fe88 | `main` @ `dc58a11fbc871e70326d04fd9840297b5179023f` | PR-012-A01–A05 | PR-012-E-kernel-29bab69145af; PR-012-E-conformance-9772f14d868c | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-012-T-reducer-39c52583a6b0 | PR-012 | Implement validation, retry/exhaustion, internal-tool, result/tool competition, and capability-plan replay semantics | Done | me@jeickmeier.com | PR-012-T-surfaces-c92d829317ce | `main` @ `dc58a11fbc871e70326d04fd9840297b5179023f` | PR-012-A01–A05 | PR-012-E-kernel-29bab69145af; PR-012-E-security-829f512af935 | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-012-T-conformance-5e24f0e38931 | PR-012 | Add compatibility, golden, replay, boundary, and security fixtures | Done | me@jeickmeier.com | PR-012-T-reducer-39c52583a6b0 | `main` @ `dc58a11fbc871e70326d04fd9840297b5179023f` | PR-012-A01–A05 | PR-012-E-conformance-9772f14d868c; PR-012-E-security-829f512af935 | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-012-T-validation-473ab48b0e02 | PR-012 | Run complete validation, review terminal/security semantics, and bind immutable evidence | Done | me@jeickmeier.com | PR-012-T-conformance-5e24f0e38931 | `main` @ `dc58a11fbc871e70326d04fd9840297b5179023f` | PR-012-A01–A05 | PR-012-E-kernel-29bab69145af; PR-012-E-conformance-9772f14d868c; PR-012-E-ci-c9bbb3caa845; PR-012-E-security-829f512af935 | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-013-T-properties-a6f93d2180cb | PR-013 | Add exhaustive transition, property, model-based, lineage, deferral, interaction, and finalize tests | Done | me@jeickmeier.com | — | `codex/pr-013-kernel-hardening` @ `aba26764a448f6b2691bfac62a0f36f865f61ed6` | PR-013-A01–A02 | PR-013-E-kernel-3b7e91c5a2d4 | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-013-T-fixtures-14c7e52a8bd1 | PR-013 | Add malformed, boundary, version, and corrupt-replay candidate-v1 fixtures | Done | me@jeickmeier.com | PR-013-T-properties-a6f93d2180cb | `codex/pr-013-kernel-hardening` @ `aba26764a448f6b2691bfac62a0f36f865f61ed6` | PR-013-A01–A02 | PR-013-E-conformance-8c1d6f4a9b27 | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-013-T-fuzz-72eac06f5d39 | PR-013 | Add isolated fuzz targets, corpora, pinned tasks, and CI cadence | Done | me@jeickmeier.com | PR-013-T-fixtures-14c7e52a8bd1 | `codex/pr-013-kernel-hardening` @ `aba26764a448f6b2691bfac62a0f36f865f61ed6` | PR-013-A02–A03 | PR-013-E-fuzz-smoke-5e9a2c7d1f63; PR-013-E-nightly-4e8c2a7d5b19 | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-013-T-reference-b4d01c89ea67 | PR-013 | Publish the candidate-v1 kernel semantic reference and drift guard | Done | me@jeickmeier.com | PR-013-T-properties-a6f93d2180cb; PR-013-T-fixtures-14c7e52a8bd1 | `codex/pr-013-kernel-hardening` @ `aba26764a448f6b2691bfac62a0f36f865f61ed6` | PR-013-A01–A02 | PR-013-E-golden-7f2c19a4d8e6 | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-013-T-gate-9f35ab70c2d4 | PR-013 | Run immutable validation, complete security review, and record Phase 1/G1 evidence | Done | me@jeickmeier.com | PR-013-T-fuzz-72eac06f5d39; PR-013-T-reference-b4d01c89ea67 | [#6](https://github.com/jeickmeier/finstack-ai/pull/6) merged @ `fa6222f20e4a4616f600e867be94afe12967dcb9` | PR-013-A01–A04 | PR-013-E-ci-1a6d9c3e7b25; PR-013-E-security-7d2a5f9c1e84; PR-013-E-hosted-ci-31339759492; PR-013-E-long-fuzz-31339772213; PH1-E-exit-kernel-fa6222f20e4a; G1-D-kernel-semantics-4f52c8a91d6e | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-014-T-contracts-0d7a3e9c5b21 | PR-014 | Implement external-command kernel contracts and candidate-v1 fixtures | Done | me@jeickmeier.com | — | `codex/pr-014-runtime-commit-loop` @ `dfe42cf` | PR-014-A01; PR-014-A04; PR-014-A07 | PR-014-E-contracts-fae82274d718 | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-014-T-store-4c8e1a6d9b32 | PR-014 | Implement JournalStore and bounded non-durable memory store | Done | me@jeickmeier.com | PR-014-T-contracts-0d7a3e9c5b21 | `codex/pr-014-runtime-commit-loop` @ `25f921d` | PR-014-A01–A03; PR-014-A06 | PR-014-E-store-0f061d363c0a | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-014-T-loop-7b2d5f0a8c41 | PR-014 | Implement commit coordinator, conflict recovery, task ownership, and dispatch ordering | Done | me@jeickmeier.com | PR-014-T-store-4c8e1a6d9b32 | `codex/pr-014-runtime-commit-loop` @ `450a73b` | PR-014-A01–A05 | PR-014-E-runtime-f7d0f0ecede0; PR-014-E-wasm-1de6c1fbd157 | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-014-T-ingress-9e3a6c1d4b52 | PR-014 | Implement external routers and security audit fail-closed behavior | Done | me@jeickmeier.com | PR-014-T-loop-7b2d5f0a8c41 | `codex/pr-014-runtime-commit-loop` @ `ff9178f` | PR-014-A04; PR-014-A05; PR-014-A07 | PR-014-E-runtime-f7d0f0ecede0; PR-014-E-security-a8a835efcf19 | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-014-T-validation-2f6b8d0c5a73 | PR-014 | Run affected validation, complete security review, and bind candidate evidence | Done | me@jeickmeier.com | PR-014-T-ingress-9e3a6c1d4b52 | `codex/pr-014-runtime-commit-loop` @ `ff9178f2244c8d8fd0c62cbba18b2b4dfbee2a29` | PR-014-A01–A07 | PR-014-E-ci-cef693ae9837; PR-014-E-security-a8a835efcf19 | 2026-08-09 | 2026-08-09 | 2026-08-09 |
| PR-015-T-contract-41c8e7a2d50b | PR-015 | Freeze the provider-neutral Model ABI and cross-PR ownership through change control | Done | me@jeickmeier.com | — | `main` @ `16f3a865d122aac34d1df5b3dc1d7a0b44c4aa82` | PR-015-A01–A04 | Pack v0.20 / TDD v0.18 / Plan v0.18 at immutable implementation commit `f3a9ffb891a7dd7d5ed0afe2971d35aa266d3c3a` | 2026-08-10 | 2026-08-10 | 2026-08-10 |
| PR-015-T-runtime-7e2a9c14b6d3 | PR-015 | Implement profile locking, bounded stream assembly, and the boxed runtime Model port | Done | me@jeickmeier.com | PR-015-T-contract-41c8e7a2d50b | `main` @ `16f3a865d122aac34d1df5b3dc1d7a0b44c4aa82` | PR-015-A01; PR-015-A03; PR-015-A04 | PR-015-E-model-4e91b7c2d6a8; PR-015-E-security-2d8a6f1c4b79 | 2026-08-10 | 2026-08-10 | 2026-08-10 |
| PR-015-T-scripted-b3d6f108ca52 | PR-015 | Make ScriptedModel a deterministic leaf Model with malformed and blocking controls | Done | me@jeickmeier.com | PR-015-T-runtime-7e2a9c14b6d3 | `main` @ `16f3a865d122aac34d1df5b3dc1d7a0b44c4aa82` | PR-015-A01–A03 | PR-015-E-model-4e91b7c2d6a8; PR-015-E-compat-9c3f2a7e5b10 | 2026-08-10 | 2026-08-10 | 2026-08-10 |
| PR-015-T-acceptance-c9a4e21f7d60 | PR-015 | Add chunk-invariance, lifecycle, cancellation, and native/WASM leaf-contract proofs | Done | me@jeickmeier.com | PR-015-T-scripted-b3d6f108ca52 | `main` @ `16f3a865d122aac34d1df5b3dc1d7a0b44c4aa82` | PR-015-A01–A04 | PR-015-E-model-4e91b7c2d6a8; PR-015-E-compat-9c3f2a7e5b10 | 2026-08-10 | 2026-08-10 | 2026-08-10 |
| PR-015-T-validation-5f1c8a3de742 | PR-015 | Run affected validation, complete the mandatory security review, and bind candidate evidence | Done | me@jeickmeier.com | PR-015-T-acceptance-c9a4e21f7d60 | `main` @ `16f3a865d122aac34d1df5b3dc1d7a0b44c4aa82` | PR-015-A01–A04 | PR-015-E-ci-7b5e1d9a3c62; PR-015-E-security-2d8a6f1c4b79; PR-015-E-integration-a6c2e8f14d53 | 2026-08-10 | 2026-08-10 | 2026-08-10 |
| PR-016-T-contract-9d1e4a7c2b53 | PR-016 | Implement the target-neutral Toolset ABI, compiled schema catalog, and fail-closed policy planner | Done | me@jeickmeier.com | — | `codex/pr-016-toolset-scheduler` @ `aab7b81b8cb36eca82860ffa38ded7ba82a14247` | PR-016-A04–A05 | PR-016-E-tool-8f2c4d7a1b63; PR-016-E-compat-3e9a6c1d5b72 | 2026-08-10 | 2026-08-10 | — |
| PR-016-T-runtime-6b3f8c1d5a72 | PR-016 | Integrate post-commit tool routing, bounded execution, stream normalization, and panic containment | Done | me@jeickmeier.com | PR-016-T-contract-9d1e4a7c2b53 | `codex/pr-016-toolset-scheduler` @ `aab7b81b8cb36eca82860ffa38ded7ba82a14247` | PR-016-A01–A03 | PR-016-E-tool-8f2c4d7a1b63; PR-016-E-release-2a6f8d4c1e95; PR-016-E-ci-5d9b2e7a4c16 | 2026-08-10 | 2026-08-10 | — |
| PR-016-T-scripted-a7c2e5d9b164 | PR-016 | Add deterministic ScriptedToolset controls for concurrency, cancellation, malformed streams, and panic fixtures | Done | me@jeickmeier.com | PR-016-T-contract-9d1e4a7c2b53 | `codex/pr-016-toolset-scheduler` @ `aab7b81b8cb36eca82860ffa38ded7ba82a14247` | PR-016-A01–A05 | PR-016-E-tool-8f2c4d7a1b63 | 2026-08-10 | 2026-08-10 | — |
| PR-016-T-acceptance-4e8b1d6f3c95 | PR-016 | Prove scheduler limits, panic isolation, durable source ordering, approval floors, and validator parity | Done | me@jeickmeier.com | PR-016-T-runtime-6b3f8c1d5a72; PR-016-T-scripted-a7c2e5d9b164 | `codex/pr-016-toolset-scheduler` @ `aab7b81b8cb36eca82860ffa38ded7ba82a14247` | PR-016-A01–A05 | PR-016-E-tool-8f2c4d7a1b63; PR-016-E-compat-3e9a6c1d5b72; PR-016-E-security-7c1e5a9d3b84; PR-016-E-release-2a6f8d4c1e95 | 2026-08-10 | 2026-08-10 | — |
| PR-016-T-validation-c5a9f2e7d318 | PR-016 | Run affected validation, release/security review, and bind immutable candidate evidence | Done | me@jeickmeier.com | PR-016-T-acceptance-4e8b1d6f3c95 | `codex/pr-016-toolset-scheduler` @ `aab7b81b8cb36eca82860ffa38ded7ba82a14247` | PR-016-A01–A05 | PR-016-E-security-7c1e5a9d3b84; PR-016-E-release-2a6f8d4c1e95; PR-016-E-ci-5d9b2e7a4c16 | 2026-08-10 | 2026-08-10 | Hosted and independent review not claimed; local integration tracked at logical-PR level |
| PR-017-T-tracking-2f8a1c7d4e90 | PR-017 | Open bounded event-hub delivery tracking and confirm PR-014–PR-016 dependencies | Done | me@jeickmeier.com | — | `codex/pr-017-event-hub` @ `1bfc653c36312187f34c6ad7ba787a360145612d` | PR-017-A01–A04 | Local pre-change `mise run test-runtime` baseline passed | 2026-08-10 | 2026-08-10 | — |
| PR-017-T-hub-7b3d9e2a5c61 | PR-017 | Implement bounded event source, subscriptions, batching, filters, lag policies, and shutdown | Done | me@jeickmeier.com | PR-017-T-tracking-2f8a1c7d4e90 | `codex/pr-017-event-hub` @ `1bfc653c36312187f34c6ad7ba787a360145612d` | PR-017-A01–A04 | PR-017-E-events-a3f7c9e1d5b2; PR-017-E-security-9b5e1c7a3d64 | 2026-08-10 | 2026-08-10 | — |
| PR-017-T-stream-4c6e1a8f3d72 | PR-017 | Publish committed events before dispatch and stream validated model/tool progress incrementally | Done | me@jeickmeier.com | PR-017-T-hub-7b3d9e2a5c61 | `codex/pr-017-event-hub` @ `1bfc653c36312187f34c6ad7ba787a360145612d` | PR-017-A02; PR-017-A04 | PR-017-E-events-a3f7c9e1d5b2; PR-017-E-compat-6d2a8f4c1e93 | 2026-08-10 | 2026-08-10 | — |
| PR-017-T-acceptance-8d2f5b9c1a43 | PR-017 | Prove queue bounds, batching order, slow-observer isolation, and reported progress drops | Done | me@jeickmeier.com | PR-017-T-stream-4c6e1a8f3d72 | `codex/pr-017-event-hub` @ `1bfc653c36312187f34c6ad7ba787a360145612d` | PR-017-A01–A04 | PR-017-E-events-a3f7c9e1d5b2; PR-017-E-security-9b5e1c7a3d64 | 2026-08-10 | 2026-08-10 | — |
| PR-017-T-validation-5e7a3c6d9b21 | PR-017 | Run affected validation, complete TM-17 review, and bind candidate evidence | Done | me@jeickmeier.com | PR-017-T-acceptance-8d2f5b9c1a43 | `main` @ `966f047fd28972d35c835ddf0441f8ba67348b25` | PR-017-A01–A04 | PR-017-E-compat-6d2a8f4c1e93; PR-017-E-security-9b5e1c7a3d64; PR-017-E-ci-4f8a2d6c9b15; PR-017-E-integration-966f047fd289 | 2026-08-10 | 2026-08-10 | Hosted and independent review not claimed |
| PR-018-T-tracking-0a7d4c9e2f61 | PR-018 | Record the execution envelope, confirm PR-014/PR-017 dependencies, and freeze the PR-018 contract slice | Done | me@jeickmeier.com | — | `codex/pr-018-context-middleware` @ baseline `3e699c37865b0f9c97b3f956fe3314bb6a734c0b` | PR-018-A01–A08 | Plan v0.18 / SHA-256 `555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4` | 2026-08-10 | 2026-08-10 | External actions remain unauthorized |
| PR-018-T-ports-4d2a8f6c1b73 | PR-018 | Implement ContextProvider, Middleware, and immutable Observer public contracts with bounded normalized values | Done | me@jeickmeier.com | PR-018-T-tracking-0a7d4c9e2f61 | `codex/pr-018-context-middleware` @ `aac573b593dd3950a4143672aeadc3765fa88a29` | PR-018-A01; PR-018-A03–A05 | PR-018-E-extensions-8c2a6f4d1b73; PR-018-E-compat-5e9b3d7a2c16 | 2026-08-10 | 2026-08-10 | — |
| PR-018-T-chain-7e3b1c5a9d24 | PR-018 | Implement deterministic context assembly, middleware ordering/outcome validation, compaction integrity, and committed-cursor recovery | Done | me@jeickmeier.com | PR-018-T-ports-4d2a8f6c1b73 | `codex/pr-018-context-middleware` @ `aac573b593dd3950a4143672aeadc3765fa88a29` | PR-018-A02; PR-018-A04–A08 | PR-018-E-extensions-8c2a6f4d1b73; PR-018-E-security-7a1d4f8c3b62 | 2026-08-10 | 2026-08-10 | — |
| PR-018-T-acceptance-2c8e5a1d7b46 | PR-018 | Add public fixture, budget, interaction, observer, compaction, and crash-prefix acceptance proofs | Done | me@jeickmeier.com | PR-018-T-chain-7e3b1c5a9d24 | `codex/pr-018-context-middleware` @ `aac573b593dd3950a4143672aeadc3765fa88a29` | PR-018-A01–A08 | PR-018-E-extensions-8c2a6f4d1b73; PR-018-E-compat-5e9b3d7a2c16; PR-018-E-security-7a1d4f8c3b62 | 2026-08-10 | 2026-08-10 | — |
| PR-018-T-validation-9b4f2d6a8c15 | PR-018 | Run affected validation, complete TM-01/TM-02/TM-17/TM-21 review, and bind immutable evidence | Done | me@jeickmeier.com | PR-018-T-acceptance-2c8e5a1d7b46 | `main` @ `3fe0314c6434211e1c8f493f24401888f9609050` | PR-018-A01–A08 | PR-018-E-extensions-8c2a6f4d1b73; PR-018-E-compat-5e9b3d7a2c16; PR-018-E-security-7a1d4f8c3b62; PR-018-E-ci-2f6c9a5e1d84; PR-018-E-integration-3fe0314c6434 | 2026-08-10 | 2026-08-10 | Hosted and independent review not claimed |
| PR-019-T-tracking-3c7a1e5d9b42 | PR-019 | Confirm dependencies, freeze cancellation/deadline/retry scope, and establish the runtime baseline | Done | me@jeickmeier.com | — | `codex/pr-019-cancellation-timers` @ baseline `3d9f671d9e046a5df67a87ac71c3a0997ac13cd6` | PR-019-A01–A04 | Pre-change `mise run test-runtime` passed | 2026-08-10 | 2026-08-10 | External actions remain unauthorized |
| PR-019-T-cancellation-8e2b6d1a4c73 | PR-019 | Implement hierarchical run/model/tool cancellation and bounded shutdown diagnostics | Done | me@jeickmeier.com | PR-019-T-tracking-3c7a1e5d9b42 | `codex/pr-019-cancellation-timers` | PR-019-A01; PR-019-A04 | `mise run test-lifecycle`; cancellation reconciliation and graceful/forced shutdown fixtures passed | 2026-08-10 | 2026-08-10 | — |
| PR-019-T-time-5a9d3c7e1b64 | PR-019 | Implement monotonic deadline waits, persisted-wall restart conversion, timer firing, and retry resumption | Done | me@jeickmeier.com | PR-019-T-cancellation-8e2b6d1a4c73 | `codex/pr-019-cancellation-timers` | PR-019-A02–A03 | `mise run test-lifecycle`; monotonic, overdue, backward-clamp, pre-dispatch deadline, and restart fixtures passed | 2026-08-10 | 2026-08-10 | — |
| PR-019-T-acceptance-1d6a8c4f2b95 | PR-019 | Add scripted cancellation-boundary, wall-jump, restart-attempt, and task-leak proofs | Done | me@jeickmeier.com | PR-019-T-time-5a9d3c7e1b64 | `codex/pr-019-cancellation-timers` | PR-019-A01–A04 | `mise run test-runtime`; `mise run test-lifecycle`; `mise run check-minimal`; `mise run check-wasm` passed | 2026-08-10 | 2026-08-10 | — |
| PR-019-T-validation-7b3e9a2d5c16 | PR-019 | Run affected validation, complete lifecycle/security review, and bind immutable evidence | Done | me@jeickmeier.com | PR-019-T-acceptance-1d6a8c4f2b95 | `codex/pr-019-cancellation-timers` @ `941403749648d2ec3e6e85a8af1a0b73afd80866` | PR-019-A01–A04 | PR-019-E-lifecycle-6a2d8f4c1b73; PR-019-E-compat-9e3b7d1a5c82; PR-019-E-security-4c8e1a7d3b65; PR-019-E-ci-2f7a9c5e1d84 | 2026-08-10 | 2026-08-10 | Hosted and independent review not claimed |
| PR-020-T-tracking-4d8a2c6f1b73 | PR-020 | Confirm Phase 2 dependencies, gate/ADR obligations, exclusions, and native-runtime baseline | Done | me@jeickmeier.com | — | `codex/pr-020-native-runtime-gate` @ baseline `a39fd402f4e089cecf34074cfd57498de1e58e0b` | PR-020-A01–A04 | PR-014–PR-019 are locally integrated; G2 remains Not ready until this PR and Phase 2 exit evidence pass | 2026-08-10 | 2026-08-10 | External actions remain unauthorized |
| PR-020-T-faults-8b3e7d1a5c94 | PR-020 | Implement deterministic manual drive and complete commit/effect crash-prefix and race matrices | Done | me@jeickmeier.com | PR-020-T-tracking-4d8a2c6f1b73 | `codex/pr-020-native-runtime-gate` | PR-020-A01 | `mise run test-runtime-gate`; committed-prefix recovery and failed-append/no-dispatch proofs passed | 2026-08-10 | 2026-08-11 | — |
| PR-020-T-stress-2f6c9a4e7b15 | PR-020 | Add bounded runtime stress, leak/channel cleanup, and feasible Miri-compatible coverage | Done | me@jeickmeier.com | PR-020-T-faults-8b3e7d1a5c94 | `codex/pr-020-native-runtime-gate` | PR-020-A02–A03 | `mise run test-runtime-gate`; `mise run test-miri`; 128 idle-owner and 24 model-run stress proofs passed | 2026-08-10 | 2026-08-11 | — |
| PR-020-T-benchmarks-7a1d5e9c3b62 | PR-020 | Document runtime ownership/failure semantics and publish reducer/stream/tool/idle-memory benchmark baselines | Done | me@jeickmeier.com | PR-020-T-stress-2f6c9a4e7b15 | `codex/pr-020-native-runtime-gate` | PR-020-A03 | `mise run benchmark-smoke`; [`runtime-execution.md`](runtime-execution.md); host-specific reducer/model/tool/idle-memory baselines staged | 2026-08-10 | 2026-08-11 | — |
| PR-020-T-validation-5c8e2a7d4b93 | PR-020 | Run complete Phase 2 validation/security/ADR review, bind immutable evidence, and record the delegated G2 decision if all criteria pass | Done | me@jeickmeier.com | PR-020-T-benchmarks-7a1d5e9c3b62 | local `main` merge `f101b260dd6c9ca6ffa82d805824669c5d974ca0` | PR-020-A01–A04 | PR-020-E-integration-9a4c2e7d5b81; PH2-E-exit-e2e-3d7a1c9e5b62; PH2-E-exit-faults-8e2c6a4d1b75; PH2-E-exit-bounds-5b9d3e7a2c61; PH2-E-exit-lifecycle-1c6f8a4d9b32; G2-D-native-runtime-7c2e9a4d1b65 | 2026-08-10 | 2026-08-11 | Local integration, Phase 2 exit review, and G2 passed; hosted and independent review not claimed |
| PR-021-T-tracking-6a2d8f4c1b73 | PR-021 | Confirm G2, Phase 3 entrances, PR-021 ownership, ADR triggers, TM-06, exclusions, and clean integration baseline | Done | me@jeickmeier.com | — | `codex/pr-021-rust-sdk` @ baseline `6d69aa720b426ebb9afe3524743a6bb464737516` | PR-021-A01–A05 | G2-D-native-runtime-7c2e9a4d1b65; PH3-E-entrance-g2-4d8a1c6e9b35; PH3-E-entrance-ports-6b2e9d4a7c15 | 2026-08-11 | 2026-08-11 | No ADR trigger; package discovery, dynamic loading, AgentSpec, capabilities, bundles, and public preview remain excluded |
| PR-021-T-registration-9e3b7d1a5c82 | PR-021 | Implement typed six-port registration, namespaced identities, aliases, duplicate/replacement rules, extension sources, and diagnostics | Done | me@jeickmeier.com | PR-021-T-tracking-6a2d8f4c1b73 | `codex/pr-021-rust-sdk` @ `d12abc5ec917be1d725cafdd7e9250cb23f9799f` | PR-021-A01–A02; PR-021-A05 | PR-021-E-sdk-8a4d2c6f1b73 | 2026-08-11 | 2026-08-11 | — |
| PR-021-T-factories-4c8e1a7d3b65 | PR-021 | Implement target-correct typed factories, construction cancellation/deadline context, ready-handle caching, warmup, and lifecycle ownership | Done | me@jeickmeier.com | PR-021-T-registration-9e3b7d1a5c82 | `codex/pr-021-rust-sdk` @ `d12abc5ec917be1d725cafdd7e9250cb23f9799f` | PR-021-A03–A04 | PR-021-E-sdk-8a4d2c6f1b73; PR-021-E-security-6c2f8a4d1b75 | 2026-08-11 | 2026-08-11 | — |
| PR-021-T-resolution-2f7a9c5e1d84 | PR-021 | Build immutable ResolvedAgent/ResolvedRunPlan direct-handle shapes with deterministic resolution and no hot-path registry retention | Done | me@jeickmeier.com | PR-021-T-factories-4c8e1a7d3b65 | `codex/pr-021-rust-sdk` @ `d12abc5ec917be1d725cafdd7e9250cb23f9799f` | PR-021-A02–A05 | PR-021-E-sdk-8a4d2c6f1b73; PR-021-E-compat-3e7b9d1a5c82 | 2026-08-11 | 2026-08-11 | — |
| PR-021-T-validation-7b3d1a8c5e92 | PR-021 | Run focused/native/minimal/WASM/architecture/compatibility/security validation and bind immutable local evidence | Done | me@jeickmeier.com | PR-021-T-resolution-2f7a9c5e1d84 | local `main` merge `ec56d175a8b31fe49d5ea672f89020bc3ec553cd` | PR-021-A01–A05 | PR-021-E-sdk-8a4d2c6f1b73; PR-021-E-compat-3e7b9d1a5c82; PR-021-E-security-6c2f8a4d1b75; PR-021-E-ci-9d3a7c1e5b84; PR-021-E-integration-ec56d175a8b3 | 2026-08-11 | 2026-08-11 | Local integration passed; hosted and independent review not claimed |
| PR-022-T-tracking-3a7d1c9e5b62 | PR-022 | Confirm PR-021 dependency, freeze A01–A12 ownership, record service/kernel boundaries, ADR triggers, and the clean baseline | Done | me@jeickmeier.com | — | `codex/pr-022-agent-spec` @ baseline `72a27005613a7fb109daf0919c114e19ca893419` | PR-022-A01–A12 | Plan v0.18 / SHA-256 `555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`; PR-022 plan artifact | 2026-08-11 | 2026-08-11 | No ADR trigger at admission; TM-04/TM-15/TM-16/TM-20 review required; external actions remain unauthorized |
| PR-022-T-spec-6c2f8a4d1b75 | PR-022 | Implement strict AgentSpec, CapabilitySpec, AgentBuilder, canonical fingerprinting, RunResult decoding, and compatibility vectors | Done | me@jeickmeier.com | PR-022-T-tracking-3a7d1c9e5b62 | `codex/pr-022-agent-spec` | PR-022-A01; PR-022-A03–A05; PR-022-A09 | Focused SDK tests pass | 2026-08-11 | 2026-08-11 | — |
| PR-022-T-services-9d3a7c1e5b84 | PR-022 | Implement target-correct AgentInvoker, BudgetLedger, and scoped ArtifactStore runtime service contracts | Done | me@jeickmeier.com | PR-022-T-spec-6c2f8a4d1b75 | `codex/pr-022-agent-spec` | PR-022-A07; PR-022-A10–A12 | Focused runtime tests pass | 2026-08-11 | 2026-08-11 | — |
| PR-022-T-durable-2f7a9c5e1d84 | PR-022 | Add durable child-preparation and budget record vocabulary, replay state, and retry-conflict invariants | Done | me@jeickmeier.com | PR-022-T-services-9d3a7c1e5b84 | `codex/pr-022-agent-spec` | PR-022-A07; PR-022-A11 | Kernel replay and semantic-reference tests pass | 2026-08-11 | 2026-08-11 | — |
| PR-022-T-resolution-7b3d1a8c5e92 | PR-022 | Implement finite bundle/catalog resolution, exact credential-free locks, capability activation, and required-service checks | Done | me@jeickmeier.com | PR-022-T-spec-6c2f8a4d1b75; PR-022-T-services-9d3a7c1e5b84 | `codex/pr-022-agent-spec` | PR-022-A02; PR-022-A06; PR-022-A08; PR-022-A10 | Bundle resolve/activate/lock reconstruction tests pass | 2026-08-11 | 2026-08-11 | — |
| PR-022-T-coordination-4e9b2d6a1c75 | PR-022 | Implement parent-session child preparation, budget settlement-before-start, artifact staging, recovery, and race proofs | Done | me@jeickmeier.com | PR-022-T-durable-2f7a9c5e1d84; PR-022-T-resolution-7b3d1a8c5e92 | `codex/pr-022-agent-spec` | PR-022-A07; PR-022-A11–A12 | Crash/reconcile, all-placement, charge/release, and artifact lifecycle tests pass | 2026-08-11 | 2026-08-11 | — |
| PR-022-T-validation-1f6a9c4e2b83 | PR-022 | Run focused/native/minimal/WASM/architecture/compatibility/security validation and bind immutable local evidence | Done | me@jeickmeier.com | PR-022-T-coordination-4e9b2d6a1c75 | local `main` merge `9f4385b588ba840690c0b82f1816ee8378b2740e` | PR-022-A01–A12 | PR-022-E-specs-a6c2e8f14d53; PR-022-E-composition-b4e7c1a9d365; PR-022-E-compat-c9f2a6d1e847; PR-022-E-security-d3a7e1c5b962; PR-022-E-ci-e8c4a1d7b356; PR-022-E-integration-f2d6a9c3e174 | 2026-08-11 | 2026-08-11 | Local integration passed; hosted and independent review not claimed |
| PR-023-T-tracking-5a8d2c7e1b64 | PR-023 | Confirm PR-021/PR-022 dependencies, freeze A01–A05, ADR-037/TM-10/TM-21 review, exclusions, and clean baseline | Done | me@jeickmeier.com | — | `codex/pr-023-test-kit` @ baseline `b48885afcb9d4574a2b287112252bed167a28f93` | PR-023-A01–A05 | [`artifacts/pr-023/plan.md`](artifacts/pr-023/plan.md) | 2026-08-11 | 2026-08-11 | No ADR trigger; external actions remain unauthorized |
| PR-023-T-fixtures-2c7a9e4d1b63 | PR-023 | Complete deterministic scripted context/middleware/store/observer, manual time/ID, fault, and trace helpers | Done | me@jeickmeier.com | PR-023-T-tracking-5a8d2c7e1b64 | `codex/pr-023-test-kit` | PR-023-A03–A04 | `mise run test-pr023` passed | 2026-08-11 | 2026-08-11 | 2026-08-11 |
| PR-023-T-conformance-8e3b1d6a5c72 | PR-023 | Publish stable-diagnostic conformance functions for all six primary ports | Done | me@jeickmeier.com | PR-023-T-fixtures-2c7a9e4d1b63 | `codex/pr-023-test-kit` | PR-023-A01–A03 | `mise run test-pr023` passed | 2026-08-11 | 2026-08-11 | 2026-08-11 |
| PR-023-T-goldens-4d9a2c7e6b15 | PR-023 | Add public golden scenarios and compaction integrity/checkpoint projection conformance | Done | me@jeickmeier.com | PR-023-T-fixtures-2c7a9e4d1b63 | `codex/pr-023-test-kit` | PR-023-A04–A05 | `mise run test-pr023` passed | 2026-08-11 | 2026-08-11 | 2026-08-11 |
| PR-023-T-external-7b2e9a4d1c65 | PR-023 | Prove public-only leaf provider/toolset and SDK-driven golden usage without private internals | Done | me@jeickmeier.com | PR-023-T-conformance-8e3b1d6a5c72; PR-023-T-goldens-4d9a2c7e6b15 | `codex/pr-023-test-kit` | PR-023-A01–A04 | `mise run test-pr023` passed | 2026-08-11 | 2026-08-11 | 2026-08-11 |
| PR-023-T-validation-1d6a9c4e2b83 | PR-023 | Run focused/target/dependency/schema/architecture/security/aggregate validation and bind immutable evidence | Done | me@jeickmeier.com | PR-023-T-external-7b2e9a4d1c65 | local `main` merge `949f34010a2fe121b62681560f00eac1f58dfe76` | PR-023-A01–A05 | PR-023-E-kit-5c8a2d7e1b64; PR-023-E-external-8e3b6a1d5c72; PR-023-E-compaction-4d9a7c2e6b15; PR-023-E-security-7b2e4a9d1c65; PR-023-E-ci-1d6a4c9e2b83; PR-023-E-integration-949f34010a2f | 2026-08-11 | 2026-08-11 | Local integration passed; hosted and independent review not claimed |

## Blocker ledger

Blocker IDs use `<scope>-B-short-slug-xxxxxxxxxxxx`, with a stable scope such as `PR-018`, `PH4`, or `G4` and a random 12-hex suffix. This permits parallel branches without a central counter. A blocked status without a live row here is invalid.

| Blocker | Scope | Description | Owner | Opened | Next action | Review date | Issue | Status | Resolution evidence |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| PR-003-B-no-remote-ede93913b2ea | PR-003 | Hosted PR runs and immutable Linux/macOS/Windows CI evidence were blocked until a Git remote existed. | me@jeickmeier.com | 2026-08-08 | — | 2026-08-08 | https://github.com/jeickmeier/finstack-ai | Resolved | PR-003-E-hosted-ci-b7f2fe44f7c1; PR-003-E-hosted-release-smoke-be7df0f3c45c |
| PR-007-B-contract-0c60297af156 | PR-007 | Underspecified block DTO/`ModelRef`/BlobRef-metadata/A01-evidence contract blocked coding until pack v0.13 amendment. | me@jeickmeier.com | 2026-08-08 | — | 2026-08-08 | `pr-007-content-messages` | Resolved | PLAN-0.11 / pack v0.13 amendment |
| PR-008-B-contract-ed6437d9ed08 | PR-008 | Underspecified PR-008 public vocabulary, event taxonomy, missing DTOs, record inventory, and digest ownership blocked coding until pack v0.15 amendment. | me@jeickmeier.com | 2026-08-08 | — | 2026-08-08 | [#5](https://github.com/jeickmeier/finstack-ai/pull/5) | Resolved | PLAN-0.13 / pack v0.15 amendment @ `158c0996d0ff559b39ee5cc7b6d1c8b2144aa79e` |

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
