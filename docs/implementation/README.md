# Implementation control

This directory is the live control surface for implementation. It answers what is happening now and what evidence exists; the [planning baseline](../planning/README.md) remains authoritative for requirements, design, sequencing, security controls, and acceptance prose.

## Registers

| Register | Owns |
| --- | --- |
| [ADR database](adr-register.md) and [record directory](adrs/README.md) | Decision identity, record maturity, implementation state, accountable role, delivery links, supersession, and evidence state. |
| [Delivery ledger](delivery-ledger.md) | Phase, gate, logical PR, implementation task, and blocker status. |
| [Evidence register](evidence-register.md) | Acceptance coverage, reproducible validation, artifacts, reviews, and explicit gate decisions. |
| [Exceptions register](exceptions-register.md) | Approved, time-bounded deviations from Engineering Standards. |

## Authority boundary

The registers may record status, assignments, links, evidence, and approved exceptions. They must not redefine:

- product scope or acceptance outcomes;
- architectural or technical decisions;
- security controls or residual-risk acceptance;
- compatibility promises; or
- phase, gate, or logical-PR requirements.

If implementation reveals a required design change, mark the affected work `Blocked`, open the required ADR or versioned planning amendment, and link that change from the register. Editing a register alone cannot authorize the change.

## Current baseline

The registers were initialized from documentation pack v0.8 and are reconciled through documentation pack v0.20 and G4-D-binding-parity-101224c5eb60 on 2026-08-14:

- 37 decisions are accepted and indexed. ADR-001 through ADR-003, ADR-005, ADR-006, ADR-007, ADR-015, ADR-017, ADR-018, ADR-019, ADR-023, ADR-029, ADR-030, ADR-031, ADR-032, and ADR-033 are `Implemented` / `Verified` through their mapped phase evidence and named gate decisions. ADR-004, ADR-008, ADR-009, ADR-010, ADR-011, ADR-012, ADR-013, ADR-016, ADR-020, ADR-022, ADR-025, ADR-026, ADR-027, ADR-028, ADR-034, ADR-035, ADR-036, and ADR-037 are `In progress` / `Partial`. SharedArrayBuffer is documented as post-preview reconsideration. ADR-004/008/012/013/016/020/025–028/034/037 stay `Partial` because G5 remains. ADR-035 stays `Partial` because G6 remains. ADR-024 retains its documented partial later scope.
- Phase 0 through Phase 6 are `Done`; PR-001–PR-050 are `Done` at local `main` merge `5c987e379d0cd5a8e63f734eda26b69b07c4b337`; PR-051 is `In progress` on `codex/pr-051-wasmtime-host-cache`; PR-052–PR-066 are `Todo`. Phase 6 entrance and exit are `Passed` (exit 4/4). Phase 7 is `In progress` with entrance `Passed` (3/3) and exit `0/4`. G4 is `Passed`. G5 and G6 remain `Not ready`.
- G0 is `Passed` via `G0-D-foundation-ready-bcf021e4873a`; G1 is `Passed` via `G1-D-kernel-semantics-4f52c8a91d6e`; G2 is `Passed` via `G2-D-native-runtime-7c2e9a4d1b65`; G3 is `Passed` via `G3-D-native-preview-14a386c7db24`; G4 is `Passed` via `G4-D-binding-parity-101224c5eb60`; G5–G8 remain `Not ready`.
- PR-001–PR-050 acceptance criteria are closed at local merge `5c987e379d0cd5a8e63f734eda26b69b07c4b337`. Artifacts live under [`artifacts/pr-043/`](artifacts/pr-043/) through [`artifacts/pr-050/`](artifacts/pr-050/). No actual pull request or hosted merge, npm/pypi publication, tag, SharedArrayBuffer, exact 0.0.3 checkpoint cut, G5 decision, or G6 decision is claimed.

Phase and gate closure is recorded only after the named gate decision against an immutable merged commit; green CI alone does not pass a gate.

## Update discipline

Update the affected registers in the same review unit as the work they describe:

1. Assign a logical PR before implementation begins, map it to its issue, branch, or actual pull request, and create only the implementation tasks needed to execute it.
2. Record blockers when discovered, including owner, next action, and review date.
3. Add acceptance references and reproducible evidence before review; link artifacts instead of pasting large logs.
4. After merge, record the merged commit and date before changing a logical PR to `Done`.
5. Record phase completion and gate passage separately. A gate requires an explicit approval entry in the evidence register.
6. Append corrections or superseding records; do not erase historical evidence, decisions, waivers, or gate outcomes.

Use ISO 8601 dates (`YYYY-MM-DD`) and immutable commit identifiers wherever a record refers to completed work. Use `—` only for a field that is not yet known or not yet applicable; never use it to conceal a required completion condition.

## Status ownership

- The contributor or delivery owner updates tasks and logical PRs.
- The accountable workstream owner reviews PR completion and phase roll-up.
- The named gate approver records a gate decision only after reviewing its evidence.
- Maintainers approve ADR state changes and planning amendments.
- Exception review and approval follow the repository governance in force when the waiver is requested.

No status is inferred from a merged pull request, a green check, or elapsed time. The completion rules in the delivery and evidence registers control.
