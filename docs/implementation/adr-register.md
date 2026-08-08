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

PR-004 promotes ADR-001 through ADR-037 record state to `Standalone`. Implementation and evidence axes remain unchanged unless mapped delivery work produces new facts.

## Decision and implementation index

Every row's standalone-record work is owned by [PR-004](delivery-ledger.md#phase-0). The `Planned delivery` column maps implementation or enforcement work; inferred mappings are execution pointers, not new architecture decisions.

Index last reconciled: 2026-08-08 (PR-004 standalone records).

| ADR | Topic key | Accountable role | Planned delivery | Decision | Record | Implementation | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- |
| ADR-001 | `microkernel-boundary` | Core/runtime lead | PR-002, PR-008–PR-010; G1 | Accepted | Standalone | In progress | Partial (PR-002 enforcement) |
| ADR-002 | `kernel-continuation` | Core/runtime lead | PR-002, PR-008–PR-010; G1 | Accepted | Standalone | Not started | Missing |
| ADR-003 | `deterministic-effects` | Core/runtime lead | PR-002, PR-008–PR-010; G1 | Accepted | Standalone | Not started | Missing |
| ADR-004 | `commit-before-effect` | Core/runtime lead | PR-014, PR-020, PR-048; G2, G5 | Accepted | Standalone | Not started | Missing |
| ADR-005 | `six-ports` | Core/runtime lead | PR-002, PR-015–PR-018, PR-021, PR-026 | Accepted | Standalone | Not started | Missing |
| ADR-006 | `direct-native-path` | Core/runtime lead | PR-015–PR-018, PR-021, PR-026 | Accepted | Standalone | Not started | Missing |
| ADR-007 | `shared-binding-engine` | Bindings lead | PR-005, PR-027–PR-038; G4 | Accepted | Standalone | In progress | Partial (PR-005 harness Done @ `c1108d2`; Python/WASM parity deferred) |
| ADR-008 | `declarative-capabilities` | Ecosystem lead | PR-012, PR-022, PR-032, PR-038, PR-048 | Accepted | Standalone | Not started | Missing |
| ADR-009 | `observer-middleware-separation` | Core/runtime lead | PR-017, PR-018, PR-020, PR-057 | Accepted | Standalone | Not started | Missing |
| ADR-010 | `optional-wasm-isolation` | Runtime/security owner | PR-002, PR-049–PR-054; G6 | Accepted | Standalone | In progress | Partial (PR-002 wasm-host / graph checks) |
| ADR-011 | `no-native-dylib-abi` | Runtime/security owner | PR-002, PR-049–PR-054; G6 | Accepted | Standalone | In progress | Partial (PR-002 plugin-path classification) |
| ADR-012 | `immutable-lanes` | Durability/ecosystem lead | PR-006–PR-008, PR-014, PR-046–PR-048 | Accepted | Standalone | Not started | Missing |
| ADR-013 | `at-least-once-effects` | Durability/ecosystem lead | PR-006–PR-008, PR-014, PR-043, PR-046–PR-048 | Accepted | Standalone | Not started | Missing |
| ADR-014 | `protocol-separation` | Runtime/security owner | PR-049–PR-054, PR-058 | Accepted | Standalone | Not started | Missing |
| ADR-015 | `canonical-cbor` | Durability/ecosystem lead | PR-004 profile; PR-008 digest-domain freeze; PR-039 codec/digests | Accepted | Standalone | Not started | Missing |
| ADR-016 | `sqlite-before-multilane` | Durability/ecosystem lead | PR-040 before PR-047 | Accepted | Standalone | Not started | Missing |
| ADR-017 | `single-python-wheel` | Bindings lead | Freeze before PR-027; verify through PR-032 | Accepted | Standalone | Not started | Missing |
| ADR-018 | `python-version-matrix` | Bindings lead | Approve before PR-027; verify through PR-032 | Accepted | Standalone | Not started | Missing |
| ADR-019 | `browser-host-adapter` | Bindings lead | PR-034, PR-038 | Accepted | Standalone | Not started | Missing |
| ADR-020 | `capability-delivery` | Ecosystem lead | PR-012, PR-022, PR-032, PR-038, PR-048 | Accepted | Standalone | Not started | Missing |
| ADR-021 | `shared-framing` | Runtime/security owner | PR-058 | Accepted | Standalone | Not started | Missing |
| ADR-022 | `json-schema-2020-12` | Ecosystem lead | Decide before PR-012; PR-031 and binding peers | Accepted | Standalone | Not started | Missing |
| ADR-023 | `reference-provider` | Ecosystem lead | PR-024 | Accepted | Standalone | Not started | Missing |
| ADR-024 | `governance-and-license` | Quality/release owner | PR-001, PR-004; G0 | Accepted | Standalone | In progress | Partial |
| ADR-025 | `generic-deferred-effects` | Durability/ecosystem lead | PR-008, PR-014, PR-042–PR-044, PR-048 | Accepted | Standalone | Not started | Missing |
| ADR-026 | `run-lineage` | Durability/ecosystem lead | PR-006, PR-008, PR-046–PR-048 | Accepted | Standalone | Not started | Missing |
| ADR-027 | `typed-interactions` | Durability/ecosystem lead | PR-008, PR-018, PR-044, PR-048 | Accepted | Standalone | Not started | Missing |
| ADR-028 | `before-finalize` | Core/runtime lead | PR-009, PR-018, PR-048 | Accepted | Standalone | Not started | Missing |
| ADR-029 | `typed-identifiers` | Core/runtime lead | PR-006 | Accepted | Standalone | Implemented | Verified |
| ADR-030 | `boxed-port-abi` | Core/runtime lead | PR-015–PR-018, PR-026; G3 | Accepted | Standalone | Not started | Missing |
| ADR-031 | `worker-based-wasm` | Bindings lead | PR-033, PR-036, PR-038 | Accepted | Standalone | Not started | Missing |
| ADR-032 | `disposable-snapshots` | Durability/ecosystem lead | PR-041 | Accepted | Standalone | Not started | Missing |
| ADR-033 | `explicit-interruption` | Core/runtime lead | PR-042 | Accepted | Standalone | Not started | Missing |
| ADR-034 | `durable-middleware-outcomes` | Core/runtime lead | PR-018, PR-048 | Accepted | Standalone | Not started | Missing |
| ADR-035 | `experimental-wit-versioning` | Runtime/security owner | PR-049–PR-054, PR-062 | Accepted | Standalone | Not started | Missing |
| ADR-036 | `blob-storage-boundary` | Ecosystem lead | PR-007, PR-022, PR-037, PR-056 | Accepted | Standalone | In progress | Partial |
| ADR-037 | `middleware-compaction` | Core/runtime lead | PR-018, PR-023, PR-048, PR-056 | Accepted | Standalone | Not started | Missing |

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
| 2026-08-08 | ADR-024 | Implementation | me@jeickmeier.com | Not started | In progress | PR-001 license/governance files | — | — |
| 2026-08-08 | ADR-024 | Evidence | me@jeickmeier.com | Missing | Partial | PR-001 license/governance files (ADR text still PR-004) | — | — |
| 2026-08-08 | ADR-024 | Evidence | me@jeickmeier.com | Partial | Missing | Revert uncommitted Partial claim; durable evidence awaits immutable commit | — | — |
| 2026-08-08 | ADR-024 | Evidence | me@jeickmeier.com | Missing | Partial | PR-001 license/governance files at `73bfe88c8dbc92c4e4c6eba1a5a7814240c2e862`; standalone ADR text remains PR-004 | PR-001-E-ownership-review-b14626f70259; PR-001-E-security-md-f70391db8ac2 | me@jeickmeier.com |
| 2026-08-08 | ADR-001 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-001-microkernel-boundary.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-002 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-002-kernel-continuation.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-003 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-003-deterministic-effects.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-004 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-004-commit-before-effect.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-005 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-005-six-ports.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-006 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-006-direct-native-path.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-007 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-007-shared-binding-engine.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-008 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-008-declarative-capabilities.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-009 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-009-observer-middleware-separation.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-010 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-010-optional-wasm-isolation.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-011 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-011-no-native-dylib-abi.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-012 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-012-immutable-lanes.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-013 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-013-at-least-once-effects.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-014 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-014-protocol-separation.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-015 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-015-canonical-cbor.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-016 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-016-sqlite-before-multilane.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-017 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-017-single-python-wheel.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-018 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-018-python-version-matrix.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-019 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-019-browser-host-adapter.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-020 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-020-capability-delivery.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-021 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-021-shared-framing.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-022 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-022-json-schema-2020-12.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-023 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-023-reference-provider.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-024 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-024-governance-and-license.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-025 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-025-generic-deferred-effects.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-026 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-026-run-lineage.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-027 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-027-typed-interactions.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-028 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-028-before-finalize.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-029 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-029-typed-identifiers.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-030 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-030-boxed-port-abi.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-031 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-031-worker-based-wasm.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-032 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-032-disposable-snapshots.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-033 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-033-explicit-interruption.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-034 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-034-durable-middleware-outcomes.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-035 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-035-experimental-wit-versioning.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-036 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-036-blob-storage-boundary.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-037 | Record | me@jeickmeier.com | Indexed | Standalone | PR-004 standalone record ADR-037-middleware-compaction.md | — | me@jeickmeier.com |
| 2026-08-08 | ADR-029 | Implementation | me@jeickmeier.com | Not started | In progress | PR-006 typed identifier / RawJson / error foundations on `pr-006-kernel-value-types` | — | — |
| 2026-08-08 | ADR-029 | Implementation | me@jeickmeier.com | In progress | Implemented | PR-006 merged [#4](https://github.com/jeickmeier/finstack-ai/pull/4) @ `56d7777956df145213b03d2b0b5c1922db42b346` | PR-006-E-hosted-ci-9a1ccbb88ae5; PR-006-E-test-kernel-bc373cf935e4 | me@jeickmeier.com |
| 2026-08-08 | ADR-036 | Implementation | me@jeickmeier.com | Not started | In progress | PR-007 BlobRef / no seventh-port boundary at `4a0a77f668adcfa2679dc161779c4456d721c0c2` | PR-007-E-security-1146ce970799 | — |
| 2026-08-08 | ADR-036 | Evidence | me@jeickmeier.com | Missing | Partial | PR-007 local TM-20 / BlobRef fixtures; full Verified awaits merge + remaining mapped PRs | PR-007-E-security-1146ce970799; PR-007-E-conformance-8b7b9c728785 | me@jeickmeier.com |

## Current record and evidence links

| ADR | Standalone record | Assigned to | Current evidence | Change reference | Updated |
| --- | --- | --- | --- | --- | --- |
| ADR-001 | [ADR-001-microkernel-boundary.md](adrs/ADR-001-microkernel-boundary.md) | me@jeickmeier.com | Partial (PR-002 enforcement): PR-002-E-architecture-6ed3268e6ff5; PR-002-E-dep-direction-321ce9b2b4b4 at `ee9754fe2d0f015181dcefa97e715392aadd28ed` | PR-004 standalone ADR; PR-002 architecture enforcement | 2026-08-08 |
| ADR-002 | [ADR-002-kernel-continuation.md](adrs/ADR-002-kernel-continuation.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-003 | [ADR-003-deterministic-effects.md](adrs/ADR-003-deterministic-effects.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-004 | [ADR-004-commit-before-effect.md](adrs/ADR-004-commit-before-effect.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-005 | [ADR-005-six-ports.md](adrs/ADR-005-six-ports.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-006 | [ADR-006-direct-native-path.md](adrs/ADR-006-direct-native-path.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-007 | [ADR-007-shared-binding-engine.md](adrs/ADR-007-shared-binding-engine.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-008 | [ADR-008-declarative-capabilities.md](adrs/ADR-008-declarative-capabilities.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-009 | [ADR-009-observer-middleware-separation.md](adrs/ADR-009-observer-middleware-separation.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-010 | [ADR-010-optional-wasm-isolation.md](adrs/ADR-010-optional-wasm-isolation.md) | me@jeickmeier.com | Partial (PR-002 wasm-host / graph checks): PR-002-E-architecture-6ed3268e6ff5; PR-002-E-wasm-binding-5253da7a99f9; PR-002-E-review-627382618b20 at `ee9754fe2d0f015181dcefa97e715392aadd28ed` | PR-004 standalone ADR; PR-002 wasm-host checks | 2026-08-08 |
| ADR-011 | [ADR-011-no-native-dylib-abi.md](adrs/ADR-011-no-native-dylib-abi.md) | me@jeickmeier.com | Partial (PR-002 plugin-path classification): PR-002-E-architecture-6ed3268e6ff5; PR-002-E-dep-direction-321ce9b2b4b4; PR-002-E-review-627382618b20 at `ee9754fe2d0f015181dcefa97e715392aadd28ed` | PR-004 standalone ADR; PR-002 plugin-path classification | 2026-08-08 |
| ADR-012 | [ADR-012-immutable-lanes.md](adrs/ADR-012-immutable-lanes.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-013 | [ADR-013-at-least-once-effects.md](adrs/ADR-013-at-least-once-effects.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-014 | [ADR-014-protocol-separation.md](adrs/ADR-014-protocol-separation.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-015 | [ADR-015-canonical-cbor.md](adrs/ADR-015-canonical-cbor.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-016 | [ADR-016-sqlite-before-multilane.md](adrs/ADR-016-sqlite-before-multilane.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-017 | [ADR-017-single-python-wheel.md](adrs/ADR-017-single-python-wheel.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-018 | [ADR-018-python-version-matrix.md](adrs/ADR-018-python-version-matrix.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-019 | [ADR-019-browser-host-adapter.md](adrs/ADR-019-browser-host-adapter.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-020 | [ADR-020-capability-delivery.md](adrs/ADR-020-capability-delivery.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-021 | [ADR-021-shared-framing.md](adrs/ADR-021-shared-framing.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-022 | [ADR-022-json-schema-2020-12.md](adrs/ADR-022-json-schema-2020-12.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-023 | [ADR-023-reference-provider.md](adrs/ADR-023-reference-provider.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-024 | [ADR-024-governance-and-license.md](adrs/ADR-024-governance-and-license.md) | me@jeickmeier.com | Standalone record plus PR-001 license/governance files at `73bfe88c8dbc92c4e4c6eba1a5a7814240c2e862`; evidence IDs PR-001-E-ownership-review-b14626f70259, PR-001-E-security-md-f70391db8ac2 | PR-004 standalone ADR; PR-001 license/governance files | 2026-08-08 |
| ADR-025 | [ADR-025-generic-deferred-effects.md](adrs/ADR-025-generic-deferred-effects.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-026 | [ADR-026-run-lineage.md](adrs/ADR-026-run-lineage.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-027 | [ADR-027-typed-interactions.md](adrs/ADR-027-typed-interactions.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-028 | [ADR-028-before-finalize.md](adrs/ADR-028-before-finalize.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-029 | [ADR-029-typed-identifiers.md](adrs/ADR-029-typed-identifiers.md) | me@jeickmeier.com | Verified: PR-006-E-test-kernel-bc373cf935e4; PR-006-E-conformance-adfd96007628; PR-006-E-hosted-ci-9a1ccbb88ae5 at merge `56d7777956df145213b03d2b0b5c1922db42b346` | PR-004 standalone ADR; PR-006 [#4](https://github.com/jeickmeier/finstack-ai/pull/4) | 2026-08-08 |
| ADR-030 | [ADR-030-boxed-port-abi.md](adrs/ADR-030-boxed-port-abi.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-031 | [ADR-031-worker-based-wasm.md](adrs/ADR-031-worker-based-wasm.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-032 | [ADR-032-disposable-snapshots.md](adrs/ADR-032-disposable-snapshots.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-033 | [ADR-033-explicit-interruption.md](adrs/ADR-033-explicit-interruption.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-034 | [ADR-034-durable-middleware-outcomes.md](adrs/ADR-034-durable-middleware-outcomes.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-035 | [ADR-035-experimental-wit-versioning.md](adrs/ADR-035-experimental-wit-versioning.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |
| ADR-036 | [ADR-036-blob-storage-boundary.md](adrs/ADR-036-blob-storage-boundary.md) | me@jeickmeier.com | Partial (PR-007 BlobRef boundary): PR-007-E-security-1146ce970799; PR-007-E-conformance-8b7b9c728785; PR-007-E-merge-ci-acf14a6a28ca at merge `81a8706aeeca6a47ab0d64bc0bef681d6efc4621` | PR-004 standalone ADR; PR-007 content/messages | 2026-08-08 |
| ADR-037 | [ADR-037-middleware-compaction.md](adrs/ADR-037-middleware-compaction.md) | me@jeickmeier.com | — | PR-004 standalone ADR | 2026-08-08 |

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
