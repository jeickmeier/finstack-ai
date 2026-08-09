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

## Initial state

The registers were initialized from documentation pack v0.8 and reconciled through documentation pack v0.15 on 2026-08-08 (PR-008 contract freeze on `pr-008-contract-amendment` / [#5](https://github.com/jeickmeier/finstack-ai/pull/5)):

- 37 decisions are accepted and indexed; ADR-007 remains `Partial` (shared conformance harness present; binding parity deferred); ADR-024 license/governance files remain `Partial`; ADR-036 is `In progress` / `Partial` under PR-007.
- Phase 0 is `Done` at merge `c1108d207389a947d16e9b0dd7a76026108c01eb`; PR-001–PR-007 are `Done`; PR-008 is `In progress` on [#5](https://github.com/jeickmeier/finstack-ai/pull/5) (A01–A08 reopened as `Failed` by `PR-008-E-code-review-f82034377d5e`; remediation active; `PR-008-B-contract-ed6437d9ed08` Resolved); PR-009–PR-066 are `Todo`.
- Gate G0 is `Passed` via `G0-D-foundation-ready-bcf021e4873a`; G1–G8 remain `Not ready`.
- PR-001–PR-007 acceptance criteria are closed. PR-007 evidence is bound under [`artifacts/pr-007/`](artifacts/pr-007/). No exceptions are open. No open blockers.

Phase 0 / G0 closure is recorded only after the named gate decision against the immutable merged commit; green CI alone does not pass G0.

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
