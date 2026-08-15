# Implementation control

This directory is the live control surface for implementation. It answers what is happening now and what evidence exists; the [planning baseline](../planning/README.md) remains authoritative for requirements, design, sequencing, security controls, and acceptance prose.

## Registers

| Register | Owns |
| --- | --- |
| [ADR database](adr-register.md) and [record directory](adrs/README.md) | Decision identity, record maturity, implementation state, accountable role, delivery links, supersession, and evidence state. |
| [Delivery ledger](delivery-ledger.md) | Phase, gate, logical PR, implementation task, and blocker status. |
| [Evidence register](evidence-register.md) | Acceptance coverage, reproducible validation, artifacts, reviews, and explicit gate decisions. |
| [Exceptions register](exceptions-register.md) | Approved, time-bounded deviations from Engineering Standards. |
| [Public API change backlog](public-api-change-backlog.md) | Triaged public-surface deltas before `0.1.0`. Phase 8 entrance evidence; not the published preview policy. |

## Authority boundary

The registers may record status, assignments, links, evidence, and approved exceptions. They must not redefine:

- product scope or acceptance outcomes;
- architectural or technical decisions;
- security controls or residual-risk acceptance;
- compatibility promises; or
- phase, gate, or logical-PR requirements.

If implementation reveals a required design change, mark the affected work `Blocked`, open the required ADR or versioned planning amendment, and link that change from the register. Editing a register alone cannot authorize the change.

## Current baseline

The registers were initialized from documentation pack v0.8 and are reconciled through documentation pack v0.20, G4-D-binding-parity-101224c5eb60, G5-D-durable-beta-a9568bd869b5, and G6-D-plugin-alpha-018aaea9aa00 on 2026-08-15:

- 37 decisions are accepted and indexed. ADR-001 through ADR-021, ADR-023 through ADR-035 are `Implemented` / `Verified` through their mapped phase evidence and named gate decisions. ADR-022, ADR-036, and ADR-037 are `In progress` / `Partial`. SharedArrayBuffer is documented as post-preview reconsideration. ADR-022 stays `Partial` because browser/WASM binding peers remain. ADR-024 is `Implemented` / `Verified` for governance files only (RFC/license/contribution links); that is not G7. ADR-036 and ADR-037 stay `Partial` because G7 still owns preview closeout.
- Phase 0 through Phase 7 are `Done`; Phase 8 is `In progress` with entrance `Passed` (2/2). PR-001–PR-060 are `Done` at local `main` merge `b5266602bd77b546a65a22f91de01ab12bd652cd`; PR-061–PR-066 remain `Todo`. Phase 6 entrance and exit are `Passed` (exit 4/4). Phase 7 entrance is `Passed` (3/3) and exit is `Passed` (4/4). G4, G5, and G6 are `Passed`.
- G0 is `Passed` via `G0-D-foundation-ready-bcf021e4873a`; G1 is `Passed` via `G1-D-kernel-semantics-4f52c8a91d6e`; G2 is `Passed` via `G2-D-native-runtime-7c2e9a4d1b65`; G3 is `Passed` via `G3-D-native-preview-14a386c7db24`; G4 is `Passed` via `G4-D-binding-parity-101224c5eb60`; G5 is `Passed` via `G5-D-durable-beta-a9568bd869b5`; G6 is `Passed` via `G6-D-plugin-alpha-018aaea9aa00`; G7 and G8 remain `Not ready`.
- PR-001–PR-060 acceptance criteria are closed at local merge `b5266602bd77b546a65a22f91de01ab12bd652cd`. Artifacts live under [`artifacts/pr-043/`](artifacts/pr-043/) through [`artifacts/pr-060/`](artifacts/pr-060/). No actual pull request or hosted merge, npm/pypi publication, tag, SharedArrayBuffer, or exact 0.0.3/0.0.4 checkpoint cut is claimed.

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
