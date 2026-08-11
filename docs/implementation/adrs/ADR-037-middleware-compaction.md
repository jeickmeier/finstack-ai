# ADR-037: middleware compaction

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

Model-context compaction is before_model middleware; it preserves canonical history and records only a versioned derived projection/checkpoint

## Consequences

Compaction cannot edit canonical history or protected content.

## Rejected alternatives

Mutating canonical conversation history for compaction, or inventing a new middleware stage for it.

## Compatibility and schema-change classification

Middleware/compaction contract via before_model; derived projection/checkpoint only.

## Security classification

- References: SEC-INV-013; TM-21
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-MW, FR-CTX
- Affected Technical Design: Technical Design §17.6 (middleware compaction on before_model)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-018, PR-023, PR-048, PR-056
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate
- Reconsideration source: [Technical Design §37](../../planning/03-finstack-ai-technical-design.md#37-technical-decision-status)

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

Exact Technical Design §37 gate: Before public preview only if PR-018/PR-056 conformance evidence proves the existing stage/outcome contract cannot preserve required semantics.

Additionally requires a superseding ADR and primary-document reconciliation.

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Partial through locally integrated PR-018 unique compaction ownership, protected source-ordered projection, tool-pair atomicity, evidence/checkpoint digests, child Model relation, native/WASM, and TM-21 evidence (`PR-018-E-integration-3fe0314c6434`) at `3fe0314c6434211e1c8f493f24401888f9609050`; PR-023/PR-048/PR-056 remain
