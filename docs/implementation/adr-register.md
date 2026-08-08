# ADR database

This register is the implementation index for architectural decisions. It does not duplicate decision text or rationale. The canonical decision summaries are in [Architecture Specification section 25](../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary); detailed delivery metadata for ADR-015 through ADR-024 is in [PRD section 18](../planning/01-finstack-ai-product-requirements.md#18-resolved-foundational-product-decisions), and reconsideration conditions for ADR-029 through ADR-037 are in [Technical Design section 37](../planning/03-finstack-ai-technical-design.md#37-technical-decision-status).

There are no open architectural decisions and no recorded supersession relationships in the pre-implementation baseline.

## Status model

Each state axis is independent:

| Axis | Allowed values | Completion meaning |
| --- | --- | --- |
| Decision | `Proposed`, `Accepted`, `Superseded`, `Rejected` | Maintainer-approved decision state. |
| Record | `Indexed`, `Standalone` | `Standalone` means a versioned ADR file exists in the [ADR record directory](adrs/README.md), meets PR-004 governance requirements, and is linked below. |
| Implementation | `Not started`, `In progress`, `Implemented`, `N/A - policy` | `Implemented` requires the mapped delivery work to be complete. Policy decisions still require enforcement evidence before `N/A - policy` is used. |
| Evidence | `Missing`, `Partial`, `Verified` | `Verified` requires linked records in the [evidence register](evidence-register.md). |

All existing ADRs begin `Accepted / Indexed / Not started / Missing`. Creating this database does not make their record state `Standalone` and does not complete logical PR-004.

## Decision and implementation index

Every row's standalone-record work is owned by [PR-004](delivery-ledger.md#phase-0). The `Planned delivery` column maps implementation or enforcement work; inferred mappings are execution pointers, not new architecture decisions.

Index last reconciled: 2026-08-08.

| ADR | Topic key | Accountable role | Planned delivery | Decision | Record | Implementation | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- |
| ADR-001 | `microkernel-boundary` | Core/runtime lead | PR-002, PR-008–PR-010; G1 | Accepted | Indexed | Not started | Missing |
| ADR-002 | `kernel-continuation` | Core/runtime lead | PR-002, PR-008–PR-010; G1 | Accepted | Indexed | Not started | Missing |
| ADR-003 | `deterministic-effects` | Core/runtime lead | PR-002, PR-008–PR-010; G1 | Accepted | Indexed | Not started | Missing |
| ADR-004 | `commit-before-effect` | Core/runtime lead | PR-014, PR-020, PR-048; G2, G5 | Accepted | Indexed | Not started | Missing |
| ADR-005 | `six-ports` | Core/runtime lead | PR-002, PR-015–PR-018, PR-021, PR-026 | Accepted | Indexed | Not started | Missing |
| ADR-006 | `direct-native-path` | Core/runtime lead | PR-015–PR-018, PR-021, PR-026 | Accepted | Indexed | Not started | Missing |
| ADR-007 | `shared-binding-engine` | Bindings lead | PR-005, PR-027–PR-038; G4 | Accepted | Indexed | Not started | Missing |
| ADR-008 | `declarative-capabilities` | Ecosystem lead | PR-012, PR-022, PR-032, PR-038, PR-048 | Accepted | Indexed | Not started | Missing |
| ADR-009 | `observer-middleware-separation` | Core/runtime lead | PR-017, PR-018, PR-020, PR-057 | Accepted | Indexed | Not started | Missing |
| ADR-010 | `optional-wasm-isolation` | Runtime/security owner | PR-002, PR-049–PR-054; G6 | Accepted | Indexed | Not started | Missing |
| ADR-011 | `no-native-dylib-abi` | Runtime/security owner | PR-002, PR-049–PR-054; G6 | Accepted | Indexed | Not started | Missing |
| ADR-012 | `immutable-lanes` | Durability/ecosystem lead | PR-006–PR-008, PR-014, PR-046–PR-048 | Accepted | Indexed | Not started | Missing |
| ADR-013 | `at-least-once-effects` | Durability/ecosystem lead | PR-006–PR-008, PR-014, PR-043, PR-046–PR-048 | Accepted | Indexed | Not started | Missing |
| ADR-014 | `protocol-separation` | Runtime/security owner | PR-049–PR-054, PR-058 | Accepted | Indexed | Not started | Missing |
| ADR-015 | `canonical-cbor` | Durability/ecosystem lead | PR-004 profile; PR-039 implementation | Accepted | Indexed | Not started | Missing |
| ADR-016 | `sqlite-before-multilane` | Durability/ecosystem lead | PR-040 before PR-047 | Accepted | Indexed | Not started | Missing |
| ADR-017 | `single-python-wheel` | Bindings lead | Freeze before PR-027; verify through PR-032 | Accepted | Indexed | Not started | Missing |
| ADR-018 | `python-version-matrix` | Bindings lead | Approve before PR-027; verify through PR-032 | Accepted | Indexed | Not started | Missing |
| ADR-019 | `browser-host-adapter` | Bindings lead | PR-034, PR-038 | Accepted | Indexed | Not started | Missing |
| ADR-020 | `capability-delivery` | Ecosystem lead | PR-012, PR-022, PR-032, PR-038, PR-048 | Accepted | Indexed | Not started | Missing |
| ADR-021 | `shared-framing` | Runtime/security owner | PR-058 | Accepted | Indexed | Not started | Missing |
| ADR-022 | `json-schema-2020-12` | Ecosystem lead | Decide before PR-012; PR-031 and binding peers | Accepted | Indexed | Not started | Missing |
| ADR-023 | `reference-provider` | Ecosystem lead | PR-024 | Accepted | Indexed | Not started | Missing |
| ADR-024 | `governance-and-license` | Quality/release owner | PR-001, PR-004; G0 | Accepted | Indexed | Not started | Missing |
| ADR-025 | `generic-deferred-effects` | Durability/ecosystem lead | PR-008, PR-014, PR-042–PR-044, PR-048 | Accepted | Indexed | Not started | Missing |
| ADR-026 | `run-lineage` | Durability/ecosystem lead | PR-006, PR-008, PR-046–PR-048 | Accepted | Indexed | Not started | Missing |
| ADR-027 | `typed-interactions` | Durability/ecosystem lead | PR-008, PR-018, PR-044, PR-048 | Accepted | Indexed | Not started | Missing |
| ADR-028 | `before-finalize` | Core/runtime lead | PR-009, PR-018, PR-048 | Accepted | Indexed | Not started | Missing |
| ADR-029 | `typed-identifiers` | Core/runtime lead | PR-006 | Accepted | Indexed | Not started | Missing |
| ADR-030 | `boxed-port-abi` | Core/runtime lead | PR-015–PR-018, PR-026; G3 | Accepted | Indexed | Not started | Missing |
| ADR-031 | `worker-based-wasm` | Bindings lead | PR-033, PR-036, PR-038 | Accepted | Indexed | Not started | Missing |
| ADR-032 | `disposable-snapshots` | Durability/ecosystem lead | PR-041 | Accepted | Indexed | Not started | Missing |
| ADR-033 | `explicit-interruption` | Core/runtime lead | PR-042 | Accepted | Indexed | Not started | Missing |
| ADR-034 | `durable-middleware-outcomes` | Core/runtime lead | PR-018, PR-048 | Accepted | Indexed | Not started | Missing |
| ADR-035 | `experimental-wit-versioning` | Runtime/security owner | PR-049–PR-054, PR-062 | Accepted | Indexed | Not started | Missing |
| ADR-036 | `blob-storage-boundary` | Ecosystem lead | PR-007, PR-022, PR-037, PR-056 | Accepted | Indexed | Not started | Missing |
| ADR-037 | `middleware-compaction` | Core/runtime lead | PR-018, PR-023, PR-048, PR-056 | Accepted | Indexed | Not started | Missing |

## Security review seed

[Security and Threat Model](../planning/06-finstack-ai-security-threat-model.md) references must be copied into each applicable standalone ADR under PR-004. The table is a pre-seeded review map, not a substitute for PR-004's row-by-row threat-boundary review. Every standalone ADR must explicitly record either its applicable SEC-INV/TM references or a reviewed `No direct security control` classification. Absence from this table would mean `Pending review`, not `No impact`.

| ADRs | Pre-seeded security references |
| --- | --- |
| ADR-001–ADR-003 | SEC-INV-001, SEC-INV-002; TM-01, TM-02, TM-14 |
| ADR-004 | SEC-INV-002; TM-12, TM-14 |
| ADR-005–ADR-007 | TM-04–TM-06 |
| ADR-008, ADR-020 | SEC-INV-001; TM-01 |
| ADR-009, ADR-028 | SEC-INV-009; TM-17 |
| ADR-010, ADR-011, ADR-035 | SEC-INV-006, SEC-INV-007, SEC-INV-011, SEC-INV-012; TM-06–TM-08 |
| ADR-012, ADR-016, ADR-026, ADR-029 | SEC-INV-008; TM-12, TM-15, TM-19 |
| ADR-013, ADR-025, ADR-033 | SEC-INV-003, SEC-INV-004, SEC-INV-008; TM-10, TM-14 |
| ADR-014, ADR-021 | TM-09 |
| ADR-015 | SEC-INV-007, SEC-INV-010; TM-09, TM-12, TM-16 |
| ADR-017, ADR-018, ADR-024 | TM-18 |
| ADR-019, ADR-031 | TM-05 |
| ADR-022 | TM-02, TM-16 |
| ADR-023 | SEC-INV-005; TM-04 |
| ADR-027 | SEC-INV-003, SEC-INV-004; TM-11 |
| ADR-030 | TM-06 |
| ADR-032 | TM-13 |
| ADR-034 | SEC-INV-013; TM-21 |
| ADR-036 | TM-16, TM-20 |
| ADR-037 | SEC-INV-013; TM-21 |

## Change control

- ADR-001 through ADR-028 may change only through a new superseding ADR and reconciliation of every affected primary planning document.
- ADR-029 through ADR-037 additionally require the applicable reconsideration evidence in [Technical Design section 37](../planning/03-finstack-ai-technical-design.md#37-technical-decision-status).
- A decision cannot move to `Superseded` until the replacement ADR is accepted and both directions of the relationship are recorded below.
- An implementation state cannot move to `Implemented` until all mapped delivery work is done or explicitly dispositioned and supporting evidence is `Verified`.
- New ADRs receive the next unused numeric ID. IDs are never reused.

## Assignment and implementation change log

Add a row whenever an ADR is assigned or any state axis changes. This is append-only; the latest valid row drives the corresponding state in the decision index or the assignment in the current record-and-evidence links below.

| Date | ADR | Axis | Assigned to | Previous value | New value | Change reference | Evidence | Reviewed by |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |

<!-- Example shape only; remove this comment when adding the first real row.
| YYYY-MM-DD | ADR-NNN | Decision/Record/Implementation/Evidence/Assignment | person/team | Not started | In progress | issue or PR URL | scoped evidence ID | reviewer |
-->

## Current record and evidence links

Add a row when an ADR gains a standalone record, an active assignee, or implementation evidence. A `Standalone`, `Partial`, or `Verified` state without the corresponding link here is invalid.

| ADR | Standalone record | Assigned to | Current evidence | Change reference | Updated |
| --- | --- | --- | --- | --- | --- |

<!-- Example shape only; remove this comment when adding the first real row.
| ADR-NNN | `adrs/ADR-NNN-short-topic.md` | person/team | scoped evidence IDs | issue or PR URL | YYYY-MM-DD |
-->

## Supersession log

No supersessions are recorded.

| Date | Superseded ADR | Superseding ADR | Planning reconciliation | Approval evidence |
| --- | --- | --- | --- | --- |

## Standalone record requirements

PR-004 creates versioned records under [`docs/implementation/adrs/`](adrs/README.md), named `ADR-NNN-short-topic.md`. A record reaches `Standalone` only when it is linked in the current record index and contains:

- status, date, accountable role, and decision owners;
- context, decision, consequences, and rejected alternatives;
- compatibility and schema-change classification;
- affected requirements, technical design, security controls, and delivery references;
- supersession metadata and reconsideration conditions; and
- approval and implementation-evidence links.

The standalone record owns the detailed rationale after acceptance, while this database remains the operational index.
