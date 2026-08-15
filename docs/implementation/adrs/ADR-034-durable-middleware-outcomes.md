# ADR-034: durable middleware outcomes

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

State-changing middleware outcomes are recorded by default; only explicitly recompute-safe outcomes may rerun during replay

## Consequences

Replay prefers recorded middleware outcomes unless declared recompute-safe.

## Rejected alternatives

Recomputing all middleware outcomes on replay without safety declarations.

## Compatibility and schema-change classification

Middleware outcome durability; recompute-safe outcomes require explicit declaration and fixtures.

## Security classification

- References: SEC-INV-013; TM-21
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: standalone recording of the accepted planning baseline; no new trust boundary, threat, or control is introduced by creating this ADR file.

## Affected requirements, design, and delivery

- Affected requirements: FR-MW, FR-DUR
- Affected Technical Design: Technical Design §17 (durable middleware outcomes)

- Architecture Specification §25 decision summary: [link](../../planning/02-finstack-ai-architecture-specification.md#25-architecture-decision-summary)
- Implementation Plan logical PR-004 and mapped delivery: PR-018, PR-048
- Engineering Standards public-contract and ADR-trigger rules apply where this decision defines a compatibility or architecture gate
- Reconsideration source: [Technical Design §37](../../planning/03-finstack-ai-technical-design.md#37-technical-decision-status)

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.

## Reconsideration conditions

Exact Technical Design §37 gate: PR-018 descriptor/conformance review.

Additionally requires a superseding ADR and primary-document reconciliation.

## Approval and implementation-evidence links

- Approval: accepted in the planning baseline (Architecture Specification §25); recorded as standalone under PR-004
- Implementation evidence: Partial through locally integrated PR-018 exact committed context/middleware guards, recorded output reconstruction, reuse/reconcile/recompute classification, and explicit non-repeatable uncertainty evidence (`PR-018-E-integration-3fe0314c6434`) at `3fe0314c6434211e1c8f493f24401888f9609050`; PR-048 recorded-outcome replay and G5-D-durable-beta-a9568bd869b5 close the mapped durability work. Verified
