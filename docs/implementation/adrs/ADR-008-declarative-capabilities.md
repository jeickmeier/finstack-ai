# ADR-008: declarative capabilities

## Status

Accepted

## Date

2026-08-08

## Accountable role

Ecosystem lead

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

This decision was accepted in the planning baseline and is recorded here so later
implementation pull requests cannot silently reinterpret it. Canonical summary
text lives in Architecture Specification §25; this standalone record captures
stable decision context, consequences, compatibility classification, security
linkage, and change-control metadata required by PR-004.

## Decision

Capabilities are declarative composition, distinct from executable extensions

## Consequences

Capabilities compose instructions and references without becoming ambient authority.

## Rejected alternatives

Treating capabilities as executable extensions or ambient authority.

## Compatibility and schema-change classification

AgentSpec/capability schema family; strict reject-unknown for specs and locks.

## Security classification

- References: SEC-INV-001; TM-01
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-CAP, FR-SPEC
- Affected Technical Design: Technical Design §10 (declarative capabilities)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-012, PR-022, PR-032, PR-038, PR-048
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Partial — PR-022 declarative specs plus PR-038 JS catalog UX and fail-closed model activation on candidate `f1ff388563dfce92169c9fe6ae8c845fec86794b`. PR-048 remains.
