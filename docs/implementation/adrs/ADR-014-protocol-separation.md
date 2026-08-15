# ADR-014: protocol separation

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

Remote client protocol and plugin protocol remain logically distinct

## Consequences

Protocol evolution and plugin ABI evolution do not couple accidentally.

## Rejected alternatives

Unifying remote client and plugin message vocabularies into one protocol.

## Compatibility and schema-change classification

Remote and WIT/protocol families remain distinct; shared framing is not shared vocabulary.

## Security classification

- References: TM-09
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-PLG
- Affected Technical Design: Technical Design §27–§28 (protocol separation)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-049–PR-054, PR-058
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of every affected primary planning document (ADR register change-control rules).

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Verified through PR-049–PR-054 experimental `@0.0.4` WIT worlds (`G6-D-plugin-alpha-018aaea9aa00`) plus locally integrated PR-058 remote and process families (`PR-058-E-integration-99ac0fb69952`) at `99ac0fb69952c9e6bfe8354e9d1f930ee061b4a0`. Worlds, remote session DTOs, and process handshake kinds stay pairwise distinct.
