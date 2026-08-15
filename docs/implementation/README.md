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
| [Preview compatibility policy](preview-compatibility-policy.md) | Adopter-facing tagged `0.1.0` preview promise. Not a 1.0 SemVer guarantee. |
| [1.0 compatibility policy](1.0-compatibility-policy.md) | Adopter-facing 1.0 SemVer promise. Approved by `COMP-1.0-D-contract-freeze-00b78667ecc4`. G8 passed via `G8-D-general-availability-a889a29a3f54`. |
| [1.0 leaf versioning](1.0-leaf-versioning.md) | Lockstep core; independent leaves only after named coupling harm. |
| [1.0 deprecation inventory](1.0-deprecation-inventory.md) | Remaining deprecations with `since` and removal versions. |
| [Public preview roadmap](public-preview-roadmap.md) | In-repo preview limitations and deferred Phase 9 themes. |
| [Public GA roadmap](public-ga-roadmap.md) | Post-1.0 in-repo themes. Marketplace, channel catalog, and product UIs stay excluded. |
| [1.0 compatibility matrix](1.0-compatibility-matrix.md) | Frozen families vs experimental surfaces at tagged local `1.0.0`. |
| [Preview feedback review](preview-feedback-review.md) | First-party review of telemetry, issue patterns, API pain, and migration needs. Does not satisfy Phase 9 entrance. |
| [Performance budgets](perf-budgets.md) | Operational NFR-PERF-001–007 register. Fail vs warning hosts, size table, and measurement commands. |
| [Release engineering](release-engineering.md) | Local recreate, signing/provenance procedure, rollback/hotfix, nightly/canary definitions. Local tag `v1.0.0` exists; registries unpublished. |
| [Support windows](support-windows.md) | Operational support-window and maintenance-branch procedure. In force at G8. Not LTS. |

## Authority boundary

The registers may record status, assignments, links, evidence, and approved exceptions. They must not redefine:

- product scope or acceptance outcomes;
- architectural or technical decisions;
- security controls or residual-risk acceptance;
- compatibility promises; or
- phase, gate, or logical-PR requirements.

If implementation reveals a required design change, mark the affected work `Blocked`, open the required ADR or versioned planning amendment, and link that change from the register. Editing a register alone cannot authorize the change.

## Current baseline

The registers were initialized from documentation pack v0.8 and are reconciled through documentation pack v0.21, G4-D-binding-parity-101224c5eb60, G5-D-durable-beta-a9568bd869b5, G6-D-plugin-alpha-018aaea9aa00, G7-D-public-preview-f7c7e70b9e04, and G8-D-general-availability-a889a29a3f54 on 2026-08-15:

- 37 decisions are accepted and indexed. ADR-001 through ADR-021 and ADR-023 through ADR-037 are `Implemented` / `Verified` through their mapped phase evidence and named gate decisions. ADR-022 stays `In progress` / `Partial` because browser/WASM binding peers remain. SharedArrayBuffer is documented as post-preview reconsideration. ADR-024 is `Implemented` / `Verified` for governance files only (RFC/license/contribution links).
- Phase 0 through Phase 9 are `Done`. PR-001–PR-066 are `Done`. Phase 8 and Phase 9 entrance and exit are `Passed`. G0–G8 are `Passed`.
- G0 is `Passed` via `G0-D-foundation-ready-bcf021e4873a`; G1 is `Passed` via `G1-D-kernel-semantics-4f52c8a91d6e`; G2 is `Passed` via `G2-D-native-runtime-7c2e9a4d1b65`; G3 is `Passed` via `G3-D-native-preview-14a386c7db24`; G4 is `Passed` via `G4-D-binding-parity-101224c5eb60`; G5 is `Passed` via `G5-D-durable-beta-a9568bd869b5`; G6 is `Passed` via `G6-D-plugin-alpha-018aaea9aa00`; G7 is `Passed` via `G7-D-public-preview-f7c7e70b9e04`; G8 is `Passed` via `G8-D-general-availability-a889a29a3f54`.
- PR-001–PR-066 acceptance criteria are closed at local merge `4888cfe285b2897b1da4e6a2d13ceb8c8bdda1a5` (PR-066 A01–A04 Passed). G8 is recorded against `6e9ec39fae89a70f696ee740de2d2094670cba3e`. Local tag `v1.0.0` points at that commit and is not pushed. Tag `v0.1.0` remains the last GitHub tag. Phase 9 entrance stays `PH9-E-entrance-tag-b610b0ba93b5` and `PH9-E-entrance-feedback-ee6999c59a12`. No npm/pypi/crates.io publication, GitHub Release, announce, or SharedArrayBuffer is claimed. The §21.12 soak gap is an accepted G8 residual.

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
