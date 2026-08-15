# ADR-035: experimental wit versioning

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

WIT plugin alpha uses experimental @0.x tool/context packages with one coarse completion; @1.0.0 worlds freeze only at the framework 1.0.0 gate and resource-based streaming remains deferred

## Consequences

Plugin authors target experimental worlds until the 1.0 gate.

## Rejected alternatives

Freezing WIT @1.0.0 worlds before the framework 1.0 compatibility gate.

## Compatibility and schema-change classification

WIT package/world versioning; experimental @0.x until framework 1.0.

## Security classification

- References: SEC-INV-006, SEC-INV-007, SEC-INV-011, SEC-INV-012; TM-06, TM-07, TM-08
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-PLG
- Affected Technical Design: Technical Design §27 (experimental WIT versioning)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-049–PR-054, PR-062
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate
- Reconsideration source: [Technical Design §37](../../planning/03-finstack-ai-technical-design.md#37-technical-decision-status)

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

Exact Technical Design §37 gate: Post-plugin-alpha evidence on async support, cleanup, overhead, and compatibility.

Additionally requires a superseding ADR and primary-document reconciliation.

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Partial via PR-049-E-candidate-9fbb520f2c56 and PR-049-E-security-9fbb520f2c56 at `9fbb520f2c56a3d08d080b7297a5bed78e417f03` (`@0.0.4` types/host/toolset, coarse `list-tools`/`call`, `@1.0.0` generation block). G6 owns Implemented.
