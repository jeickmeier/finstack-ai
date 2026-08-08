# ADR-029: typed identifiers

## Status

Accepted

## Date

2026-08-08

## Accountable role

Core/runtime lead

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

This decision was accepted in the planning baseline and is recorded here so later
implementation pull requests cannot silently reinterpret it. Canonical summary
text lives in Architecture Specification §25; this standalone record captures
stable decision context, consequences, compatibility classification, security
linkage, and change-control metadata required by PR-004.

## Decision

Allocated runtime entity IDs use typed UUID values serialized as lowercase UUID strings and new values use UUIDv7; human-selected agent/component/capability/tool/bundle keys remain validated namespaced strings

## Consequences

IDs serialize portably; configuration keys remain validated namespaced strings.

## Rejected alternatives

Untyped string IDs for allocated runtime entities, or UUIDv4-only allocation without ordering benefits.

## Compatibility and schema-change classification

Identifier schema/serialization family; lowercase UUID strings and namespaced keys.

## Security classification

- References: SEC-INV-008; TM-12, TM-15, TM-19
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-KRN
- Affected Technical Design: Technical Design §5 (typed identifiers)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-006
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate
- Reconsideration source: [Technical Design §37](../../planning/03-finstack-ai-technical-design.md#37-technical-decision-status)

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

Exact Technical Design §37 gate: Only before PR-006 schema fixtures freeze.

Additionally requires a superseding ADR and primary-document reconciliation.

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Missing until mapped delivery work completes and evidence is verified
