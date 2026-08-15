# ADR-011: no native dylib abi

## Status

Accepted

## Date

2026-08-08

## Accountable role

Runtime/security owner

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

This decision was accepted in the planning baseline and is recorded here so later
implementation pull requests cannot silently reinterpret it. Canonical summary
text lives in Architecture Specification §25; this standalone record captures
stable decision context, consequences, compatibility classification, security
linkage, and change-control metadata required by PR-004.

## Decision

Native dynamic-library plugins are not a supported primary ABI

## Consequences

Untrusted extensions use WIT/Wasmtime or process isolation, not native dylib loading.

## Rejected alternatives

Native dynamic-library plugins as a supported primary ABI.

## Compatibility and schema-change classification

Plugin ABI policy; native dylib loading remains unsupported without a new ADR.

## Security classification

- References: SEC-INV-006, SEC-INV-007, SEC-INV-011, SEC-INV-012; TM-06, TM-07, TM-08
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-PLG
- Affected Technical Design: Technical Design §27 (no native dylib ABI)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-002, PR-049–PR-054; G6
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Partial via PR-002 plugin-path classification, PR-049/PR-050 WIT leaf, and PR-051-E-security-36f1ce2e263b / PR-051-E-integration-cf7eaebab383 (no dylib loader or libloading on the Wasmtime host). G6 remains.
