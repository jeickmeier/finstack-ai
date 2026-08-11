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

The registers were initialized from documentation pack v0.8 and are reconciled through documentation pack v0.20 and local PR-023 merge `949f34010a2fe121b62681560f00eac1f58dfe76` on 2026-08-11:

- 37 decisions are accepted and indexed. ADR-001 through ADR-003 are `Implemented` / `Verified` through the merged Phase 1 evidence and G1 decision. ADR-004–ADR-006, ADR-008, ADR-009, ADR-020, ADR-022, ADR-030, ADR-034, ADR-036, and ADR-037 are `In progress` / `Partial`; PR-022 adds immutable declarative capability, exact-lock, child/budget/artifact-service, and activation-rebuild evidence, while PR-023 adds immutable public adversarial compaction conformance. Explicitly mapped later work remains open. ADR-025 through ADR-028 remain `In progress` / `Partial` because their remaining routing, durability, and lifecycle work is later. ADR-007 and ADR-024 also retain their documented partial later scope.
- Phase 0 through Phase 2 are `Done`; Phase 3 is `In progress`; PR-001–PR-023 are `Done`; PR-024–PR-066 are `Todo`. Both Phase 3 entrance criteria and PR-023 A01–A05 pass with local integration evidence.
- G0 is `Passed` via `G0-D-foundation-ready-bcf021e4873a`; G1 is `Passed` via `G1-D-kernel-semantics-4f52c8a91d6e`; G2 is `Passed` via `G2-D-native-runtime-7c2e9a4d1b65`; G3–G8 remain `Not ready`.
- PR-001–PR-023 acceptance criteria are closed. PR-020 A01–A04, all four Phase 2 exits, local integration, and G2 are bound under [`artifacts/pr-020/`](artifacts/pr-020/). PR-021 A01–A05 and PR-022 A01–A12 are bound under their corresponding artifact directories. PR-023 A01–A05, candidate validation, security review, and local integration are bound under [`artifacts/pr-023/`](artifacts/pr-023/). No PR-023 actual pull request, push, hosted run, independent review, publication, exception, or blocker is claimed.

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
