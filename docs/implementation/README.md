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

The registers were initialized from documentation pack v0.8 and are reconciled through documentation pack v0.20, local PR-016 merge `b2f678693fcd3eb2fe09aafca55c4e06c17371ec`, and local PR-017 merge `966f047fd28972d35c835ddf0441f8ba67348b25` on 2026-08-10:

- 37 decisions are accepted and indexed. ADR-001 through ADR-003 are `Implemented` / `Verified` through the merged Phase 1 evidence and G1 decision. ADR-005, ADR-006, ADR-009, ADR-022, and ADR-030 are `In progress` / `Partial`; PR-015 contributes immutable Model-port evidence, PR-016 contributes immutable Toolset, Draft 2020-12 validator, direct-native, and boxed-ABI evidence, and PR-017 contributes locally integrated bounded event-delivery and observer-separation evidence while their remaining mapped work is open. ADR-025 through ADR-028 remain `In progress` / `Partial` because their planned runtime, routing, durability, and lifecycle work remains. ADR-007, ADR-024, and ADR-036 also retain their documented partial later scope.
- Phase 0 and Phase 1 are `Done`; PR-001–PR-017 are `Done`; PR-018–PR-066 are `Todo`.
- G0 is `Passed` via `G0-D-foundation-ready-bcf021e4873a`; G1 is `Passed` via `G1-D-kernel-semantics-4f52c8a91d6e`; G2–G8 remain `Not ready`.
- PR-001–PR-017 acceptance criteria are closed. PR-015 A01–A04, PR-016 A01–A05, and PR-017 A01–A04 are bound to immutable implementation and local merge evidence retained under [`artifacts/pr-015/`](artifacts/pr-015/), [`artifacts/pr-016/`](artifacts/pr-016/), and [`artifacts/pr-017/`](artifacts/pr-017/). No PR-017 actual pull request, push, hosted run, independent review, G2 decision, exception, or blocker is claimed.

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
